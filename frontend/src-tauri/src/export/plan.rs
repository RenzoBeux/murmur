//! Turns an export request into a fully-materialized [`BundlePlan`].
//!
//! Everything here runs on the async runtime and finishes **before** the zip
//! writer starts: all DB reads, all markdown generation, and the folder-name
//! allocation pass. The resulting plan owns every byte of text and every source
//! path it references, which is what lets the write phase move to a blocking
//! thread (a `tauri::State` is not `'static`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use log::warn;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::audio::audio_processing::sanitize_filename;
use crate::database::models::{MeetingModel, ProjectModel};
use crate::database::repositories::attachment::AttachmentsRepository;
use crate::database::repositories::meeting::MeetingsRepository;
use crate::database::repositories::project::ProjectsRepository;
use crate::database::repositories::summary::SummaryProcessesRepository;

use super::markdown::{
    build_project_readme, build_summary_markdown, build_transcript_markdown, MeetingMeta,
    ReadmeRow, TranscriptFormat,
};
use super::naming::{bundle_filename, meeting_folder_name, NameAllocator};
use super::zipper::{BundlePlan, PlannedEntry};

/// What the user is exporting.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExportScope {
    #[serde(rename_all = "camelCase")]
    Meeting { meeting_id: String },
    #[serde(rename_all = "camelCase")]
    Project {
        project_id: String,
        /// The subset the user left checked. Empty means nothing is selected,
        /// which is an error rather than "everything".
        #[serde(default)]
        meeting_ids: Vec<String>,
    },
}

/// Which kinds of content go into the archive.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportContents {
    pub transcript: bool,
    pub summary: bool,
    pub attachments: bool,
    /// Off by default in the UI — an hour of audio is 50-100 MB.
    pub audio: bool,
}

impl ExportContents {
    fn is_empty(&self) -> bool {
        !self.transcript && !self.summary && !self.attachments && !self.audio
    }

