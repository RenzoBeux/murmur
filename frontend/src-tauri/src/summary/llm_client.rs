use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::web_search::{self, WebSource};

const REQUEST_TIMEOUT_DURATION: Duration = Duration::from_secs(300);

/// An image attachment ready to send to a vision-capable model.
#[derive(Debug, Clone)]
pub struct ImageInput {
    /// e.g. "image/png" — already validated against the supported set.
    pub media_type: String,
    /// Base64-encoded file bytes (no data: prefix).
    pub base64_data: String,
}

/// Optional per-call knobs. Bundled into a struct because `generate_answer`
/// already carries a long positional argument list and most callers want none
/// of these.
#[derive(Debug, Clone, Copy, Default)]
pub struct LlmExtras {
    /// Ask the provider for its own server-side web search tool. Silently
    /// ignored by providers that have none — check `web_search_support` first if
    /// you need to know whether it will actually happen.
    pub web_search: bool,
}

/// A model's answer plus whatever it cited while producing it.
#[derive(Debug, Clone, Default)]
pub struct LlmAnswer {
    pub text: String,
    /// Empty unless the provider actually searched.
    pub sources: Vec<WebSource>,
    /// How many searches the provider ran, when it reports that.
    pub search_count: u32,
}

impl LlmAnswer {
    fn text_only(text: String) -> Self {
        Self {
            text,
            ..Default::default()
        }
    }
}

// Request-side message content. Text-only messages must keep serializing as a
// plain JSON string (untagged), so older OpenAI-compatible servers see exactly
// the wire format they saw before multimodal support existed.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum RequestContent {
    Text(String),
    OpenAiParts(Vec<OpenAiContentPart>),
    ClaudeParts(Vec<ClaudeContentPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpenAiContentPart {
    Text { text: String },
    ImageUrl { image_url: OpenAiImageUrl },
}

#[derive(Debug, Serialize)]
pub struct OpenAiImageUrl {
    /// data:{media_type};base64,{data}
    pub url: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClaudeContentPart {
    Text { text: String },
    Image { source: ClaudeImageSource },
}

#[derive(Debug, Serialize)]
pub struct ClaudeImageSource {
    #[serde(rename = "type")]
    pub source_type: String, // always "base64"
    pub media_type: String,
    pub data: String,
}

// Generic structure for OpenAI-compatible API chat messages
#[derive(Debug, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: RequestContent,
}

impl ChatMessage {
    pub fn text(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: RequestContent::Text(content.into()),
        }
    }
}

/// Build the user message for an OpenAI-compatible provider: plain text when
/// there are no images, otherwise a parts array with the text first.
fn openai_user_message(user_prompt: &str, images: &[ImageInput]) -> ChatMessage {
    if images.is_empty() {
        return ChatMessage::text("user", user_prompt);
    }
    let mut parts = vec![OpenAiContentPart::Text {
        text: user_prompt.to_string(),
    }];
    parts.extend(images.iter().map(|img| OpenAiContentPart::ImageUrl {
        image_url: OpenAiImageUrl {
            url: format!("data:{};base64,{}", img.media_type, img.base64_data),
        },
    }));
    ChatMessage {
        role: "user".to_string(),
        content: RequestContent::OpenAiParts(parts),
    }
}

/// Build the user message for Claude: content blocks with images first (per
/// Anthropic guidance) followed by the text.
fn claude_user_message(user_prompt: &str, images: &[ImageInput]) -> ChatMessage {
    if images.is_empty() {
        return ChatMessage::text("user", user_prompt);
    }
    let mut parts: Vec<ClaudeContentPart> = images
        .iter()
        .map(|img| ClaudeContentPart::Image {
            source: ClaudeImageSource {
                source_type: "base64".to_string(),
                media_type: img.media_type.clone(),
                data: img.base64_data.clone(),
            },
        })
        .collect();
    parts.push(ClaudeContentPart::Text {
        text: user_prompt.to_string(),
    });
    ChatMessage {
        role: "user".to_string(),
        content: RequestContent::ClaudeParts(parts),
    }
}

// Generic structure for OpenAI-compatible API chat requests
#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

// Generic structure for OpenAI-compatible API chat responses
#[derive(Deserialize, Debug)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,
}

#[derive(Deserialize, Debug)]
pub struct Choice {
    pub message: MessageContent,
}

#[derive(Deserialize, Debug)]
pub struct MessageContent {
    pub content: String,
    /// Web-search citations, when the provider ran a search. OpenRouter fills
    /// this in for `:online` models; plain chat/completions responses omit it.
    #[serde(default)]
    pub annotations: Vec<serde_json::Value>,
}

