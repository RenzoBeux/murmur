//! Markdown generation for export bundles.
//!
//! Every function here is pure, so the whole content layer is unit-testable
//! without a database, a Tauri handle, or a filesystem.
//!
//! Each emitted file carries its own YAML frontmatter: a `transcript.md`
//! pulled out of the zip and dropped into Obsidian should still know what
//! meeting it came from.

use serde::Deserialize;

use crate::database::models::{ProjectModel, Transcript};

/// How transcript lines are rendered. Both toggles are user-facing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TranscriptFormat {
    pub include_timestamps: bool,
    pub include_speakers: bool,
}

impl Default for TranscriptFormat {
    /// Hand-written rather than derived: the product default for speakers is
    /// **on**, which `#[derive(Default)]` would get wrong.
    fn default() -> Self {
        Self {
            include_timestamps: false,
            include_speakers: true,
        }
    }
}

/// Everything the frontmatter of a per-meeting file needs.
#[derive(Debug, Default, Clone)]
pub struct MeetingMeta<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub created_at_rfc3339: &'a str,
    pub project_name: Option<&'a str>,
    pub duration_seconds: Option<f64>,
    /// True when exporting a single meeting that currently sits in the trash.
    pub trashed: bool,
}

/// One line of the project README's meeting table.
#[derive(Debug, Clone)]
pub struct ReadmeRow {
    pub title: String,
    pub created_at_rfc3339: String,
    pub duration_seconds: Option<f64>,
    /// The allocated folder name, or None when the meeting contributed no
    /// files and therefore has no folder in the archive.
    pub folder: Option<String>,
}

/// Map a stored speaker tag to the label the UI shows. Mirrors
/// `speakerDisplayName` in `frontend/src/lib/speakerLabel.ts`.
pub fn speaker_display_name(tag: &str) -> String {
    match tag {
        "mic" => "You".to_string(),
        "system" => "Others".to_string(),
        other => match other.strip_prefix("speaker_") {
            Some(n) if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) => {
                format!("Speaker {n}")
            }
            _ => other.to_string(),
        },
    }
}

/// Render a meeting's transcript. `None` when there is nothing worth writing,
/// which is what keeps empty `transcript.md` files out of the archive.
pub fn build_transcript_markdown(
    meta: &MeetingMeta<'_>,
    transcripts: &[Transcript],
    fmt: TranscriptFormat,
) -> Option<String> {
    let mut body = String::new();

    for seg in transcripts {
        let text = seg.transcript.trim();
        if text.is_empty() {
            continue;
        }

        let mut line = String::new();

        if fmt.include_timestamps {
            if let Some(stamp) = segment_timestamp(seg) {
                line.push_str(&format!("[{stamp}] "));
            }
        }

        if fmt.include_speakers {
            if let Some(tag) = seg.speaker.as_deref().filter(|s| !s.trim().is_empty()) {
                line.push_str(&format!("**{}:** ", speaker_display_name(tag.trim())));
            }
        }

        line.push_str(text);
        body.push_str(&line);
        body.push_str("\n\n");
    }

    if body.trim().is_empty() {
        return None;
    }

    let mut out = frontmatter(meta, "transcript");
    out.push_str(&format!("# {}\n\n", meta.title));
    out.push_str("## Transcript\n\n");
    out.push_str(body.trim_end());
    out.push('\n');
    Some(out)
}

/// Render a meeting's summary from the raw `summary_processes.result` blob.
///
/// `None` when the meeting has no summary, when the blob does not parse, or
/// when it renders to nothing — the last case covers rows that hold only
/// BlockNote `summary_json` with an empty `markdown` key, which the shared
/// renderer cannot turn into text.
pub fn build_summary_markdown(meta: &MeetingMeta<'_>, summary_result: Option<&str>) -> Option<String> {
    let raw = summary_result?;
    let value = crate::mcp::tools::parse_summary(raw)?;
    let rendered = crate::mcp::tools::render_summary(&value);
    let rendered = rendered.trim();

    // `render_summary` returns this sentinel rather than an empty string.
    if rendered.is_empty() || rendered == "(Summary is empty.)" {
        return None;
    }

    let mut out = frontmatter(meta, "summary");
    out.push_str(&format!("# {}\n\n", meta.title));
    out.push_str("## Summary\n\n");
    out.push_str(rendered);
    out.push('\n');
    Some(out)
}

