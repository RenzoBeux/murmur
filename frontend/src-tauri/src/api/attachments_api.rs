use log::{error as log_error, info as log_info, warn as log_warn};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::DialogExt;

use crate::audio::audio_processing::sanitize_filename;
use crate::database::models::MeetingAttachmentModel;
use crate::database::repositories::attachment::AttachmentsRepository;
use crate::database::repositories::meeting::MeetingsRepository;
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize)]
pub struct AttachmentDto {
    pub id: String,
    pub meeting_id: String,
    pub file_name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub created_at: String,
    pub is_image: bool,
    /// Absolute path for `convertFileSrc` display only; mutating commands
    /// always go by attachment id and re-resolve the path from the DB.
    pub absolute_path: String,
}

fn to_dto<R: Runtime>(
    app: &AppHandle<R>,
    model: MeetingAttachmentModel,
) -> Result<AttachmentDto, String> {
    let dir = meeting_attachments_dir(app, &model.meeting_id)?;
    Ok(AttachmentDto {
        is_image: model.mime_type.starts_with("image/"),
        absolute_path: dir.join(&model.stored_name).to_string_lossy().to_string(),
        id: model.id,
        meeting_id: model.meeting_id,
        file_name: model.file_name,
        mime_type: model.mime_type,
        size_bytes: model.size_bytes,
        created_at: model.created_at.to_rfc3339(),
    })
}

pub fn attachments_base_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data dir: {}", e))?
        .join("attachments"))
}

pub fn meeting_attachments_dir<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
) -> Result<PathBuf, String> {
    // Meeting ids are generated as "meeting-<uuid>" but sanitize anyway so a
    // hostile id can never escape the attachments root.
    let safe_id = sanitize_filename(meeting_id);
    if safe_id.is_empty() || safe_id.contains("..") {
        return Err(format!("Invalid meeting id: {}", meeting_id));
    }
    Ok(attachments_base_dir(app)?.join(safe_id))
}

/// Best-effort removal of a meeting's attachment files after a hard purge.
/// Never fails the caller — the DB rows are already gone.
pub fn remove_meeting_attachment_files<R: Runtime>(app: &AppHandle<R>, meeting_id: &str) {
    let dir = match meeting_attachments_dir(app, meeting_id) {
        Ok(dir) => dir,
        Err(e) => {
            log_warn!("Skipping attachment cleanup for {}: {}", meeting_id, e);
            return;
        }
    };
    if !dir.exists() {
        return;
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => log_info!("Removed attachment folder {}", dir.display()),
        Err(e) => log_warn!(
            "Failed to remove attachment folder {}: {}",
            dir.display(),
            e
        ),
    }
}

/// Infer a MIME type from the file extension. Intentionally small: unknown
/// extensions fall back to octet-stream, which the UI treats as "generic file".
fn mime_from_extension(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "pdf" => "application/pdf",
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// Pick a collision-free filename inside `dir` for `original_name`, keeping the
/// extension and suffixing `-1`, `-2`, … on the stem when taken.
fn collision_free_name(dir: &Path, original_name: &str) -> Result<String, String> {
    let source = Path::new(original_name);
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let raw_stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("attachment");
    let mut stem = sanitize_filename(raw_stem);
    if stem.is_empty() || stem == "." || stem == ".." {
        stem = "attachment".to_string();
    }
    // Keep names comfortably under filesystem limits even with a suffix.
    if stem.len() > 100 {
        let mut end = 100;
        while !stem.is_char_boundary(end) {
            end -= 1;
        }
        stem.truncate(end);
    }

    let make_name = |stem: &str, n: u32| {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!("-{}", n)
        };
        if ext.is_empty() {
            format!("{}{}", stem, suffix)
        } else {
            format!("{}{}.{}", stem, suffix, ext)
        }
    };

    for n in 0..10_000 {
        let candidate = make_name(&stem, n);
        if !dir.join(&candidate).exists() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "Could not find a free filename for {} in {}",
        original_name,
        dir.display()
    ))
}

