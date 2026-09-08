//! Publisher signatures over a bundle (IC-015).
//!
//! Ed25519 over the canonical statement in [`crate::statement`]. The split
//! matters: the statement says *what* is covered, this says *who* vouched for
//! it, and [`Verdict`] says what a verifier may conclude. Collapsing any two
//! of those is how a signature check ends up meaning less than it appears to.
//!
//! **No account, no network.** A developer holds a key locally and signs
//! offline; a runtime verifies offline. That is a deliberate rule from the
//! identity contract -- "requiring an account merely to run or sign local
//! software would make Krate Cloud part of the file's execution boundary" --
//! not an implementation shortcut.
//!
//! # What a verdict may and may not say
//!
//! A valid signature proves one thing: the holder of this key signed these
//! exact bytes. It does not say the signer is trustworthy, that the app is
//! safe, or that the key belongs to the name printed beside it. Those are
//! separate decisions made against a trust store the person controls, and
//! keeping them separate is why [`Verdict::Valid`] carries the key rather
//! than a boolean.
//!
//! # What is deliberately not here yet
//!
//! Delegated release keys, expiry, revocation and fork lineage are specified
//! (identity contract, "Key hierarchy") and are NOT implemented. This module
//! is the primitive they will be built on. It says so rather than implying a
//! completeness it does not have: [`Verdict`] has no `Revoked` arm, and a
//! caller cannot accidentally treat an unrevoked-because-unchecked signature
//! as one that was checked.

use ring::signature::{self, Ed25519KeyPair, KeyPair, UnparsedPublicKey};

use crate::statement::{Mismatch, SignedStatement};

/// Version of the signature encoding, carried beside every signature.
pub const SIGNATURE_SCHEMA: &str = "krate.bundle.signature.v1";

/// A publisher's signing key.
///
/// Held by the developer, never by Krate. The private half is deliberately
/// not exposed after construction: a key that can be read out of the struct
/// is a key that ends up in a log line.
pub struct SigningKey {
    pair: Ed25519KeyPair,
}

impl SigningKey {
    /// Load a key from a PKCS#8 document.
    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Self, SigningError> {
        Ed25519KeyPair::from_pkcs8(pkcs8)
            .map(|pair| SigningKey { pair })
            .map_err(|_| SigningError::BadKey)
    }

    /// The public half, as raw Ed25519 bytes.
    pub fn public_key(&self) -> Vec<u8> {
        self.pair.public_key().as_ref().to_vec()
    }

    /// Sign a statement.
    ///
    /// Takes the statement rather than arbitrary bytes so a caller cannot
    /// sign something that is not a statement and later have it verified as
    /// one. The signature covers `canonical_bytes()`, nothing else.
    pub fn sign(&self, statement: &SignedStatement) -> Signature {
        Signature {
            schema: SIGNATURE_SCHEMA.to_string(),
            public_key: self.public_key(),
            bytes: self
                .pair
                .sign(&statement.canonical_bytes())
                .as_ref()
                .to_vec(),
        }
    }
}

/// A signature and the key that made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub schema: String,
    /// The public key, so a verifier can check without a directory lookup.
    ///
    /// Carrying the key does NOT make it trusted -- anyone can attach their
    /// own. It makes the signature checkable offline; whether that key is one
    /// you accept is the separate question [`Verdict`] keeps separate.
    pub public_key: Vec<u8>,
    pub bytes: Vec<u8>,
}

/// What a verifier concluded.
///
/// Every arm is a distinct thing that happened, because the fixes differ. A
/// tampered file is not an untrusted signer is not an unsigned app, and one
/// "invalid" for all three would send people looking in the wrong place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The signature matches the statement, and the statement matches the
    /// bundle. Carries the key so the caller can make the trust decision it
    /// is responsible for -- this is not "trusted", it is "genuinely signed
    /// by the holder of this key".
    Valid { public_key: Vec<u8> },
    /// The signature does not match the statement: a wrong key, a corrupted
    /// signature, or bytes signed by nobody.
    BadSignature,
    /// The signature is genuine, but the bundle no longer matches what was
    /// signed. Names every difference (see [`Mismatch`]).
    Tampered { problems: Vec<Mismatch> },
    /// The signature's schema is from a future Krate. Refused rather than
    /// guessed at: a signature is a promise about an exact format.
    UnknownSchema { schema: String },
}

