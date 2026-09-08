//! Release keys delegated by a publisher root (IC-015).
//!
//! The root key establishes publisher continuity and should stay offline or
//! in protected hardware, appearing only for deliberate operations. Day-to-day
//! publishing -- a developer's laptop, a CI runner -- uses a delegated release
//! key, so a stolen release key can be revoked without discarding the
//! publisher's identity.
//!
//! A delegation is itself a signed statement: the root signs a record naming
//! the release key and what it may do. Verifying a bundle therefore checks two
//! signatures, and the questions they answer are different:
//!
//! 1. did this release key sign this bundle? (`signing::verify`)
//! 2. did the root authorise this release key, for this namespace, at the time
//!    the signature was made? (this module)
//!
//! # Time is a parameter, never a clock read
//!
//! Expiry and revocation both depend on "when", and a verifier that read the
//! system clock itself would be answering a different question on every
//! machine -- and could be pushed either way by a wrong clock without ever
//! saying so. The caller passes the instant it is asking about, so the answer
//! is reproducible and a test can ask about any moment.

use serde::{Deserialize, Serialize};

use crate::signing::SigningKey;

/// Version of the delegation encoding.
pub const DELEGATION_SCHEMA: &str = "krate.delegation.v1";

/// What a delegated key is allowed to do.
///
/// One purpose today. It is an enum rather than a bare "yes" so that adding a
/// second purpose later cannot silently widen every delegation already
/// written -- an unknown purpose is refused, not ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Purpose {
    /// Sign releases of an application.
    #[serde(rename = "release")]
    Release,
}

/// A root key's statement that a release key may publish for a namespace.
///
/// Every field the identity contract's "Key hierarchy" lists is here, and all
/// of them are inside the signed bytes: a delegation whose expiry or namespace
/// could be edited after signing would not be a delegation at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delegation {
    pub schema: String,
    /// The publisher root, as lowercase hex Ed25519 public key.
    pub root: String,
    /// The namespace this key may sign for.
    ///
    /// A trailing `/*` delegates a range: `acme/*` covers `acme/notes` and
    /// `acme/clock` but never `acme-corp/notes`. Anything else is an exact
    /// namespace and matches only itself.
    pub namespace: String,
    /// The delegated release key, as lowercase hex.
    pub key: String,
    pub purpose: Purpose,
    /// Unix seconds. A signature made before this is not yet authorised.
    pub not_before: u64,
    /// Unix seconds. A signature made after this is expired.
    pub expires: u64,
    /// Named approvers an organisation requires. Empty means none required.
    ///
    /// Carried and signed even when empty, so adding a requirement later
    /// cannot be done to an existing delegation without breaking it.
    #[serde(default)]
    pub approvers: Vec<String>,
}

impl Delegation {
    /// The exact bytes the root signs.
    ///
    /// Length-prefixed like the bundle statement, and for the same reason: no
    /// two different delegations may serialize to the same bytes by shifting a
    /// field boundary.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        fn field(out: &mut Vec<u8>, bytes: &[u8]) {
            out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            out.extend_from_slice(bytes);
        }
        let mut out = Vec::new();
        field(&mut out, self.schema.as_bytes());
        field(&mut out, self.root.as_bytes());
        field(&mut out, self.namespace.as_bytes());
        field(&mut out, self.key.as_bytes());
        field(&mut out, purpose_name(self.purpose).as_bytes());
        out.extend_from_slice(&self.not_before.to_le_bytes());
        out.extend_from_slice(&self.expires.to_le_bytes());
        out.extend_from_slice(&(self.approvers.len() as u64).to_le_bytes());
        for approver in &self.approvers {
            field(&mut out, approver.as_bytes());
        }
        out
    }

    /// Does this delegation cover `namespace`?
    ///
    /// Exact match, or a `prefix/*` range. The `/` before the star is
    /// required: without it `acme*` would also cover `acme-corp`, which is a
    /// different publisher's name that merely starts the same way.
    pub fn covers(&self, namespace: &str) -> bool {
        match self.namespace.strip_suffix("/*") {
            Some(prefix) => {
                namespace.len() > prefix.len()
                    && namespace.starts_with(prefix)
                    && namespace.as_bytes()[prefix.len()] == b'/'
            }
            None => self.namespace == namespace,
        }
    }
}

fn purpose_name(purpose: Purpose) -> &'static str {
    match purpose {
        Purpose::Release => "release",
    }
}