/// The project archive's `README.md`: what this export is, and an index into
/// the folders beside it.
///
/// Callers must build `rows` **after** folder names have been allocated, so
/// the links point at the deduped names that actually exist in the archive.
pub fn build_project_readme(
    project: &ProjectModel,
    rows: &[ReadmeRow],
    exported_at_rfc3339: &str,
    included: &[&str],
    fmt: TranscriptFormat,
) -> String {
    let mut out = String::new();

    out.push_str("---\n");
    out.push_str(&format!("project: {}\n", yaml_value(&project.name)));
    out.push_str(&format!("project_id: {}\n", yaml_value(&project.id)));
    out.push_str(&format!("exported_at: {exported_at_rfc3339}\n"));
    out.push_str(&format!("meetings: {}\n", rows.len()));
    out.push_str("---\n\n");

    out.push_str(&format!("# {}\n\n", project.name));

    if let Some(description) = project
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        out.push_str(description);
        out.push_str("\n\n");
    }

    out.push_str("## Meetings\n\n");
    out.push_str("| Date | Meeting | Duration | Folder |\n");
    out.push_str("| --- | --- | --- | --- |\n");
    for row in rows {
        let date = row.created_at_rfc3339.get(0..10).unwrap_or("—");
        let duration = row
            .duration_seconds
            .map(format_duration)
            .unwrap_or_else(|| "—".to_string());
        let folder = match &row.folder {
            Some(folder) => format!("[{}](./{}/)", escape_cell(folder), encode_path(folder)),
            None => "(no exported content)".to_string(),
        };
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            date,
            escape_cell(&row.title),
            duration,
            folder
        ));
    }

    out.push_str("\n## About this export\n\n");
    out.push_str(&format!("Exported from Murmur on {exported_at_rfc3339}.\n\n"));
    out.push_str(&format!("- Included: {}\n", included.join(", ")));
    out.push_str(&format!(
        "- Transcript formatting: speaker labels {}, timestamps {}\n",
        on_off(fmt.include_speakers),
        on_off(fmt.include_timestamps)
    ));
    out
}

/// YAML frontmatter shared by `transcript.md` and `summary.md`.
fn frontmatter(meta: &MeetingMeta<'_>, kind: &str) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("title: {}\n", yaml_value(meta.title)));
    out.push_str(&format!("date: {}\n", meta.created_at_rfc3339));
    out.push_str(&format!("meeting_id: {}\n", yaml_value(meta.id)));
    if let Some(project) = meta.project_name.map(str::trim).filter(|p| !p.is_empty()) {
        out.push_str(&format!("project: {}\n", yaml_value(project)));
    }
    if let Some(seconds) = meta.duration_seconds {
        out.push_str(&format!("duration: {}\n", format_duration(seconds)));
    }
    if meta.trashed {
        out.push_str("trashed: true\n");
    }
    out.push_str(&format!("type: {kind}\n"));
    out.push_str("---\n\n");
    out
}

/// A transcript segment's display timestamp: recording-relative when we have
/// it, else the stored wall-clock string.
///
/// `audio_start_time` is preferred because it is what the player seeks on and
/// what the rows are sorted by. The `timestamp` column is a bare local
/// "14:30:05" with no date or offset, and for imported audio it records the
/// import time rather than the meeting time.
fn segment_timestamp(seg: &Transcript) -> Option<String> {
    match seg.audio_start_time {
        Some(seconds) if seconds.is_finite() && seconds >= 0.0 => {
            Some(crate::mcp::tools::fmt_timestamp(Some(seconds)))
        }
        _ => {
            let stamp = seg.timestamp.trim();
            (!stamp.is_empty()).then(|| stamp.to_string())
        }
    }
}

/// Human-readable duration for frontmatter and the README table.
pub fn format_duration(seconds: f64) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "—".to_string();
    }
    let total = seconds.round() as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "< 1m".to_string()
    }
}

/// YAML-safe scalar: quote whenever the bare form could be misparsed.
pub fn yaml_value(s: &str) -> String {
    const RISKY_LEADING: &[char] = &[
        '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@',
        '`',
    ];
    const RISKY_WORDS: &[&str] = &[
        "true", "false", "null", "yes", "no", "on", "off", "y", "n", "~",
    ];

    let needs_quote = s.is_empty()
        || s.contains(':')
        || s.contains('#')
        || s.contains('"')
        || s.contains('\n')
        || s.contains('\r')
        || s.contains('\t')
        || s.starts_with(char::is_whitespace)
        || s.ends_with(char::is_whitespace)
        || s.starts_with(RISKY_LEADING)
        || RISKY_WORDS.iter().any(|w| s.eq_ignore_ascii_case(w))
        // A bare number would round-trip as a number, not a string.
        || s.parse::<f64>().is_ok();

    if needs_quote {
        format!(
            "\"{}\"",
            s.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t")
        )
    } else {
        s.to_string()
    }
}

