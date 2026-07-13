// "Local Pro" diarization: pyannote community-1 running fully locally in a
// Python sidecar process.
//
// Nothing is bundled with the installer. On first use we download the `uv`
// binary (~15 MB), and `uv run` provisions a cached Python environment with
// pyannote.audio plus a PyTorch build matched to the machine (CPU-only is
// ~1–2 GB; a CUDA wheel for GPU is larger, ~2.5–3.5 GB) plus the gated
// community-1 model from Hugging Face (requires the user's HF token).
// Everything lands under <app_data_dir>/diarization-pro/ so uninstalling the
// app removes it.
//
// The sidecar is spawned per job and prints segments JSON on stdout — being a
// subprocess, a native crash there costs one job, not the app (unlike the
// in-process sherpa-onnx path).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use log::{info, warn};
use tauri::{AppHandle, Manager, Runtime};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

use crate::diarization::engine::DiarSegment;
use crate::diarization::models::download_file;
use crate::diarization::remote::{map_remote_segments, RemoteSegment};

// Dependencies (pyannote.audio, soundfile) and the `torch-backend = "auto"`
// GPU selection live in the PEP 723 inline metadata at the top of
// localpro_diarize.py, so `uv run` resolves everything straight from the
// script — no --with flags needed here.
const PYTHON_VERSION: &str = "3.12";
const SIDECAR_SCRIPT: &str = include_str!("localpro_diarize.py");
// First run: ~1–2 GB env + model download, then CPU/MPS inference. Generous.
const JOB_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);

// Pinned uv release. `latest` is convenient but unverifiable; a fixed version
// lets us pin a per-triple SHA-256 (from the release's `.sha256` sidecars).
const UV_VERSION: &str = "0.11.28";

/// The pinned uv download URL and its expected SHA-256 for this build target.
fn uv_download() -> Result<(String, &'static str)> {
    let (asset, sha256): (&str, &str) = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        (
            "uv-x86_64-pc-windows-msvc.zip",
            "0a23463216d09c6a72ff80ef5dc5a795f07dc1575cb84d24596c2f124a441b7b",
        )
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        (
            "uv-aarch64-pc-windows-msvc.zip",
            "3248109afad3ec59baad299d324ff53de17e2d9a3b3e21580ffd26744b11e036",
        )
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        (
            "uv-aarch64-apple-darwin.tar.gz",
            "33540eb7c883ab857eff79bd5ac2aa31fe27b595abecb4a9c003a2c998447232",
        )
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        (
            "uv-x86_64-apple-darwin.tar.gz",
            "2ad79983127ffca7d77b77ce6a24278d7e4f7b817a1acf72fea5f8124b4aac5e",
        )
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        (
            "uv-x86_64-unknown-linux-gnu.tar.gz",
            "e490a6464492183c5d4534a5527fb4440f7f2bb2f228162ad7e4afe076dc0224",
        )
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        (
            "uv-aarch64-unknown-linux-gnu.tar.gz",
            "03e9fe0a81b0718d0bc84625de3885df6cc3f89a8b6af6121d6b9f6113fb6533",
        )
    } else {
        return Err(anyhow!("Local Pro diarization is not supported on this platform"));
    };
    let url = format!(
        "https://github.com/astral-sh/uv/releases/download/{}/{}",
        UV_VERSION, asset
    );
    Ok((url, sha256))
}

fn uv_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "uv.exe"
    } else {
        "uv"
    }
}

fn tool_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| anyhow!("Failed to resolve app data dir: {e}"))?
        .join("diarization-pro");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create {}", dir.display()))?;
    Ok(dir)
}

/// Extract the `uv` binary from a downloaded release archive (.zip on
/// Windows, .tar.gz elsewhere) into `dest`.
fn extract_uv(archive: &Path, dest: &Path) -> Result<()> {
    let wanted = uv_binary_name();
    let archive_name = archive.to_string_lossy();

    if archive_name.ends_with(".zip") {
        let file = std::fs::File::open(archive).context("Failed to open uv archive")?;
        let mut zip = zip::ZipArchive::new(file).context("Invalid uv zip archive")?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            if entry.name().rsplit('/').next() == Some(wanted) {
                let mut out = std::fs::File::create(dest)
                    .with_context(|| format!("Failed to create {}", dest.display()))?;
                std::io::copy(&mut entry, &mut out)?;
                return Ok(());
            }
        }
    } else {
        let file = std::fs::File::open(archive).context("Failed to open uv archive")?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(gz);
        for entry in tar.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.into_owned();
            if path.file_name().and_then(|n| n.to_str()) == Some(wanted) {
                let mut out = std::fs::File::create(dest)
                    .with_context(|| format!("Failed to create {}", dest.display()))?;
                std::io::copy(&mut entry, &mut out)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
                }
                return Ok(());
            }
        }
    }
    bail!("uv archive did not contain the {} binary", wanted)
}

/// Download the `uv` binary on first use. Emits the same
/// `diarization-model-download-progress` events the dialog already renders.
async fn ensure_uv<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf> {
    let dir = tool_dir(app)?;
    let uv_path = dir.join(uv_binary_name());
    if uv_path.exists() {
        return Ok(uv_path);
    }

    let (url, sha256) = uv_download()?;
    info!("Downloading uv {UV_VERSION} from {url}");
    let suffix = if url.ends_with(".zip") { "zip" } else { "tar.gz" };
    let archive = dir.join(format!(".uv-download.{suffix}"));
    download_file(app, "uv", &url, &archive, Some(sha256)).await?;

    let archive_clone = archive.clone();
    let uv_clone = uv_path.clone();
    tokio::task::spawn_blocking(move || extract_uv(&archive_clone, &uv_clone))
        .await
        .map_err(|e| anyhow!("uv extraction task panicked: {e}"))??;

    if let Err(e) = tokio::fs::remove_file(&archive).await {
        warn!("Failed to remove uv archive: {e}");
    }
    info!("uv ready at {}", uv_path.display());
    Ok(uv_path)
}

