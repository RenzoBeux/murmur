use std::sync::Arc;
use std::collections::VecDeque;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use anyhow::Result;
use log::{debug, error, info, warn};
use crate::batch_audio_metric;
use super::batch_processor::AudioMetricsBatcher;
use rubato::{Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction};

use super::devices::AudioDevice;
use super::recording_state::{AudioChunk, AudioError, RecordingState, DeviceType};
use super::audio_processing::{audio_to_mono, LoudnessNormalizer, NoiseSuppressionProcessor, HighPassFilter};
use super::echo_suppression::EchoSuppressor;
use super::vad::{ContinuousVadProcessor};

/// Ring buffer for synchronized audio mixing
/// Accumulates samples from mic and system streams until we have aligned windows
struct AudioMixerRingBuffer {
    mic_buffer: VecDeque<f32>,
    system_buffer: VecDeque<f32>,
    window_size_samples: usize,  // Fixed mixing window (600ms)
    max_buffer_size: usize,  // Safety limit (8× the window)
}

impl AudioMixerRingBuffer {
    fn new(sample_rate: u32) -> Self {
        // Use 50ms windows for mixing
        let window_ms = 600.0;
        let window_size_samples = (sample_rate as f32 * window_ms / 1000.0) as usize;

        // CRITICAL FIX: Increase max buffer to 400ms for system audio stability
        // System audio (especially Core Audio on macOS) can have significant jitter
        // due to sample-by-sample streaming → batching → channel transmission
        // Accounts for: RNNoise buffering + Core Audio jitter + processing delays
        let max_buffer_size = window_size_samples * 8;  // 400ms (was 200ms)

        info!("🔊 Ring buffer initialized: window={}ms ({} samples), max={}ms ({} samples)",
              window_ms, window_size_samples,
              window_ms * 8.0, max_buffer_size);

        Self {
            mic_buffer: VecDeque::with_capacity(max_buffer_size),
            system_buffer: VecDeque::with_capacity(max_buffer_size),
            window_size_samples,
            max_buffer_size,
        }
    }

    fn add_samples(&mut self, device_type: DeviceType, samples: Vec<f32>) {
        // Log buffer health periodically for diagnostics
        static mut SAMPLE_COUNTER: u64 = 0;
        unsafe {
            SAMPLE_COUNTER += 1;
            if SAMPLE_COUNTER % 200 == 0 {
                debug!("📊 Ring buffer status: mic={} samples, sys={} samples (max={})",
                       self.mic_buffer.len(), self.system_buffer.len(), self.max_buffer_size);
            }
        }

        match device_type {
            DeviceType::Microphone => self.mic_buffer.extend(samples),
            DeviceType::System => self.system_buffer.extend(samples),
        }

        // CRITICAL FIX: Add warnings before dropping samples
        // This helps diagnose timing issues in production
        if self.mic_buffer.len() > self.max_buffer_size {
            warn!("⚠️ Microphone buffer overflow: {} > {} samples, dropping oldest {} samples",
                  self.mic_buffer.len(), self.max_buffer_size,
                  self.mic_buffer.len() - self.max_buffer_size);
        }
        if self.system_buffer.len() > self.max_buffer_size {
            error!("🔴 SYSTEM AUDIO BUFFER OVERFLOW: {} > {} samples, dropping {} samples - THIS CAUSES DISTORTION!",
                  self.system_buffer.len(), self.max_buffer_size,
                  self.system_buffer.len() - self.max_buffer_size);
        }

        // Safety: prevent buffer overflow (keep only last 200ms)
        while self.mic_buffer.len() > self.max_buffer_size {
            self.mic_buffer.pop_front();
        }
        while self.system_buffer.len() > self.max_buffer_size {
            self.system_buffer.pop_front();
        }
    }

    fn can_mix(&self) -> bool {
        self.mic_buffer.len() >= self.window_size_samples ||
        self.system_buffer.len() >= self.window_size_samples
    }

    fn extract_window(&mut self) -> Option<(Vec<f32>, Vec<f32>)> {
        if !self.can_mix() {
            return None;
        }

        // Extract mic window with zero-padding for incomplete buffers
        // Zero-padding (silence) is preferred over last-sample-hold to prevent artifacts

        // Extract mic window (or pad with zeros if insufficient data)
        let mic_window = if self.mic_buffer.len() >= self.window_size_samples {
            // Enough mic data - drain window
            self.mic_buffer.drain(0..self.window_size_samples).collect()
        } else if !self.mic_buffer.is_empty() {
            // Some mic data but not enough - consume all + pad with zeros
            let available: Vec<f32> = self.mic_buffer.drain(..).collect();
            let mut padded = Vec::with_capacity(self.window_size_samples);
            padded.extend_from_slice(&available);

            // Use zero-padding (silence) to prevent repetition artifacts
            // Zero-padding is inaudible at 48kHz sample rate
            padded.resize(self.window_size_samples, 0.0);

            padded
        } else {
            // No mic data - return silence
            vec![0.0; self.window_size_samples]
        };

        // Extract system window (or pad with zeros if insufficient data)
        let sys_window = if self.system_buffer.len() >= self.window_size_samples {
            // Enough system data - drain window
            self.system_buffer.drain(0..self.window_size_samples).collect()
        } else if !self.system_buffer.is_empty() {
            // Some system data but not enough - consume all + pad with zeros
            let available: Vec<f32> = self.system_buffer.drain(..).collect();
            let mut padded = Vec::with_capacity(self.window_size_samples);
            padded.extend_from_slice(&available);

            // Use zero-padding (silence) to prevent repetition artifacts
            // Zero-padding is inaudible at 48kHz sample rate
            padded.resize(self.window_size_samples, 0.0);

            padded
        } else {
            // No system data - return silence
            vec![0.0; self.window_size_samples]
        };

        Some((mic_window, sys_window))
    }

    /// Drain whatever samples remain — a trailing partial window on stop — zero-
    /// padding the shorter side to equal length, so the final sub-window (often
    /// the last word) still reaches the VADs and the recording. Returns None once
    /// the buffer is empty, so repeated flushes are idempotent.
    fn drain_partial(&mut self) -> Option<(Vec<f32>, Vec<f32>)> {
        if self.mic_buffer.is_empty() && self.system_buffer.is_empty() {
            return None;
        }
        let mut mic: Vec<f32> = self.mic_buffer.drain(..).collect();
        let mut sys: Vec<f32> = self.system_buffer.drain(..).collect();
        let len = mic.len().max(sys.len());
        mic.resize(len, 0.0);
        sys.resize(len, 0.0);
        Some((mic, sys))
    }

}

