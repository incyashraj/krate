//! Android APK signatures, read and checked, and the challenge that proves
//! a person holds an APK's signing key (IC-800).
//!
//! Two verdicts, never one (IC-800: "keep artifact-integrity and
//! claimant-control verdicts separate"):
//!
//! - [`verify`] answers whether an APK is intact and who signed it: the APK
//!   Signature Scheme v2/v3 signing block, its content digests recomputed
//!   over the file, each signature checked with the signer's key, the
//!   signer's key checked against its certificate, and a v3 rotation
//!   lineage walked link by link. It says nothing about who is asking.
//! - [`challenge`] answers whether the person asking controls that key: a
//!   fresh statement binding package, certificate, the exact APK being
//!   imported, the account and an expiry, put in a small APK the person
//!   signs with their own tools (`apksigner`, Gradle, Android Studio), and
//!   checked by the same verifier.
//!
//! What is supported and what is not is said, never guessed: v1-only (JAR)
//! signatures, more than one signer, DSA keys, RSA keys under 2048 bits and
//! ZIP64 APKs are refused as unsupported, each in words.

mod axml;
pub mod challenge;
mod der;
#[cfg(any(test, feature = "test-signer"))]
pub mod testsign;
mod zip;

#[cfg(any(test, feature = "test-signer"))]
pub use axml::manifest_for_tests;

use ring::digest;
use serde::Serialize;

/// Why an APK was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Not an APK at all.
    NotAnApk(String),
    /// An APK this verifier does not judge, and says so.
    Unsupported(String),
    /// Signed, and the signature does not hold: changed after signing, or
    /// forged.
    Invalid(String),
    /// No v2/v3 signature to check.
    Unsigned,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NotAnApk(why) => write!(f, "this is not an APK: {why}"),
            Refused::Unsupported(what) => write!(f, "{what} is not supported yet"),
            Refused::Invalid(why) => write!(f, "the APK's signature does not hold: {why}"),
            Refused::Unsigned => write!(
                f,
                "the APK has no v2 or v3 signature (a v1-only signature is not supported)"
            ),
        }
    }
}

impl std::error::Error for Refused {}

/// Which scheme the accepted signature came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    V3,
    V2,
}

/// What a verified APK is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verified {
    /// The manifest's package name, when the APK has a manifest.
    pub package: Option<String>,
    pub scheme: Scheme,
    /// SHA-256 of the signing certificate's DER: Android's identity for the
    /// signer, the value `apksigner verify --print-certs` prints.
    pub certificate_sha256: String,
    /// Every certificate the signer rotated from, oldest first, ending with
    /// `certificate_sha256` (v3 proof-of-rotation). Just that one when there
    /// was no rotation.
    pub lineage: Vec<String>,
    /// SHA-256 of the whole file.
    pub apk_sha256: String,
}

const V2_BLOCK: u32 = 0x7109_871a;
const V3_BLOCK: u32 = 0xf053_68c0;
const V31_BLOCK: u32 = 0x1b93_ad61;
const PROOF_OF_ROTATION: u32 = 0x3ba0_6f8c;

/// Check an APK's signature and say who signed it.
pub fn verify(apk: &[u8]) -> Result<Verified, Refused> {
    let layout = zip::layout(apk)?;
    let Some(block) = layout.signing_block else {
        return Err(Refused::Unsigned);
    };
    let pairs = zip::block_pairs(apk, block)?;
    let find = |id| pairs.iter().find(|(i, _)| *i == id).map(|(_, v)| *v);
    let (scheme, block) = match (find(V3_BLOCK), find(V2_BLOCK)) {
        (Some(v3), _) => (Scheme::V3, v3),
        (None, Some(v2)) => (Scheme::V2, v2),
        (None, None) => return Err(Refused::Unsigned),
    };
    let mut signer = only_signer(apk, &layout, block, scheme)?;
    // v3.1 carries a rotated key for newer platforms while v3 keeps the
    // original for older ones (apksigner's default since rotation targets
    // Android 13). The rotated signer is the one in charge now.
    if let Some(v31) = find(V31_BLOCK) {
        let rotated = only_signer(apk, &layout, v31, Scheme::V3)?;
        if !rotated
            .lineage
            .contains(&signer.lineage[signer.lineage.len() - 1])
        {
            return Err(Refused::Invalid(
                "its rotated signer's lineage does not include the original signer".into(),
            ));
        }
        signer = rotated;
    }
    let manifest = zip::read_entry(apk, &layout, "AndroidManifest.xml")?;
    let package = manifest.as_deref().and_then(axml::package_name);
    Ok(Verified {
        package,
        scheme,
        certificate_sha256: hex(digest::digest(&digest::SHA256, &signer.certificate).as_ref()),
        lineage: signer.lineage,
        apk_sha256: hex(digest::digest(&digest::SHA256, apk).as_ref()),
    })
}

