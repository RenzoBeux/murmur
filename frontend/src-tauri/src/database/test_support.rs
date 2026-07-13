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
}
