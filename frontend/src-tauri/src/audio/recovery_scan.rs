//! Filesystem-based crash recovery, independent of the webview/IndexedDB path.
//!
//! Every recording writes `metadata.json` (with `status: "recording"`) and, after each
//! segment, an atomically-written `transcripts.json` — plus `.checkpoints/*.mp4` audio.
//! Nothing read those back before: recovery only consulted webview IndexedDB, so a fully
//! recorded meeting sitting complete on disk was invisible if the webview never journaled
//! (reload lost currentMeetingId, IndexedDB disabled/cleared, different profile).
//!
//! This module scans the recordings directory for folders still marked `"recording"`,
//! dedups against meetings already in SQLite, and offers a one-click import of the
//! on-disk transcript + checkpoint audio.

use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Runtime};

use crate::database::repositories::meeting::MeetingsRepository;
use crate::database::repositories::transcript::TranscriptsRepository;
use crate::state::AppState;

/// Lightweight summary of an interrupted (never-finalized) recording folder found on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptedRecording {
    pub folder_path: String,
    pub title: String,
    pub created_at: String,
    pub segment_count: usize,
    pub has_checkpoints: bool,
    pub has_audio: bool,
}

/// Read `metadata.json` as a tolerant `Value` — the writer struct may drift across
/// versions (e.g. retranscription adds a `source` field), so we never deserialize into a
/// strict struct that would reject the other writer's output.
fn read_metadata_value(folder: &Path) -> Option<serde_json::Value> {
    let content = std::fs::read_to_string(folder.join("metadata.json")).ok()?;
    serde_json::from_str(&content).ok()
}

/// Derive a display title from the folder name by stripping the trailing
/// `_YYYY-MM-DD_HH-MM` timestamp that `create_meeting_folder` appends. Only used as a
/// fallback when `metadata.json` has no `meeting_name`.
fn title_from_folder_name(folder: &Path) -> String {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"_\d{4}-\d{2}-\d{2}_\d{2}-\d{2}$").unwrap());
    let name = folder
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Recovered Meeting");
    let stripped = RE.replace(name, "");
    let cleaned = stripped.trim();
    if cleaned.is_empty() {
        "Recovered Meeting".to_string()
    } else {
        cleaned.replace('_', " ")
    }
}

/// Map one transcript-segment JSON object to `(sequence_id, segment)`, tolerant of BOTH
/// on-disk writer shapes (recording_saver: `display_time`, no `timestamp`; common.rs:
/// `timestamp`, no `display_time`). Always carries `speaker` through (the audit's
/// "recovered segments drop speaker" fix at the disk layer). Returns None without an `id`.
fn segment_from_value(seg: &serde_json::Value) -> Option<(u64, crate::api::api::TranscriptSegment)> {
    let id = seg.get("id").and_then(|v| v.as_str())?.to_string();
    let text = seg.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
    // Prefer an explicit timestamp; fall back to display_time; else empty.
    let timestamp = seg
        .get("timestamp")
        .and_then(|v| v.as_str())
        .or_else(|| seg.get("display_time").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let audio_start_time = seg.get("audio_start_time").and_then(|v| v.as_f64());
    let audio_end_time = seg.get("audio_end_time").and_then(|v| v.as_f64());
    let duration = seg.get("duration").and_then(|v| v.as_f64());
    let speaker = seg
        .get("speaker")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let sequence_id = seg.get("sequence_id").and_then(|v| v.as_u64()).unwrap_or(0);
    Some((
        sequence_id,
        crate::api::api::TranscriptSegment {
            id,
            text,
            timestamp,
            audio_start_time,
            audio_end_time,
            duration,
            speaker,
        },
    ))
}

/// Read the finalized `transcripts.json` into `api::TranscriptSegment`s, sorted by
/// `sequence_id`. Written once at finalize (and by explicit rewrites).
pub fn read_transcripts_json(folder: &Path) -> Vec<crate::api::api::TranscriptSegment> {
    let content = match std::fs::read_to_string(folder.join("transcripts.json")) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let segments = match value.get("segments").and_then(|s| s.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };

    let mut out: Vec<(u64, crate::api::api::TranscriptSegment)> =
        segments.iter().filter_map(segment_from_value).collect();
    out.sort_by_key(|(seq, _)| *seq);
    out.into_iter().map(|(_, s)| s).collect()
}

/// Read the append-only `transcripts.jsonl` crash-recovery log: one segment JSON object
/// per line, folded by `sequence_id` (last append wins, so an updated segment supersedes
/// an earlier one), sorted by `sequence_id`. This is the durable incremental source
/// written per segment; the pretty `transcripts.json` is written only once at finalize.
/// A torn/partial last line (crash mid-write) is skipped, not fatal.
pub fn read_transcripts_jsonl(folder: &Path) -> Vec<crate::api::api::TranscriptSegment> {
    let content = match std::fs::read_to_string(folder.join("transcripts.jsonl")) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut by_seq: std::collections::BTreeMap<u64, crate::api::api::TranscriptSegment> =
        std::collections::BTreeMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some((seq, seg)) = segment_from_value(&value) {
                by_seq.insert(seq, seg); // last write for a sequence_id wins
            }
        }
    }
    by_seq.into_values().collect()
}

