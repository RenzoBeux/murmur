use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::database::models::ChatMessageModel;

pub struct ChatMessagesRepository;

impl ChatMessagesRepository {
    pub async fn add_message(
        pool: &SqlitePool,
        meeting_id: &str,
        role: &str,
        content: &str,
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
            "INSERT INTO chat_messages (id, meeting_id, role, content, created_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(meeting_id)
        .bind(role)
        .bind(content)
        .bind(created_at)
        .execute(pool)
        .await?;

        Ok(ChatMessageModel {
            id,
            meeting_id: meeting_id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at,
        })
    }

    pub async fn list_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<ChatMessageModel>, sqlx::Error> {
        sqlx::query_as::<_, ChatMessageModel>(
            "SELECT id, meeting_id, role, content, created_at \
             FROM chat_messages WHERE meeting_id = ? ORDER BY created_at ASC",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await
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
