//! `krate apk`: what an Android app is, and proof that you hold its
//! signing key (IC-800).
//!
//! Two verdicts, kept apart on purpose:
//!
//! - **Signature** (`krate apk check`): is this APK intact, and which
//!   certificate signed it. Local, no network, the same verifier the hub
//!   runs (crates/apksig).
//! - **Key control** (`krate apk prove`): does the person asking hold that
//!   key, today, for this import. The hub issues a one-time statement, you
//!   sign a small APK carrying it with your own tools and key, and the hub
//!   answers with its signed record. It says who held the key -- not who
//!   owns the app in law, which is a separate question (IC-801).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use krate_apksig::challenge::{challenge_apk, check_answer, statement_in, Statement};
use serde_json::{json, Value};

/// What the hub signs a record of key control under, after its own domain
/// (cloud/worker/src/key-challenge.js): never readable as a lease.
pub(crate) const KEY_CONTROL_DOMAIN: &str = "krate-key-control-v1\n";

fn proofs_dir() -> PathBuf {
    crate::krate_home().join("key-proofs")
}

fn read_apk(path: &Path) -> Result<Vec<u8>> {
    let bytes =
        std::fs::read(path).with_context(|| format!("could not read {}", path.display()))?;
    if bytes.len() > 4 * 1024 * 1024 * 1024 {
        bail!("{} is larger than 4 GB", path.display());
    }
    Ok(bytes)
}

/// `krate apk check <file>`.
pub(crate) fn check_command(path: &Path, as_json: bool) -> Result<u8> {
    let bytes = read_apk(path)?;
    let verified = match krate_apksig::verify(&bytes) {
        Ok(verified) => verified,
        Err(refused) => {
            if as_json {
                println!("{}", json!({ "ok": false, "problem": refused.to_string() }));
            } else {
                println!("Signature: {refused}.");
            }
            return Ok(1);
        }
    };
    let proof = recorded_proof(&verified.apk_sha256);
    if as_json {
        println!(
            "{}",
            json!({ "ok": true, "verified": verified, "key_control": proof.as_ref().map(|p| &p["attestation"]) })
        );
        return Ok(0);
    }
    println!(
        "Package:   {}",
        verified.package.as_deref().unwrap_or("(no manifest)")
    );
    println!(
        "Signature: intact, APK Signature Scheme {}",
        match verified.scheme {
            krate_apksig::Scheme::V3 => "v3",
            krate_apksig::Scheme::V2 => "v2",
        }
    );
    println!(
        "Signed by: certificate SHA-256 {}",
        verified.certificate_sha256
    );
    if verified.lineage.len() > 1 {
        println!(
            "  rotated from {}",
            verified.lineage[..verified.lineage.len() - 1].join(", then ")
        );
    }
    match proof {
        Some(_) => println!("Key control: proved for this file (krate apk prove)"),
        None => println!(
            "Key control: not proved. To prove you hold this key: krate apk prove {}",
            path.display()
        ),
    }
    Ok(0)
}

/// `krate apk prove <file> [--answer <signed>]`.
pub(crate) fn prove_command(path: &Path, answer: Option<&Path>) -> Result<u8> {
    let bytes = read_apk(path)?;
    let verified = krate_apksig::verify(&bytes).map_err(|refused| anyhow::anyhow!("{refused}"))?;
    let package = verified
        .package
        .clone()
        .context("this APK has no AndroidManifest.xml naming its package")?;
    let identity = crate::github_auth::current()
        .context("sign in first, so the proof is yours: krate login")?;
    let hub = crate::shared_hub_url();
    match answer {
        None => ask(&hub, &identity.token, &verified, &package, path),
        Some(answer) => give(&hub, &identity.token, &verified, answer),
    }
}

