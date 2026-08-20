use chrono::{DateTime, Utc};
use sqlx::{Error as SqlxError, FromRow, SqlitePool};
use uuid::Uuid;

use crate::database::models::{MeetingModel, ProjectModel};

/// A project row plus its live (non-trashed) meeting count, for the projects list.
#[derive(Debug, Clone, FromRow)]
struct ProjectCountRow {
    id: String,
    name: String,
    description: Option<String>,
    color: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    meeting_count: i64,
}

impl ProjectCountRow {
    fn split(self) -> (ProjectModel, i64) {
        let count = self.meeting_count;
        (
            ProjectModel {
                id: self.id,
                name: self.name,
                description: self.description,
                color: self.color,
                created_at: self.created_at,
                updated_at: self.updated_at,
            },
            count,
        )
    }
}

/// Trim a project name and reject it if nothing is left. Names are free text;
/// duplicates are allowed (two "Q3 planning" projects are the user's call).
fn clean_name(name: &str) -> Result<String, SqlxError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(SqlxError::Protocol(
            "Project name must not be empty".to_string(),
        ));
    }
    Ok(name.to_string())
}

/// An empty/whitespace description is stored as NULL, so "no description" has
/// exactly one representation.
fn clean_description(description: Option<&str>) -> Option<String> {
    description
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string)
}

/// The accent colors a project can carry. Slugs, not hex values: each maps to a
/// `--project-*` theme token tuned separately for the light and dark themes,
/// the same way the speaker palette works. Must stay in sync with
/// `PROJECT_COLORS` in `frontend/src/lib/projectColors.ts`.
pub const PROJECT_COLORS: [&str; 8] = [
    "violet", "blue", "cyan", "teal", "green", "amber", "orange", "rose",
];

/// Normalize a color choice. None/blank means "unset" — the UI then derives a
/// stable color from the project id. Anything outside the palette is rejected
/// rather than stored, so a value that has no theme token can never get in.
fn clean_color(color: Option<&str>) -> Result<Option<String>, SqlxError> {
    let Some(color) = color.map(str::trim).filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    let color = color.to_ascii_lowercase();
    if !PROJECT_COLORS.contains(&color.as_str()) {
        return Err(SqlxError::Protocol(format!(
            "Unknown project color: {} (expected one of {})",
            color,
            PROJECT_COLORS.join(", ")
        )));
    }
    Ok(Some(color))
}

pub struct ProjectsRepository;

impl ProjectsRepository {
    pub async fn create(
        pool: &SqlitePool,
        name: &str,
        description: Option<&str>,
        color: Option<&str>,
    ) -> Result<ProjectModel, SqlxError> {
        let name = clean_name(name)?;
        let description = clean_description(description);
        let color = clean_color(color)?;
        let id = format!("project-{}", Uuid::new_v4());
        let now = Utc::now();

        sqlx::query(
            "INSERT INTO projects (id, name, description, color, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&name)
        .bind(&description)
        .bind(&color)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;

        Ok(ProjectModel {
            id,
            name,
            description,
            color,
            created_at: now,
            updated_at: now,
        })
    }

    /// Every project with its live meeting count, name-sorted (case-insensitive).
    /// Trashed meetings keep their project_id but are not counted — restoring one
    /// puts it back in its project.
    pub async fn list_with_counts(pool: &SqlitePool) -> Result<Vec<(ProjectModel, i64)>, SqlxError> {
        let rows = sqlx::query_as::<_, ProjectCountRow>(
            "SELECT p.id, p.name, p.description, p.color, p.created_at, p.updated_at, \
                    (SELECT COUNT(*) FROM meetings m \
                      WHERE m.project_id = p.id AND m.deleted_at IS NULL) AS meeting_count \
             FROM projects p ORDER BY p.name COLLATE NOCASE",
        )
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(ProjectCountRow::split).collect())
    }

    pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<ProjectModel>, SqlxError> {
        sqlx::query_as::<_, ProjectModel>(
            "SELECT id, name, description, color, created_at, updated_at FROM projects WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    /// Live meetings in a project, newest first.
    pub async fn list_meetings(
        pool: &SqlitePool,
        project_id: &str,
    ) -> Result<Vec<MeetingModel>, SqlxError> {
        sqlx::query_as::<_, MeetingModel>(
            "SELECT * FROM meetings WHERE project_id = ? AND deleted_at IS NULL \
             ORDER BY created_at DESC",
        )
        .bind(project_id)
        .fetch_all(pool)
        .await
    }

    pub async fn count_meetings(pool: &SqlitePool, project_id: &str) -> Result<i64, SqlxError> {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM meetings WHERE project_id = ? AND deleted_at IS NULL",
        )
        .bind(project_id)
        .fetch_one(pool)
        .await
    }

    /// Rename / re-describe / re-color a project. Returns the updated row, or
    /// None if no project has that id. A None description or color clears it.
    pub async fn update(
        pool: &SqlitePool,
        id: &str,
        name: &str,
        description: Option<&str>,
        color: Option<&str>,
    ) -> Result<Option<ProjectModel>, SqlxError> {
        let name = clean_name(name)?;
        let description = clean_description(description);
        let color = clean_color(color)?;
        let now = Utc::now();

        let result = sqlx::query(
            "UPDATE projects SET name = ?, description = ?, color = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&name)
        .bind(&description)
        .bind(&color)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;

        if result.rows_affected() == 0 {
            return Ok(None);
        }
        // Re-read instead of synthesizing the row, so the caller gets the real
        // created_at rather than a made-up one.
        Self::get(pool, id).await
    }

    /// Delete a project. Its meetings are unfiled (project_id -> NULL), never
    /// deleted. The UPDATE is explicit rather than relying on the FK's
    /// ON DELETE SET NULL, which is a no-op when foreign_keys is OFF.
    ///
    /// The project's own AI artifacts — its chat conversations and its stored
    /// brief — go the other way: they are about the folder, so they die with it.
    /// Those DELETEs are explicit for the same reason the UPDATE above is, and
    /// they run children-first so the order holds with or without FK support.
    pub async fn delete(pool: &SqlitePool, id: &str) -> Result<bool, SqlxError> {
        let mut tx = pool.begin().await?;
        sqlx::query("UPDATE meetings SET project_id = NULL WHERE project_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM project_chat_messages WHERE project_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM project_chat_threads WHERE project_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM project_summaries WHERE project_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        let result = sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    /// Move meetings into a project, or out of one when `project_id` is None.
    /// Returns how many meeting rows changed. A non-existent project id is
    /// rejected up front — with foreign_keys OFF the FK would not catch it and
    /// the meetings would land in a project that can never be opened.
    pub async fn assign_meetings(
        pool: &SqlitePool,
        meeting_ids: &[String],
        project_id: Option<&str>,
    ) -> Result<u64, SqlxError> {
        if meeting_ids.is_empty() {
            return Ok(0);
        }
        if let Some(pid) = project_id {
            if Self::get(pool, pid).await?.is_none() {
                return Err(SqlxError::Protocol(format!("Unknown project: {}", pid)));
            }
        }

        let placeholders = vec!["?"; meeting_ids.len()].join(", ");
        let sql = format!(
            "UPDATE meetings SET project_id = ?, updated_at = ? WHERE id IN ({})",
            placeholders
        );
        let mut query = sqlx::query(&sql).bind(project_id).bind(Utc::now());
        for id in meeting_ids {
            query = query.bind(id);
        }
        Ok(query.execute(pool).await?.rows_affected())
    }

    /// The project a meeting belongs to, if any (for the meeting-details badge).
    pub async fn for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<ProjectModel>, SqlxError> {
        sqlx::query_as::<_, ProjectModel>(
            "SELECT p.id, p.name, p.description, p.color, p.created_at, p.updated_at \
             FROM projects p JOIN meetings m ON m.project_id = p.id WHERE m.id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::test_support::migrated_pool;

    async fn insert_meeting(pool: &SqlitePool, id: &str) {
        sqlx::query(
            "INSERT INTO meetings (id, title, created_at, updated_at) \
             VALUES (?, ?, datetime('now'), datetime('now'))",
        )
        .bind(id)
        .bind(format!("Meeting {}", id))
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_list_update_and_delete() {
        let pool = migrated_pool().await;

        let zeta = ProjectsRepository::create(&pool, " Zeta ", Some("  "), Some("  "))
            .await
            .unwrap();
        assert_eq!(zeta.name, "Zeta", "name is trimmed");
        assert_eq!(zeta.description, None, "blank description becomes NULL");
        assert_eq!(zeta.color, None, "blank color becomes NULL");

        let alpha = ProjectsRepository::create(&pool, "alpha", Some("First one"), Some("BLUE"))
            .await
            .unwrap();

        // Name-sorted, case-insensitively: alpha before Zeta.
        let listed = ProjectsRepository::list_with_counts(&pool).await.unwrap();
        assert_eq!(
            listed.iter().map(|(p, _)| p.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "Zeta"]
        );
        assert!(listed.iter().all(|(_, n)| *n == 0));

        let updated = ProjectsRepository::update(&pool, &alpha.id, "Alpha", None, Some("rose"))
            .await
            .unwrap()
            .expect("project exists");
        assert_eq!(updated.name, "Alpha");
        assert_eq!(updated.description, None, "description can be cleared");
        assert_eq!(updated.color.as_deref(), Some("rose"), "color can be changed");
        assert_eq!(
            updated.created_at, alpha.created_at,
            "update preserves created_at"
        );

        assert!(ProjectsRepository::update(&pool, "nope", "X", None, None)
            .await
            .unwrap()
            .is_none());
        assert!(
            ProjectsRepository::create(&pool, "   ", None, None).await.is_err(),
            "empty name is rejected"
        );
        assert_eq!(
            alpha.color.as_deref(),
            Some("blue"),
            "a palette color is stored lowercased"
        );
        assert!(
            ProjectsRepository::create(&pool, "Bad", None, Some("chartreuse"))
                .await
                .is_err(),
            "a color outside the palette is rejected"
        );

        assert!(ProjectsRepository::delete(&pool, &zeta.id).await.unwrap());
        assert!(!ProjectsRepository::delete(&pool, &zeta.id).await.unwrap());
        assert_eq!(ProjectsRepository::list_with_counts(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn assign_meetings_moves_and_unfiles() {
        let pool = migrated_pool().await;
        let project = ProjectsRepository::create(&pool, "Client X", None, None)
            .await
            .unwrap();
        for id in ["m1", "m2", "m3"] {
            insert_meeting(&pool, id).await;
        }

        let moved = ProjectsRepository::assign_meetings(
            &pool,
            &["m1".to_string(), "m2".to_string()],
            Some(&project.id),
        )
        .await
        .unwrap();
        assert_eq!(moved, 2);

        let members = ProjectsRepository::list_meetings(&pool, &project.id)
            .await
            .unwrap();
        assert_eq!(members.len(), 2);
        assert!(members.iter().all(|m| m.project_id.as_deref() == Some(project.id.as_str())));
        assert_eq!(
            ProjectsRepository::for_meeting(&pool, "m1").await.unwrap().unwrap().id,
            project.id
        );
        assert!(ProjectsRepository::for_meeting(&pool, "m3").await.unwrap().is_none());

        // Unfiling one leaves the other in place.
        ProjectsRepository::assign_meetings(&pool, &["m1".to_string()], None)
            .await
            .unwrap();
        assert_eq!(
            ProjectsRepository::count_meetings(&pool, &project.id).await.unwrap(),
            1
        );

        assert!(
            ProjectsRepository::assign_meetings(&pool, &["m3".to_string()], Some("ghost"))
                .await
                .is_err(),
            "assigning to a non-existent project is rejected"
        );
        assert_eq!(
            ProjectsRepository::assign_meetings(&pool, &[], Some(&project.id))
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn counts_skip_trashed_and_delete_unfiles_meetings() {
        let pool = migrated_pool().await;
        let project = ProjectsRepository::create(&pool, "Ops", None, None).await.unwrap();
        insert_meeting(&pool, "m1").await;
        insert_meeting(&pool, "m2").await;
        ProjectsRepository::assign_meetings(
            &pool,
            &["m1".to_string(), "m2".to_string()],
            Some(&project.id),
        )
        .await
        .unwrap();

        sqlx::query("UPDATE meetings SET deleted_at = datetime('now') WHERE id = 'm2'")
            .execute(&pool)
            .await
            .unwrap();

        // A trashed meeting keeps its project_id but drops out of the count and
        // the listing, so restoring it puts it straight back in the project.
        assert_eq!(
            ProjectsRepository::count_meetings(&pool, &project.id).await.unwrap(),
            1
        );
        assert_eq!(
            ProjectsRepository::list_with_counts(&pool).await.unwrap()[0].1,
            1
        );

        ProjectsRepository::delete(&pool, &project.id).await.unwrap();
        let orphaned: Vec<Option<String>> =
            sqlx::query_scalar("SELECT project_id FROM meetings ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            orphaned,
            vec![None, None],
            "deleting a project unfiles its meetings, trashed ones included"
        );
    }
}
