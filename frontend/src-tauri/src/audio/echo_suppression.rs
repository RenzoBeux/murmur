//! Acoustic echo detection for the microphone's transcription copy.
//!
//! On a laptop without headphones the remote participants' voices leave the
//! speakers and re-enter the microphone. Both streams then reach their own VAD
//! (see `pipeline.rs`), so the same sentence is transcribed twice — once tagged
//! `system` ("them") and once tagged `mic` ("you"). That corrupts speaker
//! attribution and inflates summaries, which is worse than an audio artifact.
//!
//! The ring buffer already hands the pipeline time-aligned, equal-length mic and
//! system windows — exactly the reference signal echo cancellation needs. This
//! module uses it to silence mic sub-frames that are explained by recently
//! played system audio, so they never reach the mic VAD.
//!
//! Only the VAD/transcription copy is touched. The stereo recording keeps the
//! raw microphone on its left channel, so nothing is destroyed on disk and a
//! re-transcription can always recover the original audio.
//!
//! # Why gating and not subtraction
//!
//! We only need to answer "is this frame the remote participant coming back at
//! us?", not "reconstruct clean near-end speech for a call". A normalized
//! cross-correlation against the delayed reference answers that question without
//! the stability risks of an adaptive filter, and it is inherently self-disabling:
//! on headphones there is no correlated component, so nothing ever gates.
//!
//! # Two-stage delay estimation
//!
//! Correlation against a single delayed copy only works if the delay is known to
//! within a sample or two — pre-emphasized speech decorrelates within a fraction
//! of a millisecond. Searching every lag over the plausible 300 ms range at
//! 48 kHz for every 20 ms frame is far too expensive, and searching thousands of
//! candidates on a short frame produces spurious peaks anyway.
//!
//! So the delay is found in two stages. A **coarse** stage correlates half a
//! second of 16× decimated audio across the whole range, roughly once a second;
//! the delay is a physical constant of the speaker/mic/buffering path, so it
//! does not need to be re-derived per frame. A **fine** stage then correlates the
//! full-rate frame across a handful of lags around that estimate. Few candidates
//! over a long frame keeps the noise floor of `max |r|` low, which is what makes
//! the threshold meaningful.

use std::collections::VecDeque;

use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

/// Sub-frame length for gating decisions. Short enough that a brief "yes" from
/// the user is not swallowed by a 600 ms window of remote speech.
const FRAME_MS: usize = 20;

/// Decimation factor for the coarse delay search. 48 kHz → 3 kHz, box-filtered,
/// which keeps the speech energy that matters for alignment and widens the
/// correlation peak enough to find it on a sparse grid.
const DECIM: usize = 16;

/// Pre-emphasis coefficient for the fine stage. Whitening sharpens the true-echo
/// peak and suppresses the correlation two unrelated speech signals show simply
/// because both are low-frequency dominated. A cheap stand-in for GCC-PHAT.
const PRE_EMPHASIS: f32 = 0.95;

/// Correlation at or above this marks a frame as echo once the coupling gain is
/// known. Real laptop echo (mic ~15 cm from the speaker) is direct-path
/// dominated and correlates well above this; unrelated speech sits far below.
const CORR_THRESHOLD_DEFAULT: f32 = 0.50;

/// Correlation high enough to be echo beyond doubt. Used to gate before the
/// coupling gain has been learned, and to pick the frames that gain is learned
/// from.
const CORR_HIGH_DEFAULT: f32 = 0.70;

/// Correlation required to keep gating through the hangover frames that follow a
/// confirmed echo frame.
const CORR_HANGOVER: f32 = 0.35;

/// Frames to keep gating after a confirmed echo frame, so the tail of an echoed
/// word is not re-opened by one ambiguous sub-frame.
const HANGOVER_FRAMES: usize = 2;

/// How far the mic may exceed the predicted echo before we call it double-talk
/// and let it through. The user speaking over the remote adds energy the
/// reference cannot explain.
const DOUBLE_TALK_MARGIN: f32 = 2.5;

/// System RMS below this cannot produce audible echo, so the mic passes through.
const SYS_ACTIVITY_FLOOR: f32 = 1e-4;

/// Smoothing for the learned speaker→mic coupling gain.
const COUPLING_EMA: f32 = 0.05;