/// Escape a value going into a markdown table cell.
fn escape_cell(s: &str) -> String {
    s.replace('|', "\\|").replace(['\n', '\r'], " ")
}

/// Percent-encode the characters that would break a markdown link target.
/// Folder names keep spaces and unicode, both of which need encoding here.
fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, speaker: Option<&str>, start: Option<f64>) -> Transcript {
        Transcript {
            id: "t1".to_string(),
            meeting_id: "m1".to_string(),
            transcript: text.to_string(),
            timestamp: "14:30:05".to_string(),
            summary: None,
            action_items: None,
            key_points: None,
            audio_start_time: start,
            audio_end_time: None,
            duration: None,
            speaker: speaker.map(str::to_string),
        }
    }

    fn meta() -> MeetingMeta<'static> {
        MeetingMeta {
            id: "meeting-1",
            title: "Weekly Sync",
            created_at_rfc3339: "2026-06-02T09:00:00+00:00",
            ..Default::default()
        }
    }

    #[test]
    fn transcript_omits_timestamps_by_default() {
        let md = build_transcript_markdown(
            &meta(),
            &[seg("hello", Some("mic"), Some(5.0))],
            TranscriptFormat::default(),
        )
        .expect("has content");
        assert!(md.contains("**You:** hello"));
        assert!(!md.contains("[00:05]"), "default leaves timestamps off");
    }

    #[test]
    fn transcript_includes_timestamps_when_enabled() {
        let md = build_transcript_markdown(
            &meta(),
            &[seg("hello", Some("mic"), Some(312.0))],
            TranscriptFormat {
                include_timestamps: true,
                include_speakers: true,
            },
        )
        .expect("has content");
        assert!(md.contains("[05:12] **You:** hello"), "got: {md}");
    }

    #[test]
    fn transcript_omits_speakers_when_disabled() {
        let md = build_transcript_markdown(
            &meta(),
            &[seg("hello", Some("mic"), Some(5.0))],
            TranscriptFormat {
                include_timestamps: false,
                include_speakers: false,
            },
        )
        .expect("has content");
        assert!(md.contains("\nhello"));
        assert!(!md.contains("**"), "no speaker prefix: {md}");
    }

    #[test]
    fn speaker_tags_map_to_ui_display_names() {
        assert_eq!(speaker_display_name("mic"), "You");
        assert_eq!(speaker_display_name("system"), "Others");
        assert_eq!(speaker_display_name("speaker_2"), "Speaker 2");
        assert_eq!(speaker_display_name("speaker_12"), "Speaker 12");
        // Custom labels pass through untouched.
        assert_eq!(speaker_display_name("Ana"), "Ana");
        assert_eq!(speaker_display_name("speaker_x"), "speaker_x");
        assert_eq!(speaker_display_name("speaker_"), "speaker_");
    }

    #[test]
    fn timestamp_falls_back_to_wall_clock_when_audio_start_is_missing() {
        let md = build_transcript_markdown(
            &meta(),
            &[seg("hello", None, None)],
            TranscriptFormat {
                include_timestamps: true,
                include_speakers: true,
            },
        )
        .expect("has content");
        assert!(md.contains("[14:30:05] hello"), "got: {md}");
    }

    #[test]
    fn transcript_is_none_when_every_segment_is_blank() {
        assert!(build_transcript_markdown(&meta(), &[], TranscriptFormat::default()).is_none());
        assert!(build_transcript_markdown(
            &meta(),
            &[seg("   ", Some("mic"), Some(1.0))],
            TranscriptFormat::default()
        )
        .is_none());
    }

    #[test]
    fn summary_is_none_when_absent_or_unrenderable() {
        for raw in [None, Some(""), Some("not json"), Some("{}"), Some("[]")] {
            assert!(
                build_summary_markdown(&meta(), raw).is_none(),
                "expected None for {raw:?}"
            );
        }
        assert!(build_summary_markdown(&meta(), Some(r#"{"markdown":"   "}"#)).is_none());
        // BlockNote-only blob: `content` is an array, which render_blocks skips.
        let blocknote = r#"{"markdown":"","summary_json":[{"type":"heading1","content":[{"type":"text","text":"Hi"}]}]}"#;
        assert!(build_summary_markdown(&meta(), Some(blocknote)).is_none());
    }

    #[test]
    fn summary_renders_current_and_legacy_shapes() {
        let current = build_summary_markdown(&meta(), Some(r###"{"markdown":"## Decisions\n- ship"}"###))
            .expect("current shape renders");
        assert!(current.contains("## Decisions"));
        assert!(current.contains("type: summary"));

        let legacy = r#"{"MeetingNotes":{"sections":[{"title":"Next Steps","blocks":[{"type":"bullet","content":"file the PR"}]}]}}"#;
        let rendered = build_summary_markdown(&meta(), Some(legacy)).expect("legacy shape renders");
        assert!(rendered.contains("Next Steps"));
        assert!(rendered.contains("- file the PR"));
    }

    #[test]
    fn frontmatter_omits_absent_optional_fields() {
        let md = build_transcript_markdown(
            &meta(),
            &[seg("hi", None, None)],
            TranscriptFormat::default(),
        )
        .unwrap();
        assert!(md.starts_with("---\n"));
        assert!(md.contains("title: Weekly Sync"));
        assert!(md.contains("meeting_id: meeting-1"));
        assert!(md.contains("type: transcript"));
        assert!(!md.contains("project:"));
        assert!(!md.contains("duration:"));
        assert!(!md.contains("trashed:"));
    }

    #[test]
    fn frontmatter_includes_optional_fields_when_present() {
        let meta = MeetingMeta {
            id: "meeting-1",
            title: "Weekly Sync",
            created_at_rfc3339: "2026-06-02T09:00:00+00:00",
            project_name: Some("Q3 Planning"),
            duration_seconds: Some(2880.0),
            trashed: true,
        };
        let md =
            build_transcript_markdown(&meta, &[seg("hi", None, None)], TranscriptFormat::default())
                .unwrap();
        assert!(md.contains("project: Q3 Planning"));
        assert!(md.contains("duration: 48m"));
        assert!(md.contains("trashed: true"));
    }

    #[test]
    fn yaml_value_quotes_risky_scalars() {
        assert_eq!(yaml_value("Weekly: Sync"), "\"Weekly: Sync\"");
        assert_eq!(yaml_value("-leading"), "\"-leading\"");
        assert_eq!(yaml_value("[brackets]"), "\"[brackets]\"");
        assert_eq!(yaml_value("true"), "\"true\"");
        assert_eq!(yaml_value("NULL"), "\"NULL\"");
        assert_eq!(yaml_value("42"), "\"42\"");
        assert_eq!(yaml_value("a\nb"), "\"a\\nb\"");
        assert_eq!(yaml_value(""), "\"\"");
        // Ordinary titles stay bare.
        assert_eq!(yaml_value("Weekly Sync"), "Weekly Sync");
    }

    #[test]
    fn duration_formats_readably() {
        assert_eq!(format_duration(2880.0), "48m");
        assert_eq!(format_duration(4324.0), "1h 12m");
        assert_eq!(format_duration(30.0), "< 1m");
        assert_eq!(format_duration(0.0), "—");
        assert_eq!(format_duration(f64::NAN), "—");
    }

    fn project() -> ProjectModel {
        ProjectModel {
            id: "project-1".to_string(),
            name: "Q3 Planning".to_string(),
            description: Some("Roadmap work".to_string()),
            color: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn readme_links_folders_and_escapes_pipes() {
        let rows = vec![
            ReadmeRow {
                title: "Kick|off".to_string(),
                created_at_rfc3339: "2026-06-02T09:00:00+00:00".to_string(),
                duration_seconds: Some(2880.0),
                folder: Some("2026-06-02 Kick_off".to_string()),
            },
            ReadmeRow {
                title: "Empty one".to_string(),
                created_at_rfc3339: "2026-06-09T09:00:00+00:00".to_string(),
                duration_seconds: None,
                folder: None,
            },
        ];
        let md = build_project_readme(
            &project(),
            &rows,
            "2026-08-19T14:03:11+00:00",
            &["transcripts", "summaries"],
            TranscriptFormat::default(),
        );
        assert!(md.contains("# Q3 Planning"));
        assert!(md.contains("Roadmap work"));
        assert!(md.contains("Kick\\|off"), "pipes escaped: {md}");
        assert!(md.contains("[2026-06-02 Kick_off](./2026-06-02%20Kick_off/)"), "got: {md}");
        assert!(md.contains("(no exported content)"));
        assert!(md.contains("speaker labels on, timestamps off"));
        assert!(md.contains("meetings: 2"));
    }
}
