use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::info;

const REQUEST_TIMEOUT_DURATION: Duration = Duration::from_secs(300);

/// An image attachment ready to send to a vision-capable model.
#[derive(Debug, Clone)]
pub struct ImageInput {
    /// e.g. "image/png" — already validated against the supported set.
    pub media_type: String,
    /// Base64-encoded file bytes (no data: prefix).
    pub base64_data: String,
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
}

// Claude-specific request structure
#[derive(Debug, Serialize)]
pub struct ClaudeRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: String,
    pub messages: Vec<ChatMessage>,
}

// Claude-specific response structure
#[derive(Deserialize, Debug)]
pub struct ClaudeChatResponse {
    pub content: Vec<ClaudeChatContent>,
    // "max_tokens" here means the output was cut at the token cap (truncated).
    #[serde(default)]
    pub stop_reason: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct ClaudeChatContent {
    pub text: String,
}

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
/// # Returns
/// The generated summary text or an error message
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
        .await;
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
        LLMProvider::Claude => {
            let mut header_map = header::HeaderMap::new();
            header_map.insert(
                "x-api-key",
                api_key
                    .parse()
                    .map_err(|_| "Invalid API key format".to_string())?,
            );
            header_map.insert(
                "anthropic-version",
                "2023-06-01"
                    .parse()
                    .map_err(|_| "Invalid anthropic version".to_string())?,
            );
            ("https://api.anthropic.com/v1/messages".to_string(), header_map)
        }
        LLMProvider::BuiltInAI => {
            // This case is handled earlier with early returns
            unreachable!("BuiltInAI is handled before this match statement")
        }
        LLMProvider::ChatGptSubscription => {
            // Handled earlier with an early return (Codex responses protocol)
            unreachable!("ChatGptSubscription is handled before this match statement")
        }
    };

    // Add authorization header for non-Claude providers
    if provider != &LLMProvider::Claude {
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", api_key)
                .parse()
                .map_err(|_| "Invalid authorization header".to_string())?,
        );
    }
    headers.insert(
        header::CONTENT_TYPE,
        "application/json"
            .parse()
            .map_err(|_| "Invalid content type".to_string())?,
    );

    // Build request body based on provider
    let request_body = if provider != &LLMProvider::Claude {
        // For CustomOpenAI, apply optional parameters if provided
        let (max_tokens_val, temperature_val, top_p_val) = if provider == &LLMProvider::CustomOpenAI {
            (max_tokens, temperature, top_p)
        } else {
            (None, None, None)
        };

        serde_json::json!(ChatRequest {
            model: model_name.to_string(),
            messages: vec![
                ChatMessage::text("system", system_prompt),
                openai_user_message(user_prompt, images),
            ],
            max_tokens: max_tokens_val,
            temperature: temperature_val,
            top_p: top_p_val,
        })
    } else {
        serde_json::json!(ClaudeRequest {
            system: system_prompt.to_string(),
            model: model_name.to_string(),
            // Was hardcoded to 2048, which cut long summaries and the translation pass
            // mid-output. Default to 8192; a user-provided max_tokens overrides.
            max_tokens: max_tokens.unwrap_or(8192),
            messages: vec![claude_user_message(user_prompt, images)]
        })
    };

    info!("🐞 LLM Request to {}: model={}", provider_name(provider), model_name);

    // Send request with timeout and cancellation support
    let request_future = client
        .post(api_url)
        .headers(headers)
        .json(&request_body)
        .timeout(REQUEST_TIMEOUT_DURATION)
        .send();

    // Use tokio::select to race between cancellation and request completion
    let response = if let Some(token) = cancellation_token {
        tokio::select! {
            result = request_future => {
                result.map_err(|e| {
                    if e.is_timeout() {
                        format!("LLM request timed out after 60 seconds")
                    } else {
                        format!("Failed to send request to LLM: {}", e)
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
                format!("LLM request timed out after 60 seconds")
            } else {
                format!("Failed to send request to LLM: {}", e)
            }
        })?
    };

    if !response.status().is_success() {
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("LLM API request failed: {}", error_body));
    }

    // Parse response based on provider
    if provider == &LLMProvider::Claude {
        let chat_response = response
            .json::<ClaudeChatResponse>()
            .await
            .map_err(|e| format!("Failed to parse LLM response: {}", e))?;

        info!("🐞 LLM Response received from Claude");

        if chat_response.stop_reason.as_deref() == Some("max_tokens") {
            tracing::warn!(
                "Claude response stopped at max_tokens — summary may be truncated (raise max_tokens)"
            );
        }

        let content = chat_response
            .content
            .get(0)
            .ok_or("No content in LLM response")?
            .text
            .trim();
        Ok(content.to_string())
    } else {
        let chat_response = response
            .json::<ChatResponse>()
            .await
            .map_err(|e| format!("Failed to parse LLM response: {}", e))?;

        info!("🐞 LLM Response received from {}", provider_name(provider));

        let content = chat_response
            .choices
            .get(0)
            .ok_or("No content in LLM response")?
            .message
            .content
            .trim();
        Ok(content.to_string())
    }
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
    fn claude_response_parses_stop_reason() {
        let json = r#"{"content":[{"text":"summary"}],"stop_reason":"max_tokens"}"#;
        let resp: ClaudeChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.stop_reason.as_deref(), Some("max_tokens"));
        assert_eq!(resp.content[0].text, "summary");
    }

    #[test]
    fn claude_response_tolerates_missing_stop_reason() {
        let resp: ClaudeChatResponse =
            serde_json::from_str(r#"{"content":[{"text":"x"}]}"#).unwrap();
        assert_eq!(resp.stop_reason, None);
    }
}
