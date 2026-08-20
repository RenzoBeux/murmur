//! Pieces shared by the meeting chat, the live Ask-AI chat, and the project
//! chat: grounding modes, provider resolution, budgeting, and prompt plumbing.
//!
//! Split out of `chat_api` when the project chat became a third consumer.
//! `build_system_prompt` deliberately did NOT come along: its
//! `(has_attachments, grounding)` match is meeting-specific and carries its own
//! tests, so the project chat writes its own prompt and shares the *constants*
//! rather than the composition.

use log::{error as log_error, info as log_info};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

use crate::database::repositories::setting::SettingsRepository;
use crate::summary::llm_client::{LLMProvider, LlmAnswer};
use crate::summary::web_search::{self, WebSource};

/// The speaker-attribution paragraph every chat system prompt carries.
///
/// A constant rather than duplicated prose because both prompts must teach the
/// same thing: the label before the colon is the only reliable speaker, and a
/// name occurring *inside* an utterance is someone being talked about.
pub(crate) const SPEAKER_LABEL_RULES: &str =
    "Each transcript line that has a known speaker is prefixed `Speaker: text` — the \
     label before the colon is the ONLY reliable indicator of who is speaking. \
     \"You\" is the local microphone, \"Others\" is everyone else on the call, and \
     other labels (e.g. speaker_1) come from speaker diarization. \
     A name mentioned inside the spoken text is someone being talked to or about — \
     NOT necessarily the speaker; never attribute a statement to a person merely \
     because their name was mentioned. If you cannot tell who said something from \
     the speaker labels, say so instead of guessing. \
     Keep answers concise and reference specific speakers or moments when relevant.\n\n";

/// Transcript budget when the model's context size is unknown (LM Studio, or a
/// failed Ollama metadata fetch). ~8k tokens — safe for most local models.
pub(crate) const DEFAULT_MAX_TRANSCRIPT_CHARS: usize = 30_000;
pub(crate) const MAX_HISTORY_MESSAGES: usize = 20;

/// Rough chars-per-token used to convert a context size into a char budget.
pub(crate) const CHARS_PER_TOKEN: usize = 4;
/// Tokens reserved out of the context for the system-prompt boilerplate, chat
/// history, attachments block, and the model's answer.
pub(crate) const RESERVED_TOKENS: usize = 2_000;

/// How many transcript characters this provider/model can take. Mirrors the
/// summary path's sizing: cloud providers get everything, Ollama sizes to the
/// model's real context (the same metadata cache the summarizer uses), the
/// built-in sidecar sizes to its registry entry, and LM Studio (which doesn't
/// advertise context size) keeps the conservative default.
pub(crate) async fn transcript_char_budget(
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

/// How far past the meeting the assistant may reach when answering.
///
/// The transcript is the primary source under every mode; they differ only in
/// what happens when the answer is not in it. Stored per chat thread, so a
/// strict recap conversation and a research conversation can coexist in one
/// meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatGrounding {
    /// Transcript and attachments only — the behavior before grounding modes
    /// existed, and still the default.
    #[default]
    TranscriptOnly,
    /// May also answer from the model's own knowledge. Makes no extra network
    /// calls: the request goes to the same provider it always did.
    GeneralKnowledge,
    /// May also run the provider's own server-side web search.
    WebSearch,
}

impl ChatGrounding {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TranscriptOnly => "transcript_only",
            Self::GeneralKnowledge => "general_knowledge",
            Self::WebSearch => "web_search",
        }
    }

    /// Parse a stored or frontend-supplied mode. An unrecognized value falls
    /// back to the strictest mode rather than failing the chat turn — a bad
    /// value must never widen what the assistant is allowed to do.
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "general_knowledge" => Self::GeneralKnowledge,
            "web_search" => Self::WebSearch,
            "transcript_only" => Self::TranscriptOnly,
            other => {
                if !other.is_empty() {
                    log_error!("Unknown chat grounding mode {:?}; using transcript_only", other);
                }
                Self::TranscriptOnly
            }
        }
    }
}

/// What actually happened on one answer, recorded so the UI can label it and so
/// a degradation is visible instead of silent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingOutcome {
    /// The mode the thread asked for.
    pub requested: ChatGrounding,
    /// The mode that ran. Lower than `requested` when the provider can't search.
    pub effective: ChatGrounding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded_reason: Option<String>,
}

/// Persisted as JSON in `chat_messages.metadata` on assistant messages.
///
/// Only written when there is something to say — a plain transcript-only answer
/// stores nothing, exactly as before this feature existed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatAnswerMetadata {
    pub grounding: GroundingOutcome,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<WebSource>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub search_count: u32,
    /// Which of the project's meetings were actually in front of the model.
    ///
    /// None on every meeting-chat message and on every row written before the
    /// project chat existed, so those rows serialize byte-identically to before.
    /// Recorded because a project answer's coverage is not obvious from the text
    /// — six months later, "why didn't it mention the March call?" needs an
    /// answer, and this is it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_context: Option<crate::api::project_chat_context::ProjectChatContextInfo>,
}