/// A delegation together with the root's signature over it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedDelegation {
    pub delegation: Delegation,
    /// Lowercase hex signature by the root key.
    pub signature: String,
}

impl SignedDelegation {
    /// Sign a delegation with the publisher root.
    pub fn create(root: &SigningKey, delegation: Delegation) -> SignedDelegation {
        let signature = root.sign_delegation_bytes(&delegation.canonical_bytes());
        SignedDelegation {
            delegation,
            signature: hex(&signature),
        }
    }
}

/// A record that a key must no longer be trusted, and from when.
///
/// Revocation carries a time because the contract distinguishes "a valid
/// signature made before revocation" from "a signature made after
/// compromise". Collapsing those would either strand every release a
/// publisher ever made, or keep honouring an attacker's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revocation {
    /// The revoked release key, lowercase hex.
    pub key: String,
    /// Unix seconds from which signatures by this key are not honoured.
    ///
    /// For an expired subscription or a retired laptop this is "now": earlier
    /// releases stay valid. For a stolen key it is the moment the publisher
    /// believes the theft happened, which may be well before they noticed.
    pub compromised_from: u64,
    /// Why, in the publisher's own words. Shown to a person, never parsed.
    #[serde(default)]
    pub reason: String,
}

/// What is known about revocations right now.
///
/// The distinction the contract requires is between "checked, and this key is
/// fine" and "could not check". A verifier that treated an unreachable
/// revocation list as an empty one would report a compromised key as valid,
/// which is the failure this type exists to make unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevocationState {
    /// The list was consulted and is current.
    Known(Vec<Revocation>),
    /// The list could not be consulted -- offline, or a fetch that failed.
    ///
    /// Carries the last known list, so an already-known revocation is still
    /// honoured while offline. Not knowing about NEW revocations is the gap;
    /// forgetting old ones would be a second, worse one.
    Unknown { last_known: Vec<Revocation> },
}

impl RevocationState {
    fn find(&self, key: &str) -> Option<&Revocation> {
        let list = match self {
            RevocationState::Known(list) => list,
            RevocationState::Unknown { last_known } => last_known,
        };
        list.iter().find(|entry| entry.key == key)
    }

    fn is_current(&self) -> bool {
        matches!(self, RevocationState::Known(_))
    }
}

/// What the delegation chain says about a signature.
///
/// The five states the identity contract requires the runtime to
/// distinguish, and no fewer. Each is a different thing that happened and a
/// different thing for a person to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainVerdict {
    /// The root authorised this key for this namespace, and the signature was
    /// made inside the delegation's window with no revocation against it.
    Authorised,
    /// The root's own signature over the delegation does not verify: the
    /// delegation was forged or altered.
    ForgedDelegation,
    /// The delegation is real but does not cover this namespace.
    WrongNamespace { permitted: String, claimed: String },
    /// The signature was made outside the delegation's validity window.
    Expired { signed_at: u64, expires: u64 },
    /// The signature was made before this delegation began.
    NotYetValid { signed_at: u64, not_before: u64 },
    /// The key was revoked, and this signature was made after the moment the
    /// publisher says it was compromised.
    RevokedAtSigning {
        compromised_from: u64,
        signed_at: u64,
        reason: String,
    },
    /// The key was later revoked, but this signature predates the compromise.
    ///
    /// Still authorised. A publisher revoking a stolen laptop key must not
    /// invalidate every honest release they made with it.
    ValidBeforeRevocation {
        compromised_from: u64,
        signed_at: u64,
    },
    /// Nothing is wrong with the chain, but the revocation list could not be
    /// consulted. Distinct from `Authorised` because a caller may reasonably
    /// treat "cannot check" differently from "checked and clean".
    UnknownRevocationState,
    /// The delegation's purpose is not one this Krate understands.
    UnknownPurpose { purpose: String },
    /// The delegation's schema is from a future Krate.
    UnknownSchema { schema: String },
}

impl ChainVerdict {
    /// May this signature be honoured?
    ///
    /// True for the two states the contract says are valid, and for the
    /// unknown-revocation case -- refusing there would make every offline
    /// verification fail, which would push people to disable checking
    /// entirely. The caller is told which of the three it got, and can be
    /// stricter; it cannot be fooled into thinking it was told nothing.
    pub fn is_authorised(&self) -> bool {
        matches!(
            self,
            ChainVerdict::Authorised
                | ChainVerdict::ValidBeforeRevocation { .. }
                | ChainVerdict::UnknownRevocationState
        )
    }
}

