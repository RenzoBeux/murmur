//! Native, provider-side web search for the meeting chat.
//!
//! The LLM layer in this app has no client-side tool/function-calling loop —
//! `generate_answer` sends one request and reads one response. So "search the
//! web" is delegated to each provider's own server-side search tool, which keeps
//! a searched answer to a single round trip (Claude may add continuations; see
//! the `pause_turn` handling in `llm_client`).
//!
//! Not every provider can do this, and the ones that can't must degrade visibly
//! rather than quietly answer from training data while the UI claims it
//! searched. `web_search_support` is the single source of truth for that; the
//! frontend reads it through the `api_chat_web_search_support` command instead
//! of keeping its own copy of the table.

use serde::{Deserialize, Serialize};

use super::llm_client::LLMProvider;

/// One cited web page behind an answer. Persisted on the assistant message so
/// the citation chips survive a reload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebSource {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Short excerpt the provider says the claim came from, when it gives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cited_text: Option<String>,
}

/// Whether a provider/model pair can search the web server-side.
#[derive(Debug, Clone, PartialEq)]
pub enum WebSearchSupport {
    Native,
    /// Carries a user-facing reason, shown in the picker tooltip.
    Unsupported(&'static str),
}

impl WebSearchSupport {
    pub fn is_native(&self) -> bool {
        matches!(self, Self::Native)
    }

    /// `None` when supported, otherwise the reason it isn't.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Self::Native => None,
            Self::Unsupported(reason) => Some(reason),
        }
    }
}

/// Can this provider/model search the web?
///
/// Local providers are unsupported by construction — they have no search
/// infrastructure — which is also why enabling web search can never turn a
/// local-only setup into one that talks to the network. See
/// `lib/providerLocality.ts` on the frontend for the matching egress model.
pub fn web_search_support(provider: &LLMProvider, model: &str) -> WebSearchSupport {
    match provider {
        // Server-side `web_search` tool on the same /v1/messages endpoint.
        LLMProvider::Claude => WebSearchSupport::Native,

        // `:online` model suffix routes through OpenRouter's search plugin.
        LLMProvider::OpenRouter => WebSearchSupport::Native,

        // Needs the Responses API — chat/completions cannot search on current
        // models. `generate_answer` switches endpoints for this case only.
        LLMProvider::OpenAI => WebSearchSupport::Native,

        // Codex speaks a Responses-shaped protocol, so the tool *should* apply,
        // but that backend is not publicly specified. The first web-mode call
        // probes it and records the answer for the rest of the process.
        LLMProvider::ChatGptSubscription => {
            if crate::openai::chatgpt_oauth::web_search_rejected() {
                WebSearchSupport::Unsupported(
                    "This ChatGPT model rejected web search, so answers come from general knowledge.",
                )
            } else {
                WebSearchSupport::Native
            }
        }

        // Only Groq's "compound" systems ship built-in web search; a plain
        // Llama / GPT-OSS deployment on Groq has none.
        LLMProvider::Groq => {
            if model.to_lowercase().contains("compound") {
                WebSearchSupport::Native
            } else {
                WebSearchSupport::Unsupported(
                    "Groq can only search on its `compound` models. Pick one of those, or use Claude, OpenAI or OpenRouter.",
                )
            }
        }

        LLMProvider::Ollama | LLMProvider::LMStudio | LLMProvider::BuiltInAI => {
            WebSearchSupport::Unsupported(
                "Local models run entirely on this machine and cannot search the web. Pick a cloud provider to search.",
            )
        }

        LLMProvider::CustomOpenAI => WebSearchSupport::Unsupported(
            "Custom OpenAI-compatible endpoints don't expose a standard web search tool.",
        ),
    }
}

// -------------------- Claude --------------------

/// Anthropic's server-side search tool.
///
/// Deliberately the basic `web_search_20250305` version: it works on every
/// Claude model and calls search directly. The newer `_20260209` / `_20260318`
/// variants add dynamic filtering but require Claude 4.6+ and default to running
/// search from inside code execution — a 400 on older models, and a model's
/// generation can't be read off its slug reliably enough to switch on it.
pub fn claude_web_search_tool(max_uses: u32) -> serde_json::Value {
    serde_json::json!({
        "type": "web_search_20250305",
        "name": "web_search",
        "max_uses": max_uses,
    })
}

// -------------------- OpenRouter --------------------

/// Append OpenRouter's `:online` suffix, which enables its web plugin.
///
/// Idempotent: a model the user already picked as `…:online` is left alone.
pub fn openrouter_online_model(model: &str) -> String {
    if model.ends_with(":online") {
        model.to_string()
    } else {
        format!("{}:online", model)
    }
}

// -------------------- citation extraction --------------------