    /// Human-readable list for the README's "About this export" section.
    fn labels(&self) -> Vec<&'static str> {
        let mut labels = Vec::new();
        if self.transcript {
            labels.push("transcripts");
        }
        if self.summary {
            labels.push("summaries");
        }
        if self.attachments {
            labels.push("attached files");
        }
        if self.audio {
            labels.push("audio recordings");
        }
        labels
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportBundleRequest {
    pub scope: ExportScope,
    pub contents: ExportContents,
    #[serde(default)]
    pub transcript_format: TranscriptFormat,
}

/// Paths and stamps the planner cannot derive on its own.
#[derive(Debug, Clone)]
pub struct PlanInputs {
    /// `{app_data_dir}/attachments`.
    pub attachments_root: PathBuf,
    pub exported_at_rfc3339: String,
}

/// A plan plus the filename to pre-fill the save dialog with.
#[derive(Debug)]
pub struct PreparedBundle {
    pub plan: BundlePlan,
    pub suggested_filename: String,
}

// ---------------------------------------------------------------------------
// Availability — what the dialog shows before the user commits
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportAvailability {
    /// Present only for project scope.
    pub project: Option<ProjectInfo>,
    /// One row for meeting scope, every live meeting for project scope.
    pub meetings: Vec<MeetingExportInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingExportInfo {
    pub meeting_id: String,
    pub title: String,
    pub created_at: String,
    pub transcript_segments: i64,
    pub has_summary: bool,
    pub attachment_count: usize,
    pub attachment_bytes: u64,
    /// `None` when the meeting has no recording on disk.
    pub audio_bytes: Option<u64>,
}

/// Everything the export dialog needs, in one round trip.
///
/// This has to live in Rust: attachment and audio **sizes** come from stat-ing
/// the filesystem, and probing N meetings from the webview would be 3N invokes.
pub async fn build_availability(
    pool: &SqlitePool,
    inputs: &PlanInputs,
    scope: &ExportScope,
) -> Result<ExportAvailability, String> {
    let (project, meetings) = match scope {
        ExportScope::Meeting { meeting_id } => {
            let meeting = load_meeting(pool, meeting_id).await?;
            (None, vec![meeting])
        }
        ExportScope::Project { project_id, .. } => {
            let project = load_project(pool, project_id).await?;
            // Already filters `deleted_at IS NULL`; returns newest-first.
            let mut meetings = ProjectsRepository::list_meetings(pool, project_id)
                .await
                .map_err(|e| format!("Failed to list project meetings: {e}"))?;
            meetings.reverse();
            (
                Some(ProjectInfo {
                    id: project.id,
                    name: project.name,
                    description: project.description,
                }),
                meetings,
            )
        }
    };

    let mut rows = Vec::with_capacity(meetings.len());
    for meeting in &meetings {
        // `limit = 0` gives us the total count without materializing any rows.
        let segments = MeetingsRepository::get_meeting_transcripts_paginated(pool, &meeting.id, 0, 0)
            .await
            .map(|(_, total)| total)
            .unwrap_or(0);

        let has_summary = SummaryProcessesRepository::get_summary_data(pool, &meeting.id)
            .await
            .ok()
            .flatten()
            .and_then(|s| s.result)
            .as_deref()
            .and_then(|raw| {
                let meta = MeetingMeta {
                    id: &meeting.id,
                    title: &meeting.title,
                    created_at_rfc3339: "",
                    ..Default::default()
                };
                build_summary_markdown(&meta, Some(raw))
            })
            .is_some();

        // Sized from disk, not from `size_bytes`: a row whose file has gone
        // missing would otherwise have the dialog promise bytes that the export
        // then silently skips.
        let dir = meeting_attachments_dir(&inputs.attachments_root, &meeting.id);
        let present: Vec<u64> = AttachmentsRepository::list_for_meeting(pool, &meeting.id)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|a| !is_suspicious_stored_name(&a.stored_name))
            .filter_map(|a| std::fs::metadata(dir.join(&a.stored_name)).ok())
            .map(|meta| meta.len())
            .collect();

        rows.push(MeetingExportInfo {
            meeting_id: meeting.id.clone(),
            title: meeting.title.clone(),
            created_at: meeting.created_at.0.to_rfc3339(),
            transcript_segments: segments,
            has_summary,
            attachment_count: present.len(),
            attachment_bytes: present.iter().sum(),
            audio_bytes: locate_audio(meeting).map(|(_, _, size)| size),
        });
    }

    Ok(ExportAvailability {
        project,
        meetings: rows,
    })
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

/// Build the archive contents for `request`.
pub async fn build_plan(
    pool: &SqlitePool,
    inputs: &PlanInputs,
    request: &ExportBundleRequest,
) -> Result<PreparedBundle, String> {
    if request.contents.is_empty() {
        return Err("Select at least one thing to include.".to_string());
    }

    // One aggregate query for the whole DB rather than one per meeting.
    let durations = MeetingsRepository::get_meeting_durations(pool)
        .await
        .unwrap_or_default();

    match &request.scope {
        ExportScope::Meeting { meeting_id } => {
            plan_single_meeting(pool, inputs, request, meeting_id, &durations).await
        }
        ExportScope::Project {
            project_id,
            meeting_ids,
        } => plan_project(pool, inputs, request, project_id, meeting_ids, &durations).await,
    }
}

async fn plan_single_meeting(
    pool: &SqlitePool,
    inputs: &PlanInputs,
    request: &ExportBundleRequest,
    meeting_id: &str,
    durations: &HashMap<String, f64>,
) -> Result<PreparedBundle, String> {
    let meeting = load_meeting(pool, meeting_id).await?;

    // A meeting opened from the trash is still a meeting the user explicitly
    // named, so export it — but say so in the frontmatter.
    let trashed = is_trashed(pool, meeting_id).await;

    let project_name = match meeting.project_id.as_deref() {
        Some(id) => ProjectsRepository::get(pool, id)
            .await
            .ok()
            .flatten()
            .map(|p| p.name),
        None => None,
    };

    let mut warnings = Vec::new();
    let content = gather_meeting(
        pool,
        inputs,
        &meeting,
        project_name.as_deref(),
        durations.get(&meeting.id).copied(),
        trashed,
        request,
        &mut warnings,
    )
    .await;

    if content.is_empty() {
        return Err(empty_selection_message(&request.contents));
    }

    let entries = content.into_entries("");
    let created = meeting.created_at.0.to_rfc3339();
    let date = created.get(0..10).unwrap_or("undated");

    Ok(PreparedBundle {
        plan: BundlePlan {
            entries,
            warnings,
            meetings: 1,
        },
        suggested_filename: bundle_filename(&format!("{} {}", meeting.title, date)),
    })
}

async fn plan_project(
    pool: &SqlitePool,
    inputs: &PlanInputs,
    request: &ExportBundleRequest,
    project_id: &str,
    meeting_ids: &[String],
    durations: &HashMap<String, f64>,
) -> Result<PreparedBundle, String> {
    let project = load_project(pool, project_id).await?;

    if meeting_ids.is_empty() {
        return Err("Select at least one meeting to export.".to_string());
    }

    let mut meetings = ProjectsRepository::list_meetings(pool, project_id)
        .await
        .map_err(|e| format!("Failed to list project meetings: {e}"))?;
    // `list_meetings` is newest-first; ascending makes the README table order
    // match the date-prefixed folder order.
    meetings.reverse();
    meetings.retain(|m| meeting_ids.iter().any(|id| id == &m.id));

    if meetings.is_empty() {
        return Err("None of the selected meetings are still in this project.".to_string());
    }

    let mut warnings = Vec::new();
    let mut entries = Vec::new();
    let mut readme_rows = Vec::with_capacity(meetings.len());
    let mut folders = NameAllocator::new();
    let mut exported = 0usize;

    for meeting in &meetings {
        let content = gather_meeting(
            pool,
            inputs,
            meeting,
            Some(&project.name),
            durations.get(&meeting.id).copied(),
            false,
            request,
            &mut warnings,
        )
        .await;

        let created = meeting.created_at.0.to_rfc3339();

        // A meeting that contributed nothing gets no folder — but it still
        // appears in the README so the counts reconcile.
        let folder = if content.is_empty() {
            None
        } else {
            let folder = folders.allocate(&meeting_folder_name(&meeting.title, &created));
            entries.extend(content.into_entries(&format!("{folder}/")));
            exported += 1;
            Some(folder)
        };

        readme_rows.push(ReadmeRow {
            title: meeting.title.clone(),
            created_at_rfc3339: created,
            duration_seconds: durations.get(&meeting.id).copied(),
            folder,
        });
    }

    // Built last so its links use the deduped folder names above.
    let readme = build_project_readme(
        &project,
        &readme_rows,
        &inputs.exported_at_rfc3339,
        &request.contents.labels(),
        request.transcript_format,
    );
    entries.insert(
        0,
        PlannedEntry::Text {
            path: "README.md".to_string(),
            body: readme,
        },
    );

    Ok(PreparedBundle {
        plan: BundlePlan {
            entries,
            warnings,
            meetings: exported,
        },
        suggested_filename: bundle_filename(&project.name),
    })
}

// ---------------------------------------------------------------------------
// Per-meeting content
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct MeetingContent {
    transcript: Option<String>,
    summary: Option<String>,
    /// `(entry filename, source path, size)`.
    attachments: Vec<(String, PathBuf, u64)>,
    audio: Option<(String, PathBuf, u64)>,
}

impl MeetingContent {
    fn is_empty(&self) -> bool {
        self.transcript.is_none()
            && self.summary.is_none()
            && self.attachments.is_empty()
            && self.audio.is_none()
    }

    /// Flatten into zip entries under `prefix` (empty for meeting scope,
    /// `"<folder>/"` for project scope).
    ///
    /// Entry paths are joined with `format!`, never `Path::join` — on Windows
    /// the latter yields `\`, and some extractors read that as part of the
    /// filename rather than a separator.
    fn into_entries(self, prefix: &str) -> Vec<PlannedEntry> {
        let mut entries = Vec::new();

        if let Some(body) = self.transcript {
            entries.push(PlannedEntry::Text {
                path: format!("{prefix}transcript.md"),
                body,
            });
        }
        if let Some(body) = self.summary {
            entries.push(PlannedEntry::Text {
                path: format!("{prefix}summary.md"),
                body,
            });
        }
        if let Some((name, source, size)) = self.audio {
            entries.push(PlannedEntry::File {
                path: format!("{prefix}{name}"),
                source,
                size,
            });
        }
        for (name, source, size) in self.attachments {
            entries.push(PlannedEntry::File {
                path: format!("{prefix}files/{name}"),
                source,
                size,
            });
        }
        entries
    }
}

#[allow(clippy::too_many_arguments)]
async fn gather_meeting(
    pool: &SqlitePool,
    inputs: &PlanInputs,
    meeting: &MeetingModel,
    project_name: Option<&str>,
    duration_seconds: Option<f64>,
    trashed: bool,
    request: &ExportBundleRequest,
    warnings: &mut Vec<String>,
) -> MeetingContent {
    let created = meeting.created_at.0.to_rfc3339();
    let meta = MeetingMeta {
        id: &meeting.id,
        title: &meeting.title,
        created_at_rfc3339: &created,
        project_name,
        duration_seconds,
        trashed,
    };

    let mut content = MeetingContent::default();

    if request.contents.transcript {
        match MeetingsRepository::get_meeting_transcripts_paginated(pool, &meeting.id, 1_000_000, 0)
            .await
        {
            Ok((transcripts, _)) => {
                content.transcript =
                    build_transcript_markdown(&meta, &transcripts, request.transcript_format);
            }
            Err(e) => {
                warn!("Export: failed to load transcripts for {}: {e}", meeting.id);
                warnings.push(format!(
                    "Could not read the transcript for \"{}\".",
                    meeting.title
                ));
            }
        }
    }

    if request.contents.summary {
        let raw = SummaryProcessesRepository::get_summary_data(pool, &meeting.id)
            .await
            .ok()
            .flatten()
            .and_then(|s| s.result);
        content.summary = build_summary_markdown(&meta, raw.as_deref());
    }

    if request.contents.attachments {
        content.attachments =
            gather_attachments(pool, inputs, meeting, warnings).await;
    }

    if request.contents.audio {
        match locate_audio(meeting) {
            Some((name, source, size)) => content.audio = Some((name, source, size)),
            None => {
                if meeting.folder_path.is_some() {
                    warnings.push(format!(
                        "No recording found on disk for \"{}\".",
                        meeting.title
                    ));
                }
            }
        }
    }

    content
}

async fn gather_attachments(
    pool: &SqlitePool,
    inputs: &PlanInputs,
    meeting: &MeetingModel,
    warnings: &mut Vec<String>,
) -> Vec<(String, PathBuf, u64)> {
    let rows = match AttachmentsRepository::list_for_meeting(pool, &meeting.id).await {
        Ok(rows) => rows,
        Err(e) => {
            warn!("Export: failed to list attachments for {}: {e}", meeting.id);
            warnings.push(format!(
                "Could not read attachments for \"{}\".",
                meeting.title
            ));
            return Vec::new();
        }
    };

    // The DB only guarantees `stored_name` is unique; two attachments can share
    // a display name, and the zip is keyed on the display name.
    let mut names = NameAllocator::new();
    let mut out = Vec::with_capacity(rows.len());

    for row in rows {
        // The planner reads rows directly, so it does not inherit the traversal
        // check `resolve_attachment_path` applies on the command path.
        if is_suspicious_stored_name(&row.stored_name) {
            warn!(
                "Export: refusing attachment {} with suspicious stored_name {:?}",
                row.id, row.stored_name
            );
            warnings.push(format!("Skipped an attachment with an invalid path in \"{}\".", meeting.title));
            continue;
        }

        let source = meeting_attachments_dir(&inputs.attachments_root, &meeting.id)
            .join(&row.stored_name);

        let size = match std::fs::metadata(&source) {
            Ok(meta) => meta.len(),
            Err(_) => {
                // Reported once, here, rather than again in the writer.
                warnings.push(format!(
                    "Skipped \"{}\" from \"{}\" — the file is no longer on disk.",
                    row.file_name, meeting.title
                ));
                continue;
            }
        };

        out.push((names.allocate_filename(&row.file_name), source, size));
    }

    out
}

/// A `stored_name` should be a bare filename; anything else means the row was
/// tampered with and must never be turned into a path.
fn is_suspicious_stored_name(stored_name: &str) -> bool {
    stored_name.contains('/') || stored_name.contains('\\') || stored_name.contains("..")
}

/// `{attachments_root}/{sanitized meeting id}` — mirrors
/// `api::attachments_api::meeting_attachments_dir`, which is where the files
/// were written.
fn meeting_attachments_dir(root: &Path, meeting_id: &str) -> PathBuf {
    root.join(sanitize_filename(meeting_id))
}

/// Find a meeting's recording, returning `(entry filename, path, size)`.
fn locate_audio(meeting: &MeetingModel) -> Option<(String, PathBuf, u64)> {
    let folder = meeting
        .folder_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())?;

    let path = crate::audio::retranscription::find_audio_file(Path::new(folder)).ok()?;
    let size = std::fs::metadata(&path).ok()?.len();
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "bin".to_string());

