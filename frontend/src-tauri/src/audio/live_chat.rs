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

#[derive(Debug, Clone, Serialize)]
pub struct LiveChatMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

static LIVE_CHAT: Mutex<Vec<LiveChatMessage>> = Mutex::new(Vec::new());

/// A copy of the conversation so far, in insertion order.
pub fn snapshot() -> Vec<LiveChatMessage> {
    LIVE_CHAT.lock().unwrap().clone()
}

/// Append a message and return it (with its generated id and timestamp).
pub fn append(role: &str, content: &str) -> LiveChatMessage {
    let msg = LiveChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role: role.to_string(),
        content: content.to_string(),
        created_at: chrono::Utc::now(),
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

pub fn clear() {
    LIVE_CHAT.lock().unwrap().clear();
}