// Claude's response is read as raw JSON rather than a typed struct. Two reasons:
// with web search the `content` array is heterogeneous (`text`,
// `server_tool_use` and `web_search_tool_result` blocks interleave, and the
// answer is spread across several `text` blocks), and the `pause_turn`
// continuation has to echo the assistant turn back byte-identical because the
// `encrypted_content` in a search result must survive the round trip. See
// `generate_claude_native`.

// Native Ollama /api/chat request (NOT the OpenAI-compat shim). The shim ignores
// context sizing, so Ollama serves its small default (~4k) and silently truncates long
// prompts; the native endpoint lets us set options.num_ctx to the model's real context.
#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaChatMessage>,
    stream: bool,
    options: OllamaOptions,
}

// Ollama's native multimodal shape: content stays a plain string and images
// ride alongside as raw base64 (no data: prefix).
#[derive(Debug, Serialize)]
struct OllamaChatMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<String>,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    num_ctx: Option<u32>,
    // Ollama defaults num_predict to 128 output tokens; -1 = generate until context is
    // filled, so long summaries are not output-capped.
    num_predict: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Deserialize, Debug)]
struct OllamaChatResponse {
    message: OllamaChatResponseMessage,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
}

#[derive(Deserialize, Debug)]
struct OllamaChatResponseMessage {
    content: String,
}

/// LLM Provider enumeration for multi-provider support
#[derive(Debug, Clone, PartialEq)]
pub enum LLMProvider {
    OpenAI,
    Claude,
    Groq,
    Ollama,
    OpenRouter,
    BuiltInAI,
    CustomOpenAI,
    LMStudio,
    /// "Sign in with ChatGPT" — uses the user's ChatGPT subscription via the
    /// Codex responses endpoint instead of an API key. See openai::chatgpt_oauth.
    ChatGptSubscription,
}

impl LLMProvider {
    /// Parse provider from string (case-insensitive)
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(Self::OpenAI),
            "claude" => Ok(Self::Claude),
            "groq" => Ok(Self::Groq),
            "ollama" => Ok(Self::Ollama),
            "openrouter" => Ok(Self::OpenRouter),
            "builtin-ai" | "local-llama" | "localllama" => Ok(Self::BuiltInAI),
            "custom-openai" => Ok(Self::CustomOpenAI),
            "lmstudio" | "lm-studio" | "lm_studio" => Ok(Self::LMStudio),
            "chatgpt-subscription" | "chatgpt" => Ok(Self::ChatGptSubscription),
            _ => Err(format!("Unsupported LLM provider: {}", s)),
        }
    }
}

/// Generate text and discard any citations — the shape every summary caller
/// wants. New callers that care about sources should use `generate_answer`.
#[allow(clippy::too_many_arguments)]
pub async fn generate_summary(
    client: &Client,
    provider: &LLMProvider,
    model_name: &str,
    api_key: &str,
    system_prompt: &str,
    user_prompt: &str,
    images: &[ImageInput],
    ollama_endpoint: Option<&str>,
    custom_openai_endpoint: Option<&str>,
    lmstudio_endpoint: Option<&str>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    app_data_dir: Option<&PathBuf>,
    cancellation_token: Option<&CancellationToken>,
) -> Result<String, String> {
    generate_answer(
        client,
        provider,
        model_name,
        api_key,
        system_prompt,
        user_prompt,
        images,
        ollama_endpoint,
        custom_openai_endpoint,
        lmstudio_endpoint,
        max_tokens,
        temperature,
        top_p,
        app_data_dir,
        cancellation_token,
        LlmExtras::default(),
    )
    .await
    .map(|answer| answer.text)
}

