use crate::database::models::SummaryProcess;
use chrono::Utc;
use serde_json::Value;
use sqlx::SqlitePool;
use tracing::{error, info as log_info};

pub struct SummaryProcessesRepository;

impl SummaryProcessesRepository {
    /// Retrieves the current summary process state for a given meeting ID.
    pub async fn get_summary_data(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<SummaryProcess>, sqlx::Error> {
        sqlx::query_as::<_, SummaryProcess>("SELECT * FROM summary_processes WHERE meeting_id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
    }

    pub async fn update_meeting_summary(
        pool: &SqlitePool,
        meeting_id: &str,
        summary: &Value,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = pool.begin().await?;

        let meeting_exists: bool = sqlx::query("SELECT 1 FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(&mut *transaction)
            .await?
            .is_some();

        if !meeting_exists {
            log_info!(
                "Attempted to save summary for a non-existent meeting_id: {}",
                meeting_id
            );
            transaction.rollback().await?;
            return Ok(false);
        }

        let result_json = serde_json::to_string(summary);
        if result_json.is_err() {
            error!("Can't convert the json to string for saving to Database");
            transaction.rollback().await?;
            return Ok(false);
        }
        let now = Utc::now();

        sqlx::query("UPDATE summary_processes SET result = ?, updated_at = ? WHERE meeting_id = ?")
            .bind(&result_json.unwrap())
            .bind(now)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;

        log_info!(
            "Successfully updated summary and timestamp for meeting_id: {}",
            meeting_id
        );
        Ok(true)
    }

    /// The raw `result` blob for several meetings, in one query per batch.
    ///
    /// For building project-level context, where fetching per meeting would mean
    /// N round trips on every chat turn. Meetings with no summary row are simply
    /// absent from the map — callers must treat "missing" as "no summary yet",
    /// not as an error.
    ///
    /// Note this deliberately does NOT use `get_summary_data_for_meeting`'s
    /// shape: that one INNER JOINs `transcript_chunks`, so a meeting whose
    /// summary exists without a chunk row would silently vanish here.
    pub async fn get_results_for_meetings(
        pool: &SqlitePool,
        meeting_ids: &[String],
    ) -> Result<std::collections::HashMap<String, String>, sqlx::Error> {
        const BIND_CHUNK: usize = 500;
        let mut out = std::collections::HashMap::new();
        for chunk in meeting_ids.chunks(BIND_CHUNK) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT meeting_id, result FROM summary_processes \
                 WHERE meeting_id IN ({placeholders}) AND result IS NOT NULL AND result <> ''"
            );
            let mut q = sqlx::query_as::<_, (String, String)>(&sql);
            for id in chunk {
                q = q.bind(id);
            }
            for (id, result) in q.fetch_all(pool).await? {
                out.insert(id, result);
            }
        }
        Ok(out)
    }

    pub async fn get_summary_data_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<SummaryProcess>, sqlx::Error> {
        sqlx::query_as::<_, SummaryProcess>(
            "SELECT p.* FROM summary_processes p JOIN transcript_chunks t ON p.meeting_id = t.meeting_id WHERE p.meeting_id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
    }

    pub async fn create_or_reset_process(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<(), sqlx::Error> {
        log_info!(
            "Creating or resetting summary process for meeting_id: {}",
            meeting_id
        );
        let now = Utc::now();
        sqlx::query(
            r#"
            INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, start_time, result, error)
            VALUES (?, 'PENDING', ?, ?, ?, NULL, NULL)
            ON CONFLICT(meeting_id) DO UPDATE SET
                status = 'PENDING',
                updated_at = excluded.updated_at,
                start_time = excluded.start_time,
                result_backup = result,
                result_backup_timestamp = excluded.updated_at,
                result = result,
                error = NULL
            "#
        )
        .bind(meeting_id)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
        log_info!(
            "Backed up existing summary before regeneration for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    /// Reconcile summary processes interrupted by an app quit that were left stuck in a
    /// non-terminal state (e.g. PENDING) forever. Mark them failed and restore the prior
    /// good summary from `result_backup` so the UI shows the last summary instead of an
    /// eternal "Generating…" spinner. Called once at startup (single-instance app, so
    /// nothing is legitimately running when this runs). Best-effort.
    pub async fn reset_orphaned_processes(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
        let now = Utc::now();
        let result = sqlx::query(
            r#"
            UPDATE summary_processes
            SET status = 'failed',
                error = 'Interrupted by app restart',
                updated_at = ?,
                end_time = ?,
                result = COALESCE(result_backup, result),
                result_backup = NULL,
                result_backup_timestamp = NULL
            WHERE status NOT IN ('completed', 'failed', 'cancelled')
            "#,
        )
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
        let n = result.rows_affected();
        if n > 0 {
            log_info!("Reset {} orphaned summary process(es) at startup", n);
        }
        Ok(n)
    }

    pub async fn update_process_completed(
        pool: &SqlitePool,
        meeting_id: &str,
        result: Value, // Keep this as Value to handle both old and new formats if needed
        chunk_count: i64,
        processing_time: f64,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();
        let result_str = serde_json::to_string(&result)
            .map_err(|e| sqlx::Error::Protocol(format!("Failed to serialize result: {}", e)))?;

        sqlx::query(
            r#"
            UPDATE summary_processes
            SET status = 'completed', result = ?, updated_at = ?, end_time = ?, chunk_count = ?, processing_time = ?, error = NULL, result_backup = NULL, result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#
        )
        .bind(result_str)
        .bind(now)
        .bind(now)
        .bind(chunk_count)
        .bind(processing_time)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Summary completed and backup cleared for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    pub async fn update_process_failed(
        pool: &SqlitePool,
        meeting_id: &str,
        error: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();

        // Restore from backup if it exists, otherwise keep current result
        sqlx::query(
            r#"
            UPDATE summary_processes
            SET
                status = 'failed',
                error = ?,
                updated_at = ?,
                end_time = ?,
                result = COALESCE(result_backup, result),
                result_backup = NULL,
                result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#,
        )
        .bind(error)
        .bind(now)
        .bind(now)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Summary generation failed and backup restored for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    pub async fn update_process_cancelled(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();

        // Restore from backup if it exists, otherwise keep current result
        sqlx::query(
            r#"
            UPDATE summary_processes
            SET
                status = 'cancelled',
                updated_at = ?,
                end_time = ?,
                error = 'Generation was cancelled by user',
                result = COALESCE(result_backup, result),
                result_backup = NULL,
                result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Marked summary process as cancelled and restored backup for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::test_support::migrated_pool;

    async fn insert_meeting(pool: &SqlitePool, id: &str) {
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, 'T', datetime('now'), datetime('now'))",
        )
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn reset_orphaned_processes_fails_stuck_and_restores_backup() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        // A process stranded PENDING with a prior good summary in result_backup.
        sqlx::query(
            "INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result_backup) VALUES ('m1','PENDING',datetime('now'),datetime('now'),'PRIOR')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let n = SummaryProcessesRepository::reset_orphaned_processes(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1);

        let (status, result): (String, Option<String>) = sqlx::query_as(
            "SELECT status, result FROM summary_processes WHERE meeting_id = 'm1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(result.as_deref(), Some("PRIOR"), "prior summary restored from backup");
    }

    #[tokio::test]
    async fn reset_orphaned_processes_leaves_terminal_rows() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m2").await;
        sqlx::query(
            "INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result) VALUES ('m2','completed',datetime('now'),datetime('now'),'DONE')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let n = SummaryProcessesRepository::reset_orphaned_processes(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "completed rows are not touched");
        let status: String =
            sqlx::query_scalar("SELECT status FROM summary_processes WHERE meeting_id = 'm2'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "completed");
    }

    async fn status_of(pool: &SqlitePool, id: &str) -> String {
        sqlx::query_scalar("SELECT status FROM summary_processes WHERE meeting_id = ?")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn update_meeting_summary_guards_nonexistent_meeting() {
        let pool = migrated_pool().await;
        // No meeting row inserted.
        let ok = SummaryProcessesRepository::update_meeting_summary(
            &pool,
            "ghost",
            &serde_json::json!({ "summary": "x" }),
        )
        .await
        .unwrap();
        assert!(!ok, "summary update for a non-existent meeting returns Ok(false)");
    }

    #[tokio::test]
    async fn status_transitions_completed_then_failed() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        SummaryProcessesRepository::create_or_reset_process(&pool, "m1")
            .await
            .unwrap();
        assert_eq!(status_of(&pool, "m1").await, "PENDING");

        SummaryProcessesRepository::update_process_completed(
            &pool,
            "m1",
            serde_json::json!({ "summary": "done" }),
            3,
            1.5,
        )
        .await
        .unwrap();
        assert_eq!(status_of(&pool, "m1").await, "completed");

        SummaryProcessesRepository::update_process_failed(&pool, "m1", "boom")
            .await
            .unwrap();
        assert_eq!(status_of(&pool, "m1").await, "failed");
    }
}
