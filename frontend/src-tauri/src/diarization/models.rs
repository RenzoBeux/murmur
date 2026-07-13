// Diarization model paths + lazy download.
//
// Two ONNX files are needed:
//   1. Pyannote segmentation 3.0 (~6 MB) — finds speaker-change boundaries.
//   2. WeSpeaker English VoxCeleb ResNet293 embedding (~109 MB) — extracts
//      per-segment voice prints (English-native, large-margin trained).
//
// Both are mirrored on the official sherpa-onnx Hugging Face / GitHub releases.
// The first time a recording is stopped with diarization enabled we download
// them into the same convention the Whisper engine uses
// (`<app_data_dir>/models/diarization/` in production, `./models/diarization`
// in dev). Progress is emitted on the `diarization-model-download-progress`
// Tauri event so the UI can show a modal.

use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use log::{info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::fs;
use tokio::io::AsyncWriteExt;

const SEGMENTATION_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2";
const SEGMENTATION_FILENAME: &str = "sherpa-onnx-pyannote-segmentation-3-0/model.onnx";
// Real `model.onnx` is ~6 MB. We just want a floor that catches zero-byte or
// truncated downloads — not pinning exact size in case upstream re-quantises.
const SEGMENTATION_MIN_BYTES: u64 = 1024 * 1024; // 1 MB

// WeSpeaker English VoxCeleb ResNet293 (large-margin) — ~109 MB, trained on
// English VoxCeleb. English-native embeddings separate English speakers far
// more cleanly than the previous bilingual zh+en CAM++ (which, being
// Mandarin-centric, blurred English voices and over-segmented into "too many
// speakers"). ResNet293_LM is the highest-accuracy English option in the
// sherpa-onnx zoo. Note the upstream release tag's typo: "recongition", not
// "recognition" — that is the actual GitHub release tag.
const EMBEDDING_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/wespeaker_en_voxceleb_resnet293_LM.onnx";
const EMBEDDING_FILENAME: &str = "wespeaker_en_voxceleb_resnet293_LM.onnx";
const EMBEDDING_MIN_BYTES: u64 = 60 * 1024 * 1024; // 60 MB floor (real ~109 MB)

// SHA-256 pins for integrity verification. The segmentation digest is of the
// downloaded `.tar.bz2` archive (verified before extraction); the embedding
// digest is of the `.onnx` file itself.
const SEGMENTATION_ARCHIVE_SHA256: &str =
    "24615ee884c897d9d2ba09bb4d30da6bb1b15e685065962db5b02e76e4996488";
const EMBEDDING_SHA256: &str =
    "f65dbc820e534eef64ae12d1e289e20244d60e60f7f00d7b092092b1c458be2e";

#[derive(Debug, Clone)]
pub struct DiarizationModelPaths {
    pub segmentation: PathBuf,
    pub embedding: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsStatus {
    pub segmentation_present: bool,
    pub embedding_present: bool,
    pub models_dir: String,
}

#[derive(Debug, Clone, Serialize)]
struct ProgressPayload<'a> {
    name: &'a str,
    downloaded: u64,
    total: u64,
    percent: u8,
}

pub fn diarization_models_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf> {
    if cfg!(debug_assertions) {
        let cwd =
            std::env::current_dir().map_err(|e| anyhow!("Failed to get current dir: {e}"))?;
        for candidate in [
            cwd.join("models").join("diarization"),
            cwd.join("../models").join("diarization"),
        ] {
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        return Ok(cwd.join("models").join("diarization"));
    }
    let base = app
        .path()
        .app_data_dir()
        .map_err(|e| anyhow!("Failed to resolve app_data_dir: {e}"))?;
    Ok(base.join("models").join("diarization"))
}

pub async fn status<R: Runtime>(app: &AppHandle<R>) -> Result<ModelsStatus> {
    let dir = diarization_models_dir(app)?;
    let seg = dir.join(SEGMENTATION_FILENAME);
    let emb = dir.join(EMBEDDING_FILENAME);
    Ok(ModelsStatus {
        segmentation_present: file_at_least(&seg, SEGMENTATION_MIN_BYTES).await,
        embedding_present: file_at_least(&emb, EMBEDDING_MIN_BYTES).await,
        models_dir: dir.to_string_lossy().to_string(),
    })
}

pub async fn ensure_models<R: Runtime>(app: &AppHandle<R>) -> Result<DiarizationModelPaths> {
    let dir = diarization_models_dir(app)?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .await
            .map_err(|e| anyhow!("Failed to create diarization models dir: {e}"))?;
    }

    let seg_path = dir.join(SEGMENTATION_FILENAME);
    let emb_path = dir.join(EMBEDDING_FILENAME);

    if !file_at_least(&seg_path, SEGMENTATION_MIN_BYTES).await {
        info!("Downloading pyannote segmentation model to {}", seg_path.display());
        if let Some(parent) = seg_path.parent() {
            fs::create_dir_all(parent).await.ok();
        }
        download_tar_bz2_member(
            app,
            "segmentation",
            SEGMENTATION_URL,
            &dir,
            SEGMENTATION_FILENAME,
            Some(SEGMENTATION_ARCHIVE_SHA256),
        )
        .await?;
        if !file_at_least(&seg_path, SEGMENTATION_MIN_BYTES).await {
            return Err(anyhow!(
                "Segmentation model missing after download: {}",
                seg_path.display()
            ));
        }
    }

    if !file_at_least(&emb_path, EMBEDDING_MIN_BYTES).await {
        info!("Downloading 3D-Speaker embedding model to {}", emb_path.display());
        download_file(app, "embedding", EMBEDDING_URL, &emb_path, Some(EMBEDDING_SHA256)).await?;
        if !file_at_least(&emb_path, EMBEDDING_MIN_BYTES).await {
            return Err(anyhow!(
                "Embedding model missing after download: {}",
                emb_path.display()
            ));
        }
    }

    Ok(DiarizationModelPaths {
        segmentation: seg_path,
        embedding: emb_path,
    })
}

async fn file_at_least(path: &std::path::Path, min: u64) -> bool {
    match fs::metadata(path).await {
        Ok(m) => m.is_file() && m.len() >= min,
        Err(_) => false,
    }
}

pub(crate) async fn download_file<R: Runtime>(
    app: &AppHandle<R>,
    name: &str,
    url: &str,
    dest: &std::path::Path,
    expected_sha256: Option<&str>,
) -> Result<()> {
    let client = Client::new();
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| anyhow!("Failed to start download: {e}"))?;
    if !response.status().is_success() {
        return Err(anyhow!("Download failed with status: {}", response.status()));
    }
    let total = response.content_length().unwrap_or(0);
    let mut file = fs::File::create(dest)
        .await
        .map_err(|e| anyhow!("Failed to create file {}: {e}", dest.display()))?;
    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_percent: u8 = 0;
    let _ = app.emit(
        "diarization-model-download-progress",
        &ProgressPayload {
            name,
            downloaded: 0,
            total,
            percent: 0,
        },
    );
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| anyhow!("Download stream error: {e}"))?;
        file.write_all(&bytes)
            .await
            .map_err(|e| anyhow!("Failed to write chunk: {e}"))?;
        downloaded += bytes.len() as u64;
        if total > 0 {
            let p = ((downloaded * 100) / total).min(100) as u8;
            if p != last_percent {
                last_percent = p;
                let _ = app.emit(
                    "diarization-model-download-progress",
                    &ProgressPayload {
                        name,
                        downloaded,
                        total,
                        percent: p,
                    },
                );
            }
        }
    }
    file.flush()
        .await
        .map_err(|e| anyhow!("Failed to flush: {e}"))?;
    // Verify integrity before the file is used. Deletes the file on mismatch.
    if let Some(expected) = expected_sha256 {
        crate::download_integrity::verify_sha256(dest, expected).await?;
    }
    Ok(())
}

