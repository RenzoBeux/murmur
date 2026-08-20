//! Project CRUD and meeting-to-project assignment.
//!
//! A project is a named folder of meetings; a meeting belongs to at most one
//! (`meetings.project_id`). Everything here is a thin wrapper over
//! `ProjectsRepository` — see `database/repositories/project.rs`.

use log::{error as log_error, info as log_info};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};

use crate::{
    api::{with_durations, Meeting},
    database::{models::ProjectModel, repositories::project::ProjectsRepository},
    state::AppState,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Palette slug, or None when the project predates the color picker — the
    /// UI derives a stable color from the id in that case.
    pub color: Option<String>,
    /// Free-form background the user wrote for the AI, from the Notes tab.
    #[serde(rename = "contextNotes")]
    pub context_notes: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    /// Live (non-trashed) meetings filed under this project.
    #[serde(rename = "meetingCount")]
    pub meeting_count: i64,
}

impl Project {
    fn from_model(model: ProjectModel, meeting_count: i64) -> Self {
        Project {
            id: model.id,
            name: model.name,
            description: model.description,
            color: model.color,
            context_notes: model.context_notes,
            created_at: model.created_at.to_rfc3339(),
            updated_at: model.updated_at.to_rfc3339(),
            meeting_count,
        }
    }
}

/// Every project with its meeting count, name-sorted.
#[tauri::command]
pub async fn api_list_projects<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Project>, String> {
    let pool = state.db_manager.pool();
    ProjectsRepository::list_with_counts(pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|(project, count)| Project::from_model(project, count))
                .collect()
        })
        .map_err(|e| {
            log_error!("Error listing projects: {}", e);
            format!("Failed to list projects: {}", e)
        })
}

/// A single project, or an error if it no longer exists (e.g. deleted in
/// another window while its detail page was open).
#[tauri::command]
pub async fn api_get_project<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Project, String> {
    let pool = state.db_manager.pool();
    let project = ProjectsRepository::get(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to get project: {}", e))?
        .ok_or_else(|| format!("Project not found: {}", project_id))?;
    let count = ProjectsRepository::count_meetings(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to count project meetings: {}", e))?;
    Ok(Project::from_model(project, count))
}

#[tauri::command]
pub async fn api_create_project<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    name: String,
    description: Option<String>,
    color: Option<String>,
) -> Result<Project, String> {
    let pool = state.db_manager.pool();
    let project = ProjectsRepository::create(pool, &name, description.as_deref(), color.as_deref())
        .await
        .map_err(|e| format!("Failed to create project: {}", e))?;
    log_info!("Created project {} ({})", project.name, project.id);
    Ok(Project::from_model(project, 0))
}

/// Rename a project and/or replace its description and accent color. A
/// None/blank description or color clears it.
#[tauri::command]
pub async fn api_update_project<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    project_id: String,
    name: String,
    description: Option<String>,
    color: Option<String>,
) -> Result<Project, String> {
    let pool = state.db_manager.pool();
    let project = ProjectsRepository::update(
        pool,
        &project_id,
        &name,
        description.as_deref(),
        color.as_deref(),
    )
    .await
        .map_err(|e| format!("Failed to update project: {}", e))?
        .ok_or_else(|| format!("Project not found: {}", project_id))?;
    let count = ProjectsRepository::count_meetings(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to count project meetings: {}", e))?;
    Ok(Project::from_model(project, count))
}

/// Delete a project. Its meetings are unfiled, never deleted.
#[tauri::command]
pub async fn api_delete_project<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let pool = state.db_manager.pool();

    // Stop any brief still being generated for this project first. The row goes
    // away with the project either way, but without this the LLM calls keep
    // running — and paying — against something that no longer exists.
    crate::summary::service::SummaryService::cancel_summary(
        &crate::summary::project_service::project_cancellation_key(&project_id),
    );

    let deleted = ProjectsRepository::delete(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to delete project: {}", e))?;
    if !deleted {
        return Err(format!("Project not found: {}", project_id));
    }
    log_info!("Deleted project {}", project_id);
    Ok(())
}

/// Save the project's context notes — the background the AI reads.
///
/// Its own command rather than part of `api_update_project` because the Notes
/// tab autosaves while you type: routing that through the create/edit payload
/// would rewrite name, description and color on every keystroke, and would race
/// the edit dialog if both were open.
#[tauri::command]
pub async fn api_set_project_context_notes<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    project_id: String,
    notes: Option<String>,
) -> Result<(), String> {
    let pool = state.db_manager.pool();
    let saved = ProjectsRepository::set_context_notes(pool, &project_id, notes.as_deref())
        .await
        .map_err(|e| format!("Failed to save project notes: {}", e))?;
    if !saved {
        return Err(format!("Project not found: {}", project_id));
    }
    Ok(())
}

/// Live meetings in a project, newest first.
#[tauri::command]
pub async fn api_get_project_meetings<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Vec<Meeting>, String> {
    let pool = state.db_manager.pool();
    let models = ProjectsRepository::list_meetings(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to list project meetings: {}", e))?;
    Ok(with_durations(pool, models).await)
}

/// Move meetings into a project, or out of every project when `projectId` is
/// null. Returns how many meetings changed.
#[tauri::command]
pub async fn api_assign_meetings_to_project<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_ids: Vec<String>,
    project_id: Option<String>,
) -> Result<u64, String> {
    let pool = state.db_manager.pool();
    let moved = ProjectsRepository::assign_meetings(pool, &meeting_ids, project_id.as_deref())
        .await
        .map_err(|e| format!("Failed to move meetings: {}", e))?;
    log_info!(
        "Moved {} meeting(s) to project {:?}",
        moved,
        project_id.as_deref().unwrap_or("<none>")
    );
    Ok(moved)
}

/// The project a meeting is filed under, or null when unfiled.
#[tauri::command]
pub async fn api_get_meeting_project<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Option<Project>, String> {
    let pool = state.db_manager.pool();
    let Some(project) = ProjectsRepository::for_meeting(pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to get meeting project: {}", e))?
    else {
        return Ok(None);
    };
    let count = ProjectsRepository::count_meetings(pool, &project.id)
        .await
        .map_err(|e| format!("Failed to count project meetings: {}", e))?;
    Ok(Some(Project::from_model(project, count)))
}