/// Simplified audio capture without broadcast channels
#[derive(Clone)]
pub struct AudioCapture {
    device: Arc<AudioDevice>,
    state: Arc<RecordingState>,
    sample_rate: u32,        // Original device sample rate
    channels: u16,
    chunk_counter: Arc<std::sync::atomic::AtomicU64>,
    device_type: DeviceType,
    recording_sender: Option<mpsc::UnboundedSender<AudioChunk>>,
    needs_resampling: bool,  // Flag if resampling is required
    // CRITICAL FIX: Persistent resampler to preserve energy across chunks
    resampler: Arc<std::sync::Mutex<Option<SincFixedIn<f32>>>>,
    // Buffering for variable-size chunks → fixed-size resampler input
    resampler_input_buffer: Arc<std::sync::Mutex<Vec<f32>>>,
    resampler_chunk_size: usize,  // Fixed chunk size for resampler (512 samples)
    // Audio enhancement processors (microphone only)
    noise_suppressor: Arc<std::sync::Mutex<Option<NoiseSuppressionProcessor>>>,
    high_pass_filter: Arc<std::sync::Mutex<Option<HighPassFilter>>>,
    // EBU R128 normalizer for microphone audio (per-device, stateful)
    normalizer: Arc<std::sync::Mutex<Option<LoudnessNormalizer>>>,
    // Note: Using global recording timestamp for synchronization
}

impl AudioCapture {
    pub fn new(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        sample_rate: u32,
        channels: u16,
        device_type: DeviceType,
        recording_sender: Option<mpsc::UnboundedSender<AudioChunk>>,
    ) -> Self {
        // CRITICAL FIX: Detect if resampling is needed
        // Pipeline expects 48kHz, but Bluetooth devices often report 8kHz, 16kHz, or 44.1kHz
        const TARGET_SAMPLE_RATE: u32 = 48000;
        let needs_resampling = sample_rate != TARGET_SAMPLE_RATE;

        // Detect device kind (Bluetooth vs Wired) for adaptive processing
        // Use reasonable defaults for buffer size (512 samples is typical)
        let device_kind = super::device_detection::InputDeviceKind::detect(&device.name, 512, sample_rate);

        if needs_resampling {
            warn!(
                "⚠️ SAMPLE RATE MISMATCH DETECTED ⚠️"
            );
            warn!(
                "🔄 [{:?}] Audio device '{}' ({:?}) reports {} Hz (pipeline expects {} Hz)",
                device_type, device.name, device_kind, sample_rate, TARGET_SAMPLE_RATE
            );
            warn!(
                "🔄 Automatic resampling will be applied: {} Hz → {} Hz",
                sample_rate, TARGET_SAMPLE_RATE
            );

            // Log which resampling strategy will be used
            let ratio = TARGET_SAMPLE_RATE as f64 / sample_rate as f64;
            let strategy = if ratio >= 2.0 {
                "High-quality upsampling (sinc_len=512, Cubic interpolation)"
            } else if ratio >= 1.5 {
                "Moderate upsampling (sinc_len=384, Cubic)"
            } else if ratio > 1.0 {
                "Small upsampling (sinc_len=256, Linear)"
            } else if ratio <= 0.5 {
                "Anti-aliased downsampling (sinc_len=512, Cubic)"
            } else {
                "Moderate downsampling (sinc_len=384, Linear)"
            };
            info!("   Resampling strategy: {}", strategy);
        } else {
            info!(
                "✅ [{:?}] Audio device '{}' ({:?}) uses {} Hz (matches pipeline)",
                device_type, device.name, device_kind, sample_rate
            );
        }

        // Initialize audio enhancement processors for MICROPHONE ONLY
        // System audio doesn't need enhancement (already clean)
        let (noise_suppressor, high_pass_filter, normalizer) = if matches!(device_type, DeviceType::Microphone) {
            // Initialize noise suppression (RNNoise) at 48kHz - CONDITIONAL based on flag
            let ns = if super::ffmpeg_mixer::RNNOISE_APPLY_ENABLED {
                match NoiseSuppressionProcessor::new(TARGET_SAMPLE_RATE) {
                    Ok(processor) => {
                        info!("✅ RNNoise noise suppression ENABLED for microphone '{}' (10-15 dB reduction)", device.name);
                        Some(processor)
                    }
                    Err(e) => {
                        warn!("⚠️ Failed to create noise suppressor: {}, continuing without noise suppression", e);
                        None
                    }
                }
            } else {
                info!("ℹ️ RNNoise noise suppression DISABLED for microphone '{}' (flag: RNNOISE_APPLY_ENABLED=false)", device.name);
                info!("   Whisper handles noise well internally - RNNoise is optional");
                None
            };

            // Initialize high-pass filter (removes rumble below 80 Hz)
            let hpf = {
                let filter = HighPassFilter::new(TARGET_SAMPLE_RATE, 80.0);
                info!("✅ High-pass filter initialized for microphone '{}' (cutoff: 80 Hz)", device.name);
                Some(filter)
            };

            // Initialize EBU R128 normalizer (professional loudness standard)
            let norm = match LoudnessNormalizer::new(1, TARGET_SAMPLE_RATE) {
                Ok(normalizer) => {
                    info!("✅ EBU R128 normalizer initialized for microphone '{}' (target: -23 LUFS)", device.name);
                    Some(normalizer)
                }
                Err(e) => {
                    warn!("⚠️ Failed to create normalizer for microphone: {}, normalization disabled", e);
                    None
                }
            };

            (ns, hpf, norm)
        } else {
            // System audio: no enhancement needed
            info!("ℹ️ System audio '{}' captured raw (no enhancement)", device.name);
            (None, None, None)
        };

        // CRITICAL FIX: Initialize persistent resampler to preserve energy across chunks
        // Creating a new resampler per chunk causes energy amplification and incorrect output sizes
        // Use fixed chunk size of 512 samples with buffering for variable-size input
        const RESAMPLER_CHUNK_SIZE: usize = 512;

        let resampler = if needs_resampling {
            let ratio = TARGET_SAMPLE_RATE as f64 / sample_rate as f64;

            // Adaptive parameters based on sample rate ratio (same logic as resample_audio)
            let (sinc_len, interpolation_type, oversampling) = if ratio >= 2.0 {
                (512, SincInterpolationType::Cubic, 512)
            } else if ratio >= 1.5 {
                (384, SincInterpolationType::Cubic, 384)
            } else if ratio > 1.0 {
                (256, SincInterpolationType::Linear, 256)
            } else if ratio <= 0.5 {
                (512, SincInterpolationType::Cubic, 512)
            } else {
                (384, SincInterpolationType::Linear, 384)
            };

            let params = SincInterpolationParameters {
                sinc_len,
                f_cutoff: 0.95,
                interpolation: interpolation_type,
                oversampling_factor: oversampling,
                window: WindowFunction::BlackmanHarris2,
            };

            match SincFixedIn::<f32>::new(
                ratio,
                2.0,  // Maximum relative deviation
                params,
                RESAMPLER_CHUNK_SIZE,
                1,    // Mono
            ) {
                Ok(resampler) => {
                    info!("✅ Persistent resampler initialized for '{}' ({}Hz → {}Hz, chunk_size={})",
                          device.name, sample_rate, TARGET_SAMPLE_RATE, RESAMPLER_CHUNK_SIZE);
                    info!("   Buffering enabled for variable-size chunks (e.g., 320, 512, 1024, etc.)");
                    Some(resampler)
                }
                Err(e) => {
                    warn!("⚠️ Failed to create persistent resampler: {}, will use fallback", e);
                    None
                }
            }
        } else {
            None
        };

        Self {
            device,
            state,
            sample_rate,
            channels,
            chunk_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            device_type,
            recording_sender,
            needs_resampling,
            resampler: Arc::new(std::sync::Mutex::new(resampler)),
            resampler_input_buffer: Arc::new(std::sync::Mutex::new(Vec::with_capacity(RESAMPLER_CHUNK_SIZE * 2))),
            resampler_chunk_size: RESAMPLER_CHUNK_SIZE,
            noise_suppressor: Arc::new(std::sync::Mutex::new(noise_suppressor)),
            high_pass_filter: Arc::new(std::sync::Mutex::new(high_pass_filter)),
            normalizer: Arc::new(std::sync::Mutex::new(normalizer)),
            // Using global recording time for sync
        }
    }

