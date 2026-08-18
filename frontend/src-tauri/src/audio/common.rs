use crate::api::TranscriptSegment;
use crate::audio::decoder::DecodedAudio;
use anyhow::Result;
use log::{debug, info};
use std::path::Path;
use uuid::Uuid;

/// How to interpret a decoded file's channels when transcribing.
///
/// This is the single source of truth for the "you / them by channel" split,
/// shared by both the import and retranscription pipelines.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ChannelLayout {
    /// Downmix every channel to one mono stream; segments are left untagged
    /// (speaker = NULL). Diarization can be run later to recover speakers.
    Mixed,
    /// Two-channel "you / them" split. The `you_channel` (0 = Left, 1 = Right)
    /// is tagged `"mic"`; the other channel is tagged `"system"`. Falls back to
    /// `Mixed` behaviour when the decoded audio turns out to be mono.
    Separate { you_channel: usize },
}

/// Whether the offline paths (import, retranscription) should gate speaker
/// bleed-through, from the user's preference.
///
/// Unlike the live path, `Auto` always engages here: the output device attached
/// right now says nothing about what was playing when the audio was recorded,
/// and the detector self-disables when nothing correlates. Only an explicit
/// `Off` skips it.
pub(crate) async fn echo_suppression_enabled<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> bool {
    use crate::audio::echo_suppression::EchoSuppressionMode;

    match crate::audio::recording_preferences::load_recording_preferences(app).await {
        Ok(prefs) => prefs.echo_cancellation != EchoSuppressionMode::Off,
        Err(e) => {
            log::warn!("Could not read the echo-cancellation preference ({e}); leaving it on");
            true
        }
    }
}

/// Deinterleave one channel from interleaved multi-channel samples.
pub(crate) fn extract_channel(interleaved: &[f32], channels: u16, channel_index: usize) -> Vec<f32> {
    let ch = (channels as usize).max(1);
    if channel_index >= ch {
        return Vec::new();
    }
    interleaved
        .iter()
        .skip(channel_index)
        .step_by(ch)
        .copied()
        .collect()
}

/// Turn a decoded file into one or more 16 kHz-mono transcription jobs, each
/// paired with its source speaker tag (`Some("mic")` / `Some("system")` for a
/// channel-separated recording, or `None` for a mixed one).
///
/// With `echo_suppression` set, a channel-separated recording gets the same
/// speaker-bleed gating the live pipeline applies: the mic channel is silenced
/// wherever it is explained by the system channel. Without it, re-transcribing a
/// meeting recorded on speakers reproduces the duplicate-speaker bug, because
/// the recording deliberately stores the *raw* microphone.
///
/// Resampling is the heavy part — call this inside `spawn_blocking`.
pub(crate) fn build_channel_jobs(
    decoded: DecodedAudio,
    layout: ChannelLayout,
    echo_suppression: bool,
) -> Vec<(Vec<f32>, Option<&'static str>)> {
    match layout {
        ChannelLayout::Separate { you_channel } if decoded.channels >= 2 => {
            let src_channels = decoded.channels;
            let src_rate = decoded.sample_rate;
            let dur = decoded.duration_seconds;
            let you_channel = you_channel.min(1);
            let them_channel = 1 - you_channel;
            info!(
                "Channel split: you = ch{} (mic), them = ch{} (system)",
                you_channel, them_channel
            );
            let you = DecodedAudio {
                samples: extract_channel(&decoded.samples, src_channels, you_channel),
                sample_rate: src_rate,
                channels: 1,
                duration_seconds: dur,
            };
            let them = DecodedAudio {
                samples: extract_channel(&decoded.samples, src_channels, them_channel),
                sample_rate: src_rate,
                channels: 1,
                duration_seconds: dur,
            };
            drop(decoded);
            let them_16k = them.to_whisper_format();
            let mut you_16k = you.to_whisper_format();
            if echo_suppression {
                // Both channels came from the same interleaved file, so they are
                // already sample-aligned; only the acoustic delay remains.
                // `to_whisper_format` has already brought both to 16 kHz mono.
                you_16k =
                    crate::audio::echo_suppression::suppress_echo_offline(&you_16k, &them_16k, 16_000);
            }
            vec![(you_16k, Some("mic")), (them_16k, Some("system"))]
        }
        _ => vec![(decoded.to_whisper_format(), None)],
    }
}

