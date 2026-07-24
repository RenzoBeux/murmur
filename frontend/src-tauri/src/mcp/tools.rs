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

/// Turn free-text user input into a safe FTS5 MATCH query. Each whitespace token
/// is reduced to its alphanumeric characters (diacritics kept — `is_alphanumeric`
/// is unicode-aware) and wrapped as a quoted prefix term (`"tok"*`). Quoting makes
/// any FTS operators/punctuation in the input inert (no syntax errors, no query
/// injection); the `*` gives search-as-you-type prefix matching. Returns `""`
/// when nothing usable remains — callers must treat that as "no results".
pub fn to_fts_match_query(raw: &str) -> String {
    raw.split_whitespace()
        .filter_map(|token| {
            let cleaned: String = token.chars().filter(|c| c.is_alphanumeric()).collect();
            if cleaned.is_empty() {
                None
            } else {
                Some(format!("\"{}\"*", cleaned))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether the FTS5 `search_index` table exists in the opened DB. MCP may open a
/// pre-FTS database via `--db`; there, search gracefully falls back to LIKE.
pub async fn search_index_exists(pool: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'search_index'",
    )
    .fetch_one(pool)
    .await
    .map(|count| count > 0)
    .unwrap_or(false)
}

/// Whether the `meeting_tags` table exists. A pre-tags DB opened via `--db`
/// simply ignores any `tag` scoping argument.
async fn meeting_tags_exists(pool: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meeting_tags'",
    )
    .fetch_one(pool)
    .await
    .map(|count| count > 0)
    .unwrap_or(false)
}

/// `EXISTS (…)` fragment scoping a `meetings m` query to a single tag. The
/// fragment is a compile-time literal (the tag value is bound), so it is
/// injection-safe. Returns None when there's no usable tag or the table is
/// absent — the caller then binds nothing extra.
async fn tag_scope_clause<'a>(pool: &SqlitePool, tag: Option<&'a str>) -> Option<&'a str> {
    let tag = tag.map(str::trim).filter(|t| !t.is_empty())?;
    if meeting_tags_exists(pool).await {
        Some(tag)
    } else {
        None
    }
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

pub async fn list_meetings(pool: &SqlitePool, limit: i64, tag: Option<&str>) -> Result<String> {
    // Build the WHERE incrementally: hide trashed meetings when the schema
    // supports it, and optionally scope to a tag.
    let mut conds: Vec<&str> = Vec::new();
    if meetings_has_soft_delete(pool).await {
        conds.push("m.deleted_at IS NULL");
    }
    let scoped_tag = tag_scope_clause(pool, tag).await;
    if scoped_tag.is_some() {
        conds.push("EXISTS (SELECT 1 FROM meeting_tags mt WHERE mt.meeting_id = m.id AND mt.tag = ?)");
    }
    let where_clause = if conds.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conds.join(" AND "))
    };
    let sql = format!(
        "SELECT m.id, m.title, m.created_at, sp.status AS summary_status \
         FROM meetings m \
         LEFT JOIN summary_processes sp ON sp.meeting_id = m.id \
         {where_clause} \
         ORDER BY m.created_at DESC \
         LIMIT ?"
    );
    let mut q = sqlx::query(&sql);
    if let Some(t) = scoped_tag {
        q = q.bind(t);
    }
    let rows = q.bind(limit.clamp(1, 500)).fetch_all(pool).await?;

    if rows.is_empty() {
        return Ok(match scoped_tag {
            Some(t) => format!("No meetings found with tag '{t}'."),
            None => "No meetings found in the Murmur database yet.".to_string(),
        });
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

/// Whether the `meeting_attachments` table exists. A pre-attachments DB opened
/// via `--db` simply renders no attachments section.
async fn meeting_attachments_exists(pool: &SqlitePool) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meeting_attachments'",
    )
    .fetch_one(pool)
    .await
    .map(|count| count > 0)
    .unwrap_or(false)
}

