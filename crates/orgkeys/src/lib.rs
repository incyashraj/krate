//! An organisation's keyring (founder decision F-316): what lets an
//! organisation's app data and connection credentials sit on Krate's hub as
//! ciphertext only.
//!
//! - **Device keys.** Each member device has an X25519 key pair
//!   ([`KeyPair::generate`]); the public part is registered with the hub.
//! - **The organisation key.** A versioned X25519 key pair per
//!   organisation ([`Keyring`]). Its private parts exist only on member
//!   devices: the hub holds one envelope per device, the whole keyring (every
//!   version, so old data stays readable) sealed to that device's public key
//!   with HPKE (RFC 9180, base mode, DHKEM(X25519, HKDF-SHA256),
//!   HKDF-SHA256, ChaCha20-Poly1305), from the RustCrypto `hpke` crate.
//! - **Recovery.** A recovery code ([`RecoveryCode`]) is stretched with
//!   Argon2id into the seed of one more X25519 key pair; the keyring is
//!   sealed to its public half like to any device. Sealing needs only the
//!   public half, so a device that rotates the key re-seals for recovery
//!   without knowing the code.
//! - **Sealing to the organisation** ([`seal_for_org`]): anyone with the
//!   organisation's public key can seal a secret -- a connection's
//!   credential -- that only a member device can open ([`open_for_org`]).
//! - **An app's team data** ([`AppDataKey`]): per app and key version, a
//!   key derived with HKDF from the organisation key; opaque record and field
//!   names with HMAC-SHA256, values sealed with XChaCha20-Poly1305 under
//!   associated data that binds them to their names and version.
//!
//! Nothing here implements a primitive. Every one comes from a vetted crate
//! at a pinned version (see Cargo.toml).

use std::collections::BTreeMap;
use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use hpke::{Deserializable, Kem as _, OpModeR, OpModeS, Serializable};
use rand_core::TryRngCore;
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

#[cfg(feature = "agent")]
pub mod agent;
pub mod signing;

type Kem = hpke::kem::X25519HkdfSha256;
type Kdf = hpke::kdf::HkdfSha256;
type HpkeAead = hpke::aead::ChaCha20Poly1305;

/// The HPKE suite every envelope uses, for anyone checking.
pub const HPKE_SUITE: &str =
    "RFC 9180 base mode, DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305";

const KEYRING_INFO: &[u8] = b"krate org keyring v1\0";
const ORG_SEAL_INFO: &[u8] = b"krate org seal v1\0";
const DATA_INFO: &[u8] = b"krate store.org v1\0";
const LOCAL_INFO: &[u8] = b"krate local seal v1\0";

/// Why a key operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The bytes are not what they claim to be.
    Malformed(&'static str),
    /// Decryption failed: the wrong key, or the bytes were changed.
    Open,
    /// The operating system's randomness was unavailable.
    Random,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::Malformed(what) => write!(f, "not a valid {what}"),
            KeyError::Open => {
                f.write_str("it does not open with this key: the wrong key, or it was changed")
            }
            KeyError::Random => f.write_str("the operating system gave no randomness"),
        }
    }
}

impl std::error::Error for KeyError {}

fn random<const N: usize>() -> Result<[u8; N], KeyError> {
    let mut out = [0u8; N];
    rand_core::OsRng
        .try_fill_bytes(&mut out)
        .map_err(|_| KeyError::Random)?;
    Ok(out)
}

fn rng() -> rand_core::UnwrapErr<rand_core::OsRng> {
    rand_core::UnwrapErr(rand_core::OsRng)
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Read lower- or upper-case hex.
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

fn hex32(text: &str) -> Result<[u8; 32], KeyError> {
    unhex(text)
        .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
        .ok_or(KeyError::Malformed("32-byte hex key"))
}

/// An X25519 key pair: a device's, a version of the organisation key, or
/// the one a recovery code derives.
#[derive(Clone, PartialEq, Eq)]
pub struct KeyPair {
    secret: [u8; 32],
    public: [u8; 32],
}

impl Drop for KeyPair {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyPair({})", self.public_hex())
    }
}

impl KeyPair {
    /// A fresh key pair from the operating system's randomness.
    pub fn generate() -> KeyPair {
        let (sk, pk) = Kem::gen_keypair(&mut rng());
        KeyPair::from_parts(&sk, &pk)
    }

    fn from_parts(
        sk: &<Kem as hpke::Kem>::PrivateKey,
        pk: &<Kem as hpke::Kem>::PublicKey,
    ) -> KeyPair {
        let mut secret = [0u8; 32];
        secret.copy_from_slice(&sk.to_bytes());
        let mut public = [0u8; 32];
        public.copy_from_slice(&pk.to_bytes());
        KeyPair { secret, public }
    }

    /// RFC 9180 DeriveKeyPair from 32 bytes of keying material.
    pub fn derive(ikm: &[u8; 32]) -> KeyPair {
        let (sk, pk) = Kem::derive_keypair(ikm);
        KeyPair::from_parts(&sk, &pk)
    }

