//! "Sign in with ChatGPT" provider.
//!
//! Reuses the OAuth flow that OpenAI's Codex CLI uses so a user with a ChatGPT
//! Plus/Pro subscription can generate summaries against their subscription
//! instead of paying for API credits. Everything runs in-process — there is no
//! separate proxy to launch.
//!
//! Flow:
//! 1. `chatgpt_sign_in` runs a native PKCE OAuth flow: bind a tiny localhost
//!    listener on :1455, open the browser to `auth.openai.com`, receive the
//!    `code` on the callback, exchange it for tokens, store them in
//!    `{app_data_dir}/chatgpt_auth.json`.
//! 2. `generate_via_codex` (called from the summary path) reads that file,
//!    refreshes the access token if it is about to expire, and POSTs to the
//!    Codex responses endpoint, parsing the SSE stream into plain text.
//!
//! ⚠️ This uses the ChatGPT-subscription auth against the undocumented
//! `backend-api/codex` endpoint. It is outside OpenAI's intended API usage and
//! the endpoint can change without notice. Values below are pinned to what the
//! installed Codex CLI (`codex.exe`) actually sends.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Runtime};
use tokio_util::sync::CancellationToken;

use crate::summary::llm_client::ImageInput;

// --- Codex OAuth / endpoint constants (pinned to the Codex CLI) ---
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const REDIRECT_PORT: u16 = 1455;
const SCOPE: &str = "openid profile email offline_access";
const RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const ORIGINATOR: &str = "codex_cli_rs";
const OPENAI_BETA: &str = "responses=experimental";
const USER_AGENT: &str = "codex_cli_rs/0.0.0 (Murmur)";

const AUTH_FILENAME: &str = "chatgpt_auth.json";

/// Persisted credentials (mirrors the shape of Codex's own `auth.json`, but ours).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAuth {
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub id_token: Option<String>,
    /// ChatGPT account id, required as the `chatgpt-account-id` request header.
    pub account_id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    /// Absolute expiry of `access_token`, epoch milliseconds.
    pub expires_at_ms: i64,
}

/// Lightweight status returned to the UI (never leaks the tokens).
#[derive(Debug, Clone, Serialize)]
pub struct AuthStatus {
    pub signed_in: bool,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub account_id: Option<String>,
}

impl AuthStatus {
    fn signed_out() -> Self {
        Self {
            signed_in: false,
            email: None,
            plan: None,
            account_id: None,
        }
    }

    fn from_stored(a: &StoredAuth) -> Self {
        Self {
            signed_in: true,
            email: a.email.clone(),
            plan: a.plan.clone(),
            account_id: Some(a.account_id.clone()),
        }
    }
}

// -------------------- persistence --------------------

fn auth_file(dir: &Path) -> PathBuf {
    dir.join(AUTH_FILENAME)
}

fn load(dir: &Path) -> Option<StoredAuth> {
    let raw = std::fs::read_to_string(auth_file(dir)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn save(dir: &Path, auth: &StoredAuth) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("No pude crear el directorio de datos: {}", e))?;
    let json = serde_json::to_string_pretty(auth)
        .map_err(|e| format!("No pude serializar credenciales: {}", e))?;
    std::fs::write(auth_file(dir), json)
        .map_err(|e| format!("No pude guardar credenciales: {}", e))
}

fn clear(dir: &Path) {
    let _ = std::fs::remove_file(auth_file(dir));
}

// -------------------- PKCE + helpers --------------------

fn rand_bytes<const N: usize>() -> [u8; N] {
    use rand::RngCore;
    let mut buf = [0u8; N];
    rand::thread_rng().fill_bytes(&mut buf);
    buf
}

fn rand_string(n: usize) -> String {
    URL_SAFE_NO_PAD.encode(rand_bytes::<48>())[..n].to_string()
}

/// (verifier, challenge) — challenge = base64url(sha256(verifier)).
fn pkce() -> (String, String) {
    let verifier = URL_SAFE_NO_PAD.encode(rand_bytes::<32>());
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = URL_SAFE_NO_PAD.encode(hasher.finalize());
    (verifier, challenge)
}

