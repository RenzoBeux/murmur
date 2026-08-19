use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::database::models::{ChatMessageModel, ChatThreadModel};

/// Column default in 20260819130000_add_chat_grounding — kept in sync here so
/// rows this repository builds in memory match what SQLite would have written.
pub const DEFAULT_GROUNDING_MODE: &str = "transcript_only";

pub struct ChatMessagesRepository;

impl ChatMessagesRepository {
    /// `metadata` is JSON describing how an assistant answer was produced; pass
    /// None for user messages and for answers with nothing to record.
    pub async fn add_message(
        pool: &SqlitePool,
        meeting_id: &str,
        thread_id: &str,
        role: &str,
        content: &str,
        metadata: Option<&str>,
    ) -> Result<ChatMessageModel, sqlx::Error> {
        if role != "user" && role != "assistant" {
            return Err(sqlx::Error::Protocol(format!(
                "Invalid chat role: {}",
                role
            )));
        }

        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now();

        sqlx::query(
            "INSERT INTO chat_messages (id, meeting_id, thread_id, role, content, created_at, metadata) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(meeting_id)
        .bind(thread_id)
        .bind(role)
        .bind(content)
        .bind(created_at)
        .bind(metadata)
        .execute(pool)
        .await?;

        Ok(ChatMessageModel {
            id,
            meeting_id: meeting_id.to_string(),
            thread_id: Some(thread_id.to_string()),
            role: role.to_string(),
            content: content.to_string(),
            created_at,
            metadata: metadata.map(str::to_string),
        })
    }

    pub async fn list_for_thread(
        pool: &SqlitePool,
        thread_id: &str,
    ) -> Result<Vec<ChatMessageModel>, sqlx::Error> {
        // rowid tiebreaker: created_at has second resolution, so two rapid
        // inserts can collide; insertion order (rowid) settles the tie. The FTS
        // index already depends on stable rowids, so this adds no new assumption.
        sqlx::query_as::<_, ChatMessageModel>(
            "SELECT id, meeting_id, thread_id, role, content, created_at, metadata \
             FROM chat_messages WHERE thread_id = ? ORDER BY created_at ASC, rowid ASC",
        )
        .bind(thread_id)
        .fetch_all(pool)
        .await
    }

    pub async fn list_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<ChatMessageModel>, sqlx::Error> {
        sqlx::query_as::<_, ChatMessageModel>(
            "SELECT id, meeting_id, thread_id, role, content, created_at, metadata \
             FROM chat_messages WHERE meeting_id = ? ORDER BY created_at ASC, rowid ASC",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await
    }

    pub async fn clear_for_thread(
        pool: &SqlitePool,
        thread_id: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM chat_messages WHERE thread_id = ?")
            .bind(thread_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn clear_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM chat_messages WHERE meeting_id = ?")
            .bind(meeting_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn delete_message(pool: &SqlitePool, message_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM chat_messages WHERE id = ?")
            .bind(message_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }
}

/// A message to insert when materializing a thread from an in-memory source
/// (the live Ask-AI store), preserving the original timestamps.
#[derive(Debug, Clone)]
pub struct NewChatMessage {
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
    /// Grounding/citation JSON, as on `ChatMessageModel::metadata`.
    pub metadata: Option<String>,
}

pub struct ChatThreadsRepository;

impl ChatThreadsRepository {
    pub async fn create_thread(
        pool: &SqlitePool,
        meeting_id: &str,
        title: &str,
        origin: &str,
    ) -> Result<ChatThreadModel, sqlx::Error> {
        let id = format!("thread-{}", Uuid::new_v4());
        let created_at = Utc::now();
        sqlx::query(
            "INSERT INTO chat_threads (id, meeting_id, title, origin, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(meeting_id)
        .bind(title)
        .bind(origin)
        .bind(created_at)
        .execute(pool)
        .await?;
        Ok(ChatThreadModel {
            id,
            meeting_id: meeting_id.to_string(),
            title: title.to_string(),
            origin: origin.to_string(),
            // Matches the column default in 20260819130000_add_chat_grounding.
            grounding_mode: DEFAULT_GROUNDING_MODE.to_string(),
            created_at,
        })
    }

    pub async fn list_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<ChatThreadModel>, sqlx::Error> {
        sqlx::query_as::<_, ChatThreadModel>(
            "SELECT id, meeting_id, title, origin, grounding_mode, created_at \
             FROM chat_threads WHERE meeting_id = ? ORDER BY created_at ASC, rowid ASC",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await
    }

    pub async fn get_thread(
        pool: &SqlitePool,
        thread_id: &str,
    ) -> Result<Option<ChatThreadModel>, sqlx::Error> {
        sqlx::query_as::<_, ChatThreadModel>(
            "SELECT id, meeting_id, title, origin, grounding_mode, created_at \
             FROM chat_threads WHERE id = ?",
        )
        .bind(thread_id)
        .fetch_optional(pool)
        .await
    }

    /// Delete a thread and its messages. The messages are deleted explicitly
    /// (not left to the FK cascade) because cascade deletes do not fire the
    /// per-row FTS triggers under SQLite's default recursive_triggers=OFF —
    /// the explicit DELETE keeps search_index in sync.
    /// Change how far past the transcript a conversation may reach. Returns the
    /// number of rows updated, so callers can tell a missing thread from a no-op.
    pub async fn set_grounding_mode(
        pool: &SqlitePool,
        thread_id: &str,
        grounding_mode: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("UPDATE chat_threads SET grounding_mode = ? WHERE id = ?")
            .bind(grounding_mode)
            .bind(thread_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn delete_thread(pool: &SqlitePool, thread_id: &str) -> Result<u64, sqlx::Error> {
        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM chat_messages WHERE thread_id = ?")
            .bind(thread_id)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM chat_threads WHERE id = ?")
            .bind(thread_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }

    /// Materialize the live Ask-AI conversation as a persisted thread, in one
    /// transaction. Each message keeps its original created_at; insertion order
    /// follows the slice, so `(created_at, rowid)` ordering reproduces it.
    pub async fn create_live_thread_with_messages(
        pool: &SqlitePool,
        meeting_id: &str,
        messages: &[NewChatMessage],
        grounding_mode: &str,
    ) -> Result<ChatThreadModel, sqlx::Error> {
        let thread_id = format!("thread-{}", Uuid::new_v4());
        let thread_created_at = messages
            .first()
            .map(|m| m.created_at)
            .unwrap_or_else(Utc::now);

        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO chat_threads (id, meeting_id, title, origin, grounding_mode, created_at) \
             VALUES (?, ?, 'Live chat', 'live', ?, ?)",
        )
        .bind(&thread_id)
        .bind(meeting_id)
        .bind(grounding_mode)
        .bind(thread_created_at)
        .execute(&mut *tx)
        .await?;
        for msg in messages {
            sqlx::query(
                "INSERT INTO chat_messages (id, meeting_id, thread_id, role, content, created_at, metadata) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(meeting_id)
            .bind(&thread_id)
            .bind(&msg.role)
            .bind(&msg.content)
            .bind(msg.created_at)
            .bind(&msg.metadata)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;

        Ok(ChatThreadModel {
            id: thread_id,
            meeting_id: meeting_id.to_string(),
            title: "Live chat".to_string(),
            origin: "live".to_string(),
            grounding_mode: grounding_mode.to_string(),
            created_at: thread_created_at,
        })
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

    async fn insert_thread(pool: &SqlitePool, meeting_id: &str) -> String {
        ChatThreadsRepository::create_thread(pool, meeting_id, "Chat 1", "post")
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn add_list_and_clear_messages() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        let thread_id = insert_thread(&pool, "m1").await;

        ChatMessagesRepository::add_message(&pool, "m1", &thread_id, "user", "hello", None)
            .await
            .unwrap();
        ChatMessagesRepository::add_message(&pool, "m1", &thread_id, "assistant", "hi there", None)
            .await
            .unwrap();

        let msgs = ChatMessagesRepository::list_for_thread(&pool, &thread_id)
            .await
            .unwrap();
        assert_eq!(msgs.len(), 2);
        // Rapid inserts share a created_at (second resolution); the rowid
        // tiebreaker must still return insertion order.
        assert_eq!(msgs[0].content, "hello");
        assert_eq!(msgs[1].content, "hi there");

        let cleared = ChatMessagesRepository::clear_for_thread(&pool, &thread_id)
            .await
            .unwrap();
        assert_eq!(cleared, 2);
        assert!(ChatMessagesRepository::list_for_thread(&pool, &thread_id)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn invalid_role_is_rejected() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        let thread_id = insert_thread(&pool, "m1").await;
        let result =
            ChatMessagesRepository::add_message(&pool, "m1", &thread_id, "system", "nope", None).await;
        assert!(result.is_err(), "an invalid chat role must be rejected");
    }

    #[tokio::test]
    async fn ordering_uses_rowid_tiebreaker_on_equal_timestamps() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        let thread_id = insert_thread(&pool, "m1").await;

        // Force identical created_at values; only rowid can order these.
        for content in ["first", "second", "third"] {
            sqlx::query(
                "INSERT INTO chat_messages (id, meeting_id, thread_id, role, content, created_at) \
                 VALUES (?, 'm1', ?, 'user', ?, '2026-08-18 12:00:00')",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&thread_id)
            .bind(content)
            .execute(&pool)
            .await
            .unwrap();
        }

        let msgs = ChatMessagesRepository::list_for_thread(&pool, &thread_id)
            .await
            .unwrap();
        let contents: Vec<&str> = msgs.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, vec!["first", "second", "third"]);
    }

    #[tokio::test]
    async fn threads_crud_and_meeting_scoping() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        insert_meeting(&pool, "m2").await;

        let t1 = ChatThreadsRepository::create_thread(&pool, "m1", "Chat 1", "post")
            .await
            .unwrap();
        ChatThreadsRepository::create_thread(&pool, "m2", "Chat 1", "post")
            .await
            .unwrap();

        let threads = ChatThreadsRepository::list_for_meeting(&pool, "m1")
            .await
            .unwrap();
        assert_eq!(threads.len(), 1);
        assert_eq!(threads[0].id, t1.id);

        let found = ChatThreadsRepository::get_thread(&pool, &t1.id)
            .await
            .unwrap()
            .expect("thread should exist");
        assert_eq!(found.meeting_id, "m1");
        assert!(ChatThreadsRepository::get_thread(&pool, "thread-nope")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn delete_thread_removes_messages_and_search_index_rows() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        let thread_id = insert_thread(&pool, "m1").await;

        ChatMessagesRepository::add_message(&pool, "m1", &thread_id, "user", "findable-chat-text", None)
            .await
            .unwrap();

        let indexed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM search_index WHERE source = 'chat' AND meeting_id = 'm1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(indexed, 1, "chat insert should be FTS-indexed");

        ChatThreadsRepository::delete_thread(&pool, &thread_id)
            .await
            .unwrap();

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
        // The explicit per-message DELETE (not the FK cascade) must have fired
        // the FTS delete triggers.
        let indexed_after: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM search_index WHERE source = 'chat' AND meeting_id = 'm1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(indexed_after, 0, "thread delete must clean search_index");
    }

    #[tokio::test]
    async fn live_thread_preserves_timestamps_and_order() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;

        let base = Utc::now() - chrono::Duration::minutes(10);
        let messages = vec![
            NewChatMessage {
                role: "user".to_string(),
                content: "q1".to_string(),
                created_at: base,
                metadata: None,
            },
            NewChatMessage {
                role: "assistant".to_string(),
                content: "a1".to_string(),
                created_at: base,
                metadata: None,
            },
            NewChatMessage {
                role: "user".to_string(),
                content: "q2".to_string(),
                created_at: base + chrono::Duration::seconds(30),
                metadata: None,
            },
        ];

        let thread = ChatThreadsRepository::create_live_thread_with_messages(
            &pool,
            "m1",
            &messages,
            "web_search",
        )
        .await
        .unwrap();
        assert_eq!(thread.origin, "live");
        assert_eq!(thread.title, "Live chat");
        assert_eq!(thread.created_at, base);
        // The mode the Ask-AI panel was using has to reach the persisted thread,
        // or reopening a live conversation silently reverts to transcript-only.
        assert_eq!(thread.grounding_mode, "web_search");
        let stored: String =
            sqlx::query_scalar("SELECT grounding_mode FROM chat_threads WHERE id = ?")
                .bind(&thread.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, "web_search");

        let msgs = ChatMessagesRepository::list_for_thread(&pool, &thread.id)
            .await
            .unwrap();
        let contents: Vec<&str> = msgs.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, vec!["q1", "a1", "q2"]);
        assert_eq!(msgs[0].created_at.timestamp(), base.timestamp());
    }

    #[tokio::test]
    async fn threads_default_to_transcript_only_and_can_be_changed() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;

        let thread = ChatThreadsRepository::create_thread(&pool, "m1", "Chat 1", "post")
            .await
            .unwrap();
        assert_eq!(thread.grounding_mode, DEFAULT_GROUNDING_MODE);

        // The in-memory row must match what SQLite actually wrote.
        let reloaded = ChatThreadsRepository::get_thread(&pool, &thread.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.grounding_mode, DEFAULT_GROUNDING_MODE);

        let updated = ChatThreadsRepository::set_grounding_mode(&pool, &thread.id, "web_search")
            .await
            .unwrap();
        assert_eq!(updated, 1);
        let reloaded = ChatThreadsRepository::get_thread(&pool, &thread.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.grounding_mode, "web_search");

        // A missing thread reports zero rows rather than pretending to succeed.
        let missing = ChatThreadsRepository::set_grounding_mode(&pool, "nope", "web_search")
            .await
            .unwrap();
        assert_eq!(missing, 0);
    }

    #[tokio::test]
    async fn assistant_metadata_round_trips() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        let thread_id = insert_thread(&pool, "m1").await;

        ChatMessagesRepository::add_message(&pool, "m1", &thread_id, "user", "what is an ERP?", None)
            .await
            .unwrap();
        ChatMessagesRepository::add_message(
            &pool,
            "m1",
            &thread_id,
            "assistant",
            "An ERP is...",
            Some(r#"{"grounding":{"requested":"web_search","effective":"web_search"}}"#),
        )
        .await
        .unwrap();

        let msgs = ChatMessagesRepository::list_for_thread(&pool, &thread_id)
            .await
            .unwrap();
        assert_eq!(msgs[0].metadata, None, "user messages carry no metadata");
        assert!(msgs[1].metadata.as_deref().unwrap().contains("web_search"));
    }

    #[tokio::test]
    async fn meeting_cascade_removes_threads() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        let thread_id = insert_thread(&pool, "m1").await;
        ChatMessagesRepository::add_message(&pool, "m1", &thread_id, "user", "hello", None)
            .await
            .unwrap();

        sqlx::query("DELETE FROM meetings WHERE id = 'm1'")
            .execute(&pool)
            .await
            .unwrap();

        let threads: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_threads")
            .fetch_one(&pool)
            .await
            .unwrap();
        let msgs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_messages")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((threads, msgs), (0, 0));
    }
}
