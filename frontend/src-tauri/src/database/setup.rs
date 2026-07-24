use log::info;
use tauri::{AppHandle, Emitter, Manager};

use super::manager::DatabaseManager;
use crate::state::AppState;

/// If startup WAL-corruption recovery ran, tell the user (after a short delay so the
/// window/React listeners exist, mirroring the `first-launch-detected` pattern) that
/// their newest data was quarantined as `.bak` files, not deleted.
pub fn emit_recovery_notice(app: &AppHandle, db_manager: &DatabaseManager) {
    if let Some(notice) = db_manager.recovery_notice.clone() {
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(600)).await;
            if let Err(e) = app_handle.emit("database-recovered", notice) {
                log::warn!("Failed to emit database-recovered event: {}", e);
            }
        });
    }
}

/// Initialize database on app startup
/// Handles first launch detection and conditional initialization
pub async fn initialize_database_on_startup(app: &AppHandle) -> Result<(), String> {
    // Check if this is the first launch (no database exists yet)
    let is_first_launch = DatabaseManager::is_first_launch(app)
        .await
        .map_err(|e| format!("Failed to check first launch status: {}", e))?;

    if is_first_launch {
        info!("First launch detected - will notify window when ready");

        // Delay event emission to ensure window is ready and React listeners are registered
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            app_handle
                .emit("first-launch-detected", ())
                .expect("Failed to emit first-launch-detected event");
            info!("Emitted first-launch-detected after delay");
        });
    } else {
        // Normal flow - initialize database immediately
        let db_manager = DatabaseManager::new_from_app_handle(app)
            .await
            .map_err(|e| format!("Failed to initialize database manager: {}", e))?;

        // If WAL-corruption recovery ran during open, notify the user.
        emit_recovery_notice(app, &db_manager);

        // Rotating startup backup (best-effort; keep the last 5 snapshots). Uses
        // VACUUM INTO, which is consistent even with an active WAL. This is the DB's
        // safety net: a corrupt-on-next-launch DB can be restored from here.
        if let Ok(app_data_dir) = app.path().app_data_dir() {
            db_manager
                .backup_to_dir(&app_data_dir.join("backups"), 5)
                .await;
        }

        // Reconcile summary processes stranded in a non-terminal state by a prior quit,
        // so a meeting doesn't show an eternal "Generating…" spinner.
        if let Err(e) = crate::database::repositories::summary::SummaryProcessesRepository::reset_orphaned_processes(db_manager.pool()).await {
            log::warn!("Failed to reset orphaned summary processes: {}", e);
        }

        // Empty the trash: permanently purge meetings soft-deleted more than 30 days
        // ago (cascading to their children). Best-effort — never blocks startup.
        match crate::database::repositories::meeting::MeetingsRepository::purge_trash_older_than(db_manager.pool(), 30).await {
            Ok(purged_ids) => {
                for id in &purged_ids {
                    crate::api::attachments_api::remove_meeting_attachment_files(app, id);
                }
            }
            Err(e) => log::warn!("Failed to purge old trashed meetings: {}", e),
        }

        app.manage(AppState { db_manager });
        info!("Database initialized successfully");
    }

    Ok(())
}