fn build_authorize_url(challenge: &str, state: &str) -> String {
    // Codex-specific params: id_token_add_organizations + codex_cli_simplified_flow.
    let mut url = reqwest::Url::parse(AUTHORIZE_URL).expect("valid authorize url");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPE)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("state", state);
    url.to_string()
}

/// Decode the (unsigned) claims of a JWT. Returns Null on any failure.
fn decode_jwt_claims(jwt: &str) -> serde_json::Value {
    let payload = match jwt.split('.').nth(1) {
        Some(p) => p,
        None => return serde_json::Value::Null,
    };
    match URL_SAFE_NO_PAD.decode(payload) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    }
}

fn account_id_from_claims(claims: &serde_json::Value) -> Option<String> {
    claims
        .get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_account_id"))
        .and_then(|v| v.as_str())
        .or_else(|| claims.get("chatgpt_account_id").and_then(|v| v.as_str()))
        .map(|s| s.to_string())
}

fn email_from_claims(claims: &serde_json::Value) -> Option<String> {
    claims
        .get("email")
        .and_then(|v| v.as_str())
        .or_else(|| {
            claims
                .get("https://api.openai.com/profile")
                .and_then(|p| p.get("email"))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string())
}

fn plan_from_claims(claims: &serde_json::Value) -> Option<String> {
    claims
        .get("https://api.openai.com/auth")
        .and_then(|a| a.get("chatgpt_plan_type"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// -------------------- token exchange --------------------

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

async fn exchange_code(client: &Client, code: &str, verifier: &str) -> Result<TokenResponse, String> {
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .map_err(|e| format!("No pude intercambiar el código: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Intercambio de token falló ({}): {}", status, body));
    }
    resp.json::<TokenResponse>()
        .await
        .map_err(|e| format!("Respuesta de token inválida: {}", e))
}

async fn refresh_token(client: &Client, refresh: &str) -> Result<TokenResponse, String> {
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", CLIENT_ID),
            ("scope", SCOPE),
        ])
        .send()
        .await
        .map_err(|e| format!("No pude refrescar el token: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Refresh de token falló ({}): {}", status, body));
    }
    resp.json::<TokenResponse>()
        .await
        .map_err(|e| format!("Respuesta de refresh inválida: {}", e))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn build_stored(tok: TokenResponse) -> Result<StoredAuth, String> {
    let refresh = tok
        .refresh_token
        .ok_or("El login no devolvió refresh_token (falta scope offline_access)")?;
    let claims = tok
        .id_token
        .as_deref()
        .map(decode_jwt_claims)
        .unwrap_or(serde_json::Value::Null);
    let account_id = account_id_from_claims(&claims)
        .ok_or("No pude obtener el account id de ChatGPT del id_token")?;
    let expires_at_ms = now_ms() + tok.expires_in.unwrap_or(3600) * 1000;
    Ok(StoredAuth {
        access_token: tok.access_token,
        refresh_token: refresh,
        email: email_from_claims(&claims),
        plan: plan_from_claims(&claims),
        account_id,
        id_token: tok.id_token,
        expires_at_ms,
    })
}

// -------------------- localhost callback listener --------------------

fn read_callback(mut stream: TcpStream) -> Option<(String, String)> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .ok();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8192 {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    let request = String::from_utf8_lossy(&buf);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("");
    let parsed = reqwest::Url::parse(&format!("http://localhost{}", path)).ok();

    let (mut code, mut state) = (None, None);
    if let Some(url) = &parsed {
        for (k, v) in url.query_pairs() {
            match k.as_ref() {
                "code" => code = Some(v.to_string()),
                "state" => state = Some(v.to_string()),
                _ => {}
            }
        }
    }

    let html = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Murmur</title></head>\
<body style=\"font-family:system-ui,sans-serif;text-align:center;padding-top:4rem;color:#222\">\
<h2>✅ Conectado a ChatGPT</h2><p>Ya podés cerrar esta pestaña y volver a Murmur.</p></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();

    Some((code?, state?))
}

/// Blocking: waits (up to 5 min) for the OAuth redirect on the pre-bound listener.
fn accept_callback(listener: TcpListener) -> Result<(String, String), String> {
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(pair) = read_callback(stream) {
                    return Ok(pair);
                }
                // Not the callback (e.g. favicon.ico) — keep waiting.
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("El login expiró (5 minutos sin completarse)".to_string());
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(format!("Error esperando el callback: {}", e)),
        }
    }
}