/// Copy one source file into the meeting's attachments dir. Returns
/// (display_name, stored_name, mime, size_bytes).
async fn ingest_file(dir: &Path, source: &Path) -> Result<(String, String, String, i64), String> {
    let meta = tokio::fs::metadata(source)
        .await
        .map_err(|e| format!("{}: cannot read ({})", source.display(), e))?;
    if !meta.is_file() {
        return Err(format!("{}: not a file", source.display()));
    }

    let display_name = source
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_string())
        .unwrap_or_else(|| "attachment".to_string());

    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("Failed to create attachments folder: {}", e))?;

    let stored_name = collision_free_name(dir, &display_name)?;
    let dest = dir.join(&stored_name);
    let copied = tokio::fs::copy(source, &dest)
        .await
        .map_err(|e| format!("{}: copy failed ({})", display_name, e))?;

    let mime = mime_from_extension(Path::new(&display_name)).to_string();
    Ok((display_name, stored_name, mime, copied as i64))
}

/// Copy the given files into the meeting's attachment folder and insert rows.
/// Partial success returns Ok with the successes; Err only when nothing worked.
async fn add_paths_for_meeting<R: Runtime>(
    app: &AppHandle<R>,
    state: &tauri::State<'_, AppState>,
    meeting_id: &str,
    paths: &[String],
) -> Result<Vec<AttachmentDto>, String> {
    let pool = state.db_manager.pool();

    match MeetingsRepository::get_meeting(pool, meeting_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Err(format!("Meeting not found: {}", meeting_id)),
        Err(e) => return Err(format!("Database error: {}", e)),
    }

    let dir = meeting_attachments_dir(app, meeting_id)?;
    let mut added: Vec<AttachmentDto> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for path in paths {
        let source = PathBuf::from(path);
        match ingest_file(&dir, &source).await {
            Ok((display_name, stored_name, mime, size_bytes)) => {
                match AttachmentsRepository::add(
                    pool,
                    meeting_id,
                    &display_name,
                    &stored_name,
                    &mime,
                    size_bytes,
                )
                .await
                {
                    Ok(model) => match to_dto(app, model) {
                        Ok(dto) => added.push(dto),
                        Err(e) => failures.push(e),
                    },
                    Err(e) => {
                        // Roll the orphaned copy back so files and rows stay in sync.
                        let _ = tokio::fs::remove_file(dir.join(&stored_name)).await;
                        failures.push(format!("{}: db insert failed ({})", display_name, e));
                    }
                }
            }
            Err(e) => failures.push(e),
        }
    }

    if added.is_empty() && !failures.is_empty() {
        return Err(failures.join("; "));
    }
    for failure in &failures {
        log_warn!("Attachment skipped: {}", failure);
    }
    log_info!(
        "Added {} attachment(s) to meeting {} ({} skipped)",
        added.len(),
        meeting_id,
        failures.len()
    );
    Ok(added)
}

/// Open a native multi-select picker and attach the chosen files.
/// Returns an empty Vec when the user cancels.
#[tauri::command]
pub async fn api_add_attachments<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<AttachmentDto>, String> {
    let app_clone = app.clone();
    let picked = tokio::task::spawn_blocking(move || {
        app_clone.dialog().file().blocking_pick_files()
    })
    .await
    .map_err(|e| format!("File dialog task failed: {}", e))?;

    let Some(picked) = picked else {
        return Ok(Vec::new());
    };
    let paths: Vec<String> = picked.into_iter().map(|p| p.to_string()).collect();
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    add_paths_for_meeting(&app, &state, &meeting_id, &paths).await
}

/// Attach files from known paths (drag-drop).
#[tauri::command]
pub async fn api_add_attachments_from_paths<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    paths: Vec<String>,
) -> Result<Vec<AttachmentDto>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    add_paths_for_meeting(&app, &state, &meeting_id, &paths).await
}

