//! Storage for the project-level chat: conversations about every meeting filed
//! under a project, rather than about one meeting.
//!
//! Deliberately a near-mirror of [`super::chat`] rather than a generalization of
//! it. The meeting chat tables cannot host a project row (`meeting_id` is
//! NOT NULL with an FK to `meetings`, and that NOT NULL cannot be relaxed
//! without a table rebuild, which the FTS5 rowid triggers forbid), so the two
//! schemas are separate and these repositories keep them behaving identically.

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::database::models::{ProjectChatMessageModel, ProjectChatThreadModel};

/// Column default in 20260819140000_add_project_chat — kept in sync here so
/// rows this repository builds in memory match what SQLite would have written.
pub const DEFAULT_GROUNDING_MODE: &str = "transcript_only";

/// Every project thread is 'post'; see the CHECK in the migration.
const PROJECT_THREAD_ORIGIN: &str = "post";

pub struct ProjectChatMessagesRepository;

impl ProjectChatMessagesRepository {
    /// `metadata` is JSON describing how an assistant answer was produced —
    /// grounding outcome, web citations, and which meetings were in context.
    /// Pass None for user messages.
    pub async fn add_message(
        pool: &SqlitePool,
        project_id: &str,
        thread_id: &str,
        role: &str,
        content: &str,
        metadata: Option<&str>,
    ) -> Result<ProjectChatMessageModel, sqlx::Error> {
        if role != "user" && role != "assistant" {
            return Err(sqlx::Error::Protocol(format!("Invalid chat role: {}", role)));
        }

        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now();

        sqlx::query(
            "INSERT INTO project_chat_messages \
                 (id, project_id, thread_id, role, content, created_at, metadata) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(project_id)
        .bind(thread_id)
        .bind(role)
        .bind(content)
        .bind(created_at)
        .bind(metadata)
        .execute(pool)
        .await?;

        Ok(ProjectChatMessageModel {
            id,
            project_id: project_id.to_string(),
            thread_id: thread_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at,
            metadata: metadata.map(str::to_string),
        })
    }

    pub async fn list_for_thread(
        pool: &SqlitePool,
        thread_id: &str,
    ) -> Result<Vec<ProjectChatMessageModel>, sqlx::Error> {
        // rowid tiebreaker: created_at has second resolution, so two rapid
        // inserts can collide; insertion order (rowid) settles the tie. Same
        // rule as chat_messages, which has a regression test for it.
        sqlx::query_as::<_, ProjectChatMessageModel>(
            "SELECT id, project_id, thread_id, role, content, created_at, metadata \
             FROM project_chat_messages WHERE thread_id = ? ORDER BY created_at ASC, rowid ASC",
        )
        .bind(thread_id)
        .fetch_all(pool)
        .await
    }

    pub async fn clear_for_thread(pool: &SqlitePool, thread_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM project_chat_messages WHERE thread_id = ?")
            .bind(thread_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn clear_for_project(
        pool: &SqlitePool,
        project_id: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM project_chat_messages WHERE project_id = ?")
            .bind(project_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }

    pub async fn delete_message(pool: &SqlitePool, message_id: &str) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM project_chat_messages WHERE id = ?")
            .bind(message_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }
}

pub struct ProjectChatThreadsRepository;

impl ProjectChatThreadsRepository {
    pub async fn create_thread(
        pool: &SqlitePool,
        project_id: &str,
        title: &str,
    ) -> Result<ProjectChatThreadModel, sqlx::Error> {
        let id = format!("pthread-{}", Uuid::new_v4());
        let created_at = Utc::now();
        sqlx::query(
            "INSERT INTO project_chat_threads (id, project_id, title, origin, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(project_id)
        .bind(title)
        .bind(PROJECT_THREAD_ORIGIN)
        .bind(created_at)
        .execute(pool)
        .await?;
        Ok(ProjectChatThreadModel {
            id,
            project_id: project_id.to_string(),
            title: title.to_string(),
            origin: PROJECT_THREAD_ORIGIN.to_string(),
            // Matches the column default in the migration.
            grounding_mode: DEFAULT_GROUNDING_MODE.to_string(),
            created_at,
        })
    }

    pub async fn list_for_project(
        pool: &SqlitePool,
        project_id: &str,
    ) -> Result<Vec<ProjectChatThreadModel>, sqlx::Error> {
        sqlx::query_as::<_, ProjectChatThreadModel>(
            "SELECT id, project_id, title, origin, grounding_mode, created_at \
             FROM project_chat_threads WHERE project_id = ? ORDER BY created_at ASC, rowid ASC",
        )
        .bind(project_id)
        .fetch_all(pool)
        .await
    }

    pub async fn get_thread(
        pool: &SqlitePool,
        thread_id: &str,
    ) -> Result<Option<ProjectChatThreadModel>, sqlx::Error> {
        sqlx::query_as::<_, ProjectChatThreadModel>(
            "SELECT id, project_id, title, origin, grounding_mode, created_at \
             FROM project_chat_threads WHERE id = ?",
        )
        .bind(thread_id)
        .fetch_optional(pool)
        .await
    }

    /// Change how far past the project's meetings a conversation may reach.
    /// Returns the number of rows updated, so callers can tell a missing thread
    /// from a no-op.
    pub async fn set_grounding_mode(
        pool: &SqlitePool,
        thread_id: &str,
        grounding_mode: &str,
    ) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("UPDATE project_chat_threads SET grounding_mode = ? WHERE id = ?")
            .bind(grounding_mode)
            .bind(thread_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Delete a thread and its messages, in one transaction.
    ///
    /// The messages are deleted explicitly rather than left to the FK cascade.
    /// Nothing here is FTS-indexed, so the cascade would in fact be correct —
    /// but doing it the same way as `ChatThreadsRepository::delete_thread`
    /// (where the explicit DELETE is load-bearing, because cascades do not fire
    /// the FTS triggers under recursive_triggers=OFF) means the two files cannot
    /// drift into different behaviour. It costs one statement.
    pub async fn delete_thread(pool: &SqlitePool, thread_id: &str) -> Result<u64, sqlx::Error> {
        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM project_chat_messages WHERE thread_id = ?")
            .bind(thread_id)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM project_chat_threads WHERE id = ?")
            .bind(thread_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::repositories::project::ProjectsRepository;
    use crate::database::test_support::migrated_pool;

    async fn insert_project(pool: &SqlitePool) -> String {
        ProjectsRepository::create(pool, "Client X", None, None)
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn add_list_and_clear_messages() {
        let pool = migrated_pool().await;
        let project_id = insert_project(&pool).await;
        let thread = ProjectChatThreadsRepository::create_thread(&pool, &project_id, "Chat 1")
            .await
            .unwrap();

        ProjectChatMessagesRepository::add_message(
            &pool, &project_id, &thread.id, "user", "hi", None,
        )
        .await
        .unwrap();
        ProjectChatMessagesRepository::add_message(
            &pool,
            &project_id,
            &thread.id,
            "assistant",
            "hello",
            Some(r#"{"grounding":{"requested":"web_search","effective":"web_search"}}"#),
        )
        .await
        .unwrap();

        let messages = ProjectChatMessagesRepository::list_for_thread(&pool, &thread.id)
            .await
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");
        assert!(messages[1].metadata.is_some());

        let cleared = ProjectChatMessagesRepository::clear_for_thread(&pool, &thread.id)
            .await
            .unwrap();
        assert_eq!(cleared, 2);
    }

    #[tokio::test]
    async fn rejects_unknown_roles() {
        let pool = migrated_pool().await;
        let project_id = insert_project(&pool).await;
        let thread = ProjectChatThreadsRepository::create_thread(&pool, &project_id, "Chat 1")
            .await
            .unwrap();

        let err = ProjectChatMessagesRepository::add_message(
            &pool, &project_id, &thread.id, "system", "nope", None,
        )
        .await;
        assert!(err.is_err(), "only user/assistant are storable roles");
    }

    /// created_at has second resolution, so same-second inserts must still come
    /// back in insertion order. Mirrors the meeting-chat regression test.
    #[tokio::test]
    async fn orders_same_second_messages_by_insertion() {
        let pool = migrated_pool().await;
        let project_id = insert_project(&pool).await;
        let thread = ProjectChatThreadsRepository::create_thread(&pool, &project_id, "Chat 1")
            .await
            .unwrap();

        let stamp = Utc::now();
        for (role, content) in [
            ("user", "first"),
            ("assistant", "second"),
            ("user", "third"),
            ("assistant", "fourth"),
        ] {
            sqlx::query(
                "INSERT INTO project_chat_messages \
                     (id, project_id, thread_id, role, content, created_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&project_id)
            .bind(&thread.id)
            .bind(role)
            .bind(content)
            .bind(stamp)
            .execute(&pool)
            .await
            .unwrap();
        }

        let messages = ProjectChatMessagesRepository::list_for_thread(&pool, &thread.id)
            .await
            .unwrap();
        let contents: Vec<&str> = messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, vec!["first", "second", "third", "fourth"]);
    }

    #[tokio::test]
    async fn threads_are_scoped_to_their_project_and_default_to_strict_grounding() {
        let pool = migrated_pool().await;
        let a = insert_project(&pool).await;
        let b = ProjectsRepository::create(&pool, "Client Y", None, None)
            .await
            .unwrap()
            .id;

        let thread = ProjectChatThreadsRepository::create_thread(&pool, &a, "Chat 1")
            .await
            .unwrap();
        ProjectChatThreadsRepository::create_thread(&pool, &b, "Chat 1")
            .await
            .unwrap();

        assert_eq!(thread.grounding_mode, DEFAULT_GROUNDING_MODE);
        assert_eq!(thread.origin, "post");

        let for_a = ProjectChatThreadsRepository::list_for_project(&pool, &a)
            .await
            .unwrap();
        assert_eq!(for_a.len(), 1);
        assert_eq!(for_a[0].id, thread.id);

        // The stored default matches what create_thread reported in memory.
        let reread = ProjectChatThreadsRepository::get_thread(&pool, &thread.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reread.grounding_mode, DEFAULT_GROUNDING_MODE);
    }

    #[tokio::test]
    async fn set_grounding_mode_reports_missing_threads() {
        let pool = migrated_pool().await;
        let project_id = insert_project(&pool).await;
        let thread = ProjectChatThreadsRepository::create_thread(&pool, &project_id, "Chat 1")
            .await
            .unwrap();

        let updated =
            ProjectChatThreadsRepository::set_grounding_mode(&pool, &thread.id, "web_search")
                .await
                .unwrap();
        assert_eq!(updated, 1);

        let missing =
            ProjectChatThreadsRepository::set_grounding_mode(&pool, "pthread-nope", "web_search")
                .await
                .unwrap();
        assert_eq!(missing, 0, "a missing thread is a no-op, not an error");
    }

    #[tokio::test]
    async fn delete_thread_removes_its_messages() {
        let pool = migrated_pool().await;
        let project_id = insert_project(&pool).await;
        let thread = ProjectChatThreadsRepository::create_thread(&pool, &project_id, "Chat 1")
            .await
            .unwrap();
        ProjectChatMessagesRepository::add_message(
            &pool, &project_id, &thread.id, "user", "hi", None,
        )
        .await
        .unwrap();

        ProjectChatThreadsRepository::delete_thread(&pool, &thread.id)
            .await
            .unwrap();

        let remaining: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM project_chat_messages WHERE thread_id = ?")
                .bind(&thread.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(remaining, 0);
    }
}