/// Unload the transcription engine after a batch job (import or retranscription).
/// Skips unloading if a live recording is currently in progress, since recording
/// uses the same global engine instances.
pub(crate) async fn unload_engine_after_batch(use_parakeet: bool) {
    if crate::audio::recording_commands::is_recording().await {
        log::info!("Skipping model unload after batch: recording in progress");
        return;
    }

    if use_parakeet {
        use crate::parakeet_engine::commands::PARAKEET_ENGINE;
        let engine = {
            let guard = PARAKEET_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_ref().cloned()
        };
        if let Some(e) = engine {
            e.unload_model().await;
        }
    } else {
        use crate::whisper_engine::commands::WHISPER_ENGINE;
        let engine = {
            let guard = WHISPER_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_ref().cloned()
        };
        if let Some(e) = engine {
            e.unload_model().await;
        }
    }
}

/// Create transcript segments from transcription results.
/// Each tuple is (text, start_ms, end_ms) from VAD timestamps.
/// Retained for unit tests; production paths now tag segments per channel via
/// [`create_transcript_segments_with_speakers`].
#[allow(dead_code)]
pub(crate) fn create_transcript_segments(transcripts: &[(String, f64, f64)]) -> Vec<TranscriptSegment> {
    transcripts
        .iter()
        .map(|(text, start_ms, end_ms)| {
            let start_seconds = start_ms / 1000.0;
            let end_seconds = end_ms / 1000.0;
            let duration = end_seconds - start_seconds;

            TranscriptSegment {
                id: format!("transcript-{}", Uuid::new_v4()),
                text: text.trim().to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                audio_start_time: Some(start_seconds),
                audio_end_time: Some(end_seconds),
                duration: Some(duration),
                // Imported audio files don't have a stream source; leave unset.
                speaker: None,
            }
        })
        .collect()
}

/// Create transcript segments that carry a source-faithful speaker tag.
/// Each tuple is (text, start_ms, end_ms, speaker) where `speaker` is
/// `Some("mic")`/`Some("system")` for stereo recordings (channel origin) or
/// `None` when the source is unknown (mono/imported audio).
pub(crate) fn create_transcript_segments_with_speakers(
    transcripts: &[(String, f64, f64, Option<String>)],
) -> Vec<TranscriptSegment> {
    transcripts
        .iter()
        .map(|(text, start_ms, end_ms, speaker)| {
            let start_seconds = start_ms / 1000.0;
            let end_seconds = end_ms / 1000.0;
            let duration = end_seconds - start_seconds;

            TranscriptSegment {
                id: format!("transcript-{}", Uuid::new_v4()),
                text: text.trim().to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                audio_start_time: Some(start_seconds),
                audio_end_time: Some(end_seconds),
                duration: Some(duration),
                speaker: speaker.clone(),
            }
        })
        .collect()
}

/// Write transcripts.json to a meeting folder (atomic write with temp file)
pub(crate) fn write_transcripts_json(folder: &Path, segments: &[TranscriptSegment]) -> Result<()> {
    let transcript_path = folder.join("transcripts.json");
    let temp_path = folder.join(".transcripts.json.tmp");

    let json = serde_json::json!({
        "version": "1.0",
        "last_updated": chrono::Utc::now().to_rfc3339(),
        "total_segments": segments.len(),
        "segments": segments.iter().enumerate().map(|(i, s)| {
            serde_json::json!({
                "id": s.id,
                "text": s.text,
                "timestamp": s.timestamp,
                "audio_start_time": s.audio_start_time,
                "audio_end_time": s.audio_end_time,
                "duration": s.duration,
                "speaker": s.speaker,
                "sequence_id": i
            })
        }).collect::<Vec<_>>()
    });

    let json_string = serde_json::to_string_pretty(&json)?;
    std::fs::write(&temp_path, &json_string)?;
    std::fs::rename(&temp_path, &transcript_path)?;

    info!(
        "Wrote transcripts.json with {} segments to {}",
        segments.len(),
        transcript_path.display()
    );
    Ok(())
}