// -------------------- public: sign in / out / status --------------------

pub async fn sign_in<R: Runtime>(app: &AppHandle<R>) -> Result<AuthStatus, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("No pude resolver el directorio de datos: {}", e))?;

    // Bind BEFORE opening the browser so we never miss the redirect.
    let listener = TcpListener::bind(("127.0.0.1", REDIRECT_PORT)).map_err(|e| {
        format!(
            "No pude abrir el puerto {} para el login (¿tenés Codex corriendo un login?): {}",
            REDIRECT_PORT, e
        )
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("No pude configurar el listener: {}", e))?;

    let (verifier, challenge) = pkce();
    let state = rand_string(24);
    let auth_url = build_authorize_url(&challenge, &state);

    {
        use tauri_plugin_opener::OpenerExt;
        app.opener()
            .open_url(auth_url, None::<&str>)
            .map_err(|e| format!("No pude abrir el navegador: {}", e))?;
    }

    let (code, returned_state) = tokio::task::spawn_blocking(move || accept_callback(listener))
        .await
        .map_err(|e| format!("Fallo interno esperando el login: {}", e))??;

    if returned_state != state {
        return Err("El state no coincide (posible intento de CSRF); login abortado".to_string());
    }

    let client = Client::new();
    let tok = exchange_code(&client, &code, &verifier).await?;
    let stored = build_stored(tok)?;
    save(&dir, &stored)?;
    Ok(AuthStatus::from_stored(&stored))
}

pub fn sign_out<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("No pude resolver el directorio de datos: {}", e))?;
    clear(&dir);
    Ok(())
}

pub fn status<R: Runtime>(app: &AppHandle<R>) -> AuthStatus {
    let dir = match app.path().app_data_dir() {
        Ok(d) => d,
        Err(_) => return AuthStatus::signed_out(),
    };
    match load(&dir) {
        Some(a) => AuthStatus::from_stored(&a),
        None => AuthStatus::signed_out(),
    }
}

// -------------------- token retrieval (with refresh) --------------------

/// Returns a valid (access_token, account_id), refreshing + persisting if the
/// stored access token is within 60s of expiry.
async fn get_valid_token(client: &Client, dir: &Path) -> Result<(String, String), String> {
    let mut stored = load(dir).ok_or(
        "No estás conectado a ChatGPT. Andá a Settings → modelo y apretá 'Sign in with ChatGPT'.",
    )?;

    if stored.expires_at_ms - now_ms() < 60_000 {
        let tok = refresh_token(client, &stored.refresh_token).await?;
        stored.access_token = tok.access_token;
        stored.expires_at_ms = now_ms() + tok.expires_in.unwrap_or(3600) * 1000;
        if let Some(rt) = tok.refresh_token {
            stored.refresh_token = rt;
        }
        if let Some(idt) = tok.id_token {
            let claims = decode_jwt_claims(&idt);
            if let Some(acc) = account_id_from_claims(&claims) {
                stored.account_id = acc;
            }
            if let Some(email) = email_from_claims(&claims) {
                stored.email = Some(email);
            }
            if let Some(plan) = plan_from_claims(&claims) {
                stored.plan = Some(plan);
            }
            stored.id_token = Some(idt);
        }
        save(dir, &stored)?;
    }

    if stored.account_id.is_empty() {
        return Err("No tengo el account id de ChatGPT; volvé a iniciar sesión".to_string());
    }
    Ok((stored.access_token, stored.account_id))
}

// -------------------- codex responses call --------------------

/// Append the `output_text` parts of a `message` item to `text`.
fn collect_message_text(item: &serde_json::Value, text: &mut String) {
    if item.get("type").and_then(|t| t.as_str()) != Some("message") {
        return;
    }
    if let Some(content) = item.get("content").and_then(|c| c.as_array()) {
        for part in content {
            if part.get("type").and_then(|t| t.as_str()) == Some("output_text") {
                if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                    text.push_str(t);
                }
            }
        }
    }
}

