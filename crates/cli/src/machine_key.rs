//! This computer's machine key (IC-038): the key every app's `store.secret`
//! file, the organisation device key (F-316) and the account device key are
//! sealed under. It never reaches an app: the runtime derives a per-app key
//! from it and an app only ever sees plaintext it stored itself.
//!
//! Where it is kept:
//!
//! - **A Mac**, when Krate runs in the account's own home: the login
//!   keychain, so it is sealed by the account's password and a copied disk,
//!   a backup or a copied home folder does not carry it.
//! - **Windows**: `~/.krate/machine.key.sealed`, sealed by Windows for this
//!   user (DPAPI), for the same reason.
//! - **Linux, and any other home**: `~/.krate/machine.key`, readable only by
//!   the person. Linux has no keeper every machine runs (the Secret Service
//!   needs a desktop keyring running, which servers, minimal desktops and CI
//!   do not have), so the file is the one way that works everywhere.
//!
//! None of these stops code already running as the same person on the same
//! computer: a keychain item `security` made is readable by `security`, and
//! DPAPI opens for any process of the user. That is the line the secret
//! store's own documentation draws, and this does not claim more.
//!
//! The rule that matters most: a key recorded as kept somewhere that will
//! not hand it over (a locked keychain, a Windows profile copied from
//! another user) is an error, never a new key. A new key would make every
//! secret sealed with the old one unreadable, and the next save would write
//! over them.
//!
//! `~/.krate/machine.key.kept` records where the key is (`keychain` or
//! `windows`); with no record it is the file.
//!
//! A key already kept as a file stays a file. Every installed Mac app
//! carries the engine it was installed with, and an engine from before this
//! only knows the file: taking the file away would have it make a new key,
//! find its app's secrets unreadable, and write over them on the next save.
//! Moving those keys waits on every installed app carrying an engine that
//! knows the record (IC-038).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use sha2::{Digest, Sha256};

use crate::krate_home;

/// Where the machine key is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kept {
    /// The login keychain (macOS).
    Keychain,
    /// Sealed by Windows for this user (DPAPI).
    Windows,
    /// A file only the person can read.
    File,
}

impl Kept {
    pub(crate) fn describe(self) -> String {
        match self {
            Kept::Keychain => "in your keychain".to_string(),
            Kept::Windows => format!(
                "sealed by Windows for you, at {}",
                Here::now().sealed_file().display()
            ),
            Kept::File => format!(
                "in {}, readable only by you",
                Here::now().plain_file().display()
            ),
        }
    }

    fn record_word(self) -> Option<&'static str> {
        match self {
            Kept::Keychain => Some("keychain"),
            Kept::Windows => Some("windows"),
            Kept::File => None,
        }
    }
}

const KEYCHAIN_ITEM: crate::keychain::Item = crate::keychain::Item {
    service: "tech.krate.machine-key",
    label: "Krate secrets key",
    comment: "Seals the passwords and tokens apps keep with Krate on this Mac. Removing \
              it makes them unreadable.",
    said: "Krate's secrets key",
};

/// The Krate home being asked about, and whether its keychain may be used
/// (the account's own home on a Mac). Tests pass a folder of their own and
/// no keychain, so they never touch the person's.
struct Here {
    home: PathBuf,
    /// The keychain item, when the keychain may be used here.
    keychain: Option<&'static crate::keychain::Item>,
}

impl Here {
    fn now() -> Self {
        Here {
            home: krate_home(),
            keychain: crate::keychain::usable().then_some(&KEYCHAIN_ITEM),
        }
    }
    fn plain_file(&self) -> PathBuf {
        self.home.join("machine.key")
    }
    fn sealed_file(&self) -> PathBuf {
        self.home.join("machine.key.sealed")
    }
    fn record_file(&self) -> PathBuf {
        self.home.join("machine.key.kept")
    }
    fn keychain_read(&self) -> anyhow::Result<Option<Vec<u8>>> {
        match self.keychain {
            Some(item) => crate::keychain::read(item),
            None => Ok(None),
        }
    }
}

/// Where the key is kept now, from the record alone: never asks a keeper.
pub(crate) fn kept() -> Option<Kept> {
    kept_in(&Here::now())
}

fn kept_in(here: &Here) -> Option<Kept> {
    match fs::read_to_string(here.record_file())
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("keychain") => Some(Kept::Keychain),
        Some("windows") => Some(Kept::Windows),
        _ => here.plain_file().exists().then_some(Kept::File),
    }
}