    /// The key pair whose private half is `secret`.
    pub fn from_secret(secret: &[u8]) -> Result<KeyPair, KeyError> {
        let sk = <Kem as hpke::Kem>::PrivateKey::from_bytes(secret)
            .map_err(|_| KeyError::Malformed("X25519 private key"))?;
        let pk = Kem::sk_to_pk(&sk);
        Ok(KeyPair::from_parts(&sk, &pk))
    }

    pub fn public(&self) -> [u8; 32] {
        self.public
    }

    pub fn public_hex(&self) -> String {
        hex(&self.public)
    }

    /// The private half. Kept by Krate on the device, never sent.
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    /// The id the hub knows this key by: the first 16 hex digits of the
    /// SHA-256 of its public half's lower-case hex.
    pub fn id(&self) -> String {
        device_id(&self.public)
    }

    fn hpke_secret(&self) -> Result<<Kem as hpke::Kem>::PrivateKey, KeyError> {
        <Kem as hpke::Kem>::PrivateKey::from_bytes(&self.secret)
            .map_err(|_| KeyError::Malformed("X25519 private key"))
    }
}

/// The id of a public key: the first 16 hex digits of the SHA-256 of its
/// lower-case hex. Hex, not bytes, so the hub's store checks it with the
/// synchronous SHA-256 it already has (cloud/worker/src/org-keys.js).
pub fn device_id(public: &[u8; 32]) -> String {
    hex(&Sha256::digest(hex(public).as_bytes()))[..16].to_string()
}

fn hpke_seal(
    recipient: &[u8; 32],
    info: &[u8],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, KeyError> {
    hpke_seal_with(recipient, info, aad, plaintext, &mut rng())
}

/// [`hpke_seal`] with the randomness the ephemeral key is made from given:
/// the operating system's everywhere but the cross-language test vector.
fn hpke_seal_with<R: rand_core::CryptoRng + rand_core::RngCore>(
    recipient: &[u8; 32],
    info: &[u8],
    aad: &[u8],
    plaintext: &[u8],
    random: &mut R,
) -> Result<Vec<u8>, KeyError> {
    let pk = <Kem as hpke::Kem>::PublicKey::from_bytes(recipient)
        .map_err(|_| KeyError::Malformed("X25519 public key"))?;
    let (encapped, ciphertext) = hpke::single_shot_seal::<HpkeAead, Kdf, Kem, _>(
        &OpModeS::Base,
        &pk,
        info,
        plaintext,
        aad,
        random,
    )
    .map_err(|_| KeyError::Malformed("X25519 public key"))?;
    let mut out = encapped.to_bytes().to_vec();
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn hpke_open(
    recipient: &KeyPair,
    info: &[u8],
    aad: &[u8],
    sealed: &[u8],
) -> Result<Vec<u8>, KeyError> {
    if sealed.len() < 32 + 16 {
        return Err(KeyError::Malformed("sealed envelope"));
    }
    let encapped = <Kem as hpke::Kem>::EncappedKey::from_bytes(&sealed[..32])
        .map_err(|_| KeyError::Malformed("sealed envelope"))?;
    hpke::single_shot_open::<HpkeAead, Kdf, Kem>(
        &OpModeR::Base,
        &recipient.hpke_secret()?,
        &encapped,
        info,
        &sealed[32..],
        aad,
    )
    .map_err(|_| KeyError::Open)
}

/// Every version of one organisation's key. Sealed whole to each device, so
/// a device can read data written under any version it was given.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Keyring {
    org: String,
    keys: BTreeMap<u32, KeyPair>,
}

impl Keyring {
    /// A new organisation's keyring: version 1.
    pub fn create(org: &str) -> Keyring {
        let mut keys = BTreeMap::new();
        keys.insert(1, KeyPair::generate());
        Keyring {
            org: org.to_string(),
            keys,
        }
    }

    /// Add the next version and answer its number. Removing a member
    /// ends in this: new writes use a key the removed member never had.
    pub fn rotate(&mut self) -> u32 {
        let next = self.current().0 + 1;
        self.keys.insert(next, KeyPair::generate());
        next
    }

    /// A new organisation's keyring whose version 1 is the X25519 private
    /// key `secret`, made elsewhere -- by WebCrypto in a browser that is the
    /// owner's first device (IC-958).
    pub fn from_secret(org: &str, secret: &[u8]) -> Result<Keyring, KeyError> {
        let mut keys = BTreeMap::new();
        keys.insert(1, KeyPair::from_secret(secret)?);
        Ok(Keyring {
            org: org.to_string(),
            keys,
        })
    }

    /// [`Keyring::rotate`] to a next version made elsewhere.
    pub fn rotate_to(&mut self, secret: &[u8]) -> Result<u32, KeyError> {
        let pair = KeyPair::from_secret(secret)?;
        let next = self.current().0 + 1;
        self.keys.insert(next, pair);
        Ok(next)
    }

    pub fn org(&self) -> &str {
        &self.org
    }

    /// The newest version and its key.
    pub fn current(&self) -> (u32, &KeyPair) {
        let (v, k) = self
            .keys
            .iter()
            .next_back()
            .expect("a keyring has at least one version");
        (*v, k)
    }

    pub fn version(&self, version: u32) -> Option<&KeyPair> {
        self.keys.get(&version)
    }

    pub fn versions(&self) -> Vec<u32> {
        self.keys.keys().copied().collect()
    }

    fn to_json(&self) -> String {
        let keys: Vec<serde_json::Value> = self
            .keys
            .iter()
            .map(|(v, k)| serde_json::json!({ "v": v, "secret": hex(&k.secret) }))
            .collect();
        serde_json::json!({ "org": self.org, "keys": keys }).to_string()
    }

    fn from_json(text: &[u8]) -> Result<Keyring, KeyError> {
        let bad = KeyError::Malformed("keyring");
        let value: serde_json::Value = serde_json::from_slice(text).map_err(|_| bad.clone())?;
        let org = value["org"].as_str().ok_or(bad.clone())?.to_string();
        let mut keys = BTreeMap::new();
        for k in value["keys"].as_array().ok_or(bad.clone())? {
            let v = k["v"]
                .as_u64()
                .filter(|v| *v >= 1 && *v <= u64::from(u32::MAX))
                .ok_or(bad.clone())? as u32;
            let mut secret = hex32(k["secret"].as_str().ok_or(bad.clone())?)?;
            let pair = KeyPair::from_secret(&secret)?;
            secret.zeroize();
            keys.insert(v, pair);
        }
        if keys.is_empty() {
            return Err(bad);
        }
        Ok(Keyring { org, keys })
    }

    /// Seal the whole keyring to one device's public key (or the recovery
    /// key's): the envelope the hub keeps for that device, as base64.
    pub fn seal_to(&self, recipient: &[u8; 32]) -> Result<String, KeyError> {
        let mut plain = self.to_json().into_bytes();
        let sealed = hpke_seal(recipient, &keyring_info(&self.org), recipient, &plain);
        plain.zeroize();
        Ok(b64_encode(&sealed?))
    }

    /// Open an envelope sealed to `recipient` for organisation `org`.
    pub fn open(envelope: &str, recipient: &KeyPair, org: &str) -> Result<Keyring, KeyError> {
        let sealed = b64_decode(envelope).ok_or(KeyError::Malformed("envelope"))?;
        let mut plain = hpke_open(recipient, &keyring_info(org), &recipient.public, &sealed)?;
        let keyring = Keyring::from_json(&plain);
        plain.zeroize();
        let keyring = keyring?;
        if keyring.org != org {
            return Err(KeyError::Open);
        }
        Ok(keyring)
    }
}

fn keyring_info(org: &str) -> Vec<u8> {
    let mut info = KEYRING_INFO.to_vec();
    info.extend_from_slice(org.as_bytes());
    info
}

/// The recovery code an owner is shown once when the organisation's key
/// is made: 26 characters of Crockford base32, 130 bits.
#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryCode(String);

impl fmt::Debug for RecoveryCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecoveryCode(..)")
    }
}

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

