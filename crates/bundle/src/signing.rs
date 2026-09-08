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
use serde::{Deserialize, Serialize};

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

    /// Create a new publisher signing key, as a PKCS#8 document.
    ///
    /// Lives here rather than in the CLI so that the one crate holding the
    /// crypto dependency is the one that understands keys. A caller gets
    /// bytes to store; it never has to know which curve or encoding.
    pub fn generate_pkcs8() -> Result<Vec<u8>, SigningError> {
        let rng = ring::rand::SystemRandom::new();
        Ed25519KeyPair::generate_pkcs8(&rng)
            .map(|doc| doc.as_ref().to_vec())
            .map_err(|_| SigningError::BadKey)
    }

    /// The public half, as raw Ed25519 bytes.
    pub fn public_key(&self) -> Vec<u8> {
        self.pair.public_key().as_ref().to_vec()
    }

    /// Sign a delegation's canonical bytes with this (root) key.
    ///
    /// Deliberately separate from [`Self::sign`], and deliberately NOT a
    /// general "sign these bytes": a key that will sign anything handed to it
    /// can be walked into signing a delegation that was presented as a
    /// bundle statement, or the reverse. The two documents are
    /// domain-separated by their schema strings, which are inside the bytes
    /// each one signs, so a signature over one can never verify as the other.
    ///
    /// Returns the raw signature rather than a [`Signature`], because a
    /// `Signature` is specifically a release signature over a statement and
    /// this is not one.
    pub fn sign_delegation_bytes(&self, bytes: &[u8]) -> Vec<u8> {
        self.pair.sign(bytes).as_ref().to_vec()
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

    // Signature first: if it does not cover this statement, nothing the
    // statement says is worth reporting -- an attacker chose its contents.
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

/// What a bundle actually carries in `signature.json`.
///
/// The envelope holds the signature, the key that made it, and the two
/// claims that are not derivable from the file -- who this release belongs
/// to and what version it says it is. Everything else a verifier needs is
/// recomputed from the bundle's own bytes.
///
/// The STATEMENT is deliberately not stored. Storing it would mean parsing
/// an attacker-supplied document and trusting it to describe the file, and a
/// parser that disagreed with `canonical_bytes()` by one byte would verify a
/// signature over something other than what is on disk. Recomputing removes
/// that class of bug entirely: there is only ever one statement, the one the
/// bundle itself produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureEnvelope {
    pub schema: String,
    /// The application namespace this release claims.
    pub namespace: String,
    /// The version this release claims.
    pub version: String,
    /// Unix seconds when this was signed.
    ///
    /// Copied out of the statement so a verifier can rebuild it. Editing it
    /// here changes nothing an attacker wants: the statement is recomputed
    /// with THIS value, so a moved timestamp simply makes the signature stop
    /// verifying. It is what the delegation's window is measured against.
    #[serde(default)]
    pub signed_at: u64,
    /// Lowercase hex Ed25519 public key.
    pub public_key: String,
    /// Lowercase hex signature over the statement's canonical bytes.
    pub signature: String,
    /// SHA-256 of the statement that was signed.
    ///
    /// Not trusted, and not what decides anything -- the signature does. It
    /// exists so a verifier can say WHY verification failed: a recomputed
    /// statement with a different digest means the file changed, while the
    /// same digest with a failing signature means the key is wrong.
    #[serde(default)]
    pub statement_digest: String,
    /// The entries as they were when signed: path -> sha256.
    ///
    /// Recorded so a verifier can NAME what changed rather than only that
    /// something did. It is a convenience for the message, never an
    /// authority: it is attacker-supplied like the rest of the envelope, and
    /// the signature -- checked against a statement recomputed from the file
    /// -- is what actually decides. An attacker editing this list can change
    /// the wording of a refusal and nothing else.
    #[serde(default)]
    pub signed_entries: std::collections::BTreeMap<String, String>,
    /// The root's permission slip for the key that signed this, when the
    /// publisher uses a delegated release key.
    ///
    /// Travels IN the bundle so the chain can be checked with no network:
    /// a recipient offline on a train can still see that the publisher's
    /// root authorised this key. Absent when a publisher signs directly with
    /// their root, which is the simple case and stays simple.
    ///
    /// Carrying it grants nothing. It is signed by the root, so an attacker
    /// swapping it in has to forge a root signature, which is the thing the
    /// whole hierarchy is built to make hard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation: Option<crate::delegation::SignedDelegation>,
}

