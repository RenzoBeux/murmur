//! Building the context a project chat answers from.
//!
//! A meeting chat has one transcript and a simple question: does it fit? A
//! project has N of them, and the interesting decisions are all about what to
//! leave out and how to say so.
//!
//! Three rules shape everything here:
//!
//! 1. **Every meeting is always named**, even when nothing of it is included. A
//!    meeting the model cannot see at all is one it will confidently claim does
//!    not exist.
//! 2. **A meeting is whole or absent.** Half a transcript is what produces
//!    confident wrong answers ("they never decided" — because the decision was
//!    in the half that was cut). Budget is spent newest-meeting-first, and a
//!    meeting that does not fit is represented by its summary alone.
//! 3. **What was left out is disclosed**, in the prompt and (via
//!    [`ProjectChatContextInfo`]) in the UI. Partial coverage is fine; silent
//!    partial coverage is not.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;

use crate::api::chat_common::{
    speaker_display_name, ChatGrounding, DEFAULT_MAX_TRANSCRIPT_CHARS, RESERVED_TOKENS,
};
use crate::database::models::{MeetingModel, ProjectModel, Transcript};
use crate::database::repositories::{
    meeting::MeetingsRepository, summary::SummaryProcessesRepository,
};
use crate::summary::llm_client::LLMProvider;
use crate::summary::text_budget::{allocate_fair_shares, chars_for_tokens, elide_middle};

/// Context window, in tokens, assumed for a cloud provider/model.
///
/// `transcript_char_budget` answers `usize::MAX` for every cloud provider, which
/// is survivable for one meeting and a live grenade for twelve: on a 200k model
/// it re-bills a half-million characters on every turn, and on a 128k one every
/// message is an HTTP 400. So project context gets real numbers.
///
/// Unknown models fall to a deliberately small figure — being wrong in the
/// direction of "sent less than we could have" costs some recall, while being
/// wrong the other way costs the whole answer.
fn cloud_context_tokens(provider: &LLMProvider, model: &str) -> usize {
    let m = model.to_lowercase();
    match provider {
        LLMProvider::Claude => 200_000,
        LLMProvider::OpenAI | LLMProvider::ChatGptSubscription => {
            if m.contains("gpt-4.1") || m.contains("gpt-5") {
                1_000_000
            } else if m.contains("gpt-4o") || m.contains("o1") || m.contains("o3") {
                128_000
            } else {
                32_000
            }
        }
        LLMProvider::Groq => {
            if m.contains("llama-3.3") || m.contains("compound") {
                128_000
            } else {
                32_000
            }
        }
        // OpenRouter and custom endpoints proxy arbitrary models, so there is
        // nothing reliable to look up.
        _ => 32_000,
    }
}

/// Tokens held back for the system prompt boilerplate, the meeting inventory,
/// the chat history and the model's answer.
///
/// Larger than the meeting chat's 2,000: the inventory and per-meeting summaries
/// are themselves substantial before a single transcript line is added.
const PROJECT_RESERVED_TOKENS: usize = 8_000;

/// Share of the budget the summaries may take before transcripts get the rest.
/// Summaries are the floor of the feature — every meeting contributes one — so
/// they are served first, but they must not be able to crowd transcripts out
/// entirely.
const SUMMARY_BUDGET_FRACTION: f64 = 0.45;

/// How much of the context this provider/model can be given for a whole project.
pub(crate) async fn project_context_char_budget(
    provider: &LLMProvider,
    model: &str,
    ollama_endpoint: Option<&str>,
) -> usize {
    let context_tokens = match provider {
        LLMProvider::Ollama => {
            match crate::ollama::metadata::METADATA_CACHE
                .get_or_fetch(model, ollama_endpoint)
                .await
            {
                Ok(meta) => meta.context_size,
                Err(_) => return DEFAULT_MAX_TRANSCRIPT_CHARS,
            }
        }
        LLMProvider::BuiltInAI => {
            match crate::summary::summary_engine::models::get_model_by_name(model) {
                Some(m) => m.context_size as usize,
                None => return DEFAULT_MAX_TRANSCRIPT_CHARS,
            }
        }
        // LM Studio does not advertise its context size.
        LLMProvider::LMStudio => return DEFAULT_MAX_TRANSCRIPT_CHARS,
        cloud => cloud_context_tokens(cloud, model),
    };

    // Local models get the smaller of the two reserves: on a 4k context, holding
    // back 8k tokens would leave nothing at all.
    let reserve = PROJECT_RESERVED_TOKENS.min(context_tokens / 4).max(RESERVED_TOKENS);
    chars_for_tokens(context_tokens.saturating_sub(reserve).max(1_000))
}

