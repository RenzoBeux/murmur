//! Filesystem-safe naming for zip entries.
//!
//! A zip we hand to someone else gets extracted on a machine we know nothing
//! about, so entry names have to survive the strictest common denominator —
//! Windows Explorer. That means more than swapping illegal characters:
//! reserved device names, trailing dots and `MAX_PATH` all bite at extraction
//! time rather than when we write the archive.

use std::collections::HashSet;

use crate::audio::audio_processing::sanitize_filename;

/// Windows reserved device names. Illegal as a filename *stem* even when an
/// extension follows, so `CON.md` is rejected exactly like `CON`.
const RESERVED_STEMS: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Per-component cap. Deliberately well under the 255-byte filesystem limit so
/// that `<download dir>/<meeting folder>/files/<attachment>` still clears
/// Windows' 260-char `MAX_PATH` in Explorer's built-in extractor.
const MAX_COMPONENT_CHARS: usize = 80;

/// Make one path component safe to use as a zip entry name on any platform.
///
/// Non-ASCII is preserved — zip 2.x sets general-purpose bit 11 for names that
/// aren't CP437, and Windows 10+ honors it, so `Reunión` survives the round
/// trip intact.
pub fn sanitize_zip_component(raw: &str) -> String {
    // Whitespace is normalized BEFORE sanitizing: tabs and newlines are control
    // characters, so `sanitize_filename` would otherwise turn them into
    // underscores and leave separator gunk in the middle of the name.
    let collapsed = collapse_whitespace(raw);

    // Replaces / \ : * ? " < > | and any remaining control chars, then trims.
    let base = sanitize_filename(&collapsed);

    let mut out = truncate_chars(base.trim(), MAX_COMPONENT_CHARS);

    // Trailing dots and spaces are silently mangled by Explorer. Doing this
    // after truncation matters: the cut itself can expose one.
    out = out
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_start()
        .to_string();

    if out.is_empty() {
        return "untitled".to_string();
    }

    // Reserved-name check comes last, so a name that only *became* reserved by
    // being truncated is still caught.
    let stem = out.split('.').next().unwrap_or("");
    if RESERVED_STEMS
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return format!("_{out}");
    }

    out
}

/// Squash every run of whitespace down to a single space.
fn collapse_whitespace(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_space = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push(' ');
            }
            last_was_space = true;
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    out
}

/// Truncate at a `char` boundary so multi-byte titles never panic.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

/// Hands out unique names within one directory of the archive.
///
/// Keyed **lowercased**: a zip can legally hold both `Sync/` and `SYNC/`, but
/// extracting it on Windows or macOS collapses them and one silently
/// overwrites the other. Deduping case-insensitively is what makes the
/// extracted tree match the archive.
#[derive(Debug, Default)]
pub struct NameAllocator {
    used: HashSet<String>,
}

impl NameAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Unique name for a directory or extension-less entry.
    pub fn allocate(&mut self, desired: &str) -> String {
        let base = sanitize_zip_component(desired);
        self.claim(&base, |base, n| format!("{base}-{n}"))
    }

    /// Unique name for a file, suffixing the **stem** so the extension keeps
    /// working: `photo.png` twice yields `photo.png` and `photo-2.png`, never
    /// `photo.png-2`.
    pub fn allocate_filename(&mut self, desired: &str) -> String {
        let base = sanitize_zip_component(desired);
        let (stem, ext) = match base.rfind('.') {
            // A leading dot is part of the name (".gitignore"), not a separator.
            Some(idx) if idx > 0 => (&base[..idx], &base[idx..]),
            _ => (base.as_str(), ""),
        };
        let stem = stem.to_string();
        let ext = ext.to_string();
        self.claim(&base, move |_, n| format!("{stem}-{n}{ext}"))
    }

    fn claim(&mut self, base: &str, make_candidate: impl Fn(&str, u32) -> String) -> String {
        if self.used.insert(base.to_lowercase()) {
            return base.to_string();
        }
        for n in 2..u32::MAX {
            let candidate = make_candidate(base, n);
            if self.used.insert(candidate.to_lowercase()) {
                return candidate;
            }
        }
        unreachable!("name allocator exhausted")
    }
}

