use chrono::Utc;
use sqlx::{migrate::MigrateDatabase, Result, Sqlite, SqlitePool, Transaction};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::Manager;

/// Surfaced to the UI when startup WAL-corruption recovery ran, so the user learns
/// their newest data was quarantined (not deleted) rather than silently set aside.
#[derive(Clone, serde::Serialize)]
pub struct RecoveryNotice {
    /// Path to the pre-recovery copy of the main DB, if one was made.
    pub backup_path: Option<String>,
    /// Paths the corrupt `-wal`/`-shm` files were renamed to.
    pub quarantined: Vec<String>,
    pub recovered: bool,
}

#[derive(Clone)]
pub struct DatabaseManager {
    pool: SqlitePool,
    /// Set when startup WAL-corruption recovery ran; consumed once by the setup/import
    /// paths to emit a `database-recovered` event.
    pub recovery_notice: Option<RecoveryNotice>,
}

impl DatabaseManager {
    pub async fn new(tauri_db_path: &str, backend_db_path: &str) -> Result<Self> {
        if let Some(parent_dir) = Path::new(tauri_db_path).parent() {
            if !parent_dir.exists() {
                fs::create_dir_all(parent_dir).map_err(sqlx::Error::Io)?;
            }
        }

        if !Path::new(tauri_db_path).exists() {
            if Path::new(backend_db_path).exists() {
                log::info!(
                    "Copying database from {} to {}",
                    backend_db_path,
                    tauri_db_path
                );
                // Copy the legacy .db AND its -wal/-shm sidecars (renamed to the .sqlite
                // stem) so un-checkpointed rows in the legacy WAL are not lost on import.
                Self::copy_db_with_sidecars(Path::new(backend_db_path), Path::new(tauri_db_path))
                    .map_err(sqlx::Error::Io)?;
            } else {
                log::info!("Creating database at {}", tauri_db_path);
                Sqlite::create_database(tauri_db_path).await?;
            }
        }

        let pool = SqlitePool::connect(tauri_db_path).await?;

        // Snapshot the DB *before* applying pending migrations, so a migration that
        // corrupts data on this launch can be recovered from a prior-state copy. Only
        // when the DB pre-existed with applied migrations AND a newer migration is
        // pending (skip fresh creates and no-op launches, or the backups dir balloons).
        // Best-effort: a snapshot failure never blocks startup — only migrate() errors.
        let migrator = sqlx::migrate!("./migrations");
        let applied: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap_or(None);
        let latest = migrator.iter().map(|m| m.version).max();
        if let (Some(a), Some(l)) = (applied, latest) {
            if l > a {
                Self::snapshot_pre_migration(&pool, tauri_db_path, a).await;
            }
        }
        migrator.run(&pool).await?;

        Ok(DatabaseManager {
            pool,
            recovery_notice: None,
        })
    }