impl RecoveryCode {
    pub fn generate() -> Result<RecoveryCode, KeyError> {
        let bytes: [u8; 26] = random()?;
        Ok(RecoveryCode(
            bytes
                .iter()
                .map(|b| CROCKFORD[(b & 31) as usize] as char)
                .collect(),
        ))
    }

    /// Read a code as a person typed it: case, spaces and dashes do not
    /// matter, and O, I and L read as 0, 1 and 1.
    pub fn parse(text: &str) -> Option<RecoveryCode> {
        let code: String = text
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .map(|c| match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                other => other,
            })
            .collect();
        (code.len() == 26 && code.bytes().all(|b| CROCKFORD.contains(&b)))
            .then_some(RecoveryCode(code))
    }

    /// Grouped for reading aloud and writing down.
    pub fn display(&self) -> String {
        self.0
            .as_bytes()
            .chunks(5)
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect::<Vec<_>>()
            .join("-")
    }
}

/// How a recovery code is stretched: Argon2id with these costs and salt.
/// The hub keeps these beside the envelope; they are not secret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryParams {
    pub salt: [u8; 16],
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl RecoveryParams {
    /// OWASP's Argon2id floor (19 MiB, 2 passes, 1 lane), fresh salt.
    pub fn generate() -> Result<RecoveryParams, KeyError> {
        Ok(RecoveryParams {
            salt: random()?,
            m_kib: 19 * 1024,
            t: 2,
            p: 1,
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "alg": "argon2id", "salt": hex(&self.salt), "m_kib": self.m_kib, "t": self.t, "p": self.p })
    }

    pub fn from_json(value: &serde_json::Value) -> Result<RecoveryParams, KeyError> {
        let bad = KeyError::Malformed("recovery parameters");
        if value["alg"].as_str() != Some("argon2id") {
            return Err(bad);
        }
        let salt = unhex(value["salt"].as_str().ok_or(bad.clone())?)
            .and_then(|s| <[u8; 16]>::try_from(s.as_slice()).ok())
            .ok_or(bad.clone())?;
        let n = |k: &str| {
            value[k]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or(bad.clone())
        };
        let (m_kib, t, p) = (n("m_kib")?, n("t")?, n("p")?);
        // Never weaker than the floor, whatever a stored record says.
        if m_kib < 19 * 1024 || t < 2 || p < 1 || m_kib > 4 * 1024 * 1024 || t > 64 || p > 16 {
            return Err(bad);
        }
        Ok(RecoveryParams { salt, m_kib, t, p })
    }
}