/// Generates a summary using the specified LLM provider
///
/// # Arguments
/// * `client` - Reqwest HTTP client (reused for performance)
/// * `provider` - The LLM provider to use
/// * `model_name` - The specific model to use (e.g., "gpt-4", "claude-3-opus")
/// * `api_key` - API key for the provider (not needed for Ollama)
/// * `system_prompt` - System instructions for the LLM
/// * `user_prompt` - User query/content to process
/// * `images` - Image attachments for vision-capable models (empty slice for text-only
///   calls; BuiltInAI cannot view images and proceeds text-only)
/// * `ollama_endpoint` - Optional custom Ollama endpoint (defaults to localhost:11434)
/// * `custom_openai_endpoint` - Optional custom OpenAI-compatible endpoint
/// * `lmstudio_endpoint` - Optional custom LM Studio endpoint (defaults to localhost:1234)
/// * `max_tokens` - Optional max tokens (for CustomOpenAI provider)
/// * `temperature` - Optional temperature (for CustomOpenAI provider)
/// * `top_p` - Optional top_p (for CustomOpenAI provider)
/// * `app_data_dir` - Optional app data directory (for BuiltInAI provider)
/// * `cancellation_token` - Optional token to cancel the request
///
/// * `extras` - Optional capabilities such as provider-side web search
///
/// # Returns
/// The generated text plus any sources the provider cited, or an error message
#[allow(clippy::too_many_arguments)]
pub async fn generate_answer(
    client: &Client,
    provider: &LLMProvider,
    model_name: &str,
    api_key: &str,
    system_prompt: &str,
    user_prompt: &str,
    images: &[ImageInput],
    ollama_endpoint: Option<&str>,
    custom_openai_endpoint: Option<&str>,
    lmstudio_endpoint: Option<&str>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    app_data_dir: Option<&PathBuf>,
    cancellation_token: Option<&CancellationToken>,
    extras: LlmExtras,
) -> Result<LlmAnswer, String> {
    // Check if cancelled before starting
    if let Some(token) = cancellation_token {
        if token.is_cancelled() {
            return Err("Summary generation was cancelled".to_string());
        }
    }

    // Handle BuiltInAI provider separately (uses local sidecar, no HTTP API)
    if provider == &LLMProvider::BuiltInAI {
        if !images.is_empty() {
            tracing::warn!(
                "BuiltInAI cannot view images — proceeding text-only ({} image(s) skipped)",
                images.len()
            );
        }
        let app_data_dir = app_data_dir
            .ok_or_else(|| "app_data_dir is required for BuiltInAI provider".to_string())?;

        return crate::summary::summary_engine::generate_with_builtin(
            app_data_dir,
            model_name,
            system_prompt,
            user_prompt,
            cancellation_token,
        )
        .await
        .map(LlmAnswer::text_only)
        .map_err(|e| e.to_string());
    }

    // ChatGPT subscription talks the Codex "responses" protocol (SSE, different
    // endpoint + auth), not chat/completions — handle it in its own module. Auth
    // (token + refresh) lives in a file under app_data_dir, so no api_key needed.
    // Vision-capable models (GPT-5.x) read attached images; if the endpoint rejects
    // the image payload, the caller retries text-only.
    if provider == &LLMProvider::ChatGptSubscription {
        let app_data_dir = app_data_dir
            .ok_or_else(|| "app_data_dir is required for ChatGPT subscription".to_string())?;
        return crate::openai::chatgpt_oauth::generate_via_codex(
            client,
            model_name,
            system_prompt,
            user_prompt,
            images,
            app_data_dir,
            cancellation_token,
            extras.web_search,
        )
        .await
        .map(|answer| LlmAnswer {
            text: answer.text,
            sources: answer.sources,
            search_count: answer.search_count,
        });
    }

    // Claude has its own endpoint shape, and with web search on it also needs a
    // `pause_turn` continuation loop, so it gets a dedicated path rather than
    // more branching inside the OpenAI-compatible one below.
    if provider == &LLMProvider::Claude {
        return generate_claude_native(
            client,
            model_name,
            api_key,
            system_prompt,
            user_prompt,
            images,
            max_tokens,
            extras.web_search,
            cancellation_token,
        )
        .await;
    }

    // OpenAI can only search through the Responses API — `web_search_options` on
    // chat/completions is rejected by current models. Swap endpoints for that
    // case alone and leave the ordinary chat path untouched.
    if provider == &LLMProvider::OpenAI && extras.web_search {
        return generate_openai_responses(
            client,
            model_name,
            api_key,
            system_prompt,
            user_prompt,
            images,
            cancellation_token,
        )
        .await;
    }

    // Ollama uses its OWN native /api/chat path so we can send options.num_ctx (the
    // OpenAI-compat shim below cannot, which is why long meetings were silently
    // truncated to Ollama's ~4k default).
    if provider == &LLMProvider::Ollama {
        return generate_ollama_native(
            client,
            model_name,
            system_prompt,
            user_prompt,
            images,
            ollama_endpoint,
            temperature,
            cancellation_token,
        )
        .await
        .map(LlmAnswer::text_only);
    }

    let (api_url, mut headers) = match provider {
        LLMProvider::OpenAI => (
            "https://api.openai.com/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::Groq => (
            "https://api.groq.com/openai/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::OpenRouter => (
            "https://openrouter.ai/api/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::Ollama => {
            let host = ollama_endpoint
                .map(|s| s.to_string())
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            (
                format!("{}/v1/chat/completions", host),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::CustomOpenAI => {
            let endpoint = custom_openai_endpoint
                .ok_or_else(|| "Custom OpenAI endpoint not configured".to_string())?;
            (
                format!("{}/chat/completions", endpoint.trim_end_matches('/')),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::LMStudio => {
            let host = lmstudio_endpoint
                .map(|s| s.to_string())
                .unwrap_or_else(|| "http://localhost:1234".to_string());
            // Endpoint may be supplied with or without the /v1 suffix.
            let trimmed = host.trim_end_matches('/');
            let base = if trimmed.ends_with("/v1") {
                trimmed.to_string()
            } else {
                format!("{}/v1", trimmed)
            };
            (
                format!("{}/chat/completions", base),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::Claude | LLMProvider::BuiltInAI | LLMProvider::ChatGptSubscription => {
            // All three early-return above: Claude to its own endpoint shape,
            // BuiltInAI to the local sidecar, ChatGPT to the Codex protocol.
            unreachable!("{:?} is handled before this match statement", provider)
        }
    };

    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {}", api_key)
            .parse()
            .map_err(|_| "Invalid authorization header".to_string())?,
    );
    headers.insert(
        header::CONTENT_TYPE,
        "application/json"
            .parse()
            .map_err(|_| "Invalid content type".to_string())?,
    );

    // For CustomOpenAI, apply optional parameters if provided
    let (max_tokens_val, temperature_val, top_p_val) = if provider == &LLMProvider::CustomOpenAI {
        (max_tokens, temperature, top_p)
    } else {
        (None, None, None)
    };

    // OpenRouter enables its web plugin through the model slug rather than a
    // request field, so searching means asking for a different model id.
    let effective_model = if extras.web_search && provider == &LLMProvider::OpenRouter {
        web_search::openrouter_online_model(model_name)
    } else {
        model_name.to_string()
    };

    let request_body = serde_json::json!(ChatRequest {
        model: effective_model.clone(),
        messages: vec![
            ChatMessage::text("system", system_prompt),
            openai_user_message(user_prompt, images),
        ],
        max_tokens: max_tokens_val,
        temperature: temperature_val,
        top_p: top_p_val,
    });

    info!(
        "🐞 LLM Request to {}: model={}",
        provider_name(provider),
        effective_model
    );

    let response = send_with_cancellation(
        client
            .post(api_url)
            .headers(headers)
            .json(&request_body)
            .timeout(REQUEST_TIMEOUT_DURATION),
        cancellation_token,
    )
    .await?;

    if !response.status().is_success() {
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("LLM API request failed: {}", error_body));
    }

    let chat_response = response
        .json::<ChatResponse>()
        .await
        .map_err(|e| format!("Failed to parse LLM response: {}", e))?;

    info!(
        "🐞 LLM Response received from {}",
        provider_name(provider)
    );

    let message = &chat_response
        .choices
        .get(0)
        .ok_or("No content in LLM response")?
        .message;

    Ok(LlmAnswer {
        text: message.content.trim().to_string(),
        sources: web_search::dedupe_sources(web_search::sources_from_annotations(
            &message.annotations,
        )),
        // These providers return citations but no search count. Leave it at zero
        // rather than inventing one from the number of sources.
        search_count: 0,
    })
}

/// Send a request, aborting early if the caller's cancellation token fires.
async fn send_with_cancellation(
    request: reqwest::RequestBuilder,
    cancellation_token: Option<&CancellationToken>,
) -> Result<reqwest::Response, String> {
    let to_error = |e: reqwest::Error| {
        if e.is_timeout() {
            format!(
                "LLM request timed out after {}s",
                REQUEST_TIMEOUT_DURATION.as_secs()
            )
        } else {
            format!("Failed to send request to LLM: {}", e)
        }
    };

    match cancellation_token {
        Some(token) => tokio::select! {
            result = request.send() => result.map_err(to_error),
            _ = token.cancelled() => Err("Summary generation was cancelled".to_string()),
        },
        None => request.send().await.map_err(to_error),
    }
}

/// How many searches Claude may run for one question. Enough for a definition
/// or a comparison without letting a single chat turn balloon in cost.
const CLAUDE_MAX_WEB_SEARCHES: u32 = 5;

/// Cap on `pause_turn` continuations, so a pathological search loop cannot spin
/// forever. Whatever text has accumulated is returned when the cap is hit.
const CLAUDE_MAX_PAUSE_CONTINUATIONS: usize = 3;

/// Call Claude's Messages endpoint, optionally with its server-side web search.
///
/// Separate from the OpenAI-compatible path because the wire format differs in
/// three ways that all matter here: a top-level `system` field, a heterogeneous
/// `content` block array, and the `pause_turn` stop reason, which asks the
/// client to resend the assistant turn to let a long search continue.
#[allow(clippy::too_many_arguments)]
async fn generate_claude_native(
    client: &Client,
    model_name: &str,
    api_key: &str,
    system_prompt: &str,
    user_prompt: &str,
    images: &[ImageInput],
    max_tokens: Option<u32>,
    web_search: bool,
    cancellation_token: Option<&CancellationToken>,
) -> Result<LlmAnswer, String> {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        "x-api-key",
        api_key
            .parse()
            .map_err(|_| "Invalid API key format".to_string())?,
    );
    headers.insert(
        "anthropic-version",
        "2023-06-01"
            .parse()
            .map_err(|_| "Invalid anthropic version".to_string())?,
    );
    headers.insert(
        header::CONTENT_TYPE,
        "application/json"
            .parse()
            .map_err(|_| "Invalid content type".to_string())?,
    );

    let mut messages = vec![serde_json::to_value(claude_user_message(user_prompt, images))
        .map_err(|e| format!("Failed to build Claude message: {}", e))?];

    let mut answer = LlmAnswer::default();

    for turn in 0..=CLAUDE_MAX_PAUSE_CONTINUATIONS {
        let mut body = serde_json::json!({
            "model": model_name,
            // Was hardcoded to 2048, which cut long summaries and the translation
            // pass mid-output. Default to 8192; a user-provided max_tokens wins.
            "max_tokens": max_tokens.unwrap_or(8192),
            "system": system_prompt,
            "messages": messages,
        });
        if web_search {
            body["tools"] =
                serde_json::json!([web_search::claude_web_search_tool(CLAUDE_MAX_WEB_SEARCHES)]);
        }

        info!(
            "🐞 LLM Request to Claude: model={} web_search={} turn={}",
            model_name, web_search, turn
        );

        let response = send_with_cancellation(
            client
                .post("https://api.anthropic.com/v1/messages")
                .headers(headers.clone())
                .json(&body)
                .timeout(REQUEST_TIMEOUT_DURATION),
            cancellation_token,
        )
        .await?;

        if !response.status().is_success() {
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(format!("LLM API request failed: {}", error_body));
        }

        let raw: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse LLM response: {}", e))?;

        let content = raw
            .get("content")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([]));

        let (text, sources) = claude_text_and_sources(&content);
        if !text.is_empty() {
            if !answer.text.is_empty() {
                answer.text.push_str("\n\n");
            }
            answer.text.push_str(&text);
        }
        answer.sources.extend(sources);
        answer.search_count += raw
            .get("usage")
            .and_then(|u| u.get("server_tool_use"))
            .and_then(|t| t.get("web_search_requests"))
            .and_then(|n| n.as_u64())
            .unwrap_or(0) as u32;

        // Search failures come back inside a 200 response, so they are invisible
        // unless we look for them. The answer still stands (Claude falls back to
        // what it knows) but it is worth knowing why no sources appeared.
        for error_code in claude_search_errors(&content) {
            tracing::warn!("Claude web search returned an error: {}", error_code);
        }

        match raw.get("stop_reason").and_then(|s| s.as_str()) {
            Some("pause_turn") if turn < CLAUDE_MAX_PAUSE_CONTINUATIONS => {
                // Resend the assistant turn untouched. Each search result carries
                // `encrypted_content` the API decrypts to rebuild its context, and
                // it rejects the request outright if that is altered — which is
                // why this echoes the raw JSON rather than re-serializing a struct.
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": content,
                }));
                continue;
            }
            Some("pause_turn") => {
                tracing::warn!(
                    "Claude still paused after {} continuations; returning the partial answer",
                    CLAUDE_MAX_PAUSE_CONTINUATIONS
                );
            }
            Some("max_tokens") => {
                tracing::warn!(
                    "Claude response stopped at max_tokens — output may be truncated (raise max_tokens)"
                );
            }
            _ => {}
        }
        break;
    }

    if answer.text.trim().is_empty() {
        return Err("No content in LLM response".to_string());
    }
    answer.text = answer.text.trim().to_string();
    answer.sources = web_search::dedupe_sources(answer.sources);
    Ok(answer)
}

