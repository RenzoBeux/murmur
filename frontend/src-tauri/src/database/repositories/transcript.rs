use crate::api::{TranscriptSearchResult, TranscriptSegment};
use chrono::Utc;
use sqlx::{Connection, Error as SqlxError, SqlitePool};
use tracing::{error, info};
use uuid::Uuid;

/// Row payload for inserting the tail half of a segment after a split.
/// The caller computes timestamps; this struct just transports them to the
/// repo so the SQL stays in one place.
#[derive(Debug, Clone)]
pub struct NewSegmentRow {
    pub id: String,
    pub meeting_id: String,
    pub text: String,
    pub timestamp: String,
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub duration: Option<f64>,
    pub speaker: Option<String>,
}

pub struct TranscriptsRepository;

impl TranscriptsRepository {
    /// Saves a new meeting and its associated transcript segments.
    /// This function uses a transaction to ensure that either both the meeting
    /// and all its transcripts are saved, or none of them are.
    pub async fn save_transcript(
        pool: &SqlitePool,
        meeting_title: &str,
        transcripts: &[TranscriptSegment],
        folder_path: Option<String>,
    ) -> Result<String, SqlxError> {
        let meeting_id = format!("meeting-{}", Uuid::new_v4());

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let now = Utc::now();

        // 1. Create the new meeting
        let result = sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&meeting_id)
        .bind(meeting_title)
        .bind(now)
        .bind(now)
        .bind(&folder_path)
        .execute(&mut *transaction)
        .await;

        if let Err(e) = result {
            error!("Failed to create meeting '{}': {}", meeting_title, e);
            transaction.rollback().await?;
            return Err(e);
        }

        info!("Successfully created meeting with id: {}", meeting_id);

        // 2. Save each transcript segment with audio timing fields and speaker tag
        for segment in transcripts {
            let transcript_id = format!("transcript-{}", Uuid::new_v4());
            let result = sqlx::query(
                "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&transcript_id)
            .bind(&meeting_id)
            .bind(&segment.text)
            .bind(&segment.timestamp)
            .bind(segment.audio_start_time)
            .bind(segment.audio_end_time)
            .bind(segment.duration)
            .bind(&segment.speaker)
            .execute(&mut *transaction)
            .await;

            if let Err(e) = result {
                error!(
                    "Failed to save transcript segment for meeting {}: {}",
                    meeting_id, e
                );
                transaction.rollback().await?;
                return Err(e);
            }
        }

        info!(
            "Successfully saved {} transcript segments for meeting {}",
            transcripts.len(),
            meeting_id
        );

        // Commit the transaction
        transaction.commit().await?;

        Ok(meeting_id)
    }

