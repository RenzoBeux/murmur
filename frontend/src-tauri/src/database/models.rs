use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct MeetingModel {
    pub id: String,
    pub title: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub folder_path: Option<String>,
    /// The project this meeting is filed under, or None when unfiled.
    pub project_id: Option<String>,
}

/// A named folder grouping meetings. See `ProjectsRepository`.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectModel {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Palette slug (see `PROJECT_COLORS`), or None for projects that predate
    /// the color picker — the UI derives one from the id in that case.
    pub color: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
pub struct DateTimeUtc(pub DateTime<Utc>);

impl From<NaiveDateTime> for DateTimeUtc {
    fn from(naive: NaiveDateTime) -> Self {
        DateTimeUtc(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
    }
}

// Renamed from TranscriptSegment to Transcript to match the table name
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Transcript {
    pub id: String,
    pub meeting_id: String,
    pub transcript: String,
    pub timestamp: String,
    pub summary: Option<String>,
    pub action_items: Option<String>,
    pub key_points: Option<String>,
    // Recording-relative timestamps for audio-transcript synchronization
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub duration: Option<f64>,
    // Source-faithful speaker tag ("mic"/"system"); diarization may overwrite later.
    pub speaker: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SummaryProcess {
    pub meeting_id: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub error: Option<String>,
    pub result: Option<String>, // JSON
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    pub chunk_count: i64,
    pub processing_time: f64,
    pub metadata: Option<String>, // JSON
    pub result_backup: Option<String>, // Backup of result before regeneration
    pub result_backup_timestamp: Option<chrono::DateTime<chrono::Utc>>, // When backup was created
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TranscriptChunk {
    pub meeting_id: String,
    pub meeting_name: Option<String>,
    pub transcript_text: String,
    pub model: String,
    pub model_name: String,
    pub chunk_size: Option<i64>,
    pub overlap: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Setting {
    pub id: String,
    pub provider: String,
    pub model: String,
    #[sqlx(rename = "whisperModel")]
    #[serde(rename = "whisperModel")]
    pub whisper_model: String,
    #[sqlx(rename = "groqApiKey")]
    #[serde(rename = "groqApiKey")]
    pub groq_api_key: Option<String>,
    #[sqlx(rename = "openaiApiKey")]
    #[serde(rename = "openaiApiKey")]
    pub openai_api_key: Option<String>,
    #[sqlx(rename = "anthropicApiKey")]
    #[serde(rename = "anthropicApiKey")]
    pub anthropic_api_key: Option<String>,
    #[sqlx(rename = "ollamaApiKey")]
    #[serde(rename = "ollamaApiKey")]
    pub ollama_api_key: Option<String>,
    #[sqlx(rename = "openRouterApiKey")]
    #[serde(rename = "openRouterApiKey")]
    pub open_router_api_key: Option<String>,
    #[sqlx(rename = "ollamaEndpoint")]
    #[serde(rename = "ollamaEndpoint")]
    pub ollama_endpoint: Option<String>,
    /// Custom OpenAI-compatible endpoint configuration stored as JSON
    #[sqlx(rename = "customOpenAIConfig")]
    #[serde(rename = "customOpenAIConfig")]
    pub custom_openai_config: Option<String>,
    #[sqlx(rename = "lmStudioEndpoint")]
    #[serde(rename = "lmStudioEndpoint")]
    pub lm_studio_endpoint: Option<String>,
}

impl Setting {
    /// Parse the custom OpenAI config from JSON string
    pub fn get_custom_openai_config(&self) -> Option<crate::summary::CustomOpenAIConfig> {
        self.custom_openai_config.as_ref().and_then(|json| {
            serde_json::from_str(json).ok()
        })
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct MeetingAttachmentModel {
    pub id: String,
    pub meeting_id: String,
    /// Original display name ("whiteboard.jpg").
    pub file_name: String,
    /// Collision-free filename inside {app_data_dir}/attachments/{meeting_id}/.
    pub stored_name: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ChatMessageModel {
    pub id: String,
    pub meeting_id: String,
    /// Nullable in the schema (ALTER TABLE limitation); the repositories always
    /// write it, so None only appears on rows that pre-date the threads migration
    /// backfill (which should not exist).
    pub thread_id: Option<String>,
    pub role: String,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// JSON describing where an assistant answer came from and what it cited
    /// (see `ChatAnswerMetadata` in api/chat_api.rs). None on user messages and
    /// on every row written before chat grounding modes existed.
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ChatThreadModel {
    pub id: String,
    pub meeting_id: String,
    pub title: String,
    /// 'live' (carried over from an Ask-AI session during recording) or 'post'.
    pub origin: String,
    /// How far past the transcript this conversation may reach:
    /// 'transcript_only' | 'general_knowledge' | 'web_search'.
    pub grounding_mode: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TranscriptSetting {
    pub id: String,
    pub provider: String,
    pub model: String,
    #[sqlx(rename = "whisperApiKey")]
    #[serde(rename = "whisperApiKey")]
    pub whisper_api_key: Option<String>,
    #[sqlx(rename = "deepgramApiKey")]
    #[serde(rename = "deepgramApiKey")]
    pub deepgram_api_key: Option<String>,
    #[sqlx(rename = "elevenLabsApiKey")]
    #[serde(rename = "elevenLabsApiKey")]
    pub eleven_labs_api_key: Option<String>,
    #[sqlx(rename = "groqApiKey")]
    #[serde(rename = "groqApiKey")]
    pub groq_api_key: Option<String>,
    #[sqlx(rename = "openaiApiKey")]
    #[serde(rename = "openaiApiKey")]
    pub openai_api_key: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectChatThreadModel {
    pub id: String,
    pub project_id: String,
    pub title: String,
    /// Always 'post' -- a recording belongs to a meeting, never to a project.
    /// The field exists so this row shape matches `ChatThreadModel`.
    pub origin: String,
    /// How far past the project's meetings this conversation may reach:
    /// 'transcript_only' | 'general_knowledge' | 'web_search'.
    pub grounding_mode: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectChatMessageModel {
    pub id: String,
    pub project_id: String,
    /// NOT NULL in the schema, unlike `ChatMessageModel::thread_id` -- that one
    /// is optional only because it arrived by ALTER TABLE.
    pub thread_id: String,
    pub role: String,
    pub content: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// JSON describing where an assistant answer came from, what it cited, and
    /// which meetings were actually in context (see `ChatAnswerMetadata` in
    /// api/chat_common.rs). None on user messages.
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct ProjectSummaryModel {
    pub project_id: String,
    /// 'PENDING' | 'completed' | 'failed' | 'cancelled'. The absence of a row is
    /// what the UI shows as "idle"; that value is never stored.
    pub status: String,
    /// JSON, same envelope as `SummaryProcess::result`: {"markdown": "..."}.
    pub result: Option<String>,
    pub error: Option<String>,
    /// The previous good brief, parked here for the duration of a run and
    /// restored on failure, cancellation, or an interrupted restart.
    pub result_backup: Option<String>,
    /// JSON array of {id,title,createdAt,source,fingerprint} describing what the
    /// STORED result covers -- a snapshot, not the project's current membership.
    pub covered_meetings: Option<String>,
    /// Fingerprint over `covered_meetings`, so "has anything changed at all" is
    /// one string compare.
    pub coverage_fingerprint: Option<String>,
    /// 'collecting' | 'mapping' | 'reducing' | 'synthesizing'.
    pub stage: Option<String>,
    pub stage_current: i64,
    pub stage_total: i64,
    pub output_language: Option<String>,
    pub model_provider: Option<String>,
    pub model_name: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    pub processing_time: f64,
}
