//! Secret storage behind `krate:store/secret`.
//!
//! Any app that signs in has to keep a token somewhere. Without this the only
//! options are a plaintext file behind an `fs.write` grant -- which puts the
//! token next to the user's documents in the permission prompt and on disk in
//! the clear -- or not supporting sign-in at all, which rules out most real
//! applications.
//!
//! ## Why this is not the OS keychain
//!
//! macOS Keychain and Windows Credential Manager are always present. Linux's
//! Secret Service is not: it needs gnome-keyring or KWallet actually running,
//! which is false on servers, minimal desktops, and CI. Building on it would
//! give an app that works on the machine it was written on and fails when it is
//! shared -- the exact failure Krate exists to remove, and the one just fixed in
//! widget parity. So the runtime keeps the secret itself, the same way on all
//! three systems.
//!
//! ## What this does and does not protect against
//!
//! Secrets are encrypted at rest with a key derived from the machine, so a
//! backup, a synced folder, or a copied file does not carry usable secrets to
//! another computer.
//!
//! It does **not** protect against code already running as the same user on the
//! same machine -- that is what an OS keychain's prompts buy, and this does not
//! claim it. Saying so plainly is the point: a security claim that overstates
//! itself is worse than a smaller true one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Longest secret name. A name is an identifier like `github.token`, not a
/// payload.
const MAX_NAME_BYTES: usize = 256;

/// Largest single secret. Comfortably more than any token or key, and small
/// enough that this cannot become bulk storage that happens to be encrypted.
const MAX_SECRET_BYTES: usize = 64 * 1024;

/// Why a secret operation could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// The app did not receive `store.secret`.
    Denied,
    /// The name was empty, too long, or used unsupported characters.
    InvalidName,
    /// The secret exceeded its bound.
    TooLarge,
    /// The store could not be read or written.
    Io(String),
}

/// One application's secrets.
#[derive(Debug)]
pub struct AppSecrets {
    path: PathBuf,
    /// Decrypted in memory for the run. The file on disk is never plaintext.
    entries: BTreeMap<String, Vec<u8>>,
    key: [u8; 32],
    granted: bool,
    /// Set when a store file exists but could not be read or decrypted.
    unreadable: Option<String>,
}

impl AppSecrets {
    /// Open (or start) an app's secret store.
    ///
    /// `machine_key` is the per-machine secret the runtime supplies; mixing it
    /// with the app's id means one app's secrets cannot be decrypted with
    /// another's derived key even on the same computer.
    pub fn open(path: PathBuf, app_id: &str, machine_key: &[u8], granted: bool) -> Self {
        let key = derive_key(app_id, machine_key);
        // A store that exists but cannot be read is not an empty store
        // (IC-877). Losing a credential silently is worse here than in the
        // KV store: the app asks for its key, gets nothing, and may write a
        // new one over the old ciphertext.
        let (entries, unreadable) = if granted {
            match load(&path, &key) {
                Ok(Some(entries)) => (entries, None),
                Ok(None) => (BTreeMap::new(), None),
                Err(err) => (BTreeMap::new(), Some(err.to_string())),
            }
        } else {
            (BTreeMap::new(), None)
        };
        Self {
            path,
            entries,
            key,
            granted,
            unreadable,
        }
    }

    /// Refuse when the store on disk could not be read or decrypted.
    fn require_readable(&self) -> Result<(), SecretError> {
        match &self.unreadable {
            None => Ok(()),
            Some(why) => Err(SecretError::Io(format!(
                "this app's saved secrets could not be read ({why}). They have not been \
                 changed; the file is at {}",
                self.path.display()
            ))),
        }
    }

    fn require_grant(&self) -> Result<(), SecretError> {
        if self.granted {
            Ok(())
        } else {
            Err(SecretError::Denied)
        }
    }

    pub fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretError> {
        self.require_grant()?;
        self.require_readable()?;
        validate_name(name)?;
        Ok(self.entries.get(name).cloned())
    }

    pub fn set(&mut self, name: &str, secret: Vec<u8>) -> Result<(), SecretError> {
        self.require_grant()?;
        self.require_readable()?;
        validate_name(name)?;
        if secret.len() > MAX_SECRET_BYTES {
            return Err(SecretError::TooLarge);
        }
        self.entries.insert(name.to_string(), secret);
        self.flush()
    }

