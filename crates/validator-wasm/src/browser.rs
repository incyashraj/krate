//! What a browser tab that is an organisation owner's device asks the
//! validator module for (IC-958, the one-minute rule): signing a bundle
//! with crates/bundle, sealing and opening the organisation's keyring and
//! signing ring with crates/orgkeys, and the permission slips and
//! withdrawals the ring signs. The same code the CLI runs, so a browser and
//! a computer write the same bytes; nothing here is a JavaScript copy.
//!
//! Keys come in from the page: made by WebCrypto there (Ed25519 for the
//! signing key, X25519 for the device and the organisation key), kept by
//! the page sealed in IndexedDB. Randomness comes in from the page too --
//! `entropy`, bytes from `crypto.getRandomValues` -- because the module has
//! no imports at all (the hub instantiates it with none). HPKE's ephemeral
//! keys, the recovery code and its salt are drawn from those bytes and
//! nothing else; a call that needs more than it was given fails.
//!
//! The hub's side is [`delegation_to_json`]: it reads a slip a device
//! signed, checking the root's signature, before the store records it.

use std::cell::RefCell;

use krate_bundle::delegation::{
    org_delegation, verify_chain, Revocation, RevocationList, RevocationState, SignedDelegation,
    SignedRevocationList,
};
use krate_bundle::signing::SigningKey;
use krate_orgkeys::signing::SigningRing;
use krate_orgkeys::{
    b64_decode, device_id, hex, recovery_keypair, unhex, KeyPair, Keyring, RecoveryCode,
    RecoveryParams,
};
use serde_json::{json, Value};
use zeroize::Zeroize;