/// This computer's machine key, made the first time it is needed.
///
/// Asked once per Krate home per process: a run reads it for its secrets,
/// its sync and its organisation key, and the keychain is asked once.
pub(crate) fn get() -> Result<Vec<u8>, String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Vec<u8>>>> = OnceLock::new();
    let home = krate_home();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(key) = cache.lock().ok().and_then(|c| c.get(&home).cloned()) {
        return Ok(key);
    }
    let key = load_or_create(&Here::now())?;
    if let Ok(mut c) = cache.lock() {
        c.insert(home, key.clone());
    }
    Ok(key)
}

fn load_or_create(here: &Here) -> Result<Vec<u8>, String> {
    match fs::read_to_string(here.record_file())
        .ok()
        .as_deref()
        .map(str::trim)
    {
        Some("keychain") => {
            let key = match here.keychain_read() {
                Ok(Some(key)) if key.len() >= 32 => key,
                Ok(_) if here.keychain.is_none() => {
                    return Err(format!(
                        "Krate's secrets key is kept in the keychain of the account that \
                         owns {}, and this is not that account's own session. Open Krate \
                         as that account.",
                        here.home.display()
                    ))
                }
                Ok(_) => {
                    return Err(
                        "Krate's secrets key is recorded as being in your keychain and it is \
                         not there. If it was removed on purpose, delete ~/.krate/machine.key.kept \
                         to start again; the secrets apps kept here cannot be read without it."
                            .to_string(),
                    )
                }
                Err(err) => return Err(err.to_string()),
            };
            Ok(key)
        }
        Some("windows") => {
            let sealed = fs::read(here.sealed_file())
                .map_err(|err| format!("could not read Krate's secrets key: {err}"))?;
            let key = windows_open(&sealed).map_err(|why| {
                format!(
                    "Windows would not open Krate's secrets key at {} ({why}). It opens only \
                     for the Windows account that made it; if this folder was copied from \
                     another account or computer, move it aside to start again -- the \
                     secrets apps kept with it cannot be read here.",
                    here.sealed_file().display()
                )
            })?;
            Ok(key)
        }
        _ => Ok(adopt_or_make(here)),
    }
}

/// No record: the key is the file (kept a file, see above), or a keeper
/// already holds one (a make cut short before its record was written), or
/// it is new -- and a new key goes to the strongest keeper here that takes
/// it and gives the same bytes back, the file only when none does.
fn adopt_or_make(here: &Here) -> Vec<u8> {
    if let Ok(existing) = fs::read(here.plain_file()) {
        if existing.len() >= 32 {
            return existing;
        }
    }
    let key = held_by_a_keeper(here).unwrap_or_else(|| fresh_key(here));
    let recorded = keep_stronger(here, &key)
        .record_word()
        .is_some_and(|word| write_private(&here.record_file(), word.as_bytes()).is_ok());
    if !recorded {
        let _ = write_private(&here.plain_file(), &key);
    }
    key
}

/// A key a keeper holds with no record saying so.
fn held_by_a_keeper(here: &Here) -> Option<Vec<u8>> {
    if let Ok(Some(key)) = here.keychain_read() {
        if key.len() >= 32 {
            return Some(key);
        }
    }
    if cfg!(windows) {
        if let Ok(sealed) = fs::read(here.sealed_file()) {
            if let Ok(key) = windows_open(&sealed) {
                if key.len() >= 32 {
                    return Some(key);
                }
            }
        }
    }
    None
}

/// Give the key to the keychain or Windows when they take it and give the
/// same bytes back; otherwise it stays the file.
fn keep_stronger(here: &Here, key: &[u8]) -> Kept {
    if let Some(item) = here.keychain {
        let there = here.keychain_read().ok().flatten();
        let added = match &there {
            Some(held) => held.as_slice() == key,
            None => crate::keychain::add(item, key).is_ok(),
        };
        if added && here.keychain_read().ok().flatten().as_deref() == Some(key) {
            return Kept::Keychain;
        }
    }
    if cfg!(windows) {
        if let Ok(sealed) = windows_seal(key) {
            if windows_open(&sealed).ok().as_deref() == Some(key)
                && write_private(&here.sealed_file(), &sealed).is_ok()
            {
                return Kept::Windows;
            }
        }
    }
    Kept::File
}