    // NOTE: So for the first time users they needs to start the application
    // after they can just delete the existing .sqlite file and then copy the existing .db file to
    // the current app dir, So the system detects legacy db and copy it and starts with that data
    // (Newly created .sqlite with the copied content from .db)
    pub async fn new_from_app_handle(app_handle: &tauri::AppHandle) -> Result<Self> {
        // Resolve the app's data directory
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");
        if !app_data_dir.exists() {
            fs::create_dir_all(&app_data_dir).map_err(sqlx::Error::Io)?;
        }

        // Define database paths
        let tauri_db_path = app_data_dir
            .join("meeting_minutes.sqlite")
            .to_string_lossy()
            .to_string();
        // Legacy backend DB path (for auto-migration if exists)
        let backend_db_path = app_data_dir
            .join("meeting_minutes.db")
            .to_string_lossy()
            .to_string();

        // WAL file paths for defensive cleanup
        let wal_path = app_data_dir.join("meeting_minutes.sqlite-wal");
        let shm_path = app_data_dir.join("meeting_minutes.sqlite-shm");

        log::info!("Tauri DB path: {}", tauri_db_path);
        log::info!("Legacy backend DB path: {}", backend_db_path);

        // Try to open database with defensive WAL handling
        match Self::new(&tauri_db_path, &backend_db_path).await {
            Ok(db_manager) => {
                log::info!("Database opened successfully");
                Ok(db_manager)
            }
            Err(e) => {
                // Check if error is due to corrupted WAL file
                let error_msg = e.to_string();
                if error_msg.contains("malformed") || error_msg.contains("corrupt") {
                    log::warn!("Database appears corrupted, likely due to orphaned WAL file. Attempting recovery...");
                    log::warn!("Error details: {}", error_msg);

                    // QUARANTINE, don't delete. This branch fires exactly after a crash —
                    // the case where the -wal may hold the newest committed meetings.
                    // Deleting it destroyed that data with no undo. Instead we snapshot the
                    // main DB and rename the wal/shm aside so recovery is still possible,
                    // and capture what we did into a RecoveryNotice so the UI can tell the user.
                    let ts = Utc::now().format("%Y%m%d-%H%M%S");
                    let main_path = Path::new(&tauri_db_path);
                    let mut backup_path: Option<String> = None;
                    let mut quarantined: Vec<String> = Vec::new();
                    if main_path.exists() {
                        let backup = format!("{}.corrupt-{}.bak", tauri_db_path, ts);
                        match fs::copy(main_path, &backup) {
                            Ok(_) => {
                                log::warn!("Backed up main DB before recovery: {}", backup);
                                backup_path = Some(backup);
                            }
                            Err(e) => log::warn!("Failed to back up main DB before recovery: {}", e),
                        }
                    }
                    if wal_path.exists() {
                        let dest = wal_path.with_extension(format!("sqlite-wal.corrupt-{}.bak", ts));
                        match fs::rename(&wal_path, &dest) {
                            Ok(_) => {
                                log::warn!("Quarantined WAL file to: {:?}", dest);
                                quarantined.push(dest.to_string_lossy().to_string());
                            }
                            Err(e) => log::warn!("Failed to quarantine WAL file: {}", e),
                        }
                    }
                    if shm_path.exists() {
                        let dest = shm_path.with_extension(format!("sqlite-shm.corrupt-{}.bak", ts));
                        match fs::rename(&shm_path, &dest) {
                            Ok(_) => {
                                log::warn!("Quarantined SHM file to: {:?}", dest);
                                quarantined.push(dest.to_string_lossy().to_string());
                            }
                            Err(e) => log::warn!("Failed to quarantine SHM file: {}", e),
                        }
                    }

                    // Retry connection after quarantining WAL files
                    log::info!("Retrying database connection after WAL quarantine...");
                    match Self::new(&tauri_db_path, &backend_db_path).await {
                        Ok(mut db_manager) => {
                            log::info!("Database opened successfully after WAL recovery");
                            db_manager.recovery_notice = Some(RecoveryNotice {
                                backup_path,
                                quarantined,
                                recovered: true,
                            });
                            Ok(db_manager)
                        }
                        Err(retry_err) => {
                            log::error!(
                                "Database connection failed even after WAL cleanup: {}",
                                retry_err
                            );
                            Err(retry_err)
                        }
                    }
                } else {
                    // Not a WAL-related error, propagate original error
                    log::error!("Database connection failed: {}", error_msg);
                    Err(e)
                }
            }
        }
    }

    /// Check if this is the first launch (sqlite database doesn't exist yet)
    pub async fn is_first_launch(app_handle: &tauri::AppHandle) -> Result<bool> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");

        let tauri_db_path = app_data_dir.join("meeting_minutes.sqlite");

