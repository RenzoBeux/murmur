//! Test-only helpers shared across the database repository tests.
//!
//! `migrated_pool()` builds an in-memory SQLite pool with ALL real migrations
//! applied, so repository tests exercise the true schema instead of ad-hoc DDL.

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

/// An in-memory SQLite pool with every migration in `./migrations` applied.
///
/// `max_connections(1)` is REQUIRED: an in-memory database is per-connection, so a
/// multi-connection pool would hand out connections that each see an empty schema.
pub(crate) async fn migrated_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("run migrations on in-memory pool");
    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn harness_applies_all_migrations() {
        // Proves the whole migration set runs on a fresh DB and the FTS/pragma/etc.
        // features they use are compiled into the bundled sqlite.
        let pool = migrated_pool().await;
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn every_migration_is_recorded_and_key_columns_exist() {
        let pool = migrated_pool().await;

        // The number of applied migrations matches the embedded migration set,
        // so a half-applied schema (or a migration that failed silently) fails
        // loudly. Comparing to the embedded count means this test auto-updates
        // when a migration is added.
        let migrator = sqlx::migrate!("./migrations");
        let expected = migrator.iter().count() as i64;
        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(applied, expected, "every embedded migration should be applied");

        // Spot-check columns/tables from later migrations so a dropped migration
        // is caught by a concrete schema assertion, not just the count.
        for (table, column) in [
            ("meetings", "folder_path"),
            ("meetings", "attendees"),
            ("meetings", "deleted_at"),
            ("transcripts", "speaker"),
            ("transcripts", "audio_start_time"),
            ("summary_processes", "result_backup"),
        ] {
            assert!(
                column_exists(&pool, table, column).await,
                "expected column {table}.{column} to exist"
            );
        }

        for table in ["meeting_notes", "chat_messages"] {
            let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap_or(-1);
            assert_eq!(n, 0, "expected table {table} to exist and be empty");
        }
    }

    async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> bool {
        // Table names here are hardcoded literals, so the format! is injection-safe.
        let names: Vec<String> =
            sqlx::query_scalar(&format!("SELECT name FROM pragma_table_info('{table}')"))
                .fetch_all(pool)
                .await
                .unwrap();
        names.iter().any(|n| n == column)
    }
}