/// Markdown list of a meeting's attachments, or None when there are none (or
/// the table doesn't exist in this DB).
async fn render_attachments(pool: &SqlitePool, meeting_id: &str) -> Option<String> {
    if !meeting_attachments_exists(pool).await {
        return None;
    }
    let rows = sqlx::query(
        "SELECT file_name, mime_type, size_bytes FROM meeting_attachments \
         WHERE meeting_id = ? ORDER BY created_at ASC, id ASC",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await
    .ok()?;
    if rows.is_empty() {
        return None;
    }
    let mut out = String::from("## Attachments\n");
    for row in rows {
        let file_name: String = row.try_get("file_name").unwrap_or_default();
        let mime_type: String = row.try_get("mime_type").unwrap_or_default();
        let size_bytes: i64 = row.try_get("size_bytes").unwrap_or_default();
        out.push_str(&format!("- {file_name} ({mime_type}, {size_bytes} bytes)\n"));
    }
    Some(out)
}

pub async fn get_meeting(pool: &SqlitePool, meeting_id: &str) -> Result<String> {
    let summary = get_summary(pool, meeting_id).await?;
    let transcript = get_transcript(pool, meeting_id).await?;
    match render_attachments(pool, meeting_id).await {
        Some(attachments) => Ok(format!(
            "{summary}\n\n---\n\n{attachments}\n---\n\n{transcript}"
        )),
        None => Ok(format!("{summary}\n\n---\n\n{transcript}")),
    }
}

pub async fn search_transcripts(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
    tag: Option<&str>,
) -> Result<String> {
    let query = query.trim();
    if query.is_empty() {
        return Ok("Please provide a non-empty search query.".to_string());
    }
    let limit = limit.clamp(1, 100);
    let scoped_tag = tag_scope_clause(pool, tag).await;

    // Prefer the FTS5 index (all content sources, diacritic-insensitive); fall
    // back to LIKE over transcripts/chunks for a pre-FTS DB opened via `--db`.
    let results: Vec<(String, String, String)> = if search_index_exists(pool).await {
        fts_search_rows(pool, query, limit, scoped_tag).await?
    } else {
        like_search_rows(pool, query, limit, scoped_tag).await?
    };

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

/// FTS5 search across every indexed source (transcripts, chunks, summaries,
/// notes, chat), deduped to one hit per meeting (best `rank` first) and
/// excluding trashed meetings. A DB that has `search_index` is necessarily
/// post-soft-delete, so `meetings.deleted_at` is guaranteed to exist.
async fn fts_search_rows(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
    tag: Option<&str>,
) -> Result<Vec<(String, String, String)>> {
    let match_query = to_fts_match_query(query);
    if match_query.is_empty() {
        return Ok(Vec::new());
    }
    let tag_cond = if tag.is_some() {
        " AND EXISTS (SELECT 1 FROM meeting_tags mt WHERE mt.meeting_id = m.id AND mt.tag = ?)"
    } else {
        ""
    };
    // Group to one best-ranked row per meeting IN SQL (rank/snippet computed in the
    // innermost MATCH query; ROW_NUMBER keeps each meeting's best row) so LIMIT
    // counts distinct meetings, not per-segment rows.
    let sql = format!(
        "SELECT mid, title, ctx FROM ( \
             SELECT mid, title, ctx, r, \
                    ROW_NUMBER() OVER (PARTITION BY mid ORDER BY r) AS rn \
             FROM ( \
                 SELECT search_index.meeting_id AS mid, m.title AS title, \
                        snippet(search_index, 3, '', '', '…', 12) AS ctx, \
                        rank AS r \
                 FROM search_index \
                 JOIN meetings m ON m.id = search_index.meeting_id \
                 WHERE search_index MATCH ? AND m.deleted_at IS NULL{tag_cond} \
             ) \
         ) WHERE rn = 1 ORDER BY r LIMIT ?"
    );
    let mut q = sqlx::query(&sql).bind(&match_query);
    if let Some(t) = tag {
        q = q.bind(t);
    }
    let rows = q.bind(limit).fetch_all(pool).await?;

    let mut results: Vec<(String, String, String)> = Vec::new();
    for row in &rows {
        let id: String = row.try_get("mid")?;
        let title: String = row.try_get("title")?;
        let ctx: String = row.try_get("ctx")?;
        results.push((id, title, ctx));
    }
    Ok(results)
}

/// Legacy LIKE search over transcripts + legacy chunks, used only when the FTS5
/// `search_index` is absent (a pre-FTS DB opened via `--db`). Keeps the graceful
/// soft-delete filter (`sd_and` is a compile-time literal → injection-safe).
async fn like_search_rows(
    pool: &SqlitePool,
    query: &str,
    limit: i64,
    tag: Option<&str>,
) -> Result<Vec<(String, String, String)>> {
    let like = format!("%{}%", query.to_lowercase());
    let sd_and = if meetings_has_soft_delete(pool).await {
        "m.deleted_at IS NULL AND "
    } else {
        ""
    };
    let tag_cond = if tag.is_some() {
        " AND EXISTS (SELECT 1 FROM meeting_tags mt WHERE mt.meeting_id = m.id AND mt.tag = ?)"
    } else {
        ""
    };

    let mut results: Vec<(String, String, String)> = Vec::new(); // (id, title, context)
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let seg_sql = format!(
        "SELECT m.id, m.title, t.transcript \
         FROM meetings m \
         JOIN transcripts t ON t.meeting_id = m.id \
         WHERE {sd_and}LOWER(t.transcript) LIKE ?{tag_cond} \
         ORDER BY m.created_at DESC \
         LIMIT ?"
    );
    let mut seg_q = sqlx::query(&seg_sql).bind(&like);
    if let Some(t) = tag {
        seg_q = seg_q.bind(t);
    }
    let seg_rows = seg_q.bind(limit).fetch_all(pool).await?;
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
             WHERE {sd_and}LOWER(tc.transcript_text) LIKE ?{tag_cond} \
             ORDER BY m.created_at DESC \
             LIMIT ?"
        );
        let mut chunk_q = sqlx::query(&chunk_sql).bind(&like);
        if let Some(t) = tag {
            chunk_q = chunk_q.bind(t);
        }
        let chunk_rows = chunk_q.bind(limit).fetch_all(pool).await?;
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
    Ok(results)
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

        let listing = list_meetings(&pool, 10, None).await.unwrap();
        assert!(listing.contains("Standup"));
        assert!(listing.contains("📝 summary"));

        let transcript = get_transcript(&pool, "m1").await.unwrap();
        assert!(transcript.contains("[00:05] speaker_1: We shipped the roadmap"));

        let summary = get_summary(&pool, "m1").await.unwrap();
        assert!(summary.contains("- done"));

        let found = search_transcripts(&pool, "ROADMAP", 5, None).await.unwrap();
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

        let listing = list_meetings(&pool, 10, None).await.unwrap();
        assert!(listing.contains("Kept"));
        assert!(!listing.contains("Trashed"), "trashed meeting hidden from list_meetings");

        let found = search_transcripts(&pool, "alpha", 10, None).await.unwrap();
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

        assert!(list_meetings(&pool, 10, None).await.unwrap().contains("Legacy"));
        assert!(search_transcripts(&pool, "beta", 10, None).await.unwrap().contains("`m1`"));
    }

    /// When the FTS5 `search_index` exists, MCP search uses it: cross-source,
    /// diacritic-insensitive, and still excluding trashed meetings.
    #[tokio::test]
    async fn mcp_search_uses_fts_index_when_present() {
        use crate::database::repositories::meeting::MeetingsRepository;
        use crate::database::test_support::migrated_pool;

        let pool = migrated_pool().await;
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES ('m1','Budget','2026-07-01','2026-07-01')")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp) VALUES ('t1','m1','discutimos el café','[00:00]')")
            .execute(&pool).await.unwrap();

        assert!(search_index_exists(&pool).await, "migrated DB has the FTS index");
        // Diacritic-insensitive FTS match ("cafe" → "café").
        let found = search_transcripts(&pool, "cafe", 10, None).await.unwrap();
        assert!(found.contains("`m1`"), "FTS finds diacritic-folded match");

        // Trashed meetings are excluded from FTS search too.
        MeetingsRepository::delete_meeting(&pool, "m1").await.unwrap();
        let after = search_transcripts(&pool, "cafe", 10, None).await.unwrap();
        assert!(!after.contains("`m1`"), "trashed meeting excluded from FTS search");
    }

    #[tokio::test]
    async fn mcp_tag_scoping_filters_list_and_search() {
        use crate::database::repositories::meeting::MeetingsRepository;
        use crate::database::test_support::migrated_pool;

        let pool = migrated_pool().await;
        for (id, title) in [("m1", "Tagged"), ("m2", "Untagged")] {
            sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, ?, '2026-07-01','2026-07-01')")
                .bind(id).bind(title).execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp) VALUES (?, ?, 'shared keyword zeta', '[00:00]')")
                .bind(format!("t-{id}")).bind(id).execute(&pool).await.unwrap();
        }
        MeetingsRepository::add_tag(&pool, "m1", "work").await.unwrap();

        // list_meetings scoped to the tag returns only the tagged meeting.
        let listed = list_meetings(&pool, 10, Some("work")).await.unwrap();
        assert!(listed.contains("`m1`") && !listed.contains("`m2`"), "tag scopes list_meetings");

        // Search scoped to the tag returns only m1, though both transcripts match.
        let searched = search_transcripts(&pool, "zeta", 10, Some("work")).await.unwrap();
        assert!(searched.contains("`m1`") && !searched.contains("`m2`"), "tag scopes search");

        // An unused tag yields nothing.
        assert!(list_meetings(&pool, 10, Some("nope"))
            .await
            .unwrap()
            .contains("No meetings found with tag"));
    }
}
