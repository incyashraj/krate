//! The maker's own publisher key: every app a person makes is signed with
//! it, from its first file (IC-983).
//!
//! Signing used to be a separate act (`krate sign --key`) with a key file
//! the person made and kept, so almost nothing anyone made was signed, and
//! an unsigned app's data is keyed by the app id its author chose -- any
//! other unsigned app claiming the id reaches it (K-247). The identity
//! contract's answer is a deliberate identity key; this makes it the
//! default instead of a chore:
//!
//! - **One key per person per computer**, made the first time something is
//!   made or shared, with no account, no question and no network.
//! - **Where it lives.** On a Mac, in the login keychain, when Krate runs as
//!   the account's own home (the keychain belongs to the account; a Krate
//!   home somewhere else -- a test, a moved `HOME` -- keeps its key in its
//!   own folder instead). Everywhere else, and whenever the keychain will
//!   not take it, in `~/.krate/keys/maker.key`, readable only by the
//!   person. Windows' `cmdkey` cannot hand a secret back (see api_key.rs),
//!   so Windows keeps the file too. `krate identity` says which.
//! - **What is recorded in the open.** `~/.krate/keys/maker.pub`: the
//!   public key and where the private half is. Only the public half and the
//!   signature ever go into a bundle; the private half never leaves this
//!   computer.
//! - **Never replaced behind the person's back.** A key that is recorded
//!   but cannot be read -- a locked keychain -- is an error that says how to
//!   unlock it, never a reason to make a second key: a second key is a
//!   second publisher, and the person's data would split between them.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::krate_home;

/// The keychain service the key is kept under on a Mac.
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "tech.krate.maker-key";

/// Where the private half of the key is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kept {
    /// The login keychain (macOS).
    Keychain,
    /// `~/.krate/keys/maker.key`, readable only by the person.
    File(PathBuf),
}

impl Kept {
    /// In the words `krate identity` and a make use.
    pub(crate) fn describe(&self) -> String {
        match self {
            Kept::Keychain => "in your keychain".to_string(),
            Kept::File(path) => format!("in {} (only you can read it)", path.display()),
        }
    }

    fn record_word(&self) -> &'static str {
        match self {
            Kept::Keychain => "keychain",
            Kept::File(_) => "file",
        }
    }
}

/// The maker's key, ready to sign.
pub(crate) struct MakerKey {
    pub(crate) key: krate_bundle::signing::SigningKey,
    /// The public key, lowercase hex: the publisher every app signed with
    /// it verifies to, and what its data is keyed by.
    pub(crate) public: String,
    pub(crate) kept: Kept,
    /// Made by this call: the person is told once.
    pub(crate) new: bool,
}

/// Is signing with the maker's key turned off on this machine?
///
/// `KRATE_MAKER_SIGNING=off` is for a machine that makes apps for other
/// people -- the web builder -- where one key would sign everyone's apps as
/// one publisher. It is not a person's setting.
pub(crate) fn signing_off() -> bool {
    std::env::var("KRATE_MAKER_SIGNING")
        .map(|v| v.trim().eq_ignore_ascii_case("off"))
        .unwrap_or(false)
}

fn keys_dir() -> PathBuf {
    krate_home().join("keys")
}

fn public_record() -> PathBuf {
    keys_dir().join("maker.pub")
}

fn private_file() -> PathBuf {
    keys_dir().join("maker.key")
}

/// The maker's public key, if this computer has one -- read from the open
/// record alone, without touching the private half (a run asks this on
/// every open, and must never raise a keychain prompt).
pub(crate) fn public() -> Option<String> {
    let text = std::fs::read_to_string(public_record()).ok()?;
    let line = text.lines().next()?.trim().to_string();
    is_public_hex(&line).then_some(line)
}

/// Where the record says the private half is, if there is a record.
pub(crate) fn kept() -> Option<Kept> {
    let text = std::fs::read_to_string(public_record()).ok()?;
    match text.lines().nth(1).map(str::trim) {
        Some("keychain") => Some(Kept::Keychain),
        Some("file") => Some(Kept::File(private_file())),
        _ => None,
    }
}

fn is_public_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// Read back only from the keychain, which only a Mac has.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

fn from_pkcs8(document: &[u8]) -> Result<(krate_bundle::signing::SigningKey, String)> {
    let key = krate_bundle::signing::SigningKey::from_pkcs8(document)
        .map_err(|err| anyhow::anyhow!("your maker key could not be read: {err}"))?;
    let public = hex(&key.public_key());
    Ok((key, public))
}