/// Preferred recovery read: the durable append-only `transcripts.jsonl` when present and
/// non-empty (recordings written with the jsonl saver), else the finalized
/// `transcripts.json` (legacy folders, or a folder finalized before this change).
pub fn read_transcripts_for_recovery(folder: &Path) -> Vec<crate::api::api::TranscriptSegment> {
    if folder.join("transcripts.jsonl").exists() {
        let segs = read_transcripts_jsonl(folder);
        if !segs.is_empty() {
            return segs;
        }
    }
    read_transcripts_json(folder)
}

/// True if the folder has any `.checkpoints/*.mp4` audio chunk.
fn folder_has_checkpoints(folder: &Path) -> bool {
    std::fs::read_dir(folder.join(".checkpoints"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .any(|e| e.path().extension().and_then(|s| s.to_str()) == Some("mp4"))
        })
        .unwrap_or(false)
}

/// Canonicalized path string for dedup comparison; falls back to the lexical path when
/// the target no longer exists (e.g. a deleted meeting's folder_path in SQLite).
fn normalize_path(p: &Path) -> String {
    std::fs::canonicalize(p)
        .map(|c| c.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string_lossy().to_string())
}

/// Core (pure, testable) scan: find interrupted-recording folders under `roots`,
/// excluding any whose normalized path is already in `known` (imported to SQLite).
fn scan_roots(roots: &[PathBuf], known: &HashSet<String>) -> Vec<InterruptedRecording> {
    let mut out: Vec<InterruptedRecording> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for root in roots {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let folder = entry.path();
            if !folder.is_dir() {
                continue;
            }
            let norm = normalize_path(&folder);
            if !seen.insert(norm.clone()) {
                continue; // same folder reached via two roots
            }
            if known.contains(&norm) {
                continue; // already saved to SQLite
            }
            let meta = match read_metadata_value(&folder) {
                Some(m) => m,
                None => continue,
            };
            let status = meta.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if status != "recording" {
                continue; // completed / error / recovered — not interrupted
            }
            let segments = read_transcripts_for_recovery(&folder);
            let has_checkpoints = folder_has_checkpoints(&folder);
            if segments.is_empty() && !has_checkpoints {
                continue; // nothing to recover
            }
            let title = meta
                .get("meeting_name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| title_from_folder_name(&folder));
            let created_at = meta
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            out.push(InterruptedRecording {
                folder_path: folder.to_string_lossy().to_string(),
                title,
                created_at,
                segment_count: segments.len(),
                has_checkpoints,
                has_audio: folder.join("audio.mp4").exists(),
            });
        }
    }
    // newest first
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out
}