fn ask(
    hub: &str,
    token: &str,
    verified: &krate_apksig::Verified,
    package: &str,
    path: &Path,
) -> Result<u8> {
    let mut request = [0u8; 8];
    getrandom::fill(&mut request).map_err(|_| anyhow::anyhow!("no randomness for a request id"))?;
    let request: String = request.iter().map(|b| format!("{b:02x}")).collect();
    let body = json!({
        "package": package,
        "certificate_sha256": verified.certificate_sha256,
        "apk_sha256": verified.apk_sha256,
        "request": request,
    });
    let answer = post(
        hub,
        token,
        "/port/key-challenge",
        "application/json",
        body.to_string().as_bytes(),
    )?;
    let statement: Statement = serde_json::from_value(answer["statement"].clone())
        .context("the hub's challenge did not read")?;
    if statement.package != package
        || statement.certificate_sha256 != verified.certificate_sha256
        || statement.apk_sha256 != verified.apk_sha256
        || statement.request != request
    {
        bail!("the hub's challenge is not for this APK; nothing was written");
    }
    let file = challenge_path(path);
    std::fs::write(&file, challenge_apk(&statement))
        .with_context(|| format!("could not write {}", file.display()))?;
    std::fs::create_dir_all(proofs_dir())?;
    std::fs::write(
        proofs_dir().join(format!("pending-{}.json", statement.nonce)),
        serde_json::to_vec_pretty(&statement)?,
    )?;
    println!("Sign this file with the key you sign {package} with:");
    println!("  {}", file.display());
    println!();
    println!("For example, with Android's apksigner:");
    println!(
        "  apksigner sign --ks <your keystore> --min-sdk-version 24 {}",
        file.display()
    );
    println!();
    println!("Then send it back (within 15 minutes):");
    println!(
        "  krate apk prove {} --answer {}",
        path.display(),
        file.display()
    );
    println!();
    println!("The file proves only that you hold the key. It cannot be installed.");
    Ok(0)
}

fn challenge_path(apk: &Path) -> PathBuf {
    let stem = apk.file_stem().and_then(|s| s.to_str()).unwrap_or("app");
    apk.with_file_name(format!("{stem}.krate-challenge.apk"))
}