/// Write the embedded sidecar script into the tool dir (refreshed every run
/// so app updates propagate script changes).
fn ensure_script(dir: &Path) -> Result<PathBuf> {
    let script = dir.join("diarize.py");
    std::fs::write(&script, SIDECAR_SCRIPT)
        .with_context(|| format!("Failed to write {}", script.display()))?;
    Ok(script)
}

/// Parse the sidecar's stdout: the segments JSON is the last non-empty line
/// (libraries occasionally chat on stdout before it).
fn parse_segments_stdout(stdout: &str) -> Result<Vec<RemoteSegment>> {
    #[derive(serde::Deserialize)]
    struct SegmentsOutput {
        segments: Vec<RemoteSegment>,
    }

    let last_line = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .ok_or_else(|| anyhow!("Sidecar produced no output"))?;
    let parsed: SegmentsOutput = serde_json::from_str(last_line.trim())
        .with_context(|| format!("Sidecar output was not segments JSON: {last_line}"))?;
    Ok(parsed.segments)
}

/// Run community-1 diarization locally via the Python sidecar.
pub async fn run_local_pro<R: Runtime>(
    app: &AppHandle<R>,
    wav_path: &Path,
    num_speakers: Option<u32>,
    hf_token: &str,
    on_progress: &(dyn Fn(&str) + Send + Sync),
) -> Result<Vec<DiarSegment>> {
    on_progress("preparing-env");
    let uv = ensure_uv(app).await?;
    let dir = tool_dir(app)?;
    let script = ensure_script(&dir)?;

    let job_args = serde_json::json!({
        "wav_path": wav_path.to_string_lossy(),
        "num_speakers": num_speakers,
    })
    .to_string();

    // Everything uv/HF download is contained under the tool dir so an
    // uninstall (or manual cleanup) removes it.
    let mut cmd = Command::new(&uv);
    cmd.arg("run")
        .arg("--no-project")
        .arg("--python")
        .arg(PYTHON_VERSION)
        .arg(&script)
        .arg(&job_args)
        .env("HF_TOKEN", hf_token)
        .env("UV_CACHE_DIR", dir.join("uv-cache"))
        .env("UV_PYTHON_INSTALL_DIR", dir.join("python"))
        .env("HF_HOME", dir.join("hf-cache"))
        // On macOS the pipeline runs on the Metal (MPS) backend; if it hits an
        // op MPS hasn't implemented yet, fall back to CPU for that op instead
        // of crashing the job. No-op on Windows/Linux (torch ignores it when
        // not using MPS).
        .env("PYTORCH_ENABLE_MPS_FALLBACK", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    info!("Spawning Local Pro diarization sidecar (uv run; deps + torch-backend from inline script metadata)");
    let mut child = cmd.spawn().context(
        "Failed to start the local diarization sidecar. If this persists, delete the \
         'diarization-pro' folder in the app data directory and try again.",
    )?;

    // Stream stderr for logging and to flip the UI from "preparing" to
    // "running" once the pipeline actually starts inferring. The future is
    // polled via join! (not spawned), so borrowing `on_progress` is fine.
    let stderr = child.stderr.take();
    let stderr_task = async {
        let mut collected: Vec<String> = Vec::new();
        if let Some(stderr) = stderr {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                info!("[localpro] {line}");
                if line.starts_with("running on") {
                    on_progress("running");
                }
                collected.push(line);
                if collected.len() > 200 {
                    collected.remove(0);
                }
            }
        }
        collected
    };

    let mut stdout_pipe = child.stdout.take();
    let stdout_task = async {
        let mut buf = String::new();
        if let Some(stdout) = stdout_pipe.as_mut() {
            let _ = stdout.read_to_string(&mut buf).await;
        }
        buf
    };

    let run = async {
        let (stderr_tail, stdout) = tokio::join!(stderr_task, stdout_task);
        let status = child.wait().await.context("Sidecar process error")?;
        Ok::<_, anyhow::Error>((status, stdout, stderr_tail))
    };

    let (status, stdout, stderr_tail) = tokio::time::timeout(JOB_TIMEOUT, run)
        .await
        .map_err(|_| anyhow!("Local Pro diarization timed out after 2 hours"))??;

    if !status.success() {
        let tail = stderr_tail
            .iter()
            .rev()
            .take(5)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        if tail.contains("gated") || tail.contains("401") || tail.contains("could not load the pipeline") {
            bail!(
                "Hugging Face rejected the request. Make sure you accepted the model \
                 conditions at huggingface.co/pyannote/speaker-diarization-community-1 \
                 and that your token is valid.\n{tail}"
            );
        }
        bail!("Local Pro diarization failed (exit {status}):\n{tail}");
    }

    let segments = parse_segments_stdout(&stdout)?;
    info!("Local Pro diarization returned {} segments", segments.len());
    Ok(map_remote_segments(&segments))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_segments_stdout_takes_last_nonempty_line() {
        let stdout = "some library banner\n\n{\"segments\":[{\"speaker\":\"SPEAKER_01\",\"start\":1.0,\"end\":2.5}]}\n";
        let segments = parse_segments_stdout(stdout).unwrap();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].speaker, "SPEAKER_01");
        assert!((segments[0].end - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_segments_stdout_rejects_garbage() {
        assert!(parse_segments_stdout("").is_err());
        assert!(parse_segments_stdout("not json at all").is_err());
    }
}