/// The maker's key: the one recorded on this computer, or a new one made
/// now. Never a second one beside a recorded key that cannot be read.
pub(crate) fn load_or_create() -> Result<MakerKey> {
    let recorded = public();
    match (recorded.as_deref(), kept()) {
        (Some(public), Some(Kept::Keychain)) => {
            let document = match keychain_read()? {
                Some(document) => document,
                None => bail!(
                    "your maker key is recorded as being in your keychain, and it is not \
                     there. Krate will not make a second one: apps you made would stop \
                     being yours. If you moved it, put it back; to start over, remove {}",
                    public_record().display()
                ),
            };
            let (key, found) = from_pkcs8(&document)?;
            if found != public {
                bail!(
                    "the maker key in your keychain is not the one this computer recorded \
                     ({}...). Krate will not sign with it.",
                    &public[..12]
                );
            }
            Ok(MakerKey {
                key,
                public: found,
                kept: Kept::Keychain,
                new: false,
            })
        }
        (Some(public), Some(Kept::File(path))) => {
            let document = std::fs::read(&path).with_context(|| {
                format!(
                    "your maker key should be at {} and could not be read",
                    path.display()
                )
            })?;
            let (key, found) = from_pkcs8(&document)?;
            if found != public {
                bail!(
                    "the maker key at {} is not the one this computer recorded ({}...). \
                     Krate will not sign with it.",
                    path.display(),
                    &public[..12]
                );
            }
            Ok(MakerKey {
                key,
                public: found,
                kept: Kept::File(path),
                new: false,
            })
        }
        _ => adopt_or_make(),
    }
}

/// No record: a key already in the keychain or the folder (the record was
/// deleted, the home was rebuilt) is taken back, not replaced; otherwise a
/// new key is made.
fn adopt_or_make() -> Result<MakerKey> {
    if keychain_usable() {
        if let Some(document) = keychain_read()? {
            let (key, public) = from_pkcs8(&document)?;
            write_record(&public, &Kept::Keychain)?;
            return Ok(MakerKey {
                key,
                public,
                kept: Kept::Keychain,
                new: false,
            });
        }
    }
    let path = private_file();
    if let Ok(document) = std::fs::read(&path) {
        let (key, public) = from_pkcs8(&document)?;
        let kept = Kept::File(path);
        write_record(&public, &kept)?;
        return Ok(MakerKey {
            key,
            public,
            kept,
            new: false,
        });
    }

    let document = krate_bundle::signing::SigningKey::generate_pkcs8()
        .map_err(|err| anyhow::anyhow!("could not make a maker key: {err}"))?;
    let (key, public) = from_pkcs8(&document)?;
    std::fs::create_dir_all(keys_dir())
        .with_context(|| format!("could not make {}", keys_dir().display()))?;
    // The keychain when it takes the key AND gives the same key back; the
    // folder otherwise. Either way the record names where it went, so the
    // next read never guesses.
    let kept = if keychain_usable()
        && keychain_write(&document).is_ok()
        && keychain_read().ok().flatten().as_deref() == Some(document.as_slice())
    {
        Kept::Keychain
    } else {
        write_private(&path, &document)?;
        Kept::File(path)
    };
    write_record(&public, &kept)?;
    Ok(MakerKey {
        key,
        public,
        kept,
        new: true,
    })
}

fn write_record(public: &str, kept: &Kept) -> Result<()> {
    let path = public_record();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let staging = path.with_extension("pub-new");
    std::fs::write(&staging, format!("{public}\n{}\n", kept.record_word()))
        .with_context(|| format!("could not write {}", path.display()))?;
    std::fs::rename(&staging, &path).with_context(|| format!("could not write {}", path.display()))
}