    /// Bulk-update the `speaker` column for a set of transcript rows by id.
    /// All updates run in a single transaction so a failure mid-write rolls
    /// back cleanly. Used by post-recording diarization (re-runs on past
    /// meetings) — typical batch size is one row per transcript segment, so
    /// per-row UPDATE is fine.
    pub async fn update_speakers(
        pool: &SqlitePool,
        updates: &[(String, Option<String>)],
    ) -> Result<usize, SqlxError> {
        if updates.is_empty() {
            return Ok(0);
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let mut affected: usize = 0;
        for (id, speaker) in updates {
            let result = sqlx::query("UPDATE transcripts SET speaker = ? WHERE id = ?")
                .bind(speaker)
                .bind(id)
                .execute(&mut *transaction)
                .await;

            match result {
                Ok(res) => affected += res.rows_affected() as usize,
                Err(e) => {
                    error!("Failed to update speaker for transcript {}: {}", id, e);
                    transaction.rollback().await?;
                    return Err(e);
                }
            }
        }

        transaction.commit().await?;
        info!(
            "Updated speaker on {} of {} transcript rows",
            affected,
            updates.len()
        );
        Ok(affected)
    }

    /// Rename every transcript row of a meeting currently tagged `from_speaker`
    /// to `to_speaker`. Used by the post-diarization "name speakers" step to
    /// turn a cluster tag (e.g. "speaker_1") into a human label in one atomic
    /// UPDATE. Returns the number of rows changed.
    pub async fn rename_speaker(
        pool: &SqlitePool,
        meeting_id: &str,
        from_speaker: &str,
        to_speaker: &str,
    ) -> Result<usize, SqlxError> {
        let result = sqlx::query(
            "UPDATE transcripts SET speaker = ? WHERE meeting_id = ? AND speaker = ?",
        )
        .bind(to_speaker)
        .bind(meeting_id)
        .bind(from_speaker)
        .execute(pool)
        .await?;
        info!(
            "Renamed speaker '{}' -> '{}' on {} rows in meeting {}",
            from_speaker,
            to_speaker,
            result.rows_affected(),
            meeting_id
        );
        Ok(result.rows_affected() as usize)
    }

    /// Update the `transcript` (text) column for a single segment.
    /// Returns true if a row matched the id.
    pub async fn update_segment_text(
        pool: &SqlitePool,
        segment_id: &str,
        new_text: &str,
    ) -> Result<bool, SqlxError> {
        let result = sqlx::query("UPDATE transcripts SET transcript = ? WHERE id = ?")
            .bind(new_text)
            .bind(segment_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Bulk-delete transcript rows by id in a single transaction.
    /// Returns the number of rows actually removed.
    pub async fn delete_segments(
        pool: &SqlitePool,
        segment_ids: &[String],
    ) -> Result<usize, SqlxError> {
        if segment_ids.is_empty() {
            return Ok(0);
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let mut affected: usize = 0;
        for id in segment_ids {
            let result = sqlx::query("DELETE FROM transcripts WHERE id = ?")
                .bind(id)
                .execute(&mut *transaction)
                .await;

            match result {
                Ok(res) => affected += res.rows_affected() as usize,
                Err(e) => {
                    error!("Failed to delete transcript {}: {}", id, e);
                    transaction.rollback().await?;
                    return Err(e);
                }
            }
        }

        transaction.commit().await?;
        info!(
            "Deleted {} of {} transcript rows",
            affected,
            segment_ids.len()
        );
        Ok(affected)
    }

    /// Merge: keep one segment row (update its text/end/duration/speaker) and
    /// delete the others, all atomically. The caller computes the merged
    /// values from the source rows.
    pub async fn merge_segments(
        pool: &SqlitePool,
        keeper_id: &str,
        merged_text: &str,
        audio_end_time: f64,
        duration: f64,
        speaker: Option<&str>,
        deleted_ids: &[String],
    ) -> Result<(), SqlxError> {
        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let update = sqlx::query(
            "UPDATE transcripts
             SET transcript = ?, audio_end_time = ?, duration = ?, speaker = ?
             WHERE id = ?",
        )
        .bind(merged_text)
        .bind(audio_end_time)
        .bind(duration)
        .bind(speaker)
        .bind(keeper_id)
        .execute(&mut *transaction)
        .await;

        if let Err(e) = update {
            error!("Failed to update merge keeper {}: {}", keeper_id, e);
            transaction.rollback().await?;
            return Err(e);
        }

        for id in deleted_ids {
            let result = sqlx::query("DELETE FROM transcripts WHERE id = ?")
                .bind(id)
                .execute(&mut *transaction)
                .await;
            if let Err(e) = result {
                error!("Failed to delete merge source {}: {}", id, e);
                transaction.rollback().await?;
                return Err(e);
            }
        }

        transaction.commit().await?;
        info!(
            "Merged {} rows into keeper {}",
            deleted_ids.len() + 1,
            keeper_id
        );
        Ok(())
    }

    /// Split: update the source row to hold only the head, then insert a new
    /// row for the tail. The caller computes interpolated timestamps.
    pub async fn split_segment(
        pool: &SqlitePool,
        source_id: &str,
        head_text: &str,
        head_end_time: f64,
        head_duration: f64,
        tail: &NewSegmentRow,
    ) -> Result<(), SqlxError> {
        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let update = sqlx::query(
            "UPDATE transcripts
             SET transcript = ?, audio_end_time = ?, duration = ?
             WHERE id = ?",
        )
        .bind(head_text)
        .bind(head_end_time)
        .bind(head_duration)
        .bind(source_id)
        .execute(&mut *transaction)
        .await;

        if let Err(e) = update {
            error!("Failed to update split source {}: {}", source_id, e);
            transaction.rollback().await?;
            return Err(e);
        }

        let insert = sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&tail.id)
        .bind(&tail.meeting_id)
        .bind(&tail.text)
        .bind(&tail.timestamp)
        .bind(tail.audio_start_time)
        .bind(tail.audio_end_time)
        .bind(tail.duration)
        .bind(&tail.speaker)
        .execute(&mut *transaction)
        .await;

        if let Err(e) = insert {
            error!("Failed to insert split tail for source {}: {}", source_id, e);
            transaction.rollback().await?;
            return Err(e);
        }

        transaction.commit().await?;
        info!("Split segment {} into head + tail {}", source_id, tail.id);
        Ok(())
    }

    /// Bulk insert transcript rows with explicit ids. Used by undo to restore
    /// segments previously deleted or merged away. Idempotent against
    /// already-present rows via INSERT OR IGNORE.
    pub async fn bulk_insert_segments(
        pool: &SqlitePool,
        rows: &[NewSegmentRow],
    ) -> Result<usize, SqlxError> {
        if rows.is_empty() {
            return Ok(0);
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let mut affected: usize = 0;
        for row in rows {
            let result = sqlx::query(
                "INSERT OR IGNORE INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )
            .bind(&row.id)
            .bind(&row.meeting_id)
            .bind(&row.text)
            .bind(&row.timestamp)
            .bind(row.audio_start_time)
            .bind(row.audio_end_time)
            .bind(row.duration)
            .bind(&row.speaker)
            .execute(&mut *transaction)
            .await;

            match result {
                Ok(res) => affected += res.rows_affected() as usize,
                Err(e) => {
                    error!("Failed to insert segment {}: {}", row.id, e);
                    transaction.rollback().await?;
                    return Err(e);
                }
            }
        }

        transaction.commit().await?;
        info!("Inserted {} of {} segments", affected, rows.len());
        Ok(affected)
    }

    /// Update text + audio bounds + duration on a single segment.
    /// Used by undo of `split` to restore the source row's pre-split state.
    pub async fn update_segment_bounds(
        pool: &SqlitePool,
        segment_id: &str,
        new_text: &str,
        audio_end_time: f64,
        duration: f64,
    ) -> Result<bool, SqlxError> {
        let result = sqlx::query(
            "UPDATE transcripts
             SET transcript = ?, audio_end_time = ?, duration = ?
             WHERE id = ?",
        )
        .bind(new_text)
        .bind(audio_end_time)
        .bind(duration)
        .bind(segment_id)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Searches for a query string within the transcripts.
    /// It returns a list of matching transcripts with context.
    pub async fn search_transcripts(
        pool: &SqlitePool,
        query: &str,
    ) -> Result<Vec<TranscriptSearchResult>, SqlxError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let search_query = format!("%{}%", query.to_lowercase());

        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT m.id, m.title, t.transcript, t.timestamp
             FROM meetings m
             JOIN transcripts t ON m.id = t.meeting_id
             WHERE m.deleted_at IS NULL AND LOWER(t.transcript) LIKE ?
             LIMIT 100",
        )
        .bind(&search_query)
        .fetch_all(pool)
        .await?;

        let results = rows
            .into_iter()
            .map(|(id, title, transcript, timestamp)| {
                // Reuse the UTF-8-safe snippet helper (char-index based) so accented
                // text (á/é/ñ) at the window edge never panics via a byte-boundary slice.
                let match_context = crate::mcp::tools::snippet(&transcript, query);
                TranscriptSearchResult {
                    id,
                    title,
                    match_context,
                    timestamp,
                }
            })
            .collect();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::test_support::migrated_pool;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn insert_meeting(pool: &SqlitePool, id: &str) {
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, 'T', datetime('now'), datetime('now'))",
        )
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    }

    fn seg(id: &str, text: &str, start: f64, end: f64) -> NewSegmentRow {
        NewSegmentRow {
            id: id.to_string(),
            meeting_id: "m1".to_string(),
            text: text.to_string(),
            timestamp: "[00:00]".to_string(),
            audio_start_time: Some(start),
            audio_end_time: Some(end),
            duration: Some(end - start),
            speaker: None,
        }
    }

    async fn count_segments(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM transcripts")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn bulk_insert_is_idempotent_and_merge_is_atomic() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;

        let rows = vec![
            seg("s1", "one", 0.0, 10.0),
            seg("s2", "two", 10.0, 20.0),
            seg("s3", "three", 20.0, 30.0),
        ];
        assert_eq!(
            TranscriptsRepository::bulk_insert_segments(&pool, &rows)
                .await
                .unwrap(),
            3
        );

        // INSERT OR IGNORE → re-inserting the same ids is a no-op (undo-safe).
        assert_eq!(
            TranscriptsRepository::bulk_insert_segments(&pool, &rows)
                .await
                .unwrap(),
            0,
            "duplicate ids must be ignored"
        );
        assert_eq!(count_segments(&pool).await, 3);

        // Merge s2+s3 into s1 in one transaction.
        TranscriptsRepository::merge_segments(
            &pool,
            "s1",
            "one two three",
            30.0,
            30.0,
            Some("mic"),
            &["s2".to_string(), "s3".to_string()],
        )
        .await
        .unwrap();

        assert_eq!(count_segments(&pool).await, 1, "merged segments are deleted");
        let (text, end, speaker): (String, f64, Option<String>) = sqlx::query_as(
            "SELECT transcript, audio_end_time, speaker FROM transcripts WHERE id = 's1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(text, "one two three");
        assert_eq!(end, 30.0);
        assert_eq!(speaker.as_deref(), Some("mic"));
    }

    #[tokio::test]
    async fn split_segment_replaces_source_and_inserts_tail() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        TranscriptsRepository::bulk_insert_segments(&pool, &[seg("s1", "hello world", 0.0, 10.0)])
            .await
            .unwrap();

        let tail = seg("s1b", "world", 5.0, 10.0);
        TranscriptsRepository::split_segment(&pool, "s1", "hello", 5.0, 5.0, &tail)
            .await
            .unwrap();

        assert_eq!(count_segments(&pool).await, 2);
        let head: String = sqlx::query_scalar("SELECT transcript FROM transcripts WHERE id = 's1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(head, "hello");
        let tail_text: String =
            sqlx::query_scalar("SELECT transcript FROM transcripts WHERE id = 's1b'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(tail_text, "world");
    }

    #[tokio::test]
    async fn update_segment_bounds_updates_existing_and_reports_missing() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        TranscriptsRepository::bulk_insert_segments(&pool, &[seg("s1", "orig", 0.0, 10.0)])
            .await
            .unwrap();

        let updated = TranscriptsRepository::update_segment_bounds(&pool, "s1", "edited", 8.0, 8.0)
            .await
            .unwrap();
        assert!(updated);
        let (text, end): (String, f64) =
            sqlx::query_as("SELECT transcript, audio_end_time FROM transcripts WHERE id = 's1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(text, "edited");
        assert_eq!(end, 8.0);

        let missing = TranscriptsRepository::update_segment_bounds(&pool, "nope", "x", 1.0, 1.0)
            .await
            .unwrap();
        assert!(!missing, "updating a non-existent segment returns false");
    }

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE meetings (id TEXT PRIMARY KEY, title TEXT, deleted_at TEXT)",
            "CREATE TABLE transcripts (id TEXT, meeting_id TEXT, transcript TEXT, timestamp TEXT)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool
    }

    /// Regression: a transcript full of accented (multi-byte) text must not panic
    /// the search command. The previous byte-slicing helper crashed whenever an
    /// accented char straddled the ±100 window; snippet() is char-index safe.
    #[tokio::test]
    async fn search_transcripts_is_utf8_safe_for_spanish() {
        let pool = test_pool().await;
        // Long accented transcript so the match sits >100 chars from the start,
        // forcing the context window to slice inside multi-byte territory.
        let transcript = format!(
            "{}reunión de mañana con el equipo español{}",
            "é".repeat(150),
            "ñ".repeat(150)
        );
        sqlx::query("INSERT INTO meetings (id, title) VALUES ('m1', 'Reunión')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp) VALUES ('t1', 'm1', ?, '2026-07-11')")
            .bind(&transcript)
            .execute(&pool)
            .await
            .unwrap();

        // Must return Ok (not panic/hang) and find the match.
        let results = TranscriptsRepository::search_transcripts(&pool, "español")
            .await
            .expect("search must not error");
        assert_eq!(results.len(), 1);
        assert!(results[0].match_context.contains("español"));
    }

    /// Soft-deleted (trashed) meetings must not leak into transcript search,
    /// and restoring one brings its matches back.
    #[tokio::test]
    async fn search_excludes_trashed_meetings() {
        use crate::database::repositories::meeting::MeetingsRepository;

        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp) VALUES ('t1','m1','the quarterly roadmap','[00:00]')")
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            TranscriptsRepository::search_transcripts(&pool, "roadmap")
                .await
                .unwrap()
                .len(),
            1,
            "live meeting is searchable"
        );

        assert!(MeetingsRepository::delete_meeting(&pool, "m1").await.unwrap());
        assert!(
            TranscriptsRepository::search_transcripts(&pool, "roadmap")
                .await
                .unwrap()
                .is_empty(),
            "trashed meetings must not appear in search"
        );

        assert!(MeetingsRepository::restore_meeting(&pool, "m1").await.unwrap());
        assert_eq!(
            TranscriptsRepository::search_transcripts(&pool, "roadmap")
                .await
                .unwrap()
                .len(),
            1,
            "restored meeting is searchable again"
        );
    }
}
