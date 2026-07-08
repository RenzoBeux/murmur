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

    // Attendee roster (canonical name spellings), same source the summary uses.
    let attendees = match MeetingsRepository::get_meeting_attendees(pool, &meeting_id).await {
        Ok(attendees) => attendees,
        Err(e) => {
            log_error!(
                "Failed to load attendees for chat (meeting={}): {}. Continuing without roster.",
                meeting_id,
                e
            );
            None
        }
    };

    // Build prompts.
    let transcript_text = build_transcript_text(&meeting);
    let system_prompt = build_system_prompt(&meeting.title, &transcript_text, attendees.as_deref());
    let user_prompt = build_user_prompt(&history, trimmed_message);

    // Resolve provider + auxiliary config.
    let provider_enum = LLMProvider::from_str(&provider)?;

    let api_key: String = match &provider_enum {
        LLMProvider::Ollama
        | LLMProvider::BuiltInAI
        | LLMProvider::CustomOpenAI
        | LLMProvider::LMStudio => String::new(),
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

    let lmstudio_endpoint = if matches!(provider_enum, LLMProvider::LMStudio) {
        SettingsRepository::get_model_config(pool)
            .await
            .ok()
            .flatten()
            .and_then(|c| c.lm_studio_endpoint)
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
        lmstudio_endpoint.as_deref(),
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

/// Map a stored speaker tag to the same display name the UI renders, so the
/// LLM sees consistent labels. Mirrors `speakerDisplayName` in the frontend
/// `lib/speakerLabel.ts`.
fn speaker_display_name(tag: &str) -> &str {
    match tag {
        "mic" => "You",
        "system" => "Others",
        other => other,
    }
}

fn build_transcript_text(meeting: &crate::api::api::MeetingDetails) -> String {
    let mut joined = meeting
        .transcripts
        .iter()
        .filter(|t| !t.text.is_empty())
        .map(|t| {
            // Prefix each line with the speaker (when set) so the LLM can
            // attribute statements. Falls back to plain text for old
            // transcripts that pre-date diarization.
            match t.speaker.as_deref().filter(|s| !s.is_empty()) {
                Some(tag) => format!("{}: {}", speaker_display_name(tag), t.text),
                None => t.text.clone(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    if joined.chars().count() > MAX_TRANSCRIPT_CHARS {
        let truncated: String = joined.chars().take(MAX_TRANSCRIPT_CHARS).collect();
        joined = format!("{}\n\n[transcript truncated for length]", truncated);
    }
    joined
}

fn build_system_prompt(
    meeting_title: &str,
    transcript_text: &str,
    attendees: Option<&str>,
) -> String {
    let mut prompt = String::new();
    prompt.push_str(
        "You are a helpful assistant answering questions about a recorded meeting.\n\
         Ground every answer strictly in the meeting transcript below. \
         Quote only verbatim text that actually appears in the transcript. \
         If the answer is not in the transcript, say you cannot find it rather than guessing. \
         Each transcript line that has a known speaker is prefixed `Speaker: text` — the \
         label before the colon is the ONLY reliable indicator of who is speaking. \
         \"You\" is the local microphone, \"Others\" is everyone else on the call, and \
         other labels (e.g. speaker_1) come from speaker diarization. \
         A name mentioned inside the spoken text is someone being talked to or about — \
         NOT necessarily the speaker; never attribute a statement to a person merely \
         because their name was mentioned. If you cannot tell who said something from \
         the speaker labels, say so instead of guessing. \
         Keep answers concise and reference specific speakers or moments when relevant.\n\n",
    );
    if let Some(roster) = attendees.map(str::trim).filter(|a| !a.is_empty()) {
        prompt.push_str(&format!(
            "Attendees (canonical names, provided by the user):\n{roster}\n\
             The transcript comes from automatic speech recognition and may misspell \
             names. When a name in the transcript closely resembles an attendee name, \
             use the attendee's canonical spelling in your answers. Do not invent people \
             who are neither in this list nor in the transcript.\n\n"
        ));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_includes_attendee_roster_when_provided() {
        let prompt = build_system_prompt("Standup", "You: hello", Some("Renzo, Lean, Sofía"));

        assert!(prompt.contains("Renzo, Lean, Sofía"));
        assert!(prompt.contains("canonical spelling"));
    }

    #[test]
    fn system_prompt_omits_roster_block_when_absent_or_blank() {
        for attendees in [None, Some(""), Some("   \n")] {
            let prompt = build_system_prompt("Standup", "You: hello", attendees);
            assert!(!prompt.contains("Attendees (canonical names"));
        }
    }

    #[test]
    fn system_prompt_always_carries_attribution_rules() {
        let prompt = build_system_prompt("Standup", "You: hello", None);

        assert!(prompt.contains("ONLY reliable indicator"));
        assert!(prompt.contains("NOT necessarily the speaker"));
    }
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