/// Owner-only from the moment it exists: written beside, then renamed.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let staging = path.with_extension("key-new");
    let _ = std::fs::remove_file(&staging);
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)
            .with_context(|| format!("could not write {}", staging.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    #[cfg(not(unix))]
    std::fs::write(&staging, bytes)
        .with_context(|| format!("could not write {}", staging.display()))?;
    std::fs::rename(&staging, path).with_context(|| format!("could not write {}", path.display()))
}

/* ---- the keychain ------------------------------------------------------- */

/// The keychain is used when this is a Mac and Krate's home is the
/// account's own: the keychain belongs to the account, so a Krate home
/// anywhere else (a test, a moved `HOME`) keeps its key in its own folder
/// and never writes into the person's keychain.
fn keychain_usable() -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(account_home) = account_home() else {
            return false;
        };
        let Some(home) = crate::home_dir() else {
            return false;
        };
        let same = match (account_home.canonicalize(), home.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        same && Path::new("/usr/bin/security").exists()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The account's home as the system knows it, not as `HOME` says.
#[cfg(target_os = "macos")]
fn account_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    // SAFETY: getpwuid_r writes into the buffers we own and hands back a
    // pointer into them; we read pw_dir before they go out of scope.
    unsafe {
        let mut pwd: libc::passwd = std::mem::zeroed();
        let mut buf = vec![0 as libc::c_char; 4096];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc = libc::getpwuid_r(
            libc::getuid(),
            &mut pwd,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        );
        if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
            return None;
        }
        let dir = CStr::from_ptr(pwd.pw_dir).to_string_lossy().into_owned();
        Some(PathBuf::from(dir))
    }
}

/// The key's PKCS#8 document from the keychain: `Ok(None)` when there is no
/// such item, an error when the keychain could not be asked (locked, or the
/// person said no) -- the two must never be confused, or a locked keychain
/// would make a second key.
fn keychain_read() -> Result<Option<Vec<u8>>> {
    #[cfg(target_os = "macos")]
    {
        if !keychain_usable() {
            return Ok(None);
        }
        let out = std::process::Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                KEYCHAIN_SERVICE,
                "-a",
                "krate",
                "-w",
            ])
            .output()
            .context("could not ask the keychain for your maker key")?;
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            return unhex(&text)
                .map(Some)
                .context("the maker key in your keychain is damaged");
        }
        // 44: errSecItemNotFound. Anything else is the keychain refusing.
        if out.status.code() == Some(44) {
            return Ok(None);
        }
        bail!(
            "your keychain did not hand over your maker key ({}). Unlock it -- open \
             Keychain Access, or run `security unlock-keychain` -- and try again.",
            String::from_utf8_lossy(&out.stderr).trim()
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(None)
    }
}

/// Put the key in the keychain. The secret goes in on `security -i`'s
/// standard input, never on a command line another process could read.
/// Never `-U`: an item already there is somebody's key, not ours to
/// overwrite.
fn keychain_write(document: &[u8]) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        let mut child = std::process::Command::new("/usr/bin/security")
            .arg("-i")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("could not reach the keychain")?;
        let line = format!(
            "add-generic-password -s {KEYCHAIN_SERVICE} -a krate -l \"Krate maker key\" \
             -j \"Signs the apps you make with Krate. Removing it means Krate cannot \
             sign updates to them.\" -w {}\n",
            hex(document)
        );
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(line.as_bytes())?;
        }
        let out = child.wait_with_output()?;
        let said = String::from_utf8_lossy(&out.stderr);
        if out.status.success() && !said.contains("rror") {
            Ok(())
        } else {
            bail!("the keychain did not take the key: {}", said.trim())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = document;
        bail!("no keychain here")
    }
}

/* ---- signing ------------------------------------------------------------ */

/// What signing a file as its maker did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Signed {
    /// It carried a signature already (the maker's, another publisher's or
    /// an organisation's): left exactly as it was.
    Already,
    /// Signed now, by this public key.
    Now { public: String },
    /// Signing as a maker is off on this machine (the web builder).
    Off,
}

/// Sign `bundle` in place with the maker's key, unless it already carries
/// a signature. The namespace is the app's own id: the maker is vouching
/// for the manifest they wrote.
pub(crate) fn sign_if_unsigned(bundle: &Path) -> Result<Signed> {
    if signing_off() {
        return Ok(Signed::Off);
    }
    let opened = krate_bundle::open(bundle)
        .map_err(|err| anyhow::anyhow!("could not open {} to sign it: {err}", bundle.display()))?;
    if !matches!(
        opened.signature_state(),
        Ok(krate_bundle::SignatureState::Absent)
    ) {
        return Ok(Signed::Already);
    }
    let app_id = opened.manifest().app.id.clone();
    let version = opened.manifest().app.version.clone();
    drop(opened);
    let maker = load_or_create()?;
    if maker.new {
        eprintln!(
            "Made your publisher key ({}...), kept {}. Every app you make is signed with it, \
             so it stays yours and keeps its data through every rebuild. It never leaves \
             this computer.",
            &maker.public[..12],
            maker.kept.describe()
        );
    }
    let signed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    krate_bundle::sign_bundle(bundle, &maker.key, &app_id, &version, signed_at)
        .map_err(|err| anyhow::anyhow!("could not sign {}: {err}", bundle.display()))?;
    Ok(Signed::Now {
        public: maker.public,
    })
}

