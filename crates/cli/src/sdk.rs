//! The embedded guest SDK, materialized on demand.
//!
//! `krate create` builds a generated app, and that app's `Cargo.toml` points at
//! the Krate WIT interfaces and the Rust bindings crate. Rather than require a
//! repo checkout, the binary carries the SDK inside it (see `build.rs`) and
//! writes it to a per-version cache directory the first time it is needed.
//! Later runs reuse the same directory.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

// The build script embeds `EMBEDDED_SDK: &[(&str, &[u8])]`.
include!(concat!(env!("OUT_DIR"), "/embedded_sdk.rs"));

/// The SDK version tag the cache directory is keyed on, so a new binary lays
/// down a fresh SDK rather than reusing a stale one.
fn sdk_version() -> &'static str {
    // Distinct SDK contents get a distinct cache dir. The git sha embedded by
    // build.rs changes whenever the tree does, which is a safe over-invalidation.
    option_env!("KRATE_GIT_SHA").unwrap_or("dev")
}

/// The directory the SDK is materialized into for this binary's version.
fn sdk_cache_dir() -> Result<PathBuf> {
    let base = cache_root().context("find a cache directory")?;
    Ok(base.join("krate").join("sdk").join(sdk_version()))
}

/// The platform cache root: `XDG_CACHE_HOME` or `~/.cache` on Unix,
/// `LOCALAPPDATA` on Windows, falling back to a temp dir.
fn cache_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(windows)]
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        return Some(PathBuf::from(dir));
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Some(PathBuf::from(home).join(".cache"));
    }
    Some(std::env::temp_dir())
}

/// Ensure the embedded SDK is present on disk and return its root directory.
///
/// This is idempotent: if the version's cache directory already holds the
/// files, it is returned as-is. Writing goes through a temp directory that is
/// renamed into place, so a concurrent or interrupted run never leaves a
/// half-written SDK behind.
pub fn ensure_materialized() -> Result<PathBuf> {
    let root = sdk_cache_dir()?;
    // A marker file records that the full SDK was written; its presence means
    // the directory is complete.
    let marker = root.join(".complete");
    if marker.is_file() {
        return Ok(root);
    }

    let parent = root.parent().context("sdk cache has no parent")?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;

    // Write into a sibling temp dir, then rename into place.
    let staging = parent.join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    for (rel, bytes) in EMBEDDED_SDK {
        write_file(&staging.join(rel), bytes)?;
    }
    write_file(&staging.join(".complete"), b"")?;

    // If another process won the race, keep theirs and drop ours.
    if marker.is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Ok(root);
    }
    let _ = std::fs::remove_dir_all(&root);
    match std::fs::rename(&staging, &root) {
        Ok(()) => Ok(root),
        Err(_) if marker.is_file() => {
            let _ = std::fs::remove_dir_all(&staging);
            Ok(root)
        }
        Err(err) => Err(err).with_context(|| format!("install SDK into {}", root.display())),
    }
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_the_wit_and_bindings() {
        // The embedded set must at least carry both WIT worlds and the bindings.
        let paths: Vec<&str> = EMBEDDED_SDK.iter().map(|(p, _)| *p).collect();
        assert!(paths
            .iter()
            .any(|p| p.ends_with("wit/krate/phase2/world.wit")));
        assert!(paths
            .iter()
            .any(|p| p.ends_with("wit/krate/phase3/world.wit")));
        assert!(paths.contains(&"crates/bindings-rust/Cargo.toml"));
        assert!(paths.contains(&"crates/bindings-rust/src/lib.rs"));
    }

    #[test]
    fn materialized_bindings_cargo_is_standalone() {
        // The embedded bindings Cargo.toml must not inherit from a workspace.
        let (_, bytes) = EMBEDDED_SDK
            .iter()
            .find(|(p, _)| *p == "crates/bindings-rust/Cargo.toml")
            .expect("bindings Cargo.toml embedded");
        let text = std::str::from_utf8(bytes).expect("utf8");
        assert!(!text.contains(".workspace = true"));
        assert!(text.contains("version = \"0.1.0-dev\""));
    }

    #[test]
    fn materialize_is_idempotent_and_complete() {
        let root = ensure_materialized().expect("materialize");
        assert!(root.join("wit/krate/phase2/world.wit").is_file());
        assert!(root.join("crates/bindings-rust/src/bindings.rs").is_file());
        // A second call returns the same directory without error.
        let again = ensure_materialized().expect("materialize again");
        assert_eq!(root, again);
    }
}
