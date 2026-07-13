use crate::api::{MeetingDetails, MeetingTranscript};
use crate::database::models::{MeetingModel, Transcript};
use chrono::Utc;
use sqlx::{Connection, Error as SqlxError, SqliteConnection, SqlitePool};
use tracing::{error, info};

pub struct MeetingsRepository;

impl MeetingsRepository {
    /// Live (non-trashed) meetings, newest first. Soft-deleted rows
    /// (`deleted_at IS NOT NULL`) are hidden — they live in the trash until the
    /// retention purge removes them or the user restores them.
    pub async fn get_meetings(pool: &SqlitePool) -> Result<Vec<MeetingModel>, sqlx::Error> {
        let meetings = sqlx::query_as::<_, MeetingModel>(
            "SELECT * FROM meetings WHERE deleted_at IS NULL ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await?;
        Ok(meetings)
    }

    /// All non-empty `folder_path`s, used by the filesystem recovery scan to dedup
    /// interrupted-recording folders against meetings already saved to SQLite.
    /// Intentionally includes trashed (soft-deleted) meetings: their folders are
    /// still "known" to the DB, so recovery must not resurrect them as new imports.
    pub async fn list_folder_paths(pool: &SqlitePool) -> Result<Vec<String>, SqlxError> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT folder_path FROM meetings WHERE folder_path IS NOT NULL AND folder_path != ''",
        )
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(|(p,)| p).collect())
    }

    /// Soft-delete: move a meeting to the trash by stamping `deleted_at`.
    ///
    /// The meeting vanishes from every listing/search (they filter
    /// `deleted_at IS NULL`) but its transcripts, summary, and chunks are left
    /// untouched, so [`restore_meeting`](Self::restore_meeting) fully reverses it.
    /// Returns `false` if the id doesn't exist or is already trashed (the
    /// `deleted_at IS NULL` guard makes a repeat delete a no-op). Permanent
    /// removal happens later via [`purge_meeting`](Self::purge_meeting) or the
    /// retention sweep [`purge_trash_older_than`](Self::purge_trash_older_than).
    pub async fn delete_meeting(pool: &SqlitePool, meeting_id: &str) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let result = sqlx::query(
            "UPDATE meetings SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(Utc::now().naive_utc())
        .bind(meeting_id)
        .execute(pool)
        .await?;

        let moved = result.rows_affected() > 0;
        if moved {
            info!("Soft-deleted meeting {} (moved to trash)", meeting_id);
        }
        Ok(moved)
    }

    /// Reverse a soft-delete: clear `deleted_at` so the meeting reappears in
    /// listings/search with all of its (never-touched) child rows intact.
    /// Returns `false` if the id doesn't exist or isn't currently trashed.
    pub async fn restore_meeting(pool: &SqlitePool, meeting_id: &str) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let result = sqlx::query(
            "UPDATE meetings SET deleted_at = NULL WHERE id = ? AND deleted_at IS NOT NULL",
        )
        .bind(meeting_id)
        .execute(pool)
        .await?;

        let restored = result.rows_affected() > 0;
        if restored {
            info!("Restored meeting {} from trash", meeting_id);
        }
        Ok(restored)
    }

    /// Permanently delete a single meeting and all of its associated data
    /// (transcripts, summary processes, transcript chunks) in one transaction.
    /// This is the irreversible hard delete — used to empty the trash. Returns
    /// `false` if the meeting doesn't exist.
    pub async fn purge_meeting(pool: &SqlitePool, meeting_id: &str) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        match delete_meeting_with_transaction(&mut transaction, meeting_id).await {
            Ok(success) => {
                if success {
                    transaction.commit().await?;
                    info!(
                        "Permanently purged meeting {} and all associated data",
                        meeting_id
                    );
                    Ok(true)
                } else {
                    transaction.rollback().await?;
                    Ok(false)
                }
            }
            Err(e) => {
                let _ = transaction.rollback().await;
                error!("Failed to purge meeting {}: {}", meeting_id, e);
                Err(e)
            }
        }
    }

    /// Retention sweep: permanently purge every trashed meeting whose
    /// `deleted_at` is older than `days` days, cascading to its children. Runs
    /// best-effort at startup. All purges share one transaction so a mid-sweep
    /// failure leaves the trash exactly as it was. Returns the number purged.
    pub async fn purge_trash_older_than(pool: &SqlitePool, days: i64) -> Result<u64, SqlxError> {
        let cutoff = (Utc::now() - chrono::Duration::days(days)).naive_utc();

        let stale: Vec<(String,)> = sqlx::query_as(
            "SELECT id FROM meetings WHERE deleted_at IS NOT NULL AND deleted_at < ?",
        )
        .bind(cutoff)
        .fetch_all(pool)
        .await?;

        if stale.is_empty() {
            return Ok(0);
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let mut purged: u64 = 0;
        for (id,) in &stale {
            match delete_meeting_with_transaction(&mut transaction, id).await {
                Ok(true) => purged += 1,
                Ok(false) => {}
                Err(e) => {
                    let _ = transaction.rollback().await;
                    error!("Failed to purge trashed meeting {}: {}", id, e);
                    return Err(e);
                }
            }
        }

        transaction.commit().await?;
        if purged > 0 {
            info!(
                "Purged {} trashed meeting(s) past the {}-day retention window",
                purged, days
            );
        }
        Ok(purged)
    }

    pub async fn get_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<MeetingDetails>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        // Get meeting details
        let meeting: Option<MeetingModel> =
            sqlx::query_as("SELECT id, title, created_at, updated_at, folder_path FROM meetings WHERE id = ?")
                .bind(meeting_id)
                .fetch_optional(&mut *transaction)
                .await?;

        if meeting.is_none() {
            transaction.rollback().await?;
            return Err(SqlxError::RowNotFound);
        }

        if let Some(meeting) = meeting {
            // Get all transcripts for this meeting
            let transcripts =
                sqlx::query_as::<_, Transcript>("SELECT * FROM transcripts WHERE meeting_id = ?")
                    .bind(meeting_id)
                    .fetch_all(&mut *transaction)
                    .await?;

            transaction.commit().await?;

            // Convert Transcript to MeetingTranscript
            let meeting_transcripts = transcripts
                .into_iter()
                .map(|t| MeetingTranscript {
                    id: t.id,
                    text: t.transcript,
                    timestamp: t.timestamp,
                    audio_start_time: t.audio_start_time,
                    audio_end_time: t.audio_end_time,
                    duration: t.duration,
                    speaker: t.speaker,
                })
                .collect::<Vec<_>>();

            Ok(Some(MeetingDetails {
                id: meeting.id,
                title: meeting.title,
                created_at: meeting.created_at.0.to_rfc3339(),
                updated_at: meeting.updated_at.0.to_rfc3339(),
                transcripts: meeting_transcripts,
            }))
        } else {
            transaction.rollback().await?;
            Ok(None)
        }
    }

    /// Get meeting metadata without transcripts (for pagination)
    pub async fn get_meeting_metadata(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<MeetingModel>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let meeting: Option<MeetingModel> =
            sqlx::query_as("SELECT id, title, created_at, updated_at, folder_path FROM meetings WHERE id = ?")
                .bind(meeting_id)
                .fetch_optional(pool)
                .await?;

        Ok(meeting)
    }

    /// Get meeting transcripts with pagination support
    pub async fn get_meeting_transcripts_paginated(
        pool: &SqlitePool,
        meeting_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<Transcript>, i64), SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        // Get total count of transcripts for this meeting
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM transcripts WHERE meeting_id = ?"
        )
        .bind(meeting_id)
        .fetch_one(pool)
        .await?;

        // Get paginated transcripts ordered by audio_start_time
        let transcripts = sqlx::query_as::<_, Transcript>(
            "SELECT * FROM transcripts
             WHERE meeting_id = ?
             ORDER BY audio_start_time ASC
             LIMIT ? OFFSET ?"
        )
        .bind(meeting_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

        Ok((transcripts, total.0))
    }

    pub async fn update_meeting_title(
        pool: &SqlitePool,
        meeting_id: &str,
        new_title: &str,
    ) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let now = Utc::now().naive_utc();

        let rows_affected =
            sqlx::query("UPDATE meetings SET title = ?, updated_at = ? WHERE id = ?")
                .bind(new_title)
                .bind(now)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?;
        if rows_affected.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn get_meeting_attendees(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<String>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT attendees FROM meetings WHERE id = ?")
                .bind(meeting_id)
                .fetch_optional(pool)
                .await?;

        Ok(row.and_then(|(attendees,)| attendees).filter(|a| !a.trim().is_empty()))
    }

    pub async fn update_meeting_attendees(
        pool: &SqlitePool,
        meeting_id: &str,
        attendees: Option<&str>,
    ) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        // Store NULL instead of empty/whitespace-only strings
        let normalised = attendees.map(str::trim).filter(|a| !a.is_empty());

        let result =
            sqlx::query("UPDATE meetings SET attendees = ?, updated_at = ? WHERE id = ?")
                .bind(normalised)
                .bind(Utc::now().naive_utc())
                .bind(meeting_id)
                .execute(pool)
                .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn update_meeting_name(
        pool: &SqlitePool,
        meeting_id: &str,
        new_title: &str,
    ) -> Result<bool, SqlxError> {
        let mut transaction = pool.begin().await?;
        let now = Utc::now();

        // Update meetings table
        let meeting_update =
            sqlx::query("UPDATE meetings SET title = ?, updated_at = ? WHERE id = ?")
                .bind(new_title)
                .bind(now)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?;

        if meeting_update.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false); // Meeting not found
        }

        // Update transcript_chunks table
        sqlx::query("UPDATE transcript_chunks SET meeting_name = ? WHERE meeting_id = ?")
            .bind(new_title)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;
        Ok(true)
    }
}