impl std::fmt::Display for ChainVerdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainVerdict::Authorised => write!(f, "the publisher authorised this signing key"),
            ChainVerdict::ForgedDelegation => write!(
                f,
                "the permission slip for this signing key was not signed by \
                 the publisher it names"
            ),
            ChainVerdict::WrongNamespace { permitted, claimed } => write!(
                f,
                "this key may sign for {permitted}, but the app claims to be {claimed}"
            ),
            ChainVerdict::Expired { signed_at, expires } => write!(
                f,
                "the signing key's permission ran out at {expires} and this \
                 was signed at {signed_at}"
            ),
            ChainVerdict::NotYetValid {
                signed_at,
                not_before,
            } => write!(
                f,
                "this was signed at {signed_at}, before the key was allowed to \
                 sign anything ({not_before})"
            ),
            ChainVerdict::RevokedAtSigning {
                compromised_from,
                signed_at,
                reason,
            } => {
                write!(
                    f,
                    "the publisher reported this signing key stolen as of \
                     {compromised_from}, and this was signed afterwards at \
                     {signed_at}"
                )?;
                if !reason.is_empty() {
                    write!(f, " ({reason})")?;
                }
                Ok(())
            }
            ChainVerdict::ValidBeforeRevocation { .. } => write!(
                f,
                "the publisher has since retired this signing key, but this \
                 app was signed while it was still good"
            ),
            ChainVerdict::UnknownRevocationState => write!(
                f,
                "the signature checks out, but whether the key has since been \
                 withdrawn could not be checked from here"
            ),
            ChainVerdict::UnknownPurpose { purpose } => write!(
                f,
                "this key was delegated for {purpose:?}, which this copy of \
                 Krate does not understand"
            ),
            ChainVerdict::UnknownSchema { schema } => write!(
                f,
                "this permission slip uses a newer format ({schema}) than this \
                 copy of Krate understands"
            ),
        }
    }
}

