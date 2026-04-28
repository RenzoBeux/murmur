use log::{error, info};
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;

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