/// How a single meeting is represented in the assembled context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeetingDetail {
    /// Summary and full transcript.
    Full,
    /// Summary only — the transcript did not fit the budget.
    SummaryOnly,
    /// Named in the inventory, but there is nothing to include.
    Empty,
}

/// What the assembled context actually contains, recorded on the assistant
/// message so the UI can show it and so a past answer stays explicable.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProjectChatContextInfo {
    pub meetings_total: usize,
    pub meetings_with_summary: usize,
    /// Meetings whose full transcript is in context.
    pub meetings_with_transcript: usize,
    /// True when any summary or transcript had to be trimmed to fit.
    pub truncated: bool,
}

pub(crate) struct ProjectContext {
    pub text: String,
    pub info: ProjectChatContextInfo,
}

/// One meeting's raw material, before any budgeting.
struct MeetingSource {
    title: String,
    date: String,
    summary: Option<String>,
    transcript: String,
    segments: usize,
}

/// Render a meeting's stored summary blob as plain markdown for a prompt.
///
/// Prefers the canonical English cache when one exists: per-meeting summaries
/// may have been translated, and feeding a synthesis step a mix of languages
/// produces a mix back. Falls back to the stored markdown.
///
/// Deliberately not `export::markdown::build_summary_markdown` — that wraps its
/// output in YAML frontmatter and a duplicate `# title` for the export archive,
/// which is noise in a prompt.
pub(crate) fn render_summary_for_prompt(raw: &str) -> Option<String> {
    let value = crate::mcp::tools::parse_summary(raw)?;

    if let Some(cached) = value
        .get("english_cache")
        .and_then(|c| c.get("markdown"))
        .and_then(|m| m.as_str())
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        return Some(cached.to_string());
    }

    let rendered = crate::mcp::tools::render_summary(&value);
    let rendered = rendered.trim();
    // render_summary returns this sentinel rather than an empty string.
    if rendered.is_empty() || rendered == "(Summary is empty.)" {
        return None;
    }
    Some(rendered.to_string())
}

fn meeting_date(m: &MeetingModel) -> String {
    // MeetingModel::created_at is the DateTimeUtc newtype, not a bare DateTime.
    m.created_at.0.format("%Y-%m-%d").to_string()
}

/// Load every meeting's summary and transcript in two queries, then assemble the
/// context that fits `budget_chars`.
pub(crate) async fn build_project_context(
    pool: &SqlitePool,
    project: &ProjectModel,
    meetings: &[MeetingModel],
    budget_chars: usize,
) -> Result<ProjectContext, String> {
    if meetings.is_empty() {
        return Ok(ProjectContext {
            text: format!(
                "Project: {}\n{}\n\n(No meetings are filed under this project yet.)\n",
                project.name,
                project
                    .description
                    .as_deref()
                    .map(|d| format!("Description: {d}"))
                    .unwrap_or_default()
            ),
            info: ProjectChatContextInfo::default(),
        });
    }

    let ids: Vec<String> = meetings.iter().map(|m| m.id.clone()).collect();

    let summaries = SummaryProcessesRepository::get_results_for_meetings(pool, &ids)
        .await
        .map_err(|e| format!("Failed to load project summaries: {}", e))?;
    let transcripts = MeetingsRepository::get_transcripts_for_meetings(pool, &ids)
        .await
        .map_err(|e| format!("Failed to load project transcripts: {}", e))?;

    let sources: Vec<MeetingSource> = meetings
        .iter()
        .map(|m| {
            let segs = transcripts.get(&m.id).map(Vec::as_slice).unwrap_or(&[]);
            MeetingSource {
                title: m.title.clone(),
                date: meeting_date(m),
                summary: summaries.get(&m.id).and_then(|raw| render_summary_for_prompt(raw)),
                transcript: transcript_lines(segs),
                segments: segs.len(),
            }
        })
        .collect();

    Ok(assemble(project, &sources, budget_chars))
}