/// The names in an APK's central directory (for a caller looking for a
/// particular entry).
pub fn entry_names(apk: &[u8]) -> Result<Vec<String>, Refused> {
    let layout = zip::layout(apk)?;
    zip::names(apk, &layout)
}

/// One entry of an APK, inflated.
pub fn read_entry(apk: &[u8], name: &str) -> Result<Option<Vec<u8>>, Refused> {
    let layout = zip::layout(apk)?;
    zip::read_entry(apk, &layout, name)
}

fn malformed(what: &str) -> Refused {
    Refused::Invalid(format!("its {what} is malformed"))
}

struct Signer {
    certificate: Vec<u8>,
    lineage: Vec<String>,
}

/// The one signer of a v2/v3/v3.1 block, checked.
fn only_signer(
    apk: &[u8],
    layout: &zip::Layout,
    block: &[u8],
    scheme: Scheme,
) -> Result<Signer, Refused> {
    let signers = Reader(block)
        .prefixed()
        .and_then(sequence)
        .ok_or_else(|| malformed("signer list"))?;
    if signers.len() != 1 {
        return Err(Refused::Unsupported(format!(
            "an APK with {} signers (Krate checks one signer)",
            signers.len()
        )));
    }
    check_signer(apk, layout, signers[0], scheme)
}

fn check_signer(
    apk: &[u8],
    layout: &zip::Layout,
    signer: &[u8],
    scheme: Scheme,
) -> Result<Signer, Refused> {
    let mut r = Reader(signer);
    let signed_data = r.prefixed().ok_or_else(|| malformed("signed data"))?;
    if scheme == Scheme::V3 {
        r.u32().ok_or_else(|| malformed("signer"))?; // min SDK
        r.u32().ok_or_else(|| malformed("signer"))?; // max SDK
    }
    let signatures = r
        .prefixed()
        .and_then(sequence)
        .ok_or_else(|| malformed("signatures"))?;
    let public_key = r.prefixed().ok_or_else(|| malformed("public key"))?;

    // The signatures over the signed data, with the signer's key. Every one
    // this verifier can check must hold, and there must be at least one.
    let mut checked = Vec::new();
    for sig in &signatures {
        let mut s = Reader(sig);
        let algorithm = s.u32().ok_or_else(|| malformed("signature"))?;
        let value = s.prefixed().ok_or_else(|| malformed("signature"))?;
        let Some(alg) = Algorithm::from_id(algorithm) else {
            continue;
        };
        verify_signature(alg, public_key, signed_data, value).map_err(|why| {
            Refused::Invalid(format!("the signer's signature does not verify ({why})"))
        })?;
        checked.push(alg);
    }
    if checked.is_empty() {
        return Err(Refused::Unsupported(
            "an APK signed only with algorithms Krate cannot check (DSA, or ECDSA with SHA-512)"
                .into(),
        ));
    }

    // What was signed: the content digests, the certificates, attributes.
    let mut d = Reader(signed_data);
    let digests = d
        .prefixed()
        .and_then(sequence)
        .ok_or_else(|| malformed("digests"))?;
    let certificates = d
        .prefixed()
        .and_then(sequence)
        .ok_or_else(|| malformed("certificates"))?;
    if scheme == Scheme::V3 {
        d.u32().ok_or_else(|| malformed("signed data"))?;
        d.u32().ok_or_else(|| malformed("signed data"))?;
    }
    let attributes = d
        .prefixed()
        .and_then(sequence)
        .ok_or_else(|| malformed("attributes"))?;

    // The first certificate is the signer's, and its key must be the key
    // that made the signatures -- or anyone could name another's
    // certificate and sign with their own key.
    let certificate = *certificates
        .first()
        .ok_or_else(|| malformed("certificates"))?;
    let spki = der::certificate_spki(certificate).ok_or_else(|| malformed("certificate"))?;
    if spki != public_key {
        return Err(Refused::Invalid(
            "the signing key is not the key in the signer's certificate".into(),
        ));
    }

    // The digests must cover every algorithm that signed, and match the file.
    let listed: Vec<(u32, &[u8])> = digests
        .iter()
        .map(|entry| {
            let mut e = Reader(entry);
            Some((e.u32()?, e.prefixed()?))
        })
        .collect::<Option<_>>()
        .ok_or_else(|| malformed("digests"))?;
    for alg in &checked {
        let (_, recorded) = listed
            .iter()
            .find(|(id, _)| *id == alg.id())
            .ok_or_else(|| Refused::Invalid("a signed algorithm has no content digest".into()))?;
        let actual = content_digest(apk, layout, alg.chunk_digest());
        if actual.as_slice() != *recorded {
            return Err(Refused::Invalid(
                "the APK's contents changed after it was signed".into(),
            ));
        }
    }

    let mut lineage = vec![hex(digest::digest(&digest::SHA256, certificate).as_ref())];
    if scheme == Scheme::V3 {
        for attribute in attributes {
            let mut a = Reader(attribute);
            if a.u32() == Some(PROOF_OF_ROTATION) {
                lineage = rotation_lineage(a.rest(), certificate)?;
            }
        }
    }
    Ok(Signer {
        certificate: certificate.to_vec(),
        lineage,
    })
}