    Some((format!("recording.{extension}"), path, size))
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

async fn load_meeting(pool: &SqlitePool, meeting_id: &str) -> Result<MeetingModel, String> {
    MeetingsRepository::get_meeting_metadata(pool, meeting_id)
        .await
        .map_err(|e| format!("Failed to load meeting: {e}"))?
        .ok_or_else(|| format!("Meeting not found: {meeting_id}"))
}

async fn load_project(pool: &SqlitePool, project_id: &str) -> Result<ProjectModel, String> {
    ProjectsRepository::get(pool, project_id)
        .await
        .map_err(|e| format!("Failed to load project: {e}"))?
        .ok_or_else(|| format!("Project not found: {project_id}"))
}

/// `get_meeting_metadata` does not select `deleted_at`, so ask separately.
async fn is_trashed(pool: &SqlitePool, meeting_id: &str) -> bool {
    sqlx::query_scalar::<_, Option<String>>("SELECT deleted_at FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .is_some()
}

/// Why a single-meeting export produced nothing, phrased for the thing the
/// user actually asked for.
fn empty_selection_message(contents: &ExportContents) -> String {
    let only = |label: &str| format!("This meeting has no {label} to export.");
    match (
        contents.transcript,
        contents.summary,
        contents.attachments,
        contents.audio,
    ) {
        (true, false, false, false) => only("transcript"),
        (false, true, false, false) => only("summary"),
        (false, false, true, false) => only("attached files"),
        (false, false, false, true) => only("recording"),
        _ => "This meeting has none of the selected content to export.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::test_support::migrated_pool;

    fn inputs() -> PlanInputs {
        PlanInputs {
            attachments_root: PathBuf::from("/nonexistent-attachments-root"),
            exported_at_rfc3339: "2026-08-19T14:03:11+00:00".to_string(),
        }
    }

    fn all_contents() -> ExportContents {
        ExportContents {
            transcript: true,
            summary: true,
            attachments: true,
            audio: false,
        }
    }

    async fn insert_meeting(pool: &SqlitePool, id: &str, title: &str, date: &str, project: Option<&str>) {
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, project_id) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(title)
        .bind(date)
        .bind(date)
        .bind(project)
        .execute(pool)
        .await
        .expect("insert meeting");
    }

    async fn insert_transcript(pool: &SqlitePool, meeting_id: &str, text: &str, start: f64) {
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, speaker) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(format!("t-{meeting_id}-{start}"))
        .bind(meeting_id)
        .bind(text)
        .bind("14:30:05")
        .bind(start)
        .bind("mic")
        .execute(pool)
        .await
        .expect("insert transcript");
    }

    async fn insert_project(pool: &SqlitePool, id: &str, name: &str) {
        sqlx::query(
            "INSERT INTO projects (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind("2026-06-01T00:00:00+00:00")
        .bind("2026-06-01T00:00:00+00:00")
        .execute(pool)
        .await
        .expect("insert project");
    }

    fn request(scope: ExportScope, contents: ExportContents) -> ExportBundleRequest {
        ExportBundleRequest {
            scope,
            contents,
            transcript_format: TranscriptFormat::default(),
        }
    }

    fn paths(bundle: &PreparedBundle) -> Vec<String> {
        bundle
            .plan
            .entries
            .iter()
            .map(|e| e.path().to_string())
            .collect()
    }

    #[tokio::test]
    async fn errors_when_nothing_is_selected() {
        let pool = migrated_pool().await;
        let req = request(
            ExportScope::Meeting {
                meeting_id: "m1".to_string(),
            },
            ExportContents {
                transcript: false,
                summary: false,
                attachments: false,
                audio: false,
            },
        );
        let err = build_plan(&pool, &inputs(), &req).await.unwrap_err();
        assert!(err.contains("at least one"), "got: {err}");
    }

    #[tokio::test]
    async fn errors_when_the_only_selection_is_empty_for_a_single_meeting() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1", "Empty", "2026-06-02T09:00:00+00:00", None).await;

        let req = request(
            ExportScope::Meeting {
                meeting_id: "m1".to_string(),
            },
            ExportContents {
                transcript: true,
                summary: false,
                attachments: false,
                audio: false,
            },
        );
        let err = build_plan(&pool, &inputs(), &req).await.unwrap_err();
        assert!(err.contains("no transcript"), "got: {err}");
    }

    #[tokio::test]
    async fn single_meeting_puts_files_at_the_archive_root() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1", "Weekly Sync", "2026-06-02T09:00:00+00:00", None).await;
        insert_transcript(&pool, "m1", "hello there", 5.0).await;

        let bundle = build_plan(
            &pool,
            &inputs(),
            &request(
                ExportScope::Meeting {
                    meeting_id: "m1".to_string(),
                },
                all_contents(),
            ),
        )
        .await
        .expect("plan builds");

        assert_eq!(paths(&bundle), vec!["transcript.md"]);
        assert_eq!(bundle.plan.meetings, 1);
        assert_eq!(bundle.suggested_filename, "Weekly Sync 2026-06-02.zip");
    }