        Ok(!tauri_db_path.exists())
    }

    /// Import a legacy database from the specified path and initialize
    pub async fn import_legacy_database(
        app_handle: &tauri::AppHandle,
        legacy_db_path: &str,
    ) -> Result<Self> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");

        if !app_data_dir.exists() {
            fs::create_dir_all(&app_data_dir).map_err(sqlx::Error::Io)?;
        }

        // Fail loud if a real database already exists. new() only imports the legacy .db
        // into a *non-existent* .sqlite; if a real .sqlite is already present the legacy
        // data would be silently ignored while this command reports success — misleading
        // the user into thinking their old history was imported. Block that instead.
        let target_sqlite_path = app_data_dir.join("meeting_minutes.sqlite");
        if let Ok(meta) = fs::metadata(&target_sqlite_path) {
            // A real DB is a 100-byte header + at least one 4096-byte page. A near-empty
            // placeholder from an aborted first run is not a genuine conflict.
            if meta.len() > 4096 {
                return Err(sqlx::Error::Protocol(format!(
                    "A database already exists at {}. Importing would risk overwriting your current meetings. \
                     Back up and remove the current database before importing the legacy one.",
                    target_sqlite_path.display()
                )));
            }
        }

        // Copy legacy database to app data directory as meeting_minutes.db
        let target_legacy_path = app_data_dir.join("meeting_minutes.db");

        // Guard against a same-path copy. Onboarding auto-detects the legacy DB at
        // exactly this target path, so legacy_db_path == target_legacy_path is common.
        // std::fs::copy truncates the destination before reading, so copying a file
        // onto itself zeroes it (data loss on macOS/Linux). If they resolve to the same
        // file, the legacy DB is already in place — skip the copy and just initialize.
        let same_file = fs::canonicalize(legacy_db_path)
            .ok()
            .zip(fs::canonicalize(&target_legacy_path).ok())
            .map(|(a, b)| a == b)
            .unwrap_or(false);

        if same_file {
            log::info!(
                "Legacy DB is already at the target path ({}); skipping self-copy",
                target_legacy_path.display()
            );
        } else {
            log::info!(
                "Copying legacy database from {} to {}",
                legacy_db_path,
                target_legacy_path.display()
            );
            // Copy the -wal/-shm sidecars too, so un-checkpointed rows in the legacy
            // WAL survive the import.
            Self::copy_db_with_sidecars(Path::new(legacy_db_path), &target_legacy_path)
                .map_err(sqlx::Error::Io)?;
        }

        // Now use the standard initialization which will detect and migrate the legacy db
        Self::new_from_app_handle(app_handle).await
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Copy a SQLite DB and its `-wal`/`-shm` sidecars, renaming the sidecars to the
    /// destination's stem so SQLite associates them with the copied DB. SQLite pairs a
    /// `-wal` with its DB by filename, so `foo.db-wal` must become `foo.sqlite-wal` when
    /// copying `foo.db` -> `foo.sqlite`, or the un-checkpointed data in it is ignored
    /// (silent loss of the newest rows). The `-shm` is a derived cache; a copy failure
    /// there is non-fatal.
    fn copy_db_with_sidecars(src: &Path, dst: &Path) -> std::io::Result<()> {
        fs::copy(src, dst)?;
        for suffix in ["-wal", "-shm"] {
            let mut src_side = src.as_os_str().to_os_string();
            src_side.push(suffix);
            let src_side = PathBuf::from(src_side);
            if src_side.exists() {
                let mut dst_side = dst.as_os_str().to_os_string();
                dst_side.push(suffix);
                let dst_side = PathBuf::from(dst_side);
                if let Err(e) = fs::copy(&src_side, &dst_side) {
                    log::warn!("Failed to copy DB sidecar {:?}: {}", src_side, e);
                }
            }
        }
        Ok(())
    }

    /// Best-effort `VACUUM INTO` snapshot into `<db_dir>/backups/pre-migration/`,
    /// keeping the newest 5. Consistent even with an active WAL. Never blocks startup.
    async fn snapshot_pre_migration(pool: &SqlitePool, db_path: &str, from_version: i64) {
        let dir = match Path::new(db_path).parent() {
            Some(p) => p.join("backups").join("pre-migration"),
            None => return,
        };
        if let Err(e) = fs::create_dir_all(&dir) {
            log::warn!("Failed to create pre-migration snapshot dir {:?}: {}", dir, e);
            return;
        }
        let ts = Utc::now().format("%Y%m%d-%H%M%S");
        let dest = dir.join(format!("pre-migration-v{}-{}.sqlite", from_version, ts));
        // SQLite string literal: escape single quotes by doubling them.
        let dest_sql = dest.to_string_lossy().replace('\'', "''");
        match sqlx::query(&format!("VACUUM INTO '{}'", dest_sql))
            .execute(pool)
            .await
        {
            Ok(_) => log::info!("Pre-migration snapshot written: {:?}", dest),
            Err(e) => {
                log::warn!("Pre-migration snapshot failed (continuing): {}", e);
                return;
            }
        }
        Self::prune_snapshots(&dir, "pre-migration-", 5);
    }

    /// Keep only the newest `keep` `<name_prefix>*.sqlite` files in `dir`
    /// (timestamped names sort chronologically). Best-effort.
    fn prune_snapshots(dir: &Path, name_prefix: &str, keep: usize) {
        let mut snapshots: Vec<_> = match fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with(name_prefix) && n.ends_with(".sqlite"))
                        .unwrap_or(false)
                })
                .collect(),
            Err(e) => {
                log::warn!("Failed to list snapshots for pruning in {:?}: {}", dir, e);
                return;
            }
        };
        snapshots.sort();
        if snapshots.len() > keep {
            for old in &snapshots[..snapshots.len() - keep] {
                match fs::remove_file(old) {
                    Ok(_) => log::info!("Pruned old snapshot: {:?}", old),
                    Err(e) => log::warn!("Failed to prune old snapshot {:?}: {}", old, e),
                }
            }
        }
    }

    /// Snapshot the live database to a rotating backup via `VACUUM INTO`, keeping the
    /// newest `keep` snapshots. `VACUUM INTO` produces a consistent copy even with an
    /// active WAL, so this is safe to run at startup. Best-effort: any failure is
    /// logged and swallowed so a backup problem never blocks the app.
    pub async fn backup_to_dir(&self, backups_dir: &Path, keep: usize) {
        if let Err(e) = fs::create_dir_all(backups_dir) {
            log::warn!("Failed to create DB backups dir {:?}: {}", backups_dir, e);
            return;
        }

        let ts = Utc::now().format("%Y%m%d-%H%M%S");
        let dest = backups_dir.join(format!("meeting_minutes-{}.sqlite", ts));
        // SQLite string literal: escape single quotes by doubling them.
        let dest_sql = dest.to_string_lossy().replace('\'', "''");

        match sqlx::query(&format!("VACUUM INTO '{}'", dest_sql))
            .execute(&self.pool)
            .await
        {
            Ok(_) => log::info!("Database backup created: {:?}", dest),
            Err(e) => {
                log::warn!("Database backup (VACUUM INTO) failed: {}", e);
                return;
            }
        }

        Self::prune_snapshots(backups_dir, "meeting_minutes-", keep);
    }

    pub async fn with_transaction<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Transaction<'_, Sqlite>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut tx = self.pool.begin().await?;
        let result = f(&mut tx).await;

        match result {
            Ok(val) => {
                tx.commit().await?;
                Ok(val)
            }
            Err(err) => {
                tx.rollback().await?;
                Err(err)
            }
        }
    }

    /// Cleanup database connection and checkpoint WAL
    /// This should be called on application shutdown to ensure:
    /// - All WAL changes are written to the main database file
    /// - The .wal and .shm files are deleted
    /// - Connection pool is gracefully closed
    pub async fn cleanup(&self) -> Result<()> {
        log::info!("Starting database cleanup...");

        // Force checkpoint of WAL to main database file and remove WAL file
        // TRUNCATE mode: checkpoints all pages AND deletes the WAL file
        match sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await
        {
            Ok(_) => log::info!("WAL checkpoint completed successfully"),
            Err(e) => log::warn!("WAL checkpoint failed (non-fatal): {}", e),
        }

        // Close the connection pool gracefully
        self.pool.close().await;
        log::info!("Database connection pool closed");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    #[tokio::test]
    async fn backup_to_dir_creates_snapshot_and_prunes_old() {
        let dir = std::env::temp_dir().join("murmur_backup_to_dir_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Use a file-backed source DB (VACUUM INTO copies real pages).
        let src_db = dir.join("source.sqlite");
        let src_url = format!("sqlite://{}?mode=rwc", src_db.to_string_lossy().replace('\\', "/"));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&src_url)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE t (x INTEGER)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO t (x) VALUES (1)").execute(&pool).await.unwrap();
        let mgr = DatabaseManager { pool, recovery_notice: None };

        // Seed older snapshots whose timestamped names sort before today's real one.
        for name in [
            "meeting_minutes-20250101-000001.sqlite",
            "meeting_minutes-20250101-000002.sqlite",
            "meeting_minutes-20250101-000003.sqlite",
        ] {
            fs::write(dir.join(name), b"old").unwrap();
        }

        // Creates one real VACUUM INTO snapshot, then prunes to keep the 2 newest.
        mgr.backup_to_dir(&dir, 2).await;

        let mut remaining: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| (e.file_name().to_string_lossy().to_string(), e.metadata().map(|m| m.len()).unwrap_or(0)))
            .filter(|(n, _)| n.starts_with("meeting_minutes-") && n.ends_with(".sqlite"))
            .collect();
        remaining.sort();
        eprintln!("remaining snapshots: {:?}", remaining);

        assert_eq!(remaining.len(), 2, "should keep exactly `keep` snapshots");
        // The freshly created backup (today's date) must survive pruning and be a
        // valid, non-empty SQLite file (not one of the 3-byte "old" seeds).
        let (newest_name, newest_len) = remaining.iter().max_by(|a, b| a.0.cmp(&b.0)).unwrap();
        assert!(
            *newest_len > 3,
            "newest snapshot {} should be a real DB copy, was {} bytes",
            newest_name,
            newest_len
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn copy_db_with_sidecars_moves_wal_only_rows() {
        let dir = std::env::temp_dir().join("murmur_copy_sidecars_test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Build a WAL-mode source DB and leave a row un-checkpointed in the -wal.
        let src = dir.join("legacy.db");
        let src_url = format!("sqlite://{}?mode=rwc", src.to_string_lossy().replace('\\', "/"));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&src_url)
            .await
            .unwrap();
        sqlx::query("PRAGMA journal_mode=WAL").execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE t (x INTEGER)").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO t (x) VALUES (42)").execute(&pool).await.unwrap();
        // Do NOT checkpoint; keep the pool (and its -wal) alive across the copy.
        assert!(src.with_extension("db-wal").exists() || dir.join("legacy.db-wal").exists());

        // Copy to a differently-stemmed destination.
        let dst = dir.join("current.sqlite");
        DatabaseManager::copy_db_with_sidecars(&src, &dst).unwrap();
        assert!(dst.exists(), "main DB copied");
        assert!(dir.join("current.sqlite-wal").exists(), "wal sidecar renamed to dest stem");

        // Opening the destination must see the wal-only row.
        let dst_url = format!("sqlite://{}?mode=ro", dst.to_string_lossy().replace('\\', "/"));
        let dst_pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&dst_url)
            .await
            .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT x FROM t").fetch_one(&dst_pool).await.unwrap();
        assert_eq!(n, 42, "wal-only row must be present in the copy");

        dst_pool.close().await;
        pool.close().await;
        let _ = fs::remove_dir_all(&dir);
    }
}