/// The key pair a recovery code stands for.
pub fn recovery_keypair(code: &RecoveryCode, params: &RecoveryParams) -> Result<KeyPair, KeyError> {
    let argon = argon2::Argon2::new(
        argon2::Algorithm::Argon2id,
        argon2::Version::V0x13,
        argon2::Params::new(params.m_kib, params.t, params.p, Some(32))
            .map_err(|_| KeyError::Malformed("recovery parameters"))?,
    );
    let mut seed = [0u8; 32];
    argon
        .hash_password_into(code.0.as_bytes(), &params.salt, &mut seed)
        .map_err(|_| KeyError::Malformed("recovery parameters"))?;
    let pair = KeyPair::derive(&seed);
    seed.zeroize();
    Ok(pair)
}

/// Seal a secret to the organisation's public key, for `context` (say
/// `connection:finance-api`). Anyone holding the public key can; only a
/// device holding that version of the keyring opens it.
pub fn seal_for_org(
    org_public: &[u8; 32],
    context: &str,
    plaintext: &[u8],
) -> Result<String, KeyError> {
    let mut info = ORG_SEAL_INFO.to_vec();
    info.extend_from_slice(context.as_bytes());
    Ok(b64_encode(&hpke_seal(
        org_public, &info, org_public, plaintext,
    )?))
}

/// The context a company connection's credential is sealed for: the
/// organisation, the connection's name, the header it goes in and the
/// address it goes to, as the hub keeps them. Binding the address means a
/// hub that moved a connection to a server of its own would hand devices a
/// credential that no longer opens; binding the name, that it cannot pass
/// one connection's credential off as another's. The admin's browser
/// (docs/landing/org/orgseal.js) builds the same text.
pub fn connection_context(org: &str, name: &str, header_name: &str, base: &str) -> String {
    format!("connection:{org}/{name}\n{header_name}\n{base}")
}

/// Open what [`seal_for_org`] sealed to version `version` of the key.
pub fn open_for_org(
    keyring: &Keyring,
    version: u32,
    context: &str,
    sealed: &str,
) -> Result<Vec<u8>, KeyError> {
    let key = keyring.version(version).ok_or(KeyError::Open)?;
    let mut info = ORG_SEAL_INFO.to_vec();
    info.extend_from_slice(context.as_bytes());
    let bytes = b64_decode(sealed).ok_or(KeyError::Malformed("sealed secret"))?;
    hpke_open(key, &info, &key.public, &bytes)
}

fn hkdf(ikm: &[u8], info: &[u8]) -> [u8; 64] {
    let mut out = [0u8; 64];
    hkdf::Hkdf::<Sha256>::new(None, ikm)
        .expand(info, &mut out)
        .expect("64 bytes is a valid HKDF-SHA256 length");
    out
}

/// What a row of an app's team data is: a value, a removed field, or a
/// deleted record. The hub sees this; it bounds and expires rows by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Value,
    Null,
    Tombstone,
}

impl RowKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RowKind::Value => "v",
            RowKind::Null => "n",
            RowKind::Tombstone => "t",
        }
    }

    pub fn parse(text: &str) -> Option<RowKind> {
        match text {
            "v" => Some(RowKind::Value),
            "n" => Some(RowKind::Null),
            "t" => Some(RowKind::Tombstone),
            _ => None,
        }
    }
}

/// One app's data key at one version of the organisation key.
pub struct AppDataKey {
    version: u32,
    mac: [u8; 32],
    enc: [u8; 32],
}

impl Drop for AppDataKey {
    fn drop(&mut self) {
        self.mac.zeroize();
        self.enc.zeroize();
    }
}

fn length_prefixed(out: &mut Vec<u8>, part: &str) {
    out.extend_from_slice(&(part.len() as u32).to_be_bytes());
    out.extend_from_slice(part.as_bytes());
}