async fn delete_meeting_with_transaction(
    transaction: &mut SqliteConnection,
    meeting_id: &str,
) -> Result<bool, SqlxError> {
    // Check if meeting exists
    let meeting_exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_optional(&mut *transaction)
        .await?;

    if meeting_exists.is_none() {
        error!("Meeting {} not found for deletion", meeting_id);
        return Ok(false);
    }

    // Delete from related tables in proper order
    // 1. Delete from transcript_chunks
    sqlx::query("DELETE FROM transcript_chunks WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 2. Delete from summary_processes
    sqlx::query("DELETE FROM summary_processes WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 3. Delete from transcripts
    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 4. Finally, delete the meeting
    let result = sqlx::query("DELETE FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::test_support::migrated_pool;

    async fn insert_meeting(pool: &SqlitePool, id: &str, title: &str) {
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, ?, datetime('now'), datetime('now'))",
        )
        .bind(id)
        .bind(title)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delete_meeting_is_soft_and_keeps_children() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1", "Meeting").await;
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp) VALUES ('t1','m1','hi','[00:00]')")
            .execute(&pool).await.unwrap();

        assert!(MeetingsRepository::delete_meeting(&pool, "m1").await.unwrap());

        // The meeting row still exists — just stamped as trashed.
        let deleted_at: Option<String> =
            sqlx::query_scalar("SELECT deleted_at FROM meetings WHERE id='m1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(deleted_at.is_some(), "delete stamps deleted_at");

        // Its children are untouched, so the delete is reversible.
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcripts WHERE meeting_id='m1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "soft delete must not touch child rows");

        // And it vanishes from the live listing.
        let live = MeetingsRepository::get_meetings(&pool).await.unwrap();
        assert!(
            live.iter().all(|m| m.id != "m1"),
            "trashed meeting is hidden from get_meetings"
        );

        // A second delete is a no-op (already trashed).
        assert!(!MeetingsRepository::delete_meeting(&pool, "m1").await.unwrap());
    }

    #[tokio::test]
    async fn restore_meeting_unhides_and_is_reversible() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1", "Meeting").await;
        assert!(MeetingsRepository::delete_meeting(&pool, "m1").await.unwrap());

        assert!(MeetingsRepository::restore_meeting(&pool, "m1").await.unwrap());
        let live = MeetingsRepository::get_meetings(&pool).await.unwrap();
        assert!(
            live.iter().any(|m| m.id == "m1"),
            "restored meeting is visible again"
        );
        let deleted_at: Option<String> =
            sqlx::query_scalar("SELECT deleted_at FROM meetings WHERE id='m1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(deleted_at.is_none(), "restore clears deleted_at");

        // Restoring a live meeting or a missing id is a no-op.
        assert!(!MeetingsRepository::restore_meeting(&pool, "m1").await.unwrap());
        assert!(!MeetingsRepository::restore_meeting(&pool, "nope").await.unwrap());
    }

    #[tokio::test]
    async fn delete_missing_meeting_returns_false() {
        let pool = migrated_pool().await;
        assert!(!MeetingsRepository::delete_meeting(&pool, "nope").await.unwrap());
    }

    #[tokio::test]
    async fn purge_meeting_hard_deletes_children() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1", "Meeting").await;
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp) VALUES ('t1','m1','hi','[00:00]')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO summary_processes (meeting_id, status, created_at, updated_at) VALUES ('m1','completed', datetime('now'), datetime('now'))")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcript_chunks (meeting_id, transcript_text, model, model_name, created_at) VALUES ('m1','txt','m','mn', datetime('now'))")
            .execute(&pool).await.unwrap();

        assert!(MeetingsRepository::purge_meeting(&pool, "m1").await.unwrap());

        for q in [
            "SELECT COUNT(*) FROM meetings WHERE id='m1'",
            "SELECT COUNT(*) FROM transcripts WHERE meeting_id='m1'",
            "SELECT COUNT(*) FROM summary_processes WHERE meeting_id='m1'",
            "SELECT COUNT(*) FROM transcript_chunks WHERE meeting_id='m1'",
        ] {
            let n: i64 = sqlx::query_scalar(q).fetch_one(&pool).await.unwrap();
            assert_eq!(n, 0, "purge hard-deletes everything: {q}");
        }
        assert!(!MeetingsRepository::purge_meeting(&pool, "m1").await.unwrap());
    }

    #[tokio::test]
    async fn purge_trash_older_than_removes_only_stale_trash() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m_old", "Old").await;
        insert_meeting(&pool, "m_recent", "Recent").await;
        insert_meeting(&pool, "m_live", "Live").await;

        // Trash two; backdate one's deletion to 40 days ago (past the 30-day window).
        assert!(MeetingsRepository::delete_meeting(&pool, "m_old").await.unwrap());
        assert!(MeetingsRepository::delete_meeting(&pool, "m_recent").await.unwrap());
        sqlx::query("UPDATE meetings SET deleted_at = datetime('now','-40 days') WHERE id='m_old'")
            .execute(&pool)
            .await
            .unwrap();

        let purged = MeetingsRepository::purge_trash_older_than(&pool, 30)
            .await
            .unwrap();
        assert_eq!(purged, 1, "only the 40-day-old trash is purged");

        let ids: Vec<String> = sqlx::query_scalar("SELECT id FROM meetings ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            ids,
            vec!["m_live".to_string(), "m_recent".to_string()],
            "recent trash and live meetings survive the sweep"
        );

        // Nothing left old enough → second sweep is a no-op.
        assert_eq!(
            MeetingsRepository::purge_trash_older_than(&pool, 30)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn update_title_and_attendees_roundtrip() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1", "Old").await;

        assert!(MeetingsRepository::update_meeting_title(&pool, "m1", "New Title")
            .await
            .unwrap());
        let title: String = sqlx::query_scalar("SELECT title FROM meetings WHERE id='m1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(title, "New Title");
        assert!(!MeetingsRepository::update_meeting_title(&pool, "nope", "x")
            .await
            .unwrap());

        assert!(
            MeetingsRepository::update_meeting_attendees(&pool, "m1", Some("Alice, Bob"))
                .await
                .unwrap()
        );
        assert_eq!(
            MeetingsRepository::get_meeting_attendees(&pool, "m1")
                .await
                .unwrap()
                .as_deref(),
            Some("Alice, Bob")
        );
        MeetingsRepository::update_meeting_attendees(&pool, "m1", Some("   "))
            .await
            .unwrap();
        assert_eq!(
            MeetingsRepository::get_meeting_attendees(&pool, "m1")
                .await
                .unwrap(),
            None,
            "whitespace-only attendees normalize to NULL"
        );
    }

    #[tokio::test]
    async fn get_missing_meeting_is_row_not_found() {
        let pool = migrated_pool().await;
        let err = MeetingsRepository::get_meeting(&pool, "nope")
            .await
            .unwrap_err();
        assert!(
            matches!(err, sqlx::Error::RowNotFound),
            "get_meeting on a missing id returns RowNotFound, got {err:?}"
        );
    }
}
