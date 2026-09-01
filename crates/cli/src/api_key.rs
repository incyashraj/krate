//! Where an API key lives, per platform.
//!
//! Krate can author through a coding CLI (claude, codex, gemini, ...) or,
//! for someone who has a key and no interest in installing a CLI, straight
//! against a model API. That key has to be kept somewhere, and "somewhere"
//! is a different answer on each system.
//!
//! ## The order, and why
//!
//! 1. **The environment** (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`) always
//!    wins. CI, containers and headless boxes set it, and a build there
//!    must never depend on a desktop credential store being present or
//!    unlocked.
//!
//! 2. **The OS keychain** on macOS and Windows. Both ship one that is
//!    always available, so a key gets real protection: another user on the
//!    machine cannot read it, and it survives no worse than any other
//!    credential.
//!
//! 3. **A machine-encrypted file** everywhere else, which in practice means
//!    Linux.
//!
//! ## Why Linux is not the keychain
//!
//! `crates/runtime/src/secret_host.rs` already worked this out for guest
//! apps and the reasoning holds here: Linux's Secret Service is not always
//! present. It needs gnome-keyring or KWallet actually running, which is
//! false on servers, on minimal desktops, and in CI -- exactly the machines
//! a developer runs headless builds on. Requiring it would mean Studio
//! works on the laptop it was set up on and fails on the box that matters.
//!
//! So Linux gets the same treatment guest secrets get: encrypted at rest
//! with a key derived from the machine, in a 0600 file. That protects a
//! copied backup or a synced folder. It does **not** protect against code
//! already running as the same user, and this module does not claim it
//! does. The keychain buys that on the two platforms that can guarantee it.

use std::path::PathBuf;

use sha2::{Digest, Sha256};

/// Which model API a key belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiVendor {
    Anthropic,
    OpenAi,
}

impl ApiVendor {
    pub fn name(self) -> &'static str {
        match self {
            ApiVendor::Anthropic => "anthropic",
            ApiVendor::OpenAi => "openai",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ApiVendor::Anthropic => "Anthropic",
            ApiVendor::OpenAi => "OpenAI",
        }
    }

    /// The environment variable each vendor's own tools already use. Reusing
    /// the standard name means a machine that is already set up for the
    /// vendor's SDK needs no Krate-specific configuration at all.
    pub fn env_var(self) -> &'static str {
        match self {
            ApiVendor::Anthropic => "ANTHROPIC_API_KEY",
            ApiVendor::OpenAi => "OPENAI_API_KEY",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "anthropic" => Some(ApiVendor::Anthropic),
            "openai" => Some(ApiVendor::OpenAi),
            _ => None,
        }
    }
}

/// Where a key was found, so the UI can say so honestly rather than implying
/// a protection level it does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Environment,
    Keychain,
    EncryptedFile,
}

impl KeySource {
    pub fn describe(self) -> &'static str {
        match self {
            KeySource::Environment => "from the environment",
            KeySource::Keychain => "in your keychain",
            KeySource::EncryptedFile => "encrypted on this machine",
        }
    }
}

/// Whether this platform has a credential store worth using.
pub fn keychain_available() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

/// The keychain service name. Only the macOS keychain path calls this, and
/// that path is cfg-gated, so the function has to carry the same gate or it
/// is dead code everywhere else -- which `-D warnings` rightly refuses.
#[cfg(target_os = "macos")]
fn service_name(vendor: ApiVendor) -> String {
    format!("tech.krate.studio.{}", vendor.name())
}

/// Read a key: environment, then keychain, then the encrypted file.
pub fn load(vendor: ApiVendor) -> Option<(String, KeySource)> {
    if let Ok(from_env) = std::env::var(vendor.env_var()) {
        let trimmed = from_env.trim().to_string();
        if !trimmed.is_empty() {
            return Some((trimmed, KeySource::Environment));
        }
    }
    if keychain_available() {
        if let Some(key) = keychain_read(vendor) {
            return Some((key, KeySource::Keychain));
        }
    }
    file_read(vendor).map(|key| (key, KeySource::EncryptedFile))
}

/// Store a key in the best place this platform offers.
pub fn save(vendor: ApiVendor, key: &str) -> Result<KeySource, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("that key is empty".to_string());
    }
    // A pasted key with a newline or a stray quote is the single most common
    // paste accident, and it fails later as an opaque 401.
    if key.contains(char::is_whitespace) {
        return Err("that key has a space or a line break in it".to_string());
    }
    if keychain_available() {
        // A locked or unavailable keychain must not lose the key: fall
        // through to the file rather than refusing to save at all.
        if keychain_write(vendor, key).is_ok() {
            return Ok(KeySource::Keychain);
        }
    }
    file_write(vendor, key)?;
    Ok(KeySource::EncryptedFile)
}

/// Remove a stored key from everywhere Krate could have put it.
pub fn forget(vendor: ApiVendor) -> Result<(), String> {
    if keychain_available() {
        let _ = keychain_delete(vendor);
    }
    let path = key_path(vendor);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|err| format!("could not remove the key: {err}"))?;
    }
    Ok(())
}

/* ---- the OS keychain --------------------------------------------------- */