fn transcript_lines(segments: &[Transcript]) -> String {
    segments
        .iter()
        .filter(|s| !s.transcript.trim().is_empty())
        .map(|s| match s.speaker.as_deref().filter(|t| !t.trim().is_empty()) {
            Some(tag) => format!("{}: {}", speaker_display_name(tag.trim()), s.transcript.trim()),
            None => s.transcript.trim().to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The pure half: given each meeting's material and a budget, decide what goes
/// in. Separated from the queries so it can be tested without a database.
fn assemble(
    project: &ProjectModel,
    sources: &[MeetingSource],
    budget_chars: usize,
) -> ProjectContext {
    let mut truncated = false;

    // --- Summaries first: every meeting contributes one if it has one. -------
    let summary_budget = (budget_chars as f64 * SUMMARY_BUDGET_FRACTION) as usize;
    let summary_lengths: Vec<usize> = sources
        .iter()
        .map(|s| s.summary.as_deref().map(|t| t.chars().count()).unwrap_or(0))
        .collect();
    let summary_shares = allocate_fair_shares(&summary_lengths, summary_budget);

    let mut summaries: Vec<Option<String>> = Vec::with_capacity(sources.len());
    let mut summary_used = 0usize;
    for (i, src) in sources.iter().enumerate() {
        match &src.summary {
            Some(text) => {
                let fitted = elide_middle(text, summary_shares[i]);
                if fitted.chars().count() < text.chars().count() {
                    truncated = true;
                }
                summary_used += fitted.chars().count();
                summaries.push(Some(fitted));
            }
            None => summaries.push(None),
        }
    }

    // --- Transcripts: newest meeting first, whole meetings only. -------------
    // `sources` is oldest-first (it mirrors the brief's chronology), so walk it
    // backwards: recent meetings are what questions are usually about, and a
    // half-included meeting is worse than an excluded one.
    let mut transcript_budget = budget_chars.saturating_sub(summary_used);
    let mut detail = vec![MeetingDetail::Empty; sources.len()];
    let mut included_transcripts: Vec<Option<String>> = vec![None; sources.len()];

    for i in (0..sources.len()).rev() {
        let src = &sources[i];
        if src.transcript.is_empty() {
            continue;
        }
        let len = src.transcript.chars().count();
        if len <= transcript_budget {
            transcript_budget -= len;
            included_transcripts[i] = Some(src.transcript.clone());
        }
    }

    for (i, src) in sources.iter().enumerate() {
        detail[i] = if included_transcripts[i].is_some() {
            MeetingDetail::Full
        } else if summaries[i].is_some() {
            MeetingDetail::SummaryOnly
        } else if src.segments > 0 {
            // Has a transcript that did not fit, and no summary to stand in.
            truncated = true;
            MeetingDetail::SummaryOnly
        } else {
            MeetingDetail::Empty
        };
    }

    // --- Render ------------------------------------------------------------
    let mut out = String::new();
    out.push_str(&format!("Project: {}\n", project.name));
    if let Some(desc) = project.description.as_deref().filter(|d| !d.trim().is_empty()) {
        out.push_str(&format!("Description: {}\n", desc.trim()));
    }
    out.push_str(&format!("Meetings in this project: {}\n\n", sources.len()));

    out.push_str("--- MEETINGS IN THIS PROJECT ---\n");
    for (i, src) in sources.iter().enumerate() {
        let availability = match detail[i] {
            MeetingDetail::Full => "full transcript and summary available below",
            MeetingDetail::SummaryOnly if summaries[i].is_some() => {
                "SUMMARY ONLY - its full transcript is not included"
            }
            MeetingDetail::SummaryOnly => "no summary, and its transcript is not included",
            MeetingDetail::Empty => "no summary and no transcript recorded",
        };
        out.push_str(&format!(
            "{}. \"{}\" ({}) - {}\n",
            i + 1,
            src.title,
            src.date,
            availability
        ));
    }
    out.push_str("--- END MEETINGS ---\n\n");

    for (i, src) in sources.iter().enumerate() {
        out.push_str(&format!(
            "===== MEETING {}: \"{}\" ({}) =====\n",
            i + 1,
            src.title,
            src.date
        ));
        match &summaries[i] {
            Some(text) => {
                out.push_str("--- SUMMARY ---\n");
                out.push_str(text);
                out.push_str("\n--- END SUMMARY ---\n");
            }
            None => out.push_str("(No summary has been generated for this meeting.)\n"),
        }
        match &included_transcripts[i] {
            Some(text) => {
                out.push_str("--- TRANSCRIPT ---\n");
                out.push_str(text);
                out.push_str("\n--- END TRANSCRIPT ---\n");
            }
            None if src.segments > 0 => out.push_str(
                "(This meeting's full transcript is not included in this conversation.)\n",
            ),
            None => out.push_str("(No transcript was recorded for this meeting.)\n"),
        }
        out.push('\n');
    }

    let info = ProjectChatContextInfo {
        meetings_total: sources.len(),
        meetings_with_summary: summaries.iter().filter(|s| s.is_some()).count(),
        meetings_with_transcript: included_transcripts.iter().filter(|t| t.is_some()).count(),
        truncated,
    };

    ProjectContext { text: out, info }
}

/// The project chat's system prompt.
///
/// Mirrors `chat_api::build_system_prompt`'s two-axis shape — what counts as a
/// source, then what to do when the sources fall short — and shares its
/// speaker-attribution rules, but adds the two rules a single-meeting prompt has
/// no need for: attribute every claim to a meeting, and do not treat a
/// summary-only meeting as quotable.
pub(crate) fn build_project_system_prompt(
    context: &str,
    info: &ProjectChatContextInfo,
    grounding: ChatGrounding,
) -> String {
    let mut p = String::new();
    p.push_str(
        "You are a helpful assistant answering questions about a project — a set of related \
         recorded meetings.\n",
    );

    match grounding {
        ChatGrounding::TranscriptOnly => p.push_str(
            "Ground every answer strictly in the meeting summaries and transcripts below. \
             Quote only verbatim text that actually appears in them. ",
        ),
        _ => p.push_str(
            "The meeting summaries and transcripts below are your primary source, and you should \
             always prefer them when they cover the question. Quote only verbatim text that \
             actually appears in them. ",
        ),
    }

    match grounding {
        ChatGrounding::TranscriptOnly => p.push_str(
            "If the answer is not in these meetings, say you cannot find it rather than guessing. ",
        ),
        ChatGrounding::GeneralKnowledge => p.push_str(
            "When the answer is not in these meetings, say so plainly first (for example \
             \"That wasn't discussed in any of these meetings\") and then answer from your own \
             general knowledge, keeping the two clearly separated. Never present outside \
             knowledge as something that was said in a meeting, and never attribute it to a \
             speaker. If you are unsure of a general fact, say so rather than inventing it. ",
        ),
        ChatGrounding::WebSearch => p.push_str(
            "When the answer is not in these meetings, say so plainly first (for example \
             \"That wasn't discussed in any of these meetings\") and then answer from the web or \
             your own general knowledge, keeping the two clearly separated. Search the web when \
             these meetings do not cover the question, when the answer depends on current \
             information, or when the user asks what a term, product or company mentioned in \
             passing actually is. Do not search for questions the meetings already answer. Never \
             present outside knowledge as something that was said in a meeting, and never \
             attribute it to a speaker. ",
        ),
    }

    p.push_str(
        "Say which meeting each claim comes from, by title and date. When meetings disagree, or \
         when a decision changed over time, say so and give the order — never silently merge them \
         into a single position. The meetings are listed oldest first, so later meetings \
         supersede earlier ones. ",
    );

    if info.meetings_with_transcript < info.meetings_total {
        p.push_str(
            "Some meetings below are marked SUMMARY ONLY: their full transcripts are not in this \
             conversation. Do not quote those meetings verbatim, and if a question needs detail \
             they do not carry, say that the full transcript was not available rather than \
             guessing. ",
        );
    }

    p.push_str(crate::api::chat_common::SPEAKER_LABEL_RULES);
    p.push_str(context);
    p
}

/// Per-meeting availability for the UI's "what can this chat see" notice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeetingContextEntry {
    #[serde(rename = "meetingId")]
    pub meeting_id: String,
    pub title: String,
    #[serde(rename = "hasSummary")]
    pub has_summary: bool,
    #[serde(rename = "hasTranscript")]
    pub has_transcript: bool,
}

/// What the project's meetings currently offer, for the chat notice and the
/// brief's coverage display. One query each, never per meeting.
pub(crate) async fn project_meeting_availability(
    pool: &SqlitePool,
    meetings: &[MeetingModel],
) -> Result<Vec<ProjectMeetingContextEntry>, String> {
    if meetings.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = meetings.iter().map(|m| m.id.clone()).collect();

    let summaries = SummaryProcessesRepository::get_results_for_meetings(pool, &ids)
        .await
        .map_err(|e| format!("Failed to load summaries: {}", e))?;
    let sizes = MeetingsRepository::get_transcript_sizes(pool, &ids)
        .await
        .map_err(|e| format!("Failed to load transcript sizes: {}", e))?;

    Ok(meetings
        .iter()
        .map(|m| ProjectMeetingContextEntry {
            meeting_id: m.id.clone(),
            title: m.title.clone(),
            // "Has a summary" means one that actually renders, not merely a row:
            // a blob holding only empty BlockNote JSON is not a summary.
            has_summary: summaries
                .get(&m.id)
                .and_then(|raw| render_summary_for_prompt(raw))
                .is_some(),
            has_transcript: sizes.get(&m.id).map(|(_, segs)| *segs > 0).unwrap_or(false),
        })
        .collect())
}

/// Unused today, but kept beside the budget it belongs to: the char sizes that
/// decide whether the whole project fits, without loading any of it.
#[allow(dead_code)]
pub(crate) async fn project_transcript_sizes(
    pool: &SqlitePool,
    meetings: &[MeetingModel],
) -> Result<HashMap<String, (i64, i64)>, String> {
    let ids: Vec<String> = meetings.iter().map(|m| m.id.clone()).collect();
    MeetingsRepository::get_transcript_sizes(pool, &ids)
        .await
        .map_err(|e| format!("Failed to load transcript sizes: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn project() -> ProjectModel {
        ProjectModel {
            id: "project-1".into(),
            name: "Client X".into(),
            description: Some("Rollout work".into()),
            color: None,
            created_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    fn src(title: &str, summary: Option<&str>, transcript_len: usize) -> MeetingSource {
        MeetingSource {
            title: title.into(),
            date: "2026-03-04".into(),
            summary: summary.map(str::to_string),
            transcript: "x".repeat(transcript_len),
            segments: if transcript_len > 0 { 1 } else { 0 },
        }
    }

    #[test]
    fn every_meeting_is_named_even_with_nothing_to_include() {
        let sources = vec![src("Kickoff", None, 0), src("Retro", None, 0)];
        let ctx = assemble(&project(), &sources, 10_000);

        assert!(ctx.text.contains("\"Kickoff\""));
        assert!(ctx.text.contains("\"Retro\""));
        assert!(ctx.text.contains("no summary and no transcript recorded"));
        assert_eq!(ctx.info.meetings_total, 2);
        assert_eq!(ctx.info.meetings_with_summary, 0);
        assert_eq!(ctx.info.meetings_with_transcript, 0);
    }

    #[test]
    fn small_project_gets_every_transcript_verbatim() {
        let sources = vec![
            src("Kickoff", Some("decided A"), 500),
            src("Sync", Some("decided B"), 500),
        ];
        let ctx = assemble(&project(), &sources, 100_000);

        assert_eq!(ctx.info.meetings_with_transcript, 2);
        assert!(!ctx.info.truncated);
        assert_eq!(ctx.text.matches("--- TRANSCRIPT ---").count(), 2);
    }

    /// The key anti-hallucination property: a meeting is whole or absent, and
    /// the newest ones win the budget.
    #[test]
    fn tight_budget_drops_whole_meetings_newest_first() {
        let sources = vec![
            src("Oldest", Some("s1"), 4_000),
            src("Middle", Some("s2"), 4_000),
            src("Newest", Some("s3"), 4_000),
        ];
        // Enough for two transcripts, not three.
        let ctx = assemble(&project(), &sources, 9_000);

        assert_eq!(ctx.info.meetings_with_transcript, 2);
        assert_eq!(ctx.info.meetings_total, 3);
        // The newest meetings are the ones that made it in; the oldest is the
        // one that got dropped. `blocks[0]` is the header + inventory.
        let blocks: Vec<&str> = ctx.text.split("===== MEETING ").collect();
        assert_eq!(blocks.len(), 4);
        assert!(!blocks[1].contains("--- TRANSCRIPT ---"), "oldest dropped");
        assert!(blocks[1].contains("not included in this conversation"));
        assert!(blocks[2].contains("--- TRANSCRIPT ---"));
        assert!(blocks[3].contains("--- TRANSCRIPT ---"), "newest kept");
        // And the inventory says so up front.
        assert!(ctx.text.contains("SUMMARY ONLY"));
    }

    #[test]
    fn no_meeting_is_ever_partially_included() {
        let sources = vec![src("Only", Some("s"), 50_000)];
        // Too small for the transcript, big enough for the summary.
        let ctx = assemble(&project(), &sources, 1_000);
        assert_eq!(ctx.info.meetings_with_transcript, 0);
        assert!(!ctx.text.contains("--- TRANSCRIPT ---"));
    }

    #[test]
    fn one_huge_summary_cannot_starve_the_others() {
        let huge = "h".repeat(40_000);
        let sources = vec![
            src("Huge", Some(&huge), 0),
            src("Small A", Some("short a"), 0),
            src("Small B", Some("short b"), 0),
        ];
        let ctx = assemble(&project(), &sources, 10_000);

        assert_eq!(ctx.info.meetings_with_summary, 3);
        assert!(ctx.text.contains("short a"));
        assert!(ctx.text.contains("short b"));
        assert!(ctx.info.truncated, "the huge summary was elided to fit");
    }

    #[test]
    fn empty_project_says_so_without_erroring() {
        let ctx = assemble(&project(), &[], 10_000);
        assert_eq!(ctx.info.meetings_total, 0);
    }

    #[test]
    fn system_prompt_warns_only_when_coverage_is_partial() {
        let full = ProjectChatContextInfo {
            meetings_total: 2,
            meetings_with_summary: 2,
            meetings_with_transcript: 2,
            truncated: false,
        };
        let partial = ProjectChatContextInfo {
            meetings_with_transcript: 1,
            ..full.clone()
        };

        let p = build_project_system_prompt("CTX", &full, ChatGrounding::TranscriptOnly);
        assert!(!p.contains("SUMMARY ONLY"));
        assert!(p.contains("Say which meeting each claim comes from"));
        assert!(p.contains("ONLY reliable indicator"), "shares the speaker rules");
        assert!(p.contains("strictly in the meeting summaries"));

        let p = build_project_system_prompt("CTX", &partial, ChatGrounding::TranscriptOnly);
        assert!(p.contains("Do not quote those meetings verbatim"));
    }

    #[test]
    fn grounding_modes_change_only_the_fallback_clause() {
        let info = ProjectChatContextInfo::default();

        let strict = build_project_system_prompt("C", &info, ChatGrounding::TranscriptOnly);
        assert!(strict.contains("say you cannot find it rather than guessing"));
        assert!(!strict.contains("Search the web"));

        let general = build_project_system_prompt("C", &info, ChatGrounding::GeneralKnowledge);
        assert!(general.contains("your own general knowledge"));
        assert!(!general.contains("Search the web"));

        let web = build_project_system_prompt("C", &info, ChatGrounding::WebSearch);
        assert!(web.contains("Search the web"));
    }

    #[test]
    fn summary_prompt_prefers_the_canonical_english_cache() {
        let raw = r#"{"markdown":"Resumen en español","english_cache":{"markdown":"English summary"}}"#;
        assert_eq!(render_summary_for_prompt(raw).as_deref(), Some("English summary"));

        let no_cache = r#"{"markdown":"Just this"}"#;
        assert_eq!(render_summary_for_prompt(no_cache).as_deref(), Some("Just this"));
    }

    #[test]
    fn empty_or_unparseable_summaries_are_treated_as_absent() {
        assert!(render_summary_for_prompt("not json").is_none());
        assert!(render_summary_for_prompt(r#"{"markdown":"   "}"#).is_none());
        assert!(render_summary_for_prompt(r#"{"summary_json":[]}"#).is_none());
    }

    /// The budget must be a real number for every provider — `usize::MAX` is
    /// what makes a twelve-meeting project either a 400 or a surprise bill.
    #[tokio::test]
    async fn no_provider_gets_an_unbounded_budget() {
        for (provider, model) in [
            (LLMProvider::Claude, "claude-sonnet-4"),
            (LLMProvider::OpenAI, "gpt-4o"),
            (LLMProvider::Groq, "llama-3.3-70b"),
            (LLMProvider::OpenRouter, "anything"),
            (LLMProvider::CustomOpenAI, "anything"),
            (LLMProvider::ChatGptSubscription, "gpt-5"),
            (LLMProvider::LMStudio, "local"),
        ] {
            let budget = project_context_char_budget(&provider, model, None).await;
            assert!(budget < usize::MAX, "{:?} must have a real budget", provider);
            assert!(budget > 0, "{:?} must have a usable budget", provider);
        }
    }
}