/// A v3 proof-of-rotation: each certificate signed into the lineage by the
/// one before it, ending with the signer's own.
fn rotation_lineage(value: &[u8], signer_cert: &[u8]) -> Result<Vec<String>, Refused> {
    let mut r = Reader(value);
    let _version = r.u32().ok_or_else(|| malformed("rotation lineage"))?;
    let mut out = Vec::new();
    let mut previous: Option<(Vec<u8>, u32)> = None;
    let mut last_cert: Vec<u8> = Vec::new();
    while !r.0.is_empty() {
        let mut node = Reader(r.prefixed().ok_or_else(|| malformed("rotation lineage"))?);
        let signed = node
            .prefixed()
            .ok_or_else(|| malformed("rotation lineage"))?;
        let _flags = node.u32().ok_or_else(|| malformed("rotation lineage"))?;
        let algorithm = node.u32().ok_or_else(|| malformed("rotation lineage"))?;
        let signature = node
            .prefixed()
            .ok_or_else(|| malformed("rotation lineage"))?;
        let mut s = Reader(signed);
        let cert = s.prefixed().ok_or_else(|| malformed("rotation lineage"))?;
        let signed_algorithm = s.u32().ok_or_else(|| malformed("rotation lineage"))?;
        if let Some((prev_cert, prev_algorithm)) = &previous {
            if signed_algorithm != *prev_algorithm {
                return Err(Refused::Invalid(
                    "its rotation lineage names the wrong algorithm".into(),
                ));
            }
            let alg = Algorithm::from_id(*prev_algorithm).ok_or_else(|| {
                Refused::Unsupported(
                    "a rotation signed with an algorithm Krate cannot check".into(),
                )
            })?;
            let spki =
                der::certificate_spki(prev_cert).ok_or_else(|| malformed("rotation lineage"))?;
            verify_signature(alg, spki, signed, signature).map_err(|why| {
                Refused::Invalid(format!(
                    "a key rotation in its lineage does not verify ({why})"
                ))
            })?;
        }
        out.push(hex(digest::digest(&digest::SHA256, cert).as_ref()));
        last_cert = cert.to_vec();
        previous = Some((cert.to_vec(), algorithm));
    }
    if out.is_empty() || last_cert != signer_cert {
        return Err(Refused::Invalid(
            "its rotation lineage does not end with the signer's certificate".into(),
        ));
    }
    Ok(out)
}

/// The signature algorithms this verifier checks, by APK algorithm id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Algorithm {
    RsaPssSha256,
    RsaPssSha512,
    RsaPkcs1Sha256,
    RsaPkcs1Sha512,
    EcdsaSha256,
}

impl Algorithm {
    pub(crate) fn from_id(id: u32) -> Option<Self> {
        Some(match id {
            0x0101 => Algorithm::RsaPssSha256,
            0x0102 => Algorithm::RsaPssSha512,
            0x0103 => Algorithm::RsaPkcs1Sha256,
            0x0104 => Algorithm::RsaPkcs1Sha512,
            0x0201 => Algorithm::EcdsaSha256,
            _ => return None,
        })
    }

    pub(crate) fn id(self) -> u32 {
        match self {
            Algorithm::RsaPssSha256 => 0x0101,
            Algorithm::RsaPssSha512 => 0x0102,
            Algorithm::RsaPkcs1Sha256 => 0x0103,
            Algorithm::RsaPkcs1Sha512 => 0x0104,
            Algorithm::EcdsaSha256 => 0x0201,
        }
    }