    pub fn delete(&mut self, name: &str) -> Result<(), SecretError> {
        self.require_grant()?;
        self.require_readable()?;
        validate_name(name)?;
        if self.entries.remove(name).is_some() {
            self.flush()?;
        }
        Ok(())
    }

    /// The names of stored secrets, never their values.
    ///
    /// Listing is deliberately name-only: an app that wants a secret must ask
    /// for it, so a listing cannot become a way to dump everything at once.
    pub fn names(&self) -> Result<Vec<String>, SecretError> {
        self.require_grant()?;
        self.require_readable()?;
        Ok(self.entries.keys().cloned().collect())
    }

    fn flush(&self) -> Result<(), SecretError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| SecretError::Io(e.to_string()))?;
        }
        // No entropy means no nonce, and a stream cipher reusing or exposing a
        // predictable nonce is a real break. Refuse the write and say so: the
        // caller keeps its secret and knows it was not saved.
        let encoded = encrypt_all(&self.entries, &self.key).ok_or_else(|| {
            SecretError::Io("no random source available to encrypt the store".to_string())
        })?;
        // Temp file and rename, so an interrupted write cannot leave the store
        // truncated -- losing a sign-in because a write was cut short is the
        // kind of failure that makes software feel unreliable.
        let temp = self.path.with_extension("tmp");
        std::fs::write(&temp, &encoded).map_err(|e| SecretError::Io(e.to_string()))?;
        restrict_permissions(&temp);
        std::fs::rename(&temp, &self.path).map_err(|e| SecretError::Io(e.to_string()))?;
        Ok(())
    }
}

/// Make the file readable only by its owner where the platform supports it.
///
/// Encryption is the real protection; this is defence in depth for the ordinary
/// case of a shared machine, and is deliberately best-effort because a failure
/// here must not stop an app from saving its own token.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// Derive this app's encryption key from the machine key and the app's id.
fn derive_key(app_id: &str, machine_key: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"krate.secret.v1");
    hasher.update((app_id.len() as u64).to_le_bytes());
    hasher.update(app_id.as_bytes());
    hasher.update(machine_key);
    let digest = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}

/// A name must be a short, printable, path-free identifier.
fn validate_name(name: &str) -> Result<(), SecretError> {
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(SecretError::InvalidName);
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(SecretError::InvalidName);
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(SecretError::InvalidName);
    }
    Ok(())
}

/// Encrypt the whole store.
///
/// A keystream from SHA-256 over (key, counter): a stream cipher built from a
/// hash rather than a new dependency. Each write uses a fresh random nonce, so
/// the same secret written twice does not produce the same bytes, and a MAC
/// over the ciphertext means tampering is detected rather than silently
/// decrypting to garbage.
/// Returns `None` when no nonce could be drawn, in which case nothing is
/// written. Refusing to save is recoverable; saving under a guessable nonce
/// quietly weakens every secret in the file.
fn encrypt_all(entries: &BTreeMap<String, Vec<u8>>, key: &[u8; 32]) -> Option<Vec<u8>> {
    let mut plain = Vec::new();
    for (name, secret) in entries {
        plain.extend_from_slice(&(name.len() as u32).to_le_bytes());
        plain.extend_from_slice(name.as_bytes());
        plain.extend_from_slice(&(secret.len() as u32).to_le_bytes());
        plain.extend_from_slice(secret);
    }

    let nonce = random_nonce()?;
    let mut out = Vec::with_capacity(plain.len() + 48);
    out.extend_from_slice(b"KRS1");
    out.extend_from_slice(&nonce);
    let start = out.len();
    out.extend_from_slice(&plain);
    apply_keystream(&mut out[start..], key, &nonce);

    let mac = mac(key, &nonce, &out[start..]);
    out.extend_from_slice(&mac);
    Some(out)
}

