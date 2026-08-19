use std::path::PathBuf;

use log::{error, info};
use serde::Serialize;
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;

use crate::database::models::Transcript;
use crate::database::repositories::meeting::MeetingsRepository;
use crate::database::repositories::summary::SummaryProcessesRepository;
use crate::state::AppState;

pub mod markdown;
pub mod naming;
pub mod plan;
pub mod zipper;

use markdown::{speaker_display_name, yaml_value};

#[tauri::command]
pub async fn export_meeting_markdown<R: Runtime>(
    app: AppHandle<R>,
    content: String,
    suggested_filename: String,
) -> Result<Option<String>, String> {
    info!(
        "export_meeting_markdown: opening save dialog (suggested filename: {})",
        suggested_filename
    );

    let app_clone = app.clone();
    let chosen = tokio::task::spawn_blocking(move || {
        app_clone
            .dialog()
            .file()
            .add_filter("Markdown", &["md"])
            .set_file_name(&suggested_filename)
            .blocking_save_file()
    })
    .await
    .map_err(|e| format!("Save dialog task failed: {e}"))?;

    match chosen {
        Some(path) => {
            let path_str = path.to_string();
            std::fs::write(&path_str, content).map_err(|e| {
                error!("Failed to write markdown export to {}: {}", path_str, e);
                format!("Failed to write file: {e}")
            })?;
            info!("Exported meeting markdown to {}", path_str);
            Ok(Some(path_str))
        }
        None => {
            info!("User cancelled markdown export save dialog");
            Ok(None)
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ExportAllResult {
    pub folder: Option<String>,
    pub exported: usize,
}

/// Build one meeting's Obsidian-style markdown: YAML frontmatter + optional
/// rendered summary + the transcript. Pure, so it is unit-testable.
pub fn build_meeting_markdown(
    title: &str,
    created_at_rfc3339: &str,
    meeting_id: &str,
    transcripts: &[Transcript],
    summary_result: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("title: {}\n", yaml_value(title)));
    out.push_str(&format!("date: {}\n", created_at_rfc3339));
    out.push_str(&format!("meeting_id: {}\n", meeting_id));
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n\n", title));

    if let Some(raw) = summary_result {
        if let Some(value) = crate::mcp::tools::parse_summary(raw) {
            let rendered = crate::mcp::tools::render_summary(&value);
            if !rendered.trim().is_empty() {
                out.push_str("## Summary\n\n");
                out.push_str(rendered.trim());
                out.push_str("\n\n");
            }
        }
    }

    out.push_str("## Transcript\n\n");
    for seg in transcripts {
        let text = seg.transcript.trim();
        if text.is_empty() {
            continue;
        }
        match seg.speaker.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(sp) => out.push_str(&format!(
                "**{}:** {}\n\n",
                speaker_display_name(sp.trim()),
                text
            )),
            None => out.push_str(&format!("{}\n\n", text)),
        }
    }
    out
}

/// `<slug>-<YYYY-MM-DD>.md` filename for a meeting.
fn export_filename(title: &str, created_at_rfc3339: &str) -> String {
    let date = created_at_rfc3339.get(0..10).unwrap_or("undated");
    let raw: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let slug: String = raw
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "meeting".to_string() } else { slug };
    format!("{}-{}.md", slug, date)
}

/// Export every meeting to a chosen folder as an individual markdown file.
#[tauri::command]
pub async fn export_all_markdown<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<ExportAllResult, String> {
    let app_clone = app.clone();
    let folder =
        tokio::task::spawn_blocking(move || app_clone.dialog().file().blocking_pick_folder())
            .await
            .map_err(|e| format!("Folder dialog task failed: {e}"))?;
    let folder = match folder {
        Some(f) => f.to_string(),
        None => {
            info!("User cancelled bulk export folder picker");
            return Ok(ExportAllResult {
                folder: None,
                exported: 0,
            });
        }
    };

    let pool = state.db_manager.pool();
    let meetings = MeetingsRepository::get_meetings(pool)
        .await
        .map_err(|e| format!("Failed to list meetings: {e}"))?;

    let mut exported = 0usize;
    for m in &meetings {
        let created = m.created_at.0.to_rfc3339();
        let (transcripts, _total) =
            MeetingsRepository::get_meeting_transcripts_paginated(pool, &m.id, 1_000_000, 0)
                .await
                .map_err(|e| format!("Failed to load transcripts for {}: {e}", m.id))?;
        let summary = SummaryProcessesRepository::get_summary_data(pool, &m.id)
            .await
            .ok()
            .flatten();
        let summary_result = summary.as_ref().and_then(|s| s.result.as_deref());

        let md = build_meeting_markdown(&m.title, &created, &m.id, &transcripts, summary_result);
        let path = std::path::Path::new(&folder).join(export_filename(&m.title, &created));
        if let Err(e) = std::fs::write(&path, md) {
            error!("Failed to write {}: {}", path.display(), e);
            continue;
        }
        exported += 1;
    }

    info!("Bulk export wrote {} meeting file(s) to {}", exported, folder);
    Ok(ExportAllResult {
        folder: Some(folder),
        exported,
    })
}