/// Delay search bounds, in milliseconds. The lead side is nonzero because the
/// ring buffer aligns the two streams by *arrival*, and the mic path carries
/// extra buffering (high-pass, loudness normalizer) that can make the system
/// stream appear to arrive late.
const MAX_LAG_MS: usize = 240;
const MAX_LEAD_MS: usize = 60;

/// Decimated span correlated by the coarse stage, and how often it reruns.
const COARSE_SPAN_MS: usize = 500;
const COARSE_REFRESH_FRAMES: usize = 50; // ~1 s

/// Minimum coarse correlation before a delay estimate is trusted.
const COARSE_CORR_FLOOR: f32 = 0.25;

/// Full-rate refinement around the coarse estimate: half-width and step, in
/// samples at the pipeline's rate. Deliberately few candidates — every extra one
/// raises the correlation a purely random frame can reach.
const FINE_SPAN_SAMPLES: isize = 40;
const FINE_STEP_SAMPLES: isize = 2;

/// Crossfade applied at gate transitions so the VAD's onset detector does not
/// see a click where a frame was silenced.
const RAMP_MS: usize = 5;

/// User-facing control over echo suppression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EchoSuppressionMode {
    /// Engage unless the active output device is confidently headphones.
    Auto,
    /// Always engage.
    On,
    /// Never engage.
    Off,
}

impl Default for EchoSuppressionMode {
    fn default() -> Self {
        Self::Auto
    }
}

impl EchoSuppressionMode {
    /// Resolve the mode against the current output device. `output_is_headphones`
    /// is only consulted for `Auto`; being wrong there is safe in the engage
    /// direction, because the detector self-disables when nothing correlates.
    pub fn should_engage(self, output_is_headphones: bool) -> bool {
        match self {
            Self::On => true,
            Self::Off => false,
            Self::Auto => !output_is_headphones,
        }
    }
}

/// Diagnostics for a suppression session, logged when the pipeline stops.
#[derive(Debug, Clone, Copy)]
pub struct EchoStats {
    pub frames_total: u64,
    pub frames_gated: u64,
    pub delay_ms: Option<f32>,
    pub coupling_gain: Option<f32>,
}

impl EchoStats {
    pub fn gated_ratio(&self) -> f32 {
        if self.frames_total == 0 {
            0.0
        } else {
            self.frames_gated as f32 / self.frames_total as f32
        }
    }
}

/// Silences microphone frames that are explained by recently played system audio.
pub struct EchoSuppressor {
    sample_rate: u32,
    frame_len: usize,
    ramp_len: usize,

    /// Recent system samples at full rate — the reference the mic frames are
    /// compared against. Holds enough history to reach the deepest lag.
    sys_history: VecDeque<f32>,
    history_capacity: usize,

    /// 16× decimated histories of both streams, advanced in lockstep so index
    /// `i` in one is the same instant as index `i` in the other. The coarse
    /// delay search runs entirely on these.
    mic_decim: VecDeque<f32>,
    sys_decim: VecDeque<f32>,
    decim_capacity: usize,
    mic_decim_acc: (f32, usize),
    sys_decim_acc: (f32, usize),

    /// Speaker→mic delay in full-rate samples (positive = mic trails system).
    coarse_delay: Option<isize>,
    frames_since_coarse: usize,

    /// Learned speaker→mic coupling, as a ratio of RMS. Bootstrapped from frames
    /// that correlate beyond doubt.
    coupling_gain: Option<f32>,

    hangover: usize,
    prev_gain: f32,

    corr_threshold: f32,
    corr_high: f32,

    /// Scratch buffers reused across frames to keep the hot path allocation-free.
    scratch_mic: Vec<f32>,
    scratch_ref: Vec<f32>,

    frames_total: u64,
    frames_gated: u64,
}

