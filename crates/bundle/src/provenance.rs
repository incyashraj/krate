//! Content-addressed identity for a `.krate` bundle.
//!
//! A bundle needs a name that cannot lie about what is inside it. Today two
//! files called `notes.krate` are indistinguishable until you open them, and
//! "the app I verified" and "the app I ran" are the same claim only by trust.
//!
//! A bundle has THREE identities, because it answers three different
//! questions and they change independently (IC-712). See [`Layer`].
//!
//! The one this module started with -- component, manifest and assets -- is
//! [`Layer::Execution`]: what runs. It deliberately ignores the archive's
//! bytes, because two archives can hold identical contents and differ in
//! timestamps, compression or entry order, so hashing the file would call the
//! same app two different apps and re-packing would break every reference.
//!
//! What it also ignored was `source/` and `sdk/`, while being printed under
//! the heading "Identity". Two bundles whose source differed showed one value
//! on the screen where a person decides whether to trust an app (K-245), so
//! [`Layer::Project`] now covers what can be rebuilt, and
//! [`digest_archive_bytes`] covers the file itself. Never substitute one for
//! another: that substitution IS the defect.
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

/// Which layer of a bundle's identity a digest is about (IC-712).
///
/// One digest cannot answer every question, and using one as though it did is
/// how a partial digest gets printed under the word "Identity". A bundle has
/// three honest identities and they change independently:
///
/// - [`Layer::Execution`] -- what actually runs: the manifest, the component
///   and the assets. Two bundles with this digest equal behave identically,
///   whatever else they carry. This is the right key for a cache, a
///   capability decision, or "is this the app that was verified to work".
/// - [`Layer::Project`] -- everything a person could rebuild or read: the
///   execution set plus `source/` and `sdk/`. Two bundles equal here are the
///   same editable project. Different source with identical behaviour is a
///   real difference, and this is the layer that notices.
/// - [`Layer::Archive`] -- the file's own bytes. Repacking changes it while
///   the other two hold, which is exactly why it must never be the identity
///   a person is asked to compare: it would call one app two apps.
///
/// Never substitute one for another. That substitution is the defect
/// (K-245): a digest over the execution set was labelled whole-bundle
/// identity, so two bundles whose source differed printed one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Execution,
    Project,
    Archive,
}