/// 32 bytes from the operating system's own secure generator (getrandom:
/// the kernel on Linux and macOS, BCryptGenRandom on Windows). Only a system
/// whose generator fails outright falls back to a value derived from this
/// install, and says so: apps keep working, and the person is told the
/// protection is weaker here.
fn fresh_key(here: &Here) -> Vec<u8> {
    let mut key = vec![0u8; 32];
    if getrandom::fill(&mut key).is_err() {
        eprintln!(
            "warning: this computer's random number generator did not answer, so Krate's \
             machine key is derived from this install instead; secrets kept by apps are \
             less protected here"
        );
        let mut hasher = Sha256::new();
        hasher.update(here.home.to_string_lossy().as_bytes());
        hasher.update(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos().to_le_bytes())
                .unwrap_or([0; 16]),
        );
        hasher.update(std::process::id().to_le_bytes());
        key.copy_from_slice(&hasher.finalize());
    }
    key
}

/// Write a file only the person can read, whole or not at all.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let staged = path.with_extension("staged");
    fs::write(&staged, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&staged, path)
}

/* ---- Windows: DPAPI ----------------------------------------------------- */

/// Words that tie the sealed key to this use, so a DPAPI blob made by
/// anything else does not open as Krate's key.
#[cfg(windows)]
const DPAPI_ENTROPY: &[u8] = b"krate machine key v1";