/// Answer text and citations from Claude's `content` block array.
///
/// Only `text` blocks carry the answer; `server_tool_use` and
/// `web_search_tool_result` blocks are search bookkeeping. Text is spread across
/// several blocks once citations are involved, so every one has to be joined —
/// reading `content[0]` would return "I'll search for that" and drop the answer.
fn claude_text_and_sources(content: &serde_json::Value) -> (String, Vec<WebSource>) {
    let mut text = String::new();
    let mut sources = Vec::new();
    let Some(blocks) = content.as_array() else {
        return (text, sources);
    };

    for block in blocks {
        if block.get("type").and_then(|t| t.as_str()) != Some("text") {
            continue;
        }
        if let Some(chunk) = block.get("text").and_then(|t| t.as_str()) {
            let chunk = chunk.trim();
            if !chunk.is_empty() {
                if !text.is_empty() && !text.ends_with(' ') {
                    text.push(' ');
                }
                text.push_str(chunk);
            }
        }
        if let Some(citations) = block.get("citations").and_then(|c| c.as_array()) {
            sources.extend(claude_sources_from_citations(citations));
        }
    }

    (text, sources)
}

fn claude_sources_from_citations(citations: &[serde_json::Value]) -> Vec<WebSource> {
    citations
        .iter()
        .filter(|c| {
            c.get("type").and_then(|t| t.as_str()) == Some("web_search_result_location")
        })
        .filter_map(|c| {
            Some(WebSource {
                url: c.get("url").and_then(|u| u.as_str())?.to_string(),
                title: c.get("title").and_then(|t| t.as_str()).map(str::to_string),
                cited_text: c
                    .get("cited_text")
                    .and_then(|t| t.as_str())
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty()),
            })
        })
        .collect()
}