impl AppDataKey {
    /// DK = HKDF-SHA256(the organisation key's private half at `version`,
    /// "krate store.org v1" || app id || version), split into a MAC key and
    /// an encryption key. `None` when the keyring lacks that version.
    pub fn derive(keyring: &Keyring, version: u32, app_id: &str) -> Option<AppDataKey> {
        let key = keyring.version(version)?;
        let mut info = DATA_INFO.to_vec();
        length_prefixed(&mut info, app_id);
        info.extend_from_slice(&version.to_be_bytes());
        let mut both = hkdf(key.secret(), &info);
        let mut mac = [0u8; 32];
        let mut enc = [0u8; 32];
        mac.copy_from_slice(&both[..32]);
        enc.copy_from_slice(&both[32..]);
        both.zeroize();
        Some(AppDataKey { version, mac, enc })
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    fn hmac(&self, parts: &[&str], label: &str) -> String {
        let mut input = Vec::new();
        length_prefixed(&mut input, label);
        for part in parts {
            length_prefixed(&mut input, part);
        }
        let mut mac =
            <Hmac<Sha256> as Mac>::new_from_slice(&self.mac).expect("HMAC takes a 32-byte key");
        mac.update(&input);
        hex(&mac.finalize().into_bytes())
    }

    /// The opaque name of a record, the same on every device.
    pub fn record_key(&self, collection: &str, id: &str) -> String {
        self.hmac(&[collection, id], "record")
    }

    /// The opaque name of one field of a record (`""` is the record's
    /// tombstone).
    pub fn field_key(&self, collection: &str, id: &str, field: &str) -> String {
        self.hmac(&[collection, id, field], "field")
    }

    fn aad(&self, record: &str, field: &str, kind: RowKind) -> Vec<u8> {
        let mut aad = DATA_INFO.to_vec();
        aad.extend_from_slice(&self.version.to_be_bytes());
        length_prefixed(&mut aad, record);
        length_prefixed(&mut aad, field);
        length_prefixed(&mut aad, kind.as_str());
        aad
    }

    /// Seal one row's plaintext, bound to its opaque names, kind and
    /// version: base64 of a 24-byte random nonce and the ciphertext.
    pub fn seal(
        &self,
        record: &str,
        field: &str,
        kind: RowKind,
        plaintext: &[u8],
    ) -> Result<String, KeyError> {
        let nonce: [u8; 24] = random()?;
        self.seal_with_nonce(record, field, kind, plaintext, &nonce)
    }

    /// [`AppDataKey::seal`] with the nonce given, for the cross-language
    /// test vectors (`crates/orgdata/tests/vectors/org-records.json`): the
    /// same bytes in, the same bytes out, on a computer and in a browser
    /// tab. A nonce must never be used twice under one key; everything else
    /// calls [`AppDataKey::seal`], which draws a fresh one.
    pub fn seal_with_nonce(
        &self,
        record: &str,
        field: &str,
        kind: RowKind,
        plaintext: &[u8],
        nonce: &[u8; 24],
    ) -> Result<String, KeyError> {
        let cipher = XChaCha20Poly1305::new_from_slice(&self.enc)
            .map_err(|_| KeyError::Malformed("data key"))?;
        let aad = self.aad(record, field, kind);
        let ct = cipher
            .encrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| KeyError::Open)?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        Ok(b64_encode(&out))
    }

    /// Open a row sealed by [`AppDataKey::seal`]. Any change to the
    /// ciphertext, its names, its kind or its version refuses.
    pub fn open(
        &self,
        record: &str,
        field: &str,
        kind: RowKind,
        sealed: &str,
    ) -> Result<Vec<u8>, KeyError> {
        let bytes = b64_decode(sealed).ok_or(KeyError::Malformed("sealed row"))?;
        if bytes.len() < 24 + 16 {
            return Err(KeyError::Malformed("sealed row"));
        }
        let cipher = XChaCha20Poly1305::new_from_slice(&self.enc)
            .map_err(|_| KeyError::Malformed("data key"))?;
        let aad = self.aad(record, field, kind);
        cipher
            .decrypt(
                XNonce::from_slice(&bytes[..24]),
                Payload {
                    msg: &bytes[24..],
                    aad: &aad,
                },
            )
            .map_err(|_| KeyError::Open)
    }
}

/// Seal bytes kept on this device (the device key, at rest) under a key
/// derived from `key_material` -- the machine key Krate already keeps for
/// store.secret -- and `context`.
pub fn seal_local(
    key_material: &[u8],
    context: &str,
    plaintext: &[u8],
) -> Result<String, KeyError> {
    let mut info = LOCAL_INFO.to_vec();
    info.extend_from_slice(context.as_bytes());
    let mut key = hkdf(key_material, &info);
    let cipher =
        XChaCha20Poly1305::new_from_slice(&key[..32]).map_err(|_| KeyError::Malformed("local key"));
    key.zeroize();
    let nonce: [u8; 24] = random()?;
    let ct = cipher?
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: context.as_bytes(),
            },
        )
        .map_err(|_| KeyError::Open)?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Ok(b64_encode(&out))
}

pub fn open_local(key_material: &[u8], context: &str, sealed: &str) -> Result<Vec<u8>, KeyError> {
    let bytes = b64_decode(sealed).ok_or(KeyError::Malformed("sealed file"))?;
    if bytes.len() < 24 + 16 {
        return Err(KeyError::Malformed("sealed file"));
    }
    let mut info = LOCAL_INFO.to_vec();
    info.extend_from_slice(context.as_bytes());
    let mut key = hkdf(key_material, &info);
    let cipher =
        XChaCha20Poly1305::new_from_slice(&key[..32]).map_err(|_| KeyError::Malformed("local key"));
    key.zeroize();
    cipher?
        .decrypt(
            XNonce::from_slice(&bytes[..24]),
            Payload {
                msg: &bytes[24..],
                aad: context.as_bytes(),
            },
        )
        .map_err(|_| KeyError::Open)
}