/// Navigate a `response.completed` / `response.output_item.done` event and pull
/// out any `output_text` text (fallback when no deltas were streamed).
fn extract_completed_text(event: &serde_json::Value) -> Option<String> {
    let mut text = String::new();
    // response.completed → response.output: [items]
    if let Some(output) = event
        .get("response")
        .and_then(|r| r.get("output"))
        .and_then(|o| o.as_array())
    {
        for item in output {
            collect_message_text(item, &mut text);
        }
    }
    // response.output_item.done → item: { single item }
    if let Some(item) = event.get("item") {
        collect_message_text(item, &mut text);
    }
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Parse a buffered SSE body into the final assistant text.
fn parse_sse(body: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut fallback: Option<String> = None;

    for line in body.lines() {
        let line = line.trim_start();
        let data = match line.strip_prefix("data:") {
            Some(d) => d.trim(),
            None => continue,
        };
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let event: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match event.get("type").and_then(|t| t.as_str()) {
            Some("response.output_text.delta") => {
                if let Some(d) = event.get("delta").and_then(|d| d.as_str()) {
                    out.push_str(d);
                }
            }
            Some("response.completed") | Some("response.output_item.done") => {
                if let Some(t) = extract_completed_text(&event) {
                    fallback = Some(t);
                }
            }
            Some("response.failed") | Some("error") => {
                let msg = event
                    .get("response")
                    .and_then(|r| r.get("error"))
                    .or_else(|| event.get("error"))
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| data.to_string());
                return Err(format!("ChatGPT devolvió un error: {}", msg));
            }
            _ => {}
        }
    }

    let text = if !out.trim().is_empty() {
        out
    } else {
        fallback.unwrap_or_default()
    };

    if text.trim().is_empty() {
        return Err("La respuesta de ChatGPT vino vacía".to_string());
    }
    Ok(text.trim().to_string())
}

/// Build the Responses-API `input` array: a single user message whose content is
/// the text prompt followed by any images as `input_image` parts. The responses
/// schema takes `image_url` as a plain string (a data URI here), unlike the
/// chat/completions `{ "url": ... }` shape. With no images this is byte-for-byte
/// the old text-only body, so image-less requests are unchanged.
fn build_codex_input(user_prompt: &str, images: &[ImageInput]) -> serde_json::Value {
    let mut content = vec![serde_json::json!({
        "type": "input_text",
        "text": user_prompt,
    })];
    for img in images {
        content.push(serde_json::json!({
            "type": "input_image",
            "image_url": format!("data:{};base64,{}", img.media_type, img.base64_data),
        }));
    }
    serde_json::json!([{
        "type": "message",
        "role": "user",
        "content": content,
    }])
}

/// Generate a completion through the ChatGPT subscription (Codex responses
/// endpoint). Vision-capable models (GPT-5.x) can read attached images, which are
/// sent as `input_image` data URIs; pass an empty slice for a text-only call. If
/// the endpoint rejects the image payload, the caller's text-only retry recovers.
pub async fn generate_via_codex(
    client: &Client,
    model: &str,
    system_prompt: &str,
    user_prompt: &str,
    images: &[ImageInput],
    app_data_dir: &Path,
    cancellation_token: Option<&CancellationToken>,
) -> Result<String, String> {
    let (token, account_id) = get_valid_token(client, app_data_dir).await?;
    let session_id = uuid::Uuid::new_v4().to_string();

    let body = serde_json::json!({
        "model": model,
        "instructions": system_prompt,
        "input": build_codex_input(user_prompt, images),
        "store": false,
        "stream": true,
        "include": ["reasoning.encrypted_content"],
        "prompt_cache_key": session_id,
        "reasoning": { "effort": "low" }
    });

    let request = client
        .post(RESPONSES_URL)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", token))
        .header("chatgpt-account-id", &account_id)
        .header("OpenAI-Beta", OPENAI_BETA)
        .header("originator", ORIGINATOR)
        .header("session_id", &session_id)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .json(&body)
        .timeout(Duration::from_secs(120))
        .send();

    let response = if let Some(token) = cancellation_token {
        tokio::select! {
            r = request => r.map_err(|e| format!("Request a ChatGPT falló: {}", e))?,
            _ = token.cancelled() => return Err("Generación cancelada".to_string()),
        }
    } else {
        request
            .await
            .map_err(|e| format!("Request a ChatGPT falló: {}", e))?
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "El endpoint codex de ChatGPT respondió {}: {}",
            status, body
        ));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("Error leyendo la respuesta de ChatGPT: {}", e))?;
    parse_sse(&body)
}

