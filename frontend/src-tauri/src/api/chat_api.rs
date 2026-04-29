use log::{error as log_error, info as log_info};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

use crate::database::repositories::{
    chat::ChatMessagesRepository, meeting::MeetingsRepository, setting::SettingsRepository,
};
use crate::state::AppState;
use crate::summary::llm_client::{generate_summary, LLMProvider};

const MAX_TRANSCRIPT_CHARS: usize = 30_000;
const MAX_HISTORY_MESSAGES: usize = 20;

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub meeting_id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

impl From<crate::database::models::ChatMessageModel> for ChatMessage {
    fn from(m: crate::database::models::ChatMessageModel) -> Self {
        Self {
            id: m.id,
            meeting_id: m.meeting_id,
            role: m.role,
            content: m.content,
            created_at: m.created_at.to_rfc3339(),
        }
    }
}

#[tauri::command]
pub async fn api_send_chat_message<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    message: String,
    provider: String,
    model: String,
) -> Result<ChatMessage, String> {
    let trimmed_message = message.trim();
    if meeting_id.trim().is_empty() {
        return Err("meeting_id is required".to_string());
    }
    if trimmed_message.is_empty() {
        return Err("message cannot be empty".to_string());
    }
    if provider.trim().is_empty() || model.trim().is_empty() {
        return Err("provider and model are required".to_string());
    }

    log_info!(
        "api_send_chat_message: meeting={} provider={} model={} ({} chars)",
        meeting_id,
        provider,
        model,
        trimmed_message.len()
    );

    let pool = state.db_manager.pool();

    // Verify meeting exists and load it (transcripts are loaded inline).
    let meeting = MeetingsRepository::get_meeting(pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to load meeting: {}", e))?
        .ok_or_else(|| format!("Meeting {} not found", meeting_id))?;

    // Load chat history BEFORE persisting the new user message so the LLM sees
    // the prior conversation followed by the current question.
    let history_raw = ChatMessagesRepository::list_for_meeting(pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to load chat history: {}", e))?;
    let history: Vec<ChatMessage> = history_raw.into_iter().map(Into::into).collect();

    // Persist user message immediately so it's not lost if the LLM call fails.
    let user_msg = ChatMessagesRepository::add_message(pool, &meeting_id, "user", trimmed_message)
        .await
        .map_err(|e| format!("Failed to save user message: {}", e))?;

    // Build prompts.
    let transcript_text = build_transcript_text(&meeting);
    let system_prompt = build_system_prompt(&meeting.title, &transcript_text);
    let user_prompt = build_user_prompt(&history, trimmed_message);

    // Resolve provider + auxiliary config.
    let provider_enum = LLMProvider::from_str(&provider)?;

    let api_key: String = match &provider_enum {
        LLMProvider::Ollama | LLMProvider::BuiltInAI | LLMProvider::CustomOpenAI => String::new(),
        LLMProvider::OpenAI | LLMProvider::Claude | LLMProvider::Groq | LLMProvider::OpenRouter => {
            match SettingsRepository::get_api_key(pool, &provider).await {
                Ok(Some(key)) if !key.is_empty() => key,
                _ => {
                    return Err(format!(
                        "API key not configured for {}. Add one in Settings.",
                        provider
                    ))
                }
            }
        }
    };

    let ollama_endpoint = if matches!(provider_enum, LLMProvider::Ollama) {
        SettingsRepository::get_model_config(pool)
            .await
            .ok()
            .flatten()
            .and_then(|c| c.ollama_endpoint)
    } else {
        None
    };

    let (
        custom_openai_endpoint,
        custom_openai_api_key,
        custom_openai_max_tokens,
        custom_openai_temperature,
        custom_openai_top_p,
    ) = if matches!(provider_enum, LLMProvider::CustomOpenAI) {
        match SettingsRepository::get_custom_openai_config(pool).await {
            Ok(Some(cfg)) => (
                Some(cfg.endpoint),
                cfg.api_key,
                cfg.max_tokens.map(|t| t as u32),
                cfg.temperature,
                cfg.top_p,
            ),
            _ => return Err("Custom OpenAI provider selected but no configuration found".to_string()),
        }
    } else {
        (None, None, None, None, None)
    };

    let final_api_key = if matches!(provider_enum, LLMProvider::CustomOpenAI) {
        custom_openai_api_key.unwrap_or_default()
    } else {
        api_key
    };

    let app_data_dir = if matches!(provider_enum, LLMProvider::BuiltInAI) {
        Some(
            app.path()
                .app_data_dir()
                .map_err(|e| format!("Failed to resolve app data dir: {}", e))?,
        )
    } else {
        None
    };

    let client = reqwest::Client::new();
    let answer_result = generate_summary(
        &client,
        &provider_enum,
        &model,
        &final_api_key,
        &system_prompt,
        &user_prompt,
        ollama_endpoint.as_deref(),
        custom_openai_endpoint.as_deref(),
        custom_openai_max_tokens,
        custom_openai_temperature,
        custom_openai_top_p,
        app_data_dir.as_ref(),
        None,
    )
    .await;

    let answer = match answer_result {
        Ok(text) => text.trim().to_string(),
        Err(e) => {
            log_error!("Chat LLM call failed for {}: {}", meeting_id, e);
            // Roll back the user message so the conversation isn't left dangling
            // with a question that has no response.
            let _ = ChatMessagesRepository::delete_message(pool, &user_msg.id).await;
            return Err(format!("Chat failed: {}", e));
        }
    };

    let assistant_msg =
        ChatMessagesRepository::add_message(pool, &meeting_id, "assistant", &answer)
            .await
            .map_err(|e| format!("Failed to save assistant message: {}", e))?;

    Ok(assistant_msg.into())
}

#[tauri::command]
pub async fn api_get_chat_history<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<ChatMessage>, String> {
    log_info!("api_get_chat_history: meeting={}", meeting_id);
    let pool = state.db_manager.pool();
    let rows = ChatMessagesRepository::list_for_meeting(pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to load chat history: {}", e))?;
    Ok(rows.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub async fn api_clear_chat_history<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<(), String> {
    log_info!("api_clear_chat_history: meeting={}", meeting_id);
    let pool = state.db_manager.pool();
    ChatMessagesRepository::clear_for_meeting(pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to clear chat history: {}", e))?;
    Ok(())
}

fn build_transcript_text(meeting: &crate::api::api::MeetingDetails) -> String {
    let mut joined = meeting
        .transcripts
        .iter()
        .map(|t| t.text.as_str())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");

    if joined.chars().count() > MAX_TRANSCRIPT_CHARS {
        let truncated: String = joined.chars().take(MAX_TRANSCRIPT_CHARS).collect();
        joined = format!("{}\n\n[transcript truncated for length]", truncated);
    }
    joined
}

fn build_system_prompt(meeting_title: &str, transcript_text: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are a helpful assistant answering questions about a recorded meeting.\n\
         Ground every answer strictly in the meeting transcript below. \
         Quote only verbatim text that actually appears in the transcript. \
         If the answer is not in the transcript, say you cannot find it rather than guessing. \
         Keep answers concise and reference specific moments or speakers when relevant.\n\n",
    );
    prompt.push_str(&format!("Meeting title: {}\n\n", meeting_title));
    prompt.push_str("--- TRANSCRIPT ---\n");
    if transcript_text.is_empty() {
        prompt.push_str("(no transcript available)\n");
    } else {
        prompt.push_str(transcript_text);
        prompt.push('\n');
    }
    prompt.push_str("--- END TRANSCRIPT ---\n");
    prompt
}

fn build_user_prompt(history: &[ChatMessage], current_message: &str) -> String {
    let recent: &[ChatMessage] = if history.len() > MAX_HISTORY_MESSAGES {
        &history[history.len() - MAX_HISTORY_MESSAGES..]
    } else {
        history
    };
    let mut out = String::new();
    out.push_str("Conversation so far:\n");
    if recent.is_empty() {
        out.push_str("(no prior messages)\n");
    } else {
        for msg in recent {
            let role = if msg.role == "user" { "User" } else { "Assistant" };
            out.push_str(&format!("{}: {}\n", role, msg.content));
        }
    }
    out.push_str(&format!("User: {}\nAssistant:", current_message));
    out
}
