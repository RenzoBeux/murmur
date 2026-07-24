use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::database::models::MeetingAttachmentModel;

pub struct AttachmentsRepository;

impl AttachmentsRepository {
    pub async fn add(
        pool: &SqlitePool,
        meeting_id: &str,
        file_name: &str,
        stored_name: &str,
        mime_type: &str,
        size_bytes: i64,
    ) -> Result<MeetingAttachmentModel, sqlx::Error> {
        let id = Uuid::new_v4().to_string();
        let created_at = Utc::now();

        sqlx::query(
            "INSERT INTO meeting_attachments \
             (id, meeting_id, file_name, stored_name, mime_type, size_bytes, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(meeting_id)
        .bind(file_name)
        .bind(stored_name)
        .bind(mime_type)
        .bind(size_bytes)
        .bind(created_at)
        .execute(pool)
        .await?;

        Ok(MeetingAttachmentModel {
            id,
            meeting_id: meeting_id.to_string(),
            file_name: file_name.to_string(),
            stored_name: stored_name.to_string(),
            mime_type: mime_type.to_string(),
            size_bytes,
            created_at,
        })
    }

    pub async fn list_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Vec<MeetingAttachmentModel>, sqlx::Error> {
        sqlx::query_as::<_, MeetingAttachmentModel>(
            "SELECT id, meeting_id, file_name, stored_name, mime_type, size_bytes, created_at \
             FROM meeting_attachments WHERE meeting_id = ? ORDER BY created_at ASC, id ASC",
        )
        .bind(meeting_id)
        .fetch_all(pool)
        .await
    }

    pub async fn get(
        pool: &SqlitePool,
        attachment_id: &str,
    ) -> Result<Option<MeetingAttachmentModel>, sqlx::Error> {
        sqlx::query_as::<_, MeetingAttachmentModel>(
            "SELECT id, meeting_id, file_name, stored_name, mime_type, size_bytes, created_at \
             FROM meeting_attachments WHERE id = ?",
        )
        .bind(attachment_id)
        .fetch_optional(pool)
        .await
    }

    pub async fn delete(pool: &SqlitePool, attachment_id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM meeting_attachments WHERE id = ?")
            .bind(attachment_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
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
    async fn add_list_get_delete_roundtrip() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;

        let a = AttachmentsRepository::add(&pool, "m1", "photo.png", "photo.png", "image/png", 123)
            .await
            .unwrap();
        AttachmentsRepository::add(&pool, "m1", "photo.png", "photo-1.png", "image/png", 456)
            .await
            .unwrap();

        let list = AttachmentsRepository::list_for_meeting(&pool, "m1")
            .await
            .unwrap();
        assert_eq!(list.len(), 2);

        let got = AttachmentsRepository::get(&pool, &a.id).await.unwrap();
        assert_eq!(got.unwrap().stored_name, "photo.png");

        assert!(AttachmentsRepository::delete(&pool, &a.id).await.unwrap());
        assert!(!AttachmentsRepository::delete(&pool, &a.id).await.unwrap());
        assert_eq!(
            AttachmentsRepository::list_for_meeting(&pool, "m1")
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn purge_removes_attachment_rows() {
        let pool = migrated_pool().await;
        insert_meeting(&pool, "m1").await;
        AttachmentsRepository::add(&pool, "m1", "doc.pdf", "doc.pdf", "application/pdf", 9)
            .await
            .unwrap();

        assert!(
            crate::database::repositories::meeting::MeetingsRepository::purge_meeting(&pool, "m1")
                .await
                .unwrap()
        );

        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meeting_attachments")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "hard purge must remove attachment rows");
    }
}