pub(crate) fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Whether this provider/model can search, reported to the picker so the web
/// option can be disabled with a reason instead of silently doing nothing.
#[derive(Debug, Serialize)]
pub struct WebSearchSupportInfo {
    pub supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Provider configuration resolved from settings, ready to hand to
/// `generate_summary`. Shared by the saved-meeting and live chat paths.
pub(crate) struct ResolvedLlmConfig {
    pub(crate) provider_enum: LLMProvider,
    /// Final key: the provider's API key, or the custom-OpenAI key when that
    /// provider is selected, or empty for keyless providers.
    pub(crate) api_key: String,
    pub(crate) ollama_endpoint: Option<String>,
    pub(crate) lmstudio_endpoint: Option<String>,
    pub(crate) custom_openai_endpoint: Option<String>,
    pub(crate) custom_openai_max_tokens: Option<u32>,
    pub(crate) custom_openai_temperature: Option<f32>,
    pub(crate) custom_openai_top_p: Option<f32>,
    pub(crate) app_data_dir: Option<std::path::PathBuf>,
}

pub(crate) async fn resolve_llm_config<R: Runtime>(
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

/// Decide what grounding actually runs.
///
/// A thread set to web search on a provider that can't search falls back to
/// general knowledge rather than pretending — the reason travels with the answer
/// so the UI can explain it instead of the user wondering why no sources showed
/// up. Only web search can degrade; the other two modes need nothing from the
/// provider.
pub(crate) fn resolve_grounding(
    requested: ChatGrounding,
    provider: &LLMProvider,
    model: &str,
) -> (ChatGrounding, Option<String>) {
    if requested != ChatGrounding::WebSearch {
        return (requested, None);
    }
    match web_search::web_search_support(provider, model) {
        web_search::WebSearchSupport::Native => (ChatGrounding::WebSearch, None),
        web_search::WebSearchSupport::Unsupported(reason) => {
            log_info!("Web search unavailable for {:?}/{}: {}", provider, model, reason);
            (ChatGrounding::GeneralKnowledge, Some(reason.to_string()))
        }
    }
}

/// Serialize what happened, for `chat_messages.metadata`.
///
/// Returns None for a plain transcript-only answer: those store nothing, so
/// existing conversations keep exactly the rows they had before grounding modes.
pub(crate) fn build_answer_metadata(
    requested: ChatGrounding,
    effective: ChatGrounding,
    degraded_reason: Option<String>,
    answer: &LlmAnswer,
) -> Option<String> {
    if requested == ChatGrounding::TranscriptOnly {
        return None;
    }
    build_metadata_json(requested, effective, degraded_reason, answer, None)
}

/// As [`build_answer_metadata`], but always writes a row because the project
/// chat always has something to record: which meetings it could see.
pub(crate) fn build_project_answer_metadata(
    requested: ChatGrounding,
    effective: ChatGrounding,
    degraded_reason: Option<String>,
    answer: &LlmAnswer,
    project_context: crate::api::project_chat_context::ProjectChatContextInfo,
) -> Option<String> {
    build_metadata_json(
        requested,
        effective,
        degraded_reason,
        answer,
        Some(project_context),
    )
}

fn build_metadata_json(
    requested: ChatGrounding,
    effective: ChatGrounding,
    degraded_reason: Option<String>,
    answer: &LlmAnswer,
    project_context: Option<crate::api::project_chat_context::ProjectChatContextInfo>,
) -> Option<String> {
    let metadata = ChatAnswerMetadata {
        grounding: GroundingOutcome {
            requested,
            effective,
            degraded_reason,
        },
        sources: answer.sources.clone(),
        search_count: answer.search_count,
        project_context,
    };
    serde_json::to_string(&metadata)
        .map_err(|e| log_error!("Failed to serialize chat metadata: {}", e))
        .ok()
}

/// Map a stored speaker tag to the same display name the UI renders, so the
/// LLM sees consistent labels. Mirrors `speakerDisplayName` in the frontend
/// `lib/speakerLabel.ts`.
pub(crate) fn speaker_display_name(tag: &str) -> &str {
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
pub(crate) fn build_transcript_text<'a, I>(lines: I, max_chars: usize) -> String
where
    I: IntoIterator<Item = (Option<&'a str>, &'a str)>,
{
    let joined = lines
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

    // Keeps the meeting's opening AND its conclusion/action-items instead of
    // only the head — the old head-only cut hid the end of every long meeting,
    // so "what did we decide at the end?" always failed.
    let joined = crate::summary::text_budget::elide_middle(&joined, max_chars);
    joined
}

pub(crate) fn build_user_prompt(history: &[(&str, &str)], current_message: &str) -> String {
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