impl EchoSuppressor {
    pub fn new(sample_rate: u32) -> Self {
        let per_ms = sample_rate as usize / 1000;
        let frame_len = (per_ms * FRAME_MS).max(1);
        let ramp_len = (per_ms * RAMP_MS).max(1);

        // The reference must reach back past the deepest lag from the *earliest*
        // frame of a window, so keep the lag range plus a second of slack.
        let history_capacity = per_ms * (MAX_LAG_MS + MAX_LEAD_MS) + sample_rate as usize;
        let decim_capacity =
            (per_ms * (COARSE_SPAN_MS + MAX_LAG_MS + MAX_LEAD_MS)) / DECIM + FRAME_MS;

        let corr_threshold = env_override("MURMUR_ECHO_CORR_THRESHOLD", CORR_THRESHOLD_DEFAULT);
        let corr_high = env_override("MURMUR_ECHO_CORR_HIGH", CORR_HIGH_DEFAULT).max(corr_threshold);

        info!(
            "🔁 Echo suppression enabled: frame={}ms, delay search -{}..{}ms, corr thresholds {:.2}/{:.2}",
            FRAME_MS, MAX_LEAD_MS, MAX_LAG_MS, corr_threshold, corr_high,
        );

        Self {
            sample_rate,
            frame_len,
            ramp_len,
            sys_history: VecDeque::with_capacity(history_capacity),
            history_capacity,
            mic_decim: VecDeque::with_capacity(decim_capacity),
            sys_decim: VecDeque::with_capacity(decim_capacity),
            decim_capacity,
            mic_decim_acc: (0.0, 0),
            sys_decim_acc: (0.0, 0),
            coarse_delay: None,
            frames_since_coarse: 0,
            coupling_gain: None,
            hangover: 0,
            prev_gain: 1.0,
            corr_threshold,
            corr_high,
            scratch_mic: Vec::with_capacity(frame_len),
            scratch_ref: Vec::with_capacity(frame_len),
            frames_total: 0,
            frames_gated: 0,
        }
    }

    /// Return the mic window with echo-dominated frames silenced.
    ///
    /// `mic` and `sys` are the time-aligned windows the ring buffer produced.
    /// They are always equal in length (both `extract_window` and `drain_partial`
    /// zero-pad the shorter side), and the returned vector matches `mic` so the
    /// VAD timeline is preserved regardless of what was gated.
    pub fn process_mic(&mut self, mic: &[f32], sys: &[f32]) -> Vec<f32> {
        // The reference must be in place before any frame consults it, so the
        // current window is reachable at zero and negative lag.
        self.sys_history.extend(sys.iter().copied());
        let base = self.sys_history.len() - sys.len();

        // Both decimated histories advance by the same count, which is what lets
        // the coarse search treat equal indices as the same instant.
        let n = mic.len().min(sys.len());
        decimate_into(&mic[..n], &mut self.mic_decim_acc, &mut self.mic_decim);
        decimate_into(&sys[..n], &mut self.sys_decim_acc, &mut self.sys_decim);
        trim_to(&mut self.mic_decim, self.decim_capacity);
        trim_to(&mut self.sys_decim, self.decim_capacity);

        let mut out = Vec::with_capacity(mic.len());
        let mut offset = 0;

        while offset < mic.len() {
            let flen = self.frame_len.min(mic.len() - offset);
            let mic_frame = &mic[offset..offset + flen];

            let gain = self.decide_frame_gain(mic_frame, base + offset, flen);
            self.append_ramped(&mut out, mic_frame, gain);

            offset += flen;
        }

        trim_to(&mut self.sys_history, self.history_capacity);
        out
    }

