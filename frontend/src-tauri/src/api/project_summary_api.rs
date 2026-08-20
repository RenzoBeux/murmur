//! Tauri commands for the stored project brief.
//!
//! [`api_get_project_summary`] reads status straight off the row rather than
//! from any in-process registry. That is deliberate and load-bearing: the user
//! can start a brief and navigate away, so the UI must be able to re-attach to a
//! run in flight with nothing but the project id.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tracing::warn;

use crate::api::project_chat_context::{project_meeting_availability, ProjectMeetingContextEntry};
use crate::database::repositories::{
    project::ProjectsRepository, project_summary::ProjectSummariesRepository,
};
use crate::state::AppState;
use crate::summary::project_service::{generate_project_summary_background, project_cancellation_key};
use crate::summary::service::SummaryService;

/// One meeting the stored brief was built from — a snapshot taken at generation
/// time, not a live join. A meeting later removed from the project has no row to
/// join to, and that is precisely the entry the coverage UI must still render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoveredMeeting {
    pub id: String,
    pub title: String,
    #[serde(rename = "createdAt", default)]
    pub created_at: String,
    /// "summary" | "transcript" | "none".
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub fingerprint: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummaryCoverage {
    /// What the stored brief covers.
    pub covered: Vec<CoveredMeeting>,
    /// In the project now, absent from the brief.
    pub added: Vec<ProjectMeetingContextEntry>,
    /// In the brief, no longer in the project.
    pub removed: Vec<CoveredMeeting>,
    /// Still in both, but its own summary changed since.
    pub changed: Vec<CoveredMeeting>,
    pub is_stale: bool,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummaryProgress {
    pub stage: Option<String>,
    pub current: i64,
    pub total: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSummaryResponse {
    /// 'idle' when no brief has ever been generated; otherwise the stored status.
    pub status: String,
    pub markdown: Option<String>,
    pub error: Option<String>,
    pub generated_at: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub language: Option<String>,
    pub progress: ProjectSummaryProgress,
    pub coverage: ProjectSummaryCoverage,
    /// Every meeting currently in the project, and what it can contribute.
    pub meetings: Vec<ProjectMeetingContextEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartProjectSummaryResponse {
    pub started: bool,
    /// True when a run was already in flight, so the UI polls instead of erroring.
    pub already_running: bool,
}

fn markdown_of(result: Option<&str>) -> Option<String> {
    let raw = result?;
    let value = crate::mcp::tools::parse_summary(raw)?;
    let rendered = crate::mcp::tools::render_summary(&value);
    let rendered = rendered.trim();
    if rendered.is_empty() || rendered == "(Summary is empty.)" {
        return None;
    }
    Some(rendered.to_string())
}

#[tauri::command]
pub async fn api_get_project_summary(
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<ProjectSummaryResponse, String> {
    let pool = state.db_manager.pool();

    let meetings = ProjectsRepository::list_meetings(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to load project meetings: {}", e))?;
    let availability = project_meeting_availability(pool, &meetings).await?;

    let row = ProjectSummariesRepository::get(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to load project brief: {}", e))?;

    let Some(row) = row else {
        return Ok(ProjectSummaryResponse {
            status: "idle".to_string(),
            markdown: None,
            error: None,
            generated_at: None,
            provider: None,
            model: None,
            language: None,
            progress: ProjectSummaryProgress::default(),
            coverage: ProjectSummaryCoverage::default(),
            meetings: availability,
        });
    };

    let covered: Vec<CoveredMeeting> = row
        .covered_meetings
        .as_deref()
        .and_then(|raw| {
            serde_json::from_str::<Vec<CoveredMeeting>>(raw)
                .map_err(|e| warn!("Ignoring unreadable project coverage: {}", e))
                .ok()
        })
        .unwrap_or_default();

    let coverage = diff_coverage(covered, &availability, pool, &meetings).await?;

    Ok(ProjectSummaryResponse {
        status: row.status,
        markdown: markdown_of(row.result.as_deref()),
        error: row.error,
        generated_at: row.end_time.map(|t| t.to_rfc3339()),
        provider: row.model_provider,
        model: row.model_name,
        language: row.output_language,
        progress: ProjectSummaryProgress {
            stage: row.stage,
            current: row.stage_current,
            total: row.stage_total,
        },
        coverage,
        meetings: availability,
    })
}

/// Compare what the stored brief covers against the project as it stands now.
///
/// A set diff, not a clock comparison: meeting timestamps are optional and the
/// list order is not guaranteed, so anything time-based would be fragile.
async fn diff_coverage(
    covered: Vec<CoveredMeeting>,
    availability: &[ProjectMeetingContextEntry],
    pool: &sqlx::SqlitePool,
    meetings: &[crate::database::models::MeetingModel],
) -> Result<ProjectSummaryCoverage, String> {
    use std::collections::HashMap;

    let covered_by_id: HashMap<&str, &CoveredMeeting> =
        covered.iter().map(|c| (c.id.as_str(), c)).collect();

    // A meeting that had nothing to contribute is not really "covered" — once it
    // has a summary it must show up as newly available, not as already included.
    let added: Vec<ProjectMeetingContextEntry> = availability
        .iter()
        .filter(|m| {
            covered_by_id
                .get(m.meeting_id.as_str())
                .map(|c| c.source == "none")
                .unwrap_or(true)
        })
        .cloned()
        .collect();

    let live_ids: std::collections::HashSet<&str> =
        availability.iter().map(|m| m.meeting_id.as_str()).collect();
    let removed: Vec<CoveredMeeting> = covered
        .iter()
        .filter(|c| c.source != "none" && !live_ids.contains(c.id.as_str()))
        .cloned()
        .collect();

    // "Changed" means the meeting's own summary was rewritten since the brief
    // read it. Fingerprints are over the rendered markdown, so merely switching
    // models does not count.
    let ids: Vec<String> = meetings.iter().map(|m| m.id.clone()).collect();
    let current = crate::database::repositories::summary::SummaryProcessesRepository::get_results_for_meetings(pool, &ids)
        .await
        .map_err(|e| format!("Failed to load meeting summaries: {}", e))?;

    let changed: Vec<CoveredMeeting> = covered
        .iter()
        .filter(|c| c.source == "summary" && live_ids.contains(c.id.as_str()))
        .filter(|c| {
            let now = current
                .get(&c.id)
                .and_then(|raw| crate::api::project_chat_context::render_summary_for_prompt(raw))
                .map(|body| crate::summary::service::stable_text_fingerprint(&body));
            now.as_deref() != Some(c.fingerprint.as_str())
        })
        .cloned()
        .collect();

    let is_stale = !added.is_empty() || !removed.is_empty() || !changed.is_empty();
    Ok(ProjectSummaryCoverage {
        covered,
        added,
        removed,
        changed,
        is_stale,
    })
}

#[tauri::command]
pub async fn api_generate_project_summary<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    project_id: String,
    provider: String,
    model: String,
    summary_language: Option<String>,
) -> Result<StartProjectSummaryResponse, String> {
    if project_id.trim().is_empty() {
        return Err("project_id is required".to_string());
    }
    if provider.trim().is_empty() || model.trim().is_empty() {
        return Err("Pick a model before generating a project brief.".to_string());
    }

    let pool = state.db_manager.pool();

    // Guard before claiming the row, so a project with nothing to read never
    // leaves a PENDING row behind for the orphan sweep to clean up.
    let meetings = ProjectsRepository::list_meetings(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to load project meetings: {}", e))?;
    if meetings.is_empty() {
        return Err("This project has no meetings yet.".to_string());
    }
    let availability = project_meeting_availability(pool, &meetings).await?;
    if !availability.iter().any(|m| m.has_summary || m.has_transcript) {
        return Err(
            "No meeting in this project has a summary or a transcript yet. Generate a meeting \
             summary first."
                .to_string(),
        );
    }

    // Atomic claim: returns false when a run is already in flight.
    let started = ProjectSummariesRepository::try_begin(
        pool,
        &project_id,
        &provider,
        &model,
        summary_language.as_deref(),
    )
    .await
    .map_err(|e| format!("Failed to start project brief: {}", e))?;

    if !started {
        return Ok(StartProjectSummaryResponse {
            started: false,
            already_running: true,
        });
    }

    let pool = pool.clone();
    tauri::async_runtime::spawn(generate_project_summary_background(
        app,
        pool,
        project_id,
        provider,
        model,
        summary_language,
    ));

    Ok(StartProjectSummaryResponse {
        started: true,
        already_running: false,
    })
}

#[tauri::command]
pub async fn api_cancel_project_summary(
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<bool, String> {
    let cancelled = SummaryService::cancel_summary(&project_cancellation_key(&project_id));
    if !cancelled {
        // Nothing in flight in this process — reconcile the row anyway, so a
        // status stranded by a previous run cannot spin forever.
        let _ =
            ProjectSummariesRepository::update_cancelled(state.db_manager.pool(), &project_id).await;
    }
    Ok(cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, has_summary: bool) -> ProjectMeetingContextEntry {
        ProjectMeetingContextEntry {
            meeting_id: id.into(),
            title: format!("Meeting {id}"),
            has_summary,
            has_transcript: true,
        }
    }

    fn covered(id: &str, source: &str, fingerprint: &str) -> CoveredMeeting {
        CoveredMeeting {
            id: id.into(),
            title: format!("Meeting {id}"),
            created_at: "2026-03-04T00:00:00Z".into(),
            source: source.into(),
            fingerprint: fingerprint.into(),
        }
    }

    #[tokio::test]
    async fn coverage_reports_added_removed_and_nothing_when_in_sync() {
        let pool = crate::database::test_support::migrated_pool().await;

        // In sync: one covered meeting, still present.
        let c = diff_coverage(
            vec![covered("m1", "summary", "fp")],
            &[entry("m1", true)],
            &pool,
            &[],
        )
        .await
        .unwrap();
        assert!(c.added.is_empty());
        assert!(c.removed.is_empty());

        // A meeting added since the brief.
        let c = diff_coverage(
            vec![covered("m1", "summary", "fp")],
            &[entry("m1", true), entry("m2", true)],
            &pool,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(c.added.len(), 1);
        assert_eq!(c.added[0].meeting_id, "m2");
        assert!(c.is_stale);

        // A covered meeting removed from the project.
        let c = diff_coverage(
            vec![covered("m1", "summary", "fp"), covered("m2", "summary", "fp")],
            &[entry("m1", true)],
            &pool,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(c.removed.len(), 1);
        assert_eq!(c.removed[0].id, "m2");
        assert!(c.is_stale);
    }

    /// A meeting that contributed nothing must resurface as "added" once it has
    /// a summary — otherwise the brief looks complete while missing it.
    #[tokio::test]
    async fn an_uncovered_meeting_counts_as_added_not_covered() {
        let pool = crate::database::test_support::migrated_pool().await;
        let c = diff_coverage(
            vec![covered("m1", "none", "")],
            &[entry("m1", true)],
            &pool,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(c.added.len(), 1);
        assert!(c.is_stale);
    }

    #[test]
    fn markdown_is_read_out_of_the_stored_envelope() {
        // Built with json! rather than written as a literal: the brief starts
        // with a `##` heading, and `"#` inside a raw string closes it early.
        let blob = serde_json::json!({ "markdown": "## Where things stand\n\nGood." }).to_string();
        assert_eq!(
            markdown_of(Some(&blob)).as_deref(),
            Some("## Where things stand\n\nGood.")
        );
        assert!(markdown_of(None).is_none());
        assert!(markdown_of(Some("not json")).is_none());
        assert!(markdown_of(Some(r#"{"markdown":"  "}"#)).is_none());
    }
}