async fn download_tar_bz2_member<R: Runtime>(
    app: &AppHandle<R>,
    name: &str,
    url: &str,
    extract_dir: &std::path::Path,
    expected_member: &str,
    archive_sha256: Option<&str>,
) -> Result<()> {
    let tmp = extract_dir.join(format!(".{name}.tar.bz2"));
    download_file(app, name, url, &tmp, archive_sha256).await?;

    let extract_owned = extract_dir.to_path_buf();
    let extract_for_closure = extract_owned.clone();
    let tmp_path = tmp.clone();
    let expected = expected_member.to_string();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let f = std::fs::File::open(&tmp_path)
            .map_err(|e| anyhow!("Failed to open archive: {e}"))?;
        let bz = bzip2::read::BzDecoder::new(f);
        let mut archive = tar::Archive::new(bz);
        archive
            .unpack(&extract_for_closure)
            .map_err(|e| anyhow!("Failed to extract archive: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| anyhow!("Extraction task panicked: {e}"))??;

    if let Err(e) = fs::remove_file(&tmp).await {
        warn!("Failed to remove archive {}: {e}", tmp.display());
    }
    let final_path = extract_owned.join(&expected);
    if !final_path.exists() {
        return Err(anyhow!(
            "Archive did not contain expected file: {}",
            expected
        ));
    }
    Ok(())
}