/// Check that a root authorised this release key, for this namespace, at the
/// time the signature was made.
///
/// `signed_at` is when the release signature was made, and `revocations` is
/// what is known about withdrawn keys. Neither is read from the environment:
/// a verifier that consulted its own clock or fetched its own list would give
/// different answers on different machines with nothing saying why.
pub fn verify_chain(
    signed: &SignedDelegation,
    namespace: &str,
    release_key: &[u8],
    signed_at: u64,
    revocations: &RevocationState,
) -> ChainVerdict {
    let delegation = &signed.delegation;
    if delegation.schema != DELEGATION_SCHEMA {
        return ChainVerdict::UnknownSchema {
            schema: delegation.schema.clone(),
        };
    }

    // The root's signature over the delegation. Everything after this trusts
    // the delegation's contents, so nothing may be read from it before.
    let Some(signature_bytes) = unhex(&signed.signature) else {
        return ChainVerdict::ForgedDelegation;
    };
    let Some(root_key) = unhex(&delegation.root) else {
        return ChainVerdict::ForgedDelegation;
    };
    let root = ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, &root_key);
    if root
        .verify(&delegation.canonical_bytes(), &signature_bytes)
        .is_err()
    {
        return ChainVerdict::ForgedDelegation;
    }

    // The delegation is genuine. Does it authorise THIS key?
    if unhex(&delegation.key).as_deref() != Some(release_key) {
        return ChainVerdict::ForgedDelegation;
    }

    if delegation.purpose != Purpose::Release {
        return ChainVerdict::UnknownPurpose {
            purpose: purpose_name(delegation.purpose).to_string(),
        };
    }

    if !delegation.covers(namespace) {
        return ChainVerdict::WrongNamespace {
            permitted: delegation.namespace.clone(),
            claimed: namespace.to_string(),
        };
    }

    if signed_at < delegation.not_before {
        return ChainVerdict::NotYetValid {
            signed_at,
            not_before: delegation.not_before,
        };
    }
    if signed_at > delegation.expires {
        return ChainVerdict::Expired {
            signed_at,
            expires: delegation.expires,
        };
    }

    // Revocation last, because it is the only check whose answer can be
    // "I do not know".
    if let Some(revocation) = revocations.find(&delegation.key) {
        return if signed_at >= revocation.compromised_from {
            ChainVerdict::RevokedAtSigning {
                compromised_from: revocation.compromised_from,
                signed_at,
                reason: revocation.reason.clone(),
            }
        } else {
            ChainVerdict::ValidBeforeRevocation {
                compromised_from: revocation.compromised_from,
                signed_at,
            }
        };
    }

    if !revocations.is_current() {
        return ChainVerdict::UnknownRevocationState;
    }
    ChainVerdict::Authorised
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

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::Ed25519KeyPair;

    fn key() -> SigningKey {
        let rng = SystemRandom::new();
        let doc = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate");
        SigningKey::from_pkcs8(doc.as_ref()).expect("load")
    }

    const HOUR: u64 = 3600;
    const SIGNED_AT: u64 = 1_000 * HOUR;

    fn delegation_for(root: &SigningKey, release: &SigningKey, namespace: &str) -> Delegation {
        Delegation {
            schema: DELEGATION_SCHEMA.to_string(),
            root: hex(&root.public_key()),
            namespace: namespace.to_string(),
            key: hex(&release.public_key()),
            purpose: Purpose::Release,
            not_before: SIGNED_AT - HOUR,
            expires: SIGNED_AT + HOUR,
            approvers: Vec::new(),
        }
    }

    fn clean() -> RevocationState {
        RevocationState::Known(Vec::new())
    }

    #[test]
    fn a_root_delegated_key_signing_in_window_is_authorised() {
        let root = key();
        let release = key();
        let signed = SignedDelegation::create(&root, delegation_for(&root, &release, "acme/notes"));
        let verdict = verify_chain(
            &signed,
            "acme/notes",
            &release.public_key(),
            SIGNED_AT,
            &clean(),
        );
        assert_eq!(verdict, ChainVerdict::Authorised);
        assert!(verdict.is_authorised());
    }

    /// A delegation nobody with the root key wrote must not authorise
    /// anything -- the attack the whole chain exists to stop.
    #[test]
    fn a_delegation_the_root_did_not_sign_is_refused() {
        let root = key();
        let impostor = key();
        let release = key();
        // Signed by someone else, but NAMING the real root.
        let forged =
            SignedDelegation::create(&impostor, delegation_for(&root, &release, "acme/notes"));
        assert_eq!(
            verify_chain(
                &forged,
                "acme/notes",
                &release.public_key(),
                SIGNED_AT,
                &clean()
            ),
            ChainVerdict::ForgedDelegation,
        );
    }

    /// Editing a delegation after the root signed it must break it. The
    /// expiry is the field an attacker would most want to move.
    #[test]
    fn extending_the_expiry_after_signing_breaks_the_delegation() {
        let root = key();
        let release = key();
        let mut signed =
            SignedDelegation::create(&root, delegation_for(&root, &release, "acme/notes"));
        signed.delegation.expires = SIGNED_AT + 10_000 * HOUR;
        assert_eq!(
            verify_chain(
                &signed,
                "acme/notes",
                &release.public_key(),
                SIGNED_AT,
                &clean()
            ),
            ChainVerdict::ForgedDelegation,
            "every field is inside the signed bytes, including the window",
        );
    }

    /// A delegation for one key must not authorise a different key, even
    /// when the delegation itself is genuine.
    #[test]
    fn a_genuine_delegation_does_not_authorise_a_different_key() {
        let root = key();
        let release = key();
        let other = key();
        let signed = SignedDelegation::create(&root, delegation_for(&root, &release, "acme/notes"));
        assert_eq!(
            verify_chain(
                &signed,
                "acme/notes",
                &other.public_key(),
                SIGNED_AT,
                &clean()
            ),
            ChainVerdict::ForgedDelegation,
        );
    }

    #[test]
    fn a_delegation_does_not_reach_another_namespace() {
        let root = key();
        let release = key();
        let signed = SignedDelegation::create(&root, delegation_for(&root, &release, "acme/notes"));
        match verify_chain(
            &signed,
            "acme/clock",
            &release.public_key(),
            SIGNED_AT,
            &clean(),
        ) {
            ChainVerdict::WrongNamespace { permitted, claimed } => {
                assert_eq!(permitted, "acme/notes");
                assert_eq!(claimed, "acme/clock");
            }
            other => panic!("expected a namespace refusal, got {other:?}"),
        }
    }

    /// A range delegation covers a publisher's own apps and nobody else's.
    #[test]
    fn a_range_delegation_stops_at_the_namespace_separator() {
        let root = key();
        let release = key();
        let signed = SignedDelegation::create(&root, delegation_for(&root, &release, "acme/*"));

        for inside in ["acme/notes", "acme/clock", "acme/deep/nested"] {
            assert!(
                verify_chain(&signed, inside, &release.public_key(), SIGNED_AT, &clean())
                    .is_authorised(),
                "{inside} is inside acme/*",
            );
        }
        // The case a naive prefix check gets wrong: a different publisher
        // whose name merely starts the same way.
        for outside in ["acme-corp/notes", "acme", "acmex/notes"] {
            assert!(
                !verify_chain(&signed, outside, &release.public_key(), SIGNED_AT, &clean())
                    .is_authorised(),
                "{outside} is NOT inside acme/* -- a prefix match without the \
                 separator would hand one publisher another's namespace",
            );
        }
    }

    #[test]
    fn a_signature_after_the_delegation_expired_is_refused() {
        let root = key();
        let release = key();
        let signed = SignedDelegation::create(&root, delegation_for(&root, &release, "acme/notes"));
        let late = SIGNED_AT + 2 * HOUR;
        match verify_chain(&signed, "acme/notes", &release.public_key(), late, &clean()) {
            ChainVerdict::Expired { signed_at, expires } => {
                assert_eq!(signed_at, late);
                assert_eq!(expires, SIGNED_AT + HOUR);
            }
            other => panic!("expected expiry, got {other:?}"),
        }
    }

    #[test]
    fn a_signature_before_the_delegation_began_is_refused() {
        let root = key();
        let release = key();
        let signed = SignedDelegation::create(&root, delegation_for(&root, &release, "acme/notes"));
        let early = SIGNED_AT - 2 * HOUR;
        assert!(matches!(
            verify_chain(
                &signed,
                "acme/notes",
                &release.public_key(),
                early,
                &clean()
            ),
            ChainVerdict::NotYetValid { .. }
        ));
    }

    /// The distinction the contract names: a signature made BEFORE the
    /// compromise stays good, one made after does not.
    #[test]
    fn revocation_splits_a_keys_history_at_the_moment_of_compromise() {
        let root = key();
        let release = key();
        let signed = SignedDelegation::create(&root, delegation_for(&root, &release, "acme/notes"));
        let stolen_at = SIGNED_AT;
        let revocations = RevocationState::Known(vec![Revocation {
            key: hex(&release.public_key()),
            compromised_from: stolen_at,
            reason: "laptop stolen".to_string(),
        }]);

        // Signed a minute before the theft: still good.
        let before = verify_chain(
            &signed,
            "acme/notes",
            &release.public_key(),
            stolen_at - 60,
            &revocations,
        );
        assert!(
            before.is_authorised(),
            "revoking a key must not invalidate honest releases made with it, got {before:?}",
        );
        assert!(matches!(before, ChainVerdict::ValidBeforeRevocation { .. }));

        // Signed a minute after: refused, and the reason is carried.
        match verify_chain(
            &signed,
            "acme/notes",
            &release.public_key(),
            stolen_at + 60,
            &revocations,
        ) {
            ChainVerdict::RevokedAtSigning { reason, .. } => assert_eq!(reason, "laptop stolen"),
            other => panic!("expected a revoked verdict, got {other:?}"),
        }
    }

    /// "Could not check" must never be reported as "checked and clean".
    #[test]
    fn an_unreachable_revocation_list_is_not_an_empty_one() {
        let root = key();
        let release = key();
        let signed = SignedDelegation::create(&root, delegation_for(&root, &release, "acme/notes"));

        let offline = RevocationState::Unknown {
            last_known: Vec::new(),
        };
        let verdict = verify_chain(
            &signed,
            "acme/notes",
            &release.public_key(),
            SIGNED_AT,
            &offline,
        );
        assert_eq!(
            verdict,
            ChainVerdict::UnknownRevocationState,
            "offline must be its own answer, not silently the clean one",
        );
        assert_ne!(verdict, ChainVerdict::Authorised);
    }

    /// A revocation already known stays honoured while offline: not knowing
    /// about NEW revocations is the gap, forgetting old ones would be worse.
    #[test]
    fn a_known_revocation_still_bites_while_offline() {
        let root = key();
        let release = key();
        let signed = SignedDelegation::create(&root, delegation_for(&root, &release, "acme/notes"));
        let offline = RevocationState::Unknown {
            last_known: vec![Revocation {
                key: hex(&release.public_key()),
                compromised_from: SIGNED_AT - HOUR,
                reason: String::new(),
            }],
        };
        assert!(matches!(
            verify_chain(
                &signed,
                "acme/notes",
                &release.public_key(),
                SIGNED_AT,
                &offline
            ),
            ChainVerdict::RevokedAtSigning { .. }
        ));
    }

    /// All five states the identity contract requires are reachable and
    /// distinct. If a future change collapses two of them, this fails.
    #[test]
    fn the_five_states_the_contract_requires_are_all_distinguishable() {
        let root = key();
        let release = key();
        let signed = SignedDelegation::create(&root, delegation_for(&root, &release, "acme/notes"));
        let me = release.public_key();
        let revoked = RevocationState::Known(vec![Revocation {
            key: hex(&me),
            compromised_from: SIGNED_AT,
            reason: String::new(),
        }]);

        let states = [
            // 1. a valid signature made before revocation
            verify_chain(&signed, "acme/notes", &me, SIGNED_AT - 60, &revoked),
            // 2. a signature made after compromise
            verify_chain(&signed, "acme/notes", &me, SIGNED_AT + 60, &revoked),
            // 3. an expired delegation
            verify_chain(&signed, "acme/notes", &me, SIGNED_AT + 2 * HOUR, &clean()),
            // 4. an unknown revocation state while offline
            verify_chain(
                &signed,
                "acme/notes",
                &me,
                SIGNED_AT,
                &RevocationState::Unknown {
                    last_known: Vec::new(),
                },
            ),
            // 5. a signature that never matched (wrong key for this slip)
            verify_chain(
                &signed,
                "acme/notes",
                &key().public_key(),
                SIGNED_AT,
                &clean(),
            ),
        ];

        for (i, a) in states.iter().enumerate() {
            for (j, b) in states.iter().enumerate() {
                if i != j {
                    assert_ne!(
                        a, b,
                        "states {i} and {j} must stay distinguishable: the \
                         contract requires the runtime to tell them apart",
                    );
                }
            }
        }
    }

    /// The two documents are domain-separated, so a signature over one can
    /// never be presented as a signature over the other.
    ///
    /// This is asserted in a doc comment on `sign_delegation_bytes`; a claim
    /// like that is worth nothing until something checks it. The schema
    /// string is the first field of both canonical encodings, so the byte
    /// streams cannot collide -- and this proves the encodings differ rather
    /// than trusting that reasoning.
    #[test]
    fn a_delegation_and_a_bundle_statement_never_share_signed_bytes() {
        use crate::statement::SignedStatement;

        let root = key();
        let release = key();
        let delegation = delegation_for(&root, &release, "acme/notes");
        let statement = SignedStatement::build("acme/notes", "1.0.0", &Default::default());

        assert_ne!(
            delegation.canonical_bytes(),
            statement.canonical_bytes(),
            "two documents that sign different things must not encode alike",
        );
        // And the discriminator is the very first field, so no amount of
        // shared content afterwards can make them collide.
        assert!(
            delegation
                .canonical_bytes()
                .starts_with(&(DELEGATION_SCHEMA.len() as u64).to_le_bytes()),
            "the schema length leads the delegation encoding",
        );
        assert_ne!(
            DELEGATION_SCHEMA,
            crate::statement::STATEMENT_SCHEMA,
            "the two schemas are what separate the domains",
        );
    }

    #[test]
    fn a_delegation_from_a_newer_krate_is_refused_not_guessed_at() {
        let root = key();
        let release = key();
        let mut signed =
            SignedDelegation::create(&root, delegation_for(&root, &release, "acme/notes"));
        signed.delegation.schema = "krate.delegation.v2".to_string();
        assert!(matches!(
            verify_chain(
                &signed,
                "acme/notes",
                &release.public_key(),
                SIGNED_AT,
                &clean()
            ),
            ChainVerdict::UnknownSchema { .. }
        ));
    }

    /// Only the three states the contract calls valid may be honoured.
    #[test]
    fn no_refusal_is_reported_as_authorised() {
        for refusal in [
            ChainVerdict::ForgedDelegation,
            ChainVerdict::WrongNamespace {
                permitted: "a".into(),
                claimed: "b".into(),
            },
            ChainVerdict::Expired {
                signed_at: 2,
                expires: 1,
            },
            ChainVerdict::NotYetValid {
                signed_at: 1,
                not_before: 2,
            },
            ChainVerdict::RevokedAtSigning {
                compromised_from: 1,
                signed_at: 2,
                reason: String::new(),
            },
            ChainVerdict::UnknownPurpose {
                purpose: "x".into(),
            },
            ChainVerdict::UnknownSchema { schema: "x".into() },
        ] {
            assert!(!refusal.is_authorised(), "{refusal:?} must not be honoured",);
        }
    }
}
