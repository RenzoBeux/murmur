//! Tauri commands for the project-level chat.
//!
//! A one-for-one mirror of `chat_api`'s meeting commands, including the order of
//! operations in [`api_send_project_chat_message`], which is load-bearing:
//! history is read before the new question is stored, the question is stored
//! immediately so a crash cannot lose it, and it is deleted again if the model
//! call fails so no conversation is left with a dangling unanswered turn.
//!
//! The payloads serialize snake_case, identical in shape to the meeting chat's,
//! so the frontend's message list, thread switcher and grounding picker work
//! against both with no adapters.

use log::{error as log_error, info as log_info};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};

use crate::api::chat_common::{
    build_project_answer_metadata, build_user_prompt, resolve_grounding, resolve_llm_config,
    ChatAnswerMetadata, ChatGrounding,
};
use crate::api::project_chat_context::{
    build_project_context, build_project_system_prompt, project_context_char_budget,
    project_meeting_availability, ProjectMeetingContextEntry,
};
use crate::database::repositories::{
    project::ProjectsRepository,
    project_chat::{ProjectChatMessagesRepository, ProjectChatThreadsRepository},
};
use crate::state::AppState;
use crate::summary::llm_client::{generate_answer, LlmExtras};

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectChatMessage {
    pub id: String,
    pub project_id: String,
    pub thread_id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ChatAnswerMetadata>,
}