fn give(
    hub: &str,
    token: &str,
    verified: &krate_apksig::Verified,
    answer_path: &Path,
) -> Result<u8> {
    let answer = read_apk(answer_path)?;
    let carried = statement_in(&answer).map_err(|e| anyhow::anyhow!("{e}"))?;
    if carried.apk_sha256 != verified.apk_sha256 {
        bail!("that challenge is for another APK file; ask for one for this file");
    }
    // Locally first, so a wrong key or an unsigned file says so at once and
    // does not use the challenge up.
    let pending = proofs_dir().join(format!("pending-{}.json", carried.nonce));
    if let Some(issued) = std::fs::read(&pending)
        .ok()
        .and_then(|b| serde_json::from_slice::<Statement>(&b).ok())
    {
        if let Err(why) = check_answer(&answer, &issued, crate::unix_now()) {
            bail!("{why}");
        }
    }
    let proof = post(
        hub,
        token,
        "/port/key-challenge/answer",
        "application/vnd.android.package-archive",
        &answer,
    )?;
    let attestation = read_attestation(&proof, &carried)?;
    std::fs::create_dir_all(proofs_dir())?;
    std::fs::write(
        proofs_dir().join(format!("{}.json", verified.apk_sha256)),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    let _ = std::fs::remove_file(pending);
    println!(
        "Proved: you hold the signing key of {} (certificate {}...).",
        attestation["package"].as_str().unwrap_or("?"),
        &verified.certificate_sha256[..16]
    );
    println!("Recorded for this file. It says you hold the key, not who owns the app in law.");
    Ok(0)
}

/// The hub's record, checked: signed with the hub's pinned key under the
/// key-control domain, and about exactly the statement this answer carried.
pub(crate) fn read_attestation(proof: &Value, carried: &Statement) -> Result<Value> {
    let key =
        krate_lease::lease_key().context("this Krate has no hub key to check the proof with")?;
    read_attestation_with(proof, carried, &key)
}

fn read_attestation_with(proof: &Value, carried: &Statement, key: &[u8; 32]) -> Result<Value> {
    let attestation = &proof["attestation"];
    let signature = proof["signature"]
        .as_str()
        .and_then(unhex)
        .filter(|s| s.len() == 64)
        .context("the hub's proof has no signature")?;
    let signed = format!(
        "{KEY_CONTROL_DOMAIN}{}",
        krate_lease::canonical_json(attestation)
    );
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(signed.as_bytes(), &signature)
        .map_err(|_| anyhow::anyhow!("the hub's proof is not signed by the hub's key"))?;
    let same = attestation["kind"] == "android-signing-key-control"
        && attestation["package"] == carried.package.as_str()
        && attestation["certificate_sha256"] == carried.certificate_sha256.as_str()
        && attestation["apk_sha256"] == carried.apk_sha256.as_str()
        && attestation["request"] == carried.request.as_str()
        && attestation["account"] == carried.account.as_str()
        && attestation["nonce"] == carried.nonce.as_str();
    if !same {
        bail!("the hub's proof is about something other than this challenge");
    }
    Ok(attestation.clone())
}

/// A proof on record for this exact APK, if its signature still holds.
fn recorded_proof(apk_sha256: &str) -> Option<Value> {
    let proof: Value = serde_json::from_slice(
        &std::fs::read(proofs_dir().join(format!("{apk_sha256}.json"))).ok()?,
    )
    .ok()?;
    let attestation = &proof["attestation"];
    let carried = Statement {
        v: 1,
        purpose: String::new(),
        package: attestation["package"].as_str()?.to_string(),
        certificate_sha256: attestation["certificate_sha256"].as_str()?.to_string(),
        apk_sha256: attestation["apk_sha256"].as_str()?.to_string(),
        request: attestation["request"].as_str()?.to_string(),
        account: attestation["account"].as_str()?.to_string(),
        nonce: attestation["nonce"].as_str()?.to_string(),
        expires_at: 0,
    };
    (carried.apk_sha256 == apk_sha256 && read_attestation(&proof, &carried).is_ok())
        .then_some(proof)
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

fn post(hub: &str, token: &str, route: &str, content_type: &str, body: &[u8]) -> Result<Value> {
    let url = format!("{}{route}", hub.trim_end_matches('/'));
    let sent = krate_http::post(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", content_type)
        .set("Accept", "application/json")
        .send_bytes(body);
    let text = match sent {
        Ok(response) => response
            .into_string()
            .context("could not read the hub's answer")?,
        Err(ureq::Error::Status(_, response)) => {
            let text = response.into_string().unwrap_or_default();
            let said: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            bail!("{}", said["message"].as_str().unwrap_or(text.trim()));
        }
        Err(err) => bail!("could not reach the hub at {hub}: {err}"),
    };
    serde_json::from_str(&text).context("the hub's answer did not read")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn statement() -> Statement {
        Statement {
            v: 1,
            purpose: "p".into(),
            package: "com.example.owned".into(),
            certificate_sha256: "aa".repeat(32),
            apk_sha256: "bb".repeat(32),
            request: "r1".into(),
            account: "acct".into(),
            nonce: "00".repeat(16),
            expires_at: 10,
        }
    }

    fn signed_proof(seed: &[u8; 32], attestation: Value, domain: &str) -> Value {
        let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(seed).unwrap();
        let bytes = format!("{domain}{}", krate_lease::canonical_json(&attestation));
        let signature: String = pair
            .sign(bytes.as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        json!({ "attestation": attestation, "signature": signature })
    }

    fn attestation_for(s: &Statement) -> Value {
        json!({
            "v": 1, "kind": "android-signing-key-control", "package": s.package,
            "certificate_sha256": s.certificate_sha256, "signed_with": s.certificate_sha256,
            "apk_sha256": s.apk_sha256, "request": s.request, "account": s.account,
            "nonce": s.nonce, "verified_at": 5,
        })
    }

    #[test]
    fn a_proof_reads_only_when_the_hub_signed_exactly_this_challenge() {
        use ring::signature::KeyPair;
        let seed = [7u8; 32];
        let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        let key: [u8; 32] = pair.public_key().as_ref().try_into().unwrap();
        let read = |proof: &Value, s: &Statement| read_attestation_with(proof, s, &key);
        let s = statement();
        let good = signed_proof(&seed, attestation_for(&s), KEY_CONTROL_DOMAIN);
        assert!(read(&good, &s).is_ok());
        // Signed as a lease: not a proof.
        let as_lease = signed_proof(&seed, attestation_for(&s), "krate-lease-v1\n");
        assert!(read(&as_lease, &s).is_err());
        // Signed by another key.
        let other = signed_proof(&[8u8; 32], attestation_for(&s), KEY_CONTROL_DOMAIN);
        assert!(read(&other, &s).is_err());
        // About another challenge.
        let mut elsewhere = s.clone();
        elsewhere.nonce = "11".repeat(16);
        assert!(read(&good, &elsewhere).is_err());
    }

    #[test]
    fn the_challenge_file_sits_beside_the_app() {
        assert_eq!(
            challenge_path(Path::new("/x/My App.apk")),
            PathBuf::from("/x/My App.krate-challenge.apk")
        );
    }
}