    /// Decide the gain for one mic frame: 1.0 to pass, 0.0 to silence.
    /// `ref_zero_lag` is the index in `sys_history` aligned with the frame.
    fn decide_frame_gain(&mut self, mic_frame: &[f32], ref_zero_lag: usize, flen: usize) -> f32 {
        self.frames_total += 1;

        self.frames_since_coarse += 1;
        if self.frames_since_coarse >= COARSE_REFRESH_FRAMES {
            self.frames_since_coarse = 0;
            self.refresh_coarse_delay();
        }

        let mic_rms = rms(mic_frame);

        // Whiten the mic frame once; every candidate lag is compared against it.
        self.scratch_mic.clear();
        pre_emphasize_into(mic_frame, &mut self.scratch_mic);

        let mut best_corr = 0.0f32;
        let mut best_ref_rms = 0.0f32;

        // Until the coarse stage converges there is no trustworthy alignment, so
        // test only zero lag. A wide blind sweep here would raise the noise floor
        // of `max |r|` above the threshold and gate real speech.
        let center = self.coarse_delay.unwrap_or(0);
        let mut lag = center - FINE_SPAN_SAMPLES;
        while lag <= center + FINE_SPAN_SAMPLES {
            let step = lag;
            lag += FINE_STEP_SAMPLES;

            let Some(start) = checked_ref_start(ref_zero_lag, step) else {
                continue;
            };
            if start + flen > self.sys_history.len() {
                continue;
            }

            self.scratch_ref.clear();
            self.scratch_ref
                .extend((0..flen).map(|i| self.sys_history.get(start + i).copied().unwrap_or(0.0)));

            let ref_rms = rms(&self.scratch_ref);
            if ref_rms < SYS_ACTIVITY_FLOOR {
                continue;
            }

            pre_emphasize_in_place(&mut self.scratch_ref);
            let corr = normalized_corr(&self.scratch_mic, &self.scratch_ref);
            if corr > best_corr {
                best_corr = corr;
                best_ref_rms = ref_rms;
            }
        }

        // Nothing was playing loudly enough to echo.
        if best_ref_rms < SYS_ACTIVITY_FLOOR {
            self.hangover = 0;
            return 1.0;
        }

        // Learn the coupling from frames that are echo beyond doubt.
        if best_corr >= self.corr_high {
            let observed = mic_rms / best_ref_rms;
            self.coupling_gain = Some(match self.coupling_gain {
                Some(g) => g * (1.0 - COUPLING_EMA) + observed * COUPLING_EMA,
                None => observed,
            });
        }

        let is_echo = match self.coupling_gain {
            // With the coupling known, a mic frame louder than the reference can
            // explain means the user is talking over the remote — let it through.
            Some(g) => {
                best_corr >= self.corr_threshold
                    && mic_rms <= g * best_ref_rms * DOUBLE_TALK_MARGIN
            }
            // Before bootstrap, only unambiguous correlation gates.
            None => best_corr >= self.corr_high,
        };

        if is_echo {
            self.hangover = HANGOVER_FRAMES;
            self.frames_gated += 1;
            0.0
        } else if self.hangover > 0 && best_corr >= CORR_HANGOVER {
            self.hangover -= 1;
            self.frames_gated += 1;
            0.0
        } else {
            self.hangover = 0;
            1.0
        }
    }

    /// Locate the speaker→mic delay by correlating half a second of decimated
    /// audio across the whole plausible range. Runs about once a second; the
    /// delay is a property of the hardware path, not of the moment.
    fn refresh_coarse_delay(&mut self) {
        let span = (self.sample_rate as usize / 1000 * COARSE_SPAN_MS) / DECIM;
        let max_lag = ((self.sample_rate as usize / 1000 * MAX_LAG_MS) / DECIM) as isize;
        let max_lead = ((self.sample_rate as usize / 1000 * MAX_LEAD_MS) / DECIM) as isize;

        let n = self.mic_decim.len().min(self.sys_decim.len());
        if n < span + max_lag as usize {
            return; // not enough history yet
        }

        // Compare the most recent `span` of mic against the same instants of
        // system audio, shifted by each candidate lag.
        let mic: Vec<f32> = self.mic_decim.iter().skip(n - span).copied().collect();
        let mic_start_abs = n - span;

        let mut best_corr = 0.0f32;
        let mut best_lag = 0isize;

        for lag in -max_lead..=max_lag {
            let sys_start = mic_start_abs as isize - lag;
            if sys_start < 0 || (sys_start as usize) + span > n {
                continue;
            }
            let sys_start = sys_start as usize;
            let sys: Vec<f32> = self
                .sys_decim
                .iter()
                .skip(sys_start)
                .take(span)
                .copied()
                .collect();

            let corr = normalized_corr(&mic, &sys);
            if corr > best_corr {
                best_corr = corr;
                best_lag = lag;
            }
        }

        if best_corr >= COARSE_CORR_FLOOR {
            let delay = best_lag * DECIM as isize;
            if self.coarse_delay != Some(delay) {
                debug!(
                    "🔁 Echo delay estimate: {:.1} ms (coarse corr {:.2})",
                    delay as f32 * 1000.0 / self.sample_rate as f32,
                    best_corr
                );
            }
            self.coarse_delay = Some(delay);
        }
    }

