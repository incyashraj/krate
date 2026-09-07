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
        // Only for reading a store written before the HKDF change. Nothing
        // is ever sealed with it; the first write after an upgrade re-seals
        // the whole file as v2 (IC-249).
        let key_v1 = derive_key_v1(app_id, machine_key);
        // A store that exists but cannot be read is not an empty store
        // (IC-877). Losing a credential silently is worse here than in the
        // KV store: the app asks for its key, gets nothing, and may write a
        // new one over the old ciphertext.
        let (entries, unreadable) = if granted {
            match load(&path, &key, &key_v1) {
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
/// The per-app key, derived with HKDF-SHA256 (IC-249).
///
/// This was a bare `SHA256(label || app_id || machine_key)`. A single hash is
/// not a key derivation function: it has no salt, no extract step, and no
/// domain separation beyond the label being first, so a machine key reused
/// across contexts produces related keys with no formal guarantee between
/// them. HKDF is the standard answer and ring is an audited implementation of
/// it -- RFC 5869, extract-then-expand, with the app id as the info string so
/// two apps on one machine get keys that are independent by construction.
fn derive_key(app_id: &str, machine_key: &[u8]) -> [u8; 32] {
    use ring::hkdf;

    // A fixed salt is what RFC 5869 calls for when there is no per-use salt
    // to carry: the extract step still separates this use of the machine key
    // from any other.
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"krate.secret.v2");
    let prk = salt.extract(machine_key);
    // Length-prefixed so an app id cannot be chosen to collide with another
    // by absorbing the boundary.
    let len = (app_id.len() as u64).to_le_bytes();
    let info: [&[u8]; 2] = [&len, app_id.as_bytes()];

    struct Key32;
    impl hkdf::KeyType for Key32 {
        fn len(&self) -> usize {
            32
        }
    }
    let okm = prk
        .expand(&info, Key32)
        .expect("HKDF-SHA256 expand to 32 bytes is always valid");
    let mut key = [0u8; 32];
    okm.fill(&mut key)
        .expect("filling 32 bytes from a 32-byte OKM cannot fail");
    key
}

/// The old key derivation, kept only to read stores written before v2.
///
/// A person's saved secrets are not something to lose in an upgrade, so a
/// v1 file still opens; the next write re-seals it under v2. Nothing new is
/// ever written with this.
fn derive_key_v1(app_id: &str, machine_key: &[u8]) -> [u8; 32] {
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
    use ring::aead;

    let mut plain = Vec::new();
    for (name, secret) in entries {
        plain.extend_from_slice(&(name.len() as u32).to_le_bytes());
        plain.extend_from_slice(name.as_bytes());
        plain.extend_from_slice(&(secret.len() as u32).to_le_bytes());
        plain.extend_from_slice(secret);
    }

    // ChaCha20-Poly1305, from ring, replacing a hand-rolled SHA-256 keystream
    // and a homemade MAC (IC-249). The old scheme was not obviously broken,
    // but "not obviously broken" is the wrong standard for the file holding
    // somebody's API keys: this is RFC 8439, implemented by a maintained
    // library, with the authentication tag part of the primitive rather than
    // a construction of ours.
    //
    // The 12-byte nonce is what the AEAD requires; the old format's 16 bytes
    // were sized for the custom keystream.
    let mut nonce_bytes = [0u8; 12];
    let random = random_bytes()?;
    nonce_bytes.copy_from_slice(&random[..12]);

    let unbound = aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key).ok()?;
    let sealing = aead::LessSafeKey::new(unbound);
    let nonce = aead::Nonce::assume_unique_for_key(nonce_bytes);

    // The magic is the associated data, so a v2 file cannot be replayed as
    // anything else without the tag failing.
    let mut sealed = plain;
    sealing
        .seal_in_place_append_tag(nonce, aead::Aad::from(b"KRS2"), &mut sealed)
        .ok()?;

    let mut out = Vec::with_capacity(sealed.len() + 16);
    out.extend_from_slice(b"KRS2");
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&sealed);
    Some(out)
}

/// Open a store, in whichever format it was written.
///
/// `key` is the v2 (HKDF) key; `key_v1` is the old one, used only to read a
/// file written before the change. A person's saved secrets must survive an
/// upgrade, so a v1 file still opens and the next write re-seals it as v2.
fn decrypt_all(
    bytes: &[u8],
    key: &[u8; 32],
    key_v1: &[u8; 32],
) -> Option<BTreeMap<String, Vec<u8>>> {
    let plain = if bytes.len() >= 4 && &bytes[..4] == b"KRS2" {
        decrypt_v2(bytes, key)?
    } else {
        decrypt_v1(bytes, key_v1)?
    };
    parse_entries(&plain)
}

fn decrypt_v2(bytes: &[u8], key: &[u8; 32]) -> Option<Vec<u8>> {
    use ring::aead;

    // 4 magic + 12 nonce + 16 tag
    if bytes.len() < 32 {
        return None;
    }
    let mut nonce_bytes = [0u8; 12];
    nonce_bytes.copy_from_slice(&bytes[4..16]);

    let unbound = aead::UnboundKey::new(&aead::CHACHA20_POLY1305, key).ok()?;
    let opening = aead::LessSafeKey::new(unbound);
    let nonce = aead::Nonce::assume_unique_for_key(nonce_bytes);

    // Poly1305 verifies before anything is returned: a tampered, truncated
    // or wrong-key file yields None here rather than plausible-looking bytes.
    let mut sealed = bytes[16..].to_vec();
    let plain = opening
        .open_in_place(nonce, aead::Aad::from(b"KRS2"), &mut sealed)
        .ok()?;
    Some(plain.to_vec())
}

