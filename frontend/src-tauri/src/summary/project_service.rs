//! Generating a project's cross-meeting brief.
//!
//! Deliberately NOT routed through `SummaryService::process_transcript_background`.
//! That function renames the meeting from its summary's H1 as a side effect, and
//! writes `transcript_chunks` — neither of which a project has any business
//! doing. This is the much smaller thing: gather briefs, synthesize once, store.
//!
//! The map step makes **no LLM calls** for meetings that already have a summary,
//! which is the common case and why a whole project usually costs one call. A
//! meeting with no summary contributes its transcript directly to the synthesis
//! input rather than getting its own condense call: N extra calls would turn a
//! one-minute job into an hour, and would produce a second, invisible summary of
//! a meeting that already has a visible "generate summary" button of its own.

use chrono::Utc;
use reqwest::Client;
use serde_json::json;
use sqlx::SqlitePool;
use tauri::{AppHandle, Emitter, Runtime};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::api::chat_common::resolve_llm_config;
use crate::api::project_chat_context::{project_context_char_budget, render_summary_for_prompt};
use crate::database::models::{MeetingModel, ProjectModel};
use crate::database::repositories::{
    meeting::MeetingsRepository, project::ProjectsRepository,
    project_summary::ProjectSummariesRepository, summary::SummaryProcessesRepository,
};
use crate::summary::project_prompts::{
    build_batch_digest_system_prompt, build_batch_digest_user_prompt,
    build_synthesis_system_prompt, build_synthesis_user_prompt, meeting_block, uncovered_section,
};
use crate::summary::service::SummaryService;
use crate::summary::text_budget::{allocate_fair_shares, elide_middle};

/// Emitted at every stage boundary so a multi-minute job visibly moves. The DB
/// row remains the source of truth — an event lost to a webview reload must not
/// leave the UI thinking nothing is happening.
pub const PROJECT_SUMMARY_PROGRESS_EVENT: &str = "project-summary-progress";

/// Give up rather than loop forever when digests keep overflowing the budget.
const MAX_REDUCE_ROUNDS: usize = 3;

/// Namespace for the shared cancellation registry.
///
/// Project ids are `project-<uuid>` and meeting ids are `meeting-<uuid>`, so a
/// collision is already impossible — the prefix makes that deliberate rather
/// than lucky.
pub(crate) fn project_cancellation_key(project_id: &str) -> String {
    format!("project:{project_id}")
}

#[derive(Debug, Clone, serde::Serialize)]
struct ProgressPayload<'a> {
    project_id: &'a str,
    stage: &'a str,
    current: i64,
    total: i64,
}

/// Where a meeting's contribution to the brief came from. Stored per meeting in
/// `covered_meetings` so the UI can distinguish "not summarized yet" from
/// "summarized, and here is what it said".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BriefSource {
    Summary,
    Transcript,
    None,
}

impl BriefSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::Transcript => "transcript",
            Self::None => "none",
        }
    }
}

struct MeetingBrief {
    id: String,
    title: String,
    date: String,
    created_at: String,
    body: String,
    source: BriefSource,
    fingerprint: String,
}

fn meeting_date(m: &MeetingModel) -> String {
    m.created_at.0.format("%Y-%m-%d").to_string()
}

/// Kick off a brief. Returns immediately; the work runs on the async runtime.
///
/// The caller must already have claimed the job via
/// `ProjectSummariesRepository::try_begin`, so this cannot start a second run.
pub async fn generate_project_summary_background<R: Runtime>(
    app: AppHandle<R>,
    pool: SqlitePool,
    project_id: String,
    provider: String,
    model: String,
    language: Option<String>,
) {
    let started = std::time::Instant::now();
    let key = project_cancellation_key(&project_id);
    let token = SummaryService::register_cancellation_token(&key);

    let outcome = run(
        &app,
        &pool,
        &project_id,
        &provider,
        &model,
        language.as_deref(),
        &token,
    )
    .await;

    // Cleanup on every exit path, so a cancelled or failed run cannot leave a
    // token behind that a later run would then find and clobber.
    SummaryService::cleanup_cancellation_token(&key);

    match outcome {
        Ok(Some((markdown, covered, fingerprint, resolved_language))) => {
            let elapsed = started.elapsed().as_secs_f64();
            if let Err(e) = ProjectSummariesRepository::update_completed(
                &pool,
                &project_id,
                &json!({ "markdown": markdown }),
                &covered,
                &fingerprint,
                Some(resolved_language.as_str()),
                elapsed,
            )
            .await
            {
                error!("Failed to store project brief for {}: {}", project_id, e);
            } else {
                info!(
                    "Project brief for {} completed in {:.1}s",
                    project_id, elapsed
                );
            }
        }
        // Cancelled: the row was already reconciled below.
        Ok(None) => {}
        Err(e) => {
            error!("Project brief for {} failed: {}", project_id, e);
            let _ = ProjectSummariesRepository::update_failed(&pool, &project_id, &e).await;
        }
    }

    emit_progress(&app, &project_id, "done", 0, 0);
}

fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    project_id: &str,
    stage: &str,
    current: i64,
    total: i64,
) {
    // Best-effort: a dropped event just means the poller reports it a moment later.
    let _ = app.emit(
        PROJECT_SUMMARY_PROGRESS_EVENT,
        ProgressPayload {
            project_id,
            stage,
            current,
            total,
        },
    );
}

async fn set_stage<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    project_id: &str,
    stage: &str,
    current: i64,
    total: i64,
) {
    if let Err(e) =
        ProjectSummariesRepository::set_stage(pool, project_id, stage, current, total).await
    {
        warn!("Failed to record project brief stage: {}", e);
    }
    emit_progress(app, project_id, stage, current, total);
}

type RunOutcome = Option<(String, serde_json::Value, String, String)>;

#[allow(clippy::too_many_arguments)]
async fn run<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    project_id: &str,
    provider: &str,
    model: &str,
    language: Option<&str>,
    token: &CancellationToken,
) -> Result<RunOutcome, String> {
    set_stage(app, pool, project_id, "collecting", 0, 0).await;

    let project = ProjectsRepository::get(pool, project_id)
        .await
        .map_err(|e| format!("Failed to load project: {}", e))?
        .ok_or_else(|| "This project no longer exists.".to_string())?;

    // Snapshot semantics: the meeting set is read once, here. A job whose input
    // shifts under it would produce a brief matching no consistent state; the
    // coverage diff on the next read is what surfaces later changes.
    let mut meetings = ProjectsRepository::list_meetings(pool, project_id)
        .await
        .map_err(|e| format!("Failed to load project meetings: {}", e))?;
    meetings.reverse(); // oldest first

    if meetings.is_empty() {
        return Err("This project has no meetings yet.".to_string());
    }

    let config = resolve_llm_config(app, pool, provider).await?;
    let budget =
        project_context_char_budget(&config.provider_enum, model, config.ollama_endpoint.as_deref())
            .await;

    let briefs = collect_briefs(pool, &meetings, budget).await?;
    let covered_count = briefs
        .iter()
        .filter(|b| b.source != BriefSource::None)
        .count();
    if covered_count == 0 {
        return Err(
            "No meeting in this project has a summary or a transcript yet. Generate a meeting \
             summary first, then regenerate this brief."
                .to_string(),
        );
    }

    if token.is_cancelled() {
        let _ = ProjectSummariesRepository::update_cancelled(pool, project_id).await;
        return Ok(None);
    }

    let language = language
        .and_then(crate::summary::processor::language_name_from_code)
        .unwrap_or("English")
        .to_string();

    let client = Client::new();
    let markdown = synthesize(
        app,
        pool,
        &client,
        &config,
        model,
        &project,
        &briefs,
        &language,
        budget,
        token,
    )
    .await?;

    if token.is_cancelled() {
        let _ = ProjectSummariesRepository::update_cancelled(pool, project_id).await;
        return Ok(None);
    }

    let markdown = crate::summary::service::strip_title_if_present(&markdown);
    let uncovered: Vec<(String, String)> = briefs
        .iter()
        .filter(|b| b.source == BriefSource::None)
        .map(|b| (b.title.clone(), b.date.clone()))
        .collect();
    let markdown = format!("{}{}", markdown, uncovered_section(&uncovered));

    let covered = json!(briefs
        .iter()
        .map(|b| json!({
            "id": b.id,
            "title": b.title,
            "createdAt": b.created_at,
            "source": b.source.as_str(),
            "fingerprint": b.fingerprint,
        }))
        .collect::<Vec<_>>());

    let fingerprint = coverage_fingerprint(&briefs);
    Ok(Some((markdown, covered, fingerprint, language)))
}

/// One fingerprint over the whole covered set, so "has anything changed at all"
/// is a single string compare on read.
fn coverage_fingerprint(briefs: &[MeetingBrief]) -> String {
    let joined = briefs
        .iter()
        .map(|b| format!("{}:{}", b.id, b.fingerprint))
        .collect::<Vec<_>>()
        .join("\n");
    crate::summary::service::stable_text_fingerprint(&joined)
}