    #[tokio::test]
    async fn respects_the_content_checkboxes() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1", "Weekly Sync", "2026-06-02T09:00:00+00:00", None).await;
        insert_transcript(&pool, "m1", "hello", 1.0).await;
        sqlx::query(
            "INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result, chunk_count, processing_time) \
             VALUES (?, 'completed', ?, ?, ?, 0, 0.0)",
        )
        .bind("m1")
        .bind("2026-06-02T09:00:00+00:00")
        .bind("2026-06-02T09:00:00+00:00")
        .bind(r###"{"markdown":"## Decisions\n- ship it"}"###)
        .execute(&pool)
        .await
        .unwrap();

        let summary_only = build_plan(
            &pool,
            &inputs(),
            &request(
                ExportScope::Meeting {
                    meeting_id: "m1".to_string(),
                },
                ExportContents {
                    transcript: false,
                    summary: true,
                    attachments: false,
                    audio: false,
                },
            ),
        )
        .await
        .expect("plan builds");

        assert_eq!(paths(&summary_only), vec!["summary.md"]);
    }

    #[tokio::test]
    async fn project_plan_excludes_soft_deleted_meetings() {
        let pool = migrated_pool().await;
        insert_project(&pool, "p1", "Q3 Planning").await;
        for (id, title, date) in [
            ("m1", "Kickoff", "2026-06-02T09:00:00+00:00"),
            ("m2", "Design review", "2026-06-09T09:00:00+00:00"),
            ("m3", "Trashed one", "2026-06-16T09:00:00+00:00"),
        ] {
            insert_meeting(&pool, id, title, date, Some("p1")).await;
            insert_transcript(&pool, id, "content", 1.0).await;
        }
        sqlx::query("UPDATE meetings SET deleted_at = ? WHERE id = 'm3'")
            .bind("2026-06-17T09:00:00+00:00")
            .execute(&pool)
            .await
            .unwrap();

        let bundle = build_plan(
            &pool,
            &inputs(),
            &request(
                ExportScope::Project {
                    project_id: "p1".to_string(),
                    meeting_ids: vec!["m1".into(), "m2".into(), "m3".into()],
                },
                all_contents(),
            ),
        )
        .await
        .expect("plan builds");

        let paths = paths(&bundle);
        assert_eq!(paths[0], "README.md");
        assert!(paths.contains(&"2026-06-02 Kickoff/transcript.md".to_string()));
        assert!(paths.contains(&"2026-06-09 Design review/transcript.md".to_string()));
        assert!(
            !paths.iter().any(|p| p.contains("Trashed")),
            "soft-deleted meetings never reach the archive: {paths:?}"
        );
        assert_eq!(bundle.plan.meetings, 2);
        assert_eq!(bundle.suggested_filename, "Q3 Planning.zip");
    }