    /// Append the frame at `gain`, crossfading from the previous frame's gain so
    /// a silenced frame does not start with a click.
    fn append_ramped(&mut self, out: &mut Vec<f32>, frame: &[f32], gain: f32) {
        if (gain - self.prev_gain).abs() < f32::EPSILON {
            if gain >= 1.0 {
                out.extend_from_slice(frame);
            } else {
                out.extend(frame.iter().map(|s| s * gain));
            }
        } else {
            let ramp = self.ramp_len.min(frame.len());
            for (i, &s) in frame.iter().enumerate() {
                let g = if i < ramp {
                    let t = i as f32 / ramp as f32;
                    self.prev_gain + (gain - self.prev_gain) * t
                } else {
                    gain
                };
                out.push(s * g);
            }
        }
        self.prev_gain = gain;
    }

    pub fn stats(&self) -> EchoStats {
        EchoStats {
            frames_total: self.frames_total,
            frames_gated: self.frames_gated,
            delay_ms: self
                .coarse_delay
                .map(|d| d as f32 * 1000.0 / self.sample_rate as f32),
            coupling_gain: self.coupling_gain,
        }
    }
}

/// Window the offline helper feeds the suppressor, matching the ring buffer's
/// mixing window so the streaming and offline paths behave identically.
const OFFLINE_WINDOW_MS: usize = 600;

/// Run echo suppression over a complete, already-aligned pair of channels.
///
/// For the offline paths (import, retranscription), where a stereo recording's
/// left and right channels *are* the mic and system streams — written by the
/// pipeline from the same aligned windows, so the only remaining offset is the
/// acoustic one. Without this, re-transcribing a meeting reproduces exactly the
/// duplicate-speaker bug the live path now prevents.
///
/// Feeds the suppressor in windows rather than one huge call, so memory stays
/// bounded on multi-hour recordings.
pub fn suppress_echo_offline(mic: &[f32], sys: &[f32], sample_rate: u32) -> Vec<f32> {
    let window = (sample_rate as usize / 1000 * OFFLINE_WINDOW_MS).max(1);
    let mut es = EchoSuppressor::new(sample_rate);
    let mut out = Vec::with_capacity(mic.len());

    let mut offset = 0;
    while offset < mic.len() {
        let end = (offset + window).min(mic.len());
        let mic_win = &mic[offset..end];

        // Zero-pad the reference when the system channel is the shorter of the
        // two, mirroring what the ring buffer does live.
        let sys_win: Vec<f32> = (offset..end)
            .map(|i| sys.get(i).copied().unwrap_or(0.0))
            .collect();

        out.extend(es.process_mic(mic_win, &sys_win));
        offset = end;
    }

    let stats = es.stats();
    info!(
        "🔁 Offline echo suppression: {}/{} frames gated ({:.1}%), delay={}",
        stats.frames_gated,
        stats.frames_total,
        stats.gated_ratio() * 100.0,
        stats
            .delay_ms
            .map(|d| format!("{d:.1}ms"))
            .unwrap_or_else(|| "not locked".to_string()),
    );

    if looks_like_duplicated_channels(&stats) {
        warn!(
            "🔁 Offline echo suppression skipped: the two channels look like the same signal \
             (delay {:.1}ms, coupling {:.2}), not a microphone plus its echo. Returning the \
             channel unmodified rather than blanking it.",
            stats.delay_ms.unwrap_or(0.0),
            stats.coupling_gain.unwrap_or(0.0),
        );
        return mic.to_vec();
    }

    out
}

/// Distinguish "a microphone hearing its own speakers" from "two copies of one
/// signal". Real acoustic echo always carries some delay (speaker to mic, plus
/// output buffering) and arrives attenuated and room-filtered. A near-zero delay
/// at unity coupling across almost every frame means the channels are duplicates
/// — a downmixed or dual-mono file — and gating would silently blank one of them.
fn looks_like_duplicated_channels(stats: &EchoStats) -> bool {
    const NEAR_ZERO_DELAY_MS: f32 = 2.0;
    const UNITY_COUPLING: std::ops::RangeInclusive<f32> = 0.8..=1.25;

    stats.gated_ratio() > 0.9
        && stats.delay_ms.is_some_and(|d| d.abs() < NEAR_ZERO_DELAY_MS)
        && stats
            .coupling_gain
            .is_some_and(|g| UNITY_COUPLING.contains(&g))
}