/// `<YYYY-MM-DD> <title>` — the folder one meeting occupies inside a project
/// archive. Date first so alphabetical order equals chronological order.
pub fn meeting_folder_name(title: &str, created_at_rfc3339: &str) -> String {
    let date = created_at_rfc3339.get(0..10).unwrap_or("undated");
    sanitize_zip_component(&format!("{date} {title}"))
}

/// Suggested filename for the save dialog, always `.zip`.
pub fn bundle_filename(stem: &str) -> String {
    format!("{}.zip", sanitize_zip_component(stem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_windows_illegal_characters() {
        assert_eq!(
            sanitize_zip_component(r#"a/b\c:d*e?f"g<h>i|j"#),
            "a_b_c_d_e_f_g_h_i_j"
        );
    }

    #[test]
    fn escapes_reserved_device_names_on_the_stem() {
        assert_eq!(sanitize_zip_component("CON"), "_CON");
        assert_eq!(sanitize_zip_component("con"), "_con");
        assert_eq!(sanitize_zip_component("NUL"), "_NUL");
        assert_eq!(sanitize_zip_component("COM1"), "_COM1");
        assert_eq!(sanitize_zip_component("LPT9"), "_LPT9");
        // The extension does not rescue it.
        assert_eq!(sanitize_zip_component("CON.md"), "_CON.md");
        // Not reserved.
        assert_eq!(sanitize_zip_component("CONSOLE"), "CONSOLE");
        assert_eq!(sanitize_zip_component("COM10"), "COM10");
    }

    #[test]
    fn trims_trailing_dots_and_spaces() {
        assert_eq!(sanitize_zip_component("Notes... "), "Notes");
        assert_eq!(sanitize_zip_component("  Notes  "), "Notes");
        assert_eq!(sanitize_zip_component("Notes."), "Notes");
    }

    #[test]
    fn collapses_whitespace_runs() {
        assert_eq!(sanitize_zip_component("Weekly   \t Sync"), "Weekly Sync");
    }

    #[test]
    fn truncates_on_a_char_boundary_without_panicking() {
        let long = "é".repeat(200);
        let out = sanitize_zip_component(&long);
        assert_eq!(out.chars().count(), MAX_COMPONENT_CHARS);
        assert!(out.is_char_boundary(out.len()));
    }

    #[test]
    fn falls_back_to_untitled_when_nothing_survives() {
        assert_eq!(sanitize_zip_component(""), "untitled");
        assert_eq!(sanitize_zip_component("   "), "untitled");
        assert_eq!(sanitize_zip_component("."), "untitled");
        assert_eq!(sanitize_zip_component(".."), "untitled");
    }

    #[test]
    fn keeps_non_ascii_titles() {
        assert_eq!(sanitize_zip_component("Reunión de equipo"), "Reunión de equipo");
    }

    #[test]
    fn allocator_dedupes_case_insensitively() {
        let mut alloc = NameAllocator::new();
        assert_eq!(alloc.allocate("Sync"), "Sync");
        assert_eq!(alloc.allocate("SYNC"), "SYNC-2");
        assert_eq!(alloc.allocate("sync"), "sync-3");
    }

    #[test]
    fn allocator_suffixes_the_stem_not_the_extension() {
        let mut alloc = NameAllocator::new();
        assert_eq!(alloc.allocate_filename("photo.png"), "photo.png");
        assert_eq!(alloc.allocate_filename("photo.png"), "photo-2.png");
        assert_eq!(alloc.allocate_filename("photo.png"), "photo-3.png");
    }

    #[test]
    fn allocator_handles_dotfiles_and_extensionless_names() {
        let mut alloc = NameAllocator::new();
        assert_eq!(alloc.allocate_filename(".gitignore"), ".gitignore");
        assert_eq!(alloc.allocate_filename(".gitignore"), ".gitignore-2");
        assert_eq!(alloc.allocate_filename("README"), "README");
        assert_eq!(alloc.allocate_filename("README"), "README-2");
    }

    #[test]
    fn meeting_folder_puts_the_date_first() {
        assert_eq!(
            meeting_folder_name("Weekly Sync", "2026-06-02T09:00:00+00:00"),
            "2026-06-02 Weekly Sync"
        );
        assert_eq!(meeting_folder_name("Sync", "bogus"), "undated Sync");
    }

    #[test]
    fn bundle_filename_always_ends_in_zip() {
        assert_eq!(bundle_filename("Q3: Planning"), "Q3_ Planning.zip");
    }
}