/// Mark a folder as recovered so subsequent scans skip it even independent of the SQLite
/// dedup. Read-modify-write of `metadata.json` (atomic temp+rename), mirroring the writer.
fn mark_folder_recovered(folder: &Path, meeting_id: &str) -> Result<()> {
    let path = folder.join("metadata.json");
    let mut value: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = value.as_object_mut() {
        obj.insert("status".to_string(), serde_json::json!("recovered"));
        obj.insert(
            "recovered_at".to_string(),
            serde_json::json!(chrono::Utc::now().to_rfc3339()),
        );
        obj.insert("meeting_id".to_string(), serde_json::json!(meeting_id));
    }
    let temp = folder.join(".metadata.json.tmp");
    std::fs::write(&temp, serde_json::to_string_pretty(&value)?)?;
    std::fs::rename(&temp, &path)?;
    Ok(())
}

/// Roots that recordings can land in. Recordings always use the default recordings
/// folder (recording_saver::initialize_meeting_folder), regardless of any save-folder
/// preference, so that single root is the correct place to scan.
fn recording_roots() -> Vec<PathBuf> {
    vec![super::recording_preferences::get_default_recordings_folder()]
}

fn known_folder_set(paths: &[String]) -> HashSet<String> {
    paths.iter().map(|s| normalize_path(Path::new(s))).collect()
}

