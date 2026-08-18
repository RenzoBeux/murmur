//! Safety net for speaker bleed-through that survives the DSP gate.
//!
//! `echo_suppression` silences the mic frames it can explain from the system
//! reference, but it needs about a second to lock onto the speaker→mic delay and
//! it deliberately errs toward letting audio through when the correlation is
//! ambiguous. Whatever slips past shows up as the same sentence transcribed
//! twice: once tagged `system` ("them"), once tagged `mic` ("you").
//!
//! This runs over the *finished*, ordered segment list at save time rather than
//! on the live event stream. Workers transcribe in parallel, so a mic segment
//! and the system segment it duplicates can be emitted in either order — there
//! is no reliable "have I seen the original yet?" at emit time. By save time the
//! full list exists and the comparison is deterministic.
//!
//! Only the `mic` copy is ever dropped. The system stream is the faithful record
//! of what the remote participant said; the mic copy is the room hearing it back.

use log::info;

use crate::api::TranscriptSegment;

/// Speaker tag written by `device_type_to_speaker` for each source.
const SPEAKER_MIC: &str = "mic";
const SPEAKER_SYSTEM: &str = "system";

/// How far apart two segments may sit and still be the same utterance. Covers
/// the speaker→mic delay plus the difference in where each stream's VAD chose to
/// cut, which is the dominant term.
const MAX_TIME_GAP_SECS: f64 = 2.5;

/// Similarity at or above this counts as the same utterance. Not 1.0, because
/// the echoed copy is acoustically degraded and Whisper renders it slightly
/// differently ("yeah okay" vs "yeah, ok").
const SIMILARITY_THRESHOLD: f64 = 0.85;

/// Utterances this short are dropped only on an exact match — "yes" or "okay"
/// said independently by both sides is common and must not be swallowed.
const SHORT_UTTERANCE_CHARS: usize = 12;

/// Remove `mic` segments that duplicate a nearby `system` segment.
///
/// Returns the filtered list; the caller persists that. Segment order and every
/// surviving segment are left untouched.
pub fn drop_echoed_mic_segments(segments: Vec<TranscriptSegment>) -> Vec<TranscriptSegment> {
    // Index the system segments once, with their text pre-normalized.
    let system: Vec<(f64, f64, String)> = segments
        .iter()
        .filter(|s| s.speaker.as_deref() == Some(SPEAKER_SYSTEM))
        .map(|s| {
            (
                s.audio_start_time.unwrap_or(0.0),
                s.audio_end_time.unwrap_or(0.0),
                normalize(&s.text),
            )
        })
        .collect();

    if system.is_empty() {
        return segments;
    }

    let before = segments.len();
    let kept: Vec<TranscriptSegment> = segments
        .into_iter()
        .filter(|seg| {
            if seg.speaker.as_deref() != Some(SPEAKER_MIC) {
                return true;
            }
            let mic_text = normalize(&seg.text);
            if mic_text.is_empty() {
                return true;
            }
            let mic_start = seg.audio_start_time.unwrap_or(0.0);
            let mic_end = seg.audio_end_time.unwrap_or(mic_start);

            let echoed = system.iter().any(|(sys_start, sys_end, sys_text)| {
                if !within_gap(mic_start, mic_end, *sys_start, *sys_end) {
                    return false;
                }
                if mic_text.len() <= SHORT_UTTERANCE_CHARS {
                    return mic_text == *sys_text;
                }
                similarity(&mic_text, sys_text) >= SIMILARITY_THRESHOLD
            });

            !echoed
        })
        .collect();

    let dropped = before - kept.len();
    if dropped > 0 {
        info!(
            "🔁 Transcript dedup: dropped {} echoed mic segment(s) of {} total",
            dropped, before
        );
    }
    kept
}

/// True when the two segments overlap, or sit within `MAX_TIME_GAP_SECS`.
fn within_gap(a_start: f64, a_end: f64, b_start: f64, b_end: f64) -> bool {
    let gap = if a_start > b_end {
        a_start - b_end
    } else if b_start > a_end {
        b_start - a_end
    } else {
        0.0 // overlapping
    };
    gap <= MAX_TIME_GAP_SECS
}