/* ---- base64 (standard, padded): encoding, not cryptography ------------- */

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Strict: padded, nothing but the alphabet.
pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let value = |c: u8| -> Option<u32> { B64.iter().position(|&x| x == c).map(|p| p as u32) };
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let last = index == bytes.len() / 4 - 1;
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            let v = if i >= 4 - pad { 0 } else { value(c)? };
            n |= v << (18 - 6 * i as u32);
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// A rotation's share of the company connections (IC-952): each
/// connection's credential, as the hub listed it (`name`, `base`, `header`,
/// `v`, `sealed`), opened with the version it was sealed to and sealed again
/// to the newest version of `keyring`, which the hub refuses a rotation
/// without. The plaintext lives in this function only. One that does not
/// open here -- changed, or sealed for another address -- is answered
/// `{"unopened": true}`: the hub then marks it to be entered again, rather
/// than leave it on a key a removed member holds.
pub fn reseal_credentials(
    keyring: &Keyring,
    org: &str,
    credentials: &serde_json::Value,
) -> serde_json::Map<String, serde_json::Value> {
    let (version, key) = keyring.current();
    let mut out = serde_json::Map::new();
    for c in credentials.as_array().into_iter().flatten() {
        let Some(name) = c["name"].as_str() else {
            continue;
        };
        let context = connection_context(
            org,
            name,
            c["header"].as_str().unwrap_or(""),
            c["base"].as_str().unwrap_or(""),
        );
        let opened = c["v"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or(KeyError::Open)
            .and_then(|v| open_for_org(keyring, v, &context, c["sealed"].as_str().unwrap_or("")));
        let answer = match opened {
            Ok(mut plain) => {
                use zeroize::Zeroize;
                let sealed = seal_for_org(&key.public(), &context, &plain);
                plain.zeroize();
                match sealed {
                    Ok(sealed) => serde_json::json!({ "v": version, "sealed": sealed }),
                    Err(_) => serde_json::json!({ "unopened": true }),
                }
            }
            Err(_) => serde_json::json!({ "unopened": true }),
        };
        out.insert(name.to_string(), answer);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 9180 test vector A.2.1 (DHKEM(X25519, HKDF-SHA256), HKDF-SHA256,
    /// ChaCha20-Poly1305), base mode: the receiver's DeriveKeyPair and the
    /// first encryption. Proves the crate is the suite we say, not a
    /// look-alike.
    #[test]
    fn hpke_is_rfc_9180_by_its_own_test_vector() {
        let ikm_r =
            unhex("1ac01f181fdf9f352797655161c58b75c656a6cc2716dcb66372da835542e1df").unwrap();
        let pk_r =
            unhex("4310ee97d88cc1f088a5576c77ab0cf5c3ac797f3d95139c6c84b5429c59662a").unwrap();
        let pair = KeyPair::derive(&ikm_r.try_into().unwrap());
        assert_eq!(pair.public().to_vec(), pk_r);
        // The sender's encapsulation, from its ikmE, and the first message.
        let enc =
            unhex("1afa08d3dec047a643885163f1180476fa7ddb54c6a8029ea33f95796bf2ac4a").unwrap();
        let info = unhex("4f6465206f6e2061204772656369616e2055726e").unwrap();
        let aad = unhex("436f756e742d30").unwrap();
        let ct = unhex("1c5250d8034ec2b784ba2cfd69dbdb8af406cfe3ff938e131f0def8c8b60b4db21993c62ce81883d2dd1b51a28").unwrap();
        let pt = unhex("4265617574792069732074727574682c20747275746820626561757479").unwrap();
        let mut sealed = enc;
        sealed.extend_from_slice(&ct);
        assert_eq!(hpke_open(&pair, &info, &aad, &sealed).unwrap(), pt);
    }

    #[test]
    fn a_keyring_opens_only_for_its_device_and_its_organisation() {
        let mut ring = Keyring::create("acme");
        ring.rotate();
        let dana = KeyPair::generate();
        let erin = KeyPair::generate();
        let envelope = ring.seal_to(&dana.public()).unwrap();
        let opened = Keyring::open(&envelope, &dana, "acme").unwrap();
        assert_eq!(opened, ring);
        assert_eq!(opened.versions(), vec![1, 2]);
        assert_eq!(
            Keyring::open(&envelope, &erin, "acme"),
            Err(KeyError::Open),
            "another device"
        );
        assert_eq!(
            Keyring::open(&envelope, &dana, "globex"),
            Err(KeyError::Open),
            "another organisation"
        );
        let mut bytes = b64_decode(&envelope).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(
            Keyring::open(&b64_encode(&bytes), &dana, "acme"),
            Err(KeyError::Open),
            "a changed envelope"
        );
    }

    #[test]
    fn the_recovery_code_opens_the_keyring_and_rotation_reseals_without_it() {
        let ring = Keyring::create("acme");
        let code = RecoveryCode::generate().unwrap();
        let params = RecoveryParams::generate().unwrap();
        let recovery = recovery_keypair(&code, &params).unwrap();
        // A device that rotates later needs only the public half.
        let mut rotated = ring.clone();
        rotated.rotate();
        let envelope = rotated.seal_to(&recovery.public()).unwrap();
        // Typed back by a person, with dashes and lower case.
        let typed = RecoveryCode::parse(&code.display().to_lowercase()).unwrap();
        let again = recovery_keypair(
            &typed,
            &RecoveryParams::from_json(&params.to_json()).unwrap(),
        )
        .unwrap();
        assert_eq!(Keyring::open(&envelope, &again, "acme").unwrap(), rotated);
        let other = recovery_keypair(&RecoveryCode::generate().unwrap(), &params).unwrap();
        assert_eq!(
            Keyring::open(&envelope, &other, "acme"),
            Err(KeyError::Open),
            "the wrong code"
        );
        // Stored parameters weaker than the floor are refused.
        let mut weak = params.to_json();
        weak["m_kib"] = serde_json::json!(8);
        assert!(RecoveryParams::from_json(&weak).is_err());
    }

    #[test]
    fn data_rows_are_opaque_bound_to_their_names_and_refused_when_changed() {
        let ring = Keyring::create("acme");
        let dk = AppDataKey::derive(&ring, 1, "dev.acme.tasks").unwrap();
        let r = dk.record_key("tasks", "t1");
        let f = dk.field_key("tasks", "t1", "title");
        assert_eq!(r.len(), 64);
        assert_ne!(f, dk.field_key("tasks", "t1", "done"));
        // Length-prefixed: ("ab","c") and ("a","bc") are different records.
        assert_ne!(dk.record_key("ab", "c"), dk.record_key("a", "bc"));
        // Another app's key names the same record differently.
        let other = AppDataKey::derive(&ring, 1, "dev.acme.other").unwrap();
        assert_ne!(other.record_key("tasks", "t1"), r);
        let sealed = dk.seal(&r, &f, RowKind::Value, b"Order chairs").unwrap();
        assert!(!b64_decode(&sealed)
            .unwrap()
            .windows(5)
            .any(|w| w == b"Order"));
        assert_eq!(
            dk.open(&r, &f, RowKind::Value, &sealed).unwrap(),
            b"Order chairs"
        );
        let mut bytes = b64_decode(&sealed).unwrap();
        bytes[30] ^= 0x40;
        assert_eq!(
            dk.open(&r, &f, RowKind::Value, &b64_encode(&bytes)),
            Err(KeyError::Open),
            "changed ciphertext"
        );
        assert_eq!(
            dk.open(
                &r,
                &dk.field_key("tasks", "t1", "done"),
                RowKind::Value,
                &sealed
            ),
            Err(KeyError::Open),
            "moved to another field"
        );
        assert_eq!(
            dk.open(&r, &f, RowKind::Tombstone, &sealed),
            Err(KeyError::Open),
            "kind changed"
        );
        assert_eq!(
            other.open(&r, &f, RowKind::Value, &sealed),
            Err(KeyError::Open),
            "another app's key"
        );
    }

    /// The removed member keeps version 1; what is written under version 2
    /// does not open for them.
    #[test]
    fn a_keyring_without_the_new_version_cannot_read_what_it_protects() {
        let mut ring = Keyring::create("acme");
        let kept_by_removed = ring.clone();
        ring.rotate();
        let dk2 = AppDataKey::derive(&ring, 2, "dev.acme.tasks").unwrap();
        let r = dk2.record_key("tasks", "t9");
        let f = dk2.field_key("tasks", "t9", "title");
        let sealed = dk2.seal(&r, &f, RowKind::Value, b"after removal").unwrap();
        assert!(AppDataKey::derive(&kept_by_removed, 2, "dev.acme.tasks").is_none());
        let dk1 = AppDataKey::derive(&kept_by_removed, 1, "dev.acme.tasks").unwrap();
        assert_eq!(
            dk1.open(&r, &f, RowKind::Value, &sealed),
            Err(KeyError::Open)
        );
    }

    /// Prints a `rust_random` value for the vector file: run once by hand
    /// when the vector is remade (`-- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn print_a_rust_sealed_vector_value() {
        let vector: serde_json::Value =
            serde_json::from_str(include_str!("../tests/vectors/org-seal.json")).unwrap();
        let org = KeyPair::derive(&hex32(vector["org_ikm"].as_str().unwrap()).unwrap());
        let case = &vector["rust_random"];
        println!(
            "rust_random.sealed = {}",
            seal_for_org(
                &org.public(),
                case["context"].as_str().unwrap(),
                case["plaintext"].as_str().unwrap().as_bytes()
            )
            .unwrap()
        );
    }

    #[test]
    fn a_secret_sealed_to_the_organisation_opens_only_with_its_keyring() {
        let ring = Keyring::create("acme");
        let (v, key) = ring.current();
        let sealed =
            seal_for_org(&key.public(), "connection:finance-api", b"Bearer s3cret").unwrap();
        assert_eq!(
            open_for_org(&ring, v, "connection:finance-api", &sealed).unwrap(),
            b"Bearer s3cret"
        );
        assert_eq!(
            open_for_org(&ring, v, "connection:other", &sealed),
            Err(KeyError::Open)
        );
        assert_eq!(
            open_for_org(
                &Keyring::create("acme"),
                1,
                "connection:finance-api",
                &sealed
            ),
            Err(KeyError::Open)
        );
    }

    /// The randomness a vector's ephemeral key is made from: its `ikm_e`.
    struct Fixed([u8; 32]);

    impl rand_core::RngCore for Fixed {
        fn next_u32(&mut self) -> u32 {
            unreachable!("the KEM asks for bytes")
        }
        fn next_u64(&mut self) -> u64 {
            unreachable!("the KEM asks for bytes")
        }
        fn fill_bytes(&mut self, dest: &mut [u8]) {
            dest.copy_from_slice(&self.0[..dest.len()]);
        }
    }

    impl rand_core::CryptoRng for Fixed {}

    /// The cross-language vector the dashboard's page test reads too
    /// (docs/landing/studio/org-seal-test.mjs): this crate makes exactly
    /// the bytes the page's hpke-js makes for the same ephemeral key, and
    /// opens what the page sealed with a random one.
    #[test]
    fn a_secret_sealed_by_the_dashboard_opens_here_and_the_bytes_agree() {
        let vector: serde_json::Value =
            serde_json::from_str(include_str!("../tests/vectors/org-seal.json")).unwrap();
        let ikm = hex32(vector["org_ikm"].as_str().unwrap()).unwrap();
        let org = KeyPair::derive(&ikm);
        assert_eq!(org.public_hex(), vector["org_public"].as_str().unwrap());
        let mut keys = BTreeMap::new();
        keys.insert(1, org.clone());
        let ring = Keyring {
            org: "acme".into(),
            keys,
        };
        let opens = |case: &serde_json::Value| {
            let context = case["context"].as_str().unwrap();
            let sealed = case["sealed"].as_str().unwrap();
            let plain = open_for_org(&ring, 1, context, sealed).expect(context);
            assert_eq!(
                String::from_utf8(plain).unwrap(),
                case["plaintext"].as_str().unwrap()
            );
        };
        let cases = vector["cases"].as_array().unwrap();
        assert!(cases.len() >= 2);
        for case in cases {
            let context = case["context"].as_str().unwrap();
            let mut info = ORG_SEAL_INFO.to_vec();
            info.extend_from_slice(context.as_bytes());
            let made = hpke_seal_with(
                &org.public(),
                &info,
                &org.public(),
                case["plaintext"].as_str().unwrap().as_bytes(),
                &mut Fixed(hex32(case["ikm_e"].as_str().unwrap()).unwrap()),
            )
            .unwrap();
            assert_eq!(
                b64_encode(&made),
                case["sealed"].as_str().unwrap(),
                "{context}"
            );
            opens(case);
            // The context is bound: another connection's does not open it.
            assert_eq!(
                open_for_org(
                    &ring,
                    1,
                    &context.replace("acme/", "globex/"),
                    case["sealed"].as_str().unwrap()
                ),
                Err(KeyError::Open)
            );
        }
        opens(&vector["page_random"]);
        opens(&vector["rust_random"]);
    }

    #[test]
    fn a_connections_context_binds_its_name_header_and_address() {
        let ring = Keyring::create("acme");
        let (v, key) = ring.current();
        let at = connection_context(
            "acme",
            "finance-api",
            "Authorization",
            "https://api.acme.example/finance",
        );
        let sealed = seal_for_org(&key.public(), &at, b"Bearer s3cret").unwrap();
        assert_eq!(
            open_for_org(&ring, v, &at, &sealed).unwrap(),
            b"Bearer s3cret"
        );
        for other in [
            connection_context(
                "acme",
                "payroll",
                "Authorization",
                "https://api.acme.example/finance",
            ),
            connection_context(
                "acme",
                "finance-api",
                "X-Debug",
                "https://api.acme.example/finance",
            ),
            connection_context(
                "acme",
                "finance-api",
                "Authorization",
                "https://evil.example/collect",
            ),
            connection_context(
                "globex",
                "finance-api",
                "Authorization",
                "https://api.acme.example/finance",
            ),
        ] {
            assert_eq!(
                open_for_org(&ring, v, &other, &sealed),
                Err(KeyError::Open),
                "{other}"
            );
        }
    }

    #[test]
    fn local_sealing_needs_the_same_machine_key_and_context() {
        let sealed = seal_local(b"machine key", "device", b"secret").unwrap();
        assert_eq!(
            open_local(b"machine key", "device", &sealed).unwrap(),
            b"secret"
        );
        assert_eq!(
            open_local(b"other key", "device", &sealed),
            Err(KeyError::Open)
        );
        assert_eq!(
            open_local(b"machine key", "other", &sealed),
            Err(KeyError::Open)
        );
    }

    #[test]
    fn recovery_codes_read_back_as_typed() {
        let code = RecoveryCode::generate().unwrap();
        assert_eq!(code.display().len(), 26 + 5);
        assert_eq!(RecoveryCode::parse(&code.display()), Some(code.clone()));
        assert_eq!(RecoveryCode::parse("too short"), None);
    }

    #[test]
    fn base64_round_trips_and_is_strict() {
        for case in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            assert_eq!(b64_decode(&b64_encode(case)).as_deref(), Some(case));
        }
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        for bad in ["Zm9", "Zm=v", "Zm9v!A==", "Z==="] {
            assert_eq!(b64_decode(bad), None, "{bad}");
        }
    }
}
