//! Storage for the stored cross-meeting brief of a project.
//!
//! Mirrors [`super::summary`] in vocabulary and in its backup/restore contract,
//! but is a separate table: `summary_processes.meeting_id` is its PRIMARY KEY
//! and carries an FK to `meetings`, so a project has nowhere to sit there.

use chrono::Utc;
use serde_json::Value;
use sqlx::SqlitePool;
use tracing::info as log_info;

use crate::database::models::ProjectSummaryModel;

pub struct ProjectSummariesRepository;

impl ProjectSummariesRepository {
    pub async fn get(
        pool: &SqlitePool,
        project_id: &str,
    ) -> Result<Option<ProjectSummaryModel>, sqlx::Error> {
        sqlx::query_as::<_, ProjectSummaryModel>(
            "SELECT * FROM project_summaries WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_optional(pool)
        .await
    }

    /// Claim the right to generate this project's brief, atomically.
    ///
    /// Returns `false` when a run is already in flight, in which case the caller
    /// must NOT spawn a second job — it should just let the UI poll the run that
    /// is already going. One statement rather than a read-then-write, because
    /// two concurrent generates would otherwise both pass the check and then
    /// fight: `SummaryService::register_cancellation_token` does a bare
    /// `HashMap::insert`, so the second job overwrites the first's token and the
    /// first job's cleanup removes the second's — after which nothing is
    /// cancellable and both write to this row.
    ///
    /// On success the previous good brief is copied into `result_backup` and
    /// left in `result`, so a failed or cancelled run can restore it and the UI
    /// keeps showing the old brief while the new one is generated.
    pub async fn try_begin(
        pool: &SqlitePool,
        project_id: &str,
        model_provider: &str,
        model_name: &str,
        output_language: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        let now = Utc::now();
        let result = sqlx::query(
            r#"
            INSERT INTO project_summaries
                (project_id, status, created_at, updated_at, start_time, end_time,
                 stage, stage_current, stage_total,
                 model_provider, model_name, output_language, result, error)
            VALUES (?, 'PENDING', ?, ?, ?, NULL, 'collecting', 0, 0, ?, ?, ?, NULL, NULL)
            ON CONFLICT(project_id) DO UPDATE SET
                status = 'PENDING',
                updated_at = excluded.updated_at,
                start_time = excluded.start_time,
                end_time = NULL,
                stage = 'collecting',
                stage_current = 0,
                stage_total = 0,
                model_provider = excluded.model_provider,
                model_name = excluded.model_name,
                output_language = excluded.output_language,
                result_backup = project_summaries.result,
                error = NULL
            WHERE project_summaries.status <> 'PENDING'
            "#,
        )
        .bind(project_id)
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(model_provider)
        .bind(model_name)
        .bind(output_language)
        .execute(pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Record coarse progress. A project brief can be several LLM calls, so a
    /// spinner that never moves for minutes is indistinguishable from a hang.
    pub async fn set_stage(
        pool: &SqlitePool,
        project_id: &str,
        stage: &str,
        current: i64,
        total: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE project_summaries \
             SET stage = ?, stage_current = ?, stage_total = ?, updated_at = ? \
             WHERE project_id = ?",
        )
        .bind(stage)
        .bind(current)
        .bind(total)
        .bind(Utc::now())
        .bind(project_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    pub async fn update_completed(
        pool: &SqlitePool,
        project_id: &str,
        result: &Value,
        covered_meetings: &Value,
        coverage_fingerprint: &str,
        output_language: Option<&str>,
        processing_time: f64,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();
        let result_str = serde_json::to_string(result)
            .map_err(|e| sqlx::Error::Protocol(format!("Failed to serialize brief: {}", e)))?;
        let covered_str = serde_json::to_string(covered_meetings)
            .map_err(|e| sqlx::Error::Protocol(format!("Failed to serialize coverage: {}", e)))?;

        sqlx::query(
            "UPDATE project_summaries SET \
                 status = 'completed', result = ?, error = NULL, result_backup = NULL, \
                 covered_meetings = ?, coverage_fingerprint = ?, output_language = ?, \
                 stage = NULL, stage_current = 0, stage_total = 0, \
                 updated_at = ?, end_time = ?, processing_time = ? \
             WHERE project_id = ?",
        )
        .bind(&result_str)
        .bind(&covered_str)
        .bind(coverage_fingerprint)
        .bind(output_language)
        .bind(now)
        .bind(now)
        .bind(processing_time)
        .bind(project_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Fail the run and put the previous brief back, so a failed regenerate
    /// leaves a stale brief rather than nothing. Mirrors
    /// `SummaryProcessesRepository::update_process_failed`.
    pub async fn update_failed(
        pool: &SqlitePool,
        project_id: &str,
        error: &str,
    ) -> Result<(), sqlx::Error> {
        Self::finish_without_result(pool, project_id, "failed", error).await
    }

    pub async fn update_cancelled(pool: &SqlitePool, project_id: &str) -> Result<(), sqlx::Error> {
        Self::finish_without_result(
            pool,
            project_id,
            "cancelled",
            "Generation was cancelled by user",
        )
        .await
    }

    async fn finish_without_result(
        pool: &SqlitePool,
        project_id: &str,
        status: &str,
        error: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();
        sqlx::query(
            "UPDATE project_summaries SET \
                 status = ?, error = ?, \
                 result = COALESCE(result_backup, result), result_backup = NULL, \
                 stage = NULL, stage_current = 0, stage_total = 0, \
                 updated_at = ?, end_time = ? \
             WHERE project_id = ?",
        )
        .bind(status)
        .bind(error)
        .bind(now)
        .bind(now)
        .bind(project_id)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Reconcile briefs interrupted by an app quit that were left stuck in a
    /// non-terminal state forever, restoring the prior good brief so the UI
    /// shows it instead of an eternal "Generating…". Called at startup and when
    /// switching database files, beside the meeting-summary sweep. Best-effort.
    pub async fn reset_orphaned(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
        let now = Utc::now();
        let result = sqlx::query(
            r#"
            UPDATE project_summaries
            SET status = 'failed',
                error = 'Interrupted by app restart',
                updated_at = ?,
                end_time = ?,
                stage = NULL,
                stage_current = 0,
                stage_total = 0,
                result = COALESCE(result_backup, result),
                result_backup = NULL
            WHERE status NOT IN ('completed', 'failed', 'cancelled')
            "#,
        )
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
        let n = result.rows_affected();
        if n > 0 {
            log_info!("Reset {} orphaned project brief(s) at startup", n);
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::repositories::project::ProjectsRepository;
    use crate::database::test_support::migrated_pool;
    use serde_json::json;

    async fn insert_project(pool: &SqlitePool) -> String {
        ProjectsRepository::create(pool, "Client X", None, None)
            .await
            .unwrap()
            .id
    }

    async fn status_of(pool: &SqlitePool, project_id: &str) -> String {
        ProjectSummariesRepository::get(pool, project_id)
            .await
            .unwrap()
            .unwrap()
            .status
    }

    #[tokio::test]
    async fn try_begin_rejects_a_second_concurrent_run() {
        let pool = migrated_pool().await;
        let project_id = insert_project(&pool).await;

        let first = ProjectSummariesRepository::try_begin(&pool, &project_id, "ollama", "m", None)
            .await
            .unwrap();
        assert!(first, "the first caller claims the job");

        let second = ProjectSummariesRepository::try_begin(&pool, &project_id, "ollama", "m", None)
            .await
            .unwrap();
        assert!(!second, "a run is already in flight, so the second is refused");

        assert_eq!(status_of(&pool, &project_id).await, "PENDING");
    }

    #[tokio::test]
    async fn try_begin_backs_up_the_previous_brief_and_failure_restores_it() {
        let pool = migrated_pool().await;
        let project_id = insert_project(&pool).await;

        ProjectSummariesRepository::try_begin(&pool, &project_id, "ollama", "m", None)
            .await
            .unwrap();
        ProjectSummariesRepository::update_completed(
            &pool,
            &project_id,
            &json!({ "markdown": "PRIOR" }),
            &json!([]),
            "fp-1",
            Some("English"),
            1.0,
        )
        .await
        .unwrap();

        // Regenerating parks the good brief in result_backup...
        ProjectSummariesRepository::try_begin(&pool, &project_id, "ollama", "m", None)
            .await
            .unwrap();
        let during = ProjectSummariesRepository::get(&pool, &project_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(during.status, "PENDING");
        assert!(
            during.result.as_deref().unwrap().contains("PRIOR"),
            "the old brief stays visible while the new one generates"
        );
        assert!(during.result_backup.as_deref().unwrap().contains("PRIOR"));

        // ...and a failure puts it back rather than leaving the project blank.
        ProjectSummariesRepository::update_failed(&pool, &project_id, "model exploded")
            .await
            .unwrap();
        let after = ProjectSummariesRepository::get(&pool, &project_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, "failed");
        assert!(after.result.as_deref().unwrap().contains("PRIOR"));
        assert!(after.result_backup.is_none());
        assert_eq!(after.error.as_deref(), Some("model exploded"));
    }

    #[tokio::test]
    async fn cancelling_restores_the_previous_brief() {
        let pool = migrated_pool().await;
        let project_id = insert_project(&pool).await;

        ProjectSummariesRepository::try_begin(&pool, &project_id, "ollama", "m", None)
            .await
            .unwrap();
        ProjectSummariesRepository::update_completed(
            &pool,
            &project_id,
            &json!({ "markdown": "PRIOR" }),
            &json!([]),
            "fp-1",
            None,
            1.0,
        )
        .await
        .unwrap();
        ProjectSummariesRepository::try_begin(&pool, &project_id, "ollama", "m", None)
            .await
            .unwrap();
        ProjectSummariesRepository::update_cancelled(&pool, &project_id)
            .await
            .unwrap();

        let after = ProjectSummariesRepository::get(&pool, &project_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.status, "cancelled");
        assert!(after.result.as_deref().unwrap().contains("PRIOR"));
    }

    #[tokio::test]
    async fn reset_orphaned_fails_stuck_rows_and_leaves_terminal_ones() {
        let pool = migrated_pool().await;
        let stuck = insert_project(&pool).await;
        let done = ProjectsRepository::create(&pool, "Client Y", None, None)
            .await
            .unwrap()
            .id;

        ProjectSummariesRepository::try_begin(&pool, &stuck, "ollama", "m", None)
            .await
            .unwrap();
        ProjectSummariesRepository::update_completed(
            &pool,
            &stuck,
            &json!({ "markdown": "PRIOR" }),
            &json!([]),
            "fp-1",
            None,
            1.0,
        )
        .await
        .unwrap();
        // Strand it mid-run.
        ProjectSummariesRepository::try_begin(&pool, &stuck, "ollama", "m", None)
            .await
            .unwrap();

        ProjectSummariesRepository::try_begin(&pool, &done, "ollama", "m", None)
            .await
            .unwrap();
        ProjectSummariesRepository::update_completed(
            &pool,
            &done,
            &json!({ "markdown": "DONE" }),
            &json!([]),
            "fp-2",
            None,
            1.0,
        )
        .await
        .unwrap();

        let n = ProjectSummariesRepository::reset_orphaned(&pool).await.unwrap();
        assert_eq!(n, 1, "only the stranded row is reconciled");

        let recovered = ProjectSummariesRepository::get(&pool, &stuck)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered.status, "failed");
        assert!(recovered.result.as_deref().unwrap().contains("PRIOR"));

        assert_eq!(status_of(&pool, &done).await, "completed");
    }

    #[tokio::test]
    async fn deleting_a_project_removes_its_brief() {
        let pool = migrated_pool().await;
        let project_id = insert_project(&pool).await;
        ProjectSummariesRepository::try_begin(&pool, &project_id, "ollama", "m", None)
            .await
            .unwrap();

        ProjectsRepository::delete(&pool, &project_id).await.unwrap();

        assert!(ProjectSummariesRepository::get(&pool, &project_id)
            .await
            .unwrap()
            .is_none());
    }
}