impl Verdict {
    /// True only for [`Verdict::Valid`].
    ///
    /// Named for what it checks. A method called `is_ok` would invite a
    /// caller to read "no error" as "safe to run", which is a different
    /// claim that this type deliberately does not make.
    pub fn is_genuinely_signed(&self) -> bool {
        matches!(self, Verdict::Valid { .. })
    }
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Valid { .. } => write!(f, "signed, and the file matches what was signed"),
            Verdict::BadSignature => write!(
                f,
                "the signature does not match this app -- it was made by a \
                 different key, or it is damaged"
            ),
            Verdict::Tampered { problems } => {
                write!(f, "this app was changed after it was signed:")?;
                for problem in problems {
                    write!(f, "\n  - {problem}")?;
                }
                Ok(())
            }
            Verdict::UnknownSchema { schema } => write!(
                f,
                "this signature uses a newer format ({schema}) than this copy \
                 of Krate understands; update Krate to check it"
            ),
        }
    }
}

/// Check a signature against the statement it claims to cover, and the
/// statement against the bundle in front of us.
///
/// `entries` is the bundle as it exists now. Passing it is not optional and
/// not a convenience: verifying the signature alone would prove that somebody
/// signed *a* statement, while saying nothing about the file being opened.
pub fn verify(
    statement: &SignedStatement,
    signature: &Signature,
    entries: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Verdict {
    if signature.schema != SIGNATURE_SCHEMA {
        return Verdict::UnknownSchema {
            schema: signature.schema.clone(),
        };
    }

    // Signature first. If it does not match the statement, nothing about the
    // statement is worth reporting -- an attacker chose its contents.
    let key = UnparsedPublicKey::new(&signature::ED25519, &signature.public_key);
    if key
        .verify(&statement.canonical_bytes(), &signature.bytes)
        .is_err()
    {
        return Verdict::BadSignature;
    }

    let problems = statement.check_against(entries);
    if !problems.is_empty() {
        return Verdict::Tampered { problems };
    }

    Verdict::Valid {
        public_key: signature.public_key.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SigningError {
    #[error("that is not a usable Ed25519 signing key")]
    BadKey,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use std::collections::BTreeMap;

    fn key() -> SigningKey {
        let rng = SystemRandom::new();
        let doc = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate");
        SigningKey::from_pkcs8(doc.as_ref()).expect("load")
    }

    fn bundle() -> BTreeMap<String, Vec<u8>> {
        [
            ("manifest.toml", b"id = 'dev.krate.notes'".as_slice()),
            ("code.wasm", b"\0asm\x01\0\0\0".as_slice()),
            ("source/src/lib.rs", b"fn main() {}".as_slice()),
        ]
        .iter()
        .map(|(p, b)| (p.to_string(), b.to_vec()))
        .collect()
    }

    #[test]
    fn a_signature_over_an_untouched_bundle_is_valid() {
        let key = key();
        let entries = bundle();
        let statement = SignedStatement::build("pub/notes", "1.0.0", &entries);
        let signature = key.sign(&statement);

        let verdict = verify(&statement, &signature, &entries);
        assert_eq!(
            verdict,
            Verdict::Valid {
                public_key: key.public_key()
            }
        );
        assert!(verdict.is_genuinely_signed());
    }

    /// The whole point: changing the app after signing must be caught, and
    /// reported as tampering rather than as a bad signature.
    #[test]
    fn changing_the_component_after_signing_is_reported_as_tampering() {
        let key = key();
        let entries = bundle();
        let statement = SignedStatement::build("pub/notes", "1.0.0", &entries);
        let signature = key.sign(&statement);

        let mut swapped = entries.clone();
        swapped.insert("code.wasm".into(), b"\0asm-evil".to_vec());

        match verify(&statement, &signature, &swapped) {
            Verdict::Tampered { problems } => {
                assert_eq!(problems.len(), 1);
                assert!(problems[0].to_string().contains("code.wasm"));
            }
            other => panic!("a swapped component must read as tampering, got {other:?}"),
        }
    }

    /// A different key's signature must not verify -- the case that would
    /// make the whole mechanism decorative.
    #[test]
    fn another_publishers_signature_does_not_verify() {
        let entries = bundle();
        let statement = SignedStatement::build("pub/notes", "1.0.0", &entries);
        let impostor = key().sign(&statement);
        // Re-sign the same statement with a different key, then present the
        // impostor's signature alongside the real key's public half.
        let real = key();
        let mut forged = impostor.clone();
        forged.public_key = real.public_key();

        assert_eq!(
            verify(&statement, &forged, &entries),
            Verdict::BadSignature,
            "a signature must not verify against a key that did not make it",
        );
    }

    /// Re-signing the same bundle under a different namespace must not let
    /// one publisher's signature cover another's release.
    #[test]
    fn a_signature_does_not_carry_across_namespaces() {
        let key = key();
        let entries = bundle();
        let mine = SignedStatement::build("me/notes", "1.0.0", &entries);
        let signature = key.sign(&mine);

        let theirs = SignedStatement::build("someone-else/notes", "1.0.0", &entries);
        assert_eq!(
            verify(&theirs, &signature, &entries),
            Verdict::BadSignature,
            "the namespace is inside the signed bytes, so a signature cannot \
             be lifted onto another publisher's claim",
        );
    }

    /// Rolling a release back to an older version must not reuse its
    /// signature under a newer version number.
    #[test]
    fn a_signature_does_not_carry_across_versions() {
        let key = key();
        let entries = bundle();
        let signed = SignedStatement::build("pub/notes", "1.0.0", &entries);
        let signature = key.sign(&signed);

        let renumbered = SignedStatement::build("pub/notes", "9.9.9", &entries);
        assert_eq!(
            verify(&renumbered, &signature, &entries),
            Verdict::BadSignature
        );
    }

    #[test]
    fn a_signature_from_a_newer_krate_is_refused_not_guessed_at() {
        let key = key();
        let entries = bundle();
        let statement = SignedStatement::build("pub/notes", "1.0.0", &entries);
        let mut future = key.sign(&statement);
        future.schema = "krate.bundle.signature.v2".to_string();

        match verify(&statement, &future, &entries) {
            Verdict::UnknownSchema { schema } => assert_eq!(schema, "krate.bundle.signature.v2"),
            other => panic!("an unknown schema must be refused, got {other:?}"),
        }
    }

    /// A verdict must never let "the signature is fine" be read as "the app
    /// is safe to run": only Valid answers true, and it carries the key so
    /// the trust decision stays with the caller.
    #[test]
    fn no_verdict_but_valid_claims_a_genuine_signature() {
        assert!(!Verdict::BadSignature.is_genuinely_signed());
        assert!(!Verdict::Tampered { problems: vec![] }.is_genuinely_signed());
        assert!(!Verdict::UnknownSchema { schema: "x".into() }.is_genuinely_signed());
        assert!(Verdict::Valid {
            public_key: vec![1, 2, 3]
        }
        .is_genuinely_signed());
    }

    /// Appending a file to a signed bundle is refused even though every
    /// signed byte is still present and correct.
    #[test]
    fn adding_a_file_to_a_signed_bundle_is_refused() {
        let key = key();
        let entries = bundle();
        let statement = SignedStatement::build("pub/notes", "1.0.0", &entries);
        let signature = key.sign(&statement);

        let mut padded = entries.clone();
        padded.insert("assets/payload.bin".into(), b"extra".to_vec());

        match verify(&statement, &signature, &padded) {
            Verdict::Tampered { problems } => {
                assert!(problems[0].to_string().contains("nobody signed for it"));
            }
            other => panic!("an added file must be tampering, got {other:?}"),
        }
    }
}