/// Pull web sources out of an OpenAI-style `annotations` array.
///
/// Two shapes exist in the wild and both turn up here: chat/completions
/// (OpenRouter) nests the fields under `url_citation`, while the Responses API
/// (OpenAI) puts `url` / `title` directly on the annotation. Read either.
pub fn sources_from_annotations(annotations: &[serde_json::Value]) -> Vec<WebSource> {
    let mut out = Vec::new();
    for annotation in annotations {
        if annotation.get("type").and_then(|t| t.as_str()) != Some("url_citation") {
            continue;
        }
        // Nested shape first, flat shape as the fallback.
        let fields = annotation.get("url_citation").unwrap_or(annotation);
        let Some(url) = fields.get("url").and_then(|u| u.as_str()) else {
            continue;
        };
        out.push(WebSource {
            url: url.to_string(),
            title: fields
                .get("title")
                .and_then(|t| t.as_str())
                .map(str::to_string),
            cited_text: fields
                .get("content")
                .and_then(|c| c.as_str())
                .map(truncate_excerpt),
        });
    }
    out
}

/// Excerpts are only shown as chip tooltips, and OpenRouter's run to a few
/// thousand characters — not worth persisting in full on every message.
fn truncate_excerpt(text: &str) -> String {
    const MAX: usize = 300;
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(MAX).collect();
    format!("{}…", cut.trim_end())
}

/// Collapse repeats (providers cite the same page for several sentences) while
/// keeping first-seen order, preferring whichever entry carries more detail.
pub fn dedupe_sources(sources: Vec<WebSource>) -> Vec<WebSource> {
    let mut out: Vec<WebSource> = Vec::new();
    for source in sources {
        match out.iter_mut().find(|existing| existing.url == source.url) {
            Some(existing) => {
                if existing.title.is_none() {
                    existing.title = source.title;
                }
                if existing.cited_text.is_none() {
                    existing.cited_text = source.cited_text;
                }
            }
            None => out.push(source),
        }
    }
    out
}

// -------------------- OpenAI Responses API --------------------

pub const OPENAI_RESPONSES_URL: &str = "https://api.openai.com/v1/responses";

/// Body for a one-shot Responses call with web search enabled.
///
/// `instructions` is the Responses-API equivalent of a system message, and
/// `input` takes the same content-part array the Codex path builds.
pub fn openai_responses_body(
    model: &str,
    system_prompt: &str,
    input: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "instructions": system_prompt,
        "input": input,
        "tools": [{ "type": "web_search" }],
        "store": false,
    })
}