/// Split a long speech segment at the lowest-energy (silence) point near the target size.
///
/// Scans for 100ms windows with minimal RMS energy within +/-3 seconds of each target
/// split point. If no clear silence is found, falls back to a 1-second overlap split
/// to avoid cutting words at boundaries.
pub(crate) fn split_segment_at_silence(
    segment: &crate::audio::vad::SpeechSegment,
    max_samples: usize,
) -> Vec<crate::audio::vad::SpeechSegment> {
    const SAMPLE_RATE: usize = 16000;
    // 100ms window for energy measurement (1600 samples at 16kHz)
    const ENERGY_WINDOW: usize = SAMPLE_RATE / 10;
    // Search +/-3 seconds around the target split point
    const SEARCH_RADIUS: usize = SAMPLE_RATE * 3;
    // RMS threshold below which we consider a window "silent"
    const SILENCE_RMS_THRESHOLD: f32 = 0.02;
    // Overlap to use when no silence boundary is found (1 second)
    const FALLBACK_OVERLAP: usize = SAMPLE_RATE;

    let total = segment.samples.len();
    if total <= max_samples {
        return vec![segment.clone()];
    }

    let ms_per_sample = (segment.end_timestamp_ms - segment.start_timestamp_ms)
        / segment.samples.len() as f64;
    let mut result = Vec::new();
    let mut pos = 0usize;

    while pos < total {
        let remaining = total - pos;
        if remaining <= max_samples {
            // Last chunk - take everything remaining
            let chunk_samples = segment.samples[pos..].to_vec();
            let chunk_start_ms = segment.start_timestamp_ms + (pos as f64 * ms_per_sample);
            let chunk_end_ms = segment.end_timestamp_ms;
            result.push(crate::audio::vad::SpeechSegment {
                samples: chunk_samples,
                start_timestamp_ms: chunk_start_ms,
                end_timestamp_ms: chunk_end_ms,
                confidence: segment.confidence,
            });
            break;
        }

        // Target split point
        let target = pos + max_samples;

        // Search window: [target - SEARCH_RADIUS, target + SEARCH_RADIUS]
        let search_start = target.saturating_sub(SEARCH_RADIUS).max(pos + SAMPLE_RATE);
        let search_end = (target + SEARCH_RADIUS).min(total.saturating_sub(ENERGY_WINDOW));

        // Find the lowest-energy 100ms window in the search range
        let mut best_split = target.min(total); // fallback: exact target
        let mut best_rms = f32::MAX;

        if search_start + ENERGY_WINDOW <= search_end {
            let mut idx = search_start;
            while idx + ENERGY_WINDOW <= search_end {
                let window = &segment.samples[idx..idx + ENERGY_WINDOW];
                let rms = (window.iter().map(|s| s * s).sum::<f32>() / ENERGY_WINDOW as f32).sqrt();
                if rms < best_rms {
                    best_rms = rms;
                    best_split = idx + ENERGY_WINDOW / 2; // split at center of quiet window
                }
                // Step by 10ms (160 samples) for efficiency
                idx += SAMPLE_RATE / 100;
            }
        }

        let split_at = best_split;
        if best_rms <= SILENCE_RMS_THRESHOLD {
            debug!(
                "Splitting at silence boundary: sample {} (RMS={:.4})",
                split_at, best_rms
            );
        } else {
            debug!(
                "No silence found near target (best RMS={:.4}), splitting with overlap at sample {}",
                best_rms, split_at
            );
        }

        // Determine the actual end of this chunk (with overlap if no silence)
        let chunk_end = if best_rms > SILENCE_RMS_THRESHOLD {
            (split_at + FALLBACK_OVERLAP).min(total)
        } else {
            split_at
        };

        let chunk_samples = segment.samples[pos..chunk_end].to_vec();
        let chunk_start_ms = segment.start_timestamp_ms + (pos as f64 * ms_per_sample);
        let chunk_end_ms = segment.start_timestamp_ms + (chunk_end as f64 * ms_per_sample);

        result.push(crate::audio::vad::SpeechSegment {
            samples: chunk_samples,
            start_timestamp_ms: chunk_start_ms,
            end_timestamp_ms: chunk_end_ms,
            confidence: segment.confidence,
        });

        // Advance position to where the current chunk actually ends
        // to avoid transcribing the overlap region twice
        pos = chunk_end;
    }

    result
}