    #[tokio::test]
    async fn project_plan_honors_the_meeting_subset() {
        let pool = migrated_pool().await;
        insert_project(&pool, "p1", "Q3 Planning").await;
        for (id, title, date) in [
            ("m1", "Kickoff", "2026-06-02T09:00:00+00:00"),
            ("m2", "Design review", "2026-06-09T09:00:00+00:00"),
        ] {
            insert_meeting(&pool, id, title, date, Some("p1")).await;
            insert_transcript(&pool, id, "content", 1.0).await;
        }

        let bundle = build_plan(
            &pool,
            &inputs(),
            &request(
                ExportScope::Project {
                    project_id: "p1".to_string(),
                    meeting_ids: vec!["m2".into()],
                },
                all_contents(),
            ),
        )
        .await
        .expect("plan builds");

        let paths = paths(&bundle);
        assert_eq!(paths.len(), 2, "README + one transcript: {paths:?}");
        assert!(paths.contains(&"2026-06-09 Design review/transcript.md".to_string()));
        assert_eq!(bundle.plan.meetings, 1);
    }

    #[tokio::test]
    async fn project_plan_dedupes_colliding_folder_names() {
        let pool = migrated_pool().await;
        insert_project(&pool, "p1", "Q3 Planning").await;
        for id in ["m1", "m2"] {
            insert_meeting(&pool, id, "Kickoff", "2026-06-02T09:00:00+00:00", Some("p1")).await;
            insert_transcript(&pool, id, "content", 1.0).await;
        }

        let bundle = build_plan(
            &pool,
            &inputs(),
            &request(
                ExportScope::Project {
                    project_id: "p1".to_string(),
                    meeting_ids: vec!["m1".into(), "m2".into()],
                },
                all_contents(),
            ),
        )
        .await
        .expect("plan builds");

        let paths = paths(&bundle);
        assert!(paths.contains(&"2026-06-02 Kickoff/transcript.md".to_string()), "{paths:?}");
        assert!(paths.contains(&"2026-06-02 Kickoff-2/transcript.md".to_string()), "{paths:?}");
    }