/// Error codes from failed searches. A successful search puts a *list* in
/// `content`; a failed one puts a single error *object* there instead.
fn claude_search_errors(content: &serde_json::Value) -> Vec<String> {
    let Some(blocks) = content.as_array() else {
        return Vec::new();
    };
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("web_search_tool_result"))
        .filter_map(|b| {
            b.get("content")?
                .get("error_code")?
                .as_str()
                .map(str::to_string)
        })
        .collect()
}

/// Call OpenAI's Responses API with web search enabled.
///
/// Only reachable in web-search mode. Chat/completions cannot search on current
/// models — `web_search_options` is rejected by the gpt-5 series and the old
/// `*-search-preview` models were retired — so searching means a different
/// endpoint with a different request and response shape. Every other mode keeps
/// using the chat/completions path.
async fn generate_openai_responses(
    client: &Client,
    model_name: &str,
    api_key: &str,
    system_prompt: &str,
    user_prompt: &str,
    images: &[ImageInput],
    cancellation_token: Option<&CancellationToken>,
) -> Result<LlmAnswer, String> {
    // The Codex path already builds exactly this content-part array.
    let input = crate::openai::chatgpt_oauth::build_codex_input(user_prompt, images);
    let body = web_search::openai_responses_body(model_name, system_prompt, input);

    info!(
        "🐞 LLM Request to OpenAI (Responses + web search): model={}",
        model_name
    );

    let response = send_with_cancellation(
        client
            .post(web_search::OPENAI_RESPONSES_URL)
            .header(header::AUTHORIZATION, format!("Bearer {}", api_key))
            .header(header::CONTENT_TYPE, "application/json")
            .json(&body)
            .timeout(REQUEST_TIMEOUT_DURATION),
        cancellation_token,
    )
    .await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!(
            "OpenAI Responses request failed ({}): {}",
            status, error_body
        ));
    }

    let raw: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse OpenAI Responses payload: {}", e))?;

    let (text, sources, search_count) = web_search::parse_openai_responses(&raw)?;
    Ok(LlmAnswer {
        text,
        sources,
        search_count,
    })
}

