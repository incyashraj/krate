//! Content-addressed identity for a `.krate` bundle.
//!
//! A bundle needs a name that cannot lie about what is inside it. Today two
//! files called `notes.krate` are indistinguishable until you open them, and
//! "the app I verified" and "the app I ran" are the same claim only by trust.
//!
//! This computes a digest over what the bundle *is* -- its component, manifest,
//! and every asset -- rather than over the archive that carries it. That
//! distinction matters: two archives can hold identical contents and differ in
//! bytes (timestamps, compression, entry order), so hashing the file would call
//! the same app two different apps, and re-packing would break every reference
//! to it.
//!
//! This is the primitive the rest of distribution rests on. A registry stores
//! bundles by digest, a signature signs the digest, an update points from one
//! digest to another, and a revocation names a digest. None of that needs a
//! network to be useful now: a person can check today that the file they
//! received is the file that was verified.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

/// Version of the digest computation.
///
/// Recorded alongside the digest because the algorithm is a promise: if the way
/// a digest is computed ever changes, an old digest must remain checkable
/// rather than silently comparing unequal.
pub const DIGEST_SCHEMA: &str = "krate.bundle.digest.v1";

/// A bundle's content identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleDigest {
    /// The digest scheme used, so an old digest stays checkable.
    pub schema: String,
    /// Lowercase hex SHA-256 over the bundle's canonical contents.
    pub digest: String,
    /// Per-entry digests, so a mismatch says *which* file differs rather than
    /// only that something does. Debugging "this bundle is not what you
    /// verified" is otherwise guesswork.
    pub entries: BTreeMap<String, String>,
}

impl BundleDigest {
    /// Short form for logs and user-facing text.
    ///
    /// Twelve hex characters, the same length git uses for a short commit, which
    /// is enough to be unambiguous in practice while staying readable aloud.
    pub fn short(&self) -> String {
        self.digest.chars().take(12).collect()
    }
}

/// Compute the identity of a bundle from its contents.
///
/// `entries` maps each logical path inside the bundle (`manifest.toml`,
/// `code.wasm`, `assets/...`) to its bytes.
///
/// The digest covers the paths as well as the bytes, so moving an asset from
/// one name to another changes the identity -- otherwise two bundles that place
/// the same bytes differently would claim to be the same app.
pub fn digest_entries(entries: &BTreeMap<String, Vec<u8>>) -> BundleDigest {
    let mut per_entry = BTreeMap::new();
    let mut outer = Sha256::new();

    outer.update(DIGEST_SCHEMA.as_bytes());
    outer.update([0u8]);

    // BTreeMap iterates in sorted order, which is what makes this reproducible:
    // the same contents give the same digest regardless of the order they were
    // added or the order a ZIP happens to store them.
    for (path, bytes) in entries {
        let mut entry = Sha256::new();
        entry.update(bytes);
        let entry_digest = hex(&entry.finalize());

        // Length-prefixed so a path and its content cannot be confused for a
        // different split of the same bytes.
        outer.update((path.len() as u64).to_le_bytes());
        outer.update(path.as_bytes());
        outer.update(entry_digest.as_bytes());

        per_entry.insert(path.clone(), entry_digest);
    }

    BundleDigest {
        schema: DIGEST_SCHEMA.to_string(),
        digest: hex(&outer.finalize()),
        entries: per_entry,
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

    #[test]
    fn the_same_contents_always_give_the_same_identity() {
        // The property the whole idea rests on: verify a bundle once, and
        // anyone can check later that they have that same bundle.
        let a = digest_entries(&entries(&[
            ("manifest.toml", b"[app]"),
            ("code.wasm", b"\0asm"),
        ]));
        let b = digest_entries(&entries(&[
            ("code.wasm", b"\0asm"),
            ("manifest.toml", b"[app]"),
        ]));
        assert_eq!(a.digest, b.digest, "insertion order must not matter");
    }

    #[test]
    fn changing_one_byte_changes_the_identity() {
        let a = digest_entries(&entries(&[("code.wasm", b"\0asm\x01")]));
        let b = digest_entries(&entries(&[("code.wasm", b"\0asm\x02")]));
        assert_ne!(a.digest, b.digest);
    }

    #[test]
    fn moving_a_file_changes_the_identity() {
        // Same bytes, different place. If these matched, a bundle could move its
        // component into an asset and still claim to be the verified app.
        let a = digest_entries(&entries(&[("assets/model.bin", b"weights")]));
        let b = digest_entries(&entries(&[("assets/other.bin", b"weights")]));
        assert_ne!(a.digest, b.digest);
    }

    #[test]
    fn a_mismatch_names_the_file_that_differs() {
        let a = digest_entries(&entries(&[
            ("manifest.toml", b"[app]"),
            ("code.wasm", b"one"),
        ]));
        let b = digest_entries(&entries(&[
            ("manifest.toml", b"[app]"),
            ("code.wasm", b"two"),
        ]));
        assert_eq!(a.entries["manifest.toml"], b.entries["manifest.toml"]);
        assert_ne!(a.entries["code.wasm"], b.entries["code.wasm"]);
    }

    #[test]
    fn a_path_cannot_be_confused_with_content() {
        // Without length-prefixing the path, "ab" + "c" and "a" + "bc" could
        // hash the same, and a crafted asset name could impersonate another
        // bundle's identity.
        let a = digest_entries(&entries(&[("ab", b"c")]));
        let b = digest_entries(&entries(&[("a", b"bc")]));
        assert_ne!(a.digest, b.digest);
    }

    #[test]
    fn the_short_form_is_readable_and_stable() {
        let digest = digest_entries(&entries(&[("code.wasm", b"\0asm")]));
        assert_eq!(digest.short().len(), 12);
        assert!(digest.digest.starts_with(&digest.short()));
    }

    #[test]
    fn an_empty_bundle_still_has_an_identity() {
        // Not a useful bundle, but the function must not panic or return
        // something that compares equal to a real one.
        let empty = digest_entries(&BTreeMap::new());
        let real = digest_entries(&entries(&[("code.wasm", b"\0asm")]));
        assert_eq!(empty.digest.len(), 64);
        assert_ne!(empty.digest, real.digest);
    }
}