/// Gather each meeting's contribution, in two queries.
async fn collect_briefs(
    pool: &SqlitePool,
    meetings: &[MeetingModel],
    budget: usize,
) -> Result<Vec<MeetingBrief>, String> {
    let ids: Vec<String> = meetings.iter().map(|m| m.id.clone()).collect();

    let summaries = SummaryProcessesRepository::get_results_for_meetings(pool, &ids)
        .await
        .map_err(|e| format!("Failed to load meeting summaries: {}", e))?;

    // Only meetings without a usable summary need their transcript read, so the
    // common case (everything summarized) never touches the transcripts table.
    let needs_transcript: Vec<String> = meetings
        .iter()
        .filter(|m| {
            summaries
                .get(&m.id)
                .and_then(|raw| render_summary_for_prompt(raw))
                .is_none()
        })
        .map(|m| m.id.clone())
        .collect();

    let transcripts = if needs_transcript.is_empty() {
        Default::default()
    } else {
        MeetingsRepository::get_transcripts_for_meetings(pool, &needs_transcript)
            .await
            .map_err(|e| format!("Failed to load meeting transcripts: {}", e))?
    };

    // Transcripts stand in for missing summaries, but they are far longer, so
    // they share a slice of the budget rather than taking it all.
    let raw_transcripts: Vec<String> = meetings
        .iter()
        .map(|m| {
            transcripts
                .get(&m.id)
                .map(|segs| {
                    segs.iter()
                        .filter(|s| !s.transcript.trim().is_empty())
                        .map(|s| s.transcript.trim())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        })
        .collect();
    let transcript_lengths: Vec<usize> = raw_transcripts
        .iter()
        .map(|t| t.chars().count())
        .collect();
    let transcript_shares = allocate_fair_shares(&transcript_lengths, budget / 2);

    Ok(meetings
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let date = meeting_date(m);
            let created_at = m.created_at.0.to_rfc3339();

            if let Some(body) = summaries.get(&m.id).and_then(|raw| render_summary_for_prompt(raw))
            {
                let fingerprint = crate::summary::service::stable_text_fingerprint(&body);
                return MeetingBrief {
                    id: m.id.clone(),
                    title: m.title.clone(),
                    date,
                    created_at,
                    body,
                    source: BriefSource::Summary,
                    fingerprint,
                };
            }

            if !raw_transcripts[i].is_empty() && transcript_shares[i] > 0 {
                let body = elide_middle(&raw_transcripts[i], transcript_shares[i]);
                let fingerprint = crate::summary::service::stable_text_fingerprint(&body);
                return MeetingBrief {
                    id: m.id.clone(),
                    title: m.title.clone(),
                    date,
                    created_at,
                    body: format!(
                        "(No summary exists for this meeting; the raw transcript follows.)\n{body}"
                    ),
                    source: BriefSource::Transcript,
                    fingerprint,
                };
            }

            MeetingBrief {
                id: m.id.clone(),
                title: m.title.clone(),
                date,
                created_at,
                body: String::new(),
                source: BriefSource::None,
                fingerprint: String::new(),
            }
        })
        .collect())
}

/// One synthesis call when the briefs fit; otherwise condense in chronological
/// batches first and synthesize over the digests.
#[allow(clippy::too_many_arguments)]
async fn synthesize<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    client: &Client,
    config: &crate::api::chat_common::ResolvedLlmConfig,
    model: &str,
    project: &ProjectModel,
    briefs: &[MeetingBrief],
    language: &str,
    budget: usize,
    token: &CancellationToken,
) -> Result<String, String> {
    let covered: Vec<&MeetingBrief> = briefs
        .iter()
        .filter(|b| b.source != BriefSource::None)
        .collect();
    let uncovered_count = briefs.len() - covered.len();

    let first_date = covered.first().map(|b| b.date.clone()).unwrap_or_default();
    let last_date = covered.last().map(|b| b.date.clone()).unwrap_or_default();

    let mut blocks: Vec<String> = covered
        .iter()
        .map(|b| meeting_block(&b.title, &b.date, &b.body))
        .collect();

    // Reduce in batches until the whole set fits one call.
    let mut round = 0;
    while blocks.join("\n\n").chars().count() > budget && round < MAX_REDUCE_ROUNDS {
        round += 1;
        let batches = batch_by_budget(&blocks, budget);
        if batches.len() <= 1 {
            break;
        }
        set_stage(app, pool, &project.id, "reducing", 0, batches.len() as i64).await;

        let mut digests = Vec::with_capacity(batches.len());
        let total = batches.len();
        for (i, batch) in batches.into_iter().enumerate() {
            if token.is_cancelled() {
                return Err("cancelled".to_string());
            }
            set_stage(app, pool, &project.id, "reducing", (i + 1) as i64, total as i64).await;

            let system = build_batch_digest_system_prompt(language);
            let user = build_batch_digest_user_prompt(
                &project.name,
                i + 1,
                total,
                total,
                &first_date,
                &last_date,
                &batch.join("\n\n"),
            );
            match call_llm(client, config, model, &system, &user, token).await {
                Ok(text) => digests.push(text),
                Err(e) => {
                    // One failed slice must not sink the brief; the rest still
                    // describe the project. All of them failing is fatal below.
                    warn!("Project brief batch {} of {} failed: {}", i + 1, total, e);
                }
            }
        }
        if digests.is_empty() {
            return Err("Every part of the project failed to condense.".to_string());
        }
        blocks = digests;
    }

    if token.is_cancelled() {
        return Err("cancelled".to_string());
    }
    set_stage(app, pool, &project.id, "synthesizing", 0, 0).await;

    // After MAX_REDUCE_ROUNDS a slightly lossy brief beats no brief.
    let joined = elide_middle(&blocks.join("\n\n"), budget);

    let system = build_synthesis_system_prompt(
        &project.name,
        project.description.as_deref(),
        project.context_notes.as_deref(),
        covered.len(),
        &first_date,
        &last_date,
        uncovered_count,
        language,
    );
    let user = build_synthesis_user_prompt(&joined);
    call_llm(client, config, model, &system, &user, token).await
}