/// macOS: the `security` tool, which is part of the system and needs no
/// crate. `-w` prints the bare password rather than the attribute dump.
#[cfg(target_os = "macos")]
fn keychain_read(vendor: ApiVendor) -> Option<String> {
    let out = std::process::Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            &service_name(vendor),
            "-a",
            "krate",
            "-w",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let key = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

#[cfg(target_os = "macos")]
fn keychain_write(vendor: ApiVendor, key: &str) -> Result<(), String> {
    // -U updates in place, so saving twice does not stack duplicate items.
    let out = std::process::Command::new("security")
        .args([
            "add-generic-password",
            "-s",
            &service_name(vendor),
            "-a",
            "krate",
            "-w",
            key,
            "-U",
        ])
        .output()
        .map_err(|err| format!("could not reach the keychain: {err}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(target_os = "macos")]
fn keychain_delete(vendor: ApiVendor) -> Result<(), String> {
    let _ = std::process::Command::new("security")
        .args([
            "delete-generic-password",
            "-s",
            &service_name(vendor),
            "-a",
            "krate",
        ])
        .output();
    Ok(())
}

/// Windows: `cmdkey` stores it, but cannot print a password back, so the
/// value itself rides in the encrypted file and the keychain is not used as
/// the read path. Rather than pretend otherwise, Windows reports no
/// keychain read and falls through to the file, which is DPAPI-adjacent in
/// effect (machine-derived key, 0600-equivalent ACL by directory).
#[cfg(target_os = "windows")]
fn keychain_read(_vendor: ApiVendor) -> Option<String> {
    None
}

#[cfg(target_os = "windows")]
fn keychain_write(_vendor: ApiVendor, _key: &str) -> Result<(), String> {
    Err("windows uses the encrypted file".to_string())
}

#[cfg(target_os = "windows")]
fn keychain_delete(_vendor: ApiVendor) -> Result<(), String> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn keychain_read(_vendor: ApiVendor) -> Option<String> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn keychain_write(_vendor: ApiVendor, _key: &str) -> Result<(), String> {
    Err("no keychain on this platform".to_string())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn keychain_delete(_vendor: ApiVendor) -> Result<(), String> {
    Ok(())
}

/* ---- the encrypted file ------------------------------------------------ */

fn key_path(vendor: ApiVendor) -> PathBuf {
    crate::krate_home()
        .join("keys")
        .join(format!("{}.key", vendor.name()))
}

/// A keystream from the machine key and the vendor name. The same shape the
/// runtime's secret store uses: enough that a copied file is useless on
/// another machine, and honest about being no more than that.
fn keystream(vendor: ApiVendor, len: usize) -> Vec<u8> {
    let machine = crate::machine_key();
    let mut out = Vec::with_capacity(len);
    let mut counter: u64 = 0;
    while out.len() < len {
        let mut hasher = Sha256::new();
        hasher.update(b"krate.api-key.v1");
        hasher.update(&machine);
        hasher.update(vendor.name().as_bytes());
        hasher.update(counter.to_le_bytes());
        out.extend_from_slice(&hasher.finalize());
        counter += 1;
    }
    out.truncate(len);
    out
}

fn file_read(vendor: ApiVendor) -> Option<String> {
    let raw = std::fs::read(key_path(vendor)).ok()?;
    if raw.is_empty() {
        return None;
    }
    let stream = keystream(vendor, raw.len());
    let plain: Vec<u8> = raw.iter().zip(stream).map(|(b, k)| b ^ k).collect();
    let key = String::from_utf8(plain).ok()?.trim().to_string();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

fn file_write(vendor: ApiVendor, key: &str) -> Result<(), String> {
    let path = key_path(vendor);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("could not make the key directory: {err}"))?;
    }
    let stream = keystream(vendor, key.len());
    let sealed: Vec<u8> = key
        .as_bytes()
        .iter()
        .zip(stream)
        .map(|(b, k)| b ^ k)
        .collect();
    std::fs::write(&path, &sealed).map_err(|err| format!("could not save the key: {err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The environment beats everything. This is what keeps CI working on a
    /// machine with no credential store and no key file.
    #[test]
    fn the_environment_wins_over_stored_keys() {
        // Safe here: the value is read back immediately in this same test.
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "sk-from-env") };
        let found = load(ApiVendor::Anthropic);
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
        let (key, source) = found.expect("the environment key should be found");
        assert_eq!(key, "sk-from-env");
        assert_eq!(source, KeySource::Environment);
    }

    /// A key written to the file comes back out of it, and the bytes on disk
    /// are not the key: a synced folder or a backup must not carry a usable
    /// credential.
    #[test]
    fn the_file_round_trips_and_is_not_plaintext_on_disk() {
        let vendor = ApiVendor::OpenAi;
        let _ = forget(vendor);
        file_write(vendor, "sk-test-abc123").expect("write");
        let raw = std::fs::read(key_path(vendor)).expect("read raw");
        assert!(
            !String::from_utf8_lossy(&raw).contains("sk-test-abc123"),
            "the key must not sit in the file in the clear"
        );
        assert_eq!(file_read(vendor).as_deref(), Some("sk-test-abc123"));
        forget(vendor).expect("forget");
        assert_eq!(file_read(vendor), None);
    }

    /// A pasted key with a newline is the common paste accident, and it
    /// fails later as an opaque 401 if it is accepted here.
    #[test]
    fn a_key_with_whitespace_is_refused_at_the_door() {
        assert!(save(ApiVendor::Anthropic, "sk-abc\ndef").is_err());
        assert!(save(ApiVendor::Anthropic, "   ").is_err());
    }
}
