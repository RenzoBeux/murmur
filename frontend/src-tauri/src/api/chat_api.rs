use log::{error as log_error, info as log_info};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

use crate::audio::live_chat::{self, LiveChatMessage};
use crate::database::repositories::{
    chat::{ChatMessagesRepository, ChatThreadsRepository},
    meeting::MeetingsRepository,
    setting::SettingsRepository,
};
use crate::state::AppState;
use crate::summary::llm_client::{generate_answer, LLMProvider, LlmAnswer, LlmExtras};
use crate::summary::web_search::{self, WebSource};

// Shared with the live Ask-AI chat and the project chat; see api/chat_common.rs.
pub use crate::api::chat_common::{ChatAnswerMetadata, ChatGrounding, GroundingOutcome, WebSearchSupportInfo};
use crate::api::chat_common::{
    build_answer_metadata, build_transcript_text, build_user_prompt, resolve_grounding,
    resolve_llm_config, transcript_char_budget, SPEAKER_LABEL_RULES,
};


#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub meeting_id: String,
    pub thread_id: Option<String>,
    pub role: String,
    pub content: String,
    pub created_at: String,
    /// Parsed from the stored JSON. None on user messages, on older rows, and
    /// on any row whose JSON no longer parses (a schema change must not make an
    /// existing conversation unreadable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ChatAnswerMetadata>,
}