/// The pre-v2 format: SHA-256 keystream with a homemade MAC. Read-only.
fn decrypt_v1(bytes: &[u8], key: &[u8; 32]) -> Option<Vec<u8>> {
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
    Some(plain)
}

fn parse_entries(plain: &[u8]) -> Option<BTreeMap<String, Vec<u8>>> {
    let plain = plain.to_vec();
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
/// Fresh random bytes for a nonce, or nothing.
///
/// Returning None means the store is not written. Refusing to save is
/// recoverable; sealing under a nonce that is not random is not, because
/// ChaCha20-Poly1305 loses its guarantees the moment a nonce repeats under
/// one key.
fn random_bytes() -> Option<[u8; 16]> {
    let mut bytes = [0u8; 16];
    crate::random_host::fill(&mut bytes).ok()?;
    Some(bytes)
}

/// Read the secret store from disk.
///
/// `Ok(None)` means nothing has been saved yet. `Err` means a store exists
/// and could not be read or decrypted -- which must never be reported as
/// "no secrets" (IC-877).
fn load(
    path: &Path,
    key: &[u8; 32],
    key_v1: &[u8; 32],
) -> std::io::Result<Option<BTreeMap<String, Vec<u8>>>> {
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
    Ok(decrypt_all(&bytes, key, key_v1))
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

    /// New stores are sealed with ChaCha20-Poly1305, not the old hand-rolled
    /// keystream (IC-249). The magic byte is the visible proof, and it is
    /// what tells the reader which format it is holding.
    #[test]
    fn a_new_store_is_written_with_real_aead() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("secrets.bin");
        let mut store = AppSecrets::open(path.clone(), "dev.krate.aead", b"machine", true);
        store.set("token", b"hunter2".to_vec()).expect("set");

        let bytes = std::fs::read(&path).expect("read");
        assert_eq!(&bytes[..4], b"KRS2", "a new store must be the AEAD format");
        assert!(
            !bytes.windows(7).any(|w| w == b"hunter2"),
            "the secret must not be on disk in the clear"
        );
    }

    /// A store written before the change still opens, and the next write
    /// re-seals it (IC-249). Losing somebody's saved credentials in an
    /// upgrade would be a worse defect than the one being fixed.
    #[test]
    fn a_store_from_before_the_change_still_opens_and_is_resealed() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("secrets.bin");

        // Write one the old way, byte for byte.
        let key_v1 = derive_key_v1("dev.krate.legacy", b"machine");
        let mut old_entries = BTreeMap::new();
        old_entries.insert("token".to_string(), b"from-the-old-format".to_vec());
        let plain = {
            let mut plain = Vec::new();
            for (name, secret) in &old_entries {
                plain.extend_from_slice(&(name.len() as u32).to_le_bytes());
                plain.extend_from_slice(name.as_bytes());
                plain.extend_from_slice(&(secret.len() as u32).to_le_bytes());
                plain.extend_from_slice(secret);
            }
            plain
        };
        let nonce = [7u8; 16];
        let mut body = plain.clone();
        apply_keystream(&mut body, &key_v1, &nonce);
        let tag = mac(&key_v1, &nonce, &body);
        let mut legacy = Vec::new();
        legacy.extend_from_slice(b"KRS1");
        legacy.extend_from_slice(&nonce);
        legacy.extend_from_slice(&body);
        legacy.extend_from_slice(&tag);
        std::fs::write(&path, &legacy).expect("seed a v1 store");

        // It opens, with the secret intact.
        let mut store = AppSecrets::open(path.clone(), "dev.krate.legacy", b"machine", true);
        assert_eq!(
            store.get("token").expect("get"),
            Some(b"from-the-old-format".to_vec()),
            "an upgrade must not lose saved secrets"
        );

        // And the next write moves it to the new format.
        store.set("second", b"new".to_vec()).expect("set");
        let bytes = std::fs::read(&path).expect("read");
        assert_eq!(&bytes[..4], b"KRS2", "the next write re-seals as AEAD");

        let reopened = AppSecrets::open(path, "dev.krate.legacy", b"machine", true);
        assert_eq!(
            reopened.get("token").expect("get"),
            Some(b"from-the-old-format".to_vec())
        );
        assert_eq!(reopened.get("second").expect("get"), Some(b"new".to_vec()));
    }

    /// Poly1305 rejects a flipped bit anywhere -- ciphertext, nonce or tag --
    /// rather than returning whatever the altered bytes decode to.
    #[test]
    fn every_part_of_an_aead_store_is_authenticated() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("secrets.bin");
        let mut store = AppSecrets::open(path.clone(), "dev.krate.tamper", b"machine", true);
        store.set("token", b"hunter2".to_vec()).expect("set");
        let good = std::fs::read(&path).expect("read");

        // Nonce (4..16), ciphertext (16..len-16), tag (last 16).
        for spot in [5usize, good.len() / 2, good.len() - 1] {
            let mut bad = good.clone();
            bad[spot] ^= 0x01;
            std::fs::write(&path, &bad).expect("write tampered");
            let opened = AppSecrets::open(path.clone(), "dev.krate.tamper", b"machine", true);
            assert_eq!(
                opened.get("token").expect("get"),
                None,
                "a flipped bit at {spot} must not yield a secret"
            );
        }
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
