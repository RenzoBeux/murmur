//! Streaming SHA-256 integrity verification for downloaded artifacts.
//!
//! Every model/binary download in this app historically verified only by file
//! *size* (a floor with some tolerance). This module adds cryptographic pinning:
//! callers compare each finished download against a known-good SHA-256 and
//! delete + reject on mismatch, so a compromised mirror, tampered release asset,
//! or MITM cannot feed the app a malicious ONNX/GGUF model or executable.

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// One entry in a per-source download manifest: the file that should be present
/// and its expected lowercase-hex SHA-256.
///
/// `sha256 == None` means "not pinned yet" — used only where an authoritative
/// digest can't be sourced (e.g. an uncontrolled host). Callers must treat a
/// `None` as an explicit, logged gap, never as "verified".
#[derive(Debug, Clone)]
pub struct ExpectedArtifact {
    pub filename: &'static str,
    pub sha256: Option<&'static str>,
}

/// Compute the SHA-256 of a file, streaming it in fixed-size chunks so memory
/// stays constant regardless of file size. The blocking I/O + hashing runs on a
/// dedicated thread so it never stalls the async runtime.
pub async fn sha256_file(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || sha256_file_blocking(&path))
        .await
        .context("hashing task panicked")?
}

fn sha256_file_blocking(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("open {} for hashing", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Verify a downloaded file against an expected SHA-256 (case-insensitive hex,
/// with an optional leading `sha256:`).
///
/// On mismatch the file is **deleted** — so a partial or tampered artifact can't
/// be picked up later by a size-only check — and an `Err` is returned. A
/// malformed expectation errors *before* hashing and leaves the file untouched.
pub async fn verify_sha256(path: impl AsRef<Path>, expected_hex: &str) -> Result<()> {
    let path = path.as_ref();
    let expected = normalize_hex(expected_hex);
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(anyhow!(
            "invalid expected SHA-256 (must be 64 hex chars): {:?}",
            expected_hex
        ));
    }
    let actual = sha256_file(path).await?;
    if actual == expected {
        return Ok(());
    }
    // Delete the bad file so nothing downstream can load it.
    let _ = std::fs::remove_file(path);
    Err(anyhow!(
        "SHA-256 mismatch for {}: expected {}, got {} (file deleted)",
        path.display(),
        expected,
        actual
    ))
}

fn normalize_hex(s: &str) -> String {
    let lower = s.trim().to_ascii_lowercase();
    lower.trim_start_matches("sha256:").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Known SHA-256 test vectors (FIPS 180-2).
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    // sha256("abc")
    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("murmur_dlint_{}_{}", std::process::id(), name))
    }

    #[tokio::test]
    async fn hashes_empty_file() {
        let p = tmp("empty");
        std::fs::write(&p, b"").unwrap();
        assert_eq!(sha256_file(&p).await.unwrap(), EMPTY_SHA256);
        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn verifies_known_content_case_insensitive_and_keeps_file() {
        let p = tmp("abc");
        std::fs::write(&p, b"abc").unwrap();
        verify_sha256(&p, ABC_SHA256).await.unwrap();
        // Uppercase + `sha256:` prefix must still match.
        verify_sha256(&p, &format!("SHA256:{}", ABC_SHA256.to_uppercase()))
            .await
            .unwrap();
        assert!(p.exists(), "a verified file must be kept");
        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn mismatch_errors_and_deletes_file() {
        let p = tmp("bad");
        std::fs::write(&p, b"abc").unwrap();
        let err = verify_sha256(&p, EMPTY_SHA256).await.unwrap_err();
        assert!(err.to_string().contains("SHA-256 mismatch"));
        assert!(!p.exists(), "a mismatched file must be deleted");
    }

    #[tokio::test]
    async fn rejects_malformed_expected_hex_without_touching_file() {
        let p = tmp("malformed");
        std::fs::write(&p, b"abc").unwrap();
        assert!(verify_sha256(&p, "not-a-hash").await.is_err());
        assert!(p.exists(), "a malformed expectation must not delete the file");
        let _ = std::fs::remove_file(&p);
    }
}
