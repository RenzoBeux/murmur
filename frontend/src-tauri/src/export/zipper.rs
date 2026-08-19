//! Streaming zip writer for export bundles.
//!
//! Deliberately knows nothing about Tauri or the database — it takes an owned
//! [`BundlePlan`] and a destination path. That is what lets the whole thing
//! run inside `spawn_blocking` (a `tauri::State` is not `'static`, so nothing
//! borrowed from it could cross the boundary) and what makes it testable with
//! nothing but a temp dir.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use log::warn;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Everything the writer needs, fully materialized ahead of time.
#[derive(Debug, Default)]
pub struct BundlePlan {
    pub entries: Vec<PlannedEntry>,
    /// Non-fatal problems collected while planning; carried through to the UI.
    pub warnings: Vec<String>,
    /// Meetings that contributed at least one entry.
    pub meetings: usize,
}

#[derive(Debug)]
pub enum PlannedEntry {
    /// Generated markdown, held in memory (bounded by one transcript).
    Text { path: String, body: String },
    /// A file on disk, streamed in rather than buffered.
    File {
        path: String,
        source: PathBuf,
        size: u64,
    },
}

impl PlannedEntry {
    pub fn path(&self) -> &str {
        match self {
            PlannedEntry::Text { path, .. } | PlannedEntry::File { path, .. } => path,
        }
    }
}

#[derive(Debug, Default)]
pub struct ZipStats {
    pub files_written: usize,
    pub bytes_written: u64,
    pub warnings: Vec<String>,
}

/// Write `plan` to `dest`, atomically.
///
/// The archive is built at `<dest>.part` and renamed into place only after
/// `finish()` and `sync_all()` succeed, so a failed or interrupted export
/// never leaves a truncated file where the user pointed the save dialog.
pub fn write_zip(plan: BundlePlan, dest: &Path) -> Result<ZipStats, String> {
    let part = part_path(dest);

    let stats = match write_entries(plan, &part) {
        Ok(stats) => stats,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };

    std::fs::rename(&part, dest).map_err(|e| {
        let _ = std::fs::remove_file(&part);
        format!("Failed to finalize {}: {e}", dest.display())
    })?;

    Ok(stats)
}

fn write_entries(plan: BundlePlan, part: &Path) -> Result<ZipStats, String> {
    let file = File::create(part)
        .map_err(|e| format!("Failed to create {}: {e}", part.display()))?;
    let mut zip = ZipWriter::new(file);

    let BundlePlan {
        entries,
        mut warnings,
        ..
    } = plan;

    let mut files_written = 0usize;
    let mut bytes_written = 0u64;

    for entry in entries {
        let path = entry.path().to_string();

        // Belt and braces: the planner builds every component through
        // `sanitize_zip_component`, but a malformed entry here would produce an
        // archive that writes outside its own directory on extraction.
        if !is_safe_entry_path(&path) {
            warn!("Skipping unsafe zip entry path: {path}");
            warnings.push(format!("Skipped an entry with an unsafe path: {path}"));
            continue;
        }

        match entry {
            PlannedEntry::Text { body, .. } => {
                zip.start_file(path.as_str(), SimpleFileOptions::default())
                    .map_err(|e| format!("Failed to add {path}: {e}"))?;
                zip.write_all(body.as_bytes())
                    .map_err(|e| format!("Failed to write {path}: {e}"))?;
                files_written += 1;
                bytes_written += body.len() as u64;
            }
            PlannedEntry::File { source, size, .. } => {
                let mut src = match File::open(&source) {
                    Ok(f) => f,
                    Err(e) => {
                        // A row whose file vanished must not sink the export.
                        warn!("Skipping missing export source {}: {e}", source.display());
                        warnings.push(format!(
                            "Skipped {} — the file is no longer on disk.",
                            file_label(&source, &path)
                        ));
                        continue;
                    }
                };

                let options = SimpleFileOptions::default().large_file(size >= u64::from(u32::MAX));
                zip.start_file(path.as_str(), options)
                    .map_err(|e| format!("Failed to add {path}: {e}"))?;
                let copied = io::copy(&mut src, &mut zip)
                    .map_err(|e| format!("Failed to write {path}: {e}"))?;
                files_written += 1;
                bytes_written += copied;
            }
        }
    }

    // `finish()` explicitly rather than relying on Drop, which swallows errors.
    let mut file = zip
        .finish()
        .map_err(|e| format!("Failed to finalize archive: {e}"))?;
    file.flush()
        .map_err(|e| format!("Failed to flush archive: {e}"))?;
    file.sync_all()
        .map_err(|e| format!("Failed to sync archive to disk: {e}"))?;

    Ok(ZipStats {
        files_written,
        bytes_written,
        warnings,
    })
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Zip entry names are always `/`-separated relative paths.
fn is_safe_entry_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.split('/').any(|part| part.is_empty() || part == "..")
        // A Windows drive prefix ("C:") would be honored by some extractors.
        && !path.contains(':')
}