/// Lowercase, strip punctuation, collapse whitespace — so the comparison sees
/// the words rather than Whisper's punctuation choices.
fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_was_space = true;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Similarity as the ratio of the longest common subsequence to the longer
/// string, over word tokens. Word-level rather than character-level so a single
/// misheard word costs one token instead of a whole span of characters.
fn similarity(a: &str, b: &str) -> f64 {
    let a_words: Vec<&str> = a.split(' ').filter(|w| !w.is_empty()).collect();
    let b_words: Vec<&str> = b.split(' ').filter(|w| !w.is_empty()).collect();
    if a_words.is_empty() || b_words.is_empty() {
        return 0.0;
    }

    // Classic LCS table. Segments are single utterances (tens of words), so the
    // quadratic cost is irrelevant here.
    let mut prev = vec![0usize; b_words.len() + 1];
    let mut cur = vec![0usize; b_words.len() + 1];
    for i in 1..=a_words.len() {
        for j in 1..=b_words.len() {
            cur[j] = if a_words[i - 1] == b_words[j - 1] {
                prev[j - 1] + 1
            } else {
                prev[j].max(cur[j - 1])
            };
        }
        std::mem::swap(&mut prev, &mut cur);
        cur.iter_mut().for_each(|v| *v = 0);
    }

    prev[b_words.len()] as f64 / a_words.len().max(b_words.len()) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(speaker: &str, start: f64, end: f64, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            id: format!("{speaker}-{start}"),
            text: text.to_string(),
            timestamp: String::new(),
            audio_start_time: Some(start),
            audio_end_time: Some(end),
            duration: Some(end - start),
            speaker: Some(speaker.to_string()),
        }
    }

    fn texts(segs: &[TranscriptSegment]) -> Vec<&str> {
        segs.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn drops_the_mic_copy_of_an_echoed_utterance() {
        let input = vec![
            seg(SPEAKER_SYSTEM, 10.0, 13.0, "So the migration should land next Tuesday."),
            seg(SPEAKER_MIC, 10.4, 13.2, "so the migration should land next tuesday"),
        ];
        let out = drop_echoed_mic_segments(input);
        assert_eq!(texts(&out), vec!["So the migration should land next Tuesday."]);
    }

    #[test]
    fn tolerates_the_wording_drift_of_a_degraded_echo() {
        let input = vec![
            seg(SPEAKER_SYSTEM, 4.0, 7.0, "I think we should ship it on Friday, honestly."),
            // The echoed copy loses a word and mangles punctuation.
            seg(SPEAKER_MIC, 4.3, 7.1, "I think we should ship it Friday honestly"),
        ];
        let out = drop_echoed_mic_segments(input);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].speaker.as_deref(), Some(SPEAKER_SYSTEM));
    }

    #[test]
    fn keeps_the_users_own_speech() {
        let input = vec![
            seg(SPEAKER_SYSTEM, 10.0, 13.0, "So the migration should land next Tuesday."),
            seg(SPEAKER_MIC, 13.5, 15.0, "Can we push that to Thursday instead?"),
        ];
        let out = drop_echoed_mic_segments(input);
        assert_eq!(out.len(), 2, "an actual reply must survive");
    }

    #[test]
    fn keeps_a_repeat_that_is_too_far_apart_to_be_echo() {
        let input = vec![
            seg(SPEAKER_SYSTEM, 10.0, 13.0, "We should really document this properly."),
            // Same words, but a minute later — the user agreeing, not an echo.
            seg(SPEAKER_MIC, 70.0, 73.0, "We should really document this properly."),
        ];
        let out = drop_echoed_mic_segments(input);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn short_confirmations_need_an_exact_match() {
        // "Yeah exactly" from both sides within the window: identical text is
        // treated as echo, but a near-miss is not.
        let echoed = drop_echoed_mic_segments(vec![
            seg(SPEAKER_SYSTEM, 5.0, 5.6, "Yeah, exactly."),
            seg(SPEAKER_MIC, 5.3, 5.9, "yeah exactly"),
        ]);
        assert_eq!(echoed.len(), 1);

        let distinct = drop_echoed_mic_segments(vec![
            seg(SPEAKER_SYSTEM, 5.0, 5.6, "Yeah, exactly."),
            seg(SPEAKER_MIC, 5.3, 5.9, "Yeah, agreed."),
        ]);
        assert_eq!(distinct.len(), 2, "a near-miss on a short phrase is not echo");
    }

    #[test]
    fn never_drops_a_system_segment() {
        // Two system segments that duplicate each other must both survive —
        // only the mic copy is ever considered echo.
        let input = vec![
            seg(SPEAKER_SYSTEM, 1.0, 3.0, "Let us start with the roadmap review."),
            seg(SPEAKER_SYSTEM, 3.1, 5.0, "Let us start with the roadmap review."),
        ];
        let out = drop_echoed_mic_segments(input);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn passes_through_when_there_is_no_system_audio() {
        let input = vec![
            seg(SPEAKER_MIC, 1.0, 3.0, "Recording a solo note here."),
            seg(SPEAKER_MIC, 3.0, 5.0, "Recording a solo note here."),
        ];
        let out = drop_echoed_mic_segments(input);
        assert_eq!(out.len(), 2, "with no system stream nothing can be echo");
    }

    #[test]
    fn segments_without_a_speaker_tag_are_untouched() {
        let input = vec![
            seg(SPEAKER_SYSTEM, 1.0, 3.0, "The quarterly numbers look fine to me."),
            TranscriptSegment {
                speaker: None,
                ..seg(SPEAKER_MIC, 1.2, 3.1, "The quarterly numbers look fine to me.")
            },
        ];
        let out = drop_echoed_mic_segments(input);
        assert_eq!(out.len(), 2, "an untagged segment is not attributable to the mic");
    }

    #[test]
    fn similarity_is_symmetric_and_bounded() {
        assert!((similarity("a b c", "a b c") - 1.0).abs() < 1e-9);
        assert_eq!(similarity("a b c", "x y z"), 0.0);
        let ab = similarity("one two three four", "one two four");
        let ba = similarity("one two four", "one two three four");
        assert!((ab - ba).abs() < 1e-9);
        assert!((0.0..=1.0).contains(&ab));
    }

    #[test]
    fn normalize_strips_punctuation_and_case() {
        assert_eq!(normalize("  Hello,  WORLD!! "), "hello world");
        assert_eq!(normalize("---"), "");
    }
}