/// Generate a summary via Ollama's native `/api/chat` endpoint with `options.num_ctx`
/// set to the model's real trained context, so long prompts are not silently truncated
/// to Ollama's ~4k default (the OpenAI-compat shim cannot set num_ctx).
async fn generate_ollama_native(
    client: &Client,
    model_name: &str,
    system_prompt: &str,
    user_prompt: &str,
    images: &[ImageInput],
    ollama_endpoint: Option<&str>,
    temperature: Option<f32>,
    cancellation_token: Option<&CancellationToken>,
) -> Result<String, String> {
    let host = ollama_endpoint
        .map(|s| s.to_string())
        .unwrap_or_else(|| "http://localhost:11434".to_string());
    let url = format!("{}/api/chat", host.trim_end_matches('/'));

    // Set num_ctx to the model's real context (the same value the summary chunker sizes
    // chunks against, so a chunk always fits). NOTE: on a very-large-context model this
    // asks Ollama to allocate a large KV cache and could OOM on limited hardware — the
    // same assumption the chunker already makes.
    let num_ctx = crate::ollama::metadata::METADATA_CACHE
        .get_or_fetch(model_name, ollama_endpoint)
        .await
        .map(|m| m.context_size as u32)
        .ok();

    let request_body = OllamaChatRequest {
        model: model_name.to_string(),
        messages: vec![
            OllamaChatMessage {
                role: "system".to_string(),
                content: system_prompt.to_string(),
                images: Vec::new(),
            },
            OllamaChatMessage {
                role: "user".to_string(),
                content: user_prompt.to_string(),
                images: images.iter().map(|img| img.base64_data.clone()).collect(),
            },
        ],
        stream: false,
        options: OllamaOptions {
            num_ctx,
            num_predict: -1,
            temperature,
        },
    };

    info!(
        "🐞 LLM Request to Ollama (native /api/chat): model={}, num_ctx={:?}",
        model_name, num_ctx
    );

    let request_future = client
        .post(&url)
        .json(&request_body)
        .timeout(REQUEST_TIMEOUT_DURATION)
        .send();

    let response = if let Some(token) = cancellation_token {
        tokio::select! {
            result = request_future => {
                result.map_err(|e| {
                    if e.is_timeout() {
                        "Ollama request timed out".to_string()
                    } else {
                        format!("Failed to send request to Ollama: {}", e)
                    }
                })?
            }
            _ = token.cancelled() => {
                return Err("Summary generation was cancelled".to_string());
            }
        }
    } else {
        request_future.await.map_err(|e| {
            if e.is_timeout() {
                "Ollama request timed out".to_string()
            } else {
                format!("Failed to send request to Ollama: {}", e)
            }
        })?
    };

    if !response.status().is_success() {
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("Ollama API request failed: {}", error_body));
    }

    let chat_response = response
        .json::<OllamaChatResponse>()
        .await
        .map_err(|e| format!("Failed to parse Ollama response: {}", e))?;

    // Truncation detection: if the served context was smaller than the prompt tokens.
    if let (Some(ctx), Some(eval)) = (num_ctx, chat_response.prompt_eval_count) {
        if eval >= ctx {
            tracing::warn!(
                "Ollama prompt_eval_count {} >= num_ctx {} — prompt may have been truncated",
                eval,
                ctx
            );
        }
    }
    if chat_response.done_reason.as_deref() == Some("length") {
        tracing::warn!("Ollama response stopped at 'length' — output may be truncated");
    }

    Ok(chat_response.message.content.trim().to_string())
}

