use super::ffmpeg::find_ffmpeg_path; // Correct path to encode module
use super::AudioDevice;
use std::io::Write;
use std::sync::Arc;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};
use tracing::{debug, error};

pub struct AudioInput {
    pub data: Arc<Vec<f32>>,
    pub sample_rate: u32,
    pub channels: u16,
    pub device: Arc<AudioDevice>,
}

pub fn encode_single_audio(
    data: &[u8],
    sample_rate: u32,
    channels: u16,
    output_path: &PathBuf,
) -> anyhow::Result<()> {
    debug!("Starting FFmpeg process for {} bytes of audio data", data.len());

    if data.is_empty() {
        return Err(anyhow::anyhow!("No audio data provided for encoding"));
    }

    let ffmpeg_path = find_ffmpeg_path().ok_or_else(|| {
        anyhow::anyhow!("FFmpeg not found. Please install FFmpeg to save recordings.")
    })?;

    debug!("Using FFmpeg at: {:?}", ffmpeg_path);

    let mut command = Command::new(ffmpeg_path);
    command
        .args([
            "-f",
            "f32le",
            "-ar",
            &sample_rate.to_string(),
            "-ac",
            &channels.to_string(),
            "-i",
            "pipe:0",
            "-c:a",
            "aac",
            "-b:a",
            "192k", // Increased from 64k for better audio quality (especially for speech)
            "-profile:a",
            "aac_low", // Use AAC-LC profile for better compatibility
            "-movflags",
            "+faststart", // Optimize for web streaming
            "-f",
            "mp4",
            output_path.to_str().unwrap(),
        ])
        // stdout is unused (the mp4 goes to output_path) — null it rather than pipe
        // it so ffmpeg can never block on an undrained pipe we forgot to read.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    // Hide console window on Windows to prevent CMD popup during recording
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    debug!("FFmpeg command: {:?}", command);

    // Propagate spawn/stdin/wait failures as errors instead of panicking: this
    // function runs inside the accumulation task, and a panic here silently kills
    // that task (its JoinHandle is dropped) while the pipeline keeps feeding a dead
    // channel — the UI shows "recording" but nothing is persisted. Returning Err lets
    // the caller (incremental_saver -> recording_saver) surface a recording-error.
    #[allow(clippy::zombie_processes)]
    let mut ffmpeg = command
        .spawn()
        .map_err(|e| anyhow::anyhow!("Failed to spawn FFmpeg process: {}", e))?;
    debug!("FFmpeg process spawned");
    let mut stdin = ffmpeg
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to open FFmpeg stdin"))?;

    // Drain stderr on a separate thread WHILE we feed stdin. With multi-MB inputs
    // (a 30s checkpoint is ~11.5 MB) ffmpeg can fill its stderr pipe buffer before
    // we finish writing; if nobody reads it, ffmpeg blocks on stderr, we block on
    // stdin, and both sides deadlock forever — the recording then never finalizes.
    let stderr_pipe = ffmpeg
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("Failed to open FFmpeg stderr"))?;
    let stderr_reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        let mut pipe = stderr_pipe;
        let _ = pipe.read_to_string(&mut buf);
        buf
    });

    // write_all can now surface EPIPE if ffmpeg dies early — that's the existing
    // Err path (caller emits a recording-error) instead of a hang.
    stdin.write_all(data)?;

    debug!("Dropping stdin");
    drop(stdin);
    debug!("Waiting for FFmpeg process to exit");
    let status = ffmpeg
        .wait()
        .map_err(|e| anyhow::anyhow!("Failed to wait for FFmpeg process: {}", e))?;
    let stderr = stderr_reader.join().unwrap_or_default();

    debug!("FFmpeg process exited with status: {}", status);
    debug!("FFmpeg stderr: {}", stderr);

    if !status.success() {
        error!("FFmpeg process failed with status: {}", status);
        error!("FFmpeg stderr: {}", stderr);
        return Err(anyhow::anyhow!(
            "FFmpeg process failed with status: {}",
            status
        ));
    }

    Ok(())
}
