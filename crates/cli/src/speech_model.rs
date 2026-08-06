//! Provision the pinned local speech model used by generated voice apps.
//!
//! The model is downloaded once into Krate's cache, verified by SHA-256, then
//! copied into the generated app's assets. The resulting `.krate` stays
//! self-contained and performs recognition locally on the recipient's machine.

use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

pub const MODEL_ASSET_PATH: &str = "models/ggml-tiny.en.bin";

const MODEL_FILENAME: &str = "ggml-tiny.en.bin";
const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin";
const MODEL_SHA256: &str = "921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f";
const MODEL_BYTES: u64 = 77_704_715;
const MAX_DOWNLOAD_BYTES: u64 = 96 * 1024 * 1024;

pub fn provision(app_dir: &Path, quiet: bool) -> Result<PathBuf> {
    let destination = app_dir.join("assets").join(MODEL_ASSET_PATH);
    if destination.is_file() {
        return Ok(destination);
    }

    let cache = cache_dir()?.join("models").join(MODEL_FILENAME);
    if !verified_model(&cache)? {
        if !quiet {
            println!("==> downloading the local speech model once (about 75 MB)");
        }
        download_verified_model(&cache)?;
    }

    let parent = destination
        .parent()
        .context("speech model destination has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create speech asset directory {}", parent.display()))?;
    fs::copy(&cache, &destination).with_context(|| {
        format!(
            "copy the verified speech model from {} to {}",
            cache.display(),
            destination.display()
        )
    })?;
    Ok(destination)
}

fn cache_dir() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("KRATE_CACHE_DIR") {
        return Ok(PathBuf::from(root));
    }
    #[cfg(target_os = "windows")]
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(root).join("Krate").join("cache"));
    }
    if let Some(home) = crate::home_dir() {
        return Ok(home.join(".cache").join("krate"));
    }
    Ok(std::env::temp_dir().join("krate-cache"))
}

fn download_verified_model(destination: &Path) -> Result<()> {
    let parent = destination
        .parent()
        .context("speech model cache path has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create speech model cache {}", parent.display()))?;
    let partial = parent.join(format!("{MODEL_FILENAME}.download"));

    let response = ureq::get(MODEL_URL)
        .call()
        .with_context(|| format!("download the local speech model from {MODEL_URL}"))?;
    let mut reader = response.into_reader();
    let mut output = File::create(&partial)
        .with_context(|| format!("create temporary model {}", partial.display()))?;
    let mut hasher = Sha256::new();
    let mut written = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .context("read the local speech model download")?;
        if count == 0 {
            break;
        }
        written = written
            .checked_add(count as u64)
            .context("speech model size overflow")?;
        if written > MAX_DOWNLOAD_BYTES {
            let _ = fs::remove_file(&partial);
            anyhow::bail!("the speech model download exceeded 96 MB");
        }
        output
            .write_all(&buffer[..count])
            .with_context(|| format!("write temporary model {}", partial.display()))?;
        hasher.update(&buffer[..count]);
    }
    output
        .sync_all()
        .with_context(|| format!("finish temporary model {}", partial.display()))?;

    let digest = format!("{:x}", hasher.finalize());
    if written != MODEL_BYTES || digest != MODEL_SHA256 {
        let _ = fs::remove_file(&partial);
        anyhow::bail!(
            "the speech model failed integrity verification; expected {MODEL_BYTES} bytes and \
             SHA-256 {MODEL_SHA256}, received {written} bytes and {digest}"
        );
    }

    if destination.exists() {
        fs::remove_file(destination)
            .with_context(|| format!("replace stale model {}", destination.display()))?;
    }
    fs::rename(&partial, destination).with_context(|| {
        format!(
            "move verified model from {} to {}",
            partial.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn verified_model(path: &Path) -> Result<bool> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("inspect model {}", path.display()));
        }
    };
    if !metadata.is_file() || metadata.len() != MODEL_BYTES {
        return Ok(false);
    }

    let mut file =
        File::open(path).with_context(|| format!("open cached model {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .with_context(|| format!("read cached model {}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()) == MODEL_SHA256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_same_named_but_unverified_model() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(MODEL_FILENAME);
        fs::write(&path, b"not a model").expect("fake model");
        assert!(!verified_model(&path).expect("verify fake model"));
    }

    #[test]
    fn model_asset_path_matches_the_voice_template() {
        assert_eq!(MODEL_ASSET_PATH, "models/ggml-tiny.en.bin");
    }
}