impl From<crate::database::models::ProjectChatMessageModel> for ProjectChatMessage {
    fn from(m: crate::database::models::ProjectChatMessageModel) -> Self {
        // A metadata blob that no longer parses must not sink the whole message:
        // the answer text is the thing the user came for.
        let metadata = m.metadata.as_deref().and_then(|raw| {
            serde_json::from_str::<ChatAnswerMetadata>(raw)
                .map_err(|e| log_error!("Ignoring unreadable chat metadata: {}", e))
                .ok()
        });
        Self {
            id: m.id,
            project_id: m.project_id,
            thread_id: m.thread_id,
            role: m.role,
            content: m.content,
            created_at: m.created_at.to_rfc3339(),
            metadata,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectChatThread {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub origin: String,
    pub grounding_mode: ChatGrounding,
    pub created_at: String,
}

impl From<crate::database::models::ProjectChatThreadModel> for ProjectChatThread {
    fn from(t: crate::database::models::ProjectChatThreadModel) -> Self {
        Self {
            id: t.id,
            project_id: t.project_id,
            title: t.title,
            origin: t.origin,
            grounding_mode: ChatGrounding::parse(&t.grounding_mode),
            created_at: t.created_at.to_rfc3339(),
        }
    }
}

/// Verify a thread exists and belongs to the given project.
async fn require_project_thread(
    pool: &sqlx::SqlitePool,
    project_id: &str,
    thread_id: &str,
) -> Result<crate::database::models::ProjectChatThreadModel, String> {
    let thread = ProjectChatThreadsRepository::get_thread(pool, thread_id)
        .await
        .map_err(|e| format!("Failed to load chat thread: {}", e))?
        .ok_or_else(|| format!("Chat thread {} not found", thread_id))?;
    if thread.project_id != project_id {
        return Err(format!(
            "Chat thread {} does not belong to project {}",
            thread_id, project_id
        ));
    }
    Ok(thread)
}

#[tauri::command]
pub async fn api_list_project_chat_threads(
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Vec<ProjectChatThread>, String> {
    let threads =
        ProjectChatThreadsRepository::list_for_project(state.db_manager.pool(), &project_id)
            .await
            .map_err(|e| format!("Failed to list chat threads: {}", e))?;
    Ok(threads.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub async fn api_create_project_chat_thread(
    state: tauri::State<'_, AppState>,
    project_id: String,
    title: Option<String>,
) -> Result<ProjectChatThread, String> {
    let pool = state.db_manager.pool();
    if project_id.trim().is_empty() {
        return Err("project_id is required".to_string());
    }

    let existing = ProjectChatThreadsRepository::list_for_project(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to list chat threads: {}", e))?;
    let title = title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| format!("Chat {}", existing.len() + 1));

    let thread = ProjectChatThreadsRepository::create_thread(pool, &project_id, &title)
        .await
        .map_err(|e| format!("Failed to create chat thread: {}", e))?;
    Ok(thread.into())
}

#[tauri::command]
pub async fn api_delete_project_chat_thread(
    state: tauri::State<'_, AppState>,
    project_id: String,
    thread_id: String,
) -> Result<(), String> {
    let pool = state.db_manager.pool();
    require_project_thread(pool, &project_id, &thread_id).await?;
    ProjectChatThreadsRepository::delete_thread(pool, &thread_id)
        .await
        .map_err(|e| format!("Failed to delete chat thread: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn api_set_project_chat_thread_grounding(
    state: tauri::State<'_, AppState>,
    project_id: String,
    thread_id: String,
    grounding: String,
) -> Result<ProjectChatThread, String> {
    let pool = state.db_manager.pool();
    require_project_thread(pool, &project_id, &thread_id).await?;

    // Parse rather than trusting the string: an unknown value falls back to the
    // strictest mode, so a malformed request can never widen what the chat may do.
    let mode = ChatGrounding::parse(&grounding);
    ProjectChatThreadsRepository::set_grounding_mode(pool, &thread_id, mode.as_str())
        .await
        .map_err(|e| format!("Failed to update chat grounding: {}", e))?;

    let thread = require_project_thread(pool, &project_id, &thread_id).await?;
    Ok(thread.into())
}

#[tauri::command]
pub async fn api_get_project_chat_history(
    state: tauri::State<'_, AppState>,
    project_id: String,
    thread_id: String,
) -> Result<Vec<ProjectChatMessage>, String> {
    let pool = state.db_manager.pool();
    require_project_thread(pool, &project_id, &thread_id).await?;
    let messages = ProjectChatMessagesRepository::list_for_thread(pool, &thread_id)
        .await
        .map_err(|e| format!("Failed to load chat history: {}", e))?;
    Ok(messages.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub async fn api_clear_project_chat_history(
    state: tauri::State<'_, AppState>,
    project_id: String,
    thread_id: String,
) -> Result<(), String> {
    let pool = state.db_manager.pool();
    require_project_thread(pool, &project_id, &thread_id).await?;
    ProjectChatMessagesRepository::clear_for_thread(pool, &thread_id)
        .await
        .map_err(|e| format!("Failed to clear chat history: {}", e))?;
    Ok(())
}

/// What this project's meetings currently offer the chat, for the UI's
/// "what can it see" notice.
#[tauri::command]
pub async fn api_project_chat_context(
    state: tauri::State<'_, AppState>,
    project_id: String,
) -> Result<Vec<ProjectMeetingContextEntry>, String> {
    let pool = state.db_manager.pool();
    let meetings = ProjectsRepository::list_meetings(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to load project meetings: {}", e))?;
    project_meeting_availability(pool, &meetings).await
}

#[tauri::command]
pub async fn api_send_project_chat_message<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    project_id: String,
    thread_id: String,
    message: String,
    provider: String,
    model: String,
) -> Result<ProjectChatMessage, String> {
    let trimmed_message = message.trim();
    if project_id.trim().is_empty() {
        return Err("project_id is required".to_string());
    }
    if thread_id.trim().is_empty() {
        return Err("thread_id is required".to_string());
    }
    if trimmed_message.is_empty() {
        return Err("message cannot be empty".to_string());
    }
    if provider.trim().is_empty() || model.trim().is_empty() {
        return Err("provider and model are required".to_string());
    }

    let pool = state.db_manager.pool();

    let project = ProjectsRepository::get(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to load project: {}", e))?
        .ok_or_else(|| format!("Project {} not found", project_id))?;

    let thread = require_project_thread(pool, &project_id, &thread_id).await?;
    let requested_grounding = ChatGrounding::parse(&thread.grounding_mode);

    // Oldest first: the context reads as a chronology, and "later supersedes
    // earlier" is only a usable instruction if the order is stated.
    let mut meetings = ProjectsRepository::list_meetings(pool, &project_id)
        .await
        .map_err(|e| format!("Failed to load project meetings: {}", e))?;
    meetings.reverse();

    if meetings.is_empty() {
        return Err(
            "This project has no meetings yet. Add meetings to it before asking questions."
                .to_string(),
        );
    }

    log_info!(
        "api_send_project_chat_message: project={} thread={} meetings={} provider={} model={} ({} chars)",
        project_id,
        thread_id,
        meetings.len(),
        provider,
        model,
        trimmed_message.len()
    );

    // Load chat history BEFORE persisting the new user message so the LLM sees
    // the prior conversation followed by the current question.
    let history_raw = ProjectChatMessagesRepository::list_for_thread(pool, &thread_id)
        .await
        .map_err(|e| format!("Failed to load chat history: {}", e))?;

    // Persist the user message immediately so it isn't lost if the call fails.
    let user_msg = ProjectChatMessagesRepository::add_message(
        pool,
        &project_id,
        &thread_id,
        "user",
        trimmed_message,
        None,
    )
    .await
    .map_err(|e| format!("Failed to save user message: {}", e))?;

    let config = resolve_llm_config(&app, pool, &provider).await?;

    let budget = project_context_char_budget(
        &config.provider_enum,
        &model,
        config.ollama_endpoint.as_deref(),
    )
    .await;

    let context = match build_project_context(pool, &project, &meetings, budget).await {
        Ok(context) => context,
        Err(e) => {
            let _ = ProjectChatMessagesRepository::delete_message(pool, &user_msg.id).await;
            return Err(e);
        }
    };

    // Build the prompt for the mode that will actually run, so a model that
    // cannot search is never told to search.
    let (effective_grounding, degraded_reason) =
        resolve_grounding(requested_grounding, &config.provider_enum, &model);
    let system_prompt =
        build_project_system_prompt(&context.text, &context.info, effective_grounding);

    let history: Vec<(&str, &str)> = history_raw
        .iter()
        .map(|m| (m.role.as_str(), m.content.as_str()))
        .collect();
    let user_prompt = build_user_prompt(&history, trimmed_message);

    let extras = LlmExtras {
        web_search: effective_grounding == ChatGrounding::WebSearch,
    };

    // No images: `build_attachment_context` is scoped to one meeting and caps at
    // 4 images / 15 MB. Across N meetings there is no honest way to choose which
    // handful to send, and re-encoding megabytes of base64 on every turn would
    // dominate latency. The per-meeting chat still carries attachments.
    let client = reqwest::Client::new();
    let answer_result = generate_answer(
        &client,
        &config.provider_enum,
        &model,
        &config.api_key,
        &system_prompt,
        &user_prompt,
        &[],
        config.ollama_endpoint.as_deref(),
        config.custom_openai_endpoint.as_deref(),
        config.lmstudio_endpoint.as_deref(),
        config.custom_openai_max_tokens,
        config.custom_openai_temperature,
        config.custom_openai_top_p,
        config.app_data_dir.as_ref(),
        None,
        extras,
    )
    .await;

    let answer = match answer_result {
        Ok(answer) => answer,
        Err(e) => {
            log_error!("Project chat LLM call failed for {}: {}", project_id, e);
            // Roll back the user message so the conversation isn't left dangling
            // with a question that has no response.
            let _ = ProjectChatMessagesRepository::delete_message(pool, &user_msg.id).await;
            return Err(format!("Chat failed: {}", e));
        }
    };

    // The ChatGPT backend only reveals whether it accepts web search by being
    // asked, so re-resolve after the call: a first-time rejection degrades this
    // very answer, and the label has to say so rather than claiming a search.
    let (effective_grounding, degraded_reason) = if effective_grounding == ChatGrounding::WebSearch {
        resolve_grounding(requested_grounding, &config.provider_enum, &model)
    } else {
        (effective_grounding, degraded_reason)
    };

    let metadata = build_project_answer_metadata(
        requested_grounding,
        effective_grounding,
        degraded_reason,
        &answer,
        context.info,
    );

    let assistant_msg = ProjectChatMessagesRepository::add_message(
        pool,
        &project_id,
        &thread_id,
        "assistant",
        answer.text.trim(),
        metadata.as_deref(),
    )
    .await
    .map_err(|e| format!("Failed to save assistant message: {}", e))?;

    Ok(assistant_msg.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::project_chat_context::ProjectChatContextInfo;
    use crate::database::test_support::migrated_pool;
    use crate::summary::llm_client::LlmAnswer;

    #[tokio::test]
    async fn a_thread_from_another_project_is_rejected() {
        let pool = migrated_pool().await;
        let a = ProjectsRepository::create(&pool, "A", None, None).await.unwrap();
        let b = ProjectsRepository::create(&pool, "B", None, None).await.unwrap();
        let thread = ProjectChatThreadsRepository::create_thread(&pool, &a.id, "Chat 1")
            .await
            .unwrap();

        let err = require_project_thread(&pool, &b.id, &thread.id)
            .await
            .unwrap_err();
        assert!(err.contains("does not belong to project"));

        assert!(require_project_thread(&pool, &a.id, &thread.id).await.is_ok());
    }

    #[tokio::test]
    async fn a_missing_thread_is_reported_not_created() {
        let pool = migrated_pool().await;
        let p = ProjectsRepository::create(&pool, "A", None, None).await.unwrap();
        let err = require_project_thread(&pool, &p.id, "pthread-nope")
            .await
            .unwrap_err();
        assert!(err.contains("not found"));
    }

    /// Project answers always record their coverage, unlike meeting answers,
    /// which store nothing for a plain transcript-only turn.
    #[test]
    fn project_metadata_is_recorded_even_in_strict_mode() {
        let info = ProjectChatContextInfo {
            meetings_total: 3,
            meetings_with_summary: 2,
            meetings_with_transcript: 1,
            truncated: true,
            has_project_notes: false,
        };
        let json = build_project_answer_metadata(
            ChatGrounding::TranscriptOnly,
            ChatGrounding::TranscriptOnly,
            None,
            &LlmAnswer::default(),
            info,
        )
        .expect("project answers always carry coverage metadata");

        let parsed: ChatAnswerMetadata = serde_json::from_str(&json).unwrap();
        let ctx = parsed.project_context.expect("coverage is present");
        assert_eq!(ctx.meetings_total, 3);
        assert_eq!(ctx.meetings_with_transcript, 1);
        assert!(ctx.truncated);
    }
}