impl Layer {
    /// The schema tag recorded with a digest of this layer.
    ///
    /// Distinct per layer so two layers of the same bundle can never compare
    /// equal by accident, and so an old digest stays checkable.
    pub fn schema(self) -> &'static str {
        match self {
            // The historical name. Kept byte-for-byte because digests already
            // recorded elsewhere carry it, and renaming it would silently
            // invalidate them.
            Layer::Execution => DIGEST_SCHEMA,
            Layer::Project => "krate.bundle.digest.project.v1",
            Layer::Archive => "krate.bundle.digest.archive.v1",
        }
    }

    /// What this layer means, in the words a person reading a screen needs.
    pub fn describe(self) -> &'static str {
        match self {
            Layer::Execution => "what runs (manifest, component, assets)",
            Layer::Project => "what can be rebuilt (adds source and SDK)",
            Layer::Archive => "the exact file bytes",
        }
    }

    /// Does a bundle entry belong in this layer?
    ///
    /// `Archive` is not decided here: it is over the file's own bytes, not
    /// over entries, so asking this of it is a programming error and it
    /// answers false rather than pretending.
    pub fn includes(self, entry: &str) -> bool {
        match self {
            Layer::Execution => !entry.starts_with("source/") && !entry.starts_with("sdk/"),
            Layer::Project => true,
            Layer::Archive => false,
        }
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
///
/// This is the [`Layer::Execution`] identity. For the project identity, or
/// for a digest that says which layer it is, use [`digest_layer`].
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

/// Compute one layer's identity from a bundle's entries.
///
/// The same canonical rule as [`digest_entries`] -- sorted paths,
/// length-prefixed, schema-tagged -- over the subset of entries the layer
/// covers. Because the schema tag differs per layer, an execution digest and
/// a project digest of the same bundle can never collide, even when the
/// bundle carries no source at all and the two cover identical entries.
pub fn digest_layer(layer: Layer, entries: &BTreeMap<String, Vec<u8>>) -> BundleDigest {
    let mut per_entry = BTreeMap::new();
    let mut outer = Sha256::new();

    outer.update(layer.schema().as_bytes());
    outer.update([0u8]);

    for (path, bytes) in entries {
        if !layer.includes(path) {
            continue;
        }
        let mut entry = Sha256::new();
        entry.update(bytes);
        let entry_digest = hex(&entry.finalize());

        outer.update((path.len() as u64).to_le_bytes());
        outer.update(path.as_bytes());
        outer.update(entry_digest.as_bytes());

        per_entry.insert(path.clone(), entry_digest);
    }

    BundleDigest {
        schema: layer.schema().to_string(),
        digest: hex(&outer.finalize()),
        entries: per_entry,
    }
}

/// The archive's own bytes, exactly as they sit on disk.
///
/// Not derived from entries: this is the file, including its compression,
/// timestamps and entry order. It answers "is this the same file I was sent",
/// which no content digest can, and it is the only one of the three that a
/// repack changes.
///
/// It carries no per-entry map, because there are no entries here -- only
/// bytes. An empty map would invite a caller to read "no differences" out of
/// "nothing was compared".
pub fn digest_archive_bytes(bytes: &[u8]) -> BundleDigest {
    let mut outer = Sha256::new();
    outer.update(Layer::Archive.schema().as_bytes());
    outer.update([0u8]);
    outer.update((bytes.len() as u64).to_le_bytes());
    outer.update(bytes);
    BundleDigest {
        schema: Layer::Archive.schema().to_string(),
        digest: hex(&outer.finalize()),
        entries: BTreeMap::new(),
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

    /// The collision that shipped (K-245, IC-712).
    ///
    /// Two bundles, identical component and manifest, different `source/`.
    /// The execution digest is equal -- correctly, they run the same -- and
    /// the project digest must not be, because a person reading the source
    /// would see a different program.
    #[test]
    fn different_source_is_a_different_project_but_the_same_execution() {
        let a = entries(&[
            ("manifest.toml", b"name = 'clock'"),
            ("code.wasm", b"\0asm-the-same-bytes"),
            ("source/src/lib.rs", b"fn main() { tick() }"),
        ]);
        let b = entries(&[
            ("manifest.toml", b"name = 'clock'"),
            ("code.wasm", b"\0asm-the-same-bytes"),
            ("source/src/lib.rs", b"fn main() { steal_everything() }"),
            ("source/extra.rs", b"// and a file that is not in a"),
        ]);

        assert_eq!(
            digest_layer(Layer::Execution, &a).digest,
            digest_layer(Layer::Execution, &b).digest,
            "the same component and manifest run the same, whatever source ships"
        );
        assert_ne!(
            digest_layer(Layer::Project, &a).digest,
            digest_layer(Layer::Project, &b).digest,
            "different source is a different project -- this is the exact case \
             that printed one value under the word Identity"
        );
    }

    /// The SDK is part of the project too: the same source against a
    /// different SDK is not the same thing to rebuild.
    #[test]
    fn a_different_sdk_is_a_different_project() {
        let base: &[(&str, &[u8])] = &[
            ("manifest.toml", b"m"),
            ("code.wasm", b"c"),
            ("source/src/lib.rs", b"s"),
        ];
        let mut with_old = entries(base);
        with_old.insert("sdk/krate.wit".into(), b"world v1".to_vec());
        let mut with_new = entries(base);
        with_new.insert("sdk/krate.wit".into(), b"world v2".to_vec());

        assert_eq!(
            digest_layer(Layer::Execution, &with_old).digest,
            digest_layer(Layer::Execution, &with_new).digest,
        );
        assert_ne!(
            digest_layer(Layer::Project, &with_old).digest,
            digest_layer(Layer::Project, &with_new).digest,
            "an app's SDK is what keeps it rebuildable; changing it changes the project"
        );
    }

    /// Two layers of the SAME bundle must never compare equal, even when they
    /// cover exactly the same entries.
    ///
    /// A bundle with no source is the case that would collide if the layers
    /// shared a schema tag -- and a caller comparing an execution digest to a
    /// project digest would then read "identical" as "same layer".
    #[test]
    fn layers_of_one_bundle_never_collide_even_with_nothing_to_distinguish_them() {
        let no_source = entries(&[("manifest.toml", b"m"), ("code.wasm", b"c")]);
        let execution = digest_layer(Layer::Execution, &no_source);
        let project = digest_layer(Layer::Project, &no_source);

        assert_eq!(
            execution.entries, project.entries,
            "with no source the two layers cover the same entries",
        );
        assert_ne!(
            execution.digest, project.digest,
            "yet the digests must differ: the schema tag is what stops one \
             layer's value being read as another's",
        );
        assert_ne!(execution.schema, project.schema);
    }

    /// The execution digest keeps the value it has always had.
    ///
    /// Digests are already recorded elsewhere. Changing what the historical
    /// schema name computes would silently invalidate them, so the refactor
    /// into layers must be a pure extension.
    #[test]
    fn the_execution_layer_is_byte_identical_to_the_original_digest() {
        let e = entries(&[
            ("manifest.toml", b"name = 'x'"),
            ("code.wasm", b"\0asm"),
            ("assets/logo.png", b"png"),
        ]);
        assert_eq!(digest_entries(&e), digest_layer(Layer::Execution, &e));
        assert_eq!(digest_layer(Layer::Execution, &e).schema, DIGEST_SCHEMA);
    }

    /// Repacking changes the file and nothing else.
    #[test]
    fn the_archive_digest_sees_bytes_the_content_digests_cannot() {
        let one = digest_archive_bytes(b"PK\x03\x04 ... one packing");
        let two = digest_archive_bytes(b"PK\x03\x04 ... another packing");
        assert_ne!(one.digest, two.digest, "different bytes, different archive");
        assert_eq!(
            digest_archive_bytes(b"PK\x03\x04 ... one packing").digest,
            one.digest,
            "and the same bytes always give the same answer",
        );
        assert!(
            one.entries.is_empty(),
            "an archive digest compares no entries, and must not imply it did",
        );
    }

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