impl SignatureEnvelope {
    /// Build the envelope for a signature over a statement.
    pub fn new(statement: &SignedStatement, signature: &Signature) -> SignatureEnvelope {
        SignatureEnvelope {
            schema: signature.schema.clone(),
            namespace: statement.namespace.clone(),
            version: statement.version.clone(),
            signed_at: statement.signed_at,
            public_key: hex(&signature.public_key),
            signature: hex(&signature.bytes),
            statement_digest: statement.digest(),
            delegation: None,
            signed_entries: statement
                .entries
                .iter()
                .map(|e| (e.path.clone(), e.digest.clone()))
                .collect(),
        }
    }

    /// The signature this envelope carries, or `None` if its hex is not hex.
    ///
    /// Malformed hex is not an error to shout about: an envelope nobody can
    /// decode is a signature that does not verify, which is already a verdict
    /// this module has words for.
    pub fn signature(&self) -> Option<Signature> {
        Some(Signature {
            schema: self.schema.clone(),
            public_key: unhex(&self.public_key)?,
            bytes: unhex(&self.signature)?,
        })
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// Verify a bundle against the envelope it carries.
///
/// The statement is recomputed from `entries` -- never parsed from the file --
/// so what is checked is what is on disk. The envelope's recorded statement
/// digest is used only to choose the message: a mismatch there means the file
/// moved underneath a real signature, which is tampering, while a match with a
/// failing signature means the key never signed this.
pub fn verify_envelope(
    envelope: &SignatureEnvelope,
    entries: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Verdict {
    if envelope.schema != SIGNATURE_SCHEMA {
        return Verdict::UnknownSchema {
            schema: envelope.schema.clone(),
        };
    }
    let Some(signature) = envelope.signature() else {
        return Verdict::BadSignature;
    };
    let statement = SignedStatement::build(
        &envelope.namespace,
        &envelope.version,
        envelope.signed_at,
        entries,
    );

    // The file changed since it was signed: the statement it produces now is
    // not the statement that was signed. Say what changed rather than blaming
    // the key.
    if !envelope.statement_digest.is_empty() && statement.digest() != envelope.statement_digest {
        let problems = changed_since_signing(&envelope.signed_entries, entries);
        if !problems.is_empty() {
            return Verdict::Tampered { problems };
        }
    }
    verify(&statement, &signature, entries)
}

/// What changed between the entries recorded at signing time and the file now.
///
/// A statement recomputed from the current bytes matches them by construction,
/// so the comparison has to be against what the envelope recorded. That record
/// is untrusted -- it only shapes the message -- which is why this is called
/// solely after the signature has already failed to cover the recomputed
/// statement, and never to decide that something is fine.
fn changed_since_signing(
    signed: &std::collections::BTreeMap<String, String>,
    now: &std::collections::BTreeMap<String, Vec<u8>>,
) -> Vec<Mismatch> {
    use sha2::{Digest, Sha256};
    let mut problems = Vec::new();
    for (path, signed_digest) in signed {
        match now.get(path) {
            None => problems.push(Mismatch::Missing { path: path.clone() }),
            Some(bytes) => {
                let mut hasher = Sha256::new();
                hasher.update(bytes);
                if hex(&hasher.finalize()) != *signed_digest {
                    problems.push(Mismatch::Content {
                        path: path.clone(),
                        role: crate::statement::Role::of(path),
                    });
                }
            }
        }
    }
    for path in now.keys() {
        if !signed.contains_key(path) {
            problems.push(Mismatch::Uncovered { path: path.clone() });
        }
    }
    problems
}

/// The whole answer about a signed bundle: the signature AND the chain.
///
/// Two questions, deliberately kept apart until the end (IC-015):
///
/// 1. did this key sign this exact file? -- [`Verdict`]
/// 2. did the publisher's root authorise this key, for this namespace, at
///    the moment it signed? -- [`crate::delegation::ChainVerdict`]
///
/// A caller that only asked the first would accept a signature from a key the
/// publisher revoked last year. One that only asked the second would accept a
/// genuine permission slip attached to a file it does not cover. Both must
/// hold, and when one fails the answer must say WHICH -- "the signature is
/// fine but the key was withdrawn" and "the key is fine but the file changed"
/// send a person to entirely different places.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullVerdict {
    pub signature: Verdict,
    /// `None` when the publisher signed directly with their root key, which
    /// needs no delegation. Absent is not a failure.
    pub chain: Option<crate::delegation::ChainVerdict>,
}

impl FullVerdict {
    /// May this app be treated as genuinely published by its namespace?
    ///
    /// Both halves must pass. A missing chain passes only because signing
    /// directly with a root is legitimate -- there is no delegation to check,
    /// not a check that was skipped.
    pub fn is_trustworthy(&self) -> bool {
        self.signature.is_genuinely_signed()
            && self
                .chain
                .as_ref()
                .is_none_or(|chain| chain.is_authorised())
    }

    /// Did the file itself change after signing?
    ///
    /// Separated because this is the one failure that means the bytes in
    /// front of you are not the bytes anybody signed -- as opposed to a key
    /// question, which is about who vouched rather than for what.
    pub fn was_tampered(&self) -> bool {
        matches!(self.signature, Verdict::Tampered { .. })
    }
}

/// Verify both the signature and, when present, the delegation chain.
///
/// `now` is the instant the caller is asking about, used only to decide
/// whether a delegation has expired. It is a parameter for the same reason
/// the chain check takes one: a verifier reading its own clock answers a
/// different question on every machine, and a wrong clock would move the
/// answer with nothing saying so.
pub fn verify_full(
    envelope: &SignatureEnvelope,
    entries: &std::collections::BTreeMap<String, Vec<u8>>,
    revocations: &crate::delegation::RevocationState,
) -> FullVerdict {
    let signature = verify_envelope(envelope, entries);
    let chain = envelope.delegation.as_ref().map(|delegation| {
        let key = unhex(&envelope.public_key).unwrap_or_default();
        crate::delegation::verify_chain(
            delegation,
            &envelope.namespace,
            &key,
            // The signing time from the SIGNED statement, not from the
            // verifier's clock: the question is whether the delegation
            // covered the key when it signed, not whether it covers it now.
            envelope.signed_at,
            revocations,
        )
    });
    FullVerdict { signature, chain }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SigningError {
    #[error("that is not a usable Ed25519 signing key")]
    BadKey,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed signing time. Tests must never read the clock: a test whose
    /// answer depends on when it runs is a test that fails one day for a
    /// reason nobody can reproduce.
    const SIGNED_AT_FIXTURE: u64 = 1_700_000_000;
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
        let statement = SignedStatement::build("pub/notes", "1.0.0", SIGNED_AT_FIXTURE, &entries);
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
        let statement = SignedStatement::build("pub/notes", "1.0.0", SIGNED_AT_FIXTURE, &entries);
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
        let statement = SignedStatement::build("pub/notes", "1.0.0", SIGNED_AT_FIXTURE, &entries);
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
        let mine = SignedStatement::build("me/notes", "1.0.0", SIGNED_AT_FIXTURE, &entries);
        let signature = key.sign(&mine);

        let theirs =
            SignedStatement::build("someone-else/notes", "1.0.0", SIGNED_AT_FIXTURE, &entries);
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
        let signed = SignedStatement::build("pub/notes", "1.0.0", SIGNED_AT_FIXTURE, &entries);
        let signature = key.sign(&signed);

        let renumbered = SignedStatement::build("pub/notes", "9.9.9", SIGNED_AT_FIXTURE, &entries);
        assert_eq!(
            verify(&renumbered, &signature, &entries),
            Verdict::BadSignature
        );
    }

    #[test]
    fn a_signature_from_a_newer_krate_is_refused_not_guessed_at() {
        let key = key();
        let entries = bundle();
        let statement = SignedStatement::build("pub/notes", "1.0.0", SIGNED_AT_FIXTURE, &entries);
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

    /// The envelope's bookkeeping is UNTRUSTED and must never be able to
    /// talk a verifier into accepting a file.
    ///
    /// An attacker who swaps the component can rewrite `signed_entries` and
    /// `statement_digest` to match it, so the envelope agrees with the file
    /// perfectly. Only the signature can tell: it covers a statement built
    /// from the real bytes, and no edit to the envelope changes that.
    ///
    /// Both orderings are tested because they take different branches. A
    /// version that returned Valid as soon as the recorded list agreed
    /// passed every other test in this file.
    #[test]
    fn a_forged_entry_list_cannot_make_a_swapped_component_verify() {
        let key = key();
        let entries = bundle();
        let statement = SignedStatement::build("pub/notes", "1.0.0", SIGNED_AT_FIXTURE, &entries);
        let envelope = SignatureEnvelope::new(&statement, &key.sign(&statement));

        let mut swapped = entries.clone();
        swapped.insert("code.wasm".into(), b"\0asm-evil".to_vec());
        let forged_statement =
            SignedStatement::build("pub/notes", "1.0.0", SIGNED_AT_FIXTURE, &swapped);
        let forged_entries: std::collections::BTreeMap<String, String> = forged_statement
            .entries
            .iter()
            .map(|e| (e.path.clone(), e.digest.clone()))
            .collect();

        // Both fields rewritten: the envelope is internally consistent.
        let mut fully_forged = envelope.clone();
        fully_forged.statement_digest = forged_statement.digest();
        fully_forged.signed_entries = forged_entries.clone();
        assert_eq!(
            verify_envelope(&fully_forged, &swapped),
            Verdict::BadSignature,
            "an envelope that agrees with a swapped file must still be refused",
        );

        // Only the entry list rewritten, so the stale statement digest sends
        // this down the "did the file change" branch instead.
        let mut half_forged = envelope.clone();
        half_forged.signed_entries = forged_entries;
        let sneaky = verify_envelope(&half_forged, &swapped);
        assert!(
            !sneaky.is_genuinely_signed(),
            "a rewritten entry list must not verify a file the signature \
             never covered, got {sneaky:?}",
        );
    }

    /// The honest path through verify_envelope, so the test above is not
    /// passing merely because everything is refused.
    #[test]
    fn an_untouched_bundle_verifies_through_the_envelope() {
        let key = key();
        let entries = bundle();
        let statement = SignedStatement::build("pub/notes", "1.0.0", SIGNED_AT_FIXTURE, &entries);
        let envelope = SignatureEnvelope::new(&statement, &key.sign(&statement));
        assert!(verify_envelope(&envelope, &entries).is_genuinely_signed());
    }

    /// Delegation is genuinely wired, not merely available (IC-015).
    ///
    /// A release key the publisher revoked must be caught through the same
    /// call a runtime makes -- not only inside delegation.rs's own tests.
    /// Before this, verify_chain existed and nothing outside its module ever
    /// called it, so a revoked key signed apps that verified perfectly.
    #[test]
    fn a_revoked_release_key_is_caught_through_the_bundle_path() {
        use crate::delegation::{
            Delegation, Purpose, Revocation, RevocationState, SignedDelegation, DELEGATION_SCHEMA,
        };

        let root = key();
        let release = key();
        let entries = bundle();
        let signed_at = SIGNED_AT_FIXTURE;

        let delegation = Delegation {
            schema: DELEGATION_SCHEMA.to_string(),
            root: hex(&root.public_key()),
            namespace: "acme/*".to_string(),
            key: hex(&release.public_key()),
            purpose: Purpose::Release,
            not_before: signed_at - 3600,
            expires: signed_at + 3600,
            approvers: Vec::new(),
        };
        let statement = SignedStatement::build("acme/notes", "1.0.0", signed_at, &entries);
        let mut envelope = SignatureEnvelope::new(&statement, &release.sign(&statement));
        envelope.delegation = Some(SignedDelegation::create(&root, delegation));

        // Clean: both halves pass.
        let good = verify_full(&envelope, &entries, &RevocationState::Known(Vec::new()));
        assert!(
            good.is_trustworthy(),
            "a delegated key inside its window must verify: {good:?}",
        );

        // The publisher reports the key stolen BEFORE it signed this.
        let revoked = RevocationState::Known(vec![Revocation {
            key: hex(&release.public_key()),
            compromised_from: signed_at - 60,
            reason: "key leaked in a CI log".to_string(),
        }]);
        let bad = verify_full(&envelope, &entries, &revoked);
        assert!(
            !bad.is_trustworthy(),
            "a key revoked before it signed must not be trusted: {bad:?}",
        );
        // And the signature itself is still fine -- the failure must be
        // reported as a key problem, not as a changed file.
        assert!(
            bad.signature.is_genuinely_signed(),
            "the bytes were not tampered with; only the key was withdrawn",
        );
        assert!(!bad.was_tampered());
    }

    /// Signing directly with a root key needs no delegation, and the absence
    /// of one must not read as a skipped check.
    #[test]
    fn a_bundle_signed_directly_by_a_root_needs_no_delegation() {
        use crate::delegation::RevocationState;
        let root = key();
        let entries = bundle();
        let statement = SignedStatement::build("acme/notes", "1.0.0", SIGNED_AT_FIXTURE, &entries);
        let envelope = SignatureEnvelope::new(&statement, &root.sign(&statement));

        let verdict = verify_full(&envelope, &entries, &RevocationState::Known(Vec::new()));
        assert!(verdict.chain.is_none(), "there is no delegation to check");
        assert!(verdict.is_trustworthy());
    }

    /// A delegation for one namespace must not authorise a release in
    /// another, even when the release signature itself is perfect.
    #[test]
    fn a_delegation_does_not_stretch_to_another_namespace_through_the_bundle() {
        use crate::delegation::{
            Delegation, Purpose, RevocationState, SignedDelegation, DELEGATION_SCHEMA,
        };
        let root = key();
        let release = key();
        let entries = bundle();
        let signed_at = SIGNED_AT_FIXTURE;

        let delegation = Delegation {
            schema: DELEGATION_SCHEMA.to_string(),
            root: hex(&root.public_key()),
            namespace: "acme/notes".to_string(),
            key: hex(&release.public_key()),
            purpose: Purpose::Release,
            not_before: signed_at - 3600,
            expires: signed_at + 3600,
            approvers: Vec::new(),
        };
        // The release claims a DIFFERENT namespace than the slip permits.
        let statement = SignedStatement::build("acme/clock", "1.0.0", signed_at, &entries);
        let mut envelope = SignatureEnvelope::new(&statement, &release.sign(&statement));
        envelope.delegation = Some(SignedDelegation::create(&root, delegation));

        let verdict = verify_full(&envelope, &entries, &RevocationState::Known(Vec::new()));
        assert!(
            verdict.signature.is_genuinely_signed(),
            "the file itself is untouched",
        );
        assert!(
            !verdict.is_trustworthy(),
            "but the key was never authorised for this namespace: {verdict:?}",
        );
    }

    /// Appending a file to a signed bundle is refused even though every
    /// signed byte is still present and correct.
    #[test]
    fn adding_a_file_to_a_signed_bundle_is_refused() {
        let key = key();
        let entries = bundle();
        let statement = SignedStatement::build("pub/notes", "1.0.0", SIGNED_AT_FIXTURE, &entries);
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