/// Read-only scan for interrupted recordings not yet in SQLite. Safe to ship/call before
/// any UI exists — returns an empty list on a clean machine.
#[tauri::command]
pub async fn scan_interrupted_recordings(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<InterruptedRecording>, String> {
    let known = MeetingsRepository::list_folder_paths(state.db_manager.pool())
        .await
        .map_err(|e| format!("Failed to list existing meeting folders: {}", e))?;
    let known_set = known_folder_set(&known);
    Ok(scan_roots(&recording_roots(), &known_set))
}

/// Import an interrupted recording folder into SQLite (transcript + merged checkpoint
/// audio), then mark the folder `recovered`. Idempotent: a second import is a no-op error.
#[tauri::command]
pub async fn import_interrupted_recording<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_folder: String,
) -> Result<serde_json::Value, String> {
    let folder = PathBuf::from(&meeting_folder);

    // Re-guard dedup (prevents a double-click / concurrent scan from double-importing).
    let known = MeetingsRepository::list_folder_paths(state.db_manager.pool())
        .await
        .map_err(|e| format!("Failed to list existing meeting folders: {}", e))?;
    if known_folder_set(&known).contains(&normalize_path(&folder)) {
        return Err("This recording has already been imported".to_string());
    }

    let segments = read_transcripts_for_recovery(&folder);
    let has_checkpoints = folder_has_checkpoints(&folder);
    if segments.is_empty() && !has_checkpoints {
        return Err("Nothing to recover in this folder".to_string());
    }

    let meta = read_metadata_value(&folder);
    let title = meta
        .as_ref()
        .and_then(|m| m.get("meeting_name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| title_from_folder_name(&folder));

    // Persist to SQLite: creates the meeting row (folder_path set) and inserts every
    // segment with speaker preserved, in one transaction.
    let meeting_id = TranscriptsRepository::save_transcript(
        state.db_manager.pool(),
        &title,
        &segments,
        Some(meeting_folder.clone()),
    )
    .await
    .map_err(|e| format!("Failed to save recovered transcript: {}", e))?;

    // Merge checkpoint audio (non-fatal — transcripts are the priority).
    let audio = if has_checkpoints {
        match crate::audio::incremental_saver::recover_audio_from_checkpoints(
            meeting_folder.clone(),
            48000,
        )
        .await
        {
            Ok(status) => serde_json::to_value(status).unwrap_or(serde_json::Value::Null),
            Err(e) => serde_json::json!({ "status": "failed", "message": e }),
        }
    } else {
        serde_json::json!({ "status": "none", "message": "No audio checkpoints" })
    };

    // Mark recovered so it won't re-appear in scans.
    if let Err(e) = mark_folder_recovered(&folder, &meeting_id) {
        log::warn!("Imported {} but failed to mark folder recovered: {}", meeting_id, e);
    }

    Ok(serde_json::json!({
        "meeting_id": meeting_id,
        "segment_count": segments.len(),
        "audio": audio,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::test_support::migrated_pool;

    fn write_recording_folder(root: &Path, name: &str, status: &str, with_segment: bool) -> PathBuf {
        let folder = root.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("metadata.json"),
            serde_json::json!({
                "version": "1.0",
                "meeting_name": "Team Sync",
                "created_at": "2026-07-12T09:00:00Z",
                "status": status,
                "source": "recording"  // unknown-to-struct extra field must be tolerated
            })
            .to_string(),
        )
        .unwrap();
        if with_segment {
            // recording_saver-style shape: display_time + confidence, no `timestamp`.
            std::fs::write(
                folder.join("transcripts.json"),
                serde_json::json!({
                    "version": "1.0",
                    "segments": [{
                        "id": "s2",
                        "text": "second",
                        "audio_start_time": 3.0,
                        "audio_end_time": 6.0,
                        "duration": 3.0,
                        "display_time": "[00:03]",
                        "confidence": 0.9,
                        "sequence_id": 2,
                        "speaker": "mic"
                    }, {
                        "id": "s1",
                        "text": "first",
                        "audio_start_time": 0.0,
                        "audio_end_time": 3.0,
                        "duration": 3.0,
                        "display_time": "[00:00]",
                        "confidence": 0.9,
                        "sequence_id": 1,
                        "speaker": "system"
                    }]
                })
                .to_string(),
            )
            .unwrap();
        }
        folder
    }

    #[test]
    fn reader_orders_by_sequence_and_preserves_speaker() {
        let dir = std::env::temp_dir().join("murmur_recovery_reader_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let folder = write_recording_folder(&dir, "meeting_2026-07-12_09-00", "recording", true);

        let segs = read_transcripts_json(&folder);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].id, "s1", "sorted by sequence_id ascending");
        assert_eq!(segs[0].speaker.as_deref(), Some("system"));
        assert_eq!(segs[1].speaker.as_deref(), Some("mic"), "speaker preserved");
        // display_time used as the timestamp fallback for the recording_saver shape.
        assert_eq!(segs[0].timestamp, "[00:00]");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn jsonl_folds_by_sequence_id_last_wins_and_skips_torn_line() {
        let dir = std::env::temp_dir().join("murmur_recovery_jsonl_fold_test");
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join("rec");
        std::fs::create_dir_all(&folder).unwrap();
        // Out-of-order appends, seq 2 updated (last wins), and a torn final line (crash).
        let jsonl = concat!(
            "{\"id\":\"b\",\"text\":\"second\",\"sequence_id\":2,\"speaker\":\"mic\"}\n",
            "{\"id\":\"a\",\"text\":\"first\",\"sequence_id\":1,\"speaker\":\"sys\"}\n",
            "{\"id\":\"b\",\"text\":\"second-edited\",\"sequence_id\":2,\"speaker\":\"mic\"}\n",
            "{\"id\":\"c\",\"text\":\"tor",
        );
        std::fs::write(folder.join("transcripts.jsonl"), jsonl).unwrap();

        let segs = read_transcripts_jsonl(&folder);
        assert_eq!(segs.len(), 2, "seq 2 folded to one; torn line skipped");
        assert_eq!(segs[0].id, "a", "sorted by sequence_id");
        assert_eq!(segs[1].id, "b");
        assert_eq!(segs[1].text, "second-edited", "last write for a sequence_id wins");
        assert_eq!(segs[0].speaker.as_deref(), Some("sys"), "speaker preserved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovery_prefers_jsonl_then_falls_back_to_json() {
        let dir = std::env::temp_dir().join("murmur_recovery_prefers_jsonl_test");
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join("rec");
        std::fs::create_dir_all(&folder).unwrap();
        // json holds a STALE segment; jsonl is the newer durable log.
        std::fs::write(
            folder.join("transcripts.json"),
            r#"{"segments":[{"id":"old","text":"stale","sequence_id":1}]}"#,
        )
        .unwrap();
        std::fs::write(
            folder.join("transcripts.jsonl"),
            "{\"id\":\"new\",\"text\":\"fresh\",\"sequence_id\":1}\n",
        )
        .unwrap();

        let segs = read_transcripts_for_recovery(&folder);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].id, "new", "jsonl (durable) preferred over stale json");

        // Without a jsonl, recovery falls back to the finalized json.
        std::fs::remove_file(folder.join("transcripts.jsonl")).unwrap();
        let segs = read_transcripts_for_recovery(&folder);
        assert_eq!(segs[0].id, "old", "falls back to json when no jsonl");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reader_parses_common_rs_shape_with_timestamp() {
        let dir = std::env::temp_dir().join("murmur_recovery_reader_common_test");
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join("m_2026-07-12_09-00");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("transcripts.json"),
            serde_json::json!({
                "segments": [{
                    "id": "a", "text": "hi", "timestamp": "14:30:05",
                    "audio_start_time": 0.0, "audio_end_time": 1.0, "duration": 1.0,
                    "speaker": "mic", "sequence_id": 1
                }]
            })
            .to_string(),
        )
        .unwrap();
        let segs = read_transcripts_json(&folder);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].timestamp, "14:30:05");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reader_returns_empty_on_missing_file() {
        let dir = std::env::temp_dir().join("murmur_recovery_missing_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_transcripts_json(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_selects_recording_and_dedups_known() {
        let dir = std::env::temp_dir().join("murmur_recovery_scan_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rec = write_recording_folder(&dir, "rec_2026-07-12_09-00", "recording", true);
        let _done = write_recording_folder(&dir, "done_2026-07-12_10-00", "completed", true);

        // No known folders -> the recording folder is returned, the completed one is not.
        let found = scan_roots(&[dir.clone()], &HashSet::new());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].segment_count, 2);
        assert!(found[0].folder_path.contains("rec_2026-07-12_09-00"));
        assert_eq!(found[0].title, "Team Sync");

        // Mark the recording folder as known -> deduped out.
        let known = known_folder_set(&[rec.to_string_lossy().to_string()]);
        assert!(scan_roots(&[dir.clone()], &known).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn title_falls_back_to_folder_name_without_metadata_name() {
        let folder = Path::new("/tmp/Weekly_Planning_2026-07-12_09-30");
        assert_eq!(title_from_folder_name(folder), "Weekly Planning");
    }

    #[tokio::test]
    async fn scan_command_style_dedup_against_sqlite() {
        // Import path shares list_folder_paths with the scan; assert a folder_path present
        // in SQLite is excluded from a scan of the same root.
        let dir = std::env::temp_dir().join("murmur_recovery_sqlite_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let rec = write_recording_folder(&dir, "rec_2026-07-12_11-00", "recording", true);

        let pool = migrated_pool().await;
        // Insert a meeting whose folder_path is the recording folder.
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at, folder_path) VALUES ('m1','T',datetime('now'),datetime('now'),?)")
            .bind(rec.to_string_lossy().to_string())
            .execute(&pool)
            .await
            .unwrap();
        let known = MeetingsRepository::list_folder_paths(&pool).await.unwrap();
        assert_eq!(known.len(), 1);
        let found = scan_roots(&[dir.clone()], &known_folder_set(&known));
        assert!(found.is_empty(), "folder already in SQLite is deduped out");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