/// Helper function to get provider name for logging
fn provider_name(provider: &LLMProvider) -> &str {
    match provider {
        LLMProvider::OpenAI => "OpenAI",
        LLMProvider::Claude => "Claude",
        LLMProvider::Groq => "Groq",
        LLMProvider::Ollama => "Ollama",
        LLMProvider::BuiltInAI => "Built-in AI",
        LLMProvider::OpenRouter => "OpenRouter",
        LLMProvider::CustomOpenAI => "Custom OpenAI",
        LLMProvider::LMStudio => "LM Studio",
        LLMProvider::ChatGptSubscription => "ChatGPT (subscription)",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ollama_request_serializes_num_ctx_and_num_predict() {
        let req = OllamaChatRequest {
            model: "llama3.2".to_string(),
            messages: vec![OllamaChatMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
                images: Vec::new(),
            }],
            stream: false,
            options: OllamaOptions {
                num_ctx: Some(32768),
                num_predict: -1,
                temperature: Some(0.3),
            },
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["options"]["num_ctx"], 32768);
        assert_eq!(v["options"]["num_predict"], -1);
        assert_eq!(v["stream"], false);
        // Text-only message must not gain an `images` key (older Ollama versions).
        assert!(v["messages"][0].get("images").is_none());
    }

    #[test]
    fn text_only_message_serializes_content_as_plain_string() {
        let msg = ChatMessage::text("user", "hello");
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["content"], "hello");
    }

    #[test]
    fn openai_message_with_images_serializes_parts() {
        let images = vec![ImageInput {
            media_type: "image/png".to_string(),
            base64_data: "AAAA".to_string(),
        }];
        let v = serde_json::to_value(openai_user_message("look", &images)).unwrap();
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][0]["text"], "look");
        assert_eq!(v["content"][1]["type"], "image_url");
        assert_eq!(
            v["content"][1]["image_url"]["url"],
            "data:image/png;base64,AAAA"
        );
    }

    #[test]
    fn claude_message_with_images_serializes_source_blocks() {
        let images = vec![ImageInput {
            media_type: "image/jpeg".to_string(),
            base64_data: "BBBB".to_string(),
        }];
        let v = serde_json::to_value(claude_user_message("look", &images)).unwrap();
        assert_eq!(v["content"][0]["type"], "image");
        assert_eq!(v["content"][0]["source"]["type"], "base64");
        assert_eq!(v["content"][0]["source"]["media_type"], "image/jpeg");
        assert_eq!(v["content"][0]["source"]["data"], "BBBB");
        assert_eq!(v["content"][1]["type"], "text");
        assert_eq!(v["content"][1]["text"], "look");
    }

    #[test]
    fn ollama_message_with_images_serializes_flat_base64() {
        let msg = OllamaChatMessage {
            role: "user".to_string(),
            content: "look".to_string(),
            images: vec!["CCCC".to_string()],
        };
        let v = serde_json::to_value(&msg).unwrap();
        assert_eq!(v["content"], "look");
        assert_eq!(v["images"][0], "CCCC");
    }

    #[test]
    fn ollama_response_parses_done_reason_and_eval_count() {
        let json = r#"{"message":{"content":"hello"},"done_reason":"length","prompt_eval_count":5000}"#;
        let resp: OllamaChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.message.content, "hello");
        assert_eq!(resp.done_reason.as_deref(), Some("length"));
        assert_eq!(resp.prompt_eval_count, Some(5000));
    }

    #[test]
    fn ollama_response_tolerates_missing_optional_fields() {
        let resp: OllamaChatResponse =
            serde_json::from_str(r#"{"message":{"content":"hi"}}"#).unwrap();
        assert_eq!(resp.done_reason, None);
        assert_eq!(resp.prompt_eval_count, None);
    }

    #[test]
    fn claude_plain_text_response_yields_the_text_and_no_sources() {
        let content = serde_json::json!([{ "type": "text", "text": "  summary  " }]);
        let (text, sources) = claude_text_and_sources(&content);
        assert_eq!(text, "summary");
        assert!(sources.is_empty());
    }

    /// The shape a search actually returns: bookkeeping blocks interleaved with
    /// several text blocks. Reading only `content[0]` here would answer
    /// "I'll search for that." and throw the real answer away.
    #[test]
    fn claude_web_search_response_joins_text_blocks_and_collects_citations() {
        let content = serde_json::json!([
            { "type": "text", "text": "I'll search for that." },
            {
                "type": "server_tool_use",
                "id": "srvtoolu_01",
                "name": "web_search",
                "input": { "query": "what is an ERP" }
            },
            {
                "type": "web_search_tool_result",
                "tool_use_id": "srvtoolu_01",
                "content": [{
                    "type": "web_search_result",
                    "url": "https://en.wikipedia.org/wiki/Enterprise_resource_planning",
                    "title": "Enterprise resource planning - Wikipedia",
                    "encrypted_content": "EqgfCioIARgBIiQ3YTAwMjY1Mi1",
                    "page_age": "April 30, 2026"
                }]
            },
            {
                "type": "text",
                "text": "An ERP integrates core business processes.",
                "citations": [{
                    "type": "web_search_result_location",
                    "url": "https://en.wikipedia.org/wiki/Enterprise_resource_planning",
                    "title": "Enterprise resource planning - Wikipedia",
                    "encrypted_index": "Eo8BCioIAhgBIiQyYjQ0",
                    "cited_text": "Enterprise resource planning (ERP) is the integrated management of..."
                }]
            }
        ]);

        let (text, sources) = claude_text_and_sources(&content);
        assert_eq!(
            text,
            "I'll search for that. An ERP integrates core business processes."
        );
        assert_eq!(sources.len(), 1);
        assert_eq!(
            sources[0].url,
            "https://en.wikipedia.org/wiki/Enterprise_resource_planning"
        );
        assert!(sources[0].cited_text.as_deref().unwrap().starts_with("Enterprise"));
    }

    #[test]
    fn claude_non_text_blocks_never_contribute_text() {
        let content = serde_json::json!([
            { "type": "server_tool_use", "id": "x", "name": "web_search", "input": {} },
            { "type": "web_search_tool_result", "tool_use_id": "x", "content": [] }
        ]);
        let (text, sources) = claude_text_and_sources(&content);
        assert!(text.is_empty());
        assert!(sources.is_empty());
    }

    /// A failed search still comes back as HTTP 200, with `content` holding an
    /// error object instead of the usual list.
    #[test]
    fn claude_search_errors_are_detected() {
        let content = serde_json::json!([{
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_01",
            "content": { "type": "web_search_tool_result_error", "error_code": "max_uses_exceeded" }
        }]);
        assert_eq!(claude_search_errors(&content), vec!["max_uses_exceeded"]);

        let ok = serde_json::json!([{
            "type": "web_search_tool_result",
            "tool_use_id": "srvtoolu_01",
            "content": [{ "type": "web_search_result", "url": "https://a.example" }]
        }]);
        assert!(claude_search_errors(&ok).is_empty());
    }

    #[test]
    fn openai_compatible_response_without_annotations_reports_no_sources() {
        let json = r#"{"choices":[{"message":{"content":"hello"}}]}"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.choices[0].message.content, "hello");
        assert!(resp.choices[0].message.annotations.is_empty());
    }

    #[test]
    fn openrouter_online_response_carries_url_citations() {
        let json = r#"{"choices":[{"message":{"content":"An ERP is...","annotations":[
            {"type":"url_citation","url_citation":{"url":"https://sap.com","title":"What is ERP"}}
        ]}}]}"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        let sources = web_search::sources_from_annotations(&resp.choices[0].message.annotations);
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].url, "https://sap.com");
    }
}