    #[tokio::test]
    async fn project_readme_lists_meetings_that_contributed_nothing() {
        let pool = migrated_pool().await;
        insert_project(&pool, "p1", "Q3 Planning").await;
        insert_meeting(&pool, "m1", "Kickoff", "2026-06-02T09:00:00+00:00", Some("p1")).await;
        insert_transcript(&pool, "m1", "content", 1.0).await;
        insert_meeting(&pool, "m2", "Silent", "2026-06-09T09:00:00+00:00", Some("p1")).await;

        let bundle = build_plan(
            &pool,
            &inputs(),
            &request(
                ExportScope::Project {
                    project_id: "p1".to_string(),
                    meeting_ids: vec!["m1".into(), "m2".into()],
                },
                all_contents(),
            ),
        )
        .await
        .expect("plan builds");

        let readme = match &bundle.plan.entries[0] {
            PlannedEntry::Text { body, .. } => body.clone(),
            other => panic!("expected README first, got {other:?}"),
        };
        assert!(readme.contains("Silent"));
        assert!(readme.contains("(no exported content)"));
        assert_eq!(bundle.plan.meetings, 1, "only Kickoff contributed files");
    }

    #[tokio::test]
    async fn availability_reports_per_meeting_counts() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1", "Weekly Sync", "2026-06-02T09:00:00+00:00", None).await;
        insert_transcript(&pool, "m1", "one", 1.0).await;
        insert_transcript(&pool, "m1", "two", 2.0).await;