thread_local! {
    /// The page's randomness for the call in progress, consumed front to
    /// back and wiped when the call ends.
    static ENTROPY: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// The custom getrandom 0.3 backend (selected by the build script's
/// `--cfg getrandom_backend="custom"`): the page's bytes, or a refusal.
#[cfg(target_arch = "wasm32")]
#[no_mangle]
unsafe extern "Rust" fn __getrandom_v03_custom(
    dest: *mut u8,
    len: usize,
) -> Result<(), getrandom3::Error> {
    ENTROPY.with(|pool| {
        let mut pool = pool.borrow_mut();
        if pool.len() < len {
            return Err(getrandom3::Error::UNSUPPORTED);
        }
        let mut taken: Vec<u8> = pool.drain(..len).collect();
        std::ptr::copy_nonoverlapping(taken.as_ptr(), dest, len);
        taken.zeroize();
        Ok(())
    })
}

/// The least a call that seals anything must bring.
const MIN_ENTROPY: usize = 256;

pub(crate) fn with_entropy<T>(
    input: &Value,
    f: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let bytes = match input["entropy"].as_str() {
        Some(text) => b64_decode(text).ok_or("entropy is base64")?,
        None => Vec::new(),
    };
    ENTROPY.with(|pool| *pool.borrow_mut() = bytes);
    let out = f();
    ENTROPY.with(|pool| {
        let mut pool = pool.borrow_mut();
        pool.zeroize();
        pool.clear();
    });
    out
}

fn needs_entropy(input: &Value) -> Result<(), String> {
    let have = input["entropy"]
        .as_str()
        .and_then(b64_decode)
        .map_or(0, |b| b.len());
    if have < MIN_ENTROPY {
        return Err(format!(
            "this needs at least {MIN_ENTROPY} bytes of entropy from crypto.getRandomValues"
        ));
    }
    Ok(())
}

pub(crate) fn text<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

pub(crate) fn org_of(input: &Value) -> Result<&str, String> {
    let org = text(input, "org")?;
    if !krate_bundle::valid_org_slug(org) {
        return Err(format!("{org:?} is not an organisation's name"));
    }
    Ok(org)
}

pub(crate) fn key32(input: &Value, key: &str) -> Result<[u8; 32], String> {
    unhex(text(input, key)?)
        .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
        .ok_or_else(|| format!("{key} is 64 hex characters"))
}

pub(crate) fn device_of(input: &Value) -> Result<KeyPair, String> {
    let mut secret = key32(input, "device_secret")?;
    let pair = KeyPair::from_secret(&secret).map_err(|e| e.to_string());
    secret.zeroize();
    pair
}

fn pkcs8_of(input: &Value, key: &str) -> Result<Vec<u8>, String> {
    b64_decode(text(input, key)?).ok_or_else(|| format!("{key} is base64 PKCS#8"))
}

fn signing_key(pkcs8: &[u8]) -> Result<SigningKey, String> {
    SigningKey::from_pkcs8(pkcs8).map_err(|e| format!("not an Ed25519 key: {e}"))
}

fn open_ring(input: &Value, org: &str) -> Result<(SigningRing, SigningKey), String> {
    let ring = SigningRing::open(text(input, "envelope")?, &device_of(input)?, org)
        .map_err(|e| format!("the signing ring: {e}"))?;
    let key = signing_key(ring.pkcs8())?;
    Ok((ring, key))
}

fn recovery_of(input: &Value) -> Result<KeyPair, String> {
    let code = RecoveryCode::parse(text(input, "code")?)
        .ok_or("that is not a recovery code: 26 letters and digits")?;
    let params = RecoveryParams::from_json(&input["params"]).map_err(|e| e.to_string())?;
    let pair = recovery_keypair(&code, &params).map_err(|e| e.to_string())?;
    if Some(pair.public_hex().as_str()) != input["recovery_public"].as_str() {
        return Err("that recovery code does not open this organisation's keys".into());
    }
    Ok(pair)
}

/// One keyring or signing-ring operation, `{"op": ..., ...}`, answered as
/// `{"ok":true, ...}` or `{"ok":false,"problem":"..."}`.
pub fn keys_to_json(bytes: &[u8]) -> String {
    let answer = serde_json::from_slice::<Value>(bytes)
        .map_err(|e| format!("not JSON: {e}"))
        .and_then(|input| with_entropy(&input, || keys_op(&input)));
    match answer {
        Ok(mut value) => {
            value["ok"] = json!(true);
            value.to_string()
        }
        Err(problem) => json!({ "ok": false, "problem": problem }).to_string(),
    }
}

fn keys_op(input: &Value) -> Result<Value, String> {
    let e = |err: krate_orgkeys::KeyError| err.to_string();
    match text(input, "op")? {
        "device_id" => Ok(json!({ "device_id": device_id(&key32(input, "public_key")?) })),
        "public_key" => {
            // The X25519 public half of a private key the page made.
            let mut secret = key32(input, "secret")?;
            let pair = KeyPair::from_secret(&secret).map_err(e);
            secret.zeroize();
            let pair = pair?;
            Ok(json!({ "public_key": pair.public_hex(), "device_id": pair.id() }))
        }
        "keyring_create" => {
            let org = org_of(input)?;
            needs_entropy(input)?;
            let mut secret = key32(input, "secret")?;
            let keyring = Keyring::from_secret(org, &secret).map_err(e);
            secret.zeroize();
            let keyring = keyring?;
            let device = key32(input, "device_public")?;
            let code = RecoveryCode::generate().map_err(e)?;
            let params = RecoveryParams::generate().map_err(e)?;
            let recovery = recovery_keypair(&code, &params).map_err(e)?;
            Ok(json!({
                "version": 1,
                "public_key": keyring.current().1.public_hex(),
                "envelope": keyring.seal_to(&device).map_err(e)?,
                "recovery_code": code.display(),
                "recovery": {
                    "public_key": recovery.public_hex(),
                    "params": params.to_json(),
                    "blob": keyring.seal_to(&recovery.public()).map_err(e)?,
                },
            }))
        }
        "keyring_open" => {
            let org = org_of(input)?;
            let keyring = Keyring::open(text(input, "envelope")?, &device_of(input)?, org)
                .map_err(|err| format!("the organisation key: {err}"))?;
            let (version, current) = keyring.current();
            Ok(
                json!({ "version": version, "public_key": current.public_hex(), "versions": keyring.versions() }),
            )
        }
        "keyring_grant" => {
            let org = org_of(input)?;
            needs_entropy(input)?;
            let keyring = Keyring::open(text(input, "envelope")?, &device_of(input)?, org)
                .map_err(|err| format!("the organisation key: {err}"))?;
            let to = key32(input, "to_public")?;
            Ok(json!({ "version": keyring.current().0, "blob": keyring.seal_to(&to).map_err(e)? }))
        }
        "keyring_rotate" => {
            let org = org_of(input)?;
            needs_entropy(input)?;
            let device = device_of(input)?;
            let mut keyring = Keyring::open(text(input, "envelope")?, &device, org)
                .map_err(|err| format!("the organisation key: {err}"))?;
            let mut secret = key32(input, "secret")?;
            let version = keyring.rotate_to(&secret).map_err(e);
            secret.zeroize();
            let version = version?;
            let mut envelopes = serde_json::Map::new();
            for to in input["to"].as_array().into_iter().flatten() {
                let (Some(id), Some(public)) =
                    (to["device_id"].as_str(), to["public_key"].as_str())
                else {
                    continue;
                };
                let Some(public) =
                    unhex(public).and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
                else {
                    continue;
                };
                // The id is the key's, as on every device.
                if device_id(&public) != id {
                    continue;
                }
                envelopes.insert(id.to_string(), json!(keyring.seal_to(&public).map_err(e)?));
            }
            let mine = keyring.seal_to(&device.public()).map_err(e)?;
            envelopes.insert(device.id(), json!(mine.clone()));
            let recovery_blob = match input["recovery_public"].as_str() {
                Some(_) => Some(
                    keyring
                        .seal_to(&key32(input, "recovery_public")?)
                        .map_err(e)?,
                ),
                None => None,
            };
            // Every company connection's credential, sealed again to the new
            // version: the hub refuses a renewal that leaves one behind, and
            // this is the same code a computer runs (K-1120).
            let credentials =
                krate_orgkeys::reseal_credentials(&keyring, org, &input["credentials"]);
            Ok(json!({
                "version": version, "public_key": keyring.current().1.public_hex(),
                "envelopes": envelopes, "mine": mine, "recovery_blob": recovery_blob,
                "credentials": credentials,
            }))
        }
        "keyring_recover" => {
            let org = org_of(input)?;
            needs_entropy(input)?;
            let recovery = recovery_of(input)?;
            let keyring = Keyring::open(text(input, "blob")?, &recovery, org).map_err(|_| {
                "that recovery code does not open this organisation's keys".to_string()
            })?;
            let device = key32(input, "device_public")?;
            Ok(
                json!({ "version": keyring.current().0, "envelope": keyring.seal_to(&device).map_err(e)? }),
            )
        }
        "ring_create" => {
            let org = org_of(input)?;
            needs_entropy(input)?;
            // WebCrypto's Ed25519 key as its JWK gives it (`d`, `x`, as
            // hex), or a PKCS#8 document ring reads.
            let pkcs8 = if input["seed"].is_string() {
                let mut seed = key32(input, "seed")?;
                let doc = SigningKey::pkcs8_from_seed(&seed, &key32(input, "public")?)
                    .map_err(|_| "that public key is not the seed's".to_string());
                seed.zeroize();
                doc?
            } else {
                pkcs8_of(input, "pkcs8")?
            };
            let public = hex(&signing_key(&pkcs8)?.public_key());
            let ring = SigningRing::new(org, 1, pkcs8);
            let device = key32(input, "device_public")?;
            let recovery_blob = match input["recovery_public"].as_str() {
                Some(_) => Some(ring.seal_to(&key32(input, "recovery_public")?).map_err(e)?),
                None => None,
            };
            Ok(json!({
                "version": 1, "public_key": public,
                "envelope": ring.seal_to(&device).map_err(e)?, "recovery_blob": recovery_blob,
            }))
        }
        "ring_open" => {
            let org = org_of(input)?;
            let (ring, key) = open_ring(input, org)?;
            Ok(json!({ "version": ring.version(), "public_key": hex(&key.public_key()) }))
        }
        "ring_grant" => {
            let org = org_of(input)?;
            needs_entropy(input)?;
            let (ring, _) = open_ring(input, org)?;
            let to = key32(input, "to_public")?;
            Ok(json!({ "version": ring.version(), "blob": ring.seal_to(&to).map_err(e)? }))
        }
        "ring_delegate" => {
            let org = org_of(input)?;
            let (_, key) = open_ring(input, org)?;
            let release = hex(&key32(input, "key")?);
            let at = input["at"].as_u64().ok_or("at is unix seconds")?;
            Ok(json!({ "delegation": org_delegation(&key, org, &release, at) }))
        }
        "ring_revoke" => {
            let org = org_of(input)?;
            let (_, key) = open_ring(input, org)?;
            let at = input["at"].as_u64().ok_or("at is unix seconds")?;
            let mut revocations = Vec::new();
            for r in input["revocations"].as_array().into_iter().flatten() {
                let k = r["key"].as_str().and_then(unhex).filter(|b| b.len() == 32);
                let Some(k) = k else {
                    return Err("each withdrawal names a 64-hex release key".into());
                };
                revocations.push(Revocation {
                    key: hex(&k),
                    compromised_from: r["since"].as_u64().unwrap_or(at),
                    reason: r["reason"]
                        .as_str()
                        .unwrap_or("left the organisation")
                        .to_string(),
                });
            }
            let list = SignedRevocationList::create(
                &key,
                RevocationList {
                    schema: String::new(),
                    root: String::new(),
                    issued_at: at,
                    revocations,
                },
            );
            Ok(json!({ "list": list }))
        }
        "ring_recover" => {
            let org = org_of(input)?;
            needs_entropy(input)?;
            let recovery = recovery_of(input)?;
            let ring = SigningRing::open(text(input, "blob")?, &recovery, org).map_err(|_| {
                "that recovery code does not open this organisation's keys".to_string()
            })?;
            let device = key32(input, "device_public")?;
            Ok(json!({ "version": ring.version(), "envelope": ring.seal_to(&device).map_err(e)? }))
        }
        other => Err(format!("no such operation: {other}")),
    }
}

/// Mark a bundle for an organisation and sign it. The input is a frame: a
/// little-endian u32 length, that many bytes of JSON, then the bundle:
///
/// `{"org": "acme", "signed_at": <unix seconds>, and one of
///   "key": <base64 PKCS#8 of a release key>, "delegation": {slip},
///   "ring": {"envelope": ..., "device_secret": <hex>}  (sign as the organisation)}`
///
/// The answer is the same frame: JSON (`{"ok":true,"digest","namespace",
/// "public_key","signer"}` or `{"ok":false,"problem"}`), then the signed
/// bundle's bytes.
pub fn sign_frame(frame: &[u8]) -> Vec<u8> {
    let (json_text, bundle) = match split_frame(frame) {
        Some(parts) => parts,
        None => {
            return frame_of(
                &json!({ "ok": false, "problem": "not a signing frame" }),
                &[],
            )
        }
    };
    match serde_json::from_slice::<Value>(json_text)
        .map_err(|e| format!("not JSON: {e}"))
        .and_then(|input| sign_op(&input, bundle))
    {
        Ok((meta, bytes)) => frame_of(&meta, &bytes),
        Err(problem) => frame_of(&json!({ "ok": false, "problem": problem }), &[]),
    }
}

fn split_frame(frame: &[u8]) -> Option<(&[u8], &[u8])> {
    let len = u32::from_le_bytes(frame.get(..4)?.try_into().ok()?) as usize;
    let json = frame.get(4..4 + len)?;
    Some((json, &frame[4 + len..]))
}

fn frame_of(meta: &Value, bytes: &[u8]) -> Vec<u8> {
    let text = meta.to_string().into_bytes();
    let mut out = (text.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(&text);
    out.extend_from_slice(bytes);
    out
}

fn sign_op(input: &Value, bundle: &[u8]) -> Result<(Value, Vec<u8>), String> {
    let org = org_of(input)?;
    let signed_at = input["signed_at"]
        .as_u64()
        .ok_or("signed_at is unix seconds")?;
    let (key, delegation, signer) = if input["ring"].is_object() {
        let (_, key) = open_ring(&input["ring"], org)?;
        (key, None, "organisation")
    } else {
        // A release key as PKCS#8, or as WebCrypto's JWK gives it (`seed`
        // and `public`, hex).
        let pkcs8 = if input["seed"].is_string() {
            let mut seed = key32(input, "seed")?;
            let doc = SigningKey::pkcs8_from_seed(&seed, &key32(input, "public")?)
                .map_err(|_| "that public key is not the seed's".to_string());
            seed.zeroize();
            doc?
        } else {
            pkcs8_of(input, "key")?
        };
        let key = signing_key(&pkcs8)?;
        let delegation = match &input["delegation"] {
            Value::Null => None,
            slip => Some(
                serde_json::from_value::<SignedDelegation>(slip.clone())
                    .map_err(|e| format!("the permission slip is not one Krate wrote: {e}"))?,
            ),
        };
        let signer = if delegation.is_some() {
            "release-key"
        } else {
            "key"
        };
        (key, delegation, signer)
    };
    let marked = krate_bundle::mark_bytes(bundle, org).map_err(|e| e.to_string())?;
    let signed = krate_bundle::sign_bytes(&marked, &key, None, signed_at, delegation)
        .map_err(|e| e.to_string())?;
    let meta = json!({
        "ok": true,
        "digest": krate_bundle::sha256_hex(&signed.bytes),
        "namespace": signed.envelope.namespace,
        "public_key": signed.envelope.public_key,
        "signer": signer,
    });
    Ok((meta, signed.bytes))
}

/// The hub's reading of a slip a device signed: `{"delegation": {...},
/// "at": <unix seconds>}` answered `{"ok":true,"root","key","namespace",
/// "not_before","expires"}` once the root's signature over it verifies and
/// it holds at `at`, or `{"ok":false,"problem"}`.
pub fn delegation_to_json(bytes: &[u8]) -> String {
    let refused = |problem: String| json!({ "ok": false, "problem": problem }).to_string();
    let input: Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => return refused(format!("not JSON: {e}")),
    };
    let slip: SignedDelegation = match serde_json::from_value(input["delegation"].clone()) {
        Ok(slip) => slip,
        Err(e) => return refused(format!("not a permission slip Krate wrote: {e}")),
    };
    let Some(at) = input["at"].as_u64() else {
        return refused("at is unix seconds".into());
    };
    let Some(key) = unhex(&slip.delegation.key).filter(|k| k.len() == 32) else {
        return refused("the slip names no release key".into());
    };
    // A namespace the slip covers, to ask the chain about: its own, or one
    // inside its range.
    let namespace = match slip.delegation.namespace.strip_suffix("/*") {
        Some(prefix) => format!("{prefix}/app"),
        None => slip.delegation.namespace.clone(),
    };
    let verdict = verify_chain(
        &slip,
        &namespace,
        &key,
        at,
        &RevocationState::Known(Vec::new()),
    );
    if !verdict.is_authorised() {
        return refused(verdict.to_string());
    }
    let d = &slip.delegation;
    json!({
        "ok": true, "root": d.root, "key": d.key, "namespace": d.namespace,
        "not_before": d.not_before, "expires": d.expires,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use krate_orgkeys::b64_encode;

    fn entropy() -> String {
        b64_encode(&[9u8; 512])
    }

    fn call(input: Value) -> Value {
        serde_json::from_str(&keys_to_json(input.to_string().as_bytes())).unwrap()
    }

    /// A browser that made its keys in WebCrypto gets, through the module,
    /// envelopes a computer opens with crates/orgkeys, a ring that signs
    /// slips the hub's reading accepts, and a signed bundle the validator
    /// judges valid and marked for the organisation.
    #[test]
    fn a_browser_device_seals_signs_and_is_read_like_a_computer() {
        let device = KeyPair::generate();
        let ok_key = KeyPair::generate();
        let made = call(json!({
            "op": "keyring_create", "org": "acme", "secret": hex(ok_key.secret()),
            "device_public": device.public_hex(), "entropy": entropy(),
        }));
        assert_eq!(made["ok"], true, "{made}");
        let keyring = Keyring::open(made["envelope"].as_str().unwrap(), &device, "acme").unwrap();
        assert_eq!(keyring.current().1.public_hex(), ok_key.public_hex());
        assert_eq!(made["public_key"], ok_key.public_hex());

        let opened = call(json!({
            "op": "keyring_open", "org": "acme", "envelope": made["envelope"],
            "device_secret": hex(device.secret()),
        }));
        assert_eq!(opened["version"], 1, "{opened}");

        let pkcs8 = SigningKey::generate_pkcs8().unwrap();
        let root = SigningKey::from_pkcs8(&pkcs8).unwrap();
        let ring = call(json!({
            "op": "ring_create", "org": "acme", "pkcs8": b64_encode(&pkcs8),
            "device_public": device.public_hex(), "recovery_public": made["recovery"]["public_key"],
            "entropy": entropy(),
        }));
        assert_eq!(ring["public_key"], hex(&root.public_key()), "{ring}");
        let held = SigningRing::open(ring["envelope"].as_str().unwrap(), &device, "acme").unwrap();
        assert_eq!(held.pkcs8(), pkcs8.as_slice());
        // The same key as WebCrypto hands it out (seed and public key) makes
        // the same ring a computer opens.
        let from_seed = call(json!({
            "op": "ring_create", "org": "acme", "seed": hex(&pkcs8[16..48]), "public": hex(&pkcs8[51..83]),
            "device_public": device.public_hex(), "entropy": entropy(),
        }));
        assert_eq!(from_seed["public_key"], ring["public_key"], "{from_seed}");
        let reopened =
            SigningRing::open(from_seed["envelope"].as_str().unwrap(), &device, "acme").unwrap();
        assert_eq!(reopened.pkcs8(), pkcs8.as_slice());

        // The recovery code the keyring was made with opens the ring too.
        let recovered = call(json!({
            "op": "ring_recover", "org": "acme", "code": made["recovery_code"],
            "params": made["recovery"]["params"], "recovery_public": made["recovery"]["public_key"],
            "blob": ring["recovery_blob"], "device_public": device.public_hex(), "entropy": entropy(),
        }));
        assert_eq!(recovered["ok"], true, "{recovered}");
        let wrong = call(json!({
            "op": "ring_recover", "org": "acme", "code": "0000000000000000000000000A",
            "params": made["recovery"]["params"], "recovery_public": made["recovery"]["public_key"],
            "blob": ring["recovery_blob"], "device_public": device.public_hex(), "entropy": entropy(),
        }));
        assert_eq!(wrong["ok"], false);

        let release = SigningKey::from_pkcs8(&SigningKey::generate_pkcs8().unwrap()).unwrap();
        let slip = call(json!({
            "op": "ring_delegate", "org": "acme", "envelope": ring["envelope"],
            "device_secret": hex(device.secret()), "key": hex(&release.public_key()), "at": 1_760_000_000u64,
        }));
        let read: Value = serde_json::from_str(&delegation_to_json(
            json!({ "delegation": slip["delegation"], "at": 1_760_000_100u64 })
                .to_string()
                .as_bytes(),
        ))
        .unwrap();
        assert_eq!(read["ok"], true, "{read}");
        assert_eq!(read["root"], hex(&root.public_key()));
        assert_eq!(read["namespace"], "acme/*");
        let late: Value = serde_json::from_str(&delegation_to_json(
            json!({ "delegation": slip["delegation"], "at": 1_760_000_000u64 + 366 * 86_400 })
                .to_string()
                .as_bytes(),
        ))
        .unwrap();
        assert_eq!(late["ok"], false, "a slip that ran out is refused: {late}");
        let mut forged = slip["delegation"].clone();
        forged["delegation"]["namespace"] = json!("globex/*");
        let forged: Value = serde_json::from_str(&delegation_to_json(
            json!({ "delegation": forged, "at": 1_760_000_100u64 })
                .to_string()
                .as_bytes(),
        ))
        .unwrap();
        assert_eq!(forged["ok"], false, "{forged}");

        let list = call(json!({
            "op": "ring_revoke", "org": "acme", "envelope": ring["envelope"],
            "device_secret": hex(device.secret()), "at": 1_760_000_500u64,
            "revocations": [{ "key": hex(&release.public_key()), "since": 1_760_000_400u64 }],
        }));
        let list: SignedRevocationList = serde_json::from_value(list["list"].clone()).unwrap();
        assert_eq!(
            list.verify().unwrap().revocations[0].compromised_from,
            1_760_000_400
        );

        // Signing: with the organisation's own key out of the ring, and
        // with a release key and its slip.
        let bounce = include_bytes!("../../../evidence/ported/bounce.krate");
        {
            let header = json!({ "org": "acme", "signed_at": 1_760_000_100u64,
                    "ring": { "envelope": ring["envelope"], "device_secret": hex(device.secret()) } });
            let mut frame = (header.to_string().len() as u32).to_le_bytes().to_vec();
            frame.extend_from_slice(header.to_string().as_bytes());
            frame.extend_from_slice(bounce);
            let out = sign_frame(&frame);
            let (meta, bytes) = split_frame(&out).unwrap();
            let meta: Value = serde_json::from_slice(meta).unwrap();
            assert_eq!(meta["ok"], true, "{meta}");
            assert_eq!(meta["public_key"], hex(&root.public_key()));
            let judged = krate_bundle::judge_bytes(bytes, MAX).unwrap();
            assert_eq!(judged.signature.state, "valid");
            assert_eq!(judged.org_mark.as_deref(), Some("acme"));
            assert_eq!(meta["digest"], krate_bundle::sha256_hex(bytes));
        }
        let release_pkcs8 = SigningKey::generate_pkcs8().unwrap();
        let release = SigningKey::from_pkcs8(&release_pkcs8).unwrap();
        let slip = org_delegation(&root, "acme", &hex(&release.public_key()), 1_760_000_000);
        let header = json!({ "org": "acme", "signed_at": 1_760_000_100u64,
            "key": b64_encode(&release_pkcs8), "delegation": slip });
        let mut frame = (header.to_string().len() as u32).to_le_bytes().to_vec();
        frame.extend_from_slice(header.to_string().as_bytes());
        frame.extend_from_slice(bounce);
        let out = sign_frame(&frame);
        let (meta, bytes) = split_frame(&out).unwrap();
        let meta: Value = serde_json::from_slice(meta).unwrap();
        assert_eq!(
            (meta["signer"].as_str(), meta["namespace"].as_str()),
            (Some("release-key"), Some("acme/bounce"))
        );
        let judged = krate_bundle::judge_bytes(bytes, MAX).unwrap();
        assert_eq!(
            judged.signature.chain.unwrap().root,
            Some(hex(&root.public_key()))
        );

        let refused = sign_frame(b"\x02\x00\x00\x00{}");
        let (meta, _) = split_frame(&refused).unwrap();
        assert!(String::from_utf8_lossy(meta).contains("org is required"));
    }

    const MAX: u64 = crate::MAX_EXPANDED_BYTES;

    /// Without the page's randomness nothing is sealed: no silent, guessable
    /// fallback.
    /// K-1120: a browser that renews the company key re-seals every
    /// connection credential to the new version, as a computer does, so the
    /// hub takes the renewal; one that does not open here is answered
    /// unopened instead of being left on the old key.
    #[test]
    fn a_browser_renewal_reseals_the_company_connections() {
        let device = KeyPair::generate();
        let first = KeyPair::generate();
        let made = call(json!({
            "op": "keyring_create", "org": "acme", "secret": hex(first.secret()),
            "device_public": device.public_hex(), "entropy": entropy(),
        }));
        assert_eq!(made["ok"], true, "{made}");
        let context = krate_orgkeys::connection_context(
            "acme",
            "finance-api",
            "authorization",
            "https://api.acme.test/",
        );
        let sealed =
            krate_orgkeys::seal_for_org(&first.public(), &context, b"Bearer s3cret").unwrap();
        let elsewhere = krate_orgkeys::seal_for_org(
            &first.public(),
            &krate_orgkeys::connection_context(
                "acme",
                "hr",
                "authorization",
                "https://elsewhere.test/",
            ),
            b"Bearer other",
        )
        .unwrap();
        let next = KeyPair::generate();
        let rot = call(json!({
            "op": "keyring_rotate", "org": "acme", "envelope": made["envelope"],
            "device_secret": hex(device.secret()), "secret": hex(next.secret()), "to": [],
            "entropy": entropy(),
            "credentials": [
                { "name": "finance-api", "base": "https://api.acme.test/", "header": "authorization", "v": 1, "sealed": sealed },
                { "name": "hr", "base": "https://api.acme.test/hr/", "header": "authorization", "v": 1, "sealed": elsewhere },
            ],
        }));
        assert_eq!(rot["ok"], true, "{rot}");
        assert_eq!(rot["version"], 2, "{rot}");
        let back = &rot["credentials"]["finance-api"];
        assert_eq!(back["v"], 2, "sealed again to the new version: {rot}");
        let renewed = Keyring::open(rot["mine"].as_str().unwrap(), &device, "acme").unwrap();
        let opened =
            krate_orgkeys::open_for_org(&renewed, 2, &context, back["sealed"].as_str().unwrap())
                .unwrap();
        assert_eq!(opened, b"Bearer s3cret");
        assert_eq!(
            rot["credentials"]["hr"]["unopened"], true,
            "one sealed for another address is not kept: {rot}"
        );
    }

    #[test]
    fn sealing_without_entropy_is_refused() {
        let device = KeyPair::generate();
        let made = call(json!({
            "op": "keyring_create", "org": "acme", "secret": hex(KeyPair::generate().secret()),
            "device_public": device.public_hex(),
        }));
        assert_eq!(made["ok"], false);
        assert!(
            made["problem"].as_str().unwrap().contains("entropy"),
            "{made}"
        );
        assert_eq!(call(json!({ "op": "nope" }))["ok"], false);
    }
}