#[tauri::command]
pub async fn api_list_attachments<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<AttachmentDto>, String> {
    let pool = state.db_manager.pool();
    let models = AttachmentsRepository::list_for_meeting(pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to list attachments: {}", e))?;
    models
        .into_iter()
        .map(|m| to_dto(&app, m))
        .collect::<Result<Vec<_>, _>>()
}

/// Resolve an attachment row to its on-disk path, rejecting stored names that
/// could escape the meeting's attachments folder.
async fn resolve_attachment_path<R: Runtime>(
    app: &AppHandle<R>,
    state: &tauri::State<'_, AppState>,
    attachment_id: &str,
) -> Result<(MeetingAttachmentModel, PathBuf), String> {
    let pool = state.db_manager.pool();
    let model = AttachmentsRepository::get(pool, attachment_id)
        .await
        .map_err(|e| format!("Database error: {}", e))?
        .ok_or_else(|| format!("Attachment not found: {}", attachment_id))?;

    if model.stored_name.contains('/')
        || model.stored_name.contains('\\')
        || model.stored_name.contains("..")
    {
        return Err("Invalid attachment path".to_string());
    }

    let path = meeting_attachments_dir(app, &model.meeting_id)?.join(&model.stored_name);
    Ok((model, path))
}

/// Raw bytes of an attachment — used for image preview fallbacks and for
/// feeding image attachments to vision-capable LLM providers.
#[tauri::command]
pub async fn api_read_attachment_file<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    attachment_id: String,
) -> Result<Vec<u8>, String> {
    let (model, path) = resolve_attachment_path(&app, &state, &attachment_id).await?;
    tokio::fs::read(&path)
        .await
        .map_err(|e| format!("Failed to read {}: {}", model.file_name, e))
}

/// Delete an attachment: row first, then best-effort file removal.
#[tauri::command]
pub async fn api_delete_attachment<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    attachment_id: String,
) -> Result<(), String> {
    let (model, path) = resolve_attachment_path(&app, &state, &attachment_id).await?;

    let pool = state.db_manager.pool();
    let deleted = AttachmentsRepository::delete(pool, &attachment_id)
        .await
        .map_err(|e| format!("Failed to delete attachment: {}", e))?;
    if !deleted {
        return Err(format!("Attachment not found: {}", attachment_id));
    }

    if let Err(e) = tokio::fs::remove_file(&path).await {
        log_warn!(
            "Attachment row {} deleted but file {} could not be removed: {}",
            attachment_id,
            path.display(),
            e
        );
    }
    log_info!(
        "Deleted attachment {} ({}) from meeting {}",
        attachment_id,
        model.file_name,
        model.meeting_id
    );
    Ok(())
}

/// Open an attachment with the system default application.
#[tauri::command]
pub async fn api_open_attachment<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    attachment_id: String,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let (model, path) = resolve_attachment_path(&app, &state, &attachment_id).await?;
    if !path.exists() {
        return Err(format!("Attachment file missing: {}", model.file_name));
    }
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| {
            log_error!("Failed to open attachment {}: {}", attachment_id, e);
            format!("Failed to open {}: {}", model.file_name, e)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_inference_covers_common_types() {
        assert_eq!(mime_from_extension(Path::new("a.PNG")), "image/png");
        assert_eq!(mime_from_extension(Path::new("a.jpeg")), "image/jpeg");
        assert_eq!(mime_from_extension(Path::new("a.pdf")), "application/pdf");
        assert_eq!(
            mime_from_extension(Path::new("noext")),
            "application/octet-stream"
        );
    }

    #[test]
    fn collision_free_name_suffixes_taken_names() {
        let dir = std::env::temp_dir().join(format!("murmur-attach-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("photo.png"), b"x").unwrap();
        std::fs::write(dir.join("photo-1.png"), b"x").unwrap();

        let name = collision_free_name(&dir, "photo.png").unwrap();
        assert_eq!(name, "photo-2.png");

        let fresh = collision_free_name(&dir, "other.txt").unwrap();
        assert_eq!(fresh, "other.txt");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
