//! In-memory store for the live "Ask AI" conversation held during a recording.
//!
//! The conversation lives alongside the in-memory transcript segments: cleared
//! when a recording starts, drained into a persisted `chat_threads` row by the
//! stop path once the meeting id exists, and readable at any time so a webview
//! reload can rehydrate the panel (same resilience story as `get_transcript_history`).
//!
//! Like `RECORDING_MANAGER`, this is a std `Mutex` — every operation is sync and
//! the guard must NEVER be held across an `.await`.
//!
//! HONEST LIMITATION: memory-only. A hard crash loses the live conversation
//! (crash recovery re-imports transcripts.jsonl only); a future live_chat.jsonl
//! journal could close that gap.

use std::sync::Mutex;

use serde::Serialize;

use crate::database::repositories::chat::DEFAULT_GROUNDING_MODE;

#[derive(Debug, Clone, Serialize)]
pub struct LiveChatMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Same JSON as `chat_messages.metadata` — how the answer was grounded and
    /// what it cited. Carried here so the panel can show sources live, and so
    /// they survive into the persisted thread at stop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
}

static LIVE_CHAT: Mutex<Vec<LiveChatMessage>> = Mutex::new(Vec::new());

/// Grounding mode the Ask-AI panel is currently using. The live panel has no
/// `chat_threads` row to store it on yet, so it is held here and written onto
/// the thread when the conversation is persisted at stop — otherwise a live
/// conversation that searched the web would reopen as transcript-only.
static LIVE_GROUNDING: Mutex<String> = Mutex::new(String::new());

/// A copy of the conversation so far, in insertion order.
pub fn snapshot() -> Vec<LiveChatMessage> {
    LIVE_CHAT.lock().unwrap().clone()
}

/// Append a message and return it (with its generated id and timestamp).
pub fn append(role: &str, content: &str, metadata: Option<&str>) -> LiveChatMessage {
    let msg = LiveChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: role.to_string(),
        content: content.to_string(),
        created_at: chrono::Utc::now(),
        metadata: metadata.map(str::to_string),
    };
    LIVE_CHAT.lock().unwrap().push(msg.clone());
    msg
}

/// Remove a message by id (rollback of a user message whose LLM call failed).
pub fn remove(id: &str) {
    LIVE_CHAT.lock().unwrap().retain(|m| m.id != id);
}

/// Drain the conversation for persistence at stop. Leaves the store empty.
pub fn take_all() -> Vec<LiveChatMessage> {
    std::mem::take(&mut *LIVE_CHAT.lock().unwrap())
}

/// Remember the grounding mode of the most recent Ask-AI question.
pub fn set_grounding_mode(mode: &str) {
    *LIVE_GROUNDING.lock().unwrap() = mode.to_string();
}

/// The mode to stamp on the persisted thread. Falls back to the strict default
/// when the panel was never used or only used before this was introduced.
pub fn grounding_mode() -> String {
    let mode = LIVE_GROUNDING.lock().unwrap().clone();
    if mode.is_empty() {
        DEFAULT_GROUNDING_MODE.to_string()
    } else {
        mode
    }
}

pub fn clear() {
    LIVE_CHAT.lock().unwrap().clear();
    LIVE_GROUNDING.lock().unwrap().clear();
}