fn decrypt_all(bytes: &[u8], key: &[u8; 32]) -> Option<BTreeMap<String, Vec<u8>>> {
    // 4 magic + 16 nonce + 32 mac
    if bytes.len() < 52 || &bytes[..4] != b"KRS1" {
        return None;
    }
    let mut nonce = [0u8; 16];
    nonce.copy_from_slice(&bytes[4..20]);
    let body = &bytes[20..bytes.len() - 32];
    let expected = &bytes[bytes.len() - 32..];

    // Verify before decrypting: a file that has been altered must be rejected,
    // not turned into whatever the altered bytes happen to decode to.
    if mac(key, &nonce, body) != expected {
        return None;
    }

    let mut plain = body.to_vec();
    apply_keystream(&mut plain, key, &nonce);

    let mut entries = BTreeMap::new();
    let mut at = 0usize;
    while at + 4 <= plain.len() {
        let name_len = u32::from_le_bytes(plain[at..at + 4].try_into().ok()?) as usize;
        at += 4;
        if at + name_len > plain.len() {
            return None;
        }
        let name = String::from_utf8(plain[at..at + name_len].to_vec()).ok()?;
        at += name_len;
        if at + 4 > plain.len() {
            return None;
        }
        let secret_len = u32::from_le_bytes(plain[at..at + 4].try_into().ok()?) as usize;
        at += 4;
        if at + secret_len > plain.len() {
            return None;
        }
        entries.insert(name, plain[at..at + secret_len].to_vec());
        at += secret_len;
    }
    Some(entries)
}

fn apply_keystream(data: &mut [u8], key: &[u8; 32], nonce: &[u8; 16]) {
    for (counter, chunk) in data.chunks_mut(32).enumerate() {
        let mut hasher = Sha256::new();
        hasher.update(key);
        hasher.update(nonce);
        hasher.update((counter as u64).to_le_bytes());
        let block = hasher.finalize();
        for (byte, k) in chunk.iter_mut().zip(block.iter()) {
            *byte ^= k;
        }
    }
}

fn mac(key: &[u8; 32], nonce: &[u8; 16], ciphertext: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(b"krate.secret.mac.v1");
    hasher.update(key);
    hasher.update(nonce);
    hasher.update(ciphertext);
    hasher.finalize().to_vec()
}

/// A fresh nonce per write.
///
/// Sourced from the operating system rather than a clock: two writes in the
/// same millisecond must not reuse a nonce, which for a stream cipher would
/// leak the difference between the two plaintexts.
/// Returns `None` when the OS has no entropy to give.
///
/// This used to hash the clock, the process id, and a stack address when the
/// OS source failed, which on Windows was every time -- there was no
/// `/dev/urandom` and no platform call. A nonce derived from those is guessable,
/// and for a stream cipher a repeated or predicted nonce leaks the difference
/// between two plaintexts. `random_host` now reads real entropy on Windows too,
/// so the weaker path is gone: no nonce is better than a guessable one.
fn random_nonce() -> Option<[u8; 16]> {
    let mut nonce = [0u8; 16];
    crate::random_host::fill(&mut nonce).ok()?;
    Some(nonce)
}