fn file_label(source: &Path, entry_path: &str) -> String {
    source
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| entry_path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn text(path: &str, body: &str) -> PlannedEntry {
        PlannedEntry::Text {
            path: path.to_string(),
            body: body.to_string(),
        }
    }

    fn entry_names(archive: &Path) -> Vec<String> {
        let file = File::open(archive).expect("open archive");
        let mut zip = zip::ZipArchive::new(file).expect("valid archive");
        (0..zip.len())
            .map(|i| zip.by_index(i).expect("entry").name().to_string())
            .collect()
    }

    #[test]
    fn writes_expected_entry_paths_with_forward_slashes() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("bundle.zip");

        let plan = BundlePlan {
            entries: vec![
                text("README.md", "# Project"),
                text("2026-06-02 Kickoff/transcript.md", "hi"),
                text("2026-06-02 Kickoff/summary.md", "sum"),
            ],
            warnings: vec![],
            meetings: 1,
        };

        let stats = write_zip(plan, &dest).expect("write succeeds");
        assert_eq!(stats.files_written, 3);
        assert!(dest.exists());
        assert!(!dir.path().join("bundle.zip.part").exists(), "part cleaned up");

        let names = entry_names(&dest);
        assert_eq!(
            names,
            vec![
                "README.md",
                "2026-06-02 Kickoff/transcript.md",
                "2026-06-02 Kickoff/summary.md",
            ]
        );
        for name in &names {
            assert!(!name.contains('\\'), "no backslashes in {name}");
            assert!(!name.starts_with('/'), "not absolute: {name}");
            assert!(!name.contains(".."), "no traversal: {name}");
        }
    }

    #[test]
    fn streams_a_file_entry_byte_identically() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("slides.bin");
        let payload: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&source, &payload).unwrap();

        let dest = dir.path().join("bundle.zip");
        let plan = BundlePlan {
            entries: vec![PlannedEntry::File {
                path: "files/slides.bin".to_string(),
                source,
                size: payload.len() as u64,
            }],
            ..Default::default()
        };

        let stats = write_zip(plan, &dest).expect("write succeeds");
        assert_eq!(stats.bytes_written, payload.len() as u64);

        let mut zip = zip::ZipArchive::new(File::open(&dest).unwrap()).unwrap();
        let mut entry = zip.by_name("files/slides.bin").expect("entry present");
        let mut round_tripped = Vec::new();
        entry.read_to_end(&mut round_tripped).unwrap();
        assert_eq!(round_tripped, payload);
    }

    #[test]
    fn skips_a_missing_source_and_records_a_warning() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("bundle.zip");

        let plan = BundlePlan {
            entries: vec![
                text("transcript.md", "hi"),
                PlannedEntry::File {
                    path: "files/gone.png".to_string(),
                    source: dir.path().join("gone.png"),
                    size: 10,
                },
            ],
            ..Default::default()
        };

        let stats = write_zip(plan, &dest).expect("missing source is not fatal");
        assert_eq!(stats.files_written, 1);
        assert_eq!(stats.warnings.len(), 1);
        assert!(stats.warnings[0].contains("gone.png"), "{:?}", stats.warnings);
        // The archive is still valid and holds the entries that did resolve.
        assert_eq!(entry_names(&dest), vec!["transcript.md"]);
    }

    #[test]
    fn skips_unsafe_entry_paths() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("bundle.zip");

        let plan = BundlePlan {
            entries: vec![
                text("../escape.md", "no"),
                text("/absolute.md", "no"),
                text("C:/drive.md", "no"),
                text("windows\\sep.md", "no"),
                text("ok.md", "yes"),
            ],
            ..Default::default()
        };

        let stats = write_zip(plan, &dest).expect("write succeeds");
        assert_eq!(stats.files_written, 1);
        assert_eq!(stats.warnings.len(), 4);
        assert_eq!(entry_names(&dest), vec!["ok.md"]);
    }

    #[test]
    fn leaves_no_part_file_when_the_destination_is_unwritable() {
        let dir = tempfile::tempdir().unwrap();
        // A directory can never be replaced by a file rename.
        let dest = dir.path().join("occupied.zip");
        std::fs::create_dir(&dest).unwrap();

        let plan = BundlePlan {
            entries: vec![text("transcript.md", "hi")],
            ..Default::default()
        };

        assert!(write_zip(plan, &dest).is_err());
        assert!(
            !dir.path().join("occupied.zip.part").exists(),
            "the .part file must be cleaned up on failure"
        );
        assert!(dest.is_dir(), "the destination is untouched");
    }

    #[test]
    fn meeting_layout_keeps_files_at_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("meeting.zip");
        let plan = BundlePlan {
            entries: vec![text("transcript.md", "hi"), text("summary.md", "sum")],
            ..Default::default()
        };
        write_zip(plan, &dest).unwrap();
        assert_eq!(entry_names(&dest), vec!["transcript.md", "summary.md"]);
    }

    #[test]
    fn part_path_appends_rather_than_replacing_the_extension() {
        assert_eq!(
            part_path(Path::new("/tmp/My Meeting.zip")),
            PathBuf::from("/tmp/My Meeting.zip.part")
        );
    }
}