        let availability = build_availability(
            &pool,
            &inputs(),
            &ExportScope::Meeting {
                meeting_id: "m1".to_string(),
            },
        )
        .await
        .expect("availability builds");

        assert!(availability.project.is_none());
        assert_eq!(availability.meetings.len(), 1);
        let row = &availability.meetings[0];
        assert_eq!(row.transcript_segments, 2);
        assert!(!row.has_summary);
        assert_eq!(row.attachment_count, 0);
        assert_eq!(row.audio_bytes, None);
    }

    #[tokio::test]
    async fn availability_lists_project_meetings_oldest_first() {
        let pool = migrated_pool().await;
        insert_project(&pool, "p1", "Q3 Planning").await;
        insert_meeting(&pool, "m1", "Kickoff", "2026-06-02T09:00:00+00:00", Some("p1")).await;
        insert_meeting(&pool, "m2", "Design review", "2026-06-09T09:00:00+00:00", Some("p1")).await;

        let availability = build_availability(
            &pool,
            &inputs(),
            &ExportScope::Project {
                project_id: "p1".to_string(),
                meeting_ids: vec![],
            },
        )
        .await
        .expect("availability builds");

        assert_eq!(availability.project.unwrap().name, "Q3 Planning");
        let titles: Vec<_> = availability
            .meetings
            .iter()
            .map(|m| m.title.as_str())
            .collect();
        assert_eq!(titles, vec!["Kickoff", "Design review"]);
    }

    /// End-to-end over the one seam the unit tests cannot reach: composing
    /// `{attachments_root}/{meeting_id}/{stored_name}` the same way
    /// `api::attachments_api::meeting_attachments_dir` wrote it, then actually
    /// zipping the result and reading it back.
    #[tokio::test]
    async fn plans_and_zips_real_attachments_from_disk() {
        use super::super::zipper::write_zip;
        use std::io::Read;

        let pool = migrated_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let attachments_root = tmp.path().join("attachments");

        insert_meeting(&pool, "m1", "Weekly Sync", "2026-06-02T09:00:00+00:00", None).await;
        insert_transcript(&pool, "m1", "hello there", 5.0).await;

        // Two attachments sharing a display name, plus one whose file is gone.
        let dir = attachments_root.join("m1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a1.png"), b"first-bytes").unwrap();
        std::fs::write(dir.join("a2.png"), b"second-bytes").unwrap();
        for (id, stored) in [("a1", "a1.png"), ("a2", "a2.png"), ("a3", "missing.png")] {
            sqlx::query(
                "INSERT INTO meeting_attachments                  (id, meeting_id, file_name, stored_name, mime_type, size_bytes, created_at)                  VALUES (?, 'm1', 'photo.png', ?, 'image/png', 11, ?)",
            )
            .bind(id)
            .bind(stored)
            .bind("2026-06-02T09:00:00+00:00")
            .execute(&pool)
            .await
            .unwrap();
        }

        let inputs = PlanInputs {
            attachments_root,
            exported_at_rfc3339: "2026-08-19T14:03:11+00:00".to_string(),
        };
        let bundle = build_plan(
            &pool,
            &inputs,
            &request(
                ExportScope::Meeting {
                    meeting_id: "m1".to_string(),
                },
                all_contents(),
            ),
        )
        .await
        .expect("plan builds");

        // The vanished file is reported once, at plan time, and never queued.
        assert_eq!(bundle.plan.warnings.len(), 1, "{:?}", bundle.plan.warnings);
        assert!(bundle.plan.warnings[0].contains("photo.png"));

        // The dialog must not promise the file that vanished: availability
        // sizes attachments from disk, not from the (now stale) DB rows.
        let availability = build_availability(
            &pool,
            &inputs,
            &ExportScope::Meeting {
                meeting_id: "m1".to_string(),
            },
        )
        .await
        .expect("availability builds");
        assert_eq!(availability.meetings[0].attachment_count, 2, "the missing file is not counted");
        assert_eq!(
            availability.meetings[0].attachment_bytes,
            (b"first-bytes".len() + b"second-bytes".len()) as u64
        );

        let dest = tmp.path().join("out.zip");
        let stats = write_zip(bundle.plan, &dest).expect("zip writes");
        assert_eq!(stats.files_written, 3, "transcript + two attachments");

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&dest).unwrap()).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.contains(&"transcript.md".to_string()), "{names:?}");
        // Same display name twice -> the stem is suffixed, the extension kept.
        assert!(names.contains(&"files/photo.png".to_string()), "{names:?}");
        assert!(names.contains(&"files/photo-2.png".to_string()), "{names:?}");

        let mut bytes = Vec::new();
        zip.by_name("files/photo.png")
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes, b"first-bytes");
    }

    #[tokio::test]
    async fn a_trashed_meeting_is_still_exportable_and_says_so() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1", "Weekly Sync", "2026-06-02T09:00:00+00:00", None).await;
        insert_transcript(&pool, "m1", "hello", 1.0).await;
        sqlx::query("UPDATE meetings SET deleted_at = ? WHERE id = 'm1'")
            .bind("2026-06-17T09:00:00+00:00")
            .execute(&pool)
            .await
            .unwrap();

        let bundle = build_plan(
            &pool,
            &inputs(),
            &request(
                ExportScope::Meeting {
                    meeting_id: "m1".to_string(),
                },
                all_contents(),
            ),
        )
        .await
        .expect("a meeting the user named is exportable even from the trash");

        match &bundle.plan.entries[0] {
            PlannedEntry::Text { body, .. } => assert!(body.contains("trashed: true"), "{body}"),
            other => panic!("expected transcript text, got {other:?}"),
        }
    }

    #[test]
    fn scope_deserializes_from_the_frontend_shape() {
        let meeting: ExportScope =
            serde_json::from_str(r#"{"kind":"meeting","meetingId":"m1"}"#).unwrap();
        assert!(matches!(meeting, ExportScope::Meeting { meeting_id } if meeting_id == "m1"));

        let project: ExportScope = serde_json::from_str(
            r#"{"kind":"project","projectId":"p1","meetingIds":["m1","m2"]}"#,
        )
        .unwrap();
        match project {
            ExportScope::Project {
                project_id,
                meeting_ids,
            } => {
                assert_eq!(project_id, "p1");
                assert_eq!(meeting_ids, vec!["m1", "m2"]);
            }
            other => panic!("expected project scope, got {other:?}"),
        }
    }

    #[test]
    fn transcript_format_defaults_to_speakers_on_when_omitted() {
        let req: ExportBundleRequest = serde_json::from_str(
            r#"{"scope":{"kind":"meeting","meetingId":"m1"},
                "contents":{"transcript":true,"summary":true,"attachments":true,"audio":false}}"#,
        )
        .unwrap();
        assert!(req.transcript_format.include_speakers);
        assert!(!req.transcript_format.include_timestamps);
    }
}