/// A file about to leave this computer (publish, wrap, card), signed as its
/// maker first when it is not signed yet. Signed in place, so sharing it
/// again shares the same bytes; when the file cannot be written (a
/// read-only folder), a signed copy goes out instead and the file is left
/// as it was. The returned guard keeps that copy alive.
pub(crate) struct GoingOut {
    pub(crate) path: PathBuf,
    _copy: Option<tempfile::TempDir>,
}

/// Sign a file in place before a wrap or a card is made from it, where the
/// result is written beside the file. A file that cannot be changed goes
/// out unsigned, said in words: the wrap is not refused for it.
pub(crate) fn sign_in_place_for_going_out(bundle: &Path) -> Result<()> {
    let going_out = sign_for_going_out_inner(bundle, false)?;
    debug_assert_eq!(going_out.path, bundle);
    Ok(())
}

pub(crate) fn sign_for_going_out(bundle: &Path) -> Result<GoingOut> {
    sign_for_going_out_inner(bundle, true)
}

fn sign_for_going_out_inner(bundle: &Path, may_copy: bool) -> Result<GoingOut> {
    let unchanged = || GoingOut {
        path: bundle.to_path_buf(),
        _copy: None,
    };
    // A plain bundle only: a card is a picture with the bundle behind it,
    // and rewriting it as an archive would lose the picture.
    let is_zip = std::fs::File::open(bundle)
        .and_then(|mut file| {
            use std::io::Read;
            let mut magic = [0u8; 4];
            file.read_exact(&mut magic).map(|_| magic)
        })
        .map(|magic| magic == *b"PK\x03\x04")
        .unwrap_or(false);
    // A file Krate would refuse to open is not signed: the door's own
    // refusal says why, in its words, and nothing is written to it.
    if signing_off() || !is_zip || krate_bundle::open(bundle).is_err() {
        return Ok(unchanged());
    }
    let read_only = std::fs::metadata(bundle)
        .map(|m| m.permissions().readonly())
        .unwrap_or(false);
    if !read_only {
        match sign_if_unsigned(bundle) {
            Ok(Signed::Now { public }) => {
                eprintln!(
                    "signed {} as you ({}...) before it goes out",
                    bundle.display(),
                    &public[..12]
                );
                return Ok(unchanged());
            }
            Ok(_) => return Ok(unchanged()),
            // The folder would not take the signed file: fall through to
            // a signed copy. A key problem is the person's to fix.
            Err(err) if !is_write_error(&err) => return Err(err),
            Err(_) => {}
        }
    }
    if !may_copy {
        eprintln!(
            "{} could not be changed, so it goes out unsigned; move it somewhere you can \
             write and run this again to sign it",
            bundle.display()
        );
        return Ok(unchanged());
    }
    let dir = tempfile::tempdir().context("could not make a folder to sign a copy in")?;
    let copy = dir.path().join(
        bundle
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| "app.krate".into()),
    );
    std::fs::copy(bundle, &copy).with_context(|| format!("could not copy {}", bundle.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o644));
    }
    if let Signed::Now { .. } = sign_if_unsigned(&copy)? {
        eprintln!(
            "{} could not be changed, so a signed copy goes out and the file stays as it was",
            bundle.display()
        );
        return Ok(GoingOut {
            path: copy,
            _copy: Some(dir),
        });
    }
    Ok(unchanged())
}

fn is_write_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::PermissionDenied)
    }) || err.to_string().contains("ermission denied")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_refuses_what_is_not_hex() {
        assert_eq!(unhex(&hex(&[0, 1, 254, 255])), Some(vec![0, 1, 254, 255]));
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
        assert!(is_public_hex(&"a".repeat(64)));
        assert!(!is_public_hex(&"A".repeat(64)));
        assert!(!is_public_hex("abcd"));
    }
}
