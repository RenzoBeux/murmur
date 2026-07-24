//! Turns a meeting's attachments into LLM-ready context: image payloads for
//! vision-capable providers, a text block describing every attachment, and a
//! stable fingerprint for the summary cache. Shared by summary generation and
//! per-meeting chat. Attachment problems never fail the caller — they degrade
//! to text notes.

use base64::Engine;
use log::warn;
use sqlx::SqlitePool;
use tauri::{AppHandle, Runtime};

use crate::api::attachments_api::meeting_attachments_dir;
use crate::database::repositories::attachment::AttachmentsRepository;
use crate::summary::llm_client::ImageInput;
use crate::summary::service::stable_text_fingerprint;

/// Vision providers cap per-image size (Claude: 5 MB) — larger files are omitted.
const MAX_IMAGE_BYTES: u64 = 5 * 1024 * 1024;
/// At most this many images are sent per request.
const MAX_IMAGES: usize = 4;
/// Cumulative raw-byte budget across all sent images.
const MAX_TOTAL_IMAGE_BYTES: u64 = 15 * 1024 * 1024;
/// text/* attachments up to this size are inlined verbatim.
const MAX_INLINE_TEXT_BYTES: u64 = 32 * 1024;

/// Image media types the wire formats support.
fn is_supported_image_mime(mime: &str) -> bool {
    matches!(mime, "image/png" | "image/jpeg" | "image/webp" | "image/gif")
}

#[derive(Debug, Default)]
pub struct AttachmentLlmContext {
    /// Images to send to the model (already capped and base64-encoded).
    pub images: Vec<ImageInput>,
    /// Text block describing the attachments (empty when there are none).
    pub context_notes: String,
    /// Cache fingerprint over attachment identity ("" when there are none).
    pub fingerprint: String,
}

impl AttachmentLlmContext {
    pub fn notes(&self) -> Option<&str> {
        if self.context_notes.is_empty() {
            None
        } else {
            Some(&self.context_notes)
        }
    }
}

/// Load a meeting's attachments into LLM context. Never fails: DB or file
/// errors are logged and reflected as "not available" notes.
pub async fn build_attachment_context<R: Runtime>(
    app: &AppHandle<R>,
    pool: &SqlitePool,
    meeting_id: &str,
) -> AttachmentLlmContext {
    let rows = match AttachmentsRepository::list_for_meeting(pool, meeting_id).await {
        Ok(rows) => rows,
        Err(e) => {
            warn!("Failed to list attachments for {}: {}", meeting_id, e);
            return AttachmentLlmContext::default();
        }
    };
    if rows.is_empty() {
        return AttachmentLlmContext::default();
    }

    let dir = match meeting_attachments_dir(app, meeting_id) {
        Ok(dir) => dir,
        Err(e) => {
            warn!("Cannot resolve attachments dir for {}: {}", meeting_id, e);
            return AttachmentLlmContext::default();
        }
    };

    let mut images: Vec<ImageInput> = Vec::new();
    let mut file_lines: Vec<String> = Vec::new();
    let mut inlined_texts: Vec<String> = Vec::new();
    let mut fingerprint_lines: Vec<String> = Vec::new();
    let mut total_image_bytes: u64 = 0;

    for row in &rows {
        fingerprint_lines.push(format!(
            "{}|{}|{}|{}",
            row.id, row.file_name, row.mime_type, row.size_bytes
        ));
        let path = dir.join(&row.stored_name);

        if is_supported_image_mime(&row.mime_type) {
            let size = row.size_bytes.max(0) as u64;
            if size > MAX_IMAGE_BYTES {
                file_lines.push(format!(
                    "- {} ({}, omitted: larger than the 5 MB image limit)",
                    row.file_name, row.mime_type
                ));
                continue;
            }
            if images.len() >= MAX_IMAGES || total_image_bytes + size > MAX_TOTAL_IMAGE_BYTES {
                file_lines.push(format!(
                    "- {} ({}, omitted: image budget reached)",
                    row.file_name, row.mime_type
                ));
                continue;
            }
            match tokio::fs::read(&path).await {
                Ok(bytes) => {
                    total_image_bytes += bytes.len() as u64;
                    images.push(ImageInput {
                        media_type: row.mime_type.clone(),
                        base64_data: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    });
                    file_lines.push(format!("- {} ({}, shown as image)", row.file_name, row.mime_type));
                }
                Err(e) => {
                    warn!("Failed to read attachment {}: {}", path.display(), e);
                    file_lines.push(format!(
                        "- {} ({}, omitted: file could not be read)",
                        row.file_name, row.mime_type
                    ));
                }
            }
        } else if row.mime_type.starts_with("text/")
            && row.size_bytes >= 0
            && (row.size_bytes as u64) <= MAX_INLINE_TEXT_BYTES
        {
            match tokio::fs::read_to_string(&path).await {
                Ok(text) => {
                    inlined_texts.push(format!(
                        "<attachment name=\"{}\">\n{}\n</attachment>",
                        row.file_name, text
                    ));
                    file_lines.push(format!(
                        "- {} ({}, contents inlined below)",
                        row.file_name, row.mime_type
                    ));
                }
                Err(e) => {
                    warn!("Failed to read attachment {}: {}", path.display(), e);
                    file_lines.push(format!(
                        "- {} ({}, contents not available)",
                        row.file_name, row.mime_type
                    ));
                }
            }
        } else {
            file_lines.push(format!(
                "- {} ({}, contents not available)",
                row.file_name, row.mime_type
            ));
        }
    }

    let mut context_notes = format!("Attached files:\n{}", file_lines.join("\n"));
    if !inlined_texts.is_empty() {
        context_notes.push_str("\n\n");
        context_notes.push_str(&inlined_texts.join("\n\n"));
    }

    // Sorted so the fingerprint is insensitive to listing order.
    fingerprint_lines.sort();
    AttachmentLlmContext {
        images,
        context_notes,
        fingerprint: stable_text_fingerprint(&fingerprint_lines.join("\n")),
    }
}
