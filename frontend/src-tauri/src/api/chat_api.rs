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
use crate::summary::llm_client::{generate_summary, LLMProvider};

/// Transcript budget when the model's context size is unknown (LM Studio, or a
/// failed Ollama metadata fetch). ~8k tokens — safe for most local models.
const DEFAULT_MAX_TRANSCRIPT_CHARS: usize = 30_000;
const MAX_HISTORY_MESSAGES: usize = 20;

/// Rough chars-per-token used to convert a context size into a char budget.
const CHARS_PER_TOKEN: usize = 4;
/// Tokens reserved out of the context for the system-prompt boilerplate, chat
/// history, attachments block, and the model's answer.
const RESERVED_TOKENS: usize = 2_000;

/// How many transcript characters this provider/model can take. Mirrors the
/// summary path's sizing: cloud providers get everything, Ollama sizes to the
/// model's real context (the same metadata cache the summarizer uses), the
/// built-in sidecar sizes to its registry entry, and LM Studio (which doesn't
/// advertise context size) keeps the conservative default.
async fn transcript_char_budget(
    provider: &LLMProvider,
    model: &str,
    ollama_endpoint: Option<&str>,
) -> usize {
    match provider {
        LLMProvider::OpenAI
        | LLMProvider::Claude
        | LLMProvider::Groq
        | LLMProvider::OpenRouter
        | LLMProvider::CustomOpenAI
        | LLMProvider::ChatGptSubscription => usize::MAX,
        LLMProvider::Ollama => {
            match crate::ollama::metadata::METADATA_CACHE
                .get_or_fetch(model, ollama_endpoint)
                .await
            {
                Ok(meta) => {
                    meta.context_size.saturating_sub(RESERVED_TOKENS).max(1_000) * CHARS_PER_TOKEN
                }
                Err(e) => {
                    log_info!(
                        "No context metadata for {} ({}); using default transcript budget",
                        model,
                        e
                    );
                    DEFAULT_MAX_TRANSCRIPT_CHARS
                }
            }
        }
        LLMProvider::BuiltInAI => crate::summary::summary_engine::models::get_model_by_name(model)
            .map(|m| {
                (m.context_size as usize)
                    .saturating_sub(RESERVED_TOKENS)
                    .max(1_000)
                    * CHARS_PER_TOKEN
            })
            .unwrap_or(DEFAULT_MAX_TRANSCRIPT_CHARS),
        LLMProvider::LMStudio => DEFAULT_MAX_TRANSCRIPT_CHARS,
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub meeting_id: String,
    pub thread_id: Option<String>,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

impl From<crate::database::models::ChatMessageModel> for ChatMessage {
    fn from(m: crate::database::models::ChatMessageModel) -> Self {
        Self {
            id: m.id,
            meeting_id: m.meeting_id,
            thread_id: m.thread_id,
            role: m.role,
            content: m.content,
            created_at: m.created_at.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatThread {
    pub id: String,
    pub meeting_id: String,
    pub title: String,
    pub origin: String,
    pub created_at: String,
}

impl From<crate::database::models::ChatThreadModel> for ChatThread {
    fn from(t: crate::database::models::ChatThreadModel) -> Self {
        Self {
            id: t.id,
            meeting_id: t.meeting_id,
            title: t.title,
            origin: t.origin,
            created_at: t.created_at.to_rfc3339(),
        }
    }
}

/// Provider configuration resolved from settings, ready to hand to
/// `generate_summary`. Shared by the saved-meeting and live chat paths.
struct ResolvedLlmConfig {
    provider_enum: LLMProvider,
    /// Final key: the provider's API key, or the custom-OpenAI key when that
    /// provider is selected, or empty for keyless providers.
    api_key: String,
    ollama_endpoint: Option<String>,
    lmstudio_endpoint: Option<String>,
    custom_openai_endpoint: Option<String>,
    custom_openai_max_tokens: Option<u32>,
    custom_openai_temperature: Option<f32>,
    custom_openai_top_p: Option<f32>,
    app_data_dir: Option<std::path::PathBuf>,
}

async fn resolve_llm_config<R: Runtime>(
    app: &AppHandle<R>,
    pool: &sqlx::SqlitePool,
    provider: &str,
) -> Result<ResolvedLlmConfig, String> {
    let provider_enum = LLMProvider::from_str(provider)?;

    let api_key: String = match &provider_enum {
        LLMProvider::Ollama
        | LLMProvider::BuiltInAI
        | LLMProvider::CustomOpenAI
        | LLMProvider::LMStudio
        | LLMProvider::ChatGptSubscription => String::new(),
        LLMProvider::OpenAI | LLMProvider::Claude | LLMProvider::Groq | LLMProvider::OpenRouter => {
            match SettingsRepository::get_api_key(pool, provider).await {
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

    // BuiltInAI needs it for the sidecar; ChatGptSubscription needs it to locate
    // the stored OAuth tokens.
    let app_data_dir = if matches!(
        provider_enum,
        LLMProvider::BuiltInAI | LLMProvider::ChatGptSubscription
    ) {
        Some(
            app.path()
                .app_data_dir()
                .map_err(|e| format!("Failed to resolve app data dir: {}", e))?,
        )
    } else {
        None
    };

    Ok(ResolvedLlmConfig {
        provider_enum,
        api_key: final_api_key,
        ollama_endpoint,
        lmstudio_endpoint,
        custom_openai_endpoint,
        custom_openai_max_tokens,
        custom_openai_temperature,
        custom_openai_top_p,
        app_data_dir,
    })
}

/// Verify a thread exists and belongs to the given meeting.
async fn require_thread(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
    thread_id: &str,
) -> Result<(), String> {
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
    Ok(())
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

    require_thread(pool, &meeting_id, &thread_id).await?;

    // Load chat history BEFORE persisting the new user message so the LLM sees
    // the prior conversation followed by the current question.
    let history_raw = ChatMessagesRepository::list_for_thread(pool, &thread_id)
        .await
        .map_err(|e| format!("Failed to load chat history: {}", e))?;

    // Persist user message immediately so it's not lost if the LLM call fails.
    let user_msg =
        ChatMessagesRepository::add_message(pool, &meeting_id, &thread_id, "user", trimmed_message)
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
    let system_prompt = build_system_prompt(
        &meeting.title,
        &transcript_text,
        attendees.as_deref(),
        attachment_notes.as_deref(),
    );
    let history: Vec<(&str, &str)> = history_raw
        .iter()
        .map(|m| (m.role.as_str(), m.content.as_str()))
        .collect();
    let user_prompt = build_user_prompt(&history, trimmed_message);

    let client = reqwest::Client::new();
    let mut answer_result = generate_summary(
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
        let retry = generate_summary(
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
        )
        .await;
        if retry.is_ok() {
            answer_result = retry;
        }
    }

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
        ChatMessagesRepository::add_message(pool, &meeting_id, &thread_id, "assistant", &answer)
            .await
            .map_err(|e| format!("Failed to save assistant message: {}", e))?;

    Ok(assistant_msg.into())
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

    // History BEFORE appending the new question, mirroring the saved-meeting path.
    let history_raw = live_chat::snapshot();
    let user_msg = live_chat::append("user", &trimmed_message);

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
    let system_prompt = build_system_prompt(&title, &transcript_text, None, None);
    let history: Vec<(&str, &str)> = history_raw
        .iter()
        .map(|m| (m.role.as_str(), m.content.as_str()))
        .collect();
    let user_prompt = build_user_prompt(&history, &trimmed_message);

    let client = reqwest::Client::new();
    let answer_result = generate_summary(
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
    )
    .await;

    let answer = match answer_result {
        Ok(text) => text.trim().to_string(),
        Err(e) => {
            log_error!("Live chat LLM call failed: {}", e);
            // Roll back the user message so the conversation isn't left dangling.
            live_chat::remove(&user_msg.id);
            return Err(format!("Chat failed: {}", e));
        }
    };

    // If the recording stopped while the LLM call was in flight, the stop path
    // already drained (or discarded) the store — still hand the answer back so
    // the user sees it; that final Q/A pair just isn't persisted.
    if crate::audio::recording_commands::is_recording().await {
        Ok(live_chat::append("assistant", &answer))
    } else {
        Ok(LiveChatMessage {
            id: uuid::Uuid::new_v4().to_string(),
            role: "assistant".to_string(),
            content: answer,
            created_at: chrono::Utc::now(),
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

/// Join `(speaker, text)` transcript lines into the prompt's transcript block,
/// budget-truncating to head + tail when over `max_chars`. Both the saved
/// meeting's `MeetingTranscript` rows and the live recording's segments map
/// into the same line shape.
fn build_transcript_text<'a, I>(lines: I, max_chars: usize) -> String
where
    I: IntoIterator<Item = (Option<&'a str>, &'a str)>,
{
    let mut joined = lines
        .into_iter()
        .filter(|(_, text)| !text.is_empty())
        .map(|(speaker, text)| {
            // Prefix each line with the speaker (when set) so the LLM can
            // attribute statements. Falls back to plain text for old
            // transcripts that pre-date diarization.
            match speaker.filter(|s| !s.is_empty()) {
                Some(tag) => format!("{}: {}", speaker_display_name(tag), text),
                None => text.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    if max_chars != usize::MAX && joined.chars().count() > max_chars {
        // Keep the meeting's opening AND its conclusion/action-items instead of only the
        // head (the old head-only cut hid the end of every long meeting, so
        // "what did we decide at the end?" always failed). Char-boundary safe.
        let chars: Vec<char> = joined.chars().collect();
        let total = chars.len();
        let head_len = (max_chars * 3) / 5; // 60% opening
        let tail_len = max_chars - head_len; // 40% conclusion
        let head: String = chars[..head_len].iter().collect();
        let tail: String = chars[total - tail_len..].iter().collect();
        let omitted = total - head_len - tail_len;
        joined = format!(
            "{head}\n\n[... {omitted} characters from the middle of the transcript omitted for length ...]\n\n{tail}"
        );
    }
    joined
}

fn build_system_prompt(
    meeting_title: &str,
    transcript_text: &str,
    attendees: Option<&str>,
    attachment_notes: Option<&str>,
) -> String {
    // The grounding instruction adapts to whether the meeting has attachments.
    // With attachments present, the model must treat them as a source alongside
    // the transcript — otherwise the "strictly in the transcript" wording makes a
    // vision model ignore an attached image (delivered to it in the same request)
    // that plainly answers the question, and it reports the answer as missing.
    let has_attachments = attachment_notes
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .is_some();

    let mut prompt = String::new();
    prompt.push_str("You are a helpful assistant answering questions about a recorded meeting.\n");
    if has_attachments {
        prompt.push_str(
            "Ground every answer in the meeting transcript below AND in the files the user \
             attached (listed after this paragraph — any images are provided directly in this \
             conversation, and text files are inlined). The attachments are authoritative, on \
             equal footing with the transcript: when the answer appears in an attached image or \
             file, use it and note which attachment it came from. Only say you cannot find \
             something when it is absent from BOTH the transcript and every attachment; never \
             guess. ",
        );
    } else {
        prompt.push_str(
            "Ground every answer strictly in the meeting transcript below. \
             Quote only verbatim text that actually appears in the transcript. \
             If the answer is not in the transcript, say you cannot find it rather than guessing. ",
        );
    }
    prompt.push_str(
        "Each transcript line that has a known speaker is prefixed `Speaker: text` — the \
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

fn build_user_prompt(history: &[(&str, &str)], current_message: &str) -> String {
    let recent: &[(&str, &str)] = if history.len() > MAX_HISTORY_MESSAGES {
        &history[history.len() - MAX_HISTORY_MESSAGES..]
    } else {
        history
    };
    let mut out = String::new();
    out.push_str("Conversation so far:\n");
    if recent.is_empty() {
        out.push_str("(no prior messages)\n");
    } else {
        for (role, content) in recent {
            let role = if *role == "user" { "User" } else { "Assistant" };
            out.push_str(&format!("{}: {}\n", role, content));
        }
    }
    out.push_str(&format!("User: {}\nAssistant:", current_message));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_includes_attendee_roster_when_provided() {
        let prompt =
            build_system_prompt("Standup", "You: hello", Some("Renzo, Lean, Sofía"), None);

        assert!(prompt.contains("Renzo, Lean, Sofía"));
        assert!(prompt.contains("canonical spelling"));
    }

    #[test]
    fn system_prompt_omits_roster_block_when_absent_or_blank() {
        for attendees in [None, Some(""), Some("   \n")] {
            let prompt = build_system_prompt("Standup", "You: hello", attendees, None);
            assert!(!prompt.contains("Attendees (canonical names"));
        }
    }

    #[test]
    fn system_prompt_always_carries_attribution_rules() {
        let prompt = build_system_prompt("Standup", "You: hello", None, None);

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
        );
        assert!(prompt.contains("whiteboard.png"));

        let without = build_system_prompt("Standup", "You: hello", None, Some("  "));
        assert!(!without.contains("Attached files"));
    }

    #[test]
    fn system_prompt_grounds_in_attachments_when_present() {
        let prompt = build_system_prompt(
            "Standup",
            "You: hello",
            None,
            Some("Attached files:\n- owners.png (image/png, shown as image)"),
        );
        // Attachments are authorized as a source, and the transcript-only wording
        // that made the model ignore the image is gone.
        assert!(prompt.contains("The attachments are authoritative"));
        assert!(prompt.contains("BOTH the transcript and every attachment"));
        assert!(!prompt.contains("strictly in the meeting transcript"));
    }

    #[test]
    fn system_prompt_stays_transcript_only_without_attachments() {
        for notes in [None, Some(""), Some("   ")] {
            let prompt = build_system_prompt("Standup", "You: hello", None, notes);
            assert!(prompt.contains("strictly in the meeting transcript"));
            assert!(!prompt.contains("The attachments are authoritative"));
        }
    }
}