/// Text + citations + search count from a Responses payload.
///
/// The `output` array interleaves `web_search_call` and `reasoning` items with
/// the actual `message`; only `output_text` parts carry answer text.
pub fn parse_openai_responses(
    body: &serde_json::Value,
) -> Result<(String, Vec<WebSource>, u32), String> {
    let output = body
        .get("output")
        .and_then(|o| o.as_array())
        .ok_or_else(|| "OpenAI Responses payload has no `output` array".to_string())?;

    let mut text = String::new();
    let mut sources = Vec::new();
    let mut search_count = 0u32;

    for item in output {
        match item.get("type").and_then(|t| t.as_str()) {
            Some("web_search_call") => search_count += 1,
            Some("message") => {
                let Some(parts) = item.get("content").and_then(|c| c.as_array()) else {
                    continue;
                };
                for part in parts {
                    if part.get("type").and_then(|t| t.as_str()) != Some("output_text") {
                        continue;
                    }
                    if let Some(chunk) = part.get("text").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(chunk);
                    }
                    if let Some(annotations) = part.get("annotations").and_then(|a| a.as_array()) {
                        sources.extend(sources_from_annotations(annotations));
                    }
                }
            }
            _ => {}
        }
    }

    if text.trim().is_empty() {
        // A refusal or an incomplete run lands here; surface the status rather
        // than showing an empty assistant bubble.
        let status = body
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("unknown");
        return Err(format!(
            "OpenAI Responses returned no text (status: {})",
            status
        ));
    }

    Ok((
        text.trim().to_string(),
        dedupe_sources(sources),
        search_count,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_providers_never_support_web_search() {
        for provider in [
            LLMProvider::Ollama,
            LLMProvider::LMStudio,
            LLMProvider::BuiltInAI,
            LLMProvider::CustomOpenAI,
        ] {
            let support = web_search_support(&provider, "whatever");
            assert!(
                !support.is_native(),
                "{:?} must not claim web search support",
                provider
            );
            assert!(support.reason().is_some(), "{:?} needs a reason", provider);
        }
    }

    #[test]
    fn cloud_providers_with_native_search_are_supported() {
        assert!(web_search_support(&LLMProvider::Claude, "claude-opus-5").is_native());
        assert!(web_search_support(&LLMProvider::OpenAI, "gpt-5.5").is_native());
        assert!(
            web_search_support(&LLMProvider::OpenRouter, "anthropic/claude-sonnet-5").is_native()
        );
    }

    #[test]
    fn groq_only_searches_on_compound_models() {
        assert!(web_search_support(&LLMProvider::Groq, "groq/compound").is_native());
        assert!(!web_search_support(&LLMProvider::Groq, "llama-3.3-70b-versatile").is_native());
    }

    #[test]
    fn online_suffix_is_idempotent() {
        assert_eq!(
            openrouter_online_model("openai/gpt-5.5"),
            "openai/gpt-5.5:online"
        );
        assert_eq!(
            openrouter_online_model("openai/gpt-5.5:online"),
            "openai/gpt-5.5:online"
        );
    }

    #[test]
    fn reads_both_nested_and_flat_annotation_shapes() {
        // OpenRouter / chat-completions shape.
        let nested = serde_json::json!([{
            "type": "url_citation",
            "url_citation": { "url": "https://a.example", "title": "A", "content": "excerpt" }
        }]);
        let sources = sources_from_annotations(nested.as_array().unwrap());
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].url, "https://a.example");
        assert_eq!(sources[0].title.as_deref(), Some("A"));
        assert_eq!(sources[0].cited_text.as_deref(), Some("excerpt"));

        // OpenAI Responses shape.
        let flat = serde_json::json!([{
            "type": "url_citation",
            "url": "https://b.example",
            "title": "B",
            "start_index": 0,
            "end_index": 9
        }]);
        let sources = sources_from_annotations(flat.as_array().unwrap());
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].url, "https://b.example");
        assert_eq!(sources[0].title.as_deref(), Some("B"));
    }

    #[test]
    fn non_citation_annotations_are_ignored() {
        let annotations = serde_json::json!([
            { "type": "file_citation", "url": "https://ignored.example" },
            { "type": "url_citation" }
        ]);
        assert!(sources_from_annotations(annotations.as_array().unwrap()).is_empty());
    }

    #[test]
    fn dedupe_keeps_order_and_fills_missing_fields() {
        let sources = vec![
            WebSource {
                url: "https://a.example".into(),
                title: None,
                cited_text: None,
            },
            WebSource {
                url: "https://b.example".into(),
                title: Some("B".into()),
                cited_text: None,
            },
            WebSource {
                url: "https://a.example".into(),
                title: Some("A".into()),
                cited_text: Some("x".into()),
            },
        ];
        let deduped = dedupe_sources(sources);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].url, "https://a.example");
        assert_eq!(deduped[0].title.as_deref(), Some("A"));
        assert_eq!(deduped[0].cited_text.as_deref(), Some("x"));
        assert_eq!(deduped[1].url, "https://b.example");
    }

    #[test]
    fn parses_responses_output_with_search_calls_and_citations() {
        let body = serde_json::json!({
            "status": "completed",
            "output": [
                { "type": "reasoning", "summary": [] },
                { "type": "web_search_call", "id": "ws_1", "status": "completed" },
                { "type": "web_search_call", "id": "ws_2", "status": "completed" },
                {
                    "type": "message",
                    "content": [{
                        "type": "output_text",
                        "text": "An ERP integrates core business processes.",
                        "annotations": [
                            { "type": "url_citation", "url": "https://en.wikipedia.org/wiki/ERP", "title": "ERP" },
                            { "type": "url_citation", "url": "https://en.wikipedia.org/wiki/ERP", "title": "ERP" }
                        ]
                    }]
                }
            ]
        });

        let (text, sources, searches) = parse_openai_responses(&body).unwrap();
        assert_eq!(text, "An ERP integrates core business processes.");
        assert_eq!(searches, 2);
        assert_eq!(sources.len(), 1, "repeated citations collapse to one source");
        assert_eq!(sources[0].url, "https://en.wikipedia.org/wiki/ERP");
    }

    #[test]
    fn responses_without_text_is_an_error_not_an_empty_answer() {
        let body = serde_json::json!({
            "status": "incomplete",
            "output": [{ "type": "web_search_call", "id": "ws_1" }]
        });
        let err = parse_openai_responses(&body).unwrap_err();
        assert!(
            err.contains("incomplete"),
            "error should carry the status: {err}"
        );
    }

    #[test]
    fn long_excerpts_are_truncated() {
        let long = "x".repeat(1000);
        let annotations = serde_json::json!([{
            "type": "url_citation",
            "url_citation": { "url": "https://a.example", "content": long }
        }]);
        let sources = sources_from_annotations(annotations.as_array().unwrap());
        let excerpt = sources[0].cited_text.as_deref().unwrap();
        assert!(
            excerpt.chars().count() <= 301,
            "got {} chars",
            excerpt.chars().count()
        );
        assert!(excerpt.ends_with('…'));
    }
}