// -------------------- available models --------------------

/// Fallback model list, only used when Codex's cache can't be read (e.g. Codex
/// not installed). The live source of truth is `~/.codex/models_cache.json`,
/// which Codex refreshes itself — so we don't have to maintain this by hand.
const FALLBACK_MODELS: &[&str] = &["gpt-5.4", "gpt-5.5", "gpt-5.4-mini"];

fn codex_home() -> PathBuf {
    if let Ok(h) = std::env::var("CODEX_HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    dirs::home_dir().unwrap_or_default().join(".codex")
}

/// Read the model slugs from Codex's own auto-refreshed cache.
fn read_codex_models_cache() -> Option<Vec<String>> {
    let raw = std::fs::read_to_string(codex_home().join("models_cache.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let models = value.get("models")?.as_array()?;
    let slugs: Vec<String> = models
        .iter()
        .filter_map(|m| m.get("slug").and_then(|s| s.as_str()).map(String::from))
        .collect();
    if slugs.is_empty() {
        None
    } else {
        Some(slugs)
    }
}

/// The models this ChatGPT account can use with Codex. Prefers Codex's live
/// cache; falls back to a short built-in list if it can't be read.
pub fn list_models() -> Vec<String> {
    read_codex_models_cache()
        .unwrap_or_else(|| FALLBACK_MODELS.iter().map(|s| s.to_string()).collect())
}

// -------------------- Tauri commands --------------------

#[tauri::command]
pub async fn chatgpt_sign_in<R: Runtime>(app: AppHandle<R>) -> Result<AuthStatus, String> {
    sign_in(&app).await
}

#[tauri::command]
pub async fn chatgpt_sign_out<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    sign_out(&app)
}

#[tauri::command]
pub async fn chatgpt_status<R: Runtime>(app: AppHandle<R>) -> Result<AuthStatus, String> {
    Ok(status(&app))
}

#[tauri::command]
pub async fn chatgpt_list_models() -> Result<Vec<String>, String> {
    Ok(list_models())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let (verifier, challenge) = pkce();
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        assert_eq!(challenge, URL_SAFE_NO_PAD.encode(hasher.finalize()));
    }

    #[test]
    fn parse_sse_accumulates_deltas() {
        let body = "\
data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hola\"}\n\
data: {\"type\":\"response.output_text.delta\",\"delta\":\" mundo\"}\n\
data: {\"type\":\"response.completed\"}\n\
data: [DONE]\n";
        assert_eq!(parse_sse(body).unwrap(), "Hola mundo");
    }

    #[test]
    fn parse_sse_falls_back_to_completed() {
        let body = "\
data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Resumen final\"}]}]}}\n";
        assert_eq!(parse_sse(body).unwrap(), "Resumen final");
    }

    #[test]
    fn codex_input_is_text_only_without_images() {
        let input = build_codex_input("hello", &[]);
        let content = &input[0]["content"];
        assert_eq!(content.as_array().unwrap().len(), 1);
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[0]["text"], "hello");
    }

    #[test]
    fn codex_input_appends_images_as_data_uris() {
        let images = vec![ImageInput {
            media_type: "image/png".to_string(),
            base64_data: "AAAA".to_string(),
        }];
        let input = build_codex_input("look", &images);
        let content = &input[0]["content"];
        assert_eq!(content.as_array().unwrap().len(), 2);
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], "data:image/png;base64,AAAA");
    }

    #[test]
    fn account_id_read_from_namespaced_claim() {
        let claims = serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "acc-123", "chatgpt_plan_type": "plus" },
            "email": "user@example.com"
        });
        assert_eq!(account_id_from_claims(&claims).as_deref(), Some("acc-123"));
        assert_eq!(email_from_claims(&claims).as_deref(), Some("user@example.com"));
        assert_eq!(plan_from_claims(&claims).as_deref(), Some("plus"));
    }
}