#[cfg(windows)]
fn dpapi(input: &[u8], seal: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let data = CRYPT_INTEGER_BLOB {
        cbData: input.len() as u32,
        pbData: input.as_ptr() as *mut u8,
    };
    let entropy = CRYPT_INTEGER_BLOB {
        cbData: DPAPI_ENTROPY.len() as u32,
        pbData: DPAPI_ENTROPY.as_ptr() as *mut u8,
    };
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: every blob points at memory that outlives the call; DPAPI
    // only reads `data` and `entropy`, and allocates `out` with LocalAlloc,
    // which is copied and then freed exactly once below.
    let ok = unsafe {
        if seal {
            CryptProtectData(
                &data,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        } else {
            CryptUnprotectData(
                &data,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        }
    };
    if ok == 0 || out.pbData.is_null() {
        return Err(format!("error {}", std::io::Error::last_os_error()));
    }
    // SAFETY: DPAPI handed back `cbData` bytes at `pbData`.
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    // SAFETY: allocated by DPAPI with LocalAlloc; freed once.
    unsafe {
        LocalFree(out.pbData as _);
    }
    Ok(bytes)
}

fn windows_seal(key: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(windows)]
    {
        dpapi(key, true)
    }
    #[cfg(not(windows))]
    {
        let _ = key;
        Err("not Windows".to_string())
    }
}

fn windows_open(sealed: &[u8]) -> Result<Vec<u8>, String> {
    #[cfg(windows)]
    {
        dpapi(sealed, false)
    }
    #[cfg(not(windows))]
    {
        let _ = sealed;
        Err("not Windows".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Krate home of its own, with no keychain: the tests never touch
    /// the person's keychain or their real `~/.krate`.
    fn here() -> (tempfile::TempDir, Here) {
        let dir = tempfile::tempdir().unwrap();
        let here = Here {
            home: dir.path().join(".krate"),
            keychain: None,
        };
        (dir, here)
    }

    #[test]
    fn the_key_is_made_once_and_the_same_after() {
        let (_dir, here) = here();
        assert!(kept_in(&here).is_none(), "a fresh home has no key yet");
        let first = load_or_create(&here).unwrap();
        assert_eq!(first.len(), 32);
        assert_eq!(
            load_or_create(&here).unwrap(),
            first,
            "a second ask makes no second key"
        );
        let kept = kept_in(&here).expect("the key is somewhere");
        if cfg!(windows) {
            assert_eq!(kept, Kept::Windows);
            assert!(
                !here.plain_file().exists(),
                "no key in the clear on Windows"
            );
            let sealed = fs::read(here.sealed_file()).unwrap();
            assert!(
                !sealed.windows(first.len()).any(|w| w == first.as_slice()),
                "the sealed file does not carry the key"
            );
        } else {
            // A moved home never uses the keychain, so it is the file here.
            assert_eq!(kept, Kept::File);
            assert_eq!(fs::read(here.plain_file()).unwrap(), first);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(here.plain_file())
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o600, "only the person reads the key file");
            }
        }
    }

    #[test]
    fn an_older_file_key_is_kept_not_replaced() {
        let (_dir, here) = here();
        let older = vec![7u8; 32];
        write_private(&here.plain_file(), &older).unwrap();
        assert_eq!(
            load_or_create(&here).unwrap(),
            older,
            "the key apps sealed with stays"
        );
        assert_eq!(load_or_create(&here).unwrap(), older);
        assert_eq!(
            fs::read(here.plain_file()).unwrap(),
            older,
            "an engine installed before this still finds it"
        );
        assert_eq!(kept_in(&here), Some(Kept::File));
        assert!(!here.record_file().exists() && !here.sealed_file().exists());
    }

    #[test]
    fn a_keeper_that_will_not_hand_the_key_over_is_an_error_not_a_new_key() {
        let (_dir, here) = here();
        // Recorded as in the keychain, from a home that cannot reach it.
        write_private(&here.record_file(), b"keychain").unwrap();
        let err = load_or_create(&here).unwrap_err();
        assert!(err.contains("keychain"), "{err}");
        assert!(!here.plain_file().exists(), "no second key was made");
        // Recorded as sealed by Windows, and the sealed file does not open.
        write_private(&here.record_file(), b"windows").unwrap();
        write_private(&here.sealed_file(), b"not a sealed key").unwrap();
        let err = load_or_create(&here).unwrap_err();
        assert!(err.contains("Windows would not open"), "{err}");
        assert!(!here.plain_file().exists(), "no second key was made");
        assert_eq!(
            fs::read(here.sealed_file()).unwrap(),
            b"not a sealed key",
            "left as it was"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_key_windows_holds_without_a_record_is_taken_not_replaced() {
        let (_dir, here) = here();
        // A make cut short after sealing, before its record was written.
        let made = vec![9u8; 32];
        write_private(&here.sealed_file(), &windows_seal(&made).unwrap()).unwrap();
        assert_eq!(load_or_create(&here).unwrap(), made);
        assert_eq!(kept_in(&here), Some(Kept::Windows));
        assert!(!here.plain_file().exists());
    }

    /// The real login keychain, with a throwaway item, by hand on a Mac:
    /// `cargo test -p krate-cli --bin krate -- --ignored the_login_keychain`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "writes a throwaway item into this Mac's login keychain"]
    fn the_login_keychain_keeps_a_new_key_and_gives_it_back() {
        const ITEM: crate::keychain::Item = crate::keychain::Item {
            service: "tech.krate.machine-key.by-hand-test",
            label: "Krate secrets key (test)",
            comment: "Made by a Krate test; safe to delete.",
            said: "the test secrets key",
        };
        let forget = || {
            let _ = std::process::Command::new("/usr/bin/security")
                .args(["delete-generic-password", "-s", ITEM.service, "-a", "krate"])
                .output();
        };
        forget();
        let dir = tempfile::tempdir().unwrap();
        let here = Here {
            home: dir.path().join(".krate"),
            keychain: Some(&ITEM),
        };
        let made = load_or_create(&here).unwrap();
        assert_eq!(made.len(), 32);
        assert_eq!(kept_in(&here), Some(Kept::Keychain));
        assert!(
            !here.plain_file().exists(),
            "a new key is not written in the clear"
        );
        assert_eq!(
            crate::keychain::read(&ITEM).unwrap().as_deref(),
            Some(made.as_slice())
        );
        assert_eq!(
            load_or_create(&here).unwrap(),
            made,
            "the same key next time"
        );
        // Gone from the keychain with the record left: an error, no new key.
        forget();
        let err = load_or_create(&here).unwrap_err();
        assert!(err.contains("not there"), "{err}");
        assert!(
            crate::keychain::read(&ITEM).unwrap().is_none(),
            "no second key was made"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_seals_the_key_for_this_use_only() {
        let key = vec![5u8; 32];
        let sealed = windows_seal(&key).unwrap();
        assert_ne!(sealed, key);
        assert_eq!(windows_open(&sealed).unwrap(), key);
        let mut damaged = sealed.clone();
        let last = damaged.len() - 1;
        damaged[last] ^= 1;
        assert!(
            windows_open(&damaged).is_err(),
            "a changed blob does not open"
        );
    }
}