async fn call_llm(
    client: &Client,
    config: &crate::api::chat_common::ResolvedLlmConfig,
    model: &str,
    system: &str,
    user: &str,
    token: &CancellationToken,
) -> Result<String, String> {
    let raw = crate::summary::processor::generate_summary_with_retry(
        client,
        &config.provider_enum,
        model,
        &config.api_key,
        system,
        user,
        &[],
        config.ollama_endpoint.as_deref(),
        config.custom_openai_endpoint.as_deref(),
        config.lmstudio_endpoint.as_deref(),
        config.custom_openai_max_tokens,
        config.custom_openai_temperature,
        config.custom_openai_top_p,
        config.app_data_dir.as_ref(),
        Some(token),
    )
    .await?;
    Ok(crate::summary::processor::clean_llm_markdown_output(&raw))
}

/// Split blocks into chronologically contiguous batches that each fit `budget`.
/// Order is preserved: a batch that jumped around in time would break the
/// "later supersedes earlier" reading the prompts rely on.
fn batch_by_budget(blocks: &[String], budget: usize) -> Vec<Vec<String>> {
    let mut batches: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut used = 0usize;

    for block in blocks {
        let len = block.chars().count();
        if !current.is_empty() && used + len > budget {
            batches.push(std::mem::take(&mut current));
            used = 0;
        }
        used += len;
        current.push(block.clone());
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief(id: &str, source: BriefSource) -> MeetingBrief {
        MeetingBrief {
            id: id.into(),
            title: format!("Meeting {id}"),
            date: "2026-03-04".into(),
            created_at: "2026-03-04T00:00:00Z".into(),
            body: "body".into(),
            source,
            fingerprint: format!("fp-{id}"),
        }
    }

    #[test]
    fn batching_preserves_chronological_order() {
        let blocks: Vec<String> = (0..6).map(|i| format!("{}", i).repeat(100)).collect();
        let batches = batch_by_budget(&blocks, 250);

        assert!(batches.len() > 1, "a tight budget must split");
        let flattened: Vec<&String> = batches.iter().flatten().collect();
        assert_eq!(flattened.len(), 6, "no block is dropped");
        for (i, b) in flattened.iter().enumerate() {
            assert!(b.starts_with(&i.to_string()), "order preserved");
        }
    }

    #[test]
    fn a_single_oversized_block_still_gets_its_own_batch() {
        let blocks = vec!["x".repeat(1_000)];
        let batches = batch_by_budget(&blocks, 10);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 1);
    }

    #[test]
    fn coverage_fingerprint_tracks_content_not_order_of_unrelated_fields() {
        let a = vec![brief("m1", BriefSource::Summary), brief("m2", BriefSource::Summary)];
        let same = vec![brief("m1", BriefSource::Summary), brief("m2", BriefSource::Summary)];
        assert_eq!(coverage_fingerprint(&a), coverage_fingerprint(&same));

        let mut changed = a;
        changed[1].fingerprint = "fp-different".into();
        assert_ne!(coverage_fingerprint(&changed), coverage_fingerprint(&same));
    }

    #[test]
    fn cancellation_keys_cannot_collide_with_meeting_keys() {
        let key = project_cancellation_key("project-abc");
        assert_eq!(key, "project:project-abc");
        assert_ne!(key, "meeting-abc");
    }

    #[test]
    fn brief_source_round_trips_to_its_stored_string() {
        assert_eq!(BriefSource::Summary.as_str(), "summary");
        assert_eq!(BriefSource::Transcript.as_str(), "transcript");
        assert_eq!(BriefSource::None.as_str(), "none");
    }
}