fn env_override(key: &str, fallback: f32) -> f32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| (0.0..=1.0).contains(v))
        .unwrap_or(fallback)
}

/// Box-filter decimation by `DECIM`, carrying the partial bin across calls so
/// the two streams stay sample-for-sample in step.
fn decimate_into(src: &[f32], acc: &mut (f32, usize), out: &mut VecDeque<f32>) {
    for &s in src {
        acc.0 += s;
        acc.1 += 1;
        if acc.1 == DECIM {
            out.push_back(acc.0 / DECIM as f32);
            *acc = (0.0, 0);
        }
    }
}

fn trim_to(buf: &mut VecDeque<f32>, cap: usize) {
    while buf.len() > cap {
        buf.pop_front();
    }
}

/// `ref_zero_lag - lag`, rejecting the underflow a lead deeper than the
/// available history would produce.
fn checked_ref_start(ref_zero_lag: usize, lag: isize) -> Option<usize> {
    let start = ref_zero_lag as isize - lag;
    if start < 0 {
        None
    } else {
        Some(start as usize)
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|&s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

fn pre_emphasize_into(src: &[f32], dst: &mut Vec<f32>) {
    let mut prev = 0.0f32;
    for &s in src {
        dst.push(s - PRE_EMPHASIS * prev);
        prev = s;
    }
}

fn pre_emphasize_in_place(buf: &mut [f32]) {
    let mut prev = 0.0f32;
    for s in buf.iter_mut() {
        let cur = *s;
        *s = cur - PRE_EMPHASIS * prev;
        prev = cur;
    }
}

/// Magnitude of the normalized cross-correlation. Amplitude-invariant, so the
/// loudness normalization the mic path applies does not affect it.
fn normalized_corr(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let mut num = 0.0f32;
    let mut ea = 0.0f32;
    let mut eb = 0.0f32;
    for i in 0..n {
        num += a[i] * b[i];
        ea += a[i] * a[i];
        eb += b[i] * b[i];
    }
    if ea <= f32::EPSILON || eb <= f32::EPSILON {
        return 0.0;
    }
    (num / (ea.sqrt() * eb.sqrt())).abs().min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;
    /// The ring buffer's mixing window, the unit `process_mic` is really fed.
    const WINDOW: usize = (SR as usize * 600) / 1000;
    const FRAMES_PER_WINDOW: usize = WINDOW / ((SR as usize * FRAME_MS) / 1000);

    /// Deterministic noise, so the tests depend on neither a RNG crate nor
    /// run-to-run randomness.
    struct Lcg(u64);

    impl Lcg {
        fn new(seed: u64) -> Self {
            Self(seed)
        }
        fn next_f32(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 40) as f32 / (1u32 << 23) as f32) - 1.0
        }
    }

    /// Speech-like: band-limited noise under a slow amplitude envelope, so the
    /// coarse delay search has structure to lock onto.
    fn speechlike(len: usize, seed: u64) -> Vec<f32> {
        let mut lcg = Lcg::new(seed);
        let mut lp = 0.0f32;
        (0..len)
            .map(|i| {
                lp = lp * 0.7 + lcg.next_f32() * 0.3; // toward speech bandwidth
                let env = 0.4 + 0.6 * ((i as f32 / SR as f32) * 5.0).sin().abs();
                lp * env * 0.3
            })
            .collect()
    }

    /// Feed `rounds` ring-buffer windows, with the mic hearing `source` delayed
    /// by `lag` samples and attenuated, optionally plus local speech.
    fn run_echo_session(
        es: &mut EchoSuppressor,
        source: &[f32],
        lag: usize,
        rounds: usize,
        local: Option<(&[f32], f32)>,
    ) {
        for round in 0..rounds {
            let start = round * WINDOW;
            let sys = source[start..start + WINDOW].to_vec();
            let mic: Vec<f32> = (0..WINDOW)
                .map(|i| {
                    let abs = start + i;
                    let echo = if abs < lag { 0.0 } else { source[abs - lag] * 0.4 };
                    match local {
                        Some((sig, gain)) => echo + sig[round * WINDOW + i] * gain,
                        None => echo,
                    }
                })
                .collect();
            let out = es.process_mic(&mic, &sys);
            assert_eq!(out.len(), WINDOW);
        }
    }

    #[test]
    fn silent_system_never_gates_the_microphone() {
        let mut es = EchoSuppressor::new(SR);
        let mic = speechlike(WINDOW, 1);
        let sys = vec![0.0f32; WINDOW];

        let out = es.process_mic(&mic, &sys);

        assert_eq!(out.len(), mic.len());
        assert_eq!(out, mic, "with nothing playing, the mic must pass untouched");
        assert_eq!(es.stats().frames_gated, 0);
    }

    #[test]
    fn headphones_case_leaves_uncorrelated_microphone_alone() {
        // System audio is loud but reaches the ear only — the mic hears entirely
        // different content, which is what headphones look like acoustically.
        let mut es = EchoSuppressor::new(SR);

        for round in 0..10u64 {
            let mic = speechlike(WINDOW, 100 + round);
            let sys = speechlike(WINDOW, 900 + round);
            assert_eq!(es.process_mic(&mic, &sys).len(), WINDOW);
        }

        let stats = es.stats();
        assert!(
            stats.gated_ratio() < 0.05,
            "uncorrelated streams must not be gated, got {:.1}% of {} frames",
            stats.gated_ratio() * 100.0,
            stats.frames_total
        );
        assert!(
            stats.coupling_gain.is_none(),
            "no coupling should be learned when nothing echoes"
        );
    }

    #[test]
    fn delayed_attenuated_playback_is_recognized_as_echo() {
        let mut es = EchoSuppressor::new(SR);
        let lag = (SR as usize * 37) / 1000; // 37 ms — deliberately off any grid
        let rounds = 10;
        let source = speechlike(WINDOW * rounds + lag, 7);

        run_echo_session(&mut es, &source, lag, rounds, None);

        let stats = es.stats();
        assert!(
            stats.gated_ratio() > 0.7,
            "echo must be gated, only {:.1}% of {} frames were",
            stats.gated_ratio() * 100.0,
            stats.frames_total
        );
        let delay = stats.delay_ms.expect("the delay should have been found");
        assert!(
            (delay - 37.0).abs() < 2.0,
            "delay estimate {delay:.1} ms should land near the true 37 ms"
        );
    }

    #[test]
    fn double_talk_survives_the_gate() {
        let mut es = EchoSuppressor::new(SR);
        let lag = (SR as usize * 37) / 1000;
        let total_rounds = 14;
        let echo_only_rounds = 10;
        let source = speechlike(WINDOW * total_rounds + lag, 11);

        // Pure echo first, so the coupling gain is bootstrapped.
        run_echo_session(&mut es, &source, lag, echo_only_rounds, None);
        assert!(
            es.stats().coupling_gain.is_some(),
            "the coupling gain should be bootstrapped from the pure-echo rounds"
        );

        // Now the user talks over the remote: the same echo plus a much louder
        // local voice the reference cannot explain.
        let gated_before = es.stats().frames_gated;
        let local = speechlike(WINDOW * total_rounds, 23);
        let mut es_dt = es;
        for round in echo_only_rounds..total_rounds {
            let start = round * WINDOW;
            let sys = source[start..start + WINDOW].to_vec();
            let mic: Vec<f32> = (0..WINDOW)
                .map(|i| {
                    let abs = start + i;
                    let echo = if abs < lag { 0.0 } else { source[abs - lag] * 0.4 };
                    echo + local[abs] * 5.0
                })
                .collect();
            es_dt.process_mic(&mic, &sys);
        }

        let dt_rounds = total_rounds - echo_only_rounds;
        let gated_during = es_dt.stats().frames_gated - gated_before;
        let dt_frames = FRAMES_PER_WINDOW * dt_rounds;
        assert!(
            (gated_during as f32) < 0.25 * dt_frames as f32,
            "double-talk must reach the VAD, but {gated_during} of {dt_frames} frames were gated"
        );
    }

    #[test]
    fn output_length_always_matches_input() {
        let mut es = EchoSuppressor::new(SR);
        // A trailing partial window, as drain_partial produces at stop.
        let mic = speechlike(1234, 3);
        let sys = speechlike(1234, 4);
        assert_eq!(es.process_mic(&mic, &sys).len(), 1234);
    }

    /// The offline paths run at Whisper's 16 kHz, a different regime from the
    /// pipeline's 48 kHz: coarser decimation, fewer samples per frame.
    #[test]
    fn offline_helper_gates_echo_at_whisper_rate() {
        const SR16: u32 = 16_000;
        let win = (SR16 as usize * 600) / 1000;
        let lag = (SR16 as usize * 37) / 1000;
        let rounds = 14;

        // Speech-like at 16 kHz: same shape, generated at this rate.
        let mut lcg = Lcg::new(31);
        let mut lp = 0.0f32;
        let source: Vec<f32> = (0..win * rounds + lag)
            .map(|i| {
                lp = lp * 0.7 + lcg.next_f32() * 0.3;
                let env = 0.4 + 0.6 * ((i as f32 / SR16 as f32) * 5.0).sin().abs();
                lp * env * 0.3
            })
            .collect();

        let sys: Vec<f32> = source[..win * rounds].to_vec();
        let mic: Vec<f32> = (0..win * rounds)
            .map(|i| if i < lag { 0.0 } else { source[i - lag] * 0.4 })
            .collect();

        let out = suppress_echo_offline(&mic, &sys, SR16);
        assert_eq!(out.len(), mic.len());

        // Almost everything should have been silenced; compare total energy.
        let before: f64 = mic.iter().map(|&s| (s * s) as f64).sum();
        let after: f64 = out.iter().map(|&s| (s * s) as f64).sum();
        assert!(
            after < before * 0.2,
            "offline echo should be largely removed: {after:.4} vs {before:.4}"
        );
    }

    #[test]
    fn offline_helper_leaves_a_headphones_recording_alone() {
        const SR16: u32 = 16_000;
        let n = (SR16 as usize * 600 / 1000) * 12;
        let mic = speechlike(n, 41);
        let sys = speechlike(n, 42);

        let out = suppress_echo_offline(&mic, &sys, SR16);

        assert_eq!(out.len(), mic.len());
        let before: f64 = mic.iter().map(|&s| (s * s) as f64).sum();
        let after: f64 = out.iter().map(|&s| (s * s) as f64).sum();
        assert!(
            after > before * 0.99,
            "uncorrelated channels must survive intact: {after:.4} vs {before:.4}"
        );
    }

    #[test]
    fn identical_channels_are_left_alone_rather_than_blanked() {
        // A dual-mono or downmixed stereo file: both channels carry the same
        // signal at zero delay. That is duplication, not a room echo, and
        // blanking one channel would destroy the import.
        const SR16: u32 = 16_000;
        let n = (SR16 as usize * 600 / 1000) * 14;
        let both = speechlike(n, 55);

        let out = suppress_echo_offline(&both, &both, SR16);

        assert_eq!(out, both, "duplicated channels must survive untouched");
    }

    #[test]
    fn duplicate_channel_guard_only_fires_on_zero_delay_unity_coupling() {
        let dup = EchoStats {
            frames_total: 100,
            frames_gated: 99,
            delay_ms: Some(0.0),
            coupling_gain: Some(1.0),
        };
        assert!(looks_like_duplicated_channels(&dup));

        // Real echo: same heavy gating, but delayed and attenuated.
        let echo = EchoStats {
            delay_ms: Some(37.0),
            coupling_gain: Some(0.4),
            ..dup
        };
        assert!(!looks_like_duplicated_channels(&echo));

        // Zero delay but attenuated — still echo-like, not duplication.
        let quiet = EchoStats {
            delay_ms: Some(0.5),
            coupling_gain: Some(0.3),
            ..dup
        };
        assert!(!looks_like_duplicated_channels(&quiet));

        // Duplicate-looking stats but hardly anything was gated.
        let sparse = EchoStats {
            frames_gated: 10,
            ..dup
        };
        assert!(!looks_like_duplicated_channels(&sparse));
    }

    #[test]
    fn mode_resolves_against_the_output_device() {
        assert!(EchoSuppressionMode::On.should_engage(true));
        assert!(!EchoSuppressionMode::Off.should_engage(false));
        assert!(EchoSuppressionMode::Auto.should_engage(false));
        assert!(!EchoSuppressionMode::Auto.should_engage(true));
    }
}