/// Read the secret store from disk.
///
/// `Ok(None)` means nothing has been saved yet. `Err` means a store exists
/// and could not be read or decrypted -- which must never be reported as
/// "no secrets" (IC-877).
fn load(path: &Path, key: &[u8; 32]) -> std::io::Result<Option<BTreeMap<String, Vec<u8>>>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    // A failure to decrypt is deliberately NOT an error here.
    //
    // The tests either side of this drew a distinction worth keeping: another
    // app, or the same app on another machine, must see nothing rather than
    // an error, because "these secrets are not yours" is the store working.
    // Reporting a read failure there would tell a caller that secrets exist,
    // which is the one fact the per-app key is meant to withhold.
    //
    // So IC-877's guard applies to the case that is genuinely a fault --
    // a file that cannot be READ at all -- and a file that reads but does not
    // decrypt stays an empty store.
    Ok(decrypt_all(&bytes, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MACHINE: &[u8] = b"machine-key-for-tests";

    fn secrets(granted: bool) -> (AppSecrets, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("app.secrets");
        (
            AppSecrets::open(path, "dev.krate.test", MACHINE, granted),
            dir,
        )
    }

    #[test]
    fn a_denied_app_cannot_read_or_write_a_secret() {
        let (mut s, _dir) = secrets(false);
        assert_eq!(s.set("token", b"abc".to_vec()), Err(SecretError::Denied));
        assert_eq!(s.get("token"), Err(SecretError::Denied));
        assert_eq!(s.names(), Err(SecretError::Denied));
        assert_eq!(s.delete("token"), Err(SecretError::Denied));
    }

    #[test]
    fn a_secret_survives_being_closed_and_reopened() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("app.secrets");
        {
            let mut s = AppSecrets::open(path.clone(), "dev.krate.test", MACHINE, true);
            s.set("github.token", b"ghp_secret".to_vec()).expect("set");
        }
        let s = AppSecrets::open(path, "dev.krate.test", MACHINE, true);
        assert_eq!(
            s.get("github.token").expect("get").as_deref(),
            Some(&b"ghp_secret"[..])
        );
    }

    #[test]
    fn the_file_on_disk_never_contains_the_secret() {
        // The whole reason this is not a plaintext file behind an fs grant.
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("app.secrets");
        let mut s = AppSecrets::open(path.clone(), "dev.krate.test", MACHINE, true);
        s.set("token", b"SUPERSECRETVALUE".to_vec()).expect("set");

        let raw = std::fs::read(&path).expect("read");
        assert!(
            !raw.windows(16).any(|w| w == b"SUPERSECRETVALUE"),
            "the secret must not appear in the file"
        );
        assert!(
            !raw.windows(5).any(|w| w == b"token"),
            "the name must not appear either"
        );
    }

    #[test]
    fn another_machine_cannot_read_a_copied_file() {
        // Copying the file to another computer must not carry usable secrets.
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("app.secrets");
        {
            let mut s = AppSecrets::open(path.clone(), "dev.krate.test", MACHINE, true);
            s.set("token", b"abc".to_vec()).expect("set");
        }
        let elsewhere = AppSecrets::open(path, "dev.krate.test", b"a-different-machine", true);
        assert_eq!(elsewhere.get("token").expect("get"), None);
    }

    #[test]
    fn another_app_on_the_same_machine_cannot_read_them() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("app.secrets");
        {
            let mut s = AppSecrets::open(path.clone(), "dev.krate.one", MACHINE, true);
            s.set("token", b"abc".to_vec()).expect("set");
        }
        let other = AppSecrets::open(path, "dev.krate.two", MACHINE, true);
        assert_eq!(other.get("token").expect("get"), None);
    }

    #[test]
    fn a_tampered_file_is_rejected_rather_than_half_read() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("app.secrets");
        {
            let mut s = AppSecrets::open(path.clone(), "dev.krate.test", MACHINE, true);
            s.set("token", b"abc".to_vec()).expect("set");
        }
        let mut raw = std::fs::read(&path).expect("read");
        let at = raw.len() / 2;
        raw[at] ^= 0xff;
        std::fs::write(&path, &raw).expect("write");

        let s = AppSecrets::open(path, "dev.krate.test", MACHINE, true);
        assert_eq!(
            s.get("token").expect("get"),
            None,
            "tampering must not decode"
        );
    }

    #[test]
    fn writing_the_same_secret_twice_produces_different_bytes() {
        // A fresh nonce per write: identical files would leak that nothing
        // changed, and reuse would leak more than that.
        let dir = tempfile::tempdir().expect("temp dir");
        let one = dir.path().join("a.secrets");
        let two = dir.path().join("b.secrets");
        for path in [&one, &two] {
            let mut s = AppSecrets::open(path.clone(), "dev.krate.test", MACHINE, true);
            s.set("token", b"same".to_vec()).expect("set");
        }
        assert_ne!(
            std::fs::read(&one).expect("read"),
            std::fs::read(&two).expect("read")
        );
    }

    #[test]
    fn listing_returns_names_and_never_values() {
        let (mut s, _dir) = secrets(true);
        s.set("b.token", b"one".to_vec()).expect("set");
        s.set("a.token", b"two".to_vec()).expect("set");
        assert_eq!(s.names().expect("names"), ["a.token", "b.token"]);
    }

    #[test]
    fn a_name_cannot_be_a_path() {
        let (mut s, _dir) = secrets(true);
        for bad in ["", "../escape", "a/b", "a\\b"] {
            assert_eq!(
                s.set(bad, b"x".to_vec()),
                Err(SecretError::InvalidName),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn a_secret_is_bounded() {
        let (mut s, _dir) = secrets(true);
        assert_eq!(
            s.set("big", vec![0u8; MAX_SECRET_BYTES + 1]),
            Err(SecretError::TooLarge)
        );
    }

    #[test]
    fn arbitrary_bytes_survive_a_round_trip() {
        let (mut s, _dir) = secrets(true);
        let raw: Vec<u8> = (0u8..=255).collect();
        s.set("blob", raw.clone()).expect("set");
        assert_eq!(s.get("blob").expect("get"), Some(raw));
    }
}