impl From<crate::database::models::ChatMessageModel> for ChatMessage {
    fn from(m: crate::database::models::ChatMessageModel) -> Self {
        let metadata = m.metadata.as_deref().and_then(|raw| {
            serde_json::from_str::<ChatAnswerMetadata>(raw)
                .map_err(|e| log_error!("Ignoring unreadable chat metadata on {}: {}", m.id, e))
                .ok()
        });
        Self {
            id: m.id,
            meeting_id: m.meeting_id,
            thread_id: m.thread_id,
            role: m.role,
            content: m.content,
            created_at: m.created_at.to_rfc3339(),
            metadata,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatThread {
    pub id: String,
    pub meeting_id: String,
    pub title: String,
    pub origin: String,
    pub grounding_mode: ChatGrounding,
    pub created_at: String,
}

impl From<crate::database::models::ChatThreadModel> for ChatThread {
    fn from(t: crate::database::models::ChatThreadModel) -> Self {
        Self {
            id: t.id,
            meeting_id: t.meeting_id,
            title: t.title,
            origin: t.origin,
            grounding_mode: ChatGrounding::parse(&t.grounding_mode),
            created_at: t.created_at.to_rfc3339(),
        }
    }
}


/// Verify a thread exists and belongs to the given meeting.
async fn require_thread(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    thread_id: &str,
) -> Result<crate::database::models::ChatThreadModel, String> {
    let thread = ChatThreadsRepository::get_thread(pool, thread_id)
        .await
        .map_err(|e| format!("Failed to load chat thread: {}", e))?
        .ok_or_else(|| format!("Chat thread {} not found", thread_id))?;
    if thread.meeting_id != meeting_id {
        return Err(format!(
            "Chat thread {} does not belong to meeting {}",
            thread_id, meeting_id
        ));
    }
    Ok(thread)
}


#[tauri::command]
pub async fn api_send_chat_message<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    thread_id: String,
    message: String,
    provider: String,
    model: String,
) -> Result<ChatMessage, String> {
    let trimmed_message = message.trim();
    if meeting_id.trim().is_empty() {
        return Err("meeting_id is required".to_string());
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

    log_info!(
        "api_send_chat_message: meeting={} thread={} provider={} model={} ({} chars)",
        meeting_id,
        thread_id,
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

    let thread = require_thread(pool, &meeting_id, &thread_id).await?;
    let requested_grounding = ChatGrounding::parse(&thread.grounding_mode);

    // Load chat history BEFORE persisting the new user message so the LLM sees
    // the prior conversation followed by the current question.
    let history_raw = ChatMessagesRepository::list_for_thread(pool, &thread_id)
        .await
        .map_err(|e| format!("Failed to load chat history: {}", e))?;

    // Persist user message immediately so it's not lost if the LLM call fails.
    let user_msg = ChatMessagesRepository::add_message(
        pool,
        &meeting_id,
        &thread_id,
        "user",
        trimmed_message,
        None,
    )
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

    // Attachment context: image payloads for vision-capable providers and a
    // text block describing every attachment. Never fails.
    let attachment_ctx =
        crate::summary::attachment_context::build_attachment_context(&app, pool, &meeting_id).await;

    let config = resolve_llm_config(&app, pool, &provider).await?;

    // The built-in sidecar cannot view images — drop them and say so in the
    // prompt, so the model doesn't hallucinate having seen the files.
    let mut attachment_notes = attachment_ctx.notes().map(str::to_string);
    let images: &[crate::summary::llm_client::ImageInput] = if matches!(
        config.provider_enum,
        LLMProvider::BuiltInAI
    ) && !attachment_ctx.images.is_empty()
    {
        let note = format!(
            "\n({} image attachment(s) were provided but this model cannot view images.)",
            attachment_ctx.images.len()
        );
        attachment_notes = Some(attachment_notes.unwrap_or_default() + &note);
        &[]
    } else {
        &attachment_ctx.images
    };

    // Build prompts, sizing the transcript to the model's real context (cloud
    // providers get the full transcript, mirroring the summary path).
    let char_budget = transcript_char_budget(
        &config.provider_enum,
        &model,
        config.ollama_endpoint.as_deref(),
    )
    .await;
    let transcript_text = build_transcript_text(
        meeting
            .transcripts
            .iter()
            .map(|t| (t.speaker.as_deref(), t.text.as_str())),
        char_budget,
    );
    // Build the prompt for the mode that will actually run, so a model that
    // cannot search is never told to search.
    let (effective_grounding, degraded_reason) =
        resolve_grounding(requested_grounding, &config.provider_enum, &model);
    let system_prompt = build_system_prompt(
        &meeting.title,
        &transcript_text,
        attendees.as_deref(),
        attachment_notes.as_deref(),
        effective_grounding,
    );
    let extras = LlmExtras {
        web_search: effective_grounding == ChatGrounding::WebSearch,
    };
    let history: Vec<(&str, &str)> = history_raw
        .iter()
        .map(|m| (m.role.as_str(), m.content.as_str()))
        .collect();
    let user_prompt = build_user_prompt(&history, trimmed_message);

    let client = reqwest::Client::new();
    let mut answer_result = generate_answer(
        &client,
        &config.provider_enum,
        &model,
        &config.api_key,
        &system_prompt,
        &user_prompt,
        images,
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

    // A model without vision support may reject the multimodal payload; retry
    // once text-only with an omission note before failing the chat turn.
    if answer_result.is_err() && !images.is_empty() {
        log_error!(
            "Chat with {} image(s) failed for {}; retrying text-only",
            images.len(),
            meeting_id
        );
        let retry_system_prompt = format!(
            "{}\n(Note: {} image attachment(s) could not be delivered to this model and were omitted.)",
            system_prompt,
            images.len()
        );
        let retry = generate_answer(
            &client,
            &config.provider_enum,
            &model,
            &config.api_key,
            &retry_system_prompt,
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
        if retry.is_ok() {
            answer_result = retry;
        }
    }

    let answer = match answer_result {
        Ok(answer) => answer,
        Err(e) => {
            log_error!("Chat LLM call failed for {}: {}", meeting_id, e);
            // Roll back the user message so the conversation isn't left dangling
            // with a question that has no response.
            let _ = ChatMessagesRepository::delete_message(pool, &user_msg.id).await;
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

    let metadata = build_answer_metadata(
        requested_grounding,
        effective_grounding,
        degraded_reason,
        &answer,
    );

    let assistant_msg = ChatMessagesRepository::add_message(
        pool,
        &meeting_id,
        &thread_id,
        "assistant",
        answer.text.trim(),
        metadata.as_deref(),
    )
    .await
    .map_err(|e| format!("Failed to save assistant message: {}", e))?;

    Ok(assistant_msg.into())
}

/// Change how far past the transcript a conversation may reach.
#[tauri::command]
pub async fn api_set_chat_thread_grounding<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    thread_id: String,
    grounding: String,
) -> Result<ChatThread, String> {
    let pool = state.db_manager.pool();
    let thread = require_thread(pool, &meeting_id, &thread_id).await?;

    // Round-trip through the enum so an unrecognized value can never be stored.
    let mode = ChatGrounding::parse(&grounding);
    let updated = ChatThreadsRepository::set_grounding_mode(pool, &thread_id, mode.as_str())
        .await
        .map_err(|e| format!("Failed to update chat thread grounding: {}", e))?;
    if updated == 0 {
        return Err(format!("Chat thread {} not found", thread_id));
    }
    log_info!(
        "api_set_chat_thread_grounding: thread={} mode={}",
        thread_id,
        mode.as_str()
    );

    Ok(ChatThread {
        grounding_mode: mode,
        ..ChatThread::from(thread)
    })
}

/// Whether the given provider/model can search the web.
///
/// The capability table lives in Rust so it has exactly one definition; the
/// picker calls this when the selected model changes rather than keeping its own
/// copy that could drift.
#[tauri::command]
pub async fn api_chat_web_search_support<R: Runtime>(
    _app: AppHandle<R>,
    provider: String,
    model: String,
) -> Result<WebSearchSupportInfo, String> {
    let provider_enum = LLMProvider::from_str(&provider)?;
    let support = web_search::web_search_support(&provider_enum, &model);
    Ok(WebSearchSupportInfo {
        supported: support.is_native(),
        reason: support.reason().map(str::to_string),
    })
}

#[tauri::command]
pub async fn api_get_chat_history<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    thread_id: String,
) -> Result<Vec<ChatMessage>, String> {
    log_info!(
        "api_get_chat_history: meeting={} thread={}",
        meeting_id,
        thread_id
    );
    let pool = state.db_manager.pool();
    let rows = ChatMessagesRepository::list_for_thread(pool, &thread_id)
        .await
        .map_err(|e| format!("Failed to load chat history: {}", e))?;
    Ok(rows.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub async fn api_clear_chat_history<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    thread_id: String,
) -> Result<(), String> {
    log_info!(
        "api_clear_chat_history: meeting={} thread={}",
        meeting_id,
        thread_id
    );
    let pool = state.db_manager.pool();
    require_thread(pool, &meeting_id, &thread_id).await?;
    ChatMessagesRepository::clear_for_thread(pool, &thread_id)
        .await
        .map_err(|e| format!("Failed to clear chat history: {}", e))?;
    Ok(())
}

#[tauri::command]
pub async fn api_list_chat_threads(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
) -> Result<Vec<ChatThread>, String> {
    let pool = state.db_manager.pool();
    let threads = ChatThreadsRepository::list_for_meeting(pool, &meeting_id)
        .await
        .map_err(|e| format!("Failed to list chat threads: {}", e))?;
    Ok(threads.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub async fn api_create_chat_thread(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    title: Option<String>,
) -> Result<ChatThread, String> {
    if meeting_id.trim().is_empty() {
        return Err("meeting_id is required".to_string());
    }
    let pool = state.db_manager.pool();
    let title = match title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(t) => t,
        None => {
            let existing = ChatThreadsRepository::list_for_meeting(pool, &meeting_id)
                .await
                .map_err(|e| format!("Failed to list chat threads: {}", e))?;
            format!("Chat {}", existing.len() + 1)
        }
    };
    // The meeting_id FK rejects unknown meetings, so no separate existence check.
    let thread = ChatThreadsRepository::create_thread(pool, &meeting_id, &title, "post")
        .await
        .map_err(|e| format!("Failed to create chat thread: {}", e))?;
    log_info!(
        "api_create_chat_thread: meeting={} thread={} title={}",
        meeting_id,
        thread.id,
        thread.title
    );
    Ok(thread.into())
}

#[tauri::command]
pub async fn api_delete_chat_thread(
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    thread_id: String,
) -> Result<(), String> {
    let pool = state.db_manager.pool();
    require_thread(pool, &meeting_id, &thread_id).await?;
    ChatThreadsRepository::delete_thread(pool, &thread_id)
        .await
        .map_err(|e| format!("Failed to delete chat thread: {}", e))?;
    log_info!(
        "api_delete_chat_thread: meeting={} thread={}",
        meeting_id,
        thread_id
    );
    Ok(())
}

/// Ask the AI about the meeting currently being recorded. The transcript is
/// snapshotted from the live recording manager at send time (so segments that
/// arrive while the LLM call is in flight simply show up in the next question),
/// and the conversation is held in the in-memory live chat store — the stop
/// path persists it as the meeting's "Live chat" thread.
///
/// No attendees, attachments, or images are available here: those are keyed to
/// a persisted meeting row, which does not exist until the recording stops.
#[tauri::command]
pub async fn api_send_live_chat_message<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    message: String,
    provider: String,
    model: String,
    grounding: Option<String>,
) -> Result<LiveChatMessage, String> {
    let trimmed_message = message.trim().to_string();
    if trimmed_message.is_empty() {
        return Err("message cannot be empty".to_string());
    }
    if provider.trim().is_empty() || model.trim().is_empty() {
        return Err("provider and model are required".to_string());
    }
    if !crate::audio::recording_commands::is_recording().await {
        return Err("No active recording".to_string());
    }

    log_info!(
        "api_send_live_chat_message: provider={} model={} ({} chars)",
        provider,
        model,
        trimmed_message.len()
    );

    // Snapshot the live transcript (echo-deduped, same conversion the save path
    // uses) and the meeting name.
    let segments = crate::audio::recording_commands::get_transcript_history().await?;
    let api_segments = crate::audio::recording_commands::recording_segments_to_api(&segments);
    let title = crate::audio::recording_commands::get_recording_meeting_name()
        .await?
        .unwrap_or_else(|| "Current meeting".to_string());

    // The live panel has no thread row yet, so the mode arrives with the request
    // and is remembered for the thread the stop path will create.
    let requested_grounding = grounding.as_deref().map(ChatGrounding::parse).unwrap_or_default();
    live_chat::set_grounding_mode(requested_grounding.as_str());

    // History BEFORE appending the new question, mirroring the saved-meeting path.
    let history_raw = live_chat::snapshot();
    let user_msg = live_chat::append("user", &trimmed_message, None);

    let pool = state.db_manager.pool();
    let config = match resolve_llm_config(&app, pool, &provider).await {
        Ok(config) => config,
        Err(e) => {
            live_chat::remove(&user_msg.id);
            return Err(e);
        }
    };

    let char_budget = transcript_char_budget(
        &config.provider_enum,
        &model,
        config.ollama_endpoint.as_deref(),
    )
    .await;
    let transcript_text = build_transcript_text(
        api_segments
            .iter()
            .map(|s| (s.speaker.as_deref(), s.text.as_str())),
        char_budget,
    );
    let (effective_grounding, degraded_reason) =
        resolve_grounding(requested_grounding, &config.provider_enum, &model);
    let system_prompt = build_system_prompt(
        &title,
        &transcript_text,
        None,
        None,
        effective_grounding,
    );
    let extras = LlmExtras {
        web_search: effective_grounding == ChatGrounding::WebSearch,
    };
    let history: Vec<(&str, &str)> = history_raw
        .iter()
        .map(|m| (m.role.as_str(), m.content.as_str()))
        .collect();
    let user_prompt = build_user_prompt(&history, &trimmed_message);

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
            log_error!("Live chat LLM call failed: {}", e);
            // Roll back the user message so the conversation isn't left dangling.
            live_chat::remove(&user_msg.id);
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

    let metadata = build_answer_metadata(
        requested_grounding,
        effective_grounding,
        degraded_reason,
        &answer,
    );
    let answer_text = answer.text.trim().to_string();

    // If the recording stopped while the LLM call was in flight, the stop path
    // already drained (or discarded) the store — still hand the answer back so
    // the user sees it; that final Q/A pair just isn't persisted.
    if crate::audio::recording_commands::is_recording().await {
        Ok(live_chat::append(
            "assistant",
            &answer_text,
            metadata.as_deref(),
        ))
    } else {
        Ok(LiveChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: "assistant".to_string(),
            content: answer_text,
            created_at: chrono::Utc::now(),
            metadata,
        })
    }
}

/// Snapshot of the live Ask-AI conversation (empty when idle). Lets the panel
/// rehydrate after a webview reload, same as `get_transcript_history`.
#[tauri::command]
pub async fn api_get_live_chat_history() -> Result<Vec<LiveChatMessage>, String> {
    Ok(live_chat::snapshot())
}

#[tauri::command]
pub async fn api_clear_live_chat_history() -> Result<(), String> {
    live_chat::clear();
    Ok(())
}


fn build_system_prompt(
    meeting_title: &str,
    transcript_text: &str,
    attendees: Option<&str>,
    attachment_notes: Option<&str>,
    grounding: ChatGrounding,
) -> String {
    // Two independent decisions, deliberately split so they don't multiply:
    //
    //   1. What counts as a meeting source — transcript alone, or transcript
    //      plus attachments. With attachments present the model must treat them
    //      as a source, otherwise "strictly in the transcript" makes a vision
    //      model ignore an attached image (delivered in the same request) that
    //      plainly answers the question, and report the answer as missing.
    //   2. What to do when those sources fall short — this is the grounding
    //      mode. The transcript stays the primary source under every mode.
    let has_attachments = attachment_notes
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .is_some();

    let mut prompt = String::new();
    prompt.push_str("You are a helpful assistant answering questions about a recorded meeting.\n");

    match (has_attachments, grounding) {
        (true, _) => prompt.push_str(
            "Ground every answer in the meeting transcript below AND in the files the user \
             attached (listed after this paragraph — any images are provided directly in this \
             conversation, and text files are inlined). The attachments are authoritative, on \
             equal footing with the transcript: when the answer appears in an attached image or \
             file, use it and note which attachment it came from. ",
        ),
        (false, ChatGrounding::TranscriptOnly) => prompt.push_str(
            "Ground every answer strictly in the meeting transcript below. \
             Quote only verbatim text that actually appears in the transcript. ",
        ),
        (false, _) => prompt.push_str(
            "The meeting transcript below is your primary source, and you should always \
             prefer it when it covers the question. \
             Quote only verbatim text that actually appears in the transcript. ",
        ),
    }

    match grounding {
        ChatGrounding::TranscriptOnly if has_attachments => prompt.push_str(
            "Only say you cannot find something when it is absent from BOTH the transcript \
             and every attachment; never guess. ",
        ),
        ChatGrounding::TranscriptOnly => prompt.push_str(
            "If the answer is not in the transcript, say you cannot find it rather than guessing. ",
        ),
        ChatGrounding::GeneralKnowledge => prompt.push_str(
            "When the answer is not in the meeting, say so plainly first (for example \
             \"That wasn't discussed in this meeting\") and then answer from your own general \
             knowledge, keeping the two clearly separated. Never present outside knowledge as \
             something that was said in the meeting, and never attribute it to a speaker. \
             If you are unsure of a general fact, say so rather than inventing it. ",
        ),
        ChatGrounding::WebSearch => prompt.push_str(
            "When the answer is not in the meeting, say so plainly first (for example \
             \"That wasn't discussed in this meeting\") and then answer from the web or your \
             own general knowledge, keeping the two clearly separated. Search the web when the \
             meeting does not cover the question, when the answer depends on current \
             information, or when the user asks what a term, product or company mentioned in \
             passing actually is. Do not search for questions the transcript already answers. \
             Never present outside knowledge as something that was said in the meeting, and \
             never attribute it to a speaker. ",
        ),
    }
    prompt.push_str(SPEAKER_LABEL_RULES);
    if let Some(roster) = attendees.map(str::trim).filter(|a| !a.is_empty()) {
        prompt.push_str(&format!(
            "Attendees (canonical names, provided by the user):\n{roster}\n\
             The transcript comes from automatic speech recognition and may misspell \
             names. When a name in the transcript closely resembles an attendee name, \
             use the attendee's canonical spelling in your answers. Do not invent people \
             who are neither in this list nor in the transcript.\n\n"
        ));
    }
    if let Some(notes) = attachment_notes.map(str::trim).filter(|n| !n.is_empty()) {
        prompt.push_str(notes);
        prompt.push_str("\n\n");
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
        let prompt =
            build_system_prompt(
            "Standup",
            "You: hello",
            Some("Renzo, Lean, Sofía"),
            None,
            ChatGrounding::TranscriptOnly,
        );

        assert!(prompt.contains("Renzo, Lean, Sofía"));
        assert!(prompt.contains("canonical spelling"));
    }

    #[test]
    fn system_prompt_omits_roster_block_when_absent_or_blank() {
        for attendees in [None, Some(""), Some("   \n")] {
            let prompt = build_system_prompt(
                "Standup",
                "You: hello",
                attendees,
                None,
                ChatGrounding::TranscriptOnly,
            );
            assert!(!prompt.contains("Attendees (canonical names"));
        }
    }

    #[test]
    fn system_prompt_always_carries_attribution_rules() {
        let prompt = build_system_prompt(
            "Standup",
            "You: hello",
            None,
            None,
            ChatGrounding::TranscriptOnly,
        );

        assert!(prompt.contains("ONLY reliable indicator"));
        assert!(prompt.contains("NOT necessarily the speaker"));
    }

    fn lines_with_text(text: &str) -> Vec<(Option<&str>, &str)> {
        vec![(None, text)]
    }

    #[test]
    fn transcript_untruncated_when_budget_is_unlimited() {
        let text_owned = "x".repeat(100_000);
        let text = build_transcript_text(lines_with_text(&text_owned), usize::MAX);
        assert_eq!(text.len(), 100_000);
        assert!(!text.contains("omitted for length"));
    }

    #[test]
    fn transcript_keeps_head_and_tail_when_over_budget() {
        let text_owned = format!("START{}END", "x".repeat(50_000));
        let text = build_transcript_text(lines_with_text(&text_owned), 10_000);
        assert!(text.starts_with("START"));
        assert!(text.ends_with("END"));
        assert!(text.contains("omitted for length"));
        // Head + tail + omission marker stays close to the budget.
        assert!(text.chars().count() < 11_000);
    }

    #[test]
    fn transcript_under_budget_passes_through() {
        let text = build_transcript_text(lines_with_text("short transcript"), 30_000);
        assert_eq!(text, "short transcript");
    }

    #[test]
    fn transcript_prefixes_known_speakers_and_maps_tags() {
        let lines = vec![
            (Some("mic"), "hello"),
            (Some("system"), "hi"),
            (Some("speaker_1"), "hey"),
            (None, "untagged"),
            (Some(""), "blank tag"),
        ];
        let text = build_transcript_text(lines, usize::MAX);
        assert_eq!(
            text,
            "You: hello\nOthers: hi\nspeaker_1: hey\nuntagged\nblank tag"
        );
    }

    #[test]
    fn user_prompt_flattens_history_and_caps_length() {
        let history: Vec<(String, String)> = (0..30)
            .map(|i| {
                let role = if i % 2 == 0 { "user" } else { "assistant" };
                (role.to_string(), format!("msg{}", i))
            })
            .collect();
        let history_refs: Vec<(&str, &str)> = history
            .iter()
            .map(|(r, c)| (r.as_str(), c.as_str()))
            .collect();
        let prompt = build_user_prompt(&history_refs, "current");
        // Only the last MAX_HISTORY_MESSAGES survive.
        assert!(!prompt.contains("msg9\n"));
        assert!(prompt.contains("msg10"));
        assert!(prompt.contains("msg29"));
        assert!(prompt.ends_with("User: current\nAssistant:"));
    }

    #[test]
    fn system_prompt_includes_attachment_notes_when_provided() {
        let prompt = build_system_prompt(
            "Standup",
            "You: hello",
            None,
            Some("Attached files:\n- whiteboard.png (image/png, shown as image)"),
            ChatGrounding::TranscriptOnly,
        );
        assert!(prompt.contains("whiteboard.png"));

        let without = build_system_prompt(
            "Standup",
            "You: hello",
            None,
            Some("  "),
            ChatGrounding::TranscriptOnly,
        );
        assert!(!without.contains("Attached files"));
    }

    #[test]
    fn system_prompt_grounds_in_attachments_when_present() {
        let prompt = build_system_prompt(
            "Standup",
            "You: hello",
            None,
            Some("Attached files:\n- owners.png (image/png, shown as image)"),
            ChatGrounding::TranscriptOnly,
        );
        // Attachments are authorized as a source, and the transcript-only wording
        // that made the model ignore the image is gone.
        assert!(prompt.contains("The attachments are authoritative"));
        assert!(prompt.contains("BOTH the transcript and every attachment"));
        assert!(!prompt.contains("strictly in the meeting transcript"));
    }

    /// The whole point of the feature: the strict "say you cannot find it"
    /// instruction must be gone, replaced by permission to answer from outside
    /// knowledge while still labelling it as such.
    #[test]
    fn general_knowledge_mode_allows_answering_beyond_the_transcript() {
        let prompt = build_system_prompt(
            "Standup",
            "You: we should migrate the ERP",
            None,
            None,
            ChatGrounding::GeneralKnowledge,
        );

        assert!(!prompt.contains("strictly in the meeting transcript"));
        assert!(!prompt.contains("say you cannot find it rather than guessing"));
        assert!(prompt.contains("primary source"));
        assert!(prompt.contains("your own general knowledge"));
        // Outside knowledge must never be laundered into a quote from the meeting.
        assert!(prompt.contains("never attribute it to a speaker"));
        // General knowledge is the offline mode — it must not ask for a search.
        assert!(!prompt.contains("Search the web"));
    }

    #[test]
    fn web_search_mode_asks_for_searches_only_when_the_meeting_falls_short() {
        let prompt = build_system_prompt(
            "Standup",
            "You: we should migrate the ERP",
            None,
            None,
            ChatGrounding::WebSearch,
        );

        assert!(prompt.contains("Search the web"));
        assert!(prompt.contains("Do not search for questions the transcript already answers"));
        assert!(!prompt.contains("strictly in the meeting transcript"));
    }

    /// Attachments stay authoritative under every mode — relaxing grounding must
    /// not quietly demote a file the user attached.
    #[test]
    fn attachments_stay_authoritative_in_every_mode() {
        for mode in [
            ChatGrounding::TranscriptOnly,
            ChatGrounding::GeneralKnowledge,
            ChatGrounding::WebSearch,
        ] {
            let prompt = build_system_prompt(
                "Standup",
                "You: hello",
                None,
                Some("Attached files:\n- owners.png (image/png, shown as image)"),
                mode,
            );
            assert!(
                prompt.contains("The attachments are authoritative"),
                "{:?} dropped the attachment grounding",
                mode
            );
        }
    }

    /// Only web search can degrade, and only to general knowledge — never to a
    /// mode the user did not ask for in the other direction.
    #[test]
    fn web_search_degrades_to_general_knowledge_on_providers_that_cannot_search() {
        let (effective, reason) =
            resolve_grounding(ChatGrounding::WebSearch, &LLMProvider::Ollama, "llama3.2");
        assert_eq!(effective, ChatGrounding::GeneralKnowledge);
        assert!(reason.is_some(), "a degradation must explain itself");

        let (effective, reason) =
            resolve_grounding(ChatGrounding::WebSearch, &LLMProvider::Claude, "claude-opus-5");
        assert_eq!(effective, ChatGrounding::WebSearch);
        assert!(reason.is_none());
    }

    #[test]
    fn non_web_modes_are_never_degraded() {
        for mode in [ChatGrounding::TranscriptOnly, ChatGrounding::GeneralKnowledge] {
            let (effective, reason) = resolve_grounding(mode, &LLMProvider::Ollama, "llama3.2");
            assert_eq!(effective, mode);
            assert!(reason.is_none());
        }
    }

    #[test]
    fn unknown_grounding_values_fall_back_to_the_strictest_mode() {
        assert_eq!(ChatGrounding::parse("nonsense"), ChatGrounding::TranscriptOnly);
        assert_eq!(ChatGrounding::parse(""), ChatGrounding::TranscriptOnly);
        assert_eq!(
            ChatGrounding::parse("general_knowledge"),
            ChatGrounding::GeneralKnowledge
        );
        assert_eq!(ChatGrounding::parse(" web_search "), ChatGrounding::WebSearch);
    }

    /// Grounding modes must serialize as the exact strings the column stores.
    #[test]
    fn grounding_serializes_to_its_database_value() {
        for mode in [
            ChatGrounding::TranscriptOnly,
            ChatGrounding::GeneralKnowledge,
            ChatGrounding::WebSearch,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", mode.as_str()));
            assert_eq!(ChatGrounding::parse(mode.as_str()), mode);
        }
    }

    /// Transcript-only answers keep writing no metadata, so existing
    /// conversations stay byte-identical to what they were before this feature.
    #[test]
    fn transcript_only_answers_store_no_metadata() {
        let answer = LlmAnswer {
            text: "hi".to_string(),
            ..Default::default()
        };
        assert!(build_answer_metadata(
            ChatGrounding::TranscriptOnly,
            ChatGrounding::TranscriptOnly,
            None,
            &answer
        )
        .is_none());
    }

    #[test]
    fn metadata_records_sources_and_the_degradation_reason() {
        let answer = LlmAnswer {
            text: "An ERP is...".to_string(),
            sources: vec![WebSource {
                url: "https://sap.com".to_string(),
                title: Some("What is ERP".to_string()),
                cited_text: None,
            }],
            search_count: 2,
        };

        let raw = build_answer_metadata(
            ChatGrounding::WebSearch,
            ChatGrounding::WebSearch,
            None,
            &answer,
        )
        .expect("web-search answers record metadata");
        let parsed: ChatAnswerMetadata = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.grounding.requested, ChatGrounding::WebSearch);
        assert_eq!(parsed.grounding.effective, ChatGrounding::WebSearch);
        assert!(parsed.grounding.degraded_reason.is_none());
        assert_eq!(parsed.sources.len(), 1);
        assert_eq!(parsed.search_count, 2);

        let degraded = build_answer_metadata(
            ChatGrounding::WebSearch,
            ChatGrounding::GeneralKnowledge,
            Some("Local models cannot search".to_string()),
            &LlmAnswer::default(),
        )
        .expect("a degraded answer records why");
        let parsed: ChatAnswerMetadata = serde_json::from_str(&degraded).unwrap();
        assert_eq!(parsed.grounding.requested, ChatGrounding::WebSearch);
        assert_eq!(parsed.grounding.effective, ChatGrounding::GeneralKnowledge);
        assert_eq!(
            parsed.grounding.degraded_reason.as_deref(),
            Some("Local models cannot search")
        );
        assert!(parsed.sources.is_empty());
    }

    #[test]
    fn system_prompt_stays_transcript_only_without_attachments() {
        for notes in [None, Some(""), Some("   ")] {
            let prompt = build_system_prompt(
                "Standup",
                "You: hello",
                None,
                notes,
                ChatGrounding::TranscriptOnly,
            );
            assert!(prompt.contains("strictly in the meeting transcript"));
            assert!(!prompt.contains("The attachments are authoritative"));
        }
    }
}
