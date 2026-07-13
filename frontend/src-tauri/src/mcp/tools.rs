// Data access + rendering for the built-in MCP server. Direct read-only SQL
// (not the repositories) so this mirrors the schema exactly like the old
// Python `backend/mcp_server/server.py` did, including its fallbacks for
// legacy rows.

use anyhow::{anyhow, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use std::path::Path;

/// Open the Murmur database strictly read-only — this server can never
/// modify meeting data, and it coexists with a running app (WAL readers).
pub async fn open_readonly_pool(db_path: &Path) -> Result<SqlitePool> {
    if !db_path.exists() {
        return Err(anyhow!(
            "Murmur database not found at '{}'. Launch the Murmur app once to create it, \
             or pass --db <path> / set MURMUR_DB_PATH.",
            db_path.display()
        ));
    }
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .read_only(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await?;
    Ok(pool)
}

/// Render recording-relative seconds as `mm:ss` (or `h:mm:ss`).
pub fn fmt_timestamp(seconds: Option<f64>) -> String {
    let Some(seconds) = seconds else {
        return String::new();
    };
    let total = seconds.max(0.0) as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Decode a `summary_processes.result` blob. Historically stored either
/// JSON-encoded once or double-encoded (a JSON string containing JSON).
pub fn parse_summary(raw: &str) -> Option<serde_json::Value> {
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    if let serde_json::Value::String(inner) = &value {
        value = serde_json::from_str(inner).ok()?;
    }
    value.is_object().then_some(value)
}

fn render_blocks(blocks: &serde_json::Value) -> Vec<String> {
    let mut lines = Vec::new();
    let Some(blocks) = blocks.as_array() else {
        return lines;
    };
    for block in blocks {
        let Some(content) = block.get("content").and_then(|c| c.as_str()) else {
            continue;
        };
        let content = content.trim();
        if content.is_empty() {
            continue;
        }
        match block.get("type").and_then(|t| t.as_str()).unwrap_or("text") {
            "heading1" => lines.push(format!("## {content}")),
            "heading2" => lines.push(format!("### {content}")),
            "bullet" => lines.push(format!("- {content}")),
            _ => lines.push(content.to_string()),
        }
    }
    lines
}

/// Render a parsed summary as markdown.
///
/// Current format: `{"markdown": "..."}` (plus an internal english cache) —
/// return the markdown directly. Legacy formats: ordered
/// `MeetingNotes.sections` or named top-level block sections.
pub fn render_summary(summary: &serde_json::Value) -> String {
    if let Some(markdown) = summary.get("markdown").and_then(|m| m.as_str()) {
        let trimmed = markdown.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    let mut parts: Vec<String> = Vec::new();
    if let Some(name) = summary.get("MeetingName").and_then(|n| n.as_str()) {
        parts.push(format!("# {name}"));
    }

    let sections = summary
        .get("MeetingNotes")
        .and_then(|n| n.get("sections"))
        .and_then(|s| s.as_array())
        .cloned();

    if let Some(sections) = sections.filter(|s| !s.is_empty()) {
        for section in &sections {
            let body = render_blocks(section.get("blocks").unwrap_or(&serde_json::Value::Null));
            if body.is_empty() {
                continue;
            }
            if let Some(title) = section.get("title").and_then(|t| t.as_str()) {
                let title = title.trim();
                if !title.is_empty() {
                    parts.push(format!("## {title}"));
                }
            }
            parts.extend(body);
        }
    } else {
        const SECTION_KEYS: &[&str] = &[
            "People",
            "SessionSummary",
            "CriticalDeadlines",
            "KeyItemsDecisions",
            "ImmediateActionItems",
            "NextSteps",
        ];
        for key in SECTION_KEYS {
            let Some(section) = summary.get(*key) else {
                continue;
            };
            let body = render_blocks(section.get("blocks").unwrap_or(&serde_json::Value::Null));
            if body.is_empty() {
                continue;
            }
            let title = section
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or(key);
            parts.push(format!("## {title}"));
            parts.extend(body);
        }
    }

    let rendered = parts.join("\n\n").trim().to_string();
    if rendered.is_empty() {
        "(Summary is empty.)".to_string()
    } else {
        rendered
    }
}

/// Case-insensitive context snippet around the first occurrence of `query`.
pub fn snippet(text: &str, query: &str) -> String {
    let lower_text = text.to_lowercase();
    let lower_query = query.to_lowercase();
    let Some(idx) = lower_text.find(&lower_query) else {
        return text.chars().take(200).collect();
    };
    // Work in char indices to stay on UTF-8 boundaries.
    let char_idx = text[..idx].chars().count();
    let query_chars = query.chars().count();
    let chars: Vec<char> = text.chars().collect();
    let start = char_idx.saturating_sub(100);
    let end = (char_idx + query_chars + 100).min(chars.len());
    let mut out: String = chars[start..end].iter().collect();
    if start > 0 {
        out = format!("…{out}");
    }
    if end < chars.len() {
        out = format!("{out}…");
    }
    out
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// Whether the opened DB carries soft-delete (`meetings.deleted_at`). A database
/// opened read-only via `--db` may predate the soft-delete migration, so we
/// probe the schema and skip the trashed-meeting filter when the column is
/// absent rather than erroring on "no such column".
async fn meetings_has_soft_delete(pool: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM pragma_table_info('meetings') WHERE name = 'deleted_at'",
    )
    .fetch_one(pool)
    .await
    .map(|count| count > 0)
    .unwrap_or(false)
}

pub async fn list_meetings(pool: &SqlitePool, limit: i64) -> Result<String> {
    // Hide trashed meetings when the schema supports it (see meetings_has_soft_delete).
    let trash_filter = if meetings_has_soft_delete(pool).await {
        "WHERE m.deleted_at IS NULL"
    } else {
        ""
    };
    let sql = format!(
        "SELECT m.id, m.title, m.created_at, sp.status AS summary_status \
         FROM meetings m \
         LEFT JOIN summary_processes sp ON sp.meeting_id = m.id \
         {trash_filter} \
         ORDER BY m.created_at DESC \
         LIMIT ?"
    );
    let rows = sqlx::query(&sql)
        .bind(limit.clamp(1, 500))
        .fetch_all(pool)
        .await?;

    if rows.is_empty() {
        return Ok("No meetings found in the Murmur database yet.".to_string());
    }
    let mut lines = vec![format!("Found {} meeting(s):", rows.len()), String::new()];
    for row in &rows {
        let id: String = row.try_get("id")?;
        let title: String = row.try_get("title")?;
        let created_at: String = row.try_get::<String, _>("created_at").unwrap_or_default();
        let status: Option<String> = row.try_get("summary_status").ok();
        let badge = if status.as_deref().map(str::to_lowercase).as_deref() == Some("completed") {
            "📝 summary"
        } else {
            "—"
        };
        lines.push(format!("- **{title}** · `{id}` · {created_at} · {badge}"));
    }
    Ok(lines.join("\n"))
}

pub async fn get_transcript(pool: &SqlitePool, meeting_id: &str) -> Result<String> {
    let Some(meeting) = sqlx::query(
        "SELECT id, title, created_at FROM meetings WHERE id = ?",
    )
    .bind(meeting_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(format!(
            "No meeting found with id '{meeting_id}'. Use list_meetings to see valid ids."
        ));
    };

    let title: String = meeting.try_get("title")?;
    let created_at: String = meeting.try_get::<String, _>("created_at").unwrap_or_default();
    let header = format!("# {title}\n(meeting_id: {meeting_id} · created {created_at})");

    let segments = sqlx::query(
        r#"
        SELECT transcript, audio_start_time, speaker
        FROM transcripts
        WHERE meeting_id = ?
        ORDER BY audio_start_time IS NULL, audio_start_time, rowid
        "#,
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await?;

    if !segments.is_empty() {
        let mut lines = Vec::with_capacity(segments.len());
        for seg in &segments {
            let text: String = seg.try_get("transcript")?;
            let start: Option<f64> = seg.try_get("audio_start_time").ok();
            let speaker: Option<String> = seg.try_get("speaker").ok().flatten();
            let mut prefix = String::new();
            let ts = fmt_timestamp(start);
            if !ts.is_empty() {
                prefix.push_str(&format!("[{ts}] "));
            }
            if let Some(speaker) = speaker.filter(|s| !s.is_empty()) {
                prefix.push_str(&format!("{speaker}: "));
            }
            lines.push(format!("{prefix}{text}").trim().to_string());
        }
        return Ok(format!("{header}\n\n{}", lines.join("\n")));
    }

    // Legacy fallback: pre-segment recordings stored one blob per meeting.
    let chunk: Option<String> = sqlx::query_scalar(
        "SELECT transcript_text FROM transcript_chunks WHERE meeting_id = ?",
    )
    .bind(meeting_id)
    .fetch_optional(pool)
    .await?;

    match chunk.filter(|c| !c.trim().is_empty()) {
        Some(chunk) => Ok(format!("{header}\n\n{chunk}")),
        None => Ok(format!("{header}\n\n(No transcript recorded for this meeting.)")),
    }
}

pub async fn get_summary(pool: &SqlitePool, meeting_id: &str) -> Result<String> {
    let Some(title) = sqlx::query_scalar::<_, String>("SELECT title FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(format!(
            "No meeting found with id '{meeting_id}'. Use list_meetings to see valid ids."
        ));
    };

    let Some(row) = sqlx::query(
        "SELECT status, result, error FROM summary_processes WHERE meeting_id = ?",
    )
    .bind(meeting_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(format!("No summary has been generated for '{title}' yet."));
    };

    let status: String = row
        .try_get::<Option<String>, _>("status")?
        .unwrap_or_else(|| "unknown".to_string())
        .to_lowercase();
    let result: Option<String> = row.try_get("result").ok().flatten();
    let error: Option<String> = row.try_get("error").ok().flatten();

    match status.as_str() {
        "processing" | "pending" | "started" => {
            return Ok(format!(
                "Summary for '{title}' is still being generated (status: {status})."
            ))
        }
        "failed" => {
            return Ok(format!(
                "Summary generation failed for '{title}': {}",
                error.as_deref().unwrap_or("unknown error")
            ))
        }
        _ => {}
    }

    let parsed = result.as_deref().and_then(parse_summary);
    match parsed {
        Some(summary) => Ok(render_summary(&summary)),
        None => Ok(format!(
            "Summary for '{title}' is marked '{status}' but no summary data is available."
        )),
    }
}

pub async fn get_meeting(pool: &SqlitePool, meeting_id: &str) -> Result<String> {
    let summary = get_summary(pool, meeting_id).await?;
    let transcript = get_transcript(pool, meeting_id).await?;
    Ok(format!("{summary}\n\n---\n\n{transcript}"))
}

pub async fn search_transcripts(pool: &SqlitePool, query: &str, limit: i64) -> Result<String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok("Please provide a non-empty search query.".to_string());
    }
    let limit = limit.clamp(1, 100);
    let like = format!("%{}%", query.to_lowercase());

    // Exclude trashed meetings when the schema supports it. `sd_and` is a
    // compile-time literal (no user input), so interpolating it is injection-safe.
    let sd_and = if meetings_has_soft_delete(pool).await {
        "m.deleted_at IS NULL AND "
    } else {
        ""
    };

    let mut results: Vec<(String, String, String)> = Vec::new(); // (id, title, context)
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let seg_sql = format!(
        "SELECT m.id, m.title, t.transcript \
         FROM meetings m \
         JOIN transcripts t ON t.meeting_id = m.id \
         WHERE {sd_and}LOWER(t.transcript) LIKE ? \
         ORDER BY m.created_at DESC \
         LIMIT ?"
    );
    let seg_rows = sqlx::query(&seg_sql)
        .bind(&like)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    for row in &seg_rows {
        let id: String = row.try_get("id")?;
        let title: String = row.try_get("title")?;
        let text: String = row.try_get("transcript")?;
        seen.insert(id.clone());
        results.push((id, title, snippet(&text, query)));
    }

    if (results.len() as i64) < limit {
        let chunk_sql = format!(
            "SELECT m.id, m.title, tc.transcript_text \
             FROM meetings m \
             JOIN transcript_chunks tc ON tc.meeting_id = m.id \
             WHERE {sd_and}LOWER(tc.transcript_text) LIKE ? \
             ORDER BY m.created_at DESC \
             LIMIT ?"
        );
        let chunk_rows = sqlx::query(&chunk_sql)
            .bind(&like)
            .bind(limit)
            .fetch_all(pool)
            .await?;
        for row in &chunk_rows {
            let id: String = row.try_get("id")?;
            if seen.contains(&id) {
                continue;
            }
            let title: String = row.try_get("title")?;
            let text: String = row.try_get("transcript_text")?;
            results.push((id, title, snippet(&text, query)));
        }
    }

    results.truncate(limit as usize);
    if results.is_empty() {
        return Ok(format!("No transcripts matched '{query}'."));
    }
    let mut lines = vec![format!("Found {} match(es) for '{query}':", results.len()), String::new()];
    for (id, title, context) in &results {
        lines.push(format!("- **{title}** · `{id}`"));
        lines.push(format!("  > {context}"));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_timestamp_renders_minutes_and_hours() {
        assert_eq!(fmt_timestamp(None), "");
        assert_eq!(fmt_timestamp(Some(65.9)), "01:05");
        assert_eq!(fmt_timestamp(Some(3661.0)), "1:01:01");
    }

    #[test]
    fn parse_summary_handles_single_and_double_encoding() {
        assert!(parse_summary(r##"{"markdown":"# Hi"}"##).is_some());
        assert!(parse_summary(r##""{\"markdown\":\"# Hi\"}""##).is_some());
        assert!(parse_summary("not json").is_none());
        assert!(parse_summary("[1,2,3]").is_none());
    }

    #[test]
    fn render_summary_prefers_current_markdown_format() {
        let summary = serde_json::json!({
            "markdown": "## Action Items\n- Ship it",
            "english_cache": {"markdown": "internal"}
        });
        assert_eq!(render_summary(&summary), "## Action Items\n- Ship it");
    }

    #[test]
    fn render_summary_falls_back_to_legacy_blocks() {
        let summary = serde_json::json!({
            "MeetingName": "Sync",
            "SessionSummary": {
                "title": "Session Summary",
                "blocks": [
                    {"type": "bullet", "content": "Discussed roadmap"},
                    {"type": "text", "content": ""}
                ]
            }
        });
        let rendered = render_summary(&summary);
        assert!(rendered.contains("# Sync"));
        assert!(rendered.contains("## Session Summary"));
        assert!(rendered.contains("- Discussed roadmap"));
    }

    #[test]
    fn render_summary_empty_object_yields_placeholder() {
        assert_eq!(render_summary(&serde_json::json!({})), "(Summary is empty.)");
    }

    #[test]
    fn snippet_windows_around_match_and_marks_truncation() {
        let text = format!("{}NEEDLE{}", "a".repeat(150), "b".repeat(150));
        let s = snippet(&text, "needle");
        assert!(s.starts_with('…'));
        assert!(s.ends_with('…'));
        assert!(s.contains("NEEDLE"));
        assert!(s.chars().count() <= 100 + 6 + 100 + 2);
    }

    #[test]
    fn snippet_without_match_returns_head() {
        let text = "x".repeat(300);
        assert_eq!(snippet(&text, "zzz").chars().count(), 200);
    }

    #[test]
    fn snippet_is_utf8_safe_near_multibyte_chars() {
        let text = format!("{}ñeedle{}", "é".repeat(120), "ü".repeat(120));
        let s = snippet(&text, "ñeedle");
        assert!(s.contains("ñeedle"));
    }

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE meetings (id TEXT PRIMARY KEY, title TEXT, created_at TEXT, updated_at TEXT, deleted_at TEXT)",
            "CREATE TABLE transcripts (id TEXT, meeting_id TEXT, transcript TEXT, timestamp TEXT, \
             audio_start_time REAL, audio_end_time REAL, speaker TEXT)",
            "CREATE TABLE transcript_chunks (meeting_id TEXT, transcript_text TEXT)",
            "CREATE TABLE summary_processes (meeting_id TEXT, status TEXT, result TEXT, error TEXT)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        pool
    }

    #[tokio::test]
    async fn list_and_transcript_and_search_roundtrip() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1', 'Standup', '2026-07-01', '2026-07-01')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO transcripts VALUES ('t1', 'm1', 'We shipped the roadmap', '[00:05]', 5.0, 9.0, 'speaker_1')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let result_json = serde_json::json!({"markdown": "## Notes\n- done"}).to_string();
        sqlx::query("INSERT INTO summary_processes VALUES ('m1', 'completed', ?, NULL)")
            .bind(&result_json)
            .execute(&pool)
            .await
            .unwrap();

        let listing = list_meetings(&pool, 10).await.unwrap();
        assert!(listing.contains("Standup"));
        assert!(listing.contains("📝 summary"));

        let transcript = get_transcript(&pool, "m1").await.unwrap();
        assert!(transcript.contains("[00:05] speaker_1: We shipped the roadmap"));

        let summary = get_summary(&pool, "m1").await.unwrap();
        assert!(summary.contains("- done"));

        let found = search_transcripts(&pool, "ROADMAP", 5).await.unwrap();
        assert!(found.contains("`m1`"));

        let missing = get_transcript(&pool, "nope").await.unwrap();
        assert!(missing.contains("No meeting found"));
    }

    #[tokio::test]
    async fn list_and_search_hide_trashed_meetings() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1', 'Kept', '2026-07-01', '2026-07-01')")
            .execute(&pool).await.unwrap();
        // m2 is trashed (deleted_at set).
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at, deleted_at) VALUES ('m2', 'Trashed', '2026-07-02', '2026-07-02', '2026-07-03 00:00:00')")
            .execute(&pool).await.unwrap();
        for (tid, mid) in [("t1", "m1"), ("t2", "m2")] {
            sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, speaker) VALUES (?, ?, 'shared keyword alpha', '[00:00]', 0.0, 1.0, NULL)")
                .bind(tid).bind(mid).execute(&pool).await.unwrap();
        }

        let listing = list_meetings(&pool, 10).await.unwrap();
        assert!(listing.contains("Kept"));
        assert!(!listing.contains("Trashed"), "trashed meeting hidden from list_meetings");

        let found = search_transcripts(&pool, "alpha", 10).await.unwrap();
        assert!(found.contains("`m1`"));
        assert!(!found.contains("`m2`"), "trashed meeting hidden from MCP search");
    }

    /// A DB opened via `--db` that predates the soft-delete migration (no
    /// `meetings.deleted_at`) must still work — the filter is silently skipped.
    #[tokio::test]
    async fn list_and_search_tolerate_missing_soft_delete_column() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        for ddl in [
            "CREATE TABLE meetings (id TEXT PRIMARY KEY, title TEXT, created_at TEXT, updated_at TEXT)",
            "CREATE TABLE transcripts (id TEXT, meeting_id TEXT, transcript TEXT, timestamp TEXT, \
             audio_start_time REAL, audio_end_time REAL, speaker TEXT)",
            "CREATE TABLE transcript_chunks (meeting_id TEXT, transcript_text TEXT)",
            "CREATE TABLE summary_processes (meeting_id TEXT, status TEXT, result TEXT, error TEXT)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        assert!(!meetings_has_soft_delete(&pool).await);

        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1','Legacy','2026-07-01','2026-07-01')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, speaker) VALUES ('t1','m1','legacy keyword beta','[00:00]',0.0,1.0,NULL)")
            .execute(&pool).await.unwrap();

        assert!(list_meetings(&pool, 10).await.unwrap().contains("Legacy"));
        assert!(search_transcripts(&pool, "beta", 10).await.unwrap().contains("`m1`"));
    }
}