    fn chunk_digest(self) -> &'static digest::Algorithm {
        match self {
            Algorithm::RsaPssSha512 | Algorithm::RsaPkcs1Sha512 => &digest::SHA512,
            _ => &digest::SHA256,
        }
    }
}

fn verify_signature(
    alg: Algorithm,
    spki: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), String> {
    use ring::signature as sig;
    let (kind, key) = der::spki_key(spki).ok_or("the key could not be read")?;
    let verifier: &dyn sig::VerificationAlgorithm = match (alg, kind) {
        (_, der::KeyKind::Dsa) => return Err("DSA keys are not supported".into()),
        (Algorithm::EcdsaSha256, der::KeyKind::EcP256) => &sig::ECDSA_P256_SHA256_ASN1,
        (Algorithm::EcdsaSha256, der::KeyKind::EcP384) => &sig::ECDSA_P384_SHA256_ASN1,
        (Algorithm::RsaPssSha256, der::KeyKind::Rsa) => &sig::RSA_PSS_2048_8192_SHA256,
        (Algorithm::RsaPssSha512, der::KeyKind::Rsa) => &sig::RSA_PSS_2048_8192_SHA512,
        (Algorithm::RsaPkcs1Sha256, der::KeyKind::Rsa) => &sig::RSA_PKCS1_2048_8192_SHA256,
        (Algorithm::RsaPkcs1Sha512, der::KeyKind::Rsa) => &sig::RSA_PKCS1_2048_8192_SHA512,
        _ => return Err("the key does not match the signature algorithm".into()),
    };
    if kind == der::KeyKind::Rsa && der::rsa_bits(key).is_some_and(|b| b < 2048) {
        return Err("RSA keys under 2048 bits are not supported".into());
    }
    sig::UnparsedPublicKey::new(verifier, key)
        .verify(message, signature)
        .map_err(|_| "the signature is wrong".to_string())
}

/// The v2/v3 content digest: every 1 MiB chunk of the entries, the central
/// directory and the end record (with its directory offset pointing at the
/// signing block) hashed as `0xa5 || len || chunk`, and those hashed as
/// `0x5a || count || digests`.
fn content_digest(
    apk: &[u8],
    layout: &zip::Layout,
    algorithm: &'static digest::Algorithm,
) -> Vec<u8> {
    const CHUNK: usize = 1024 * 1024;
    let entries_end = layout
        .signing_block
        .map(|b| b.0)
        .unwrap_or(layout.cd_offset);
    let mut eocd = apk[layout.eocd_offset..].to_vec();
    eocd[16..20].copy_from_slice(&(entries_end as u32).to_le_bytes());
    let sections: [&[u8]; 3] = [
        &apk[..entries_end],
        &apk[layout.cd_offset..layout.cd_offset + layout.cd_size],
        &eocd,
    ];
    let mut chunk_digests = Vec::new();
    let mut count: u32 = 0;
    for section in sections {
        for chunk in section.chunks(CHUNK) {
            let mut ctx = digest::Context::new(algorithm);
            ctx.update(&[0xa5]);
            ctx.update(&(chunk.len() as u32).to_le_bytes());
            ctx.update(chunk);
            chunk_digests.extend_from_slice(ctx.finish().as_ref());
            count += 1;
        }
    }
    let mut top = digest::Context::new(algorithm);
    top.update(&[0x5a]);
    top.update(&count.to_le_bytes());
    top.update(&chunk_digests);
    top.finish().as_ref().to_vec()
}

/// Length-prefixed little-endian fields, as the signing block writes them.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn u32(&mut self) -> Option<u32> {
        let (head, rest) = self.0.split_at_checked(4)?;
        self.0 = rest;
        Some(u32::from_le_bytes(head.try_into().ok()?))
    }

    fn prefixed(&mut self) -> Option<&'a [u8]> {
        let len = self.u32()? as usize;
        let (head, rest) = self.0.split_at_checked(len)?;
        self.0 = rest;
        Some(head)
    }

    fn rest(&self) -> &'a [u8] {
        self.0
    }
}

/// A length-prefixed sequence of length-prefixed items.
fn sequence(bytes: &[u8]) -> Option<Vec<&[u8]>> {
    let mut r = Reader(bytes);
    let mut out = Vec::new();
    while !r.0.is_empty() {
        out.push(r.prefixed()?);
    }
    Some(out)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests;