/// Result of a bundle export. `path` is `None` when the user dismissed the
/// save dialog, matching `ExportAllResult`'s cancel convention.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportBundleResult {
    pub path: Option<String>,
    pub meetings_exported: usize,
    pub files_written: usize,
    pub bytes_written: u64,
    /// Non-fatal problems (a missing attachment, an unreadable transcript).
    /// Collected rather than raised so one bad row cannot sink the export.
    pub warnings: Vec<String>,
}

/// What the export dialog can offer, before the user commits to anything.
///
/// Lives in Rust because attachment and audio sizes come from stat-ing the
/// filesystem, which the webview cannot do.
#[tauri::command]
pub async fn export_bundle_availability<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    scope: plan::ExportScope,
) -> Result<plan::ExportAvailability, String> {
    let inputs = plan_inputs(&app)?;
    plan::build_availability(state.db_manager.pool(), &inputs, &scope).await
}

/// Export a meeting or a project to a `.zip` the recipient can just unzip.
#[tauri::command]
pub async fn export_bundle<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    request: plan::ExportBundleRequest,
) -> Result<ExportBundleResult, String> {
    let inputs = plan_inputs(&app)?;

    // Plan first: never show a save dialog that could only produce an empty
    // archive. Every DB read finishes here, before anything crosses to the
    // blocking thread.
    let plan::PreparedBundle {
        plan,
        suggested_filename,
    } = plan::build_plan(state.db_manager.pool(), &inputs, &request).await?;

    let app_clone = app.clone();
    let chosen = tokio::task::spawn_blocking(move || {
        app_clone
            .dialog()
            .file()
            .add_filter("ZIP archive", &["zip"])
            .set_file_name(&suggested_filename)
            .blocking_save_file()
    })
    .await
    .map_err(|e| format!("Save dialog task failed: {e}"))?;

    let Some(chosen) = chosen else {
        info!("User cancelled the export save dialog");
        return Ok(ExportBundleResult {
            path: None,
            meetings_exported: 0,
            files_written: 0,
            bytes_written: 0,
            warnings: Vec::new(),
        });
    };

    // Only the Windows save dialog reliably appends the filter's extension.
    let mut dest = PathBuf::from(chosen.to_string());
    if dest
        .extension()
        .map(|e| !e.eq_ignore_ascii_case("zip"))
        .unwrap_or(true)
    {
        let mut name = dest.file_name().unwrap_or_default().to_os_string();
        name.push(".zip");
        dest.set_file_name(name);
    }

    let meetings_exported = plan.meetings;
    let dest_for_write = dest.clone();

    // Zipping is pure blocking IO and can move hundreds of megabytes, so it
    // must not run on the async runtime the way `export_all_markdown` does.
    let stats = tokio::task::spawn_blocking(move || zipper::write_zip(plan, &dest_for_write))
        .await
        .map_err(|e| format!("Export task failed: {e}"))?
        .map_err(|e| {
            error!("Bundle export failed: {e}");
            e
        })?;

    info!(
        "Exported bundle to {} ({} meeting(s), {} file(s), {} bytes, {} warning(s))",
        dest.display(),
        meetings_exported,
        stats.files_written,
        stats.bytes_written,
        stats.warnings.len()
    );

    Ok(ExportBundleResult {
        path: Some(dest.to_string_lossy().to_string()),
        meetings_exported,
        files_written: stats.files_written,
        bytes_written: stats.bytes_written,
        warnings: stats.warnings,
    })
}

fn plan_inputs<R: Runtime>(app: &AppHandle<R>) -> Result<plan::PlanInputs, String> {
    Ok(plan::PlanInputs {
        attachments_root: crate::api::attachments_api::attachments_base_dir(app)?,
        exported_at_rfc3339: chrono::Utc::now().to_rfc3339(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_filename_slugifies_title_and_date() {
        assert_eq!(
            export_filename("Team Standup!", "2026-07-13T10:00:00+00:00"),
            "team-standup-2026-07-13.md"
        );
        assert_eq!(export_filename("   ", "2026-01-02T00:00:00Z"), "meeting-2026-01-02.md");
        assert_eq!(
            export_filename("Reunión: Q3", "2026-12-31T23:59:59Z"),
            "reunión-q3-2026-12-31.md"
        );
    }

    #[test]
    fn build_markdown_has_frontmatter_and_transcript() {
        let md = build_meeting_markdown(
            "Weekly: Sync",
            "2026-07-13T10:00:00+00:00",
            "meeting-1",
            &[],
            None,
        );
        assert!(md.starts_with("---\n"));
        assert!(md.contains("title: \"Weekly: Sync\""), "a title with ':' is quoted");
        assert!(md.contains("date: 2026-07-13T10:00:00+00:00"));
        assert!(md.contains("meeting_id: meeting-1"));
        assert!(md.contains("## Transcript"));
    }
}