    /// Process audio data directly from callback
    pub fn process_audio_data(&self, data: &[f32]) {
        // Check if still recording
        if !self.state.is_recording() {
            return;
        }

        // Manual microphone kill switch. Dropping here — before mono conversion,
        // resampling, enhancement, the ring buffer, and the recording writer —
        // is what makes this a real failsafe rather than a downstream filter:
        // no microphone sample survives this point while the switch is engaged.
        // The ring buffer zero-pads the starved mic side, so the recording gets
        // silence on its left channel and the timeline stays aligned. System
        // audio is untouched.
        if matches!(self.device_type, DeviceType::Microphone) && self.state.is_mic_muted() {
            return;
        }

        // Convert to mono if needed
        let mut mono_data = if self.channels > 1 {
            audio_to_mono(data, self.channels)
        } else {
            data.to_vec()
        };

        // CRITICAL FIX: Resample to 48kHz if device uses different sample rate
        // This fixes Bluetooth devices (like Sony WH-1000XM4) that report 16kHz or 44.1kHz
        // Without this, audio is sped up 3x and VAD fails
        //
        // IMPORTANT: Uses PERSISTENT resampler with BUFFERING to preserve energy across chunks
        // Creating a new resampler per chunk causes energy amplification (173.5% RMS)
        // Buffering handles variable chunk sizes (320, 512, 1024, etc.) by accumulating to fixed 512-sample chunks
        const TARGET_SAMPLE_RATE: u32 = 48000;
        if self.needs_resampling {
            let before_len = mono_data.len();
            let before_rms = if !mono_data.is_empty() {
                (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt()
            } else {
                0.0
            };

            // Use persistent resampler with buffering to handle variable chunk sizes
            let mut resampled_output = Vec::new();
            let mut used_persistent_resampler = false;

            if let Ok(mut buffer_lock) = self.resampler_input_buffer.lock() {
                // Add new samples to buffer
                buffer_lock.extend_from_slice(&mono_data);

                // Process complete chunks through the resampler
                if let Ok(mut resampler_lock) = self.resampler.lock() {
                    if let Some(ref mut resampler) = *resampler_lock {
                        used_persistent_resampler = true;

                        // Process as many complete chunks as we have
                        while buffer_lock.len() >= self.resampler_chunk_size {
                            // Extract exactly chunk_size samples
                            let chunk: Vec<f32> = buffer_lock.drain(0..self.resampler_chunk_size).collect();

                            // Rubato expects input as Vec<Vec<f32>> (one Vec per channel)
                            let waves_in = vec![chunk];

                            match resampler.process(&waves_in, None) {
                                Ok(mut waves_out) => {
                                    if let Some(output) = waves_out.pop() {
                                        resampled_output.extend_from_slice(&output);
                                    }
                                }
                                Err(e) => {
                                    warn!("⚠️ Persistent resampler processing failed: {}", e);
                                    used_persistent_resampler = false;
                                    break;
                                }
                            }
                        }
                        // Remaining samples in buffer will be processed in next iteration
                    }
                }
            }

            // CRITICAL: Only update mono_data if we got output from persistent resampler
            // If buffer is accumulating (< 512 samples), skip this chunk - data is safely buffered
            // and will be processed in next iteration with proper resampling
            let has_resampled_output = !resampled_output.is_empty();

            if has_resampled_output {
                mono_data = resampled_output;
            } else if !used_persistent_resampler {
                // Only fallback if persistent resampler is not available at all
                mono_data = super::audio_processing::resample_audio(
                    &mono_data,
                    self.sample_rate,
                    TARGET_SAMPLE_RATE,
                );
            } else {
                // Buffering: samples are accumulating in buffer, waiting for 512-sample chunk
                // Don't send partial/unprocessed data - return early
                // Audio is NOT lost - it's in the buffer and will be processed next iteration
                return;
            }

            // Log resampling only occasionally to avoid spam
            let chunk_id = self.chunk_counter.load(std::sync::atomic::Ordering::SeqCst);
            if chunk_id % 100 == 0 && has_resampled_output {
                let after_len = mono_data.len();
                let after_rms = if !mono_data.is_empty() {
                    (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt()
                } else {
                    0.0
                };
                let ratio = TARGET_SAMPLE_RATE as f64 / self.sample_rate as f64;
                let rms_preservation = if before_rms > 0.0 { (after_rms / before_rms) * 100.0 } else { 100.0 };

                let buffer_size = if let Ok(buf) = self.resampler_input_buffer.lock() {
                    buf.len()
                } else {
                    0
                };

                info!(
                    "🔄 [{:?}] Persistent buffered resampler: {}Hz → {}Hz (ratio: {:.2}x)",
                    self.device_type,
                    self.sample_rate,
                    TARGET_SAMPLE_RATE,
                    ratio
                );
                info!(
                    "   Chunk {}: {} → {} samples, RMS preservation: {:.1}%, buffer: {}",
                    chunk_id,
                    before_len,
                    after_len,
                    rms_preservation,
                    buffer_size
                );
            }
        }

        // AUDIO ENHANCEMENT PIPELINE (Microphone Only)
        // Processing order is critical: high-pass → noise suppression → normalization
        // This ensures noise is removed before being amplified by the normalizer
        if matches!(self.device_type, DeviceType::Microphone) {
            // STEP 1: Apply high-pass filter to remove low-frequency rumble (< 80 Hz)
            if let Ok(mut hpf_lock) = self.high_pass_filter.lock() {
                if let Some(ref mut filter) = *hpf_lock {
                    mono_data = filter.process(&mono_data);
                }
            }

            // STEP 2: Apply RNNoise noise suppression (10-15 dB reduction) - CONDITIONAL
            if super::ffmpeg_mixer::RNNOISE_APPLY_ENABLED {
                if let Ok(mut ns_lock) = self.noise_suppressor.lock() {
                    if let Some(ref mut suppressor) = *ns_lock {
                        let before_len = mono_data.len();
                        mono_data = suppressor.process(&mono_data);
                        let after_len = mono_data.len();

                        // CRITICAL MONITORING: Track buffer health
                        let chunk_id = self.chunk_counter.load(std::sync::atomic::Ordering::SeqCst);
                        if chunk_id % 100 == 0 {
                            let buffered = suppressor.buffered_samples();
                            let length_delta = (before_len as i32 - after_len as i32).abs();

                            debug!("🔇 Noise suppression health: in={}, out={}, delta={}, buffered={}, RMS={:.4}",
                                   before_len, after_len, length_delta, buffered,
                                   if !mono_data.is_empty() {
                                       (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt()
                                   } else { 0.0 });

                            // WARN if accumulating samples (potential latency buildup)
                            if buffered > 1000 {
                                warn!("⚠️ RNNoise accumulating samples: {} buffered (potential latency issue!)",
                                      buffered);
                            }

                            // WARN if significant length mismatch
                            if length_delta > 50 {
                                warn!("⚠️ RNNoise length mismatch: input={} output={} (delta={})",
                                      before_len, after_len, length_delta);
                            }
                        }
                    }
                }
            }

            // STEP 3: Apply EBU R128 normalization (professional loudness standard)
            if let Ok(mut normalizer_lock) = self.normalizer.lock() {
                if let Some(ref mut normalizer) = *normalizer_lock {
                    mono_data = normalizer.normalize_loudness(&mono_data);

                    // Log normalization occasionally for debugging
                    let chunk_id = self.chunk_counter.load(std::sync::atomic::Ordering::SeqCst);
                    if chunk_id % 200 == 0 && !mono_data.is_empty() {
                        let rms = (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt();
                        let peak = mono_data.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);
                        debug!("🎤 After normalization chunk {}: RMS={:.4}, Peak={:.4}", chunk_id, rms, peak);
                    }
                }
            }
        }

        // Create audio chunk with stream-specific timestamp (get ID first for logging)
        let chunk_id = self.chunk_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // RAW AUDIO: No gain applied here - will be applied AFTER mixing
        // This prevents amplifying system audio bleed-through in the microphone

        // DIAGNOSTIC: Log audio levels for debugging (especially mic issues)
        // if chunk_id % 100 == 0 && !mono_data.is_empty() {
        //     let raw_rms = (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt();
        //     let raw_peak = mono_data.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);

        //         info!("🎙️ [{:?}] Chunk {} - Raw: RMS={:.6}, Peak={:.6}",
        //               self.device_type, chunk_id, raw_rms, raw_peak);

        //     // Warn if microphone is completely silent
        //     if matches!(self.device_type, DeviceType::Microphone) && raw_rms == 0.0 && raw_peak == 0.0 {
        //         warn!("⚠️ Microphone producing ZERO audio - check permissions or hardware!");
        //     }
        // }
        // else if chunk_id % 100 == 0 && matches!(self.device_type, DeviceType::System) {
        //     let raw_rms = (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt();
        //     let raw_peak = mono_data.iter().map(|&x| x.abs()).fold(0.0f32, f32::max);
        //     info!("🔊 [{:?}] Chunk {} - Raw: RMS={:.6}, Peak={:.6}",
        //       self.device_type, chunk_id, raw_rms, raw_peak);
            
        //     // Warn if system audio is completely silent
        //     if raw_rms == 0.0 && raw_peak == 0.0 {
        //         warn!("⚠️ System audio producing ZERO audio - check permissions or hardware!");
        //     }
        // }

        // Publish this device's live loudness (RMS) so the recording HUD shows a REAL
        // level meter instead of a fake waveform, and so the silence watchdog can tell
        // a live-but-quiet stream from a dead one. Cheap: one pass + one atomic store.
        if !mono_data.is_empty() {
            let rms = (mono_data.iter().map(|&x| x * x).sum::<f32>() / mono_data.len() as f32).sqrt();
            self.state.set_level(&self.device_type, rms);
        }

        // Use global recording timestamp for proper synchronization
        let timestamp = self.state.get_recording_duration().unwrap_or(0.0);

        // RAW AUDIO CHUNK: No gain applied - will be mixed and gained downstream
        // Use 48kHz if we resampled, otherwise use original rate
        let audio_chunk = AudioChunk {
            data: mono_data,  // Raw audio (resampled if needed), no gain yet
            sample_rate: if self.needs_resampling { 48000 } else { self.sample_rate },
            timestamp,
            chunk_id,
            device_type: self.device_type.clone(),
        };

        // NOTE: Raw audio is NOT sent to recording saver to prevent echo
        // Only the mixed audio (from AudioPipeline) is saved to file (see pipeline.rs:726-736)
        // This ensures we only record once: mic + system properly mixed
        // Individual raw streams go only to the transcription pipeline below

        // Send to processing pipeline for transcription
        if let Err(e) = self.state.send_audio_chunk(audio_chunk) {
            // Check if this is the "pipeline not ready" error
            if e.to_string().contains("Audio pipeline not ready") {
                // This is expected during initialization, just log it as debug
                debug!("Audio pipeline not ready yet, skipping chunk {}", chunk_id);
                return;
            }

            warn!("Failed to send audio chunk: {}", e);
            // More specific error handling based on failure reason
            let error = if e.to_string().contains("channel closed") {
                AudioError::ChannelClosed
            } else if e.to_string().contains("full") {
                AudioError::BufferOverflow
            } else {
                AudioError::ProcessingFailed
            };
            self.state.report_error(error);
        } else {
            debug!("Sent audio chunk {} ({} samples)", chunk_id, data.len());
        }
    }

    /// Handle stream errors with enhanced disconnect detection
    pub fn handle_stream_error(&self, error: cpal::StreamError) {
        error!("Audio stream error for {}: {}", self.device.name, error);

        let error_str = error.to_string().to_lowercase();

        // Enhanced error detection for device disconnection
        let audio_error = if error_str.contains("device is no longer available")
            || error_str.contains("device not found")
            || error_str.contains("device disconnected")
            || error_str.contains("no such device")
            || error_str.contains("device unavailable")
            || error_str.contains("device removed")
        {
            warn!("🔌 Device disconnect detected for: {}", self.device.name);
            AudioError::DeviceDisconnected
        } else if error_str.contains("permission") || error_str.contains("access denied") {
            AudioError::PermissionDenied
        } else if error_str.contains("channel closed") {
            AudioError::ChannelClosed
        } else if error_str.contains("stream") && error_str.contains("failed") {
            AudioError::StreamFailed
        } else {
            warn!("Unknown audio error: {}", error);
            AudioError::StreamFailed
        };

        self.state.report_error(audio_error);
    }
}

/// VAD-driven audio processing pipeline
/// Uses Voice Activity Detection to segment speech in real-time and send only speech to Whisper
pub struct AudioPipeline {
    receiver: mpsc::UnboundedReceiver<AudioChunk>,
    transcription_sender: mpsc::UnboundedSender<AudioChunk>,
    state: Arc<RecordingState>,
    // Per-stream VAD so the source device (mic vs system) is preserved on each
    // emitted speech segment. This gives transcripts a stable speaker tag and
    // leaves room for proper diarization later to refine the system stream.
    mic_vad_processor: ContinuousVadProcessor,
    sys_vad_processor: ContinuousVadProcessor,
    // EBU R128 normalizer applied to the SYSTEM stream *only for its VAD/transcription
    // copy* — a quiet remote participant otherwise gets VAD-gated out and mis-heard by
    // Whisper. The recording (ring-buffer mix) keeps the raw system audio. `None` if
    // construction fails (normalization simply skipped).
    sys_loudness_normalizer: Option<LoudnessNormalizer>,
    // Silences the speaker bleed-through that would otherwise reach the mic VAD
    // and get transcribed a second time under the "you" speaker tag. Applied to
    // the mic's VAD copy ONLY — the recording keeps the raw microphone. `None`
    // when suppression is disabled or the output is headphones.
    echo_suppressor: Option<EchoSuppressor>,
    sample_rate: u32,
    chunk_id_counter: u64,
    // Performance optimization: reduce logging frequency
    last_summary_time: std::time::Instant,
    processed_chunks: u64,
    // Smart batching for audio metrics
    metrics_batcher: Option<AudioMetricsBatcher>,
    // Ring buffer aligns the mic + system streams into equal-sized windows.
    ring_buffer: AudioMixerRingBuffer,
    // Recording sender for stereo audio (L = mic, R = system)
    recording_sender_for_mixed: Option<mpsc::UnboundedSender<AudioChunk>>,
}

impl AudioPipeline {
    pub fn new(
        receiver: mpsc::UnboundedReceiver<AudioChunk>,
        transcription_sender: mpsc::UnboundedSender<AudioChunk>,
        state: Arc<RecordingState>,
        target_chunk_duration_ms: u32,
        sample_rate: u32,
        mic_device_name: String,
        mic_device_kind: super::device_detection::InputDeviceKind,
        system_device_name: String,
        system_device_kind: super::device_detection::InputDeviceKind,
        echo_suppression: bool,
    ) -> anyhow::Result<Self> {
        // Log device characteristics for adaptive buffering
        info!("🎛️ AudioPipeline initializing with device characteristics:");
        info!("   Mic: '{}' ({:?}) - Buffer: {:?}",
              mic_device_name, mic_device_kind, mic_device_kind.buffer_timeout());
        info!("   System: '{}' ({:?}) - Buffer: {:?}",
              system_device_name, system_device_kind, system_device_kind.buffer_timeout());

        // Device kind information can be used for adaptive buffering in the future
        // For now, we log it for monitoring and potential optimization
        let _ = (mic_device_name, mic_device_kind, system_device_name, system_device_kind);

        // Create VAD processor with balanced redemption time for speech accumulation
        // The VAD processor now handles 48kHz->16kHz resampling internally
        // This bridges natural pauses without excessive fragmentation
        // For mac os core audio, 900ms, for windows 400ms seems good

        let redemption_time = if cfg!(target_os = "macos") { 400 } else { 400 };

        let mic_vad_processor = match ContinuousVadProcessor::new(sample_rate, redemption_time) {
            Ok(processor) => {
                info!("VAD-driven pipeline: per-stream VAD enabled (mic + system) for speaker tagging");
                processor
            }
            Err(e) => {
                error!("Failed to create mic VAD processor: {}", e);
                return Err(anyhow::anyhow!("Failed to create mic VAD processor: {}", e));
            }
        };

        let sys_vad_processor = match ContinuousVadProcessor::new(sample_rate, redemption_time) {
            Ok(processor) => processor,
            Err(e) => {
                error!("Failed to create system VAD processor: {}", e);
                return Err(anyhow::anyhow!("Failed to create system VAD processor: {}", e));
            }
        };

        // Loudness normalizer for the system stream's VAD/transcription copy only.
        let sys_loudness_normalizer = match LoudnessNormalizer::new(1, sample_rate) {
            Ok(n) => {
                info!("System-audio loudness normalizer enabled for VAD (target -23 LUFS)");
                Some(n)
            }
            Err(e) => {
                warn!("System-audio loudness normalizer unavailable ({e}); VAD uses raw system audio");
                None
            }
        };

        // Ring buffer aligns the mic + system streams into equal-sized windows.
        let ring_buffer = AudioMixerRingBuffer::new(sample_rate);

        // The ring buffer's aligned windows double as the reference signal for
        // echo detection, so suppression lives here rather than at capture time.
        let echo_suppressor = if echo_suppression {
            Some(EchoSuppressor::new(sample_rate))
        } else {
            info!("ℹ️ Echo suppression disabled — mic audio reaches the VAD unmodified");
            None
        };

        // Note: target_chunk_duration_ms is ignored - VAD controls segmentation now
        let _ = target_chunk_duration_ms;

        Ok(Self {
            receiver,
            transcription_sender,
            state,
            mic_vad_processor,
            sys_vad_processor,
            sys_loudness_normalizer,
            echo_suppressor,
            sample_rate,
            chunk_id_counter: 0,
            // Performance optimization: reduce logging frequency
            last_summary_time: std::time::Instant::now(),
            processed_chunks: 0,
            // Initialize metrics batcher for smart batching
            metrics_batcher: Some(AudioMetricsBatcher::new()),
            ring_buffer,
            recording_sender_for_mixed: None,  // Will be set by manager
        })
    }

    /// Run the VAD-driven audio processing pipeline
    pub async fn run(mut self) -> Result<()> {
        info!("VAD-driven audio pipeline started - segments sent in real-time based on speech detection");

        // CRITICAL FIX: Continue processing until channel is closed, not based on recording state
        // This ensures ALL chunks are processed during shutdown, fixing premature meeting completion
        // Previous bug: Loop checked `while self.state.is_recording()` which caused early exit when
        // stop_recording() was called, losing flush signals and remaining chunks in the pipeline
        loop {
            // Receive audio chunks with timeout
            match tokio::time::timeout(
                std::time::Duration::from_millis(50), // Shorter timeout for responsiveness
                self.receiver.recv()
            ).await {
                Ok(Some(chunk)) => {
                    // PERFORMANCE: Check for flush signal (special chunk with ID >= u64::MAX - 10)
                    // Multiple flush signals may be sent to ensure processing
                    if chunk.chunk_id >= u64::MAX - 10 {
                        info!("📥 Received FLUSH signal #{} - flushing VAD processor", u64::MAX - chunk.chunk_id);
                        self.flush_remaining_audio()?;
                        // Continue processing to handle any remaining chunks
                        continue;
                    }

                    // PERFORMANCE OPTIMIZATION: Eliminate per-chunk logging overhead
                    // Logging in hot paths causes severe performance degradation
                    self.processed_chunks += 1;

                    // Smart batching: collect metrics instead of logging every chunk
                    if let Some(ref batcher) = self.metrics_batcher {
                        let avg_level = chunk.data.iter().map(|&x| x.abs()).sum::<f32>() / chunk.data.len() as f32;
                        let duration_ms = chunk.data.len() as f64 / chunk.sample_rate as f64 * 1000.0;

                        batch_audio_metric!(
                            Some(batcher),
                            chunk.chunk_id,
                            chunk.data.len(),
                            duration_ms,
                            avg_level
                        );
                    }

                    // CRITICAL: Log summary only every 200 chunks OR every 60 seconds (99.5% reduction)
                    // This eliminates I/O overhead in the audio processing hot path
                    // Use performance-optimized debug macro that compiles to nothing in release builds
                    if self.processed_chunks % 200 == 0 || self.last_summary_time.elapsed().as_secs() >= 60 {
                        perf_debug!("Pipeline processed {} chunks, current chunk: {} ({} samples)",
                                   self.processed_chunks, chunk.chunk_id, chunk.data.len());
                        self.last_summary_time = std::time::Instant::now();
                    }

                    // STEP 1: Add raw audio to ring buffer for mixing
                    // Microphone audio is already normalized at capture level (AudioCapture)
                    // System audio remains raw
                    self.ring_buffer.add_samples(chunk.device_type.clone(), chunk.data);

                    // STEP 2: Align both streams into equal-sized synchronized windows.
                    while self.ring_buffer.can_mix() {
                        if let Some((mic_window, sys_window)) = self.ring_buffer.extract_window() {
                            // STEP 3: Run VAD per-stream so the device that produced each speech
                            // segment is preserved as a stable speaker tag downstream.
                            // Both VADs receive equal-sized synchronized windows from the ring
                            // buffer, so their internal sample timelines stay aligned.
                            // Speaker bleed-through is silenced before the mic VAD, so the
                            // remote participant's voice is not transcribed a second time
                            // under the "you" tag. The raw mic_window continues untouched
                            // to the stereo recording below.
                            let mic_gated = self
                                .echo_suppressor
                                .as_mut()
                                .map(|es| es.process_mic(&mic_window, &sys_window));
                            let mic_input: &[f32] = mic_gated.as_deref().unwrap_or(&mic_window);
                            let mic_vad_result = self.mic_vad_processor.process_audio(mic_input);
                            // System VAD + transcription get a loudness-normalized copy so a
                            // quiet remote participant isn't gated out; recording stays raw.
                            let sys_norm = self
                                .sys_loudness_normalizer
                                .as_mut()
                                .map(|n| n.normalize_loudness(&sys_window));
                            let sys_input: &[f32] = sys_norm.as_deref().unwrap_or(&sys_window);
                            let sys_vad_result = self.sys_vad_processor.process_audio(sys_input);

                            for (vad_result, source_device) in [
                                (mic_vad_result, DeviceType::Microphone),
                                (sys_vad_result, DeviceType::System),
                            ] {
                                match vad_result {
                                    Ok(speech_segments) => {
                                        for segment in speech_segments {
                                            let duration_ms = segment.end_timestamp_ms - segment.start_timestamp_ms;

                                            if segment.samples.len() >= 800 {
                                                info!("📤 [{:?}] Sending VAD segment: {:.1}ms, {} samples",
                                                      source_device, duration_ms, segment.samples.len());

                                                let transcription_chunk = AudioChunk {
                                                    data: segment.samples,
                                                    sample_rate: 16000,
                                                    timestamp: segment.start_timestamp_ms / 1000.0,
                                                    chunk_id: self.chunk_id_counter,
                                                    device_type: source_device.clone(),
                                                };

                                                if let Err(e) = self.transcription_sender.send(transcription_chunk) {
                                                    warn!("Failed to send VAD segment: {}", e);
                                                } else {
                                                    self.chunk_id_counter += 1;
                                                }
                                            } else {
                                                debug!("⏭️ [{:?}] Dropping short VAD segment: {:.1}ms ({} samples < 800)",
                                                       source_device, duration_ms, segment.samples.len());
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        warn!("⚠️ [{:?}] VAD error: {}", source_device, e);
                                    }
                                }
                            }

                            // STEP 4: Write the recording as STEREO with the two sources
                            // kept on separate channels — Left = microphone ("you"),
                            // Right = system ("them"). Persisting channel origin on disk
                            // lets any later re-process recover the "me vs them" split
                            // deterministically from channel index instead of guessing it
                            // acoustically. A mono/mixed playback is always recoverable by
                            // downmixing L+R.
                            if let Some(ref sender) = self.recording_sender_for_mixed {
                                let frames = mic_window.len().max(sys_window.len());
                                let mut stereo = Vec::with_capacity(frames * 2);
                                for i in 0..frames {
                                    stereo.push(mic_window.get(i).copied().unwrap_or(0.0)); // L = mic
                                    stereo.push(sys_window.get(i).copied().unwrap_or(0.0)); // R = system
                                }
                                let recording_chunk = AudioChunk {
                                    data: stereo,
                                    sample_rate: self.sample_rate,
                                    timestamp: chunk.timestamp,
                                    chunk_id: self.chunk_id_counter,
                                    device_type: DeviceType::Microphone, // unused by the stereo saver
                                };
                                let _ = sender.send(recording_chunk);
                            }
                        }
                    }
                }
                Ok(None) => {
                    info!("Audio pipeline: sender closed after processing {} chunks", self.processed_chunks);
                    break;
                }
                Err(_) => {
                    // Timeout - just continue, VAD handles all segmentation
                    continue;
                }
            }
        }

        // Flush any remaining VAD segments
        self.flush_remaining_audio()?;

        // Surface what echo suppression actually did — the single most useful
        // number when a user reports either duplicated speech (too little gating)
        // or missing speech (too much).
        if let Some(ref es) = self.echo_suppressor {
            let stats = es.stats();
            info!(
                "🔁 Echo suppression summary: {}/{} frames gated ({:.1}%), delay={}, coupling={}",
                stats.frames_gated,
                stats.frames_total,
                stats.gated_ratio() * 100.0,
                stats
                    .delay_ms
                    .map(|d| format!("{d:.1}ms"))
                    .unwrap_or_else(|| "not locked".to_string()),
                stats
                    .coupling_gain
                    .map(|g| format!("{g:.3}"))
                    .unwrap_or_else(|| "not learned".to_string()),
            );
        }

        info!("VAD-driven audio pipeline ended");
        Ok(())
    }

    fn flush_remaining_audio(&mut self) -> Result<()> {
        info!("Flushing remaining audio from pipeline (processed {} chunks)", self.processed_chunks);

        // 3C.2: drain the trailing partial window (the final <600ms, often the last
        // word) from the ring buffer and push it through the VADs + recording BEFORE
        // flushing. Idempotent — drain_partial yields None once the buffer is empty.
        if let Some((mic_partial, sys_partial)) = self.ring_buffer.drain_partial() {
            // Same echo gating as the steady-state path; the raw mic_partial below
            // still reaches the recording faithfully.
            let mic_gated = self
                .echo_suppressor
                .as_mut()
                .map(|es| es.process_mic(&mic_partial, &sys_partial));
            let mic_input: &[f32] = mic_gated.as_deref().unwrap_or(&mic_partial);
            let mic_vad_result = self.mic_vad_processor.process_audio(mic_input);
            // Normalize a copy for the system VAD only; the raw sys_partial below still
            // goes to the recording faithfully.
            let sys_norm = self
                .sys_loudness_normalizer
                .as_mut()
                .map(|n| n.normalize_loudness(&sys_partial));
            let sys_input: &[f32] = sys_norm.as_deref().unwrap_or(&sys_partial);
            let sys_vad_result = self.sys_vad_processor.process_audio(sys_input);
            for (vad_result, source_device) in [
                (mic_vad_result, DeviceType::Microphone),
                (sys_vad_result, DeviceType::System),
            ] {
                if let Ok(speech_segments) = vad_result {
                    for segment in speech_segments {
                        if segment.samples.len() >= 800 {
                            let transcription_chunk = AudioChunk {
                                data: segment.samples,
                                sample_rate: 16000,
                                timestamp: segment.start_timestamp_ms / 1000.0,
                                chunk_id: self.chunk_id_counter,
                                device_type: source_device.clone(),
                            };
                            if self.transcription_sender.send(transcription_chunk).is_ok() {
                                self.chunk_id_counter += 1;
                            }
                        }
                    }
                }
            }
            // Persist the partial to the recording as stereo (L = mic, R = system).
            if let Some(ref sender) = self.recording_sender_for_mixed {
                let frames = mic_partial.len().max(sys_partial.len());
                let mut stereo = Vec::with_capacity(frames * 2);
                for i in 0..frames {
                    stereo.push(mic_partial.get(i).copied().unwrap_or(0.0));
                    stereo.push(sys_partial.get(i).copied().unwrap_or(0.0));
                }
                let recording_chunk = AudioChunk {
                    data: stereo,
                    sample_rate: self.sample_rate,
                    timestamp: 0.0,
                    chunk_id: self.chunk_id_counter,
                    device_type: DeviceType::Microphone,
                };
                let _ = sender.send(recording_chunk);
            }
        }

        // Flush both VAD processors so trailing speech from either stream is preserved
        for (flush_result, source_device) in [
            (self.mic_vad_processor.flush(), DeviceType::Microphone),
            (self.sys_vad_processor.flush(), DeviceType::System),
        ] {
            match flush_result {
                Ok(final_segments) => {
                    for segment in final_segments {
                        let duration_ms = segment.end_timestamp_ms - segment.start_timestamp_ms;

                        if segment.samples.len() >= 800 {
                            info!("📤 [{:?}] Sending final VAD segment to Whisper: {:.1}ms duration, {} samples",
                                  source_device, duration_ms, segment.samples.len());

                            let transcription_chunk = AudioChunk {
                                data: segment.samples,
                                sample_rate: 16000,
                                timestamp: segment.start_timestamp_ms / 1000.0,
                                chunk_id: self.chunk_id_counter,
                                device_type: source_device.clone(),
                            };

                            if let Err(e) = self.transcription_sender.send(transcription_chunk) {
                                warn!("Failed to send final VAD segment: {}", e);
                            } else {
                                self.chunk_id_counter += 1;
                            }
                        } else {
                            info!("⏭️ [{:?}] Skipping short final segment: {:.1}ms ({} samples < 800)",
                                  source_device, duration_ms, segment.samples.len());
                        }
                    }
                }
                Err(e) => {
                    warn!("⚠️ [{:?}] Failed to flush VAD processor: {}", source_device, e);
                }
            }
        }

        Ok(())
    }

}

/// Simple audio pipeline manager
pub struct AudioPipelineManager {
    pipeline_handle: Option<JoinHandle<Result<()>>>,
    audio_sender: Option<mpsc::UnboundedSender<AudioChunk>>,
}

impl AudioPipelineManager {
    pub fn new() -> Self {
        Self {
            pipeline_handle: None,
            audio_sender: None,
        }
    }

    /// Start the audio pipeline with device information for adaptive buffering
    pub fn start(
        &mut self,
        state: Arc<RecordingState>,
        transcription_sender: mpsc::UnboundedSender<AudioChunk>,
        target_chunk_duration_ms: u32,
        sample_rate: u32,
        recording_sender: Option<mpsc::UnboundedSender<AudioChunk>>,
        mic_device_name: String,
        mic_device_kind: super::device_detection::InputDeviceKind,
        system_device_name: String,
        system_device_kind: super::device_detection::InputDeviceKind,
        echo_suppression: bool,
    ) -> Result<()> {
        // Log device information for adaptive buffering
        info!("🎙️ Starting pipeline with device info:");
        info!("   Microphone: '{}' ({:?})", mic_device_name, mic_device_kind);
        info!("   System Audio: '{}' ({:?})", system_device_name, system_device_kind);
        info!("   Echo suppression: {}", if echo_suppression { "on" } else { "off" });

        // Create audio processing channel
        let (audio_sender, audio_receiver) = mpsc::unbounded_channel::<AudioChunk>();

        // Set sender in state for audio captures to use
        state.set_audio_sender(audio_sender.clone());

        // Create and start pipeline with device information for adaptive mixing
        let mut pipeline = AudioPipeline::new(
            audio_receiver,
            transcription_sender,
            state.clone(),
            target_chunk_duration_ms,
            sample_rate,
            mic_device_name,
            mic_device_kind,
            system_device_name,
            system_device_kind,
            echo_suppression,
        )?;

        // CRITICAL FIX: Connect recording sender to receive pre-mixed audio
        // This ensures both mic AND system audio are captured in recordings
        pipeline.recording_sender_for_mixed = recording_sender;

        let handle = tokio::spawn(async move {
            pipeline.run().await
        });

        self.pipeline_handle = Some(handle);
        self.audio_sender = Some(audio_sender);

        info!("Audio pipeline manager started with mixed audio recording");
        Ok(())
    }

    /// Stop the audio pipeline
    pub async fn stop(&mut self) -> Result<()> {
        // Drop the sender to close the pipeline
        self.audio_sender = None;

        // Wait for pipeline to finish
        if let Some(handle) = self.pipeline_handle.take() {
            match handle.await {
                Ok(result) => result,
                Err(e) => {
                    error!("Pipeline task failed: {}", e);
                    Ok(())
                }
            }
        } else {
            Ok(())
        }
    }

    /// Force immediate flush of accumulated audio and stop pipeline
    /// PERFORMANCE CRITICAL: Eliminates 30+ second shutdown delays
    pub async fn force_flush_and_stop(&mut self) -> Result<()> {
        info!("🚀 Force flushing pipeline - processing ALL accumulated audio immediately");

        // If we have a sender, send a special flush signal first
        if let Some(sender) = &self.audio_sender {
            // Create a special flush chunk to trigger immediate processing
            let flush_chunk = AudioChunk {
                data: vec![], // Empty data signals flush
                sample_rate: 16000,
                timestamp: 0.0,
                chunk_id: u64::MAX, // Special ID to indicate flush
                device_type: super::recording_state::DeviceType::Microphone,
            };

            if let Err(e) = sender.send(flush_chunk) {
                warn!("Failed to send flush signal: {}", e);
            } else {
                info!("📤 Sent flush signal to pipeline");

                // PERFORMANCE OPTIMIZATION: Reduced wait time from 50ms to 20ms
                // Pipeline should process flush signal very quickly
                tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;

                // Send multiple flush signals to ensure the pipeline catches it
                // This aggressive approach eliminates shutdown delay issues
                for i in 0..3 {
                    let additional_flush = AudioChunk {
                        data: vec![],
                        sample_rate: 16000,
                        timestamp: 0.0,
                        chunk_id: u64::MAX - (i as u64),
                        device_type: super::recording_state::DeviceType::Microphone,
                    };
                    let _ = sender.send(additional_flush);
                }

                info!("📤 Sent additional flush signals for reliability");
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            }
        }

        // Now stop normally
        self.stop().await
    }
}

impl Default for AudioPipelineManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_extracts_full_window_zero_padding_absent_system_audio() {
        let mut rb = AudioMixerRingBuffer::new(1000); // 600-sample window
        assert!(!rb.can_mix());

        rb.add_samples(DeviceType::Microphone, vec![0.5; 600]);
        assert!(rb.can_mix(), "one full channel is enough to mix");

        let (mic, sys) = rb.extract_window().expect("a window should be ready");
        assert_eq!(mic.len(), 600);
        assert_eq!(sys.len(), 600, "the short side is zero-padded to the window length");
        assert!(mic.iter().all(|&s| s == 0.5));
        assert!(sys.iter().all(|&s| s == 0.0), "absent system audio reads as silence");
        assert!(!rb.can_mix(), "the window was consumed");
    }

    #[test]
    fn ring_buffer_returns_none_until_a_window_is_available() {
        let mut rb = AudioMixerRingBuffer::new(1000);
        rb.add_samples(DeviceType::Microphone, vec![0.1; 100]); // < 600
        assert!(rb.extract_window().is_none());
    }

    #[test]
    fn ring_buffer_drops_oldest_beyond_capacity() {
        let mut rb = AudioMixerRingBuffer::new(1000); // window 600, max 4800
        rb.add_samples(DeviceType::Microphone, vec![1.0; 6000]);
        assert!(
            rb.mic_buffer.len() <= rb.max_buffer_size,
            "buffer must be capped at max_buffer_size, got {}",
            rb.mic_buffer.len()
        );
    }

    #[test]
    fn drain_partial_yields_remaining_zero_padded_then_none() {
        let mut rb = AudioMixerRingBuffer::new(1000); // 600-sample window
        assert!(rb.drain_partial().is_none(), "an empty buffer drains to None");

        // A sub-window of mic-only audio: too small to extract_window, but the
        // stop-time drain must still recover it.
        rb.add_samples(DeviceType::Microphone, vec![0.3; 200]);
        assert!(rb.extract_window().is_none(), "a partial window is not a full window");

        let (mic, sys) = rb.drain_partial().expect("partial drained");
        assert_eq!(mic.len(), 200);
        assert_eq!(sys.len(), 200, "the absent side is zero-padded to equal length");
        assert!(mic.iter().all(|&s| s == 0.3));
        assert!(sys.iter().all(|&s| s == 0.0));

        assert!(rb.drain_partial().is_none(), "draining is idempotent");
    }

    #[test]
    fn drain_partial_takes_only_the_remainder_after_full_windows() {
        let mut rb = AudioMixerRingBuffer::new(1000); // window 600
        rb.add_samples(DeviceType::Microphone, vec![1.0; 700]); // 1 full window + 100
        let _ = rb.extract_window().expect("first full window");
        let (mic, _sys) = rb.drain_partial().expect("100-sample remainder");
        assert_eq!(mic.len(), 100, "only the post-window remainder is drained");
    }

    // --- Microphone kill switch -------------------------------------------------
    //
    // These drive `AudioCapture::process_audio_data` directly — the callback the
    // cpal stream invokes — because the whole point of the failsafe is that it
    // acts *there*, before any downstream stage could leak a sample.

    use super::super::devices::configuration::DeviceType as DeviceKind;
    use super::super::devices::AudioDevice;

    /// A capture wired to a channel we can drain, standing in for a live stream.
    fn capture_for(
        device_type: DeviceType,
        state: Arc<RecordingState>,
    ) -> (AudioCapture, mpsc::UnboundedReceiver<AudioChunk>) {
        let (tx, rx) = mpsc::unbounded_channel::<AudioChunk>();
        state.set_audio_sender(tx);
        let device = Arc::new(AudioDevice::new("Test Device".to_string(), DeviceKind::Input));
        let capture = AudioCapture::new(
            device,
            state,
            48_000, // matches the pipeline, so no resampling path is involved
            1,
            device_type,
            None,
        );
        (capture, rx)
    }

    #[test]
    fn muting_stops_microphone_samples_at_the_capture_callback() {
        let state = RecordingState::new();
        state.start_recording().expect("recording starts");
        let (capture, mut rx) = capture_for(DeviceType::Microphone, state.clone());

        // Unmuted: audio flows.
        capture.process_audio_data(&[0.4f32; 4800]);
        assert!(rx.try_recv().is_ok(), "an unmuted mic must reach the pipeline");

        // Muted: nothing does, no matter how loud.
        state.set_mic_muted(true);
        for _ in 0..10 {
            capture.process_audio_data(&[0.9f32; 4800]);
        }
        assert!(
            rx.try_recv().is_err(),
            "not one microphone sample may pass while muted"
        );

        // Unmuting restores flow without needing a restart.
        state.set_mic_muted(false);
        capture.process_audio_data(&[0.4f32; 4800]);
        assert!(rx.try_recv().is_ok(), "unmuting resumes capture");
    }

    #[test]
    fn muting_the_microphone_leaves_system_audio_flowing() {
        // This is the difference from pausing: the meeting keeps being recorded,
        // only the microphone goes quiet.
        let state = RecordingState::new();
        state.start_recording().expect("recording starts");
        let (capture, mut rx) = capture_for(DeviceType::System, state.clone());

        state.set_mic_muted(true);
        capture.process_audio_data(&[0.4f32; 4800]);

        assert!(
            rx.try_recv().is_ok(),
            "system audio must keep recording while the mic is muted"
        );
    }

    #[test]
    fn muting_parks_the_level_meter_at_zero() {
        let state = RecordingState::new();
        state.start_recording().expect("recording starts");
        let (capture, _rx) = capture_for(DeviceType::Microphone, state.clone());

        capture.process_audio_data(&[0.5f32; 4800]);
        let (mic_rms, _) = state.get_levels();
        assert!(mic_rms > 0.0, "a live mic publishes a level");

        state.set_mic_muted(true);
        let (mic_rms, _) = state.get_levels();
        assert_eq!(mic_rms, 0.0, "the HUD must read muted immediately, not stale");
    }

    #[test]
    fn a_new_recording_always_starts_unmuted() {
        // A mute surviving into the next meeting would silently lose its whole
        // microphone track, so both ends of the lifecycle clear it.
        let state = RecordingState::new();
        state.start_recording().expect("recording starts");
        state.set_mic_muted(true);
        assert!(state.is_mic_muted());

        state.stop_recording();
        assert!(!state.is_mic_muted(), "stopping clears the kill switch");

        state.set_mic_muted(true);
        state.start_recording().expect("recording restarts");
        assert!(!state.is_mic_muted(), "starting clears the kill switch");
    }
}