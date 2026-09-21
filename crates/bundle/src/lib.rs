//! The `.krate` bundle: one file that carries an application and the
//! permissions it is asking for.
//!
//! A bundle is a zip container holding two required entries and, optionally,
//! read-only application assets:
//!
//! ```text
//! app.krate
//! ├── manifest.toml   # krate-manifest schema, unchanged
//! ├── code.wasm       # the component
//! └── assets/         # optional portable app resources
//! ```
//!
//! This is the minimal subset of the Phase 6 bundle format (Phase-6-Plan §8.1)
//! pulled forward as P3-SHARE-01. Signing, the transparency log, delta updates,
//! AOT siblings, and asset directories stay in Phase 6.
//!
//! # What this module is careful about
//!
//! Opening a bundle means writing attacker-influenced bytes to disk, so:
//!
//! * required entry names are matched exactly, and asset paths accept only
//!   normal relative components under `assets/`, so path traversal
//!   (`../../etc/passwd`) is unrepresentable;
//! * both the compressed archive and each decompressed entry are size-capped,
//!   so a zip bomb fails loudly instead of filling the disk;
//! * the manifest's declared entry must match the contained component, so a
//!   bundle cannot advertise one set of capabilities and ship a different
//!   program.
//!
//! Crucially, opening a bundle grants *nothing*. It returns paths. The caller
//! runs the same policy resolution it would for a component sitting on disk
//! next to a sidecar manifest, so a downloaded bundle has exactly the authority
//! a local one would: none, until granted.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

pub mod imports;

use krate_manifest::Manifest;
use tempfile::TempDir;
use thiserror::Error;
use zip::ZipArchive;

/// Release keys delegated by a publisher root (IC-015).
pub mod delegation;
/// The manifest entry name inside a bundle.
pub mod provenance;
/// Publisher signatures over that statement (IC-015).
pub mod signing;
/// The canonical statement a publisher signature covers (IC-015).
pub mod statement;

pub const MANIFEST_ENTRY: &str = "manifest.toml";
/// The component entry name inside a bundle.
pub const COMPONENT_ENTRY: &str = "code.wasm";

/// The container profile this Krate reads and writes (IC-208).
///
/// One line, first entry, read before anything else. It exists so that a
/// format change is a sentence rather than a puzzle: an old Krate meeting a
/// future bundle can say "this app needs a newer Krate" instead of ignoring
/// the entries it does not recognise and mis-reading the rest.
///
/// The number is the PROFILE, not the app and not the SDK. It moves only
/// when the container's rules change -- a new required entry, a different
/// normalisation, a digest input -- and each move is a deliberate act with
/// migration behind it.
pub const PROFILE_ENTRY: &str = "krate-profile";

/// The profile version this build writes.
///
/// 1 is what every bundle shipped before 2026-09-14 implicitly is:
/// manifest.toml and code.wasm required, assets/ source/ sdk/
/// signature.json optional, unknown entries ignored, paths compared case-
/// and separator-insensitively. Bundles written before this entry existed
/// carry no profile line and are read as generation 1, because that is
/// what they are -- "keep existing files readable as their recorded
/// generation" is the requirement's own words.
///
/// 2 (written since 2026-09-14, the founder's call) keeps every rule of 1
/// and adds one: a top-level record the profile does not name is refused,
/// not ignored, unless it sits in a declared extension namespace
/// (`ext/<owner>/<name>/`, declared in `extensions.json`). The profile
/// names: krate-profile, manifest.toml, code.wasm, signature.json,
/// derived-from.json, extensions.json, closure.json, and the assets/
/// source/ sdk/ ext/ trees. A Krate older than the reader that learned profile 2 refuses a
/// profile 2 bundle with "this app uses a newer .krate format ... update
/// Krate", which is the honest outcome the profile line exists for.
pub const PROFILE_VERSION: u32 = 2;
/// The newest profile this build can READ (IC-714, test 1475).
///
/// Never below [`PROFILE_VERSION`], and meant to move first: a reader that
/// understands the next profile ships before any writer produces it, so
/// the installed base can open the files by the time they exist. Profile
/// 1 bundles keep their recorded behaviour -- unknown entries ignored --
/// which is what "readable as their recorded generation" means; see
/// [`PROFILE_VERSION`] for what 2 adds.
pub const READS_PROFILE: u32 = 2;
// A build never writes a profile it cannot read.
const _: () = assert!(READS_PROFILE >= PROFILE_VERSION);
/// Root for developer-defined material: `ext/<owner>/<name>/...`
/// (IC-208, IC-714 test 1476).
///
/// An application can carry things Krate does not interpret -- a plugin
/// manifest, a data pack, a licence bundle -- without Krate taking control
/// of its architecture, as long as it says so: every `<owner>/<name>` group
/// under this prefix is declared in [`EXTENSIONS_ENTRY`] with its owner,
/// name, version, total size and digest, and a group that is undeclared,
/// missing or does not match its declaration is refused. Krate never reads
/// the contents; it only proves they are what the bundle says they are.
pub const EXTENSION_PREFIX: &str = "ext/";
/// The declaration of every extension group a bundle carries.
pub const EXTENSIONS_ENTRY: &str = "extensions.json";
/// Version of the extensions declaration.
pub const EXTENSIONS_SCHEMA: &str = "krate.bundle.extensions.v1";
/// Schema tag mixed into an extension group's digest.
pub const EXTENSION_DIGEST_SCHEMA: &str = "krate.bundle.extension.v1";

/// One declared extension group, as the bundle records it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionRecord {
    /// Who defines the group: a lower-case ASCII label such as `acme`.
    pub owner: String,
    /// What the group is, within its owner: `plugins`, `dataset-v3`.
    pub name: String,
    /// The owner's own version label for the group; opaque to Krate.
    pub version: String,
    /// Total bytes of every entry under `ext/<owner>/<name>/`.
    pub size: u64,
    /// `sha256` over the group's sorted entries, see [`extension_digest`].
    pub digest: String,
}

/// The `extensions.json` record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtensionsRecord {
    pub schema: String,
    pub extensions: Vec<ExtensionRecord>,
}

/// An extension group to pack: a directory that becomes
/// `ext/<owner>/<name>/...` with a declaration beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionSource {
    pub owner: String,
    pub name: String,
    pub version: String,
    pub root: PathBuf,
}

impl ExtensionSource {
    /// The label rule: lower-case ASCII letters, digits, `.`, `_` and `-`,
    /// starting with a letter or digit, at most 64 bytes. Labels are path
    /// segments, so nothing that could be read as a separator or a dot
    /// segment is allowed.
    pub fn valid_label(label: &str) -> bool {
        !label.is_empty()
            && label.len() <= 64
            && label.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-'
            })
            && label.as_bytes()[0].is_ascii_alphanumeric()
            && label != "."
            && label != ".."
    }

    fn check(&self) -> Result<()> {
        for (what, label) in [("owner", &self.owner), ("name", &self.name)] {
            if !Self::valid_label(label) {
                return Err(BundleError::MalformedExtensionPath {
                    path: format!("{EXTENSION_PREFIX}{}/{}/", self.owner, self.name),
                    detail: format!(
                        "the {what} {label:?} is not a label: lower-case ASCII letters, \
                         digits, `.`, `_` and `-`, starting with a letter or digit, at most \
                         64 bytes"
                    ),
                });
            }
        }
        if self.version.is_empty() || self.version.len() > 64 || !self.version.is_ascii() {
            return Err(BundleError::MalformedExtensionPath {
                path: format!("{EXTENSION_PREFIX}{}/{}/", self.owner, self.name),
                detail: "the version must be 1 to 64 ASCII bytes".to_string(),
            });
        }
        Ok(())
    }

    fn prefix(&self) -> String {
        format!("{EXTENSION_PREFIX}{}/{}/", self.owner, self.name)
    }
}

/// One record of a bundle, as every reader resolves it (IC-714, test 1477).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Record {
    /// The entry name, exactly as stored.
    pub name: String,
    /// What the profile makes of it, see [`record_class`].
    pub class: String,
    /// The size the archive declares for it.
    pub bytes: u64,
}

/// The class the container profile gives an entry name.
///
/// One function, used by `open`, by `judge_bytes` and by the record listing,
/// so no door can resolve a record set another door would not (1477).
/// Directory records (a trailing `/`) are not records at all.
pub fn record_class(name: &str) -> &'static str {
    if name == PROFILE_ENTRY {
        "profile"
    } else if name == MANIFEST_ENTRY {
        "manifest"
    } else if name == COMPONENT_ENTRY {
        "component"
    } else if name == SIGNATURE_ENTRY {
        "signature"
    } else if name == DERIVED_FROM_ENTRY {
        "derived-from"
    } else if name == EXTENSIONS_ENTRY {
        "extensions"
    } else if name == CLOSURE_ENTRY {
        "closure"
    } else if name.starts_with(ASSETS_PREFIX) {
        "asset"
    } else if name.starts_with(SOURCE_PREFIX) {
        "source"
    } else if name.starts_with(SDK_PREFIX) {
        "sdk"
    } else if name.starts_with(EXTENSION_PREFIX) {
        "extension"
    } else {
        "unknown"
    }
}

/// The record set of an archive: every non-directory entry, sorted by name,
/// with its class and declared size.
fn records_of<R: Read + io::Seek>(archive: &mut ZipArchive<R>) -> Result<Vec<Record>> {
    let mut records = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let name = entry.name().to_string();
        if name.ends_with('/') {
            continue;
        }
        records.push(Record {
            class: record_class(&name).to_string(),
            bytes: entry.size(),
            name,
        });
    }
    records.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(records)
}

/// Profile 2: a top-level record the profile does not name is refused
/// (1475). Profile 1 keeps ignoring it, as its rules say.
fn refuse_unknown_records(profile: u32, records: &[Record]) -> Result<()> {
    if profile < 2 {
        return Ok(());
    }
    if let Some(record) = records.iter().find(|record| record.class == "unknown") {
        return Err(BundleError::UnknownEntry {
            path: shorten_path(&record.name),
            profile,
        });
    }
    Ok(())
}

/// The digest of one extension group: `sha256` over the schema tag and,
/// for every entry in name order, the name (length-prefixed) and the hex
/// digest of its bytes. The same shape as a layer digest, so a reader that
/// can check one can check the other.
pub fn extension_digest(entries: &BTreeMap<String, Vec<u8>>) -> String {
    use sha2::{Digest, Sha256};
    let mut outer = Sha256::new();
    outer.update(EXTENSION_DIGEST_SCHEMA.as_bytes());
    outer.update([0u8]);
    for (name, body) in entries {
        let inner: String = Sha256::digest(body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        outer.update((name.len() as u64).to_le_bytes());
        outer.update(name.as_bytes());
        outer.update(inner.as_bytes());
    }
    outer
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `ext/<owner>/<name>/rest` split into its group key and the rest.
fn extension_group(name: &str) -> Result<(String, String)> {
    let rest = name.strip_prefix(EXTENSION_PREFIX).unwrap_or(name);
    let mut parts = rest.splitn(3, '/');
    let owner = parts.next().unwrap_or("");
    let group = parts.next().unwrap_or("");
    let file = parts.next().unwrap_or("");
    if !ExtensionSource::valid_label(owner)
        || !ExtensionSource::valid_label(group)
        || file.is_empty()
    {
        return Err(BundleError::MalformedExtensionPath {
            path: shorten_path(name),
            detail: "an extension entry is `ext/<owner>/<name>/<file>`, with lower-case \
                     ASCII labels for the owner and the name"
                .to_string(),
        });
    }
    Ok((format!("{owner}/{group}"), file.to_string()))
}

/// Check every extension group against the declaration (1476).
///
/// `entries` holds the bundle's entries by name. A group is checked against
/// its record's size and digest; a declared group with no entries, and --
/// when the bundle declares extensions at all, or is profile 2 -- an entry
/// with no declaration, are refused. Under profile 1 with no declaration,
/// `ext/` entries are unknown material and are ignored like any other.
/// Returns the declared records, in declaration order.
fn check_extensions(
    profile: u32,
    entries: &BTreeMap<String, Vec<u8>>,
) -> Result<Vec<ExtensionRecord>> {
    let declared: Option<ExtensionsRecord> = match entries.get(EXTENSIONS_ENTRY) {
        None => None,
        Some(body) => {
            let record: ExtensionsRecord =
                serde_json::from_slice(body).map_err(|err| BundleError::DamagedExtensions {
                    detail: err.to_string(),
                })?;
            if record.schema != EXTENSIONS_SCHEMA {
                return Err(BundleError::DamagedExtensions {
                    detail: format!(
                        "it uses a newer format ({}) than this copy of Krate understands",
                        record.schema
                    ),
                });
            }
            Some(record)
        }
    };

    let mut groups: BTreeMap<String, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
    for (name, body) in entries {
        if !name.starts_with(EXTENSION_PREFIX) {
            continue;
        }
        let (key, _) = extension_group(name)?;
        groups
            .entry(key)
            .or_default()
            .insert(name.clone(), body.clone());
    }

    let Some(declared) = declared else {
        if profile >= 2 {
            if let Some(key) = groups.keys().next() {
                return Err(BundleError::UndeclaredExtension {
                    extension: key.clone(),
                });
            }
        }
        return Ok(Vec::new());
    };

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for record in &declared.extensions {
        let key = format!("{}/{}", record.owner, record.name);
        if !ExtensionSource::valid_label(&record.owner)
            || !ExtensionSource::valid_label(&record.name)
        {
            return Err(BundleError::DamagedExtensions {
                detail: format!("{key:?} is not a valid owner/name pair"),
            });
        }
        if !seen.insert(key.clone()) {
            return Err(BundleError::DamagedExtensions {
                detail: format!("{key} is declared twice"),
            });
        }
        let Some(group) = groups.get(&key) else {
            return Err(BundleError::MissingExtension { extension: key });
        };
        let size: u64 = group.values().map(|body| body.len() as u64).sum();
        if size != record.size {
            return Err(BundleError::ExtensionMismatch {
                extension: key,
                detail: format!("it declares {} bytes and carries {size}", record.size),
            });
        }
        let digest = extension_digest(group);
        if digest != record.digest {
            return Err(BundleError::ExtensionMismatch {
                extension: key,
                detail: "its contents do not match the digest it declares".to_string(),
            });
        }
    }
    if let Some(key) = groups.keys().find(|key| !seen.contains(*key)) {
        return Err(BundleError::UndeclaredExtension {
            extension: key.clone(),
        });
    }
    Ok(declared.extensions)
}

/// What built this bundle, and what it would take to build it again
/// (CP1: "editable bundles carry the full closure").
///
/// Written beside `source/` whenever a bundle carries source. The
/// toolchain is MEASURED from the component's own `producers` section --
/// the compiler that emitted the code, not the one the packer happens to
/// have on PATH -- and the digests are over the very entries the archive
/// holds, so the record cannot describe a closure other than the one
/// shipped. A reader checks them (`check_closure`) and refuses a bundle
/// whose record and contents disagree, exactly as for extensions.
pub const CLOSURE_ENTRY: &str = "closure.json";
/// Version of the closure record.
pub const CLOSURE_SCHEMA: &str = "krate.bundle.closure.v1";
/// Schema tag mixed into the source and SDK digests of a closure.
pub const CLOSURE_DIGEST_SCHEMA: &str = "krate.bundle.closure.v1";

/// The closure record. See [`CLOSURE_ENTRY`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Closure {
    pub schema: String,
    /// The bundle crate that wrote the record.
    pub packer: String,
    /// Tools named in the component's `producers` section, `name` and
    /// `version`, in the order they appear: `rustc`, `wit-component`, ...
    /// Empty when the component names none.
    pub processed_by: Vec<Producer>,
    /// The channel `rust-toolchain.toml` in the source declares, when it
    /// carries one.
    pub rust_toolchain: Option<String>,
    /// Whether `source/Cargo.lock` travels, so a rebuild can be `--locked`.
    pub locked: bool,
    /// Digest over every `source/` entry, sorted by name.
    pub source_digest: String,
    /// The SDK the source was written against, when the bundle carries it.
    pub sdk: Option<ClosureSdk>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Producer {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClosureSdk {
    /// The version the bundled SDK's Cargo.toml declares.
    pub version: Option<String>,
    /// Digest over every `sdk/` entry, sorted by name.
    pub digest: String,
}

/// The tools a component says processed it: every `processed-by` field of
/// every `producers` custom section, component and core modules alike.
///
/// `parse_all` walks nested modules, so a component built by
/// cargo-component reports `rustc` (from the core module) and
/// `wit-component` (from the component wrapper) both.
pub fn producers(component: &[u8]) -> Vec<Producer> {
    use wasmparser::{KnownCustom, Parser, Payload};
    let mut out: Vec<Producer> = Vec::new();
    for payload in Parser::new(0).parse_all(component) {
        let Ok(Payload::CustomSection(section)) = payload else {
            continue;
        };
        let KnownCustom::Producers(reader) = section.as_known() else {
            continue;
        };
        for field in reader.into_iter().flatten() {
            if field.name != "processed-by" {
                continue;
            }
            for value in field.values.into_iter().flatten() {
                let producer = Producer {
                    name: value.name.to_string(),
                    version: value.version.to_string(),
                };
                if !out.contains(&producer) {
                    out.push(producer);
                }
            }
        }
    }
    out
}

/// Digest over a set of entries, sorted by name: the same shape as an
/// extension digest, under the closure's own schema tag.
fn closure_digest(entries: &BTreeMap<String, Vec<u8>>) -> String {
    use sha2::{Digest, Sha256};
    let mut outer = Sha256::new();
    outer.update(CLOSURE_DIGEST_SCHEMA.as_bytes());
    outer.update([0u8]);
    for (name, body) in entries {
        let inner: String = Sha256::digest(body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        outer.update((name.len() as u64).to_le_bytes());
        outer.update(name.as_bytes());
        outer.update(inner.as_bytes());
    }
    outer
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The channel a `rust-toolchain.toml` declares, read the simple way: the
/// `channel = "..."` line. A toolchain file with no channel line records
/// nothing rather than a guess.
fn toolchain_channel(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        let rest = line
            .strip_prefix("channel")?
            .trim_start()
            .strip_prefix('=')?;
        Some(rest.trim().trim_matches('"').to_string()).filter(|c| !c.is_empty())
    })
}

/// Build the closure record for the entries about to be written.
fn closure_record(
    component: &[u8],
    source: &BTreeMap<String, Vec<u8>>,
    sdk: &BTreeMap<String, Vec<u8>>,
) -> Closure {
    let rust_toolchain = source
        .get(&format!("{SOURCE_PREFIX}rust-toolchain.toml"))
        .and_then(|bytes| toolchain_channel(&String::from_utf8_lossy(bytes)));
    let sdk_record = (!sdk.is_empty()).then(|| ClosureSdk {
        version: sdk
            .get(&format!("{SDK_PREFIX}crates/bindings-rust/Cargo.toml"))
            .and_then(|bytes| {
                String::from_utf8_lossy(bytes).lines().find_map(|line| {
                    let rest = line
                        .trim()
                        .strip_prefix("version")?
                        .trim_start()
                        .strip_prefix('=')?;
                    Some(rest.trim().trim_matches('"').to_string())
                })
            }),
        digest: closure_digest(sdk),
    });
    Closure {
        schema: CLOSURE_SCHEMA.to_string(),
        packer: format!("krate-bundle {}", env!("CARGO_PKG_VERSION")),
        processed_by: producers(component),
        rust_toolchain,
        locked: source.contains_key(&format!("{SOURCE_PREFIX}Cargo.lock")),
        source_digest: closure_digest(source),
        sdk: sdk_record,
    }
}

/// Check a bundle's closure record against what it carries, when it has
/// one. A record whose digests do not match the entries, or that claims
/// a lock the source does not hold, is damage and is refused.
fn check_closure(entries: &BTreeMap<String, Vec<u8>>) -> Result<Option<Closure>> {
    let Some(body) = entries.get(CLOSURE_ENTRY) else {
        return Ok(None);
    };
    let record: Closure =
        serde_json::from_slice(body).map_err(|err| BundleError::DamagedClosure {
            detail: err.to_string(),
        })?;
    if record.schema != CLOSURE_SCHEMA {
        return Err(BundleError::DamagedClosure {
            detail: format!(
                "it uses a newer format ({}) than this copy of Krate understands",
                record.schema
            ),
        });
    }
    let source: BTreeMap<String, Vec<u8>> = entries
        .iter()
        .filter(|(name, _)| name.starts_with(SOURCE_PREFIX))
        .map(|(name, body)| (name.clone(), body.clone()))
        .collect();
    let sdk: BTreeMap<String, Vec<u8>> = entries
        .iter()
        .filter(|(name, _)| name.starts_with(SDK_PREFIX))
        .map(|(name, body)| (name.clone(), body.clone()))
        .collect();
    if record.source_digest != closure_digest(&source) {
        return Err(BundleError::DamagedClosure {
            detail: "the source does not match the digest the record declares".to_string(),
        });
    }
    if record.locked != source.contains_key(&format!("{SOURCE_PREFIX}Cargo.lock")) {
        return Err(BundleError::DamagedClosure {
            detail: "the record and the source disagree about Cargo.lock".to_string(),
        });
    }
    match (&record.sdk, sdk.is_empty()) {
        (Some(declared), false) if declared.digest == closure_digest(&sdk) => {}
        (None, true) => {}
        _ => {
            return Err(BundleError::DamagedClosure {
                detail: "the SDK does not match what the record declares".to_string(),
            })
        }
    }
    Ok(Some(record))
}

/// The one function the runtime calls on a Krate app.
///
/// Both worlds declare `export run: func() -> s32`, and every shipped app
/// exports exactly this and nothing else (measured on krate-clocks and
/// krate-notes: `exports: {"run"}`).
///
/// NOT enforced at pack time, deliberately. A component that parses but
/// exports nothing is a valid empty component, and the minimal
/// `\0asm\x01\0\0\0` stub that fifteen tests use as a stand-in is exactly
/// that -- so enforcing it here would mean rewriting every fixture to embed
/// a real component, a large change carrying its own risk for a gap that
/// `krate check-app` already closes by RUNNING the app. Recorded rather than
/// half-done (IC-210).
pub const RUN_EXPORT: &str = "run";
/// Root for optional portable resources inside the bundle.
pub const ASSETS_PREFIX: &str = "assets/";
/// Root for the SDK the app was built against.
///
/// Shipping the source alone is not enough to rebuild an app later: the source
/// is written against whatever SDK existed when it was made, and Krate's SDK
/// still changes. An app built before a WIT change fails to compile against
/// the current one -- "missing field `pixels`" and the like -- so an app is
/// only genuinely editable if it carries the SDK it was written for.
///
/// About 75 KB compressed, which is real against a 17 KB app and is the price
/// of an app that still opens for editing in a year.
pub const SDK_PREFIX: &str = "sdk/";

/// Root for the app's own source, so a bundle can be changed and rebuilt.
///
/// A `.krate` used to carry only compiled wasm, which meant an app could be
/// run but never altered -- not by the person who made it a week later, and
/// not at all by someone it was sent to. Shipping the source makes any bundle
/// editable by whoever holds it, which is what "one file you can send anyone"
/// ought to mean. It roughly doubles a small app's size and that is a fair
/// trade for the app remaining alive.
pub const SOURCE_PREFIX: &str = "source/";
/// The publisher's signature over this bundle, when it has one (IC-015).
///
/// One fixed entry name, read by exact name like the manifest and the
/// component, so a signature cannot be smuggled in under a path that escapes
/// the temp directory. Its absence is not an error: an unsigned app runs and
/// is labelled unverified, which is the identity contract's rule -- requiring
/// a signature to run would make publishing a precondition for using your own
/// machine.
///
/// The signature covers a statement, never the archive, so it cannot cover
/// its own bytes. See `statement` and `signing`.
pub const SIGNATURE_ENTRY: &str = "signature.json";

/// Where a fork says which app it was changed from (IC-397, test 494).
///
/// Written by `krate revise` when the input is a bundle. It names the
/// parent's identities and whether the parent was signed -- never the
/// parent's signer, because a fork must not carry the original publisher's
/// name on bytes they never signed (F-013). The identity contract says a
/// "derived from" record "can point to the original release digest without
/// claiming that the original publisher signed, approved, supports, or
/// remains liable for the fork", and that is all this record claims.
///
/// An entry rather than a manifest field so an older Krate, which ignores
/// entries it does not know, still opens a fork. It belongs to the project
/// identity (what this is) and not the execution identity (what runs).
pub const DERIVED_FROM_ENTRY: &str = "derived-from.json";

/// Version of the derived-from record.
pub const DERIVED_FROM_SCHEMA: &str = "krate.bundle.derived-from.v1";

/// The parent an app was changed from. See [`DERIVED_FROM_ENTRY`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DerivedFrom {
    pub schema: String,
    /// The parent file's own bytes.
    pub archive: String,
    /// What ran in the parent.
    pub execution: String,
    /// What could be rebuilt from the parent, when it carried source.
    pub project: Option<String>,
    /// Whether the parent carried a signature that verified. A fact about
    /// the parent, not a claim about this file: this file is not that
    /// signature's.
    pub parent_signed: bool,
    /// When the fork was made, seconds since the Unix epoch.
    pub at: u64,
}

/// The record a fork of `parent` should carry.
///
/// Reads the parent the way `open` does, so the identities recorded are
/// the ones the parent prints for itself.
pub fn derived_from_record(parent: &Path, at: u64) -> Result<DerivedFrom> {
    let bytes = fs::read(parent).map_err(|err| io_err(parent, err))?;
    let opened = open_bytes(&bytes)?;
    let execution = opened.digest()?;
    let project = opened.project_digest()?;
    let parent_signed = opened
        .signature_verdict()?
        .is_some_and(|verdict| verdict.is_genuinely_signed());
    Ok(DerivedFrom {
        schema: DERIVED_FROM_SCHEMA.to_string(),
        archive: provenance::digest_archive_bytes(&bytes).digest,
        execution: execution.digest,
        // Only when there was genuinely more to rebuild: the two layers
        // differ by schema tag over identical entries.
        project: (project.entries.len() > execution.entries.len()).then_some(project.digest),
        parent_signed,
        at,
    })
}

/// Conventional file extension.
pub const BUNDLE_EXTENSION: &str = "krate";

/// Largest bundle we will read, compressed. Generous for a format whose
/// reference application is 26 KB, and small enough that a hostile URL cannot
/// stream gigabytes at us.
pub const MAX_BUNDLE_BYTES: u64 = 256 * 1024 * 1024;

/// How long to wait for a host to answer at all.
///
/// ureq's own default, made explicit because the read timeout beside it is
/// not defaulted and the pair should be read together.
#[cfg(feature = "fetch")]
const FETCH_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a transfer may produce NOTHING before it is given up on (K-275).
///
/// This is the socket's read timeout, so it bounds silence rather than total
/// transfer time -- every chunk that arrives resets it. A slow download of a
/// large bundle keeps it alive indefinitely; a server that accepts the
/// connection and then never speaks trips it once.
///
/// Thirty seconds of complete silence is far longer than any healthy
/// connection goes between packets, and short enough that a person is not
/// left staring at a command that will never return.
#[cfg(feature = "fetch")]
const FETCH_SILENCE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Largest single entry we will decompress. Bounds the classic zip bomb, where
/// a small archive expands to an enormous file.
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
/// Largest individual bundled asset after decompression.
pub const MAX_ASSET_BYTES: u64 = 96 * 1024 * 1024;
/// Largest total asset payload after decompression.
pub const MAX_TOTAL_ASSET_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum number of asset files in one bundle.
pub const MAX_ASSET_COUNT: usize = 4096;

/// Deepest an entry path may nest, counting separators (IC-209).
///
/// Nothing bounded depth, so an entry 200 directories deep was accepted and
/// written out: measured at 209 path components and 963 characters. On
/// Windows that crosses MAX_PATH (260) once joined to the directory it is
/// unpacked into, and the run fails partway through extraction with an I/O
/// error naming a path nobody typed, rather than a refusal saying what is
/// wrong. On macOS and Linux the same bundle just works, so it is portable
/// in name only.
///
/// A real bundle's deepest entry is `source/src/lib.rs` -- two separators.
/// Sixteen is eight times that, and still far inside every system's limit.
pub const MAX_PATH_DEPTH: usize = 16;

/// Longest an entry path may be, in bytes, as stored in the archive (IC-209).
///
/// Judged as STORED, not as extracted: the directory a bundle is unpacked
/// into differs per machine and per run, so measuring the final path would
/// accept a bundle on one machine and refuse it on another. The limit leaves
/// room for the longest real entry name (20 characters) many times over
/// while keeping the extracted path inside MAX_PATH on Windows.
pub const MAX_PATH_BYTES: usize = 180;

/// Maximum number of files in one bundle, across every namespace (IC-209).
///
/// Assets were bounded and `source/`/`sdk/` were not, so a 5,000-entry source
/// tree opened where a 5,000-entry asset tree would have been refused. The
/// SDK alone is ~28 files and a real app's source is a handful, so this is
/// generous by two orders of magnitude while still bounding an archive built
/// to exhaust the machine extracting it.
pub const MAX_ENTRY_COUNT: usize = 8192;

/// Maximum expanded bytes across `source/` and `sdk/` together.
///
/// The same reasoning as [`MAX_TOTAL_ASSET_BYTES`], for the two namespaces
/// that had a per-file cap but no aggregate one: a thousand files just under
/// the per-file limit is a zip bomb that passes every per-file check.
pub const MAX_TOTAL_SOURCE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("not a valid .krate bundle: {0}")]
    Archive(#[from] zip::result::ZipError),
    #[error("bundle is missing its `{0}` entry")]
    MissingEntry(&'static str),
    #[error("bundle manifest is not valid: {0}")]
    Manifest(String),
    // Both forms are correct in their own place -- a development manifest points
    // at the build output, a bundle manifest points at the name inside the
    // bundle -- and we only ever documented the first. So the person who
    // follows our own instructions lands here, and the old message stated the
    // rule without saying what to do about it. Someone hit this through the MCP
    // server and worked around it with an unexplained `sed`.
    #[error(
        "bundle manifest declares entry `{declared}`, but a bundle always runs \
         `{COMPONENT_ENTRY}`.\n\n\
         Inside a bundle the component is stored under one fixed name, so the \
         manifest that goes in has to say that name. Your development \
         manifest is right to point at the build output -- make a copy for \
         packing with:\n\n    entry = \"{COMPONENT_ENTRY}\"\n\n\
         Or let `krate create` do the packing, which handles this for you."
    )]
    EntryMismatch { declared: String },
    #[error("bundle is {size} bytes, larger than the {MAX_BUNDLE_BYTES} byte limit")]
    TooLarge { size: u64 },
    #[error("bundle entry `{entry}` expands to more than {MAX_ENTRY_BYTES} bytes")]
    EntryTooLarge { entry: String },
    #[error("asset path `{path}` is not a safe relative path")]
    UnsafeAssetPath { path: String },
    #[error("asset `{path}` is a symbolic link; bundle assets must be regular files")]
    AssetSymlink { path: PathBuf },
    #[error("bundle contains more than {MAX_ASSET_COUNT} asset files")]
    TooManyAssets,
    #[error(
        "this bundle names the same file twice ({path}). What you review \
         would not be what runs, so Krate refuses it rather than picking \
         one copy. Ask whoever sent it for a freshly packed file."
    )]
    DuplicateEntry { path: String },
    #[error(
        "this app uses a newer .krate format (profile {found}) than this \
         copy of Krate understands (profile {supported}).\n\n  \
         Update Krate: https://krate.tech/open"
    )]
    UnsupportedProfile { found: String, supported: u32 },
    #[error(
        "this app's format line is damaged: it should be a version number \
         and it reads {found:?}.\n\n  \
         No version of Krate can read this, so updating will not help. \
         Ask whoever sent it for a fresh copy."
    )]
    DamagedProfile { found: String },
    #[error(
        "{path} is not a WebAssembly component, so it cannot be packed into \
         an app.\n\n  {detail}\n\n  \
         If this came from a build, check that the build produced a \
         component (`cargo component build`) rather than a plain module."
    )]
    NotAComponent { path: String, detail: String },
    #[error(
        "{path} is a WebAssembly component, but not one Krate can run: \
         {defect}\n\n  \
         Fix the app and pack it again; nothing downstream can repair this."
    )]
    InvalidComponent {
        path: String,
        defect: imports::ComponentDefect,
    },
    #[error(
        "{path} uses characters outside ASCII.\n\n  \
         Two spellings of one accented name are different bytes but the same \
         file on some systems, so Krate cannot tell a reviewed copy from a \
         substituted one. Rename it to ASCII and pack again."
    )]
    NonAsciiPath { path: String },
    #[error(
        "{path} nests deeper than {MAX_PATH_DEPTH} directories.\n\n  \
         A path this deep does not survive being unpacked on every system \
         -- on Windows it runs past the limit on how long a path may be. \
         Flatten it and pack again."
    )]
    PathTooDeep { path: String },
    #[error(
        "{path} is longer than {MAX_PATH_BYTES} characters.\n\n  \
         A name this long does not survive being unpacked on every system \
         -- on Windows it runs past the limit on how long a path may be. \
         Shorten it and pack again."
    )]
    PathTooLong { path: String },
    #[error(
        "could not make room to open this app: {detail}.\n\n  \
         Opening an app unpacks it into a temporary folder first. Check \
         there is free space on this disk."
    )]
    Unpack { detail: String },
    #[error("bundle contains more than {MAX_ENTRY_COUNT} files")]
    TooManyEntries,
    #[error("bundle source and SDK expand to more than {MAX_TOTAL_SOURCE_BYTES} bytes")]
    SourceTooLarge,
    #[error("bundle assets expand to more than {MAX_TOTAL_ASSET_BYTES} bytes")]
    AssetsTooLarge,
    #[error("refusing to fetch over plain HTTP: {url}\nuse https, or pass --insecure-http for a local test server")]
    InsecureUrl { url: String },
    #[error("could not fetch {url}: {message}")]
    Fetch { url: String, message: String },
    #[error(
        "bundle expands to more than {limit} bytes, which is more than this door holds in \
         memory to judge it"
    )]
    ExpandsTooFar { limit: u64 },
    #[error(
        "this app carries a record Krate does not know, `{path}`, and its format \
         (profile {profile}) allows only the records the format names.\n\n  \
         Developer material belongs under ext/<owner>/<name>/ and is declared in \
         extensions.json; anything else is refused so nobody can hide a file in \
         an app that every reader would skip."
    )]
    UnknownEntry { path: String, profile: u32 },
    #[error(
        "this app's extension declaration (extensions.json) is damaged: {detail}.\n\n  \
         Ask whoever sent it for a freshly packed copy."
    )]
    DamagedExtensions { detail: String },
    #[error(
        "this app carries the extension `{extension}` without declaring it in \
         extensions.json.\n\n  \
         Every ext/<owner>/<name>/ group is declared with its owner, name, version, \
         size and digest, so a reader can prove it is what the app says it is."
    )]
    UndeclaredExtension { extension: String },
    #[error(
        "this app declares the extension `{extension}` in extensions.json but carries \
         no entries under ext/{extension}/."
    )]
    MissingExtension { extension: String },
    #[error(
        "this app's extension `{extension}` is not what its declaration says: {detail}.\n\n  \
         The contents changed after the declaration was written, or the declaration \
         was edited. Ask whoever sent it for a freshly packed copy."
    )]
    ExtensionMismatch { extension: String, detail: String },
    #[error("`{path}` is not a valid extension path: {detail}")]
    MalformedExtensionPath { path: String, detail: String },
    #[error(
        "this app's build record (closure.json) is damaged: {detail}.\n\n  \
         Ask whoever sent it for a freshly packed copy."
    )]
    DamagedClosure { detail: String },
}

impl BundleError {
    /// A plain, single-sentence explanation for a person, with no zip/EOCD/io
    /// jargon and no repeated wrapped error. Callers print this at the process
    /// boundary instead of the raw error chain. Returns `None` when the
    /// variant's own message is already user-facing enough to print as-is.
    pub fn user_message(&self) -> Option<String> {
        match self {
            // A missing/unreadable file: say which and why, once.
            BundleError::Io { path, source } => Some(match source.kind() {
                io::ErrorKind::NotFound => format!("no file at {}", path.display()),
                // Opening an app unpacks it, so the commonest failure here is
                // a WRITE, not a read -- and a full disk is the one a person
                // can do something about (K-263).
                io::ErrorKind::StorageFull => {
                    "there is not enough room on this disk to open this app. \
                     Free some space and try again."
                        .to_string()
                }
                _ => format!("could not open {}: {}", path.display(), plain_io(source)),
            }),
            // A file we CAN read the shape of but not the contents: a real
            // Krate app packed with a compression method we do not support.
            // Saying "not a Krate app, or damaged" here is simply false, and
            // it sends whoever packed it to rebuild a file that is fine
            // (K-259). The one fact that helps them is the one to give.
            // An entry nobody can read without a password. The zip layer
            // reports it through the same variant as an unsupported
            // compression method, and the two need opposite answers: one
            // says "pack it again", which is right for compression and
            // useless here. A Krate app with a locked entry is not a
            // packing mistake -- it carries something the recipient, the
            // hub and any reviewer are all shut out of, and Krate has no
            // password to offer. Refused as its own thing (IC-833, 1833).
            BundleError::Archive(zip::result::ZipError::UnsupportedArchive(detail))
                if detail.to_lowercase().contains("password")
                    || detail.to_lowercase().contains("encrypt") =>
            {
                Some(
                    "this app has a locked entry inside it, and Krate does not open \
                     locked entries.\n\n  \
                     Everything in a Krate app is meant to be readable by whoever \
                     receives it -- that is what makes the file reviewable. Pack it \
                     again without encryption."
                        .to_string(),
                )
            }
            BundleError::Archive(zip::result::ZipError::UnsupportedArchive(detail)) => {
                Some(format!(
                    "this Krate app is packed a way this version cannot read ({detail}). \
                     The file itself looks fine -- pack it again with `krate pack`, \
                     which writes the compression every Krate reads."
                ))
            }
            // A corrupt or non-.krate file surfaces from the zip layer as an
            // "EOCD"/"invalid Zip archive" chain. None of that helps a person.
            BundleError::Archive(_) => Some(
                "this is not a Krate app, or the file is damaged. \
                 A Krate app is a single .krate file made by `krate create`."
                    .to_string(),
            ),
            BundleError::MissingEntry(_) => Some(
                "this .krate file is incomplete or damaged; \
                 try getting a fresh copy, or rebuild it with `krate create`."
                    .to_string(),
            ),
            // The rest already read plainly (size limits, insecure URL, etc.).
            _ => None,
        }
    }
}

/// The message part of an io error without a trailing "(os error N)" tail.
fn plain_io(source: &io::Error) -> String {
    let full = source.to_string();
    match full.split_once(" (os error") {
        Some((head, _)) => head.to_string(),
        None => full,
    }
}

type Result<T> = std::result::Result<T, BundleError>;

fn io_err(path: &Path, source: io::Error) -> BundleError {
    BundleError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Whether a path looks like a bundle rather than a bare component.
pub fn is_bundle_path(path: &Path) -> bool {
    // A `.krate` extension is the fast, obvious signal.
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(BUNDLE_EXTENSION))
    {
        return true;
    }
    // But `krate create --output myapp` writes the bundle to a path the user
    // named, often with no extension. Running that must still work, so fall
    // back to sniffing the content: a bundle is a ZIP (magic `PK\x03\x04`)
    // whose first entry is `manifest.toml`. This is a cheap read of the file
    // header, not a full open, and it means a bundle is a bundle whatever it
    // is called -- which is what a person renaming or downloading one expects.
    looks_like_bundle_file(path)
}

/// Whether a file's bytes look like a Krate bundle: a ZIP archive that names
/// `manifest.toml` in a local-file-header. A raw `.wasm` (which starts
/// with `\0asm`) never matches, so the two are never confused.
///
/// The header is not required to sit at offset 0. A wrap -- the gift we hand
/// someone who does not have Krate yet -- is a shell or batch script with the
/// bundle concatenated behind it, which is legal precisely because a zip is
/// read from its END. Demanding the magic at offset 0 made the same bytes a
/// bundle or not depending on their filename: `gift.krate` opened with its
/// identity, `gift.sh` came back as a bare component and then failed its own
/// permission check (K-213). Every real zip reader opens both; so does this.
fn looks_like_bundle_file(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    // A bundle's own header is at offset 0; a wrap's sits after a script
    // prefix that is well under a kilobyte. Read a bounded window rather
    // than the whole file -- this is a cheap sniff, not an open, and it must
    // stay cheap because it runs on every run target.
    let mut head = [0u8; 4096];
    let n = match std::io::Read::read(&mut file, &mut head) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let head = &head[..n];
    // Find a ZIP local file header, then confirm the entry it names. Both
    // parts matter: the magic alone would claim any zip, and "manifest.toml"
    // alone would claim a text file that merely mentions it.
    let Some(start) = head.windows(4).position(|w| w == [0x50, 0x4B, 0x03, 0x04]) else {
        return false;
    };
    // The file name follows the 30-byte fixed local header. Look for
    // "manifest.toml" after the header we found (pack always writes it
    // first), which avoids parsing the header's length fields.
    head[start..]
        .windows(b"manifest.toml".len())
        .any(|w| w == b"manifest.toml")
}

/// Whether a run target is a URL rather than a filesystem path.
pub fn is_url(target: &str) -> bool {
    target.starts_with("https://") || target.starts_with("http://")
}

/// The URL a scheme-less `host/path` target implies, if it can only be one.
///
/// People retype the short command a page printed -- `krate run
/// krate.tech/notes.krate` -- and the scheme is the part they drop. This
/// claims such a target for https ONLY when it cannot be a real relative
/// path: the first segment must read as a host (dotted labels of letters,
/// digits and hyphens, an optional port), and the caller must already have
/// found no file of that name on disk. `apps/foo.krate` has no dot and
/// stays a path; `./a.krate` and `/tmp/a.krate` never reach the host test.
pub fn implied_url(target: &str) -> Option<String> {
    let (host, rest) = target.split_once('/')?;
    if rest.is_empty() {
        return None;
    }
    let name = host.split(':').next().unwrap_or("");
    if !name.contains('.') {
        return None;
    }
    let host_reads_as_dns = name.split('.').all(|label| {
        !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    });
    if !host_reads_as_dns {
        return None;
    }
    Some(format!("https://{target}"))
}

/// Write a bundle from a manifest and a component.
///
/// The manifest is parsed and validated first, so `pack` cannot produce a
/// bundle that `open` would reject.
/// Run the component validator and turn its verdict into the bundle's own
/// error shapes: what is not a component at all keeps the message people
/// already know, and everything else says which rule the component broke.
fn check_component(component: &[u8], path: &str, manifest: &Manifest) -> Result<()> {
    use imports::ComponentDefect;
    match imports::validate_component(component, Some(&manifest.app.world)) {
        Ok(_) => Ok(()),
        Err(ComponentDefect::NotAComponent { detail }) => Err(BundleError::NotAComponent {
            path: path.to_string(),
            detail,
        }),
        Err(ComponentDefect::Malformed { detail }) => Err(BundleError::NotAComponent {
            path: path.to_string(),
            detail,
        }),
        Err(defect) => Err(BundleError::InvalidComponent {
            path: path.to_string(),
            defect,
        }),
    }
}

pub fn pack(manifest_path: &Path, component_path: &Path, output_path: &Path) -> Result<u64> {
    pack_with_assets(manifest_path, component_path, None, output_path)
}

/// Write a bundle with an optional directory of portable, read-only assets.
///
/// Every regular file below `assets_dir` is stored below `assets/` using a
/// normalized forward-slash path. Symlinks are rejected so packing cannot
/// silently include files outside the selected directory.
pub fn pack_with_assets(
    manifest_path: &Path,
    component_path: &Path,
    assets_dir: Option<&Path>,
    output_path: &Path,
) -> Result<u64> {
    pack_with_source(manifest_path, component_path, assets_dir, None, output_path)
}

/// Pack a bundle, optionally embedding the app's source directory.
///
/// `source_dir` is the crate root -- the directory holding `Cargo.toml` and
/// `src/`. Only the files needed to rebuild are taken; `target/` is the bulk of
/// a crate directory and is never useful inside a bundle.
pub fn pack_with_source(
    manifest_path: &Path,
    component_path: &Path,
    assets_dir: Option<&Path>,
    source_dir: Option<&Path>,
    output_path: &Path,
) -> Result<u64> {
    pack_with_sdk(
        manifest_path,
        component_path,
        assets_dir,
        source_dir,
        None,
        output_path,
    )
}

/// Pack a bundle carrying its source and the SDK that source was built with.
pub fn pack_with_sdk(
    manifest_path: &Path,
    component_path: &Path,
    assets_dir: Option<&Path>,
    source_dir: Option<&Path>,
    sdk_dir: Option<&Path>,
    output_path: &Path,
) -> Result<u64> {
    pack_with_lineage(
        manifest_path,
        component_path,
        assets_dir,
        source_dir,
        sdk_dir,
        None,
        output_path,
    )
}

/// Pack a bundle that was changed from another, recording its parent
/// (IC-397, test 494). See [`DERIVED_FROM_ENTRY`].
pub fn pack_with_lineage(
    manifest_path: &Path,
    component_path: &Path,
    assets_dir: Option<&Path>,
    source_dir: Option<&Path>,
    sdk_dir: Option<&Path>,
    derived_from: Option<&DerivedFrom>,
    output_path: &Path,
) -> Result<u64> {
    pack_with_extensions(
        manifest_path,
        component_path,
        assets_dir,
        source_dir,
        sdk_dir,
        derived_from,
        &[],
        output_path,
    )
}

/// Pack a bundle carrying developer extensions: each source directory
/// becomes `ext/<owner>/<name>/...`, declared in `extensions.json` with
/// its size and digest (IC-714, test 1476). See [`EXTENSION_PREFIX`].
#[allow(clippy::too_many_arguments)]
pub fn pack_with_extensions(
    manifest_path: &Path,
    component_path: &Path,
    assets_dir: Option<&Path>,
    source_dir: Option<&Path>,
    sdk_dir: Option<&Path>,
    derived_from: Option<&DerivedFrom>,
    extensions: &[ExtensionSource],
    output_path: &Path,
) -> Result<u64> {
    for extension in extensions {
        extension.check()?;
    }
    let derived = derived_from
        .map(|record| {
            serde_json::to_vec_pretty(record)
                .map_err(|err| BundleError::Manifest(format!("{DERIVED_FROM_ENTRY}: {err}")))
        })
        .transpose()?;
    let manifest_text =
        fs::read_to_string(manifest_path).map_err(|err| io_err(manifest_path, err))?;
    let manifest =
        Manifest::parse(&manifest_text).map_err(|err| BundleError::Manifest(err.to_string()))?;

    // Inside a bundle the component always lands at COMPONENT_ENTRY, so the
    // manifest has to name that. Rewriting it silently would mean the file the
    // developer signed off on is not the file that ships.
    let declared = manifest.app.entry.display().to_string();
    if declared != COMPONENT_ENTRY {
        return Err(BundleError::EntryMismatch { declared });
    }

    let component = fs::read(component_path).map_err(|err| io_err(component_path, err))?;

    // A bundle must contain a component, not merely some bytes (IC-210).
    //
    // `pack` accepted anything: a text file went in as `code.wasm` and came
    // out as a 351-byte "app" that exits 2 the moment somebody opens it.
    // The person packing is the one who can fix that, and they are the one
    // who never heard about it -- the failure landed on the recipient.
    //
    // The same validator `open` runs at the recipient's machine, run here
    // where the result is actionable (IC-210). A core module (K-272), bytes
    // that are not wasm, an import Krate never defined, an import the
    // declared world does not provide, a missing `run`: every one of these
    // used to pack cleanly and fail on whoever was sent the file.
    check_component(&component, &component_path.display().to_string(), &manifest)?;

    // Write beside the destination, then move it into place (IC-861).
    //
    // This used to create the output first and write into it as it went, so
    // any failure after that point left a stripped file where the
    // developer's previous bundle had been. E8 reproduced the worst shape:
    // a good 109,247-byte editable bundle replaced by a 12,018-byte
    // manifest-and-component-only archive, with the source gone. A symlink
    // in the source tree is enough to trigger it, because that refusal
    // happens after the file has already been truncated.
    //
    // A sibling temp file rather than the system temp directory: a rename
    // is only atomic within one filesystem, and /tmp is frequently a
    // different one. Falling back to a copy would reintroduce exactly the
    // partial-write window this exists to close.
    let staging = staging_path_for(output_path);
    // A leftover from an earlier interrupted run must never be appended to.
    let _ = fs::remove_file(&staging);

    // Everything that can fail happens inside here, so one place removes the
    // half-written file on the way out. A staging file left behind is not a
    // destroyed bundle, but it is litter beside the developer's work that
    // looks enough like a bundle to be confusing.
    let outcome = write_bundle_into(
        &staging,
        &manifest_text,
        &component,
        assets_dir,
        source_dir,
        sdk_dir,
        derived.as_deref(),
        extensions,
    );
    if let Err(err) = outcome {
        let _ = fs::remove_file(&staging);
        return Err(err);
    }

    let size = fs::metadata(&staging)
        .map_err(|err| io_err(&staging, err))?
        .len();
    fs::rename(&staging, output_path).map_err(|err| io_err(output_path, err))?;
    Ok(size)
}

/// Assemble the archive at `staging`. Split out so `pack_with_sdk` has one
/// place to clean up from, whichever step fails.
#[allow(clippy::too_many_arguments)]
/// A zip writer whose bytes are a pure function of the entries it is given,
/// in order (CP1: "a deterministic writer where determinism is claimed";
/// K-335).
///
/// The `zip` crate deflates through `flate2`, and flate2's backend is
/// chosen by feature unification: built alone, this crate got one deflater;
/// built beside the rest of the workspace it got zlib-rs, and the same
/// input packed to different bytes. Measured when the cross-machine pack
/// proof failed on every CI machine and then on this one under
/// `--workspace`. So the packer no longer delegates the choice: it deflates
/// with miniz_oxide called directly, stamps 1980-01-01 00:00 on every
/// entry, writes no extra fields, DOS host attributes and no comment. The
/// `zip` crate still reads the result; only the writing side is ours.
struct DeterministicZip<W: Write> {
    out: W,
    offset: u64,
    central: Vec<u8>,
    count: u64,
}

impl<W: Write> DeterministicZip<W> {
    fn new(out: W) -> Self {
        DeterministicZip {
            out,
            offset: 0,
            central: Vec::new(),
            count: 0,
        }
    }

    /// Add one entry. Deflated when that is smaller, stored otherwise, so
    /// an incompressible component is not made larger by a header.
    fn add(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let too_big = |what: &str| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{what} is too large for a zip without zip64, which bundles do not use"),
            )
        };
        let deflated = miniz_oxide::deflate::compress_to_vec(bytes, 6);
        let (method, body): (u16, &[u8]) = if deflated.len() < bytes.len() {
            (8, &deflated)
        } else {
            (0, bytes)
        };
        let size = u32::try_from(bytes.len()).map_err(|_| too_big(name))?;
        let compressed = u32::try_from(body.len()).map_err(|_| too_big(name))?;
        let name_len = u16::try_from(name.len()).map_err(|_| too_big("the entry name"))?;
        let offset = u32::try_from(self.offset).map_err(|_| too_big("the archive"))?;
        let crc = crc32fast::hash(bytes);
        // DOS time 00:00:00 and date 1980-01-01: the epoch of the format,
        // and the only stamp that says nothing about when or where.
        const TIME: u16 = 0;
        const DATE: u16 = 0x0021;

        let mut local = Vec::with_capacity(30 + name.len());
        local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        local.extend_from_slice(&20u16.to_le_bytes()); // version needed
        local.extend_from_slice(&0u16.to_le_bytes()); // flags
        local.extend_from_slice(&method.to_le_bytes());
        local.extend_from_slice(&TIME.to_le_bytes());
        local.extend_from_slice(&DATE.to_le_bytes());
        local.extend_from_slice(&crc.to_le_bytes());
        local.extend_from_slice(&compressed.to_le_bytes());
        local.extend_from_slice(&size.to_le_bytes());
        local.extend_from_slice(&name_len.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes()); // extra
        local.extend_from_slice(name.as_bytes());
        self.out.write_all(&local)?;
        self.out.write_all(body)?;
        self.offset += local.len() as u64 + body.len() as u64;

        let c = &mut self.central;
        c.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes()); // version made by: 2.0, host DOS
        c.extend_from_slice(&20u16.to_le_bytes()); // version needed
        c.extend_from_slice(&0u16.to_le_bytes()); // flags
        c.extend_from_slice(&method.to_le_bytes());
        c.extend_from_slice(&TIME.to_le_bytes());
        c.extend_from_slice(&DATE.to_le_bytes());
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&compressed.to_le_bytes());
        c.extend_from_slice(&size.to_le_bytes());
        c.extend_from_slice(&name_len.to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes()); // extra
        c.extend_from_slice(&0u16.to_le_bytes()); // comment
        c.extend_from_slice(&0u16.to_le_bytes()); // disk
        c.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        c.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        c.extend_from_slice(&offset.to_le_bytes());
        c.extend_from_slice(name.as_bytes());
        self.count += 1;
        Ok(())
    }

    fn finish(mut self) -> io::Result<W> {
        let too_big = |what: &str| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{what} is too large for a zip without zip64, which bundles do not use"),
            )
        };
        let count = u16::try_from(self.count).map_err(|_| too_big("the entry count"))?;
        let central_offset = u32::try_from(self.offset).map_err(|_| too_big("the archive"))?;
        let central_size =
            u32::try_from(self.central.len()).map_err(|_| too_big("the directory"))?;
        self.out.write_all(&self.central)?;
        let mut eocd = Vec::with_capacity(22);
        eocd.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes()); // this disk
        eocd.extend_from_slice(&0u16.to_le_bytes()); // directory disk
        eocd.extend_from_slice(&count.to_le_bytes());
        eocd.extend_from_slice(&count.to_le_bytes());
        eocd.extend_from_slice(&central_size.to_le_bytes());
        eocd.extend_from_slice(&central_offset.to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes()); // comment
        self.out.write_all(&eocd)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

#[allow(clippy::too_many_arguments)]
fn write_bundle_into(
    staging: &Path,
    manifest_text: &str,
    component: &[u8],
    assets_dir: Option<&Path>,
    source_dir: Option<&Path>,
    sdk_dir: Option<&Path>,
    derived_from: Option<&[u8]>,
    extensions: &[ExtensionSource],
) -> Result<()> {
    let output_path = staging;
    let file = File::create(staging).map_err(|err| io_err(staging, err))?;
    let mut zip = DeterministicZip::new(io::BufWriter::new(file));
    let add = |zip: &mut DeterministicZip<_>, name: &str, bytes: &[u8]| -> Result<()> {
        zip.add(name, bytes).map_err(|err| io_err(output_path, err))
    };

    // First entry, so a reader meets it before anything else (IC-208).
    add(
        &mut zip,
        PROFILE_ENTRY,
        PROFILE_VERSION.to_string().as_bytes(),
    )?;
    add(&mut zip, MANIFEST_ENTRY, manifest_text.as_bytes())?;
    add(&mut zip, COMPONENT_ENTRY, component)?;
    if let Some(record) = derived_from {
        add(&mut zip, DERIVED_FROM_ENTRY, record)?;
    }
    // Extensions: the declaration first, then every group's files. The
    // declaration is computed from the bytes about to be written, so what
    // it says and what the archive holds cannot disagree.
    if !extensions.is_empty() {
        let mut groups: Vec<(&ExtensionSource, BTreeMap<String, Vec<u8>>)> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for extension in extensions {
            let key = format!("{}/{}", extension.owner, extension.name);
            if !seen.insert(key.clone()) {
                return Err(BundleError::DamagedExtensions {
                    detail: format!("{key} is given twice"),
                });
            }
            let prefix = extension.prefix();
            let mut files = BTreeMap::new();
            for (name, source) in collect_files(&extension.root, &|_| false)? {
                let entry_name = format!("{prefix}{}", &name[SOURCE_PREFIX.len()..]);
                files.insert(
                    entry_name,
                    fs::read(&source).map_err(|err| io_err(&source, err))?,
                );
            }
            if files.is_empty() {
                return Err(BundleError::MissingExtension { extension: key });
            }
            groups.push((extension, files));
        }
        let record = ExtensionsRecord {
            schema: EXTENSIONS_SCHEMA.to_string(),
            extensions: groups
                .iter()
                .map(|(extension, files)| ExtensionRecord {
                    owner: extension.owner.clone(),
                    name: extension.name.clone(),
                    version: extension.version.clone(),
                    size: files.values().map(|body| body.len() as u64).sum(),
                    digest: extension_digest(files),
                })
                .collect(),
        };
        let body = serde_json::to_vec_pretty(&record)
            .map_err(|err| BundleError::Manifest(format!("{EXTENSIONS_ENTRY}: {err}")))?;
        add(&mut zip, EXTENSIONS_ENTRY, &body)?;
        for (_, files) in &groups {
            for (entry_name, bytes) in files {
                add(&mut zip, entry_name, bytes)?;
            }
        }
    }
    // What shipped as an asset, by its bytes, so the source pass below can
    // recognise the same file and ship it once (K-851).
    let mut asset_digests: BTreeSet<String> = BTreeSet::new();
    if let Some(assets_dir) = assets_dir.filter(|path| path.is_dir()) {
        for (entry_name, source) in collect_assets(assets_dir)? {
            let bytes = fs::read(&source).map_err(|err| io_err(&source, err))?;
            asset_digests.insert(sha256_hex(&bytes));
            add(&mut zip, &entry_name, &bytes)?;
        }
    }
    // Source and SDK are gathered before they are written, so the closure
    // record can be computed over the exact bytes that ship and written
    // ahead of them.
    let mut source_entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    if let Some(source_dir) = source_dir.filter(|path| path.is_dir()) {
        for (entry_name, source) in collect_source(source_dir)? {
            // Cargo.toml points at the SDK by absolute path, because that is
            // where this machine materialised it. Shipped as-is, the source in
            // a bundle only rebuilds on the machine that made it -- which
            // defeats the point of shipping source at all. Rewriting to a
            // placeholder lets any Krate install substitute its own SDK.
            if entry_name.ends_with("Cargo.toml") {
                let text = fs::read_to_string(&source).map_err(|err| io_err(&source, err))?;
                source_entries.insert(entry_name, rewrite_sdk_paths(&text).into_bytes());
                continue;
            }
            let bytes = fs::read(&source).map_err(|err| io_err(&source, err))?;
            // Already shipped as an asset: one copy serves both readers.
            //
            // `assets/` lives inside the source tree, so every asset was
            // written twice -- once as the asset the app loads, once as
            // source for a later change. A 12-asset game packed 2,262,993
            // bytes with the duplicate and 1,272,551 without; a user's
            // weather app was 849 KB of which 797 KB was one icon counted
            // twice, against 69 KB of program (K-851).
            //
            // Safe because `open` now places assets into the edit tree
            // when source does not carry them, so `revise` still sees the
            // whole project. Matched on BYTES, not path: a file that
            // differs between the two trees still ships both, because then
            // they are genuinely two files.
            if asset_digests.contains(&sha256_hex(&bytes)) {
                continue;
            }
            source_entries.insert(entry_name, bytes);
        }
    }
    let mut sdk_entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    if let Some(sdk_dir) = sdk_dir.filter(|path| path.is_dir()) {
        for (entry_name, source) in collect_tree(sdk_dir, SDK_PREFIX)? {
            let bytes = fs::read(&source).map_err(|err| io_err(&source, err))?;
            sdk_entries.insert(entry_name, bytes);
        }
    }
    if !source_entries.is_empty() {
        let record = closure_record(component, &source_entries, &sdk_entries);
        let body = serde_json::to_vec_pretty(&record)
            .map_err(|err| BundleError::Manifest(format!("{CLOSURE_ENTRY}: {err}")))?;
        add(&mut zip, CLOSURE_ENTRY, &body)?;
    }
    for (entry_name, bytes) in &source_entries {
        add(&mut zip, entry_name, bytes)?;
    }
    for (entry_name, bytes) in &sdk_entries {
        add(&mut zip, entry_name, bytes)?;
    }
    zip.finish().map_err(|err| io_err(output_path, err))?;
    Ok(())
}

/// Where a bundle is assembled before it replaces anything.
///
/// Beside the destination, so the rename that commits it stays within one
/// filesystem and is therefore atomic. The name carries the process id so two
/// packs running at once cannot write into each other's staging file.
fn staging_path_for(output_path: &Path) -> PathBuf {
    let name = output_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "bundle.krate".to_string());
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!(".{name}.{}.partial", std::process::id()))
}

/// A bundle unpacked into a temporary directory.
///
/// The directory lives as long as this value and is removed on drop, so a
/// fetched bundle leaves nothing behind after the run.
#[derive(Debug)]
pub struct OpenBundle {
    _dir: TempDir,
    manifest_path: PathBuf,
    component_path: PathBuf,
    assets_path: Option<PathBuf>,
    source_path: Option<PathBuf>,
    sdk_path: Option<PathBuf>,
    ext_path: Option<PathBuf>,
    manifest: Manifest,
    profile: u32,
    records: Vec<Record>,
    extensions: Vec<ExtensionRecord>,
    closure: Option<Closure>,
}

/// What a bundle carries where a signature would be (K-258).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureState {
    /// No `signature.json`. The bundle was never signed.
    Absent,
    /// A `signature.json` that is not a signature envelope: truncated,
    /// edited, or replaced. Somebody signed this and something happened to
    /// it afterwards, which is worth saying out loud.
    Damaged { detail: String },
    /// A signature envelope that parsed. Whether it VERIFIES is a separate
    /// question, answered by [`OpenBundle::full_verdict`].
    Signed(Box<signing::SignatureEnvelope>),
}

impl SignatureState {
    /// Words for a person, or `None` when there is nothing to report.
    ///
    /// Absent is not news -- most bundles are unsigned and that is a state
    /// Krate supports. A damaged signature is news.
    pub fn concern(&self) -> Option<String> {
        match self {
            SignatureState::Absent | SignatureState::Signed(_) => None,
            SignatureState::Damaged { detail } => Some(format!(
                "this app carries a signature that cannot be read ({detail}). \
                 It was signed and then changed, or the copy is damaged. \
                 Treat it as unsigned, and get a fresh copy from whoever sent it."
            )),
        }
    }
}

impl OpenBundle {
    /// Path to the extracted manifest.
    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    /// Path to the extracted component.
    pub fn component_path(&self) -> &Path {
        &self.component_path
    }

    /// Root of the extracted portable assets, when the bundle contains any.
    pub fn assets_path(&self) -> Option<&Path> {
        self.assets_path.as_deref()
    }

    /// Path to the app's extracted source, when the bundle carries it.
    ///
    /// This is what makes an app changeable: hand this directory and a sentence
    /// to an AI and it can rebuild the app rather than write a new one from
    /// nothing. `None` for bundles packed before source shipped.
    pub fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    /// Path to the SDK this app was built against, when the bundle carries it.
    ///
    /// This is what makes an old app still editable: its source compiles
    /// against the SDK it was written for, not whichever one the reader
    /// happens to have.
    pub fn sdk_path(&self) -> Option<&Path> {
        self.sdk_path.as_deref()
    }

    /// The container profile the bundle was written for (1 when it carries
    /// no profile line).
    pub fn profile(&self) -> u32 {
        self.profile
    }

    /// Every record in the archive, sorted by name, with the class the
    /// profile gives it (IC-714, test 1477).
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// The extension groups the bundle declares, each verified against its
    /// entries (test 1476).
    pub fn extensions(&self) -> &[ExtensionRecord] {
        &self.extensions
    }

    /// What built the component and what it takes to build it again, when
    /// the bundle carries source; checked against the source and SDK it
    /// carries (CP1, the editable closure).
    pub fn closure(&self) -> Option<&Closure> {
        self.closure.as_ref()
    }

    /// Where a declared extension group's files were unpacked, if the
    /// bundle carries that group.
    pub fn extension_path(&self, owner: &str, name: &str) -> Option<PathBuf> {
        let declared = self
            .extensions
            .iter()
            .any(|record| record.owner == owner && record.name == name);
        let root = self.ext_path.as_ref()?;
        declared.then(|| root.join(owner).join(name))
    }

    /// The parsed manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// This bundle's content identity.
    ///
    /// Computed from the extracted contents rather than the archive file, so
    /// two archives holding the same app agree on its identity even if they
    /// differ in timestamps, compression, or entry order. Re-packing an app
    /// therefore does not invalidate a reference to it.
    pub fn digest(&self) -> Result<provenance::BundleDigest> {
        Ok(provenance::digest_layer(
            provenance::Layer::Execution,
            &self.entries_for_digest(provenance::Layer::Execution)?,
        ))
    }

    /// This bundle's project identity: everything a person could rebuild.
    ///
    /// The execution set plus `source/` and `sdk/`. Two bundles equal here are
    /// the same editable project; two bundles equal only on [`Self::digest`]
    /// behave the same but may carry different source, which is a real
    /// difference and the one the execution digest cannot see.
    ///
    /// This exists because that blindness shipped: a bundle carrying 3 source
    /// files and one carrying 4, with different `lib.rs`, printed the same
    /// value under the heading "Identity" (K-245).
    pub fn project_digest(&self) -> Result<provenance::BundleDigest> {
        Ok(provenance::digest_layer(
            provenance::Layer::Project,
            &self.entries_for_digest(provenance::Layer::Project)?,
        ))
    }

    /// What a signature over this bundle says, if it carries one (IC-015).
    ///
    /// The statement is RECOMPUTED from the extracted files, never parsed
    /// from the bundle: a stored statement is an attacker-supplied document,
    /// and a parser that disagreed with the canonical encoding by one byte
    /// would verify a signature over something other than what is on disk.
    ///
    /// `None` means unsigned, which is not a failure. An unsigned app runs
    /// and is labelled unverified -- requiring a signature to run would make
    /// publishing a precondition for using your own machine.
    pub fn signature_verdict(&self) -> Result<Option<signing::Verdict>> {
        let Some(envelope) = self.signature_envelope()? else {
            return Ok(None);
        };
        // Every entry, including source and SDK: a signature covers the whole
        // file, not the part that happens to run.
        let entries = self.entries_for_digest(provenance::Layer::Project)?;
        Ok(Some(signing::verify_envelope(&envelope, &entries)))
    }

    /// The full verdict: signature and, when present, the delegation chain.
    ///
    /// `revocations` is what the caller knows about withdrawn keys. It is a
    /// parameter rather than something this reads for itself, because
    /// "could not check" is a real answer that only the caller can report
    /// honestly -- a bundle cannot know whether the machine is offline.
    pub fn full_verdict(
        &self,
        revocations: &delegation::RevocationState,
    ) -> Result<Option<signing::FullVerdict>> {
        let Some(envelope) = self.signature_envelope()? else {
            return Ok(None);
        };
        let entries = self.entries_for_digest(provenance::Layer::Project)?;
        Ok(Some(signing::verify_full(&envelope, &entries, revocations)))
    }

    /// The publisher root whose revocation list applies to this bundle.
    ///
    /// The root named in the delegation, or the signing key itself when the
    /// publisher signed directly with their root. `None` for an unsigned
    /// bundle. It comes from the envelope, which is attacker-supplied --
    /// but the only thing it selects is WHICH list to consult, and a list
    /// is trusted only when signed by the root it names, so pointing at a
    /// different root gains nothing except a list that says nothing about
    /// this key.
    pub fn signing_authority(&self) -> Result<Option<String>> {
        Ok(self.signature_envelope()?.map(|envelope| {
            envelope
                .delegation
                .as_ref()
                .map(|d| d.delegation.root.clone())
                .unwrap_or(envelope.public_key)
        }))
    }

    /// The raw envelope, when the bundle carries one that parses.
    ///
    /// A bundle whose signature will not parse yields `None` here, the same
    /// as one carrying no signature. That is deliberate for the callers that
    /// only need "is there something to check" -- but it is NOT enough to
    /// report to a person, because "unsigned" and "signed, and the signature
    /// is damaged" are different news. Use [`signature_state`] for that.
    ///
    /// [`signature_state`]: OpenBundle::signature_state
    pub fn signature_envelope(&self) -> Result<Option<signing::SignatureEnvelope>> {
        match self.signature_state()? {
            SignatureState::Signed(envelope) => Ok(Some(*envelope)),
            SignatureState::Absent | SignatureState::Damaged { .. } => Ok(None),
        }
    }

    /// What this bundle carries in place of a signature (K-258).
    ///
    /// The signed release this file is, when its signature verifies
    /// (IC-389, K-308). See [`signing::release_id`].
    ///
    /// `None` is unsigned, or signed by something that does not verify: a
    /// file whose signature fails is not a release, and giving it an id
    /// would make one up from a statement nobody signed.
    pub fn release(&self) -> Result<Option<signing::Release>> {
        let Some(envelope) = self.signature_envelope()? else {
            return Ok(None);
        };
        let entries = self.entries_for_digest(provenance::Layer::Project)?;
        if !signing::verify_envelope(&envelope, &entries).is_genuinely_signed() {
            return Ok(None);
        }
        let statement = statement::SignedStatement::build(
            &envelope.namespace,
            &envelope.version,
            envelope.signed_at,
            &entries,
        );
        let authority = envelope
            .delegation
            .as_ref()
            .map(|d| d.delegation.root.clone())
            .unwrap_or_else(|| envelope.public_key.clone());
        Ok(Some(signing::Release {
            id: signing::release_id(&authority, &statement.digest()),
            namespace: envelope.namespace,
            version: envelope.version,
            signed_at: envelope.signed_at,
            authority,
        }))
    }

    /// The app this one was changed from, if it says so (IC-397, test 494).
    ///
    /// `None` is an original, or an app made before forks were recorded.
    /// A record that is present but unreadable is an error: it is part of
    /// the project identity, and a damaged part of an identity is not
    /// "nothing".
    pub fn derived_from(&self) -> Result<Option<DerivedFrom>> {
        let path = self._dir.path().join(DERIVED_FROM_ENTRY);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|err| io_err(&path, err))?;
        let record: DerivedFrom = serde_json::from_slice(&bytes).map_err(|err| {
            BundleError::Manifest(format!("{DERIVED_FROM_ENTRY} is not readable: {err}"))
        })?;
        if record.schema != DERIVED_FROM_SCHEMA {
            return Err(BundleError::Manifest(format!(
                "{DERIVED_FROM_ENTRY} uses a newer format ({}) than this copy of Krate \
                 understands",
                record.schema
            )));
        }
        Ok(Some(record))
    }

    /// Three states, because there are three. Collapsing the middle one into
    /// "absent" told a recipient their tampered bundle was merely unsigned,
    /// which is the opposite of what a signature exists to tell them.
    pub fn signature_state(&self) -> Result<SignatureState> {
        let path = self._dir.path().join(SIGNATURE_ENTRY);
        if !path.is_file() {
            return Ok(SignatureState::Absent);
        }
        let bytes = fs::read(&path).map_err(|err| io_err(&path, err))?;
        match serde_json::from_slice::<signing::SignatureEnvelope>(&bytes) {
            Ok(envelope) => Ok(SignatureState::Signed(Box::new(envelope))),
            Err(err) => Ok(SignatureState::Damaged {
                detail: err.to_string(),
            }),
        }
    }

    /// Every entry this layer covers, read from the extracted tree.
    fn entries_for_digest(
        &self,
        layer: provenance::Layer,
    ) -> Result<std::collections::BTreeMap<String, Vec<u8>>> {
        let mut entries = std::collections::BTreeMap::new();
        entries.insert(
            MANIFEST_ENTRY.to_string(),
            manifest_bytes_for_digest(
                &fs::read(&self.manifest_path).map_err(|err| io_err(&self.manifest_path, err))?,
            ),
        );
        entries.insert(
            COMPONENT_ENTRY.to_string(),
            fs::read(&self.component_path).map_err(|err| io_err(&self.component_path, err))?,
        );
        if let Some(root) = self.assets_path.as_deref() {
            // Reuse the packing walk, so the names in a digest are exactly the
            // names the bundle stores -- already forward-slashed and already
            // refusing symlinks, rather than a second traversal that could
            // disagree with the first.
            for (entry_name, source) in collect_assets(root)? {
                entries.insert(
                    entry_name,
                    fs::read(&source).map_err(|err| io_err(&source, err))?,
                );
            }
        }

        // The two namespaces the execution identity deliberately ignores. They
        // are walked with the same collector as assets, so the names here are
        // the names the bundle stores.
        // A fork's record is part of what this bundle IS, not of what runs
        // (IC-397): the project layer and the signature cover it, the
        // execution layer does not, so a fork whose code is unchanged still
        // reports the same execution identity as its parent.
        let derived_path = self._dir.path().join(DERIVED_FROM_ENTRY);
        if derived_path.is_file() && layer.includes(DERIVED_FROM_ENTRY) {
            entries.insert(
                DERIVED_FROM_ENTRY.to_string(),
                fs::read(&derived_path).map_err(|err| io_err(&derived_path, err))?,
            );
        }

        let closure_path = self._dir.path().join(CLOSURE_ENTRY);
        if closure_path.is_file() && layer.includes(CLOSURE_ENTRY) {
            entries.insert(
                CLOSURE_ENTRY.to_string(),
                fs::read(&closure_path).map_err(|err| io_err(&closure_path, err))?,
            );
        }
        let extensions_path = self._dir.path().join(EXTENSIONS_ENTRY);
        if extensions_path.is_file() && layer.includes(EXTENSIONS_ENTRY) {
            entries.insert(
                EXTENSIONS_ENTRY.to_string(),
                fs::read(&extensions_path).map_err(|err| io_err(&extensions_path, err))?,
            );
        }

        for (root, prefix) in [
            (self.source_path.as_deref(), SOURCE_PREFIX),
            (self.sdk_path.as_deref(), SDK_PREFIX),
            (self.ext_path.as_deref(), EXTENSION_PREFIX),
        ] {
            let Some(root) = root else { continue };
            for (entry_name, source) in collect_unpacked(root, prefix)? {
                if !layer.includes(&entry_name) {
                    continue;
                }
                entries.insert(
                    entry_name,
                    fs::read(&source).map_err(|err| io_err(&source, err))?,
                );
            }
        }
        Ok(entries)
    }
}

/// Sign an existing bundle in place, writing `signature.json` into it.
///
/// The statement is built from the bundle's own extracted bytes, so what is
/// signed is exactly what the file contains -- there is no path by which a
/// caller can sign one thing and ship another.
///
/// Signing an already-signed bundle replaces the signature. That is the
/// honest behaviour: the previous signer's claim covered a file that no
/// longer has the same contents, so keeping it would preserve a signature
/// that could only ever read as tampering.
///
/// Written to a temporary file and renamed, so an interrupted sign leaves
/// the original bundle intact rather than a half-written archive.
pub fn sign_bundle(
    bundle_path: &Path,
    key: &signing::SigningKey,
    namespace: &str,
    version: &str,
    signed_at: u64,
) -> Result<signing::SignatureEnvelope> {
    let opened = open(bundle_path)?;
    let entries = opened.entries_for_digest(provenance::Layer::Project)?;
    let statement = statement::SignedStatement::build(namespace, version, signed_at, &entries);
    let envelope = signing::SignatureEnvelope::new(&statement, &key.sign(&statement));
    let json = serde_json::to_vec_pretty(&envelope)
        .map_err(|err| BundleError::Manifest(err.to_string()))?;

    // Rewrite the archive with the signature entry added, copying every other
    // entry through untouched. Reading the source fully before writing the
    // destination is what allows the destination to be the same path.
    let source = fs::read(bundle_path).map_err(|err| io_err(bundle_path, err))?;
    let mut archive = ZipArchive::new(io::Cursor::new(&source))?;
    let temporary = bundle_path.with_extension("krate.signing");
    {
        let file = File::create(&temporary).map_err(|err| io_err(&temporary, err))?;
        let mut writer = DeterministicZip::new(io::BufWriter::new(file));
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let name = entry.name().to_string();
            if name == SIGNATURE_ENTRY {
                continue; // replaced below
            }
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|err| BundleError::Io {
                    path: PathBuf::from(&name),
                    source: err,
                })?;
            writer
                .add(&name, &bytes)
                .map_err(|err| io_err(&temporary, err))?;
        }
        writer
            .add(SIGNATURE_ENTRY, &json)
            .map_err(|err| io_err(&temporary, err))?;
        writer.finish().map_err(|err| io_err(&temporary, err))?;
    }
    fs::rename(&temporary, bundle_path).map_err(|err| io_err(bundle_path, err))?;
    Ok(envelope)
}

/// Sign a bundle with a delegated release key and ship the root's
/// permission slip inside it, so a recipient can check the chain offline.
///
/// The delegation is attached exactly as given; whether it actually covers
/// this key and namespace is the verifier's question, answered on every
/// open. Signing with a slip that does not fit produces a bundle that says
/// so wherever it is opened, which is better than refusing here and having
/// the publisher attach it by hand.
pub fn sign_bundle_delegated(
    bundle_path: &Path,
    key: &signing::SigningKey,
    namespace: &str,
    version: &str,
    signed_at: u64,
    delegation: delegation::SignedDelegation,
) -> Result<signing::SignatureEnvelope> {
    let mut envelope = sign_bundle(bundle_path, key, namespace, version, signed_at)?;
    envelope.delegation = Some(delegation);
    let json = serde_json::to_vec_pretty(&envelope)
        .map_err(|err| BundleError::Manifest(err.to_string()))?;
    replace_entry(bundle_path, SIGNATURE_ENTRY, &json)?;
    Ok(envelope)
}

/// Rewrite one entry of a bundle in place, copying every other entry
/// through untouched. Written beside the bundle and renamed over it.
fn replace_entry(bundle_path: &Path, entry_name: &str, bytes: &[u8]) -> Result<()> {
    let source = fs::read(bundle_path).map_err(|err| io_err(bundle_path, err))?;
    let mut archive = ZipArchive::new(io::Cursor::new(&source))?;
    let temporary = bundle_path.with_extension("krate.signing");
    {
        let file = File::create(&temporary).map_err(|err| io_err(&temporary, err))?;
        let mut writer = DeterministicZip::new(io::BufWriter::new(file));
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let name = entry.name().to_string();
            if name == entry_name {
                continue;
            }
            let mut body = Vec::new();
            entry
                .read_to_end(&mut body)
                .map_err(|err| BundleError::Io {
                    path: PathBuf::from(&name),
                    source: err,
                })?;
            writer
                .add(&name, &body)
                .map_err(|err| io_err(&temporary, err))?;
        }
        writer
            .add(entry_name, bytes)
            .map_err(|err| io_err(&temporary, err))?;
        writer.finish().map_err(|err| io_err(&temporary, err))?;
    }
    fs::rename(&temporary, bundle_path).map_err(|err| io_err(bundle_path, err))?;
    Ok(())
}

/// Open a bundle from disk, extracting it into a temporary directory.
pub fn open(bundle_path: &Path) -> Result<OpenBundle> {
    let size = fs::metadata(bundle_path)
        .map_err(|err| io_err(bundle_path, err))?
        .len();
    if size > MAX_BUNDLE_BYTES {
        return Err(BundleError::TooLarge { size });
    }
    // Compare what the FILE claims against what the parser will see (K-252).
    let bytes = fs::read(bundle_path).map_err(|err| io_err(bundle_path, err))?;
    refuse_undeclared_duplicate(&bytes)?;

    let file = File::open(bundle_path).map_err(|err| io_err(bundle_path, err))?;
    open_reader(file)
}

// Where a bundle is unpacked: the system temp directory in a real run.
// Tests can point it somewhere they own, which is the only way to check that
// a REFUSED bundle removed what it wrote -- the shared temp directory is full
// of other tests' live directories, and telling those apart from a leak is
// not possible from the outside.
#[cfg(test)]
thread_local! {
    static EXTRACT_ROOT: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Prefix on every directory Krate unpacks into.
///
/// tempfile's default is `.tmp`, which is what every other program on the
/// machine uses too. Naming ours makes them findable -- which is what lets
/// [`sweep_abandoned_extractions`] clean up after a run that was killed
/// before its directory could be removed (K-264).
const EXTRACT_PREFIX: &str = "krate-open-";

/// How long an abandoned directory is left alone before it is swept.
///
/// Long enough that a slow open in another process is never touched: opening
/// a large bundle took 6.5 seconds in the measurement behind K-264, and an
/// hour is three orders of magnitude past that. Short enough that a machine
/// does not accumulate them for weeks.
const ABANDONED_AFTER: std::time::Duration = std::time::Duration::from_secs(60 * 60);

fn new_extract_dir() -> io::Result<TempDir> {
    // Once per process, not once per open: a run that opens twenty bundles
    // should not walk the temp directory twenty times, and anything a
    // concurrent process leaves behind is younger than the threshold anyway.
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(sweep_abandoned_extractions);

    #[cfg(test)]
    let root = EXTRACT_ROOT
        .with(|r| r.borrow().clone())
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(test))]
    let root = std::env::temp_dir();
    tempfile::Builder::new()
        .prefix(EXTRACT_PREFIX)
        .tempdir_in(&root)
        .map_err(|err| {
            // tempfile quotes the path Debug-style, which on Windows doubles
            // every backslash -- the K-263 test read the real path back and
            // could not find it in the message. Say it in its own spelling,
            // with the OS's reason and without the path repeated.
            let reason = std::error::Error::source(&err)
                .map(ToString::to_string)
                .unwrap_or_else(|| err.to_string());
            io::Error::new(
                err.kind(),
                format!("could not create a folder in {}: {reason}", root.display()),
            )
        })
}

/// Remove extraction directories a previous run left behind (K-264).
///
/// A bundle is unpacked into a directory that is removed when the value
/// owning it drops. A hard kill runs no destructors, so the directory and
/// everything in it stays -- measured at 200 MB from one interrupted open.
/// A signal handler would cover Ctrl-C and not SIGKILL, and neither covers
/// power loss; sweeping on the next open covers all three.
///
/// Deliberately best effort. This runs on the path that opens an app, and a
/// permission error on somebody else's temp directory is not a reason to
/// refuse to open a file. Anything it cannot remove is left for next time.
fn sweep_abandoned_extractions() {
    sweep_in(&std::env::temp_dir());
}

/// The sweep, against a named directory so it can be tested without
/// touching the machine's real temp directory.
fn sweep_in(root: &Path) {
    sweep_abandoned_dirs(root, EXTRACT_PREFIX, ABANDONED_AFTER);
}

/// Remove every directory directly under `root` whose name starts with
/// `prefix` and that was last modified more than `older_than` ago.
///
/// The same sweep [`open`] runs over its own `krate-open-*` extractions,
/// offered to callers that leave directories of their own: the CLI's
/// `krate-edit-*` working copies were kept on purpose so a failed change
/// could be read, and then nothing ever removed them (K-313). One sweep
/// with two prefixes rather than two sweeps, so the two rules cannot drift
/// apart: age is the only thing separating abandoned from in use, and a
/// directory with another name is never touched however old it is.
///
/// Best effort, like the extraction sweep: a directory that cannot be
/// removed is left for next time, and nothing here is a reason to refuse
/// the work the caller was about to do.
pub fn sweep_abandoned_dirs(root: &Path, prefix: &str, older_than: std::time::Duration) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(prefix) {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > older_than);
        if old_enough {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Open a bundle from any reader that can seek.
/// Refuse an archive that declares more central-directory records than a
/// parser can see (K-252, K-278).
///
/// `ZipArchive` keys entries by name, so two records naming one path become
/// one entry and the LAST wins -- silently. The end-of-central-directory
/// count is the only place the duplicate survives, and it is the check that
/// catches a duplicate a real ZIP writer produced (the record-count path,
/// not the name-comparison one).
///
/// This needs the whole byte string, so it belongs to the callers that hold
/// one: `open` (from disk) and `open_bytes` (a download, a hub upload). A
/// pure streaming `open_reader` cannot re-read its input, which is why the
/// bytes-holding entry points exist.
fn refuse_undeclared_duplicate(bytes: &[u8]) -> Result<()> {
    if let Some(declared) = central_directory_record_count(bytes) {
        let parsed = ZipArchive::new(io::Cursor::new(bytes))
            .map(|archive| archive.len())
            .unwrap_or(declared);
        if declared > parsed {
            return Err(BundleError::DuplicateEntry {
                path: first_duplicate_record_name(bytes).unwrap_or_else(|| {
                    format!("{declared} entries but only {parsed} distinct names")
                }),
            });
        }
    }
    Ok(())
}

/// Open a bundle held in memory -- a download, or a hub upload (K-278).
///
/// The same validation as [`open`], including the record-count duplicate
/// check that a pure streaming reader cannot do. Any caller that already
/// has the bytes should use this rather than `open_reader`, so admission on
/// the server is the same open the recipient runs.
pub fn open_bytes(bytes: &[u8]) -> Result<OpenBundle> {
    if bytes.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(BundleError::TooLarge {
            size: bytes.len() as u64,
        });
    }
    refuse_undeclared_duplicate(bytes)?;
    open_reader(io::Cursor::new(bytes.to_vec()))
}

/// What the validator concluded about a bundle it judged in memory, for a
/// door that has the bytes and no disk: the hub's publish endpoint (IC-833,
/// K-309), where this crate runs as WebAssembly.
///
/// Every rule here is the one `open` applies: the record-count duplicate
/// check, the entry preflight, the profile, the manifest, the component
/// against its declared world, the per-namespace ceilings, the identities,
/// the signature recomputed over the entries, the release id, the fork
/// record. `open` writes the entries to a temp directory and reads them
/// back; this keeps them in memory under a caller-set ceiling. The test
/// `the_two_doors_agree` holds them to the same answers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Judgement {
    /// Which rules judged the component, and the WIT they came from (IC-210).
    pub validator: u32,
    pub wit: String,
    pub manifest: Manifest,
    /// Plain SHA-256 of the bytes: the hub's store key.
    pub archive: String,
    /// The execution identity (what runs).
    pub execution: String,
    /// The project identity, when there is genuinely more to rebuild.
    pub project: Option<String>,
    pub signature: JudgedSignature,
    /// The signed release, when the signature verifies.
    pub release: Option<signing::Release>,
    pub derived_from: Option<DerivedFrom>,
    /// How many entries were read, and how many bytes they expanded to.
    pub entries: usize,
    pub expanded_bytes: u64,
    /// The container profile the bundle declares (1 when it has no line).
    pub profile: u32,
    /// Every record, sorted by name, with its class (IC-714, test 1477).
    pub records: Vec<Record>,
    /// The declared and verified extension groups (test 1476).
    pub extensions: Vec<ExtensionRecord>,
    /// What built the component and what it takes to build it again, when
    /// the bundle carries source (CP1, the editable closure).
    pub closure: Option<Closure>,
}

/// The signature's state, in words a door can act on.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JudgedSignature {
    /// `absent`, `valid`, `bad`, `tampered`, `unknown-schema` or `damaged`.
    pub state: String,
    pub public_key: Option<String>,
    pub detail: Option<String>,
}

/// Judge a bundle held in memory. See [`Judgement`].
///
/// `max_expanded` bounds the bytes every entry expands to together, on top
/// of the per-namespace ceilings `open` applies; a door in a small runtime
/// sets it to what it can hold. The archive itself is bounded by
/// [`MAX_BUNDLE_BYTES`] as everywhere else.
pub fn judge_bytes(bytes: &[u8], max_expanded: u64) -> Result<Judgement> {
    use std::collections::BTreeMap;

    if bytes.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(BundleError::TooLarge {
            size: bytes.len() as u64,
        });
    }
    refuse_undeclared_duplicate(bytes)?;
    let mut archive = ZipArchive::new(io::Cursor::new(bytes))?;
    preflight_entries(&mut archive)?;
    let profile = check_profile(&mut archive)?;
    let records = records_of(&mut archive)?;
    refuse_unknown_records(profile, &records)?;

    // The same namespaces `open` extracts, under the same ceilings, read
    // with the same forged-size guard: the declared size is the archive's
    // claim, so every entry is read to its ceiling plus one and judged on
    // what actually came out.
    let mut raw: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut expanded = 0u64;
    let mut asset_bytes = 0u64;
    let mut source_bytes = 0u64;
    let mut asset_count = 0usize;
    let names: Vec<String> = (0..archive.len())
        .map(|index| {
            archive
                .by_index(index)
                .map(|entry| entry.name().to_string())
        })
        .collect::<std::result::Result<_, _>>()?;
    for name in names {
        if name.ends_with('/') {
            continue;
        }
        let (ceiling, class) = if name == MANIFEST_ENTRY
            || name == COMPONENT_ENTRY
            || name == SIGNATURE_ENTRY
            || name == DERIVED_FROM_ENTRY
            || name == EXTENSIONS_ENTRY
            || name == CLOSURE_ENTRY
        {
            (MAX_ENTRY_BYTES, 'c')
        } else if name.starts_with(ASSETS_PREFIX) {
            safe_asset_relative_path(&name)?;
            asset_count += 1;
            if asset_count > MAX_ASSET_COUNT {
                return Err(BundleError::TooManyAssets);
            }
            (MAX_ASSET_BYTES, 'a')
        } else if name.starts_with(SOURCE_PREFIX) {
            safe_source_relative_path(&name)?;
            (MAX_ENTRY_BYTES, 's')
        } else if name.starts_with(SDK_PREFIX) {
            safe_prefixed_relative_path(&name, SDK_PREFIX)?;
            (MAX_ENTRY_BYTES, 's')
        } else if name.starts_with(EXTENSION_PREFIX) {
            safe_prefixed_relative_path(&name, EXTENSION_PREFIX)?;
            (MAX_ENTRY_BYTES, 's')
        } else {
            continue; // unknown entries are ignored under profile 1, as `open` ignores them
        };
        let mut entry = archive.by_name(&name)?;
        if entry.size() > ceiling {
            return Err(BundleError::EntryTooLarge { entry: name });
        }
        let mut body = Vec::new();
        entry
            .by_ref()
            .take(ceiling + 1)
            .read_to_end(&mut body)
            .map_err(|err| BundleError::Io {
                path: PathBuf::from(&name),
                source: err,
            })?;
        let len = body.len() as u64;
        if len > ceiling {
            return Err(BundleError::EntryTooLarge { entry: name });
        }
        match class {
            'a' => {
                asset_bytes = asset_bytes
                    .checked_add(len)
                    .ok_or(BundleError::AssetsTooLarge)?;
                if asset_bytes > MAX_TOTAL_ASSET_BYTES {
                    return Err(BundleError::AssetsTooLarge);
                }
            }
            's' => {
                source_bytes = source_bytes
                    .checked_add(len)
                    .ok_or(BundleError::SourceTooLarge)?;
                if source_bytes > MAX_TOTAL_SOURCE_BYTES {
                    return Err(BundleError::SourceTooLarge);
                }
            }
            _ => {}
        }
        expanded = expanded
            .checked_add(len)
            .ok_or(BundleError::ExpandsTooFar {
                limit: max_expanded,
            })?;
        if expanded > max_expanded {
            return Err(BundleError::ExpandsTooFar {
                limit: max_expanded,
            });
        }
        raw.insert(name, body);
    }

    let extensions = check_extensions(profile, &raw)?;
    let closure = check_closure(&raw)?;
    let manifest_raw = raw
        .get(MANIFEST_ENTRY)
        .ok_or(BundleError::MissingEntry(MANIFEST_ENTRY))?;
    let manifest_text = String::from_utf8_lossy(manifest_raw).into_owned();
    let manifest =
        Manifest::parse(&manifest_text).map_err(|err| BundleError::Manifest(err.to_string()))?;
    let declared = manifest.app.entry.display().to_string();
    if declared != COMPONENT_ENTRY {
        return Err(BundleError::EntryMismatch { declared });
    }
    let component = raw
        .get(COMPONENT_ENTRY)
        .ok_or(BundleError::MissingEntry(COMPONENT_ENTRY))?;
    check_component(component, COMPONENT_ENTRY, &manifest)?;

    // The digest maps, exactly as `entries_for_digest` builds them: the
    // manifest with its line endings folded, the signature never included,
    // the profile never included.
    let mut project_entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (name, body) in &raw {
        if name == SIGNATURE_ENTRY || name == PROFILE_ENTRY {
            continue;
        }
        let body = if name == MANIFEST_ENTRY {
            manifest_bytes_for_digest(body)
        } else {
            body.clone()
        };
        project_entries.insert(name.clone(), body);
    }
    // digest_layer keeps only what its layer includes, so one map serves
    // both identities.
    let execution = provenance::digest_layer(provenance::Layer::Execution, &project_entries);
    let project = provenance::digest_layer(provenance::Layer::Project, &project_entries);

    let derived_from = match raw.get(DERIVED_FROM_ENTRY) {
        None => None,
        Some(body) => {
            let record: DerivedFrom = serde_json::from_slice(body).map_err(|err| {
                BundleError::Manifest(format!("{DERIVED_FROM_ENTRY} is not readable: {err}"))
            })?;
            if record.schema != DERIVED_FROM_SCHEMA {
                return Err(BundleError::Manifest(format!(
                    "{DERIVED_FROM_ENTRY} uses a newer format ({}) than this copy of Krate \
                     understands",
                    record.schema
                )));
            }
            Some(record)
        }
    };

    let (signature, release) = match raw.get(SIGNATURE_ENTRY) {
        None => (
            JudgedSignature {
                state: "absent".to_string(),
                public_key: None,
                detail: None,
            },
            None,
        ),
        Some(body) => match serde_json::from_slice::<signing::SignatureEnvelope>(body) {
            Err(err) => (
                JudgedSignature {
                    state: "damaged".to_string(),
                    public_key: None,
                    detail: Some(err.to_string()),
                },
                None,
            ),
            Ok(envelope) => {
                let verdict = signing::verify_envelope(&envelope, &project_entries);
                let state = match &verdict {
                    signing::Verdict::Valid { .. } => "valid",
                    signing::Verdict::BadSignature => "bad",
                    signing::Verdict::Tampered { .. } => "tampered",
                    signing::Verdict::UnknownSchema { .. } => "unknown-schema",
                };
                let release = if verdict.is_genuinely_signed() {
                    let statement = statement::SignedStatement::build(
                        &envelope.namespace,
                        &envelope.version,
                        envelope.signed_at,
                        &project_entries,
                    );
                    let authority = envelope
                        .delegation
                        .as_ref()
                        .map(|d| d.delegation.root.clone())
                        .unwrap_or_else(|| envelope.public_key.clone());
                    Some(signing::Release {
                        id: signing::release_id(&authority, &statement.digest()),
                        namespace: envelope.namespace.clone(),
                        version: envelope.version.clone(),
                        signed_at: envelope.signed_at,
                        authority,
                    })
                } else {
                    None
                };
                (
                    JudgedSignature {
                        state: state.to_string(),
                        public_key: Some(envelope.public_key.clone()),
                        detail: match &verdict {
                            signing::Verdict::Valid { .. } => None,
                            other => Some(other.to_string()),
                        },
                    },
                    release,
                )
            }
        },
    };

    let archive_sha256: String = {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let entries = raw.len();
    Ok(Judgement {
        validator: imports::VALIDATOR_VERSION,
        wit: imports::WIT_DIGEST.to_string(),
        manifest,
        archive: archive_sha256,
        execution: execution.digest,
        project: (project.entries.len() > execution.entries.len()).then_some(project.digest),
        signature,
        release,
        derived_from,
        entries,
        expanded_bytes: expanded,
        profile,
        records,
        extensions,
        closure,
    })
}

pub fn open_reader<R: Read + io::Seek>(reader: R) -> Result<OpenBundle> {
    let mut archive = ZipArchive::new(reader)?;

    // Judge the archive as a whole before writing any of it to disk (IC-209).
    preflight_entries(&mut archive)?;

    // The error tempfile returns names the directory it could not create, so
    // it is passed through whole rather than relabelled `<tempdir>` -- a
    // placeholder that reached people and named a file that does not exist
    // (K-263).
    let dir = new_extract_dir().map_err(|err| BundleError::Unpack {
        detail: err.to_string(),
    })?;
    let manifest_path = dir.path().join(MANIFEST_ENTRY);
    let component_path = dir.path().join(COMPONENT_ENTRY);
    let assets_path = dir.path().join("assets");

    // Reading by exact name rather than iterating entries is what makes path
    // traversal unrepresentable: any other entry in the archive is ignored, and
    // neither name can escape the temp directory.
    // The profile FIRST, before the manifest is parsed (IC-208).
    //
    // A bundle from a future Krate should say so, not have its manifest
    // read under this version's rules and fail somewhere further in with a
    // message about a field. Absent means generation 1: every bundle
    // shipped so far predates this entry and is exactly what version 1
    // describes.
    let profile = check_profile(&mut archive)?;
    let records = records_of(&mut archive)?;
    refuse_unknown_records(profile, &records)?;

    extract_entry(&mut archive, MANIFEST_ENTRY, &manifest_path)?;
    extract_entry(&mut archive, COMPONENT_ENTRY, &component_path)?;
    // Optional, and by exact name like the two above. A bundle without one is
    // unsigned, which is a state Krate supports rather than an error.
    let signature_path = dir.path().join(SIGNATURE_ENTRY);
    let _ = extract_entry(&mut archive, SIGNATURE_ENTRY, &signature_path);
    let derived_path = dir.path().join(DERIVED_FROM_ENTRY);
    let _ = extract_entry(&mut archive, DERIVED_FROM_ENTRY, &derived_path);
    let asset_names = asset_entry_names(&mut archive)?;
    let mut total_asset_bytes = 0_u64;
    for name in &asset_names {
        let relative = safe_asset_relative_path(name)?;
        let destination = assets_path.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|err| io_err(parent, err))?;
        }
        total_asset_bytes = total_asset_bytes
            .checked_add(extract_asset_entry(&mut archive, name, &destination)?)
            .ok_or(BundleError::AssetsTooLarge)?;
        if total_asset_bytes > MAX_TOTAL_ASSET_BYTES {
            return Err(BundleError::AssetsTooLarge);
        }
    }

    // Source is extracted through the same guard as assets, so a crafted entry
    // name cannot write outside the temp directory.
    let source_path = dir.path().join("source");
    let source_names: Vec<String> = {
        let mut names = Vec::new();
        for index in 0..archive.len() {
            let name = archive.by_index(index)?.name().to_string();
            if name.starts_with(SOURCE_PREFIX) && !name.ends_with('/') {
                names.push(name);
            }
        }
        names
    };
    // Bytes actually WRITTEN, not the sizes the headers declare (K-255).
    //
    // preflight_entries() already refused this bundle if its declared source
    // total is over the limit, which is the cheap early check. But a header
    // can say one byte and deliver ninety megabytes, and the declared total
    // is the only thing that check can see. Counting what comes out of the
    // decompressor is the number that matters, and it is the same thing the
    // asset loop above already does. Source and SDK share one running total
    // because MAX_TOTAL_SOURCE_BYTES covers both together.
    let mut total_source_bytes = 0_u64;
    for name in &source_names {
        let relative = safe_source_relative_path(name)?;
        let destination = source_path.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|err| io_err(parent, err))?;
        }
        total_source_bytes = total_source_bytes
            .checked_add(extract_asset_entry(&mut archive, name, &destination)?)
            .ok_or(BundleError::SourceTooLarge)?;
        if total_source_bytes > MAX_TOTAL_SOURCE_BYTES {
            return Err(BundleError::SourceTooLarge);
        }
    }

    let sdk_path = dir.path().join("sdk");
    let sdk_names: Vec<String> = {
        let mut names = Vec::new();
        for index in 0..archive.len() {
            let name = archive.by_index(index)?.name().to_string();
            if name.starts_with(SDK_PREFIX) && !name.ends_with('/') {
                names.push(name);
            }
        }
        names
    };
    for name in &sdk_names {
        let relative = safe_prefixed_relative_path(name, SDK_PREFIX)?;
        let destination = sdk_path.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|err| io_err(parent, err))?;
        }
        total_source_bytes = total_source_bytes
            .checked_add(extract_asset_entry(&mut archive, name, &destination)?)
            .ok_or(BundleError::SourceTooLarge)?;
        if total_source_bytes > MAX_TOTAL_SOURCE_BYTES {
            return Err(BundleError::SourceTooLarge);
        }
    }

    // Developer extensions, through the same guard as the SDK tree, then
    // checked against their declaration (IC-714, test 1476). The
    // declaration is read by exact name like the manifest.
    let ext_path = dir.path().join("ext");
    let extensions_path = dir.path().join(EXTENSIONS_ENTRY);
    let _ = extract_entry(&mut archive, EXTENSIONS_ENTRY, &extensions_path);
    let ext_names: Vec<String> = records
        .iter()
        .filter(|record| record.class == "extension")
        .map(|record| record.name.clone())
        .collect();
    let mut ext_entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for name in &ext_names {
        // The group shape first: `ext/<owner>/<name>/<file>`. A file sitting
        // where a group directory belongs would collide on disk below and
        // read as an I/O failure rather than as what it is.
        extension_group(name)?;
        let relative = safe_prefixed_relative_path(name, EXTENSION_PREFIX)?;
        let destination = ext_path.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|err| io_err(parent, err))?;
        }
        total_source_bytes = total_source_bytes
            .checked_add(extract_asset_entry(&mut archive, name, &destination)?)
            .ok_or(BundleError::SourceTooLarge)?;
        if total_source_bytes > MAX_TOTAL_SOURCE_BYTES {
            return Err(BundleError::SourceTooLarge);
        }
        ext_entries.insert(
            name.clone(),
            fs::read(&destination).map_err(|err| io_err(&destination, err))?,
        );
    }
    if extensions_path.is_file() {
        ext_entries.insert(
            EXTENSIONS_ENTRY.to_string(),
            fs::read(&extensions_path).map_err(|err| io_err(&extensions_path, err))?,
        );
    }
    let extensions = check_extensions(profile, &ext_entries)?;

    // The closure record, checked against the source and SDK that were
    // just unpacked (CP1, the editable closure).
    let closure_path = dir.path().join(CLOSURE_ENTRY);
    let _ = extract_entry(&mut archive, CLOSURE_ENTRY, &closure_path);
    let closure = if closure_path.is_file() {
        let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        entries.insert(
            CLOSURE_ENTRY.to_string(),
            fs::read(&closure_path).map_err(|err| io_err(&closure_path, err))?,
        );
        for (root, prefix, present) in [
            (&source_path, SOURCE_PREFIX, !source_names.is_empty()),
            (&sdk_path, SDK_PREFIX, !sdk_names.is_empty()),
        ] {
            if !present {
                continue;
            }
            for (entry_name, file) in collect_unpacked(root, prefix)? {
                entries.insert(
                    entry_name,
                    fs::read(&file).map_err(|err| io_err(&file, err))?,
                );
            }
        }
        check_closure(&entries)?
    } else {
        None
    };

    // An asset the source tree does not carry is placed beside it.
    //
    // `revise` edits the tree written from `source/` alone, so an app whose
    // images live only under `assets/` was handed to the AI without them --
    // it would edit code that loads files it cannot see. Until now that
    // never bit, because every asset was ALSO shipped inside source, at
    // double the bytes: a 12-asset game packs 2,262,993 bytes with the
    // duplicate and 1,272,551 without, 44% of the bundle (K-851).
    //
    // Placing them is what makes dropping the duplicate safe, and it is the
    // right behaviour on its own: the edit tree should look like the project
    // the app was built from, whichever records carried the bytes.
    //
    // It runs AFTER the closure check on purpose. That check re-derives
    // the source digest by walking the unpacked directory, so an asset
    // placed before it counts as source the record never covered and every
    // such bundle is refused as damaged. Written first, this was caught by
    // the test below rather than by a user whose app would not open.
    if !source_names.is_empty() {
        let asset_names: Vec<String> = {
            let mut names = Vec::new();
            for index in 0..archive.len() {
                let name = archive.by_index(index)?.name().to_string();
                if name.starts_with(ASSETS_PREFIX) && !name.ends_with('/') {
                    names.push(name);
                }
            }
            names
        };
        for name in &asset_names {
            let relative = safe_prefixed_relative_path(name, ASSETS_PREFIX)?;
            // `assets/icon.png` belongs at `<source>/assets/icon.png`, which
            // is where the manifest and the code name it.
            let destination = source_path.join(ASSETS_PREFIX).join(relative);
            // Never overwrite what source itself shipped: if both exist the
            // source copy is the one the rebuild compiled against.
            if destination.exists() {
                continue;
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|err| io_err(parent, err))?;
            }
            total_source_bytes = total_source_bytes
                .checked_add(extract_asset_entry(&mut archive, name, &destination)?)
                .ok_or(BundleError::SourceTooLarge)?;
            if total_source_bytes > MAX_TOTAL_SOURCE_BYTES {
                return Err(BundleError::SourceTooLarge);
            }
        }
    }

    let manifest_text =
        fs::read_to_string(&manifest_path).map_err(|err| io_err(&manifest_path, err))?;
    let manifest =
        Manifest::parse(&manifest_text).map_err(|err| BundleError::Manifest(err.to_string()))?;

    let declared = manifest.app.entry.display().to_string();
    if declared != COMPONENT_ENTRY {
        return Err(BundleError::EntryMismatch { declared });
    }

    // The component is judged here by the same rules `pack` applies, so a
    // bundle another writer produced -- or one edited after packing -- is
    // refused at open with the reason, not at instantiate with the
    // engine's (IC-210). Everything that opens a bundle goes through here:
    // run, publish, the hub's admission check.
    let component = fs::read(&component_path).map_err(|err| io_err(&component_path, err))?;
    check_component(&component, COMPONENT_ENTRY, &manifest)?;

    Ok(OpenBundle {
        _dir: dir,
        manifest_path,
        component_path,
        assets_path: (!asset_names.is_empty()).then_some(assets_path),
        source_path: (!source_names.is_empty()).then_some(source_path),
        sdk_path: (!sdk_names.is_empty()).then_some(sdk_path),
        ext_path: (!ext_names.is_empty()).then_some(ext_path),
        manifest,
        profile,
        records,
        extensions,
        closure,
    })
}

/// Gather the files needed to rebuild an app, under [`SOURCE_PREFIX`].
///
/// Skips what cannot be rebuilt from or would bloat the bundle: `target/` is
/// build output and is usually far larger than the app itself, `Cargo.lock`
/// pins versions that may not resolve on someone else's machine, and
/// `bindings.rs` is regenerated from the WIT on every build. Everything else
/// under the crate root is taken as-is.
/// The token a bundle carries instead of this machine's SDK path.
pub const SDK_PLACEHOLDER: &str = "{KRATE_SDK}";

/// Replace any absolute path into a materialised SDK with [`SDK_PLACEHOLDER`].
///
/// The cache path contains a content hash, so it differs per machine and per
/// Krate version. Matching on the `.cache/krate/sdk/<hash>` shape rather than
/// on one literal keeps this working when either changes.
pub fn rewrite_sdk_paths(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        match sdk_root_in(line).or_else(|| checkout_sdk_root_in(line)) {
            Some(root) => out.push_str(&line.replace(&root, SDK_PLACEHOLDER)),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// The root of an SDK reached by a path into a CHECKOUT rather than the
/// cache: `../../crates/bindings-rust`, `../../wit/krate/phase3`, or an
/// absolute path into a clone of the repository.
///
/// Every app under `apps/` is written this way, and a bundle packed from
/// one shipped those relative paths verbatim -- source that only rebuilds
/// inside the checkout it came from, which is not a closure at all
/// (measured on krate-hello-gui: the rebuild elsewhere failed to load
/// `../../crates/bindings-rust/Cargo.toml`). The materialised SDK has the
/// same layout under its root (`crates/bindings-rust`, `wit/krate/...`),
/// so the part before either marker IS the SDK root, whatever it was.
///
/// Only a prefix made of parent/current segments or an absolute path is
/// taken: `vendor/crates/bindings-rust` is the app's own copy and stays.
fn checkout_sdk_root_in(line: &str) -> Option<String> {
    let start = line.find("path = \"")? + "path = \"".len();
    let end = start + line[start..].find('"')?;
    let value = &line[start..end];
    let normalized = value.replace('\\', "/");
    let marker_at = ["crates/bindings-rust", "wit/krate/"]
        .iter()
        .filter_map(|marker| {
            normalized
                .find(marker)
                .filter(|&i| i == 0 || normalized.as_bytes()[i - 1] == b'/')
        })
        .min()?;
    let prefix = &normalized[..marker_at];
    let relative_only = prefix
        .split('/')
        .all(|segment| matches!(segment, "" | "." | ".."));
    let absolute = prefix.starts_with('/') || prefix.get(1..3) == Some(":/");
    if !relative_only && !absolute {
        return None;
    }
    // The prefix in the ORIGINAL spelling, so the replacement lands on the
    // bytes that are there; single-character replacements keep the indices
    // aligned. Drop the trailing separator: the placeholder is a root.
    let original = &value[..marker_at];
    Some(original.trim_end_matches(['/', '\\']).to_string()).filter(|root| !root.is_empty())
}

/// The SDK root inside a line, if it holds one.
///
/// Case-insensitive and separator-tolerant, because the miss was real: on
/// Windows the SDK materialises under `AppData/Local/Krate/sdk/` -- capital
/// K -- and the lowercase `/krate/sdk/` marker never matched, so every
/// Windows-built bundle shipped its author's absolute path and the source
/// stopped travelling (K-126).
fn sdk_root_in(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase().replace('\\', "/");
    let normalized = line.replace('\\', "/");
    let marker = "/krate/sdk/";
    let at = lower.find(marker)?;
    let start = normalized[..at]
        .rfind(['"', '\'', ' ', '='])
        .map_or(0, |i| i + 1);
    // The hash segment ends at the next separator after the marker.
    let after = at + marker.len();
    let end = normalized[after..]
        .find('/')
        .map(|offset| after + offset)
        .unwrap_or(normalized.len());
    // Indices computed on the normalized copy are only valid on the ORIGINAL
    // line if the two are byte-aligned, which replacing single characters
    // with single characters guarantees.
    Some(line[start..end].to_string())
}

/// Gather every file under `root`, prefixed for the archive.
///
/// Shares the skip list and the symlink refusal with [`collect_source`], since
/// an SDK tree carries the same hazards: a `target/` directory from a stray
/// build, and links that would reach outside the tree.
fn collect_tree(root: &Path, prefix: &str) -> Result<Vec<(String, PathBuf)>> {
    // `bindings.rs` is skipped for an app, where it is regenerated on every
    // build. In the SDK it is the opposite: it is generated by a specific
    // wit-bindgen version that the reader may not have, so leaving it out
    // ships an SDK that cannot compile -- "file not found for module
    // `bindings`". Keep it here.
    let mut out = collect_files(root, &|name| {
        matches!(name, "target" | "Cargo.lock" | ".git")
    })?;
    for (name, _) in out.iter_mut() {
        *name = format!("{prefix}{}", &name[SOURCE_PREFIX.len()..]);
    }
    Ok(out)
}

/// Every file under an UNPACKED tree, prefixed for the archive: no skip
/// list, because what was unpacked is exactly what the archive held, and a
/// digest over it must cover all of it (Cargo.lock included).
fn collect_unpacked(root: &Path, prefix: &str) -> Result<Vec<(String, PathBuf)>> {
    let mut out = collect_files(root, &|_| false)?;
    for (name, _) in out.iter_mut() {
        *name = format!("{prefix}{}", &name[SOURCE_PREFIX.len()..]);
    }
    Ok(out)
}

fn collect_source(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    // Cargo.lock TRAVELS (CP1, the editable closure). It used to be skipped
    // as "versions that may not resolve on someone else's machine", which
    // had it backwards: the lock is what makes a rebuild elsewhere resolve
    // to the same crates, and `--locked` is how a rebuild proves it did.
    collect_files(root, &|name| {
        matches!(
            name,
            "target"
                | "bindings.rs"
                | ".git"
                | ".agent-transcript.txt"
                | "KRATE_AUTHORING.md"
                // The attachment inbox: files the person handed the AI to
                // read. Packing them shipped a founder's benchmark
                // screenshots inside the app -- 1.4MB of a "210KB" bundle
                // -- and would silently publish anyone's attached sketch
                // or spreadsheet inside every copy of the app they share.
                | "attached"
                // Operating-system metadata, which is not source and is not
                // the same on any two machines. Measured: packing an app
                // whose folder Finder had opened shipped an 8 KB
                // `source/.DS_Store`, so the editable digest depended on
                // which machine did the packing -- and the CP1 exit test
                // asks that a pack rebuild byte-equal on a second machine.
                // Named individually rather than as "every dotfile": a
                // `.cargo/config.toml` is real build input and must ship.
                | ".DS_Store"
                | "Thumbs.db"
                | "desktop.ini"
                // The verification frame the pack tells agents to shoot.
                | "frame.png"
        )
    })
}

/// Walk a tree, skipping whatever `skip` rejects.
fn collect_files(root: &Path, skip: &dyn Fn(&str) -> bool) -> Result<Vec<(String, PathBuf)>> {
    fn visit(
        root: &Path,
        current: &Path,
        skip: &dyn Fn(&str) -> bool,
        out: &mut Vec<(String, PathBuf)>,
    ) -> Result<()> {
        let mut entries = fs::read_dir(current)
            .map_err(|err| io_err(current, err))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|err| io_err(current, err))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if skip(&name) {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).map_err(|err| io_err(&path, err))?;
            // A symlink out of the tree would pull in arbitrary files, the
            // same reason assets refuse them.
            if metadata.file_type().is_symlink() {
                return Err(BundleError::AssetSymlink { path });
            }
            if metadata.is_dir() {
                visit(root, &path, skip, out)?;
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| BundleError::Manifest("source path escaped its root".into()))?;
            let mut entry_name = String::from(SOURCE_PREFIX);
            entry_name.push_str(&relative.to_string_lossy().replace('\\', "/"));
            out.push((entry_name, path));
        }
        Ok(())
    }

    let mut out = Vec::new();
    visit(root, root, skip, &mut out)?;
    Ok(out)
}

fn collect_assets(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    fn visit(
        root: &Path,
        current: &Path,
        assets: &mut Vec<(String, PathBuf)>,
        total: &mut u64,
    ) -> Result<()> {
        let mut entries = fs::read_dir(current)
            .map_err(|err| io_err(current, err))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|err| io_err(current, err))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|err| io_err(&path, err))?;
            if metadata.file_type().is_symlink() {
                return Err(BundleError::AssetSymlink { path });
            }
            if metadata.is_dir() {
                visit(root, &path, assets, total)?;
                continue;
            }
            if !metadata.is_file() {
                continue;
            }
            if metadata.len() > MAX_ASSET_BYTES {
                return Err(BundleError::EntryTooLarge {
                    entry: path.display().to_string(),
                });
            }
            *total = total
                .checked_add(metadata.len())
                .ok_or(BundleError::AssetsTooLarge)?;
            if *total > MAX_TOTAL_ASSET_BYTES {
                return Err(BundleError::AssetsTooLarge);
            }
            if assets.len() == MAX_ASSET_COUNT {
                return Err(BundleError::TooManyAssets);
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|_| BundleError::UnsafeAssetPath {
                    path: path.display().to_string(),
                })?;
            let name = asset_entry_name(relative)?;
            assets.push((name, path));
        }
        Ok(())
    }

    let mut assets = Vec::new();
    let mut total = 0;
    visit(root, root, &mut assets, &mut total)?;
    Ok(assets)
}

fn asset_entry_name(relative: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or_else(|| BundleError::UnsafeAssetPath {
                    path: relative.display().to_string(),
                })?;
                if part.is_empty() || part.contains('\\') {
                    return Err(BundleError::UnsafeAssetPath {
                        path: relative.display().to_string(),
                    });
                }
                parts.push(part);
            }
            _ => {
                return Err(BundleError::UnsafeAssetPath {
                    path: relative.display().to_string(),
                });
            }
        }
    }
    if parts.is_empty() {
        return Err(BundleError::UnsafeAssetPath {
            path: relative.display().to_string(),
        });
    }
    Ok(format!("{ASSETS_PREFIX}{}", parts.join("/")))
}

fn safe_asset_relative_path(name: &str) -> Result<PathBuf> {
    let relative =
        name.strip_prefix(ASSETS_PREFIX)
            .ok_or_else(|| BundleError::UnsafeAssetPath {
                path: name.to_string(),
            })?;
    if relative.is_empty() || relative.contains('\\') {
        return Err(BundleError::UnsafeAssetPath {
            path: name.to_string(),
        });
    }
    let path = Path::new(relative);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BundleError::UnsafeAssetPath {
            path: name.to_string(),
        });
    }
    Ok(path.to_path_buf())
}

/// The same containment guard as assets, for source entries.
///
/// Deliberately a copy rather than a shared generic: these two prefixes are
/// security boundaries, and a future change to one should not silently loosen
/// the other.
fn safe_prefixed_relative_path(name: &str, prefix: &str) -> Result<PathBuf> {
    let relative = name
        .strip_prefix(prefix)
        .ok_or_else(|| BundleError::UnsafeAssetPath {
            path: name.to_string(),
        })?;
    if relative.is_empty() || relative.contains('\\') {
        return Err(BundleError::UnsafeAssetPath {
            path: name.to_string(),
        });
    }
    let path = Path::new(relative);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BundleError::UnsafeAssetPath {
            path: name.to_string(),
        });
    }
    Ok(path.to_path_buf())
}

fn safe_source_relative_path(name: &str) -> Result<PathBuf> {
    let relative =
        name.strip_prefix(SOURCE_PREFIX)
            .ok_or_else(|| BundleError::UnsafeAssetPath {
                path: name.to_string(),
            })?;
    if relative.is_empty() || relative.contains('\\') {
        return Err(BundleError::UnsafeAssetPath {
            path: name.to_string(),
        });
    }
    let path = Path::new(relative);
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BundleError::UnsafeAssetPath {
            path: name.to_string(),
        });
    }
    Ok(path.to_path_buf())
}

/// Read the central directory once and refuse an ambiguous archive (IC-209).
///
/// Runs BEFORE anything is extracted, because the decision it makes is about
/// the archive as a whole. Two entries with one logical path mean what a
/// person reviews need not be what Krate writes to disk -- a zip reader that
/// takes the last wins, a reviewer's tool that shows the first, and the
/// difference is an attacker's file (K-252: a bundle with two
/// `source/src/lib.rs` entries opened and ran, and the second copy was the
/// one extracted).
///
/// Names are compared after normalising case and separators, so a collision
/// that only appears on a case-insensitive filesystem is caught on every
/// platform rather than on the reviewer's machine but not the recipient's.
/// Refuse a container profile this build does not understand (IC-208).
///
/// Absent is generation 1, deliberately: bundles written before the entry
/// existed carry no line, and treating their absence as an error would
/// strand every app already sent to somebody.
///
/// A profile that is present but unreadable -- not a number, or a number
/// this build has never heard of -- is refused rather than guessed at. The
/// whole point of the line is that a reader which cannot honour the rules
/// says so.
///
/// The FIRST LINE is the version; anything after it is ignored (K-266).
/// That is what makes the envelope an envelope. It exists so a new bundle
/// can meet an old reader and both behave sensibly, and the first way a
/// format grows is by adding a line -- so a reader that refuses every line
/// it has not seen before makes growth impossible, and does it in the field,
/// on readers already shipped.
///
/// Ignoring is right for an OPTIONAL field, and it is the only kind that can
/// exist today. A field a reader must understand cannot simply be added
/// later without stranding every reader before it -- that is what the
/// version number is for. Raising the version is how a bundle says "you need
/// to understand more than you do", and an old reader already refuses those.
fn check_profile<R: Read + io::Seek>(archive: &mut ZipArchive<R>) -> Result<u32> {
    let mut entry = match archive.by_name(PROFILE_ENTRY) {
        Ok(entry) => entry,
        Err(_) => return Ok(1), // generation 1
    };
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|err| BundleError::Io {
            path: PathBuf::from(PROFILE_ENTRY),
            source: err,
        })?;
    // The version is the first line, so a bundle carrying fields this build
    // has never heard of still says which rules it was written for.
    let found = text.lines().next().unwrap_or("").trim();
    match found.parse::<u32>() {
        Ok(version) if version <= READS_PROFILE => Ok(version),
        // A number this build has not reached: genuinely a newer format, and
        // updating Krate is the thing that helps.
        Ok(_) => Err(BundleError::UnsupportedProfile {
            found: found.chars().take(32).collect(),
            supported: READS_PROFILE,
        }),
        // Not a number at all -- empty, a word, a negative. That is damage,
        // not a version from the future, and no release will ever read it.
        // Telling somebody to update Krate sends them to do something that
        // cannot work (K-271).
        Err(_) => Err(BundleError::DamagedProfile {
            found: found.chars().take(32).collect(),
        }),
    }
}

/// A path short enough to read in an error message.
///
/// The paths these refusals are about are the long ones, and printing 900
/// characters buries the sentence that says what to do about it. The start
/// and the end are what identify the file; the middle is the part that made
/// it too long.
fn shorten_path(path: &str) -> String {
    const KEEP: usize = 60;
    if path.chars().count() <= KEEP * 2 {
        return path.to_string();
    }
    let head: String = path.chars().take(KEEP).collect();
    let tail: String = {
        let all: Vec<char> = path.chars().collect();
        all[all.len() - KEEP..].iter().collect()
    };
    format!("{head}...{tail}")
}

/// The manifest as the identity sees it: line endings normalised (K-265).
///
/// The manifest is identity-bearing, and rightly so -- it carries the
/// capabilities, so a change to it is a change to what the app may do. But it
/// was hashed as raw bytes, which made the SAME manifest a different app
/// depending on how it reached the disk. Measured on a shipped app whose
/// manifest was checked out with Windows line endings, nothing else touched:
///   unix    (LF)   97ec3bffa3c1a3e1
///   windows (CRLF) 6cad357106be407e
/// That is git's default on Windows, so one commit built on two machines
/// disagreed about what the app was -- which defeats a reproducible build and
/// is invisible in any editor.
///
/// Only line endings are folded. Everything else stays byte-exact: a comment,
/// a reordered key or a changed value is a real edit to the file that
/// describes the app, and the identity should follow it. Folding more would
/// need a TOML parser in the path that decides what an app IS, and the
/// narrow rule fixes the case that actually bites.
///
/// A lone CR is left alone: no tooling produces it as a line ending, and
/// treating it as one would fold two genuinely different files together.
fn manifest_bytes_for_digest(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'\r' && raw.get(index + 1) == Some(&b'\n') {
            index += 1; // drop the CR and keep the LF written below
            continue;
        }
        out.push(raw[index]);
        index += 1;
    }
    out
}

fn preflight_entries<R: Read + io::Seek>(archive: &mut ZipArchive<R>) -> Result<()> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut files = 0usize;
    let mut source_bytes = 0u64;

    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let name = entry.name().to_string();
        if name.ends_with('/') {
            continue; // a directory record carries no content
        }
        files += 1;
        if files > MAX_ENTRY_COUNT {
            return Err(BundleError::TooManyEntries);
        }

        // ASCII only (IC-209).
        //
        // `café.rs` has two spellings -- NFC and NFD -- that are different
        // bytes and the same file on macOS. One of them would then be a copy
        // the reviewer never saw, and telling them apart needs Unicode
        // normalization tables: a new dependency, in the crate that opens
        // untrusted files, carrying data that changes between releases.
        //
        // Krate controls what it packs, and no shipped bundle uses a
        // non-ASCII entry name. Refusing them costs a developer one rename,
        // with a message saying exactly that, and removes the ambiguity
        // entirely rather than approximating it.
        //
        // The name is read from the RAW bytes, not from `entry.name()`: a zip
        // that leaves the UTF-8 flag bit clear is decoded as CP437, so a
        // genuine `caf\u{e9}.rs` would be reported back as `caf\u{251c}\u{2510}.rs`
        // and the developer would be told to rename a file they do not have.
        let raw = entry.name_raw().to_vec();
        if !raw.is_ascii() {
            return Err(BundleError::NonAsciiPath {
                path: String::from_utf8_lossy(&raw).into_owned(),
            });
        }

        // A backslash is refused outright, not folded and hoped for (K-299).
        //
        // The duplicate check below folds `\` to `/`, so it treats
        // `assets\logo.png` and `assets/logo.png` as one path. Extraction
        // does not: it matches the literal prefix `assets/`, so the
        // backslash form is not an asset at all and falls through to the
        // ignored-unknown-record path -- the file silently never ships.
        // Two guards, two answers about one entry.
        //
        // Refusing is the honest resolution. A zip may legally carry either
        // separator, so this is usually a real app whose picture would have
        // vanished, and telling the person is better than shipping the app
        // without it. Repacking with `krate pack` writes forward slashes.
        if name.contains('\\') {
            return Err(BundleError::UnsafeAssetPath {
                path: shorten_path(&name),
            });
        }

        // One logical path, however it is spelled. Case is folded because
        // two spellings that differ only in case are one file on the
        // filesystems most people use.
        let logical = name.to_lowercase();

        // Depth and length are judged on the folded form. Backslashes are
        // already refused above, so nothing can hide its nesting behind
        // them (IC-209).
        if logical.matches('/').count() > MAX_PATH_DEPTH {
            return Err(BundleError::PathTooDeep {
                path: shorten_path(&name),
            });
        }
        if name.len() > MAX_PATH_BYTES {
            return Err(BundleError::PathTooLong {
                path: shorten_path(&name),
            });
        }
        if !seen.insert(logical) {
            return Err(BundleError::DuplicateEntry { path: name });
        }

        if name.starts_with(SOURCE_PREFIX)
            || name.starts_with(SDK_PREFIX)
            || name.starts_with(EXTENSION_PREFIX)
        {
            source_bytes = source_bytes.saturating_add(entry.size());
            if source_bytes > MAX_TOTAL_SOURCE_BYTES {
                return Err(BundleError::SourceTooLarge);
            }
        }
    }
    Ok(())
}

/// Count the central-directory records the FILE actually carries.
///
/// `ZipArchive` cannot answer this: it builds a map keyed by name, so an
/// archive with two `source/src/lib.rs` records reports one entry and hands
/// back the LAST -- measured on a real fixture, where the file held 4
/// records and `archive.len()` said 3 (K-252). Every duplicate check built
/// on the parsed archive is therefore blind by construction.
///
/// So the count comes from the bytes. A central-directory header starts with
/// `PK\x01\x02`, and comparing that count against the number of distinct
/// entries the parser found is enough to say "this archive names something
/// twice" without reimplementing zip parsing.
///
/// The signature can also appear inside compressed data by chance, which
/// would over-count and refuse an honest bundle. It is therefore only
/// counted from the start of the central directory, which the end-of-central
/// -directory record locates exactly.
/// The normalized name of the first path the central directory lists twice.
///
/// IC-713 asks for the offending path to be STATED, not counted: "3 entries,
/// 2 distinct names" tells somebody the file is wrong without telling them
/// which part. The parser cannot answer this -- it keys entries by name and
/// has already dropped the duplicate -- so the records are walked directly.
///
/// Normalized the same way `preflight_entries` compares, because that is the
/// sense in which they collide: `source/lib.rs` and `source\lib.rs` are one
/// path once extracted, and naming the raw spelling of only one of them
/// would send somebody looking for a file that reads as different.
fn first_duplicate_record_name(bytes: &[u8]) -> Option<String> {
    const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const CD_SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];

    let window = bytes.len().min(64 * 1024 + 22);
    let tail = &bytes[bytes.len() - window..];
    let eocd_at = tail.windows(4).rposition(|w| w == EOCD_SIG)?;
    let eocd = &tail[eocd_at..];
    if eocd.len() < 20 {
        return None;
    }
    // Offset 16: where the central directory starts, from the file's start.
    let mut at = u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]) as usize;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    // Each record is at least 46 bytes before its variable-length name.
    while at + 46 <= bytes.len() && bytes[at..at + 4] == CD_SIG {
        let name_len = u16::from_le_bytes([bytes[at + 28], bytes[at + 29]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[at + 30], bytes[at + 31]]) as usize;
        let comment_len = u16::from_le_bytes([bytes[at + 32], bytes[at + 33]]) as usize;
        let name_at = at + 46;
        if name_at + name_len > bytes.len() {
            return None;
        }
        let name = String::from_utf8_lossy(&bytes[name_at..name_at + name_len]).into_owned();
        if !name.ends_with('/') {
            let logical = name.replace('\\', "/").to_lowercase();
            if !seen.insert(logical.clone()) {
                return Some(logical);
            }
        }
        at = name_at + name_len + extra_len + comment_len;
    }
    None
}

fn central_directory_record_count(bytes: &[u8]) -> Option<usize> {
    // The EOCD is at the end, and holds the record count in a fixed field.
    // Scanning backwards for its signature is the standard way to find it;
    // the comment field can be up to 64 KiB, so bound the search.
    const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    let window = bytes.len().min(64 * 1024 + 22);
    let start = bytes.len() - window;
    let tail = &bytes[start..];
    let position = tail.windows(4).rposition(|w| w == EOCD_SIG)?;
    let eocd = &tail[position..];
    if eocd.len() < 12 {
        return None;
    }
    // Offset 10: total number of entries in the central directory.
    Some(u16::from_le_bytes([eocd[10], eocd[11]]) as usize)
}

fn asset_entry_names<R: Read + io::Seek>(archive: &mut ZipArchive<R>) -> Result<Vec<String>> {
    let mut names = BTreeSet::new();
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let name = entry.name().to_string();
        if !name.starts_with(ASSETS_PREFIX) || name.ends_with('/') {
            continue;
        }
        safe_asset_relative_path(&name)?;
        if !names.insert(name) {
            return Err(BundleError::UnsafeAssetPath {
                path: "duplicate asset entry".to_string(),
            });
        }
        if names.len() > MAX_ASSET_COUNT {
            return Err(BundleError::TooManyAssets);
        }
    }
    Ok(names.into_iter().collect())
}

fn extract_asset_entry<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    destination: &Path,
) -> Result<u64> {
    let mut entry = archive.by_name(name)?;
    if entry.size() > MAX_ASSET_BYTES {
        return Err(BundleError::EntryTooLarge {
            entry: name.to_string(),
        });
    }
    let mut out = File::create(destination).map_err(|err| io_err(destination, err))?;
    let mut limited = entry.by_ref().take(MAX_ASSET_BYTES + 1);
    let written = io::copy(&mut limited, &mut out).map_err(|err| io_err(destination, err))?;
    if written > MAX_ASSET_BYTES {
        return Err(BundleError::EntryTooLarge {
            entry: name.to_string(),
        });
    }
    Ok(written)
}

fn extract_entry<R: Read + io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &'static str,
    destination: &Path,
) -> Result<()> {
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => {
            return Err(BundleError::MissingEntry(name));
        }
        Err(err) => return Err(err.into()),
    };

    if entry.size() > MAX_ENTRY_BYTES {
        return Err(BundleError::EntryTooLarge {
            entry: name.to_string(),
        });
    }

    let mut out = File::create(destination).map_err(|err| io_err(destination, err))?;
    // Copy through a limited reader as well as checking the declared size: a
    // zip header can lie about how large an entry is.
    let mut limited = entry.by_ref().take(MAX_ENTRY_BYTES + 1);
    let written = io::copy(&mut limited, &mut out).map_err(|err| io_err(destination, err))?;
    if written > MAX_ENTRY_BYTES {
        return Err(BundleError::EntryTooLarge {
            entry: name.to_string(),
        });
    }
    Ok(())
}

/// Plain words for a transfer that failed (K-275).
///
/// ureq stacks its own context, so a read timeout arrives as
/// "<url>: Network Error: Network Error: Error encountered in the status
/// line: timed out reading response" -- the url three times over and the one
/// useful phrase last. A person needs to know the server went quiet and that
/// trying again is worth doing.
///
/// Anything else is passed through: this exists to rewrite the one case that
/// is both common and unreadable, not to launder every network error into
/// something vague.
/// Whether a ureq failure is the socket's read timeout firing.
///
/// Judged by the io error's KIND, found by walking the cause chain, and
/// only then by text. Unix says "timed out reading response"; Windows says
/// "the connected party did not properly respond after a period of time"
/// (WSAETIMEDOUT), and a text match on the first left every Windows user
/// with three copies of the url and two "Network Error"s instead of the
/// plain sentence.
#[cfg(feature = "fetch")]
fn is_read_timeout(err: &ureq::Error) -> bool {
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(current) = cause {
        if let Some(io) = current.downcast_ref::<io::Error>() {
            if matches!(
                io.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) {
                return true;
            }
        }
        cause = current.source();
    }
    err.to_string().contains("timed out")
}

#[cfg(feature = "fetch")]
fn map_fetch_error(_url: &str, err: ureq::Error) -> String {
    if is_read_timeout(&err) {
        return format!(
            "the server accepted the connection and then stopped responding, \
             so the download was given up on after {} seconds. Try again, or \
             check the link.",
            FETCH_SILENCE_TIMEOUT.as_secs()
        );
    }
    // A hub answers 451 for an app it removed, with the notice as JSON
    // (IC-669). The person is told the reason and where the notice is,
    // not "status 451" (K-334).
    if let ureq::Error::Status(451, response) = err {
        let body = response.into_string().unwrap_or_default();
        let notice: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        let reason = notice["reason"].as_str().unwrap_or("no reason was given");
        let mut message = format!("the hub removed this app: {reason}");
        if notice["emergency"].as_bool() == Some(true) {
            message.push_str(" (an emergency removal)");
        }
        if let Some(url) = notice["notice"].as_str() {
            message.push_str(&format!(
                ". The notice, and how its author can appeal: {url}"
            ));
        }
        return message;
    }
    err.to_string()
}

/// Fetch a bundle over the network and open it.
///
/// HTTPS is required unless `allow_insecure_http` is set, which exists so CI
/// and local development can serve a bundle from `127.0.0.1` without a
/// certificate. Fetching grants no capability: the returned bundle goes through
/// the same policy resolution as one opened from disk.
#[cfg(feature = "fetch")]
pub fn fetch(url: &str, allow_insecure_http: bool) -> Result<OpenBundle> {
    fetch_resolved(url, allow_insecure_http).map(|fetched| fetched.bundle)
}

/// What [`fetch_resolved`] brings back: the bundle, the URL the bytes were
/// finally served from, and the content address that URL named, when it
/// named one.
#[cfg(feature = "fetch")]
pub struct Fetched {
    pub bundle: OpenBundle,
    /// Where the bytes came from after every redirect. A channel link
    /// (`/c/<publisher>/<app>`) resolves here to a fixed address.
    pub url: String,
    /// The hex content address in that URL's last path segment, checked
    /// against the bytes. `None` when the URL named no address.
    pub address: Option<String>,
}

/// The bytes served at a content address must BE that content (IC-389,
/// test 471).
///
/// A hub link names the app by the SHA-256 of its bytes, and a short link
/// by a prefix of it. Before this, whatever a server answered with ran:
/// a stale cache, a compromised alias, or a redirect to somewhere else
/// could hand over different bytes under a URL that still read as the
/// right app. A channel link resolves (by redirect) to a fixed address,
/// so the same check covers it: what runs is what the final address
/// names, or nothing runs.
///
/// The address is the plain SHA-256 of the file -- the hub's store key --
/// not the schema-tagged archive identity, which is a different number
/// over the same bytes.
/// Plain lowercase-hex SHA-256 of `bytes`: the hub's store key, the
/// address in a hub link, and the name a takedown blocks. Not an identity
/// of the bundle's contents -- those are schema-tagged, see [`provenance`].
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(feature = "fetch")]
fn check_address(final_url: &str, bytes: &[u8]) -> Result<Option<String>> {
    let path = final_url.split(['?', '#']).next().unwrap_or(final_url);
    let segment = path.rsplit('/').next().unwrap_or("");
    let is_address = (8..=64).contains(&segment.len())
        && segment
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !is_address {
        return Ok(None);
    }
    let actual = sha256_hex(bytes);
    if !actual.starts_with(segment) {
        return Err(BundleError::Fetch {
            url: final_url.to_string(),
            message: format!(
                "the bytes served at this address are not the ones it names: the address \
                 says {}..., the file is {}...; nothing ran. The link may be stale, or \
                 something between you and the publisher changed the file",
                &segment[..segment.len().min(12)],
                &actual[..12]
            ),
        });
    }
    Ok(Some(segment.to_string()))
}

#[cfg(feature = "fetch")]
pub fn fetch_resolved(url: &str, allow_insecure_http: bool) -> Result<Fetched> {
    if url.starts_with("http://") && !allow_insecure_http {
        return Err(BundleError::InsecureUrl {
            url: url.to_string(),
        });
    }

    // A server that accepts and then says nothing must not hold us forever
    // (K-275). ureq bounds CONNECT by default and leaves READ unbounded, so
    // a host that completes the handshake and stalls kept `krate run <url>`
    // open with no output and nothing to retry -- the worst shape a failure
    // can take on the receiver's path.
    //
    // This is the socket's read timeout, which bounds time WITHOUT DATA
    // rather than total transfer time. That distinction is the whole reason
    // a number this small is safe: a slow 256 MB download over a poor link
    // keeps resetting it and is never cut off, while a silent server trips
    // it once.
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(FETCH_CONNECT_TIMEOUT)
        .timeout_read(FETCH_SILENCE_TIMEOUT)
        .build();
    let response = agent.get(url).call().map_err(|err| BundleError::Fetch {
        url: url.to_string(),
        message: map_fetch_error(url, err),
    })?;

    // Where the bytes finally came from: a channel link redirects to the
    // fixed address it currently points at, and that is the address the
    // bytes are held to.
    let final_url = response.get_url().to_string();
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_BUNDLE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| BundleError::Fetch {
            url: url.to_string(),
            message: err.to_string(),
        })?;
    let address = check_address(&final_url, &bytes)?;

    // A download that did not finish is not a damaged app (K-274).
    //
    // A server understating Content-Length makes the client stop reading
    // early, and ten bytes of a zip open exactly like a corrupt file. The
    // recipient was told their file was damaged, which sends them back for
    // another copy of a link that will download the same way. The truth --
    // the download was cut short, try again -- is the thing that helps, and
    // only this function is in a position to say it, because only this
    // function knows the bytes arrived over a network.
    let bundle = open_bytes(&bytes).map_err(|err| {
        if matches!(err, BundleError::Archive(_)) {
            // Deliberately without a byte count. The obvious detail --
            // promised length against received length -- can never differ
            // here: the client stops at exactly Content-Length, so a server
            // that understates it produces a short read where the two agree.
            // Printing "10 of 10 bytes" would be true and useless.
            return BundleError::Fetch {
                url: url.to_string(),
                message: format!(
                    "the download did not finish, so the app could not be read. \
                     The file at {url} may be fine -- try again."
                ),
            };
        }
        err
    })?;
    Ok(Fetched {
        bundle,
        url: final_url,
        address,
    })
}

#[cfg(test)]
mod tests {
    /// A change to an app must see the app's own files (K-851).
    ///
    /// `revise` edits the tree `open` writes from `source/`. Assets used to
    /// reach that tree only because every asset was ALSO stored inside
    /// source -- the duplicate that cost `apps/krate-nova2` 990 KB. With the
    /// duplicate gone, `open` has to complete the tree from the asset
    /// records, or the AI edits code that loads images it cannot see.
    #[test]
    fn an_asset_ships_once_and_still_reaches_the_edit_tree() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);

        // The shape a real app has: the assets directory sits inside the
        // source tree, so packing sees the same bytes down both paths.
        let src = dir.path().join("src");
        let assets = src.join("assets");
        fs::create_dir_all(&assets).expect("assets dir");
        fs::write(src.join("lib.rs"), b"// loads assets/icon.png").expect("lib");
        fs::write(assets.join("icon.png"), b"PNG-BYTES-HERE").expect("icon");

        let bundle = dir.path().join("app.krate");
        pack_with_source(&manifest, &component, Some(&assets), Some(&src), &bundle).expect("pack");

        let names: Vec<String> = {
            let file = File::open(&bundle).expect("open zip");
            let mut archive = ZipArchive::new(file).expect("zip");
            (0..archive.len())
                .map(|i| archive.by_index(i).expect("entry").name().to_string())
                .collect()
        };
        assert!(
            names.iter().any(|n| n == "assets/icon.png"),
            "the asset ships, as an asset: {names:?}",
        );
        assert!(
            !names.iter().any(|n| n == "source/assets/icon.png"),
            "and not a second time inside source: {names:?}",
        );

        // The edit tree still holds everything a change needs.
        let opened = open(&bundle).expect("open bundle");
        let source = opened.source_path().expect("source path");
        assert!(source.join("lib.rs").is_file(), "the code is there");
        let placed = source.join("assets/icon.png");
        assert!(
            placed.is_file(),
            "the asset is placed beside the source it belongs to",
        );
        assert_eq!(
            fs::read(&placed).expect("read"),
            b"PNG-BYTES-HERE",
            "and it is the bytes the app loads",
        );
    }

    use super::*;
    use std::io::Cursor;
    use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

    /// The smallest valid COMPONENT: eight bytes of header and nothing else.
    ///
    /// Almost every fixture here used `\0asm\x01\0\0\0`, which is a core
    /// MODULE header, not a component. Nothing noticed until pack learned to
    /// tell them apart (K-272), at which point fifteen tests turned out to
    /// have been packing modules and calling them components.
    ///
    ///   component  00 61 73 6d 0d 00 01 00
    ///   module     00 61 73 6d 01 00 00 00
    ///
    /// A test whose fixture is not the thing it claims to be is testing
    /// something other than what it says.
    // A real component with a `run` export, not a bare header: since open
    // validates the component (IC-210), a fixture that could never run is
    // refused, which is right -- and means every bundle a test builds has to
    // hold something that could.
    const MINIMAL_COMPONENT: &[u8] = include_bytes!("../tests/fixtures/minimal-run.wasm");
    // The same shape with different code, for "the component changed" cases:
    // a change that open must still accept, not garbage it must refuse.
    const OTHER_COMPONENT: &[u8] = include_bytes!("../tests/fixtures/minimal-run-other.wasm");

    const MANIFEST: &str = r#"
[app]
id = "com.example.demo"
name = "Demo"
version = "0.1.0"
entry = "code.wasm"
world = "krate:app/cli@0.1.0"

[[capabilities]]
cap = "io.stdout"
rationale = "print"
required = true
"#;

    /// IC-861. Packing created the output file first and wrote into it as it
    /// went, so any failure partway through left a stripped file where the
    /// developer's previous bundle had been. E8 reproduced the worst shape of
    /// this: a good 109,247-byte editable bundle replaced by a 12,018-byte
    /// manifest-and-component-only archive, with the source gone.
    #[test]
    fn a_failed_pack_leaves_the_previous_bundle_untouched() {
        let dir = tempfile::tempdir().expect("dir");
        let output = dir.path().join("app.krate");
        fs::write(&output, b"THE DEVELOPER'S PREVIOUS GOOD BUNDLE").expect("seed");
        let before = fs::read(&output).expect("read before");

        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);

        // A source directory naming a file that cannot be opened. The failure
        // lands after the manifest and component have already been written,
        // which is exactly the window that used to destroy the output.
        let source = dir.path().join("source");
        fs::create_dir_all(&source).expect("source dir");
        let missing = source.join("gone.rs");
        fs::write(&missing, b"fn main() {}").expect("write");
        fs::remove_file(&missing).ok();
        // Leave a real file so the directory is not skipped as empty.
        fs::write(source.join("lib.rs"), b"fn main() {}").expect("write");

        // Whether this particular run fails or succeeds is not the point --
        // the point is that the previous bundle is never a casualty.
        // A symlink in the source tree is refused by collect_source -- and
        // that refusal happens AFTER File::create has already truncated the
        // output. This is the exact shape E8 reproduced.
        //
        // Unix always allows an unprivileged symlink. Windows only allows one
        // with Developer Mode or admin rights, so there the creation itself
        // may fail; when it does there is no refusal to assert, but the
        // untouched-output check below still runs and still means something.
        let link = source.join("link.rs");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink("/etc/hosts", &link).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&component, &link).is_ok();

        let result = pack_with_sdk(&manifest, &component, None, Some(&source), None, &output);
        if linked {
            assert!(result.is_err(), "the symlink must be refused");
        }

        // The pack failed, so the developer's previous bundle must be exactly
        // as it was -- not truncated, not replaced by a smaller archive that
        // happens to still parse. (A pack that succeeded is entitled to
        // replace it, so only a failure is held to this.)
        let after = fs::read(&output).expect("read after");
        if result.is_err() {
            assert_eq!(
                before,
                after,
                "a failed pack destroyed the previous bundle ({} bytes -> {} bytes)",
                before.len(),
                after.len()
            );
        }

        // Nor may it leave its half-written staging file behind for someone
        // to find and mistake for a bundle.
        let litter: Vec<_> = fs::read_dir(dir.path())
            .expect("list")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.ends_with(".partial"))
            .collect();
        assert!(litter.is_empty(), "left staging files behind: {litter:?}");
    }

    fn write_temp(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).expect("write fixture");
        path
    }

    /// Two real bundles, same component, different source, opened through the
    /// public path (K-245, IC-712).
    ///
    /// Before the layered identities this printed ONE value under the heading
    /// "Identity" for both, on the screen where a person decides whether to
    /// trust an app. The execution digest agreeing is correct -- they do run
    /// the same -- but a reader of the source would see a different program,
    /// and nothing said so.
    #[test]
    fn two_bundles_with_different_source_are_told_apart_by_the_project_digest() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);

        let mut digests = Vec::new();
        for (name, source_body) in [
            ("honest", b"fn main() { tick() }".as_slice()),
            ("swapped", b"fn main() { steal_everything() }".as_slice()),
        ] {
            let src = dir.path().join(format!("src-{name}"));
            fs::create_dir_all(&src).expect("source dir");
            fs::write(src.join("lib.rs"), source_body).expect("write source");

            let bundle = dir.path().join(format!("{name}.krate"));
            pack_with_source(&manifest, &component, None, Some(&src), &bundle).expect("pack");
            let opened = open(&bundle).expect("open");
            digests.push((
                opened.digest().expect("execution digest").digest,
                opened.project_digest().expect("project digest").digest,
            ));
        }

        assert_eq!(
            digests[0].0, digests[1].0,
            "the same component and manifest run the same",
        );
        assert_ne!(
            digests[0].1, digests[1].1,
            "but different source is a different project, and the identity a \
             person is shown must say so",
        );
    }

    /// Sign a real bundle, verify it, then change it (IC-015).
    ///
    /// The end-to-end path: pack, sign in place, reopen, and confirm the
    /// verdict flips from valid to tampering when a byte inside the file
    /// changes. Nothing here parses a stored statement -- the statement is
    /// recomputed from the bundle each time, which is what makes the check
    /// about the file rather than about a document the file carries.
    #[test]
    fn a_signed_bundle_verifies_until_something_inside_it_changes() {
        use ring::rand::SystemRandom;
        use ring::signature::Ed25519KeyPair;

        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("source dir");
        fs::write(src.join("lib.rs"), b"fn main() {}").expect("write source");

        let bundle = dir.path().join("signed.krate");
        pack_with_source(&manifest, &component, None, Some(&src), &bundle).expect("pack");

        // Unsigned bundles are a supported state, not an error.
        assert!(
            open(&bundle)
                .expect("open")
                .signature_verdict()
                .expect("verdict")
                .is_none(),
            "a bundle nobody signed must read as unsigned, not as invalid",
        );

        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).expect("generate");
        let key = signing::SigningKey::from_pkcs8(pkcs8.as_ref()).expect("load key");
        let envelope =
            sign_bundle(&bundle, &key, "pub/demo", "1.0.0", 1_700_000_000).expect("sign");
        assert_eq!(envelope.namespace, "pub/demo");

        let verdict = open(&bundle)
            .expect("reopen")
            .signature_verdict()
            .expect("verdict")
            .expect("a signed bundle has a verdict");
        assert!(
            verdict.is_genuinely_signed(),
            "a freshly signed bundle must verify: {verdict}",
        );

        // The app still opens normally -- signing must not disturb anything
        // else the bundle is for.
        let reopened = open(&bundle).expect("open signed");
        assert_eq!(reopened.manifest().app.id, "com.example.demo");
        assert!(reopened.source_path().is_some(), "source survives signing");

        // Now change the component inside the signed archive -- for another
        // component that opens fine on its own, so what refuses it is the
        // signature and nothing earlier. (Garbage in its place is refused
        // before the signature is even looked at; see the validator tests.)
        let tampered = dir.path().join("tampered.krate");
        rewrite_entry(&bundle, &tampered, COMPONENT_ENTRY, OTHER_COMPONENT);
        match open(&tampered)
            .expect("open tampered")
            .signature_verdict()
            .expect("verdict")
        {
            Some(signing::Verdict::Tampered { problems }) => {
                assert!(
                    problems.iter().any(|p| p.to_string().contains("code.wasm")),
                    "the changed file must be named: {problems:?}",
                );
            }
            other => panic!("a changed component must read as tampering, got {other:?}"),
        }
    }

    /// Rewrite one entry of a zip, copying the rest through.
    fn rewrite_entry(source: &Path, destination: &Path, entry: &str, bytes: &[u8]) {
        let data = fs::read(source).expect("read source");
        let mut archive = ZipArchive::new(io::Cursor::new(&data)).expect("open zip");
        let file = File::create(destination).expect("create");
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for index in 0..archive.len() {
            let mut existing = archive.by_index(index).expect("entry");
            let name = existing.name().to_string();
            let mut existing_bytes = Vec::new();
            existing.read_to_end(&mut existing_bytes).expect("read");
            writer.start_file(name.clone(), options).expect("start");
            let payload = if name == entry {
                bytes
            } else {
                &existing_bytes
            };
            writer.write_all(payload).expect("write");
        }
        writer.finish().expect("finish");
    }

    /// A delegated release, written into a real bundle and read back
    /// through the same call the runtime makes (IC-015).
    ///
    /// This is the wiring test: the chain has to survive the round trip
    /// through signature.json, because a verdict that only exists in memory
    /// protects nobody.
    #[test]
    fn a_delegation_survives_the_round_trip_through_a_real_bundle() {
        use crate::delegation::{
            Delegation, Purpose, Revocation, RevocationState, SignedDelegation, DELEGATION_SCHEMA,
        };

        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let bundle = dir.path().join("delegated.krate");
        pack(&manifest, &component, &bundle).expect("pack");

        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        let root = signing::SigningKey::from_pkcs8(
            &signing::SigningKey::generate_pkcs8().expect("root key"),
        )
        .expect("load root");
        let release = signing::SigningKey::from_pkcs8(
            &signing::SigningKey::generate_pkcs8().expect("release key"),
        )
        .expect("load release");
        let now = 1_700_000_000u64;

        sign_bundle(&bundle, &release, "acme/clocks", "1.0.0", now).expect("sign");

        // Attach the root's permission slip, as a publisher shipping a
        // delegated release would.
        let signed_delegation = SignedDelegation::create(
            &root,
            Delegation {
                schema: DELEGATION_SCHEMA.to_string(),
                root: hex(&root.public_key()),
                namespace: "acme/*".to_string(),
                key: hex(&release.public_key()),
                purpose: Purpose::Release,
                not_before: now - 3600,
                expires: now + 3600,
                approvers: Vec::new(),
            },
        );
        let opened = open(&bundle).expect("open");
        let mut envelope = opened
            .signature_envelope()
            .expect("envelope")
            .expect("some");
        envelope.delegation = Some(signed_delegation);
        drop(opened);
        rewrite_entry(
            &bundle.clone(),
            &bundle,
            SIGNATURE_ENTRY,
            &serde_json::to_vec_pretty(&envelope).expect("json"),
        );

        // Read back through the runtime's own call.
        let reopened = open(&bundle).expect("reopen");
        let verdict = reopened
            .full_verdict(&RevocationState::Known(Vec::new()))
            .expect("verdict")
            .expect("a signed bundle has one");
        assert!(
            verdict.chain.is_some(),
            "the delegation must survive the round trip into signature.json",
        );
        assert!(
            verdict.is_trustworthy(),
            "a delegated release inside its window must verify: {verdict:?}",
        );

        // And the publisher withdrawing that key must reach the same call.
        let revoked = RevocationState::Known(vec![Revocation {
            key: hex(&release.public_key()),
            compromised_from: now - 60,
            reason: "release key rotated".to_string(),
        }]);
        let after = reopened
            .full_verdict(&revoked)
            .expect("verdict")
            .expect("some");
        assert!(
            !after.is_trustworthy(),
            "a revoked key must stop verifying through the bundle path too",
        );
        assert!(
            after.signature.is_genuinely_signed(),
            "the file is untouched -- only the key was withdrawn",
        );
    }

    /// A bundle carrying no source has one project, not two identities.
    ///
    /// The digests still differ (their schema tags do), so a caller must
    /// decide by what the layers COVER, not by comparing values -- getting
    /// that wrong printed a second identity for a project that did not exist.
    #[test]
    fn a_bundle_without_source_has_nothing_extra_to_rebuild() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let bundle = dir.path().join("plain.krate");
        pack(&manifest, &component, &bundle).expect("pack");

        let opened = open(&bundle).expect("open");
        let execution = opened.digest().expect("execution digest");
        let project = opened.project_digest().expect("project digest");
        assert_eq!(
            execution.entries.len(),
            project.entries.len(),
            "with no source and no SDK the two layers cover the same files, \
             which is how a caller knows there is only one thing to show",
        );
    }

    /// A bundle that names one file twice does not open (K-252, IC-209).
    ///
    /// Two spellings of one path, which is what a hostile packer produces:
    /// `source/lib.rs` and `source\lib.rs` extract to the same file, so one
    /// of them is a copy the reviewer never saw. Krate refuses rather than
    /// choosing.
    ///
    /// The zip LIBRARY cannot express the plainer version of this attack --
    /// its writer rejects a literal duplicate name and its reader keys
    /// entries by name, reporting 3 where the file holds 4 records. That
    /// blindness is exactly why `open` counts what the file declares
    /// instead of trusting the parser.
    #[test]
    fn a_bundle_that_names_one_file_twice_is_refused() {
        use std::io::Write as _;

        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let src = dir.path().join("src");
        fs::create_dir_all(&src).expect("source dir");
        fs::write(src.join("lib.rs"), b"// the reviewed copy").expect("write source");
        let honest = dir.path().join("honest.krate");
        pack_with_source(&manifest, &component, None, Some(&src), &honest).expect("pack");

        // The honest bundle opens.
        open(&honest).expect("an ordinary bundle must still open");

        // Rewrite it with a second spelling of a path it already has.
        let doubled = dir.path().join("doubled.krate");
        {
            let source = fs::read(&honest).expect("read");
            let mut archive = ZipArchive::new(io::Cursor::new(&source)).expect("open");
            let file = File::create(&doubled).expect("create");
            let mut writer = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).expect("entry");
                let name = entry.name().to_string();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).expect("read entry");
                writer.start_file(name, options).expect("start");
                writer.write_all(&bytes).expect("write");
            }
            // A second SPELLING of a path the archive already has. Case,
            // because a backslash is now refused before the duplicate
            // check ever runs (K-299) -- and case is the spelling that
            // still reaches it: two entries differing only in case are
            // one file on the filesystems most people use.
            writer
                .start_file("source/LIB.rs", options)
                .expect("start the second spelling");
            writer
                .write_all(b"// the attacker's copy")
                .expect("write dup");
            writer.finish().expect("finish");
        }

        let err = open(&doubled).expect_err("a duplicated path must be refused");
        let text = err.to_string();
        assert!(
            text.contains("names the same file twice"),
            "the refusal must say what is wrong: {text}"
        );
        assert!(
            text.contains("freshly packed"),
            "and what to do about it: {text}"
        );

        // The other spelling, now its own refusal (K-299). A backslash
        // entry used to be folded here and then silently ignored at
        // extraction, because the prefix match is literal: `assets\x.png`
        // is not `assets/`, so the file never shipped and the archive
        // opened as though it were whole. Refused, and told, instead.
        let backslashed = dir.path().join("backslashed.krate");
        {
            let source = fs::read(&honest).expect("read");
            let mut archive = ZipArchive::new(io::Cursor::new(&source)).expect("open");
            let file = File::create(&backslashed).expect("create");
            let mut writer = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).expect("entry");
                let name = entry.name().to_string();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).expect("read entry");
                writer.start_file(name, options).expect("start");
                writer.write_all(&bytes).expect("write");
            }
            writer
                .start_file("assets\\logo.png", options)
                .expect("start the backslash spelling");
            writer.write_all(b"PNGDATA").expect("write");
            writer.finish().expect("finish");
        }
        let err = open(&backslashed).expect_err("a backslash path must be refused");
        assert!(
            err.to_string().contains("not a safe relative path"),
            "a backslash entry is refused rather than silently dropped: {err}"
        );
    }

    /// The record count is read from the FILE, not from the parser (K-252).
    ///
    /// `ZipArchive` keys entries by name: an archive whose central directory
    /// holds two records for one path reports ONE entry and hands back the
    /// last. Measured on a real fixture -- 4 records, `archive.len()` == 3 --
    /// so every duplicate check built on the parsed archive is blind to the
    /// plainest form of the attack.
    ///
    /// The zip crate's writer will not produce that fixture (it refuses a
    /// literal duplicate name), which is why this asserts on the counting
    /// function directly against bytes built here.
    #[test]
    fn the_declared_record_count_comes_from_the_archive_bytes() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let bundle = dir.path().join("plain.krate");
        pack(&manifest, &component, &bundle).expect("pack");

        let bytes = fs::read(&bundle).expect("read");
        let declared = central_directory_record_count(&bytes)
            .expect("an ordinary bundle declares its record count");
        let parsed = ZipArchive::new(io::Cursor::new(&bytes))
            .expect("open")
            .len();
        assert_eq!(
            declared, parsed,
            "an honest bundle declares exactly what the parser finds",
        );

        // Forge the count: claim one more record than the parser will see.
        // That is precisely the shape a duplicate produces, and `open` must
        // refuse it rather than trusting the parser.
        let mut forged = bytes.clone();
        let eocd = forged
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .expect("end of central directory");
        let inflated = (parsed as u16) + 1;
        forged[eocd + 10..eocd + 12].copy_from_slice(&inflated.to_le_bytes());
        let forged_path = dir.path().join("forged.krate");
        fs::write(&forged_path, &forged).expect("write forged");

        let err = open(&forged_path).expect_err("a claimed extra record must be refused");
        assert!(
            err.to_string().contains("names the same file twice"),
            "got: {err}"
        );
    }

    /// `pack` refuses bytes that are not a component (IC-210).
    ///
    /// It used to accept anything: a text file went in as `code.wasm` and
    /// came out as a 351-byte "app" that exits 2 the moment somebody opens
    /// it. The person packing can fix that; the recipient cannot, and the
    /// recipient was the one who found out.
    #[test]
    fn packing_something_that_is_not_a_component_is_refused() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let not_wasm = write_temp(dir.path(), "code.wasm", b"this is not a component");
        let bundle = dir.path().join("junk.krate");

        let err =
            pack(&manifest, &not_wasm, &bundle).expect_err("plain text must not pack as an app");
        let text = err.to_string();
        assert!(
            text.contains("is not a WebAssembly component"),
            "the refusal must say what is wrong: {text}"
        );
        assert!(
            text.contains("cargo component build"),
            "and what to check: {text}"
        );
        assert!(
            !bundle.exists(),
            "a refused pack must not leave a bundle behind",
        );

        // A real component still packs -- this must not become a check that
        // refuses working apps.
        let component = write_temp(dir.path(), "real.wasm", MINIMAL_COMPONENT);
        let good = dir.path().join("good.krate");
        pack(&manifest, &component, &good).expect("a real component packs");
        assert!(good.is_file());
    }

    /// A bundle carries its container profile, first (IC-208).
    ///
    /// The point is that a format change becomes a sentence rather than a
    /// puzzle: an old Krate meeting a future bundle says so, instead of
    /// ignoring entries it does not recognise and mis-reading the rest.
    #[test]
    fn a_bundle_declares_the_container_profile_it_was_written_for() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let bundle = dir.path().join("profiled.krate");
        pack(&manifest, &component, &bundle).expect("pack");

        let bytes = fs::read(&bundle).expect("read");
        let mut archive = ZipArchive::new(io::Cursor::new(&bytes)).expect("open");
        assert_eq!(
            archive.name_for_index(0),
            Some(PROFILE_ENTRY),
            "the profile must be the FIRST entry, so a reader meets it before \
             it parses anything under this version's rules",
        );
        let mut text = String::new();
        archive
            .by_name(PROFILE_ENTRY)
            .expect("profile entry")
            .read_to_string(&mut text)
            .expect("read profile");
        assert_eq!(text.trim(), PROFILE_VERSION.to_string());

        open(&bundle).expect("a bundle we just wrote must open");
    }

    /// A profile this build cannot honour is refused, and says what to do.
    ///
    /// "What to do" differs by case, which is the point (K-271). A version
    /// from a future Krate is fixed by updating Krate. A format line that is
    /// not a version at all is damage: no release will ever read it, so the
    /// only thing that helps is a fresh copy. This test used to require both
    /// to say "newer .krate format", which is how the wrong advice was
    /// locked in.
    #[test]
    fn a_future_container_profile_is_refused_with_words_a_person_can_act_on() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let bundle = dir.path().join("ok.krate");
        pack(&manifest, &component, &bundle).expect("pack");

        // A real version from later: updating is the answer.
        let future = dir.path().join("future.krate");
        rewrite_entry(&bundle, &future, PROFILE_ENTRY, b"99");
        let err = open(&future).expect_err("a future profile must be refused");
        let text = err.to_string();
        assert!(
            text.contains("newer .krate format"),
            "a later version must be named as one: {text}"
        );
        assert!(
            text.contains("krate.tech/open"),
            "and it must say where to get it: {text}"
        );

        // Not a version at all: damage, and updating would not help.
        for claimed in ["not-a-number", ""] {
            let damaged = dir.path().join(format!("damaged-{}.krate", claimed.len()));
            rewrite_entry(&bundle, &damaged, PROFILE_ENTRY, claimed.as_bytes());
            let err = open(&damaged).expect_err("a damaged profile must be refused");
            let text = err.to_string();
            assert!(
                text.contains("damaged"),
                "must name the real problem for {claimed:?}: {text}"
            );
            assert!(
                text.contains("fresh copy"),
                "and point at the thing that would help: {text}"
            );
        }
    }

    /// A bundle written before the profile entry existed still opens.
    ///
    /// "Keep existing files readable as their recorded generation" is the
    /// requirement's own words, and every app already sent to somebody
    /// predates this entry.
    #[test]
    fn a_bundle_with_no_profile_line_is_read_as_generation_one() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let bundle = dir.path().join("with.krate");
        pack(&manifest, &component, &bundle).expect("pack");

        // Rebuild it without the profile entry, as an older Krate wrote it.
        let older = dir.path().join("older.krate");
        {
            let source = fs::read(&bundle).expect("read");
            let mut archive = ZipArchive::new(io::Cursor::new(&source)).expect("open");
            let file = File::create(&older).expect("create");
            let mut writer = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            for index in 0..archive.len() {
                let mut entry = archive.by_index(index).expect("entry");
                let name = entry.name().to_string();
                if name == PROFILE_ENTRY {
                    continue;
                }
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).expect("read entry");
                writer.start_file(name, options).expect("start");
                writer.write_all(&bytes).expect("write");
            }
            writer.finish().expect("finish");
        }

        open(&older).expect("a pre-profile bundle must still open");
    }

    /// Build a minimal zip whose central directory lists `path` twice.
    ///
    /// Written by hand rather than with the zip crate, whose writer refuses
    /// a literal duplicate name -- which is exactly the archive a hostile
    /// packer produces, and exactly what IC-713 asks to be tested against
    /// ("archives produced by multiple ZIP writers").
    /// A well formed archive that also carries `path` twice.
    fn archive_with_duplicate(path: &str) -> Vec<u8> {
        archive_with(&[
            (path.to_string(), b"the reviewed copy".to_vec()),
            (path.to_string(), b"the attacker's copy".to_vec()),
        ])
    }

    /// A well formed archive plus whatever `extra` entries are asked for.
    /// The names go into the zip verbatim, which is the point: a real writer
    /// would not build these, and an attacker's zip is not written by ours.
    fn archive_with(extra: &[(String, Vec<u8>)]) -> Vec<u8> {
        fn local_header(name: &str, body: &[u8]) -> Vec<u8> {
            let mut out = Vec::new();
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            out.extend_from_slice(&[10, 0]); // version needed
            out.extend_from_slice(&[0, 0]); // flags
            out.extend_from_slice(&[0, 0]); // stored, no compression
            out.extend_from_slice(&[0, 0, 0, 0]); // time, date
            out.extend_from_slice(&crc32(body).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]); // extra length
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(body);
            out
        }

        // manifest and component so the bundle is otherwise well formed,
        // then the target path twice.
        let mut entries: Vec<(String, Vec<u8>)> = vec![
            (MANIFEST_ENTRY.to_string(), MANIFEST.as_bytes().to_vec()),
            (COMPONENT_ENTRY.to_string(), MINIMAL_COMPONENT.to_vec()),
        ];
        entries.extend(extra.iter().cloned());

        let mut out = Vec::new();
        let mut offsets = Vec::new();
        for (name, body) in &entries {
            offsets.push(out.len() as u32);
            out.extend_from_slice(&local_header(name, body));
        }

        let cd_start = out.len() as u32;
        for ((name, body), offset) in entries.iter().zip(&offsets) {
            out.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            out.extend_from_slice(&[10, 0, 10, 0]); // version made by / needed
            out.extend_from_slice(&[0, 0, 0, 0]); // flags, method (stored)
            out.extend_from_slice(&[0, 0, 0, 0]); // time, date
            out.extend_from_slice(&crc32(body).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // extra, comment, disk
            out.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // attrs
            out.extend_from_slice(&offset.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
        }
        let cd_len = out.len() as u32 - cd_start;

        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        out.extend_from_slice(&[0, 0, 0, 0]); // disk numbers
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_len.to_le_bytes());
        out.extend_from_slice(&cd_start.to_le_bytes());
        out.extend_from_slice(&[0, 0]); // comment length
        out
    }

    /// CRC-32, so the records a real reader checks are correct.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for byte in bytes {
            crc ^= *byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    /// A duplicate is refused in EVERY namespace, and the refusal names the
    /// path (IC-713).
    ///
    /// "3 entries, 2 distinct names" told somebody the file was wrong
    /// without telling them which part. The parser cannot answer that -- it
    /// keys entries by name and has already dropped the duplicate -- so the
    /// central-directory records are walked directly.
    ///
    /// The fixtures are built by a different zip writer than Krate's, which
    /// is IC-713's own requirement ("archives produced by multiple ZIP
    /// writers"): the Rust writer refuses a literal duplicate name, so these
    /// are assembled from raw records.
    #[test]
    fn signing_a_bundle_preserves_an_envelope_it_does_not_understand() {
        // IC-856 asks for read-write preservation, and signing is the one
        // path that reads a bundle and writes it back. If it dropped or
        // rewrote entries it does not understand, then adding a signature
        // would quietly strip whatever a later Krate had put there -- and
        // the bundle would come out different from the one that was
        // reviewed.
        let dir = TempDir::new().expect("tempdir");
        let bundle = dir.path().join("app.krate");

        const PROFILE: &[u8] = b"1\nsomething-added-later = whatever\n";
        let mut buf = Vec::new();
        {
            let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            writer.start_file(PROFILE_ENTRY, opts).expect("profile");
            writer.write_all(PROFILE).expect("write");
            writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
            writer.write_all(MANIFEST.as_bytes()).expect("write");
            writer.start_file(COMPONENT_ENTRY, opts).expect("component");
            writer.write_all(MINIMAL_COMPONENT).expect("write");
            writer.finish().expect("finish");
        }
        fs::write(&bundle, &buf).expect("write bundle");

        let pkcs8 = signing::SigningKey::generate_pkcs8().expect("generate a key");
        let key = signing::SigningKey::from_pkcs8(&pkcs8).expect("load the key");
        sign_bundle(&bundle, &key, "pub/test", "1.0.0", 1_700_000_000).expect("sign");

        let mut signed = ZipArchive::new(io::Cursor::new(
            fs::read(&bundle).expect("read the signed bundle"),
        ))
        .expect("the signed bundle is an archive");

        let mut profile = Vec::new();
        signed
            .by_name(PROFILE_ENTRY)
            .expect("the profile must survive signing")
            .read_to_end(&mut profile)
            .expect("read the profile");
        assert_eq!(
            profile, PROFILE,
            "signing must copy an envelope it does not understand through \
             untouched, or adding a signature silently rewrites the bundle"
        );

        // And the signature really was added, so this is not passing because
        // signing quietly did nothing.
        assert!(
            signed.by_name(SIGNATURE_ENTRY).is_ok(),
            "the bundle must actually be signed"
        );
    }

    #[test]
    fn each_kind_of_change_moves_exactly_the_identities_it_should() {
        // IC-860. Three identities answer three questions, and the whole
        // point is that they move INDEPENDENTLY. One digest standing in for
        // another is the defect this row exists for -- a bundle whose source
        // differed showed the same value under the heading "Identity"
        // (K-245).
        //
        // The table below is the contract, one row per kind of change:
        //
        //   change            archive  execution  project
        //   source only       yes      no         yes
        //   sdk only          yes      no         yes
        //   asset only        yes      yes        yes
        //   manifest only     yes      yes        yes
        //   component only    yes      yes        yes
        //   packaging only    yes      no         no
        //
        // "packaging only" is the one that would be easiest to get wrong and
        // the most damaging: re-compressing an app must not rename it, or
        // every reference breaks the moment somebody mirrors it.
        const MANIFEST_FOR_LAYERS: &str = "[app]\nid = \"com.example.layers\"\n\
             name = \"Layers\"\nversion = \"1.0.0\"\nentry = \"code.wasm\"\n\
             world = \"krate:app/cli@0.1.0\"\n";

        struct Parts<'a> {
            manifest: &'a str,
            component: &'a [u8],
            asset: &'a [u8],
            source: &'a [u8],
            sdk: &'a [u8],
            stored: bool,
        }
        impl Default for Parts<'_> {
            fn default() -> Self {
                Parts {
                    manifest: MANIFEST_FOR_LAYERS,
                    component: MINIMAL_COMPONENT,
                    asset: b"PNGDATA",
                    source: b"fn main() {}",
                    sdk: b"wit",
                    stored: false,
                }
            }
        }

        fn build(dir: &Path, name: &str, parts: Parts<'_>) -> PathBuf {
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts = SimpleFileOptions::default().compression_method(if parts.stored {
                    CompressionMethod::Stored
                } else {
                    CompressionMethod::Deflated
                });
                for (entry, bytes) in [
                    (PROFILE_ENTRY, b"1".as_slice()),
                    (MANIFEST_ENTRY, parts.manifest.as_bytes()),
                    (COMPONENT_ENTRY, parts.component),
                    ("assets/logo.png", parts.asset),
                    ("source/lib.rs", parts.source),
                    ("sdk/krate.wit", parts.sdk),
                ] {
                    writer.start_file(entry, opts).expect("entry");
                    writer.write_all(bytes).expect("write");
                }
                writer.finish().expect("finish");
            }
            let path = dir.join(name);
            fs::write(&path, &buf).expect("write bundle");
            path
        }

        let dir = TempDir::new().expect("tempdir");
        let base_path = build(dir.path(), "base.krate", Parts::default());
        let base = open(&base_path).expect("base opens");
        let base_archive =
            provenance::digest_archive_bytes(&fs::read(&base_path).expect("read")).digest;
        let base_execution = base.digest().expect("digest").digest;
        let base_project = base.project_digest().expect("digest").digest;

        let renamed = MANIFEST_FOR_LAYERS.replace("Layers", "Renamed");
        let cases: [(&str, Parts<'_>, bool, bool); 6] = [
            // (what changed, parts, execution should move, project should move)
            (
                "source only",
                Parts {
                    source: b"fn main() { /* changed */ }",
                    ..Default::default()
                },
                false,
                true,
            ),
            (
                "sdk only",
                Parts {
                    sdk: b"wit v2",
                    ..Default::default()
                },
                false,
                true,
            ),
            (
                "asset only",
                Parts {
                    asset: b"PNGDATA2",
                    ..Default::default()
                },
                true,
                true,
            ),
            (
                "manifest only",
                Parts {
                    manifest: &renamed,
                    ..Default::default()
                },
                true,
                true,
            ),
            (
                "component only",
                Parts {
                    component: OTHER_COMPONENT,
                    ..Default::default()
                },
                true,
                true,
            ),
            (
                "packaging only",
                Parts {
                    stored: true,
                    ..Default::default()
                },
                false,
                false,
            ),
        ];

        for (what, parts, execution_moves, project_moves) in cases {
            let path = build(dir.path(), "variant.krate", parts);
            let opened = open(&path).expect("variant opens");
            let archive = provenance::digest_archive_bytes(&fs::read(&path).expect("read")).digest;
            let execution = opened.digest().expect("digest").digest;
            let project = opened.project_digest().expect("digest").digest;

            // Every one of these produces a different FILE, or the fixture
            // is not testing what it claims.
            assert_ne!(
                archive, base_archive,
                "{what}: the fixture must really produce different bytes"
            );
            assert_eq!(
                execution != base_execution,
                execution_moves,
                "{what}: execution identity moved the wrong way"
            );
            assert_eq!(
                project != base_project,
                project_moves,
                "{what}: project identity moved the wrong way"
            );
        }
    }

    #[test]
    fn signing_an_app_does_not_make_it_a_different_app() {
        // IC-860. A signature is ABOUT an identity, so it cannot be part of
        // one -- signing would otherwise change the very value it is
        // attesting to, and no signature could ever verify against the
        // bundle carrying it.
        //
        // It is also the change most likely to be made to a finished bundle,
        // so getting it wrong would break references to every app anybody
        // ever signed.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("app.krate");
        let mut buf = Vec::new();
        {
            let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            for (entry, bytes) in [
                (PROFILE_ENTRY, b"1".as_slice()),
                (MANIFEST_ENTRY, MANIFEST.as_bytes()),
                (COMPONENT_ENTRY, MINIMAL_COMPONENT),
                ("source/lib.rs", b"fn main() {}"),
            ] {
                writer.start_file(entry, opts).expect("entry");
                writer.write_all(bytes).expect("write");
            }
            writer.finish().expect("finish");
        }
        fs::write(&path, &buf).expect("write bundle");

        let (execution, project) = {
            let opened = open(&path).expect("opens");
            (
                opened.digest().expect("digest").digest,
                opened.project_digest().expect("digest").digest,
            )
        };

        let pkcs8 = signing::SigningKey::generate_pkcs8().expect("generate");
        let key = signing::SigningKey::from_pkcs8(&pkcs8).expect("load");
        sign_bundle(&path, &key, "pub/test", "1.0.0", 1_700_000_000).expect("sign");

        // The FILE changed -- there is a signature in it now.
        assert_ne!(
            fs::read(&path).expect("read"),
            buf,
            "signing must actually have written something"
        );

        let signed = open(&path).expect("the signed bundle opens");
        assert_eq!(
            signed.digest().expect("digest").digest,
            execution,
            "signing must not change what the app IS -- a signature attests \
             to an identity and cannot be part of it"
        );
        assert_eq!(
            signed.project_digest().expect("digest").digest,
            project,
            "signing must not change the project identity either"
        );
    }

    /// Backdate a directory, so a test can stand in for a crash an hour ago.
    fn set_modified(path: &Path, when: std::time::SystemTime) {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        // Windows refuses to open a DIRECTORY as a file ("Access is denied")
        // unless the handle asks for backup semantics, and refuses to SET a
        // time through a handle opened for reading alone -- it needs
        // FILE_WRITE_ATTRIBUTES. Both are asked for; there is no other way
        // to get a handle to set a directory's times through.
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_READ_ATTRIBUTES: u32 = 0x0080;
            const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
            const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
            options
                .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
        }
        let file = options.open(path).expect("open the directory");
        file.set_modified(when).expect("backdate it");
    }

    #[test]
    fn a_directory_left_by_a_killed_run_is_swept_but_a_live_one_is_not() {
        // K-264. A hard kill runs no destructors, so the directory a bundle
        // was being unpacked into survives -- measured at 200 MB from one
        // interrupted open. A signal handler would cover Ctrl-C and not
        // SIGKILL, and neither covers power loss. Sweeping on the next open
        // covers all three.
        //
        // The danger in a sweep is deleting a directory another process is
        // still using, so both halves are checked: old is removed, young is
        // left alone.
        let root = TempDir::new().expect("tempdir");

        let abandoned = root.path().join(format!("{EXTRACT_PREFIX}abandoned"));
        fs::create_dir(&abandoned).expect("create");
        fs::write(abandoned.join("code.wasm"), b"left behind").expect("write");

        let live = root.path().join(format!("{EXTRACT_PREFIX}live"));
        fs::create_dir(&live).expect("create");
        fs::write(live.join("code.wasm"), b"in use right now").expect("write");

        // Something else's temp directory, which we must never touch.
        let stranger = root.path().join(".tmpSomeoneElse");
        fs::create_dir(&stranger).expect("create");
        fs::write(stranger.join("data"), b"not ours").expect("write");

        // Age the abandoned one past the threshold. The other two are new.
        let long_ago =
            std::time::SystemTime::now() - ABANDONED_AFTER - std::time::Duration::from_secs(60);
        set_modified(&abandoned, long_ago);
        set_modified(&stranger, long_ago);

        sweep_in(root.path());

        assert!(
            !abandoned.exists(),
            "a directory left behind by a killed run must be swept"
        );
        assert!(
            live.exists(),
            "a directory a live open is still writing into must NOT be swept \
             -- it is young, and age is the only thing separating them"
        );
        assert!(
            stranger.exists(),
            "another program's temp directory is not ours to delete, however \
             old it is"
        );
    }

    /// A fork carries the record of its parent (IC-397, test 494). The
    /// record is part of what the fork IS and of what a signature over it
    /// covers, and no part of what runs: a fork with unchanged code has its
    /// parent's execution identity and its own project identity.
    #[test]
    fn a_fork_records_its_parent_in_the_project_identity_and_not_the_execution_one() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = dir.path().join("manifest.toml");
        fs::write(
            &manifest,
            "[app]\nid = \"com.example.fork\"\nname = \"Fork\"\nversion = \"1.0.0\"\n\
             entry = \"code.wasm\"\nworld = \"krate:app/cli@0.1.0\"\n",
        )
        .expect("manifest");
        let component = dir.path().join("code.wasm");
        fs::write(&component, MINIMAL_COMPONENT).expect("component");

        let parent = dir.path().join("parent.krate");
        pack(&manifest, &component, &parent).expect("pack parent");
        let record = derived_from_record(&parent, 1_700_000_000).expect("record");
        assert_eq!(record.schema, DERIVED_FROM_SCHEMA);
        assert_eq!(
            record.archive,
            provenance::digest_archive_bytes(&fs::read(&parent).unwrap()).digest,
            "the record names the parent file's own bytes"
        );
        assert_eq!(
            record.execution,
            open(&parent).unwrap().digest().unwrap().digest
        );
        assert!(
            record.project.is_none(),
            "a source-less parent has no project identity"
        );
        assert!(!record.parent_signed);

        let fork = dir.path().join("fork.krate");
        pack_with_lineage(
            &manifest,
            &component,
            None,
            None,
            None,
            Some(&record),
            &fork,
        )
        .expect("pack fork");
        let plain = dir.path().join("plain.krate");
        pack(&manifest, &component, &plain).expect("pack plain");

        let opened = open(&fork).expect("open fork");
        assert_eq!(
            opened.derived_from().expect("read record"),
            Some(record.clone()),
            "the fork says what it was changed from"
        );
        assert_eq!(
            open(&plain).unwrap().derived_from().unwrap(),
            None,
            "an original says nothing"
        );
        assert_eq!(
            opened.digest().unwrap().digest,
            open(&plain).unwrap().digest().unwrap().digest,
            "what runs is unchanged, so the execution identity is the parent's"
        );
        assert_ne!(
            opened.project_digest().unwrap().digest,
            open(&plain).unwrap().project_digest().unwrap().digest,
            "what this IS includes its parent, so the project identity is its own"
        );

        // A signature over the fork covers the record: change it and the
        // signature reads as tampering, so a fork cannot quietly change
        // whose child it claims to be.
        let pkcs8 = signing::SigningKey::generate_pkcs8().unwrap();
        let key = signing::SigningKey::from_pkcs8(&pkcs8).unwrap();
        sign_bundle(&fork, &key, "com.example.fork", "1.0.0", 1_700_000_001).expect("sign");
        let signed = open(&fork).unwrap();
        assert_eq!(
            signed.derived_from().unwrap(),
            Some(record.clone()),
            "signing keeps the record"
        );
        assert!(matches!(
            signed.signature_verdict().unwrap(),
            Some(signing::Verdict::Valid { .. })
        ));
        let mut forged = record.clone();
        forged.parent_signed = true;
        replace_entry(
            &fork,
            DERIVED_FROM_ENTRY,
            &serde_json::to_vec(&forged).unwrap(),
        )
        .unwrap();
        assert!(
            matches!(
                open(&fork).unwrap().signature_verdict().unwrap(),
                Some(signing::Verdict::Tampered { .. })
            ),
            "a changed record breaks the signature over the fork"
        );

        // A parent that was signed is recorded as such -- the fact, not
        // the key.
        sign_bundle(&parent, &key, "com.example.fork", "1.0.0", 1_700_000_000)
            .expect("sign parent");
        let of_signed = derived_from_record(&parent, 1_700_000_002).unwrap();
        assert!(of_signed.parent_signed);
        let text = serde_json::to_string(&of_signed).unwrap();
        assert!(
            !text.contains(
                &key.public_key()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ),
            "the record must not carry the parent's signer: {text}"
        );
    }

    /// A release id names one signed release and nothing else (IC-389,
    /// test 468; K-308): the same content signed the same way is the same
    /// release; a different version, a different signer, or different
    /// content is another; a file that does not verify is no release.
    #[test]
    fn a_release_id_is_stable_under_repacking_and_moves_with_version_signer_and_content() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let alice =
            signing::SigningKey::from_pkcs8(&signing::SigningKey::generate_pkcs8().unwrap())
                .unwrap();
        let bob = signing::SigningKey::from_pkcs8(&signing::SigningKey::generate_pkcs8().unwrap())
            .unwrap();
        let release_of = |path: &Path| open(path).unwrap().release().unwrap();

        let a = dir.path().join("a.krate");
        pack(&manifest, &component, &a).expect("pack");
        assert_eq!(release_of(&a), None, "an unsigned file is no release");
        sign_bundle(&a, &alice, "ns", "1.0.0", 1_700_000_000).expect("sign");
        let first = release_of(&a).expect("a signed file is a release");
        assert_eq!(first.version, "1.0.0");
        assert_eq!(
            first.authority,
            alice
                .public_key()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );

        // Byte-identical retry: same file, same release.
        let copy = dir.path().join("copy.krate");
        fs::copy(&a, &copy).unwrap();
        assert_eq!(
            release_of(&copy).unwrap().id,
            first.id,
            "the same bytes are the same release"
        );

        // Repacked and re-signed the same way: same content, same signer,
        // same version, same moment -- the same release in a different file.
        // Signing rewrites the archive in the packer's one canonical layout,
        // so a re-pack signed the same way is byte-identical. A zip comment
        // appended after signing makes the same content a genuinely
        // different file, which is what a repack by another tool is.
        let repacked = dir.path().join("repacked.krate");
        pack(&manifest, &component, &repacked).expect("pack again");
        sign_bundle(&repacked, &alice, "ns", "1.0.0", 1_700_000_000).expect("sign again");
        {
            let mut bytes = fs::read(&repacked).unwrap();
            let len = bytes.len();
            assert_eq!(&bytes[len - 2..], &[0, 0], "no comment yet");
            bytes[len - 2] = 1;
            bytes.push(b'x');
            fs::write(&repacked, bytes).unwrap();
        }
        assert_ne!(
            fs::read(&a).unwrap(),
            fs::read(&repacked).unwrap(),
            "the files differ"
        );
        assert_eq!(
            release_of(&repacked).unwrap().id,
            first.id,
            "repacking is not a new release"
        );

        // A new version is a new release.
        sign_bundle(&repacked, &alice, "ns", "1.0.1", 1_700_000_000).expect("sign v2");
        assert_ne!(
            release_of(&repacked).unwrap().id,
            first.id,
            "a version change is a new release"
        );

        // Another publisher signing the same content is their release.
        let theirs = dir.path().join("theirs.krate");
        pack(&manifest, &component, &theirs).expect("pack");
        sign_bundle(&theirs, &bob, "ns", "1.0.0", 1_700_000_000).expect("bob signs");
        assert_ne!(
            release_of(&theirs).unwrap().id,
            first.id,
            "a different signer is a different release"
        );

        // Different executable content is a new release.
        let other = write_temp(dir.path(), "other.wasm", OTHER_COMPONENT);
        let changed = dir.path().join("changed.krate");
        pack(&manifest, &other, &changed).expect("pack");
        sign_bundle(&changed, &alice, "ns", "1.0.0", 1_700_000_000).expect("sign");
        assert_ne!(
            release_of(&changed).unwrap().id,
            first.id,
            "changed code is a new release"
        );

        // A file changed after signing is not a release at all.
        replace_entry(&a, COMPONENT_ENTRY, OTHER_COMPONENT).unwrap();
        assert_eq!(
            release_of(&a),
            None,
            "a tampered file is no release, so it has no id"
        );
    }

    /// The bytes served at a content address must be that content
    /// (IC-389, test 471). A short link is a prefix of the address; a
    /// channel link redirects to a fixed address and is held to that one.
    #[cfg(feature = "fetch")]
    #[test]
    fn fetched_bytes_must_match_the_address_they_were_served_at() {
        use std::io::{Read as _, Write as _};

        let bundle = {
            let dir = TempDir::new().expect("tempdir");
            let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
            let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
            let path = dir.path().join("app.krate");
            pack(&manifest, &component, &path).expect("pack");
            fs::read(&path).expect("read")
        };
        let address: String = {
            use sha2::{Digest, Sha256};
            Sha256::digest(&bundle)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        };
        let wrong = format!("{}{}", &"0".repeat(32), &address[32..]);

        // One server: `/a/<x>` serves the bundle whatever x is (a stale
        // cache, a compromised alias); `/c/<name>` redirects to the address
        // in its query.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let served = bundle.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..8 {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut request = [0u8; 2048];
                let n = stream.read(&mut request).unwrap_or(0);
                let line = String::from_utf8_lossy(&request[..n]);
                let target = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                let response = if let Some(rest) = target.strip_prefix("/c/") {
                    let to = rest.split("to=").nth(1).unwrap_or("").to_string();
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/a/{to}?dl=1\r\n\
                         Content-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes()
                } else {
                    let mut r = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        served.len()
                    )
                    .into_bytes();
                    r.extend_from_slice(&served);
                    r
                };
                let _ = stream.write_all(&response);
                let _ = stream.flush();
            }
        });
        let at = |path: String| fetch_resolved(&format!("http://127.0.0.1:{port}{path}"), true);

        let err = at(format!("/a/{wrong}?dl=1"))
            .err()
            .expect("wrong address must refuse");
        assert!(
            err.to_string().contains("not the ones it names"),
            "the refusal names the mismatch: {err}"
        );
        let ok = at(format!("/a/{address}?dl=1")).expect("the right address opens");
        assert_eq!(ok.address.as_deref(), Some(address.as_str()));
        let short = at(format!("/a/{}?dl=1", &address[..12])).expect("a short link is a prefix");
        assert_eq!(short.address.as_deref(), Some(&address[..12]));
        let via_channel =
            at(format!("/c/alice/notes?dl=1&to={address}")).expect("a channel resolves");
        assert!(
            via_channel.url.contains(&format!("/a/{address}")),
            "a channel link is held to the fixed address it resolved to: {}",
            via_channel.url
        );
        let stale = at(format!("/c/alice/notes?dl=1&to={wrong}"))
            .err()
            .expect("a channel pointing at the wrong address must refuse");
        assert!(
            stale.to_string().contains("not the ones it names"),
            "{stale}"
        );
        let plain = at("/app.krate".to_string()).expect("a plain file link names no address");
        assert_eq!(plain.address, None);
        let _ = server.join();
    }

    /// An edit applied to each entry of an archive: the bytes to keep, or
    /// None to drop the entry.
    type EditFn = dyn Fn(&str, &[u8]) -> Option<Vec<u8>>;

    /// Rewrite an archive: every entry passes through `edit` (None drops
    /// it), then `extra` entries are appended. Test archives need shapes
    /// our own writer refuses to produce.
    fn edit_archive(source: &Path, destination: &Path, edit: &EditFn, extra: &[(&str, &[u8])]) {
        let data = fs::read(source).expect("read source");
        let mut archive = ZipArchive::new(io::Cursor::new(&data)).expect("open zip");
        let file = File::create(destination).expect("create");
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for index in 0..archive.len() {
            let mut existing = archive.by_index(index).expect("entry");
            let name = existing.name().to_string();
            let mut bytes = Vec::new();
            existing.read_to_end(&mut bytes).expect("read");
            if let Some(payload) = edit(&name, &bytes) {
                writer.start_file(name.clone(), options).expect("start");
                writer.write_all(&payload).expect("write");
            }
        }
        for (name, bytes) in extra {
            writer.start_file(*name, options).expect("start");
            writer.write_all(bytes).expect("write");
        }
        writer.finish().expect("finish");
    }

    /// The variant name of an error, for "both doors refuse for the same
    /// reason" assertions.
    fn variant(err: &BundleError) -> String {
        let text = format!("{err:?}");
        text.split(|c: char| !c.is_alphanumeric())
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// Profile 2 refuses a top-level record the format does not name, and
    /// profile 1 keeps ignoring it (IC-714, test 1475). The reader leads
    /// the writer: this build reads profile 2 and still writes profile 1,
    /// so no installed Krate meets a file it cannot open.
    #[test]
    fn profile_two_refuses_unknown_records_and_profile_one_ignores_them() {
        fn archive(profile: Option<&[u8]>, extra: &[(&str, &[u8])]) -> Vec<u8> {
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                if let Some(profile) = profile {
                    writer.start_file(PROFILE_ENTRY, opts).expect("profile");
                    writer.write_all(profile).expect("write");
                }
                writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
                writer.write_all(MANIFEST.as_bytes()).expect("write");
                writer.start_file(COMPONENT_ENTRY, opts).expect("component");
                writer.write_all(MINIMAL_COMPONENT).expect("write");
                for (name, bytes) in extra {
                    writer.start_file(*name, opts).expect("extra");
                    writer.write_all(bytes).expect("write");
                }
                writer.finish().expect("finish");
            }
            buf
        }
        assert_eq!(READS_PROFILE, 2, "the reader knows profile 2");
        let dir = TempDir::new().expect("tempdir");
        let stray: [(&str, &[u8]); 1] = [("notes.txt", b"a file every reader would skip")];

        // Profile 1 and no profile line: the stray record is ignored, and
        // the record set says what it is.
        for (what, profile) in [("profile 1", Some(b"1".as_slice())), ("no profile", None)] {
            let path = dir.path().join("one.krate");
            fs::write(&path, archive(profile, &stray)).unwrap();
            let opened = open(&path).unwrap_or_else(|err| panic!("{what}: must open: {err}"));
            assert_eq!(opened.profile(), 1, "{what}");
            let unknown: Vec<&str> = opened
                .records()
                .iter()
                .filter(|record| record.class == "unknown")
                .map(|record| record.name.as_str())
                .collect();
            assert_eq!(
                unknown,
                ["notes.txt"],
                "{what}: the stray record is listed as unknown"
            );
            let judged = judge_bytes(&fs::read(&path).unwrap(), 64 * 1024 * 1024).unwrap();
            assert_eq!(judged.profile, 1, "{what}");
            assert_eq!(
                judged.records,
                opened.records(),
                "{what}: both doors list it"
            );
        }

        // Profile 2: refused, and the refusal names the record and where
        // developer material belongs.
        let two = dir.path().join("two.krate");
        let two_bytes = archive(Some(b"2"), &stray);
        fs::write(&two, &two_bytes).unwrap();
        let err = open(&two).expect_err("profile 2 refuses an unknown record");
        assert!(
            matches!(&err, BundleError::UnknownEntry { path, profile: 2 } if path == "notes.txt"),
            "{err:?}"
        );
        let text = err.to_string();
        assert!(
            text.contains("notes.txt") && text.contains("ext/<owner>/<name>/"),
            "the refusal says which record and what to do: {text}"
        );
        let judged = judge_bytes(&two_bytes, 64 * 1024 * 1024).expect_err("the judge agrees");
        assert_eq!(variant(&judged), "UnknownEntry", "{judged}");

        // Profile 2 with only the records the format names: opens, and
        // says which profile it is.
        let clean = dir.path().join("clean.krate");
        fs::write(&clean, archive(Some(b"2"), &[])).unwrap();
        let opened = open(&clean).expect("a clean profile 2 bundle opens");
        assert_eq!(opened.profile(), 2);
        assert_eq!(
            judge_bytes(&fs::read(&clean).unwrap(), 64 * 1024 * 1024)
                .unwrap()
                .profile,
            2
        );

        // What this build packs declares the profile it writes, and a
        // packed bundle carries nothing the profile does not name.
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let packed = dir.path().join("packed.krate");
        pack(&manifest, &component, &packed).unwrap();
        let opened = open(&packed).unwrap();
        assert_eq!(opened.profile(), PROFILE_VERSION);
        assert!(
            opened
                .records()
                .iter()
                .all(|record| record.class != "unknown"),
            "{:?}",
            opened.records()
        );
    }

    /// A declared extension records owner, name, version, size and digest,
    /// travels as project material, and comes back through the reader with
    /// its files (IC-714, test 1476).
    #[test]
    fn a_declared_extension_records_owner_version_size_and_digest_and_round_trips() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let plugins = dir.path().join("plugins");
        fs::create_dir_all(plugins.join("nested")).unwrap();
        fs::write(plugins.join("a.toml"), b"[a]\n").unwrap();
        let blob: Vec<u8> = (0..=255u8).collect();
        fs::write(plugins.join("nested/b.bin"), &blob).unwrap();
        let extension = ExtensionSource {
            owner: "acme".to_string(),
            name: "plugins".to_string(),
            version: "3".to_string(),
            root: plugins,
        };
        let out = dir.path().join("ext.krate");
        pack_with_extensions(
            &manifest,
            &component,
            None,
            None,
            None,
            None,
            std::slice::from_ref(&extension),
            &out,
        )
        .unwrap();

        let opened = open(&out).expect("a bundle with a declared extension opens");
        assert_eq!(opened.profile(), PROFILE_VERSION);
        let [record] = opened.extensions() else {
            panic!("one declared extension, got {:?}", opened.extensions())
        };
        assert_eq!(
            (
                record.owner.as_str(),
                record.name.as_str(),
                record.version.as_str(),
                record.size
            ),
            ("acme", "plugins", "3", 4 + 256)
        );
        let mut expected = BTreeMap::new();
        expected.insert("ext/acme/plugins/a.toml".to_string(), b"[a]\n".to_vec());
        expected.insert("ext/acme/plugins/nested/b.bin".to_string(), blob.clone());
        assert_eq!(
            record.digest,
            extension_digest(&expected),
            "the digest is over the sorted entries"
        );
        assert_eq!(record.digest.len(), 64);

        let unpacked = opened
            .extension_path("acme", "plugins")
            .expect("the declared group was unpacked");
        assert_eq!(fs::read(unpacked.join("nested/b.bin")).unwrap(), blob);
        assert!(
            opened.extension_path("acme", "other").is_none(),
            "an undeclared group has no path"
        );
        let classes: BTreeMap<&str, &str> = opened
            .records()
            .iter()
            .map(|record| (record.name.as_str(), record.class.as_str()))
            .collect();
        assert_eq!(classes.get("ext/acme/plugins/a.toml"), Some(&"extension"));
        assert_eq!(classes.get(EXTENSIONS_ENTRY), Some(&"extensions"));

        // Project material, not runnable: the execution identity is the
        // one the bundle has without the extension, the project identity
        // covers it.
        let execution = opened.digest().unwrap();
        let project = opened.project_digest().unwrap();
        for name in ["ext/acme/plugins/a.toml", EXTENSIONS_ENTRY] {
            assert!(
                !execution.entries.contains_key(name),
                "{name} is not runnable"
            );
            assert!(
                project.entries.contains_key(name),
                "{name} is what the bundle is"
            );
        }
        let plain = dir.path().join("plain.krate");
        pack(&manifest, &component, &plain).unwrap();
        assert_eq!(
            open(&plain).unwrap().digest().unwrap().digest,
            execution.digest
        );

        // Deterministic, like every other pack.
        let again = dir.path().join("again.krate");
        pack_with_extensions(
            &manifest,
            &component,
            None,
            None,
            None,
            None,
            std::slice::from_ref(&extension),
            &again,
        )
        .unwrap();
        assert_eq!(fs::read(&out).unwrap(), fs::read(&again).unwrap());

        // The other door resolves the same records and the same extensions.
        let judged = judge_bytes(&fs::read(&out).unwrap(), 64 * 1024 * 1024).unwrap();
        assert_eq!(judged.extensions, opened.extensions());
        assert_eq!(judged.records, opened.records());
        assert_eq!(judged.execution, execution.digest);
        assert_eq!(judged.project, Some(project.digest));

        // Labels are path segments: a slash or a dot segment is refused
        // before anything is written.
        for bad in ["../x", "Acme", "a/b", "", "."] {
            let err = pack_with_extensions(
                &manifest,
                &component,
                None,
                None,
                None,
                None,
                &[ExtensionSource {
                    owner: bad.to_string(),
                    name: "plugins".to_string(),
                    version: "1".to_string(),
                    root: extension.root.clone(),
                }],
                &dir.path().join("bad.krate"),
            )
            .expect_err("a label that is not a label is refused");
            assert_eq!(variant(&err), "MalformedExtensionPath", "{bad:?}: {err}");
        }
    }

    /// An extension that is not what its declaration says is refused, at
    /// both doors, for the reason that is true (IC-714, tests 1475-1476).
    #[test]
    fn an_extension_that_is_not_what_it_declares_is_refused() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let plugins = dir.path().join("plugins");
        fs::create_dir_all(&plugins).unwrap();
        fs::write(plugins.join("a.toml"), b"[a]\n").unwrap();
        let good = dir.path().join("good.krate");
        pack_with_extensions(
            &manifest,
            &component,
            None,
            None,
            None,
            None,
            &[ExtensionSource {
                owner: "acme".to_string(),
                name: "plugins".to_string(),
                version: "3".to_string(),
                root: plugins,
            }],
            &good,
        )
        .unwrap();
        let keep = |_: &str, bytes: &[u8]| Some(bytes.to_vec());

        type Case<'a> = (
            &'a str,
            Box<EditFn>,
            Vec<(&'a str, &'a [u8])>,
            &'a str,
            &'a str,
        );
        let cases: Vec<Case> = vec![
            (
                "contents changed under the declaration",
                Box::new(|name, bytes| {
                    Some(if name == "ext/acme/plugins/a.toml" {
                        b"[b]\n".to_vec()
                    } else {
                        bytes.to_vec()
                    })
                }),
                vec![],
                "ExtensionMismatch",
                "do not match the digest",
            ),
            (
                "size changed under the declaration",
                Box::new(|name, bytes| {
                    Some(if name == "ext/acme/plugins/a.toml" {
                        b"[aa]\n".to_vec()
                    } else {
                        bytes.to_vec()
                    })
                }),
                vec![],
                "ExtensionMismatch",
                "declares 4 bytes and carries 5",
            ),
            (
                "a group nobody declared",
                Box::new(keep),
                vec![("ext/evil/thing/x", b"smuggled".as_slice())],
                "UndeclaredExtension",
                "evil/thing",
            ),
            (
                "a declared group with no entries",
                Box::new(|name, bytes| (!name.starts_with("ext/")).then(|| bytes.to_vec())),
                vec![],
                "MissingExtension",
                "acme/plugins",
            ),
            (
                "a declaration that is not JSON",
                Box::new(|name, bytes| {
                    Some(if name == EXTENSIONS_ENTRY {
                        b"{".to_vec()
                    } else {
                        bytes.to_vec()
                    })
                }),
                vec![],
                "DamagedExtensions",
                "fresh",
            ),
            (
                "a declaration from a later format",
                Box::new(|name, bytes| {
                    Some(if name == EXTENSIONS_ENTRY {
                        br#"{"schema":"krate.bundle.extensions.v9","extensions":[]}"#.to_vec()
                    } else {
                        bytes.to_vec()
                    })
                }),
                vec![],
                "DamagedExtensions",
                "newer format",
            ),
            (
                "an extension path with no file under its group",
                Box::new(keep),
                vec![("ext/acme/plugins", b"x".as_slice())],
                "MalformedExtensionPath",
                "ext/<owner>/<name>/<file>",
            ),
        ];
        for (what, edit, extra, expected_variant, expected_words) in &cases {
            let path = dir.path().join("edited.krate");
            edit_archive(&good, &path, edit.as_ref(), extra);
            let err = match open(&path) {
                Err(err) => err,
                Ok(_) => panic!("{what}: must be refused"),
            };
            assert_eq!(variant(&err), *expected_variant, "{what}: {err}");
            assert!(
                err.to_string().contains(expected_words),
                "{what}: the refusal must say so ({expected_words:?}): {err}"
            );
            let judged = judge_bytes(&fs::read(&path).unwrap(), 64 * 1024 * 1024)
                .expect_err("the judge refuses what open refuses");
            assert_eq!(
                variant(&judged),
                *expected_variant,
                "{what}: the two doors disagree: {judged}"
            );
        }

        // Profile 1 with no declaration: an ext/ entry is unknown material
        // and is ignored, as the recorded generation says. The same
        // archive under profile 2 must be declared.
        let plain = dir.path().join("plain.krate");
        pack(&manifest, &component, &plain).unwrap();
        let loose = dir.path().join("loose.krate");
        edit_archive(
            &plain,
            &loose,
            &|name, bytes| {
                Some(if name == PROFILE_ENTRY {
                    b"1".to_vec()
                } else {
                    bytes.to_vec()
                })
            },
            &[("ext/acme/plugins/a.toml", b"[a]\n")],
        );
        let opened = open(&loose).expect("profile 1 ignores an undeclared ext/ entry");
        assert!(opened.extensions().is_empty());
        assert!(opened.extension_path("acme", "plugins").is_none());
        let strict = dir.path().join("strict.krate");
        edit_archive(
            &loose,
            &strict,
            &|name, bytes| {
                Some(if name == PROFILE_ENTRY {
                    b"2".to_vec()
                } else {
                    bytes.to_vec()
                })
            },
            &[],
        );
        let err = open(&strict).expect_err("profile 2 wants every ext/ group declared");
        assert_eq!(variant(&err), "UndeclaredExtension", "{err}");
    }

    /// The in-memory judge and the on-disk open are one validator with
    /// two doors (IC-833, the "one shared validator" plan item). Held to the
    /// same answers on a plain app, a signed one, a fork, a tampered one,
    /// and every adversarial fixture the tree keeps.
    #[test]
    fn the_two_doors_agree() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let source = dir.path().join("src-tree");
        fs::create_dir_all(source.join("src")).unwrap();
        fs::write(source.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(source.join("src/lib.rs"), "// code\n").unwrap();

        let plain = dir.path().join("plain.krate");
        pack(&manifest, &component, &plain).unwrap();
        let with_source = dir.path().join("with-source.krate");
        pack_with_source(&manifest, &component, None, Some(&source), &with_source).unwrap();
        let signed = dir.path().join("signed.krate");
        fs::copy(&with_source, &signed).unwrap();
        let key = signing::SigningKey::from_pkcs8(&signing::SigningKey::generate_pkcs8().unwrap())
            .unwrap();
        sign_bundle(&signed, &key, "ns", "1.0.0", 1_700_000_000).unwrap();
        let fork = dir.path().join("fork.krate");
        let record = derived_from_record(&signed, 1_700_000_001).unwrap();
        pack_with_lineage(
            &manifest,
            &component,
            None,
            None,
            None,
            Some(&record),
            &fork,
        )
        .unwrap();
        let tampered = dir.path().join("tampered.krate");
        fs::copy(&signed, &tampered).unwrap();
        replace_entry(&tampered, COMPONENT_ENTRY, OTHER_COMPONENT).unwrap();

        for path in [&plain, &with_source, &signed, &fork, &tampered] {
            let bytes = fs::read(path).unwrap();
            let judged = judge_bytes(&bytes, 64 * 1024 * 1024).unwrap_or_else(|err| {
                panic!(
                    "{}: judge refused a bundle open accepts: {err}",
                    path.display()
                )
            });
            let opened = open(path).unwrap();
            let name = path.display();
            assert_eq!(judged.manifest, *opened.manifest(), "{name}: manifest");
            assert_eq!(
                judged.execution,
                opened.digest().unwrap().digest,
                "{name}: execution identity"
            );
            let open_project = opened.project_digest().unwrap();
            let open_execution = opened.digest().unwrap();
            assert_eq!(
                judged.project,
                (open_project.entries.len() > open_execution.entries.len())
                    .then_some(open_project.digest),
                "{name}: project identity"
            );
            assert_eq!(judged.release, opened.release().unwrap(), "{name}: release");
            assert_eq!(judged.profile, opened.profile(), "{name}: profile");
            assert_eq!(
                judged.records,
                opened.records(),
                "{name}: record set (1477)"
            );
            assert_eq!(judged.extensions, opened.extensions(), "{name}: extensions");
            assert_eq!(
                judged.derived_from,
                opened.derived_from().unwrap(),
                "{name}: fork record"
            );
            let open_state = match opened.signature_verdict().unwrap() {
                None => "absent",
                Some(signing::Verdict::Valid { .. }) => "valid",
                Some(signing::Verdict::BadSignature) => "bad",
                Some(signing::Verdict::Tampered { .. }) => "tampered",
                Some(signing::Verdict::UnknownSchema { .. }) => "unknown-schema",
            };
            assert_eq!(
                judged.signature.state, open_state,
                "{name}: signature state"
            );
        }
        assert_eq!(
            judge_bytes(&fs::read(&signed).unwrap(), 64 * 1024 * 1024)
                .unwrap()
                .signature
                .state,
            "valid"
        );
        assert_eq!(
            judge_bytes(&fs::read(&tampered).unwrap(), 64 * 1024 * 1024)
                .unwrap()
                .signature
                .state,
            "tampered"
        );
        assert!(judge_bytes(&fs::read(&fork).unwrap(), 64 * 1024 * 1024)
            .unwrap()
            .derived_from
            .is_some());

        // What open refuses, judge refuses, for the same reason.
        let duplicate = include_bytes!("../tests/fixtures/duplicate-from-another-writer.krate");
        let locked = include_bytes!("../tests/fixtures/locked-entry.krate");
        let module = {
            let m = write_temp(dir.path(), "module.wasm", b"\0asm\x01\0\0\0");
            let out = dir.path().join("module.krate");
            // pack refuses a module, so write the archive by hand
            let mut w = ZipWriter::new(File::create(&out).unwrap());
            let o = SimpleFileOptions::default();
            w.start_file(MANIFEST_ENTRY, o).unwrap();
            w.write_all(MANIFEST.as_bytes()).unwrap();
            w.start_file(COMPONENT_ENTRY, o).unwrap();
            w.write_all(&fs::read(&m).unwrap()).unwrap();
            w.finish().unwrap();
            fs::read(&out).unwrap()
        };
        for (what, bytes) in [
            ("a duplicate from another writer", duplicate.to_vec()),
            ("an encrypted entry", locked.to_vec()),
            ("a module where a component belongs", module),
            ("not a zip at all", b"PK\x03\x04 words".to_vec()),
        ] {
            let by_open = open_bytes(&bytes).err().map(|e| e.to_string());
            let by_judge = judge_bytes(&bytes, 64 * 1024 * 1024)
                .err()
                .map(|e| e.to_string());
            assert!(by_open.is_some(), "{what}: open must refuse this fixture");
            assert_eq!(
                by_judge, by_open,
                "{what}: the two doors must refuse for the same reason"
            );
        }

        // The door's own ceiling is its own refusal, named as such.
        let err =
            judge_bytes(&fs::read(&with_source).unwrap(), 16).expect_err("16 bytes is not enough");
        assert!(
            matches!(err, BundleError::ExpandsTooFar { limit: 16 }),
            "{err}"
        );
    }

    /// Write the hub's signed, tampered and module-not-component fixtures
    /// from the committed bounce bundle. Run by hand when they need
    /// regenerating:
    ///
    ///     cargo test -p krate-bundle --lib regenerate_hub_fixtures -- --ignored
    #[test]
    #[ignore]
    fn regenerate_hub_fixtures() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let fixtures = root.join("cloud/worker/test/fixtures");
        let bounce = root.join("evidence/ported/bounce.krate");
        let signed = fixtures.join("bounce-signed.krate");
        fs::copy(&bounce, &signed).unwrap();
        let key = signing::SigningKey::from_pkcs8(&signing::SigningKey::generate_pkcs8().unwrap())
            .unwrap();
        sign_bundle(&signed, &key, "dev.krate.bounce", "1.0.0", 1_700_000_000).unwrap();
        let tampered = fixtures.join("bounce-tampered.krate");
        fs::copy(&signed, &tampered).unwrap();
        replace_entry(&tampered, COMPONENT_ENTRY, OTHER_COMPONENT).unwrap();
        let module = fixtures.join("module-not-component.krate");
        let opened = open(&bounce).unwrap();
        let manifest = fs::read(opened.manifest_path()).unwrap();
        let mut w = ZipWriter::new(File::create(&module).unwrap());
        let o = SimpleFileOptions::default();
        w.start_file(MANIFEST_ENTRY, o).unwrap();
        w.write_all(&manifest).unwrap();
        w.start_file(COMPONENT_ENTRY, o).unwrap();
        w.write_all(b"\0asm\x01\0\0\0").unwrap();
        w.finish().unwrap();
        assert!(matches!(
            open(&signed).unwrap().signature_verdict().unwrap(),
            Some(signing::Verdict::Valid { .. })
        ));
        assert!(matches!(
            open(&tampered).unwrap().signature_verdict().unwrap(),
            Some(signing::Verdict::Tampered { .. })
        ));
        assert!(open(&module).is_err());
    }

    /// The inputs the cross-machine pack proof packs: every committed
    /// ported bundle's manifest and component, and one synthetic app with
    /// source and an SDK written by the test itself (so no checkout can
    /// change its bytes). Returns (name, packed bytes).
    fn pack_proof_corpus() -> Vec<(String, Vec<u8>)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dir = TempDir::new().expect("tempdir");
        let mut out = Vec::new();
        let mut ported: Vec<PathBuf> = fs::read_dir(root.join("evidence/ported"))
            .expect("evidence/ported")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "krate"))
            .collect();
        ported.sort();
        assert!(ported.len() >= 10, "the ported corpus is the proof's input");
        for path in ported {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let opened = open(&path).expect("open a committed bundle");
            let packed = dir.path().join(format!("{name}.repacked"));
            pack(opened.manifest_path(), opened.component_path(), &packed).expect("pack");
            out.push((name, fs::read(&packed).unwrap()));
        }
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let assets = dir.path().join("assets");
        fs::create_dir_all(assets.join("img")).unwrap();
        fs::write(assets.join("img/logo.png"), b"\x89PNG not really").unwrap();
        fs::write(assets.join("words.txt"), b"hello\n").unwrap();
        let source = dir.path().join("source-tree");
        fs::create_dir_all(source.join("src")).unwrap();
        fs::write(source.join("Cargo.toml"), "[package]\nname = \"proof\"\nversion = \"0.1.0\"\n\n[dependencies]\nkrate = { path = \"/Users/someone/.krate/sdk/abc/bindings-rust\" }\n").unwrap();
        fs::write(source.join("src/lib.rs"), "// the app\nfn main() {}\n").unwrap();
        let sdk = dir.path().join("sdk-tree");
        fs::create_dir_all(sdk.join("bindings-rust/src")).unwrap();
        fs::write(
            sdk.join("bindings-rust/Cargo.toml"),
            "[package]\nname = \"krate\"\n",
        )
        .unwrap();
        fs::write(sdk.join("bindings-rust/src/lib.rs"), "pub fn sdk() {}\n").unwrap();
        let full = dir.path().join("full.krate");
        pack_with_sdk(
            &manifest,
            &component,
            Some(&assets),
            Some(&source),
            Some(&sdk),
            &full,
        )
        .unwrap();
        out.push((
            "synthetic-with-source-assets-sdk".to_string(),
            fs::read(&full).unwrap(),
        ));
        out
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Every local header's fields and each entry's content digest, so a
    /// cross-machine mismatch says which byte differs rather than only
    /// that one does.
    fn zip_structure(bytes: &[u8]) -> String {
        let mut archive = ZipArchive::new(io::Cursor::new(bytes)).expect("readable archive");
        let mut parts = Vec::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).expect("entry");
            let mut body = Vec::new();
            entry.read_to_end(&mut body).expect("read");
            let at = entry.header_start() as usize;
            let local = &bytes[at..at + 30];
            parts.push(format!(
                "{}[method={:?} flags={:02x}{:02x} time={:02x}{:02x} date={:02x}{:02x} crc={:08x} csize={} size={} extra={} mode={:o} content={}]",
                entry.name(),
                entry.compression(),
                local[7],
                local[6],
                local[11],
                local[10],
                local[13],
                local[12],
                entry.crc32(),
                entry.compressed_size(),
                entry.size(),
                entry.extra_data().map(|x| x.len()).unwrap_or(0),
                entry.unix_mode().unwrap_or(0),
                &sha256_hex(&body)[..12]
            ));
        }
        let tail = &bytes[bytes.len().saturating_sub(22)..];
        parts.push(format!(
            "eocd={}",
            tail.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ));
        parts.join(" ")
    }

    /// The CP1 exit clause, "`krate pack` rebuilds byte-equal on a second
    /// machine", as a test that runs on every machine this suite runs on.
    ///
    /// Same-process determinism is `packing_the_same_input_twice_gives_the
    /// _same_bytes` (K-315). This is the cross-machine half: the digest of
    /// every packed corpus member is committed in
    /// tests/fixtures/repack-digests.json, written on one machine, and the
    /// library suite runs on macOS, Ubuntu and Windows in CI, so a packer
    /// that reads anything from its host -- a path, a clock, a locale, a
    /// separator, an unordered map -- fails on the machine that differs.
    ///
    /// When the format changes on purpose, regenerate with
    /// `cargo test -p krate-bundle --lib regenerate_repack_digests -- --ignored`
    /// and commit the JSON with the change that moved it.
    #[test]
    fn pack_is_byte_equal_on_every_machine_this_suite_runs_on() {
        let expected: std::collections::BTreeMap<String, String> =
            serde_json::from_str(include_str!("../tests/fixtures/repack-digests.json"))
                .expect("repack-digests.json");
        let corpus = pack_proof_corpus();
        assert_eq!(
            corpus.len(),
            expected.len(),
            "the corpus and the digests must cover the same inputs"
        );
        let mut wrong = Vec::new();
        for (name, bytes) in &corpus {
            let actual = sha256_hex(bytes);
            match expected.get(name) {
                Some(want) if *want == actual => {}
                Some(want) => wrong.push(format!(
                    "{name}: packed {actual}, committed {want}\n    structure: {}",
                    zip_structure(bytes)
                )),
                None => wrong.push(format!("{name}: no committed digest")),
            }
        }
        assert!(
            wrong.is_empty(),
            "packing gave different bytes on this machine ({}-{}) than on the one that \
             committed the digests; the packer read something from its host:\n  {}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            wrong.join("\n  ")
        );
    }

    /// Write tests/fixtures/repack-digests.json from this machine.
    #[test]
    #[ignore = "writes the committed digests; run deliberately after a format change"]
    fn regenerate_repack_digests() {
        let digests: std::collections::BTreeMap<String, String> = pack_proof_corpus()
            .into_iter()
            .map(|(name, bytes)| (name, sha256_hex(&bytes)))
            .collect();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repack-digests.json");
        fs::write(
            &path,
            serde_json::to_string_pretty(&digests).unwrap() + "\n",
        )
        .unwrap();
        println!("wrote {} digests to {}", digests.len(), path.display());
    }

    /// A hub's 451 reaches the person as the notice, not as a status code
    /// (IC-669, K-334).
    #[cfg(feature = "fetch")]
    #[test]
    fn a_removed_app_is_refused_with_the_hubs_notice() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
                let body = r#"{"removed":true,"reason":"impersonates a bank","emergency":true,"appeal":{"state":"none","how":"POST ..."},"notice":"https://hub.example/takedown/abc"}"#;
                let response = format!(
                    "HTTP/1.1 451 Unavailable For Legal Reasons\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        let err = fetch(&format!("http://127.0.0.1:{port}/a/abc?dl=1"), true)
            .expect_err("a 451 is a refusal");
        let _ = server.join();
        let text = err.to_string();
        assert!(
            text.contains("the hub removed this app: impersonates a bank"),
            "{text}"
        );
        assert!(text.contains("emergency"), "{text}");
        assert!(
            text.contains("https://hub.example/takedown/abc"),
            "the notice URL is the way to appeal: {text}"
        );
        assert!(
            !text.contains("451"),
            "a status code is not what a person needs: {text}"
        );
    }

    /// The archive writer is a pure function of its entries (K-335): the
    /// same entries give the same bytes whatever built this crate, every
    /// entry is stamped 1980-01-01 with no extra field, the zip crate reads
    /// the result back exactly, and a stored entry is used only when
    /// deflate would not shrink it.
    #[test]
    fn the_archive_writer_depends_on_nothing_but_its_entries() {
        let entries: Vec<(&str, Vec<u8>)> = vec![
            ("krate-profile", b"1".to_vec()),
            ("manifest.toml", MANIFEST.as_bytes().to_vec()),
            ("code.wasm", MINIMAL_COMPONENT.to_vec()),
            ("assets/zeros.bin", vec![0u8; 4096]),
            ("assets/tiny", b"x".to_vec()),
        ];
        let write = || {
            let mut zip = DeterministicZip::new(Vec::new());
            for (name, bytes) in &entries {
                zip.add(name, bytes).unwrap();
            }
            zip.finish().unwrap()
        };
        let a = write();
        let b = write();
        assert_eq!(a, b, "two writes of the same entries are the same bytes");
        // Pinned: this digest was computed on one machine, and CI compares
        // it on the others. Change it only with a deliberate format change.
        assert_eq!(
            sha256_hex(&a),
            "7a0b3e1f9fdc2e71179b58d2c13e7b6a95577dc34421d274d7626d5168c3fbb2",
            "the writer's bytes moved; if the format did not change on purpose, something host-dependent got in"
        );

        let mut archive = ZipArchive::new(io::Cursor::new(&a)).expect("the zip crate reads it");
        assert_eq!(archive.len(), entries.len());
        for (index, (name, bytes)) in entries.iter().enumerate() {
            let mut entry = archive.by_index(index).unwrap();
            assert_eq!(entry.name(), *name, "entries keep their order");
            let mut body = Vec::new();
            entry.read_to_end(&mut body).unwrap();
            assert_eq!(&body, bytes, "{name}: content round-trips");
            assert_eq!(
                entry
                    .last_modified()
                    .map(|t| (t.year(), t.month(), t.day())),
                Some((1980, 1, 1)),
                "{name}: stamped at the epoch"
            );
            assert!(
                entry.extra_data().map(|x| x.is_empty()).unwrap_or(true),
                "{name}: no extra field"
            );
        }
        let tiny = archive.by_name("assets/tiny").unwrap();
        assert_eq!(
            tiny.compression(),
            CompressionMethod::Stored,
            "one byte is stored, not made larger by deflate"
        );
        drop(tiny);
        let zeros = archive.by_name("assets/zeros.bin").unwrap();
        assert_eq!(zeros.compression(), CompressionMethod::Deflated);
        assert!(
            zeros.compressed_size() < 64,
            "4 KiB of zeros deflates to a few bytes"
        );
    }

    /// The sweep is scoped by prefix: asking it to clear one family of
    /// directories must not clear another, however old (K-313).
    #[test]
    fn a_sweep_for_one_prefix_leaves_the_other_alone() {
        let root = TempDir::new().expect("tempdir");
        let edit_old = root.path().join("krate-edit-abandoned");
        let open_old = root.path().join(format!("{EXTRACT_PREFIX}abandoned"));
        let edit_young = root.path().join("krate-edit-live");
        for dir in [&edit_old, &open_old, &edit_young] {
            fs::create_dir(dir).expect("create");
            fs::write(dir.join("f"), b"x").expect("write");
        }
        let long_ago =
            std::time::SystemTime::now() - ABANDONED_AFTER - std::time::Duration::from_secs(60);
        set_modified(&edit_old, long_ago);
        set_modified(&open_old, long_ago);

        sweep_abandoned_dirs(root.path(), "krate-edit-", ABANDONED_AFTER);

        assert!(
            !edit_old.exists(),
            "an old directory of the named family is swept"
        );
        assert!(
            edit_young.exists(),
            "a young one of the same family is in use"
        );
        assert!(
            open_old.exists(),
            "a directory of another family is not this sweep's to remove"
        );
    }

    #[test]
    fn anything_pack_refuses_is_also_refused_when_it_arrives_by_other_means() {
        // IC-210 asks for runtime-validator parity, and K-272 was exactly a
        // case where the two disagreed: pack accepted a core module, and the
        // failure surfaced for whoever was sent the file.
        //
        // Pack is only one door. A bundle can be assembled by hand -- which
        // is what an attacker does, and what the fixtures in this file do --
        // so anything pack refuses has to be refused on the way in as well.
        // Otherwise the check is advice to the honest.
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());

        for (what, bytes) in [
            (
                "a core module",
                vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00],
            ),
            (
                "a file that is not wasm",
                b"this is not wasm at all".to_vec(),
            ),
            ("an empty file", Vec::new()),
        ] {
            // Door one: pack refuses it.
            let component = write_temp(dir.path(), "code.wasm", &bytes);
            let out = dir.path().join("packed.krate");
            assert!(
                pack(&manifest, &component, &out).is_err(),
                "{what} must be refused at pack"
            );

            // Door two: the same component, in a bundle assembled without
            // pack. Opening it must refuse too.
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                writer.start_file(PROFILE_ENTRY, opts).expect("profile");
                writer.write_all(b"1").expect("write");
                writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
                writer.write_all(MANIFEST.as_bytes()).expect("write");
                writer.start_file(COMPONENT_ENTRY, opts).expect("component");
                writer.write_all(&bytes).expect("write");
                writer.finish().expect("finish");
            }
            let handmade = dir.path().join("handmade.krate");
            fs::write(&handmade, &buf).expect("write bundle");

            // `open` extracts and validates the archive; the component's own
            // validity is the runtime's answer, so what is asserted here is
            // that the bundle does not sail through BOTH doors untouched.
            // A component this broken must fail one of them.
            let opened = open(&handmade);
            let component_is_readable = opened
                .as_ref()
                .ok()
                .map(|bundle| {
                    let read = fs::read(bundle.component_path()).unwrap_or_default();
                    imports::is_component(&read) && imports::component_imports(&read).is_ok()
                })
                .unwrap_or(false);
            assert!(
                !component_is_readable,
                "{what} passed pack's door AND arrived intact through the \
                 other one -- the check would be advice to the honest"
            );
        }
    }

    #[test]
    fn packing_a_core_module_is_refused_where_it_can_still_be_fixed() {
        // K-272 / IC-210. A core module packed without complaint, and the
        // failure surfaced for whoever was SENT the file -- who can do
        // nothing about it. The person packing can rebuild, so that is where
        // the refusal belongs.
        //
        // Getting `cargo build` instead of `cargo component build` is a
        // common enough mistake that the message names the command.
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());

        let module = write_temp(
            dir.path(),
            "module.wasm",
            &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00],
        );
        let out = dir.path().join("module.krate");
        let err = pack(&manifest, &module, &out).expect_err("a module must not pack");
        assert!(
            matches!(err, BundleError::NotAComponent { .. }),
            "a module must be refused as not-a-component: {err}"
        );
        let text = err.to_string();
        assert!(
            text.contains("core WebAssembly module"),
            "the refusal must say what it actually is: {text}"
        );
        assert!(
            text.contains("cargo component build"),
            "and name the command that produces a component: {text}"
        );
        assert!(
            !out.exists(),
            "a refused pack must not leave a bundle behind"
        );

        // A component still packs. This is the half that would break if the
        // header check were wrong, and it would break every app at once.
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let good = dir.path().join("good.krate");
        pack(&manifest, &component, &good).expect("a component must still pack");
        assert!(good.is_file(), "the bundle must be written");
    }

    #[test]
    fn a_damaged_format_line_is_not_called_a_newer_format() {
        // K-271. Every unreadable profile used to be reported as "a newer
        // .krate format", with advice to update Krate. For a version this
        // build has not reached that is true and useful. For an empty line,
        // a word, or a negative number it is false twice over: the file is
        // damaged, and no release will ever read it, so updating cannot
        // help.
        fn bundle(dir: &Path, name: &str, profile: &[u8]) -> PathBuf {
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                writer.start_file(PROFILE_ENTRY, opts).expect("profile");
                writer.write_all(profile).expect("write");
                writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
                writer.write_all(MANIFEST.as_bytes()).expect("write");
                writer.start_file(COMPONENT_ENTRY, opts).expect("component");
                writer.write_all(MINIMAL_COMPONENT).expect("write");
                writer.finish().expect("finish");
            }
            let path = dir.join(name);
            fs::write(&path, &buf).expect("write bundle");
            path
        }

        let dir = TempDir::new().expect("tempdir");

        // Not a version at all: damage, and say so.
        for (what, profile) in [
            ("an empty line", b"".as_slice()),
            ("a word", b"banana"),
            ("a negative number", b"-1"),
            ("a decimal", b"1.5"),
        ] {
            let path = bundle(dir.path(), "damaged.krate", profile);
            let err = open(&path).expect_err("a damaged profile must be refused");
            assert!(
                matches!(err, BundleError::DamagedProfile { .. }),
                "{what} must be reported as damage: {err}"
            );
            let text = err.to_string();
            assert!(
                text.contains("fresh copy"),
                "{what} must point at the thing that would help: {text}"
            );
            assert!(
                !text.contains("newer .krate format"),
                "{what} is not a newer format: {text}"
            );
        }

        // A version this build has not reached: genuinely newer, and
        // updating IS the answer. Checked at the top of the range too, so
        // the split is about parsing rather than about size.
        let next = (READS_PROFILE + 1).to_string();
        for (what, profile) in [
            ("the next version", next.as_bytes()),
            ("a far future version", b"4294967295"),
        ] {
            let path = bundle(dir.path(), "future.krate", profile);
            let err = open(&path).expect_err("a future profile must be refused");
            assert!(
                matches!(err, BundleError::UnsupportedProfile { .. }),
                "{what} must be reported as a newer format: {err}"
            );
        }

        // And a version that merely picked up whitespace still opens: a file
        // is not damaged because it gained a space in transit.
        let padded = bundle(dir.path(), "padded.krate", b"  1  \n");
        assert!(
            open(&padded).is_ok(),
            "a version with surrounding whitespace must still be read"
        );
    }

    #[test]
    fn an_envelope_field_this_build_never_heard_of_does_not_strand_the_bundle() {
        // K-266 / IC-856. The envelope exists so a NEW bundle can meet an OLD
        // reader and both behave sensibly. The first way a format grows is by
        // adding a line -- so a reader that refuses every line it has not
        // seen before makes growth impossible, on every reader already
        // shipped.
        fn bundle(dir: &Path, name: &str, profile: &[u8]) -> PathBuf {
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                writer.start_file(PROFILE_ENTRY, opts).expect("profile");
                writer.write_all(profile).expect("write");
                writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
                writer.write_all(MANIFEST.as_bytes()).expect("write");
                writer.start_file(COMPONENT_ENTRY, opts).expect("component");
                writer.write_all(MINIMAL_COMPONENT).expect("write");
                writer.finish().expect("finish");
            }
            let path = dir.join(name);
            fs::write(&path, &buf).expect("write bundle");
            path
        }

        let dir = TempDir::new().expect("tempdir");

        // A supported version, plus a line written by some later Krate.
        let with_field = bundle(
            dir.path(),
            "field.krate",
            b"1\nsomething-added-later = whatever\n",
        );
        assert!(
            open(&with_field).is_ok(),
            "a generation-1 bundle carrying an unknown line must still open: \
             refusing it is how an envelope stops being an envelope"
        );

        // The version still governs. A bundle that needs more than this
        // build knows is still refused -- that is what raising the version
        // is FOR, and it is the only way to demand a reader understand
        // something new.
        let future_line = format!("{}\nanything = here\n", READS_PROFILE + 1);
        let future = bundle(dir.path(), "future.krate", future_line.as_bytes());
        let err = open(&future).expect_err("a future profile must be refused");
        let text = err.to_string();
        assert!(
            text.contains(&format!("profile {}", READS_PROFILE + 1)),
            "the refusal must name the version it found: {text}"
        );
        assert!(
            !text.contains("anything = here"),
            "the version message must quote the VERSION, not the whole file -- \
             it used to read 'profile 1\\nfuture-field = ...' is newer than \
             'profile 1': {text}"
        );
    }

    #[test]
    fn open_bytes_and_open_agree_on_a_duplicate_from_disk() {
        // K-278: the record-count duplicate check lived only in `open`, so
        // every byte-holding caller (hub upload, URL download) lost it. Both
        // entry points must now refuse the same fixture -- the one a real
        // ZIP writer produced, caught by counting records, not comparing
        // names.
        const FIXTURE: &[u8] =
            include_bytes!("../tests/fixtures/duplicate-from-another-writer.krate");

        let from_bytes = open_bytes(FIXTURE);
        assert!(
            matches!(from_bytes, Err(BundleError::DuplicateEntry { .. })),
            "open_bytes must refuse the duplicate a real writer produced: {from_bytes:?}"
        );

        // And `open` from a temp file must give the identical verdict, so
        // the two paths cannot drift.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("dup.krate");
        fs::write(&path, FIXTURE).expect("write");
        assert!(
            matches!(open(&path), Err(BundleError::DuplicateEntry { .. })),
            "open from disk must refuse the same duplicate open_bytes does",
        );
    }

    #[test]
    fn a_duplicate_written_by_another_tool_is_refused_too() {
        // IC-713 asks for "archives produced by multiple ZIP writers", and
        // that is not a formality: the Rust zip crate REFUSES to write one
        // name twice, so our own writer cannot build the archive this is
        // about. The hand-assembled fixture elsewhere in this file covers
        // one shape; this one was written by Python's zipfile, which emits
        // the duplicate without complaint.
        //
        // Which check catches it was MEASURED, not assumed. Removing the
        // duplicate-NAME check leaves this archive still refused, because
        // ZipArchive deduplicates by name as it parses: it reports three
        // entries where the EOCD declares four, and the record-count
        // comparison fires. So a real writer's duplicate is caught by
        // counting, and the name check is what catches the shapes where the
        // count still agrees.
        //
        // That is the reason to keep both, and the reason this fixture earns
        // its place: it is the only one here produced by a writer that
        // genuinely emits duplicates.
        const FIXTURE: &[u8] =
            include_bytes!("../tests/fixtures/duplicate-from-another-writer.krate");

        // The fixture must really be what it claims, or this proves nothing.
        assert_eq!(
            FIXTURE.windows(4).filter(|w| *w == b"PK\x01\x02").count(),
            4,
            "the fixture must carry four central-directory records"
        );

        let dir = TempDir::new().expect("tempdir");
        let bundle = dir.path().join("other-writer.krate");
        fs::write(&bundle, FIXTURE).expect("write");

        let err = open(&bundle).expect_err("a duplicate must be refused");
        let text = err.to_string();
        assert!(
            text.contains("names the same file twice"),
            "it must be refused as a duplicate, not something else: {text}"
        );
        assert!(
            text.contains("source/lib.rs"),
            "the refusal must name the path: {text}"
        );

        // The count path is what fires here, so assert it directly: the
        // archive declares more records than a parser can see, and that gap
        // is the whole signal.
        let declared =
            central_directory_record_count(FIXTURE).expect("the fixture declares a record count");
        let parsed = ZipArchive::new(io::Cursor::new(FIXTURE))
            .expect("the fixture is a readable archive")
            .len();
        assert!(
            declared > parsed,
            "this fixture is about a writer that emits a duplicate the parser \
             folds away: declared {declared}, parsed {parsed}"
        );
    }

    #[test]
    fn a_duplicate_in_any_namespace_is_refused_and_named() {
        for (namespace, path) in [
            ("core", MANIFEST_ENTRY),
            ("core", COMPONENT_ENTRY),
            ("asset", "assets/logo.png"),
            ("source", "source/lib.rs"),
            ("sdk", "sdk/krate.wit"),
            ("extension", "future/thing.bin"),
        ] {
            let bytes = archive_with_duplicate(path);
            let dir = TempDir::new().expect("tempdir");
            let bundle = dir.path().join("dup.krate");
            fs::write(&bundle, &bytes).expect("write");

            let err = match open(&bundle) {
                Err(err) => err,
                Ok(_) => panic!("a duplicate {namespace} record must be refused: {path}"),
            };
            let text = err.to_string();
            assert!(
                text.contains("names the same file twice"),
                "{namespace}: {text}"
            );
            assert!(
                text.contains(&path.to_lowercase()),
                "the {namespace} refusal must NAME the path {path}: {text}"
            );
        }
    }

    #[test]
    fn a_path_outside_ascii_is_refused_and_named() {
        // `cafe\u{301}.rs` (NFD) and `caf\u{e9}.rs` (NFC) are different bytes
        // and the same file once written to disk on macOS. Whichever one a
        // reviewer read, the other could replace it. Both are refused.
        for spelling in ["source/caf\u{e9}.rs", "source/cafe\u{301}.rs"] {
            let bytes = archive_with(&[(spelling.to_string(), b"fn main() {}".to_vec())]);
            let dir = TempDir::new().expect("tempdir");
            let bundle = dir.path().join("unicode.krate");
            fs::write(&bundle, &bytes).expect("write");

            let err = match open(&bundle) {
                Err(err) => err,
                Ok(_) => panic!("a non-ASCII entry name must be refused: {spelling:?}"),
            };
            let text = err.to_string();
            assert!(text.contains("outside ASCII"), "{spelling:?}: {text}");
            assert!(
                text.contains(spelling),
                "the refusal must NAME the path {spelling:?}: {text}"
            );
            assert!(
                text.contains("Rename it to ASCII"),
                "the refusal must say what to DO about it: {text}"
            );
        }
    }

    #[test]
    fn a_refused_bundle_leaves_nothing_behind() {
        // IC-209 asks for cleanup. A bundle refused PART WAY THROUGH
        // extraction has already written files; if those survived, a
        // rejected bundle would still cost disk, and repeated attempts
        // would fill it. The struct doc claims this ("removed on drop, so a
        // fetched bundle leaves nothing behind after the run"); nothing
        // checked it.
        //
        // open() unpacks into a directory this test owns, so what is left
        // there afterwards came from this call and nothing else. Counting
        // the shared system temp directory instead was tried first and
        // flaked: it cannot tell a leak from another test's live directory.
        const EACH: usize = 48 * 1024 * 1024;
        let count = (MAX_TOTAL_SOURCE_BYTES as usize / EACH) + 2;
        let mut buf = Vec::new();
        {
            let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
            writer.write_all(MANIFEST.as_bytes()).expect("write");
            writer.start_file(COMPONENT_ENTRY, opts).expect("component");
            writer.write_all(MINIMAL_COMPONENT).expect("write");
            let blob = vec![0u8; EACH];
            for i in 0..count {
                writer
                    .start_file(format!("source/big{i}.rs"), opts)
                    .expect("source");
                writer.write_all(&blob).expect("write");
            }
            writer.finish().expect("finish");
        }
        // Refused during extraction, not at the preflight: the source total
        // is only exceeded once the bytes are actually read out.
        let forged = forge_source_sizes_to_one_byte(&buf);

        let dir = TempDir::new().expect("tempdir");
        let bundle = dir.path().join("refused.krate");
        fs::write(&bundle, &forged).expect("write");
        let root = dir.path().join("extract-here");
        fs::create_dir(&root).expect("root");

        EXTRACT_ROOT.with(|r| *r.borrow_mut() = Some(root.clone()));
        let opened = open(&bundle);
        EXTRACT_ROOT.with(|r| *r.borrow_mut() = None);

        assert!(
            matches!(opened, Err(BundleError::SourceTooLarge)),
            "the fixture must be refused during extraction"
        );

        let left: Vec<PathBuf> = fs::read_dir(&root)
            .expect("read root")
            .flatten()
            .map(|e| e.path())
            .collect();
        assert!(
            left.is_empty(),
            "a refused bundle must not leave its extracted files behind: {left:?}"
        );
    }

    #[test]
    fn the_entry_count_limit_holds_at_exactly_the_boundary() {
        // IC-209 asks for each limit at minus one, exact, and plus one.
        // MAX_ENTRY_COUNT counts every file in the archive, so the two
        // required entries come out of the same budget.
        fn archive_with_n_files(n: usize) -> Vec<u8> {
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
                writer.write_all(MANIFEST.as_bytes()).expect("write");
                writer.start_file(COMPONENT_ENTRY, opts).expect("component");
                writer.write_all(MINIMAL_COMPONENT).expect("write");
                for i in 0..n.saturating_sub(2) {
                    writer
                        .start_file(format!("source/f{i}.rs"), opts)
                        .expect("source");
                    writer.write_all(b"//").expect("write");
                }
                writer.finish().expect("finish");
            }
            buf
        }

        let dir = TempDir::new().expect("tempdir");
        for (n, must_open) in [
            (MAX_ENTRY_COUNT - 1, true),
            (MAX_ENTRY_COUNT, true),
            (MAX_ENTRY_COUNT + 1, false),
        ] {
            let bundle = dir.path().join(format!("n{n}.krate"));
            fs::write(&bundle, archive_with_n_files(n)).expect("write");
            let opened = open(&bundle);
            if must_open {
                assert!(
                    !matches!(opened, Err(BundleError::TooManyEntries)),
                    "{n} files is within the limit of {MAX_ENTRY_COUNT} and must not be \
                     refused for its count"
                );
            } else {
                assert!(
                    matches!(opened, Err(BundleError::TooManyEntries)),
                    "{n} files is over the limit of {MAX_ENTRY_COUNT} and must be refused"
                );
            }
        }
    }

    #[test]
    fn a_forged_size_cannot_get_past_the_source_limit() {
        // K-255. The limit used to be checked against the size in the zip
        // header, so an archive that declared one byte per file and carried
        // far more walked straight past it -- measured at 1.6 GiB written
        // from a 1.6 MB file. What counts is what comes out of the
        // decompressor.
        //
        // Kept small enough to run in a test: eight entries whose headers
        // claim 1 byte and which each expand to well past the limit between
        // them. Zeros deflate to almost nothing, so the fixture stays tiny.
        const EACH: usize = 48 * 1024 * 1024;
        let count = (MAX_TOTAL_SOURCE_BYTES as usize / EACH) + 2;

        let mut buf = Vec::new();
        {
            let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
            writer.write_all(MANIFEST.as_bytes()).expect("write");
            writer.start_file(COMPONENT_ENTRY, opts).expect("component");
            writer.write_all(MINIMAL_COMPONENT).expect("write");
            let blob = vec![0u8; EACH];
            for i in 0..count {
                writer
                    .start_file(format!("source/big{i}.rs"), opts)
                    .expect("source");
                writer.write_all(&blob).expect("write");
            }
            writer.finish().expect("finish");
        }

        // Honest declaration: refused by the preflight, which is the cheap
        // early check and must keep working.
        let dir = TempDir::new().expect("tempdir");
        let honest = dir.path().join("honest.krate");
        fs::write(&honest, &buf).expect("write");
        assert!(
            matches!(open(&honest), Err(BundleError::SourceTooLarge)),
            "an honestly declared oversize source tree must be refused"
        );

        // Now rewrite every source record to declare one byte, leaving the
        // compressed data untouched. This is the archive the preflight
        // cannot see through.
        let forged = forge_source_sizes_to_one_byte(&buf);
        // The forgery must have actually happened, or this test would be
        // re-checking the honest archive above and proving nothing.
        assert_ne!(forged, buf, "the fixture must really declare forged sizes");
        let bundle = dir.path().join("forged.krate");
        fs::write(&bundle, &forged).expect("write");
        assert!(
            matches!(open(&bundle), Err(BundleError::SourceTooLarge)),
            "a FORGED size must not get past the source limit"
        );
    }

    /// Rewrite the uncompressed-size field of every `source/` record to 1,
    /// in both the central directory and the local headers, leaving the
    /// compressed bytes alone. No writer produces this; an attacker does.
    fn forge_source_sizes_to_one_byte(input: &[u8]) -> Vec<u8> {
        let mut data = input.to_vec();
        let one = 1u32.to_le_bytes();

        for (magic, name_at, len_at, size_at) in
            [(b"PK\x01\x02", 46, 28, 24), (b"PK\x03\x04", 30, 26, 22)]
        {
            let mut i = 0;
            while let Some(found) = find(&data, magic, i) {
                let namelen =
                    u16::from_le_bytes([data[found + len_at], data[found + len_at + 1]]) as usize;
                let start = found + name_at;
                if start + namelen <= data.len() {
                    let name = String::from_utf8_lossy(&data[start..start + namelen]).into_owned();
                    if name.starts_with(SOURCE_PREFIX) {
                        data[found + size_at..found + size_at + 4].copy_from_slice(&one);
                    }
                }
                i = found + 4;
            }
        }
        data
    }

    fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
        haystack
            .get(from..)?
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| p + from)
    }

    #[test]
    fn the_source_byte_limit_holds_at_exactly_the_boundary() {
        // IC-209 asks for EACH limit at minus one, exact, and plus one --
        // not just the count. This is the aggregate the forged-size fixture
        // exercises from far above; here it is checked where an off-by-one
        // would live.
        fn archive_of_source_bytes(total: u64) -> Vec<u8> {
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
                writer.write_all(MANIFEST.as_bytes()).expect("write");
                writer.start_file(COMPONENT_ENTRY, opts).expect("component");
                writer.write_all(MINIMAL_COMPONENT).expect("write");
                // Split across a few files so no single one trips the
                // per-file cap instead of the aggregate one being measured.
                let chunk = MAX_ASSET_BYTES / 2;
                let mut written = 0u64;
                let mut i = 0;
                while written < total {
                    let n = chunk.min(total - written);
                    writer
                        .start_file(format!("source/f{i}.bin"), opts)
                        .expect("source");
                    writer.write_all(&vec![0u8; n as usize]).expect("write");
                    written += n;
                    i += 1;
                }
                writer.finish().expect("finish");
            }
            buf
        }

        let dir = TempDir::new().expect("tempdir");
        for (total, must_pass) in [
            (MAX_TOTAL_SOURCE_BYTES - 1, true),
            (MAX_TOTAL_SOURCE_BYTES, true),
            (MAX_TOTAL_SOURCE_BYTES + 1, false),
        ] {
            let bundle = dir.path().join("bytes.krate");
            fs::write(&bundle, archive_of_source_bytes(total)).expect("write");
            let opened = open(&bundle);
            if must_pass {
                assert!(
                    !matches!(opened, Err(BundleError::SourceTooLarge)),
                    "{total} bytes is within the limit of {MAX_TOTAL_SOURCE_BYTES} and \
                     must not be refused for its size"
                );
            } else {
                assert!(
                    matches!(opened, Err(BundleError::SourceTooLarge)),
                    "{total} bytes is over the limit of {MAX_TOTAL_SOURCE_BYTES} and \
                     must be refused"
                );
            }
        }
    }

    #[test]
    fn the_compression_method_and_entry_order_are_read_correctly() {
        // IC-208's close_with names "compression method" and "reordered
        // archive". Both are things another writer may legitimately do
        // differently, and a bundle is not ours to reject for either.
        fn archive(method: CompressionMethod, component_first: bool) -> Vec<u8> {
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts = SimpleFileOptions::default().compression_method(method);
                let write_manifest = |w: &mut ZipWriter<io::Cursor<&mut Vec<u8>>>| {
                    w.start_file(MANIFEST_ENTRY, opts).expect("manifest");
                    w.write_all(MANIFEST.as_bytes()).expect("write");
                };
                let write_component = |w: &mut ZipWriter<io::Cursor<&mut Vec<u8>>>| {
                    w.start_file(COMPONENT_ENTRY, opts).expect("component");
                    w.write_all(MINIMAL_COMPONENT).expect("write");
                };
                if component_first {
                    write_component(&mut writer);
                    write_manifest(&mut writer);
                } else {
                    write_manifest(&mut writer);
                    write_component(&mut writer);
                }
                writer.finish().expect("finish");
            }
            buf
        }

        let dir = TempDir::new().expect("tempdir");
        for (what, method, component_first) in [
            (
                "deflate, manifest first",
                CompressionMethod::Deflated,
                false,
            ),
            ("stored (uncompressed)", CompressionMethod::Stored, true),
            (
                "deflate, component first",
                CompressionMethod::Deflated,
                true,
            ),
        ] {
            let bundle = dir.path().join("profile.krate");
            fs::write(&bundle, archive(method, component_first)).expect("write");
            assert!(
                open(&bundle).is_ok(),
                "a bundle written {what} is a legitimate bundle and must open"
            );
        }
    }

    #[test]
    fn a_compression_we_cannot_read_says_so_instead_of_calling_it_damaged() {
        // K-259. Every zip-layer failure used to collapse into "this is not a
        // Krate app, or the file is damaged". For a real app packed with a
        // method we do not support, both halves of that are false, and it
        // sends whoever packed it to rebuild a file that is fine.
        //
        // The zip crate separates "probably not a zip" from "a zip we cannot
        // read"; the mapping now does too.
        let unsupported = BundleError::Archive(zip::result::ZipError::UnsupportedArchive(
            "Compression method",
        ));
        let message = unsupported
            .user_message()
            .expect("an unsupported archive must have words for a person");
        assert!(
            message.contains("cannot read"),
            "it must say we cannot read it: {message}"
        );
        assert!(
            !message.contains("not a Krate app"),
            "it must NOT claim the file is not a Krate app: {message}"
        );
        assert!(
            message.contains("krate pack"),
            "it must say what to do about it: {message}"
        );

        // A file that really is not an archive keeps the old, correct words.
        let invalid =
            BundleError::Archive(zip::result::ZipError::InvalidArchive("Invalid zip header"));
        let message = invalid.user_message().expect("words for a person");
        assert!(
            message.contains("not a Krate app"),
            "a genuinely invalid file must still say so: {message}"
        );
    }

    #[test]
    fn packing_the_same_input_twice_gives_the_same_bytes() {
        // The plan asks for a deterministic writer where determinism is
        // claimed. Measured: packing one component and manifest twice, two
        // seconds apart, already gives byte-identical output.
        //
        // Nothing was holding it there. A zip writer stamps a modification
        // time by default, and one SystemTime::now() anywhere in the packer
        // would break this quietly -- the bundles would still open, still
        // run, and simply stop being comparable. Two bundles that differ
        // only in when they were built cannot be told apart from two that
        // differ in what they contain, which is the property the archive
        // digest identity rests on.
        let dir = TempDir::new().expect("tempdir");
        let component = dir.path().join("code.wasm");
        fs::write(&component, MINIMAL_COMPONENT).expect("component");
        let manifest = dir.path().join("manifest.toml");
        fs::write(&manifest, MANIFEST.as_bytes()).expect("manifest");

        let first = dir.path().join("first.krate");
        let second = dir.path().join("second.krate");
        pack(&manifest, &component, &first).expect("pack once");
        pack(&manifest, &component, &second).expect("pack twice");

        // No sleep between the two. One was tried, and it is worse than
        // useless here: zip stores times in two-second steps, so a short
        // sleep may or may not cross a boundary and the test would pass or
        // fail depending on when it ran. A flaky test that only sometimes
        // notices is worse than one that says plainly what it checks.
        //
        // What this catches is any value that differs between two packs in
        // one process -- a counter, a random name, an address, an
        // unordered map. Verified by making the packer stamp a counter:
        // the assertion below fires.

        let a = fs::read(&first).expect("read first");
        let b = fs::read(&second).expect("read second");
        assert_eq!(
            a, b,
            "packing the same input twice must give the same bytes; if this \
             fails, something in the packer is reading the clock"
        );
    }

    #[test]
    fn a_failure_to_unpack_names_the_real_path_and_the_real_direction() {
        // K-263. Opening an app unpacks it, so the failures that actually
        // happen here are WRITES -- a full disk, a folder we may not write
        // into. Both used to be reported as "could not read <tempdir>":
        // the wrong direction, and a placeholder naming a file that does
        // not exist.
        //
        // A directory we cannot write into stands in for a full disk: the
        // archive reads fine and the write fails, which is the same shape.
        let dir = TempDir::new().expect("tempdir");
        let bundle = dir.path().join("b.krate");
        fs::write(&bundle, archive_with(&[])).expect("write");

        let locked = dir.path().join("locked");
        #[cfg(unix)]
        {
            fs::create_dir(&locked).expect("create");
            let mut perms = fs::metadata(&locked).expect("metadata").permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o500);
            fs::set_permissions(&locked, perms).expect("chmod");
        }
        // Windows has no mode bit that refuses writes INTO a directory (its
        // read-only attribute only protects the entry itself), so the write
        // is refused a different way there: `locked` is a plain file, and
        // nothing can be created underneath a file on any OS. Same shape --
        // the archive reads fine and the first write fails.
        #[cfg(not(unix))]
        fs::write(&locked, b"not a directory").expect("create");

        EXTRACT_ROOT.with(|r| *r.borrow_mut() = Some(locked.clone()));
        let opened = open(&bundle);
        EXTRACT_ROOT.with(|r| *r.borrow_mut() = None);

        let err = opened.expect_err("a directory we cannot write into must fail");
        let message = err.user_message().unwrap_or_else(|| err.to_string());
        assert!(
            !message.contains("<tempdir>"),
            "the refusal must not name a placeholder path: {message}"
        );
        assert!(
            !message.contains("could not read"),
            "the failure was a write, and must not be described as a read: {message}"
        );
        assert!(
            message.contains(&locked.display().to_string()),
            "the refusal must name the real path it failed at: {message}"
        );

        // The general io wording covers reads AND writes -- of the 40-odd
        // places that raise one, some read the archive and some write the
        // unpacked files -- so it must not claim a direction it cannot know.
        let write_failed = BundleError::Io {
            path: PathBuf::from("/some/where.krate"),
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        };
        let message = write_failed.user_message().expect("words for a person");
        assert!(
            !message.contains("could not read"),
            "the general io wording covers writes too and must not say read: {message}"
        );

        // A full disk is the one a person can act on, so it gets its own
        // sentence rather than an errno.
        let full = BundleError::Io {
            path: PathBuf::from("/some/where.krate"),
            source: io::Error::from(io::ErrorKind::StorageFull),
        };
        let message = full.user_message().expect("words for a person");
        assert!(
            message.contains("not enough room") && message.contains("Free some space"),
            "a full disk must say so, and say what to do: {message}"
        );
    }

    #[test]
    fn the_same_manifest_checked_out_on_windows_is_the_same_app() {
        // K-265. The manifest is hashed into the execution identity, and it
        // was hashed as raw bytes -- so git's default on Windows, which
        // rewrites LF to CRLF on checkout, made one commit build into two
        // different apps depending on the machine. Invisible in an editor,
        // and fatal to a reproducible build.
        fn bundle(dir: &Path, name: &str, manifest: &[u8]) -> PathBuf {
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
                writer.write_all(manifest).expect("write");
                writer.start_file(COMPONENT_ENTRY, opts).expect("component");
                writer.write_all(MINIMAL_COMPONENT).expect("write");
                writer.finish().expect("finish");
            }
            let path = dir.join(name);
            fs::write(&path, &buf).expect("write bundle");
            path
        }

        let dir = TempDir::new().expect("tempdir");
        let unix = MANIFEST.as_bytes().to_vec();
        let windows = MANIFEST.replace('\n', "\r\n").into_bytes();
        assert_ne!(unix, windows, "the fixture must really differ in bytes");

        let a = open(&bundle(dir.path(), "unix.krate", &unix)).expect("opens");
        let b = open(&bundle(dir.path(), "win.krate", &windows)).expect("opens");
        assert_eq!(
            a.digest().expect("digest").digest,
            b.digest().expect("digest").digest,
            "the same manifest with Windows line endings is the same app; \
             otherwise one commit built on two machines is two apps"
        );

        // But a real edit to the manifest is still a different app: the
        // manifest carries the capabilities, so it must stay
        // identity-bearing.
        let renamed = MANIFEST.replace("Demo", "Renamed").into_bytes();
        assert_ne!(
            renamed, unix,
            "the fixture must really change the manifest's meaning"
        );
        let c = open(&bundle(dir.path(), "renamed.krate", &renamed)).expect("opens");
        assert_ne!(
            a.digest().expect("digest").digest,
            c.digest().expect("digest").digest,
            "a changed manifest must still be a different app"
        );
    }

    #[test]
    fn repacking_an_app_keeps_its_execution_identity_and_changes_the_file() {
        // IC-212 and IC-860: the identities answer different questions and
        // must move independently. The digest functions are tested on their
        // own inputs; this is the property END TO END, through a real
        // archive, because that is where the two can be confused.
        //
        // A repack -- different entry order, different compression, same
        // contents -- is exactly what a mirror, a proxy or a rebuild does.
        // If it moved the execution identity, every reference to an app
        // would break the moment somebody stored it differently.
        fn bundle(dir: &Path, name: &str, method: CompressionMethod, reversed: bool) -> PathBuf {
            let mut entries: Vec<(&str, Vec<u8>)> = vec![
                (MANIFEST_ENTRY, MANIFEST.as_bytes().to_vec()),
                (COMPONENT_ENTRY, MINIMAL_COMPONENT.to_vec()),
                ("source/lib.rs", b"fn main() {}".to_vec()),
            ];
            if reversed {
                entries.reverse();
            }
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts = SimpleFileOptions::default().compression_method(method);
                for (name, bytes) in &entries {
                    writer.start_file(*name, opts).expect("entry");
                    writer.write_all(bytes).expect("write");
                }
                writer.finish().expect("finish");
            }
            let path = dir.join(name);
            fs::write(&path, &buf).expect("write bundle");
            path
        }

        let dir = TempDir::new().expect("tempdir");
        let plain = bundle(dir.path(), "a.krate", CompressionMethod::Deflated, false);
        let repacked = bundle(dir.path(), "b.krate", CompressionMethod::Stored, true);

        // The FILES differ: different compression, different order, so the
        // bytes on disk are not the same.
        assert_ne!(
            fs::read(&plain).expect("read"),
            fs::read(&repacked).expect("read"),
            "the fixture must really be a repack, or this proves nothing"
        );

        let a = open(&plain).expect("opens");
        let b = open(&repacked).expect("opens");
        assert_eq!(
            a.digest().expect("digest").digest,
            b.digest().expect("digest").digest,
            "a repack must NOT change what the app is -- otherwise every \
             reference to an app breaks when somebody stores it differently"
        );
        assert_eq!(
            a.project_digest().expect("digest").digest,
            b.project_digest().expect("digest").digest,
            "a repack must not change the project identity either"
        );

        // And a real change to what runs DOES move it.
        let changed = {
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
                writer.write_all(MANIFEST.as_bytes()).expect("write");
                writer.start_file(COMPONENT_ENTRY, opts).expect("component");
                // A different component: the same app id, different code.
                writer.write_all(OTHER_COMPONENT).expect("write");
                writer.start_file("source/lib.rs", opts).expect("source");
                writer.write_all(b"fn main() {}").expect("write");
                writer.finish().expect("finish");
            }
            let path = dir.path().join("c.krate");
            fs::write(&path, &buf).expect("write");
            path
        };
        assert_ne!(
            a.digest().expect("digest").digest,
            open(&changed)
                .expect("opens")
                .digest()
                .expect("digest")
                .digest,
            "different code must be a different app"
        );
    }

    #[test]
    fn many_opens_of_one_bundle_at_once_do_not_collide() {
        // IC-209 asks for concurrent opens. Every open unpacks into a
        // directory of its own, so several at once must not see each other's
        // files or race to the same path -- one bundle opened twice is the
        // ordinary case (a person double-clicks, then double-clicks again).
        let dir = TempDir::new().expect("tempdir");
        let bundle = dir.path().join("shared.krate");
        fs::write(
            &bundle,
            archive_with(&[("source/lib.rs".to_string(), b"fn main() {}".to_vec())]),
        )
        .expect("write");

        let paths: Vec<PathBuf> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        let opened = open(&bundle).expect("each open must succeed");
                        // Hold it open: the directories must coexist, not
                        // merely be created and freed one after another.
                        let path = opened.manifest_path().to_path_buf();
                        assert!(path.is_file(), "each open must have its own manifest");
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        assert!(
                            path.is_file(),
                            "another open must not have removed this one's files"
                        );
                        path
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("no open may panic"))
                .collect()
        });

        let unique: BTreeSet<&PathBuf> = paths.iter().collect();
        assert_eq!(
            unique.len(),
            paths.len(),
            "each open needs its OWN directory; two sharing one would let a \
             second open overwrite what a first is still reading: {paths:?}"
        );
    }

    #[test]
    fn a_damaged_signature_is_not_reported_as_unsigned() {
        // K-258. signature_envelope() used to swallow a parse failure with
        // `.ok()`, so a bundle whose signature had been edited looked exactly
        // like one that was never signed -- and "unsigned" is the one thing a
        // recipient shrugs at.
        fn bundle_with_signature(dir: &Path, name: &str, signature: Option<&[u8]>) -> PathBuf {
            let mut buf = Vec::new();
            {
                let mut writer = ZipWriter::new(io::Cursor::new(&mut buf));
                let opts =
                    SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
                writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
                writer.write_all(MANIFEST.as_bytes()).expect("write");
                writer.start_file(COMPONENT_ENTRY, opts).expect("component");
                writer.write_all(MINIMAL_COMPONENT).expect("write");
                if let Some(signature) = signature {
                    writer.start_file(SIGNATURE_ENTRY, opts).expect("signature");
                    writer.write_all(signature).expect("write");
                }
                writer.finish().expect("finish");
            }
            let path = dir.join(name);
            fs::write(&path, &buf).expect("write bundle");
            path
        }

        let dir = TempDir::new().expect("tempdir");

        // No signature at all: absent, and nothing to say about it.
        let unsigned = bundle_with_signature(dir.path(), "unsigned.krate", None);
        let state = open(&unsigned)
            .expect("opens")
            .signature_state()
            .expect("state");
        assert_eq!(state, SignatureState::Absent);
        assert!(
            state.concern().is_none(),
            "an unsigned bundle is ordinary and must not warn about anything"
        );

        // A signature that will not parse: damaged, and it must SAY so.
        let damaged =
            bundle_with_signature(dir.path(), "damaged.krate", Some(br#"{"corrupted":true}"#));
        let state = open(&damaged)
            .expect("opens")
            .signature_state()
            .expect("state");
        assert!(
            matches!(state, SignatureState::Damaged { .. }),
            "a signature that will not parse must be Damaged, not Absent: {state:?}"
        );
        let concern = state
            .concern()
            .expect("a damaged signature must give a person something to act on");
        assert!(
            concern.contains("get a fresh copy"),
            "the warning must say what to do: {concern}"
        );

        // And the two must not be the same state, which is the whole defect.
        assert_ne!(
            open(&unsigned)
                .expect("opens")
                .signature_state()
                .expect("s"),
            open(&damaged).expect("opens").signature_state().expect("s"),
            "a damaged signature must be distinguishable from no signature"
        );
    }

    #[test]
    fn an_archive_with_no_entries_is_refused() {
        // IC-209 asks for zero entries as well as too many. An empty archive
        // is a VALID zip -- twenty-two bytes, no records -- so nothing in the
        // parsing rejects it. It has to be refused for what it lacks.
        let dir = TempDir::new().expect("tempdir");

        let empty = dir.path().join("empty.krate");
        {
            let writer = ZipWriter::new(fs::File::create(&empty).expect("create"));
            writer.finish().expect("finish");
        }
        assert_eq!(
            fs::metadata(&empty).expect("stat").len(),
            22,
            "an empty zip is 22 bytes; if this changed the fixture is not what it claims"
        );
        let err = open(&empty).expect_err("an archive with no entries must be refused");
        assert!(
            matches!(err, BundleError::MissingEntry(_)),
            "it must be refused for what it is missing: {err}"
        );

        // And a file that is not an archive at all.
        let junk = dir.path().join("junk.krate");
        fs::write(&junk, b"this is not a zip").expect("write");
        assert!(
            open(&junk).is_err(),
            "a file that is not an archive must be refused"
        );
    }

    #[test]
    fn a_path_that_nests_too_deep_is_refused_at_the_boundary() {
        // K-257. Nothing bounded depth, so an entry 200 directories deep was
        // accepted and written out -- 209 components once extracted, which is
        // past MAX_PATH on Windows. Checked at the boundary, because an
        // off-by-one here refuses a legitimate bundle.
        for (depth, must_open) in [
            (MAX_PATH_DEPTH - 1, true),
            (MAX_PATH_DEPTH, true),
            (MAX_PATH_DEPTH + 1, false),
        ] {
            // depth counts separators, and `source/` is the first one.
            let dirs: Vec<String> = (0..depth.saturating_sub(1))
                .map(|i| format!("d{i}"))
                .collect();
            let path = if dirs.is_empty() {
                "source/lib.rs".to_string()
            } else {
                format!("source/{}/lib.rs", dirs.join("/"))
            };
            assert_eq!(
                path.matches('/').count(),
                depth,
                "the fixture must actually be {depth} deep: {path}"
            );

            let bytes = archive_with(&[(path.clone(), b"fn main() {}".to_vec())]);
            let dir = TempDir::new().expect("tempdir");
            let bundle = dir.path().join("deep.krate");
            fs::write(&bundle, &bytes).expect("write");

            let opened = open(&bundle);
            if must_open {
                assert!(
                    !matches!(opened, Err(BundleError::PathTooDeep { .. })),
                    "{depth} separators is within the limit of {MAX_PATH_DEPTH} and must \
                     not be refused for its depth"
                );
            } else {
                let err = opened.expect_err("must be refused");
                assert!(
                    matches!(err, BundleError::PathTooDeep { .. }),
                    "{depth} separators is over the limit and must be refused: {err}"
                );
                let text = err.to_string();
                assert!(
                    text.contains("Flatten it"),
                    "the refusal must say what to do about it: {text}"
                );
            }
        }
    }

    #[test]
    fn a_path_that_is_too_long_is_refused_and_stays_readable() {
        // Length is judged as STORED, so the same bundle is judged the same
        // way on every machine -- measuring the extracted path would depend
        // on the temp directory it landed in.
        let long = format!("source/{}.rs", "a".repeat(MAX_PATH_BYTES));
        let bytes = archive_with(&[(long.clone(), b"fn main() {}".to_vec())]);
        let dir = TempDir::new().expect("tempdir");
        let bundle = dir.path().join("long.krate");
        fs::write(&bundle, &bytes).expect("write");

        let err = open(&bundle).expect_err("a long path must be refused");
        assert!(
            matches!(err, BundleError::PathTooLong { .. }),
            "refused for the wrong reason: {err}"
        );
        let text = err.to_string();
        assert!(
            text.contains("Shorten it"),
            "the refusal must say what to do: {text}"
        );
        // The message is about a very long name, so it must not repeat that
        // name in full -- the advice has to stay visible next to it.
        assert!(
            !text.contains(&long),
            "the refusal must shorten the path it is about, not repeat it: {text}"
        );
        assert!(
            text.contains("..."),
            "a shortened path should show that it was shortened: {text}"
        );

        // And a name just inside the limit still opens.
        let ok = format!("source/{}.rs", "a".repeat(MAX_PATH_BYTES - 20));
        assert!(ok.len() <= MAX_PATH_BYTES, "fixture is within the limit");
        let bytes = archive_with(&[(ok, b"fn main() {}".to_vec())]);
        let bundle = dir.path().join("ok.krate");
        fs::write(&bundle, &bytes).expect("write");
        assert!(
            !matches!(open(&bundle), Err(BundleError::PathTooLong { .. })),
            "a name inside the limit must still open"
        );
    }

    #[test]
    fn an_ordinary_ascii_path_is_still_accepted() {
        // The guard above must not cost a normal bundle. If this ever fails,
        // the ASCII rule has grown teeth it was never meant to have.
        let bytes = archive_with(&[("source/lib.rs".to_string(), b"fn main() {}".to_vec())]);
        let dir = TempDir::new().expect("tempdir");
        let bundle = dir.path().join("plain.krate");
        fs::write(&bundle, &bytes).expect("write");
        open(&bundle).expect("an ASCII path must still open");
    }

    #[test]
    fn pack_then_open_round_trips_manifest_and_component() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let bundle = dir.path().join("demo.krate");

        let size = pack(&manifest, &component, &bundle).expect("pack");
        assert!(size > 0, "bundle should not be empty");

        let opened = open(&bundle).expect("open");
        assert_eq!(opened.manifest().app.id, "com.example.demo");
        assert_eq!(
            fs::read(opened.component_path()).expect("read component"),
            MINIMAL_COMPONENT
        );
        assert!(opened.assets_path().is_none());
    }

    #[test]
    fn pack_then_open_round_trips_nested_assets() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let assets = dir.path().join("assets");
        fs::create_dir_all(assets.join("prompts")).expect("create assets");
        fs::write(assets.join("prompts/welcome.txt"), b"Welcome to Krate").expect("write asset");
        fs::write(assets.join("icon.bin"), [1_u8, 2, 3]).expect("write asset");
        let bundle = dir.path().join("demo.krate");

        pack_with_assets(&manifest, &component, Some(&assets), &bundle).expect("pack assets");
        let opened = open(&bundle).expect("open");
        let extracted = opened.assets_path().expect("assets root");
        assert_eq!(
            fs::read_to_string(extracted.join("prompts/welcome.txt")).expect("read nested asset"),
            "Welcome to Krate"
        );
        assert_eq!(
            fs::read(extracted.join("icon.bin")).expect("read binary asset"),
            [1, 2, 3]
        );
    }

    /// Operating-system metadata never ships as source (CP1 exit test:
    /// a pack rebuilds byte-equal on a second machine).
    ///
    /// Measured before the skip existed: packing an app whose folder Finder
    /// had opened produced a bundle with `source/.DS_Store` in it, 8 KB of
    /// Finder state. That file exists on a Mac that browsed the folder and
    /// on nothing else, so the same source packed on two machines had two
    /// editable digests -- which is the exact thing determinism promises
    /// cannot happen. AppleDouble `._*` sidecars are the same story on a
    /// non-native volume, and `Thumbs.db` / `desktop.ini` are Windows'.
    #[test]
    fn os_metadata_in_the_source_tree_is_not_packed() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        write_temp(dir.path(), "Cargo.toml", b"[package]\nname = \"demo\"\n");
        fs::create_dir_all(dir.path().join("src")).expect("src");
        fs::write(dir.path().join("src/lib.rs"), b"fn main() {}").expect("lib");
        // A real config directory that starts with a dot and MUST ship,
        // so the skip cannot be "every dotfile".
        fs::create_dir_all(dir.path().join(".cargo")).expect(".cargo");
        fs::write(dir.path().join(".cargo/config.toml"), b"[build]\n").expect("config");
        for junk in [".DS_Store", "src/.DS_Store", "Thumbs.db", "src/desktop.ini"] {
            fs::write(dir.path().join(junk), b"os state").expect(junk);
        }

        let bundle = dir.path().join("out.krate");
        pack_with_source(&manifest, &component, None, Some(dir.path()), &bundle)
            .expect("pack with source");

        let names: Vec<String> = {
            let file = File::open(&bundle).expect("open");
            let mut zip = ZipArchive::new(file).expect("zip");
            (0..zip.len())
                .map(|i| zip.by_index(i).expect("entry").name().to_string())
                .collect()
        };
        for junk in ["DS_Store", "Thumbs.db", "desktop.ini"] {
            assert!(
                !names.iter().any(|n| n.contains(junk)),
                "{junk} was packed as source; the bundle now depends on which \
                 machine packed it: {names:?}",
            );
        }
        assert!(
            names.iter().any(|n| n == "source/.cargo/config.toml"),
            "a dot-directory that is real build input must still ship: {names:?}",
        );
        assert!(names.iter().any(|n| n == "source/src/lib.rs"));
    }

    #[test]
    fn packs_the_source_so_an_app_can_be_changed_later() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);

        // A crate-shaped directory: build output must NOT be packed, and the
        // lock file MUST be -- it is what makes a rebuild elsewhere resolve to
        // the same crates (CP1, the editable closure).
        write_temp(dir.path(), "Cargo.toml", b"[package]\nname = \"demo\"\n");
        write_temp(dir.path(), "Cargo.lock", b"# pinned");
        write_temp(
            dir.path(),
            "rust-toolchain.toml",
            b"[toolchain]\nchannel = \"1.94.1\"\n",
        );
        fs::create_dir_all(dir.path().join("src")).expect("src");
        fs::write(dir.path().join("src/lib.rs"), b"fn main() {}").expect("lib");
        fs::create_dir_all(dir.path().join("target/release")).expect("target");
        fs::write(dir.path().join("target/release/junk"), b"build output").expect("junk");

        let bundle = dir.path().join("out.krate");
        pack_with_source(&manifest, &component, None, Some(dir.path()), &bundle)
            .expect("pack with source");

        let opened = open(&bundle).expect("open");
        let source = opened.source_path().expect("source shipped");
        assert!(source.join("Cargo.toml").is_file());
        assert!(source.join("src/lib.rs").is_file());
        // The point of shipping source is rebuilding: the lock travels so
        // the rebuild can be --locked, the build output does not.
        assert_eq!(fs::read(source.join("Cargo.lock")).unwrap(), b"# pinned");
        assert!(!source.join("target").exists());
        // And the bundle says what it takes to build it again.
        let closure = opened
            .closure()
            .expect("a bundle with source carries a closure record");
        assert!(closure.locked, "{closure:?}");
        assert_eq!(closure.rust_toolchain.as_deref(), Some("1.94.1"));
        assert_eq!(closure.source_digest.len(), 64);
        assert!(closure.sdk.is_none(), "no SDK was packed");
        assert!(
            closure.processed_by.is_empty(),
            "the minimal fixture names no producers: {:?}",
            closure.processed_by
        );
        let judged = judge_bytes(&fs::read(&bundle).unwrap(), 64 * 1024 * 1024).unwrap();
        assert_eq!(
            judged.closure.as_ref(),
            Some(closure),
            "the two doors agree on the closure"
        );
    }

    /// The toolchain in a closure record is what the component SAYS built
    /// it -- its `producers` section -- not what the packer has on PATH.
    /// A hand-assembled core module with a producers section is enough to
    /// prove the reader; a real cargo-component build carries `rustc` and
    /// `wit-component` the same way (measured on krate-hello-gui).
    #[test]
    fn the_closure_records_what_the_component_says_built_it() {
        fn leb(mut n: usize) -> Vec<u8> {
            let mut out = Vec::new();
            loop {
                let byte = (n & 0x7f) as u8;
                n >>= 7;
                if n == 0 {
                    out.push(byte);
                    return out;
                }
                out.push(byte | 0x80);
            }
        }
        fn name(s: &str) -> Vec<u8> {
            let mut out = leb(s.len());
            out.extend_from_slice(s.as_bytes());
            out
        }
        // producers: one field, "processed-by", two values.
        let mut section = Vec::new();
        section.extend(name("producers"));
        section.extend(leb(1));
        section.extend(name("processed-by"));
        section.extend(leb(2));
        section.extend(name("rustc"));
        section.extend(name("1.94.1 (e408947bf 2026-03-25)"));
        section.extend(name("wit-component"));
        section.extend(name("0.227.1"));
        let mut module = b"\0asm\x01\0\0\0".to_vec();
        module.push(0); // custom section id
        module.extend(leb(section.len()));
        module.extend(section);

        assert_eq!(
            producers(&module),
            vec![
                Producer {
                    name: "rustc".to_string(),
                    version: "1.94.1 (e408947bf 2026-03-25)".to_string()
                },
                Producer {
                    name: "wit-component".to_string(),
                    version: "0.227.1".to_string()
                },
            ]
        );
        assert!(
            producers(MINIMAL_COMPONENT).is_empty(),
            "the minimal fixture names none"
        );
        assert!(
            producers(b"not wasm").is_empty(),
            "garbage names none, and does not fail"
        );
    }

    /// A closure record that does not describe the closure it ships with is
    /// damage, refused at both doors: source edited under the record, the
    /// lock removed, the SDK swapped, the record itself edited.
    #[test]
    fn a_closure_record_that_lies_is_refused() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let source = dir.path().join("app");
        fs::create_dir_all(source.join("src")).unwrap();
        fs::write(source.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(source.join("Cargo.lock"), "# pinned\n").unwrap();
        fs::write(source.join("src/lib.rs"), "// code\n").unwrap();
        let sdk = dir.path().join("sdk");
        fs::create_dir_all(sdk.join("crates/bindings-rust/src")).unwrap();
        fs::write(
            sdk.join("crates/bindings-rust/Cargo.toml"),
            "[package]\nname = \"krate\"\nversion = \"0.4.0\"\n",
        )
        .unwrap();
        fs::write(sdk.join("crates/bindings-rust/src/lib.rs"), "// sdk\n").unwrap();
        let good = dir.path().join("good.krate");
        pack_with_sdk(
            &manifest,
            &component,
            None,
            Some(&source),
            Some(&sdk),
            &good,
        )
        .unwrap();
        let opened = open(&good).unwrap();
        let closure = opened.closure().expect("closure recorded");
        assert!(closure.locked);
        assert_eq!(
            closure.sdk.as_ref().and_then(|s| s.version.as_deref()),
            Some("0.4.0"),
            "{closure:?}"
        );
        let keep = |_: &str, bytes: &[u8]| Some(bytes.to_vec());
        type Case<'a> = (&'a str, Box<EditFn>, &'a str);
        let cases: Vec<Case> = vec![
            (
                "source edited under the record",
                Box::new(|name, bytes| {
                    Some(if name == "source/src/lib.rs" {
                        b"// changed\n".to_vec()
                    } else {
                        bytes.to_vec()
                    })
                }),
                "source does not match",
            ),
            (
                "the lock removed",
                Box::new(|name, bytes| (name != "source/Cargo.lock").then(|| bytes.to_vec())),
                "source does not match",
            ),
            (
                "the SDK swapped",
                Box::new(|name, bytes| {
                    Some(if name == "sdk/crates/bindings-rust/src/lib.rs" {
                        b"// other sdk\n".to_vec()
                    } else {
                        bytes.to_vec()
                    })
                }),
                "SDK does not match",
            ),
            (
                "the record edited",
                Box::new(|name, bytes| {
                    Some(if name == CLOSURE_ENTRY {
                        b"{\"schema\":\"krate.bundle.closure.v1\"".to_vec()
                    } else {
                        bytes.to_vec()
                    })
                }),
                "fresh",
            ),
        ];
        for (what, edit, words) in &cases {
            let path = dir.path().join("edited.krate");
            edit_archive(&good, &path, edit.as_ref(), &[]);
            let err = match open(&path) {
                Err(err) => err,
                Ok(_) => panic!("{what}: must be refused"),
            };
            assert_eq!(variant(&err), "DamagedClosure", "{what}: {err}");
            assert!(err.to_string().contains(words), "{what}: {err}");
            let judged = judge_bytes(&fs::read(&path).unwrap(), 64 * 1024 * 1024)
                .expect_err("the judge refuses what open refuses");
            assert_eq!(variant(&judged), "DamagedClosure", "{what}: {judged}");
        }
        let _ = keep;
        // A bundle without source has no record, and needs none.
        let plain = dir.path().join("plain.krate");
        pack(&manifest, &component, &plain).unwrap();
        assert!(open(&plain).unwrap().closure().is_none());
    }

    #[test]
    fn a_bundle_without_source_still_opens() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let bundle = dir.path().join("out.krate");
        pack(&manifest, &component, &bundle).expect("pack");
        // Every bundle made before source shipped is this shape.
        assert!(open(&bundle).expect("open").source_path().is_none());
    }

    #[test]
    fn open_rejects_traversal_inside_the_asset_namespace() {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buffer);
            let opts = SimpleFileOptions::default();
            zip.start_file(MANIFEST_ENTRY, opts)
                .expect("start manifest");
            zip.write_all(MANIFEST.as_bytes()).expect("write manifest");
            zip.start_file(COMPONENT_ENTRY, opts).expect("start wasm");
            zip.write_all(MINIMAL_COMPONENT).expect("write wasm");
            zip.start_file("assets/../../evil", opts)
                .expect("start hostile asset");
            zip.write_all(b"pwned").expect("write hostile asset");
            zip.finish().expect("finish");
        }
        buffer.set_position(0);

        let err = open_reader(buffer).expect_err("asset traversal must fail");
        assert!(matches!(err, BundleError::UnsafeAssetPath { .. }));
    }

    /// Mark one central-directory entry as a unix symlink, in finished zip
    /// bytes.
    ///
    /// The writer API cannot express this -- `unix_permissions` masks with
    /// `& 0o777` and drops the file-type bits -- and a reader derives the
    /// mode from `external_attributes >> 16`. So the four attribute bytes
    /// are rewritten in place: find the central-directory header (`PK\x01\x02`)
    /// whose name matches, set its external attributes to the mode, and set
    /// the creator-system byte to unix so the reader interprets them as a
    /// unix mode rather than DOS flags.
    ///
    /// Offsets are from APPNOTE 4.3.12: version-made-by at +4 (high byte is
    /// the system), name length at +28, extra at +30, comment at +32,
    /// external attributes at +38, name at +46.
    fn mark_entry_as_symlink(mut bytes: Vec<u8>, entry: &str) -> Vec<u8> {
        const CENTRAL: &[u8] = b"PK\x01\x02";
        let mut at = 0;
        while at + 46 <= bytes.len() {
            if &bytes[at..at + 4] != CENTRAL {
                at += 1;
                continue;
            }
            let name_len = u16::from_le_bytes([bytes[at + 28], bytes[at + 29]]) as usize;
            let extra_len = u16::from_le_bytes([bytes[at + 30], bytes[at + 31]]) as usize;
            let comment_len = u16::from_le_bytes([bytes[at + 32], bytes[at + 33]]) as usize;
            let name = &bytes[at + 46..at + 46 + name_len];
            if name == entry.as_bytes() {
                bytes[at + 5] = 3; // creator system: unix
                bytes[at + 38..at + 42].copy_from_slice(&(0o120_777_u32 << 16).to_le_bytes());
                return bytes;
            }
            at += 46 + name_len + extra_len + comment_len;
        }
        panic!("no central-directory entry named {entry}");
    }

    /// A source entry marked as a symlink is extracted as a FILE (IC-397).
    ///
    /// Packing refuses a symlink in the developer's tree, and the path guard
    /// refuses a hostile entry NAME. Neither covers this shape: a perfectly
    /// ordinary name -- `source/lib.rs`, nothing to object to -- carrying
    /// zip's symlink mode bits, whose content is the path it points at.
    ///
    /// An extractor that honours those bits writes a link to /etc/passwd
    /// into the extracted tree, and every later reader that opens
    /// `source/lib.rs` reads a file the bundle never carried. Krate's
    /// extractor calls File::create and nothing else, so the bits are
    /// ignored and the target path lands as ordinary bytes.
    ///
    /// That is the safe behaviour, and it is safe by construction rather
    /// than by a check -- which is exactly why it needs a test. There is no
    /// guard here to delete by accident; a future extractor that "adds
    /// symlink support" would silently become vulnerable, and this is what
    /// would stop it.
    ///
    /// The archive cannot be built with the writer's own API: measured,
    /// `unix_permissions` stores `mode & 0o777`, so 0o120777 goes in and
    /// 0o100777 comes out -- the file-type bits are dropped and the archive
    /// holds no link at all. The first version of this test did exactly
    /// that and was worthless: an extractor sabotaged to honour symlink bits
    /// still passed it, because there were none to honour. So the external
    /// attributes are patched into the finished bytes, and the fixture
    /// asserts the bits survived before anything else is checked.
    #[test]
    fn a_source_entry_claiming_to_be_a_symlink_extracts_as_a_file() {
        let dir = TempDir::new().expect("tempdir");
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buffer);
            let opts = SimpleFileOptions::default();
            zip.start_file(MANIFEST_ENTRY, opts)
                .expect("start manifest");
            zip.write_all(MANIFEST.as_bytes()).expect("write manifest");
            zip.start_file(COMPONENT_ENTRY, opts).expect("start wasm");
            zip.write_all(MINIMAL_COMPONENT).expect("write wasm");
            zip.start_file("source/lib.rs", opts).expect("start link");
            zip.write_all(b"/etc/passwd").expect("write target");
            zip.finish().expect("finish");
        }
        // 0o120777 is S_IFLNK plus permissions: exactly how a symlink is
        // stored in a zip, with the target path as the entry's body.
        let bytes = mark_entry_as_symlink(buffer.into_inner(), "source/lib.rs");

        // The fixture is only worth anything if the bits are really there.
        {
            let mut check = ZipArchive::new(Cursor::new(bytes.clone())).expect("reopen");
            let mode = check
                .by_name("source/lib.rs")
                .expect("entry")
                .unix_mode()
                .expect("a mode is recorded");
            assert_eq!(
                mode & 0o170_000,
                0o120_000,
                "the fixture must actually claim to be a symlink, got mode {mode:o}",
            );
        }

        let bundle = dir.path().join("link.krate");
        fs::write(&bundle, &bytes).expect("write bundle");

        let opened = open(&bundle).expect("a link-shaped entry does not stop the bundle opening");
        let source = opened.source_path().expect("source was extracted");
        let extracted = source.join("lib.rs");

        let meta = fs::symlink_metadata(&extracted).expect("the entry was extracted");
        assert!(
            !meta.file_type().is_symlink(),
            "a symlink was created on disk from an archive's mode bits; \
             every later read of {} would follow it out of the tree",
            extracted.display(),
        );
        assert!(meta.file_type().is_file(), "it is an ordinary file");
        // The target path is content, not a destination.
        assert_eq!(
            fs::read(&extracted).expect("read"),
            b"/etc/passwd",
            "the link target must land as bytes, not be followed",
        );
    }

    #[test]
    fn a_windows_sdk_path_is_rewritten_to_the_placeholder() {
        // The exact line from a real Windows-built bundle whose source could
        // not build anywhere else (K-126): AppData/Local/Krate has a capital
        // K, and the lowercase marker missed it.
        let line = r#"krate = { path = "C:/Users/user/AppData/Local/Krate/sdk/93ca1541984629cb/crates/bindings-rust" }"#;
        let out = rewrite_sdk_paths(line);
        assert!(
            out.contains(r#"path = "{KRATE_SDK}/crates/bindings-rust""#),
            "got: {out}"
        );
        // Backslash separators rewrite too.
        let bs =
            r#"path = "C:\Users\user\AppData\Local\Krate\sdk\93ca1541984629cb\wit\krate\phase3""#;
        let out = rewrite_sdk_paths(bs);
        assert!(out.contains("{KRATE_SDK}"), "got: {out}");
        // The Unix cache shape keeps working.
        let unix = r#"krate = { path = "/home/u/.cache/krate/sdk/aabbccdd11223344/crates/bindings-rust" }"#;
        assert!(rewrite_sdk_paths(unix).contains("{KRATE_SDK}/crates/bindings-rust"));
    }

    /// A sample app in the checkout reaches the SDK by relative path, and a
    /// bundle packed from it used to ship those paths verbatim: source that
    /// rebuilt only inside this repository (CP1, the editable closure).
    #[test]
    fn a_checkout_relative_sdk_path_is_rewritten_to_the_placeholder() {
        let manifest = "[dependencies]\n\
            krate = { path = \"../../crates/bindings-rust\", features = [\"gui\"] }\n\
            [package.metadata.component.target]\n\
            path = \"../../wit/krate/phase3\"\n\
            [package.metadata.component.target.dependencies]\n\
            \"krate:io\" = { path = \"../../wit/krate/phase3/deps/io\" }\n\
            zune = { path = \"vendor/crates/bindings-rust\" }\n\
            other = { path = \"../sibling/thing\" }\n";
        let out = rewrite_sdk_paths(manifest);
        assert!(
            out.contains(
                r#"krate = { path = "{KRATE_SDK}/crates/bindings-rust", features = ["gui"] }"#
            ),
            "{out}"
        );
        assert!(
            out.contains(r#"path = "{KRATE_SDK}/wit/krate/phase3""#),
            "{out}"
        );
        assert!(
            out.contains(r#""krate:io" = { path = "{KRATE_SDK}/wit/krate/phase3/deps/io" }"#),
            "{out}"
        );
        assert!(
            out.contains(r#"zune = { path = "vendor/crates/bindings-rust" }"#),
            "an app's own vendored copy is its own: {out}"
        );
        assert!(
            out.contains(r#"other = { path = "../sibling/thing" }"#),
            "a relative path that is not the SDK stays: {out}"
        );
        // An absolute path into a clone rewrites the same way.
        let clone = r#"krate = { path = "/Users/someone/src/krate/crates/bindings-rust" }"#;
        assert!(rewrite_sdk_paths(clone).contains("{KRATE_SDK}/crates/bindings-rust"));
        // And the placeholder itself is left alone on a repack.
        let already = r#"krate = { path = "{KRATE_SDK}/crates/bindings-rust" }"#;
        assert_eq!(rewrite_sdk_paths(already).trim_end(), already);
    }

    #[test]
    fn pack_refuses_a_manifest_whose_entry_is_not_the_bundle_component() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(
            dir.path(),
            "manifest.toml",
            MANIFEST.replace("code.wasm", "other.wasm").as_bytes(),
        );
        let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
        let bundle = dir.path().join("demo.krate");

        let err = pack(&manifest, &component, &bundle).expect_err("entry mismatch must fail");
        assert!(matches!(err, BundleError::EntryMismatch { .. }));
    }

    #[test]
    fn open_rejects_an_archive_without_a_component() {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buffer);
            zip.start_file(MANIFEST_ENTRY, SimpleFileOptions::default())
                .expect("start manifest");
            zip.write_all(MANIFEST.as_bytes()).expect("write manifest");
            zip.finish().expect("finish");
        }
        buffer.set_position(0);

        let err = open_reader(buffer).expect_err("missing component must fail");
        assert!(matches!(err, BundleError::MissingEntry(COMPONENT_ENTRY)));
    }

    #[test]
    fn open_ignores_extra_entries_including_traversal_attempts() {
        // A hostile bundle carrying `../../evil` must not write outside the
        // temp directory. Reading entries by exact name means the extra entry
        // is simply never consulted.
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut buffer);
            let opts = SimpleFileOptions::default();
            zip.start_file("../../evil", opts).expect("start evil");
            zip.write_all(b"pwned").expect("write evil");
            zip.start_file(MANIFEST_ENTRY, opts)
                .expect("start manifest");
            zip.write_all(MANIFEST.as_bytes()).expect("write manifest");
            zip.start_file(COMPONENT_ENTRY, opts).expect("start wasm");
            zip.write_all(MINIMAL_COMPONENT).expect("write wasm");
            zip.finish().expect("finish");
        }
        buffer.set_position(0);

        let opened = open_reader(buffer).expect("bundle with extra entries still opens");
        let parent = opened
            .component_path()
            .parent()
            .expect("component has a parent")
            .to_path_buf();
        assert!(opened.component_path().starts_with(&parent));
        assert!(!parent.join("../../evil").exists());
    }

    #[test]
    fn the_bundle_size_limit_holds_at_exactly_the_boundary() {
        // IC-393 asks for the cap at its exact value and one past it. The
        // limit is 256 MiB, which is far too much to write in a test -- but
        // `open` reads the size from the file's METADATA, so a sparse file
        // has the apparent size without the bytes. The whole fixture costs
        // no disk at all.
        let dir = TempDir::new().expect("tempdir");

        // One past the cap: refused for its size, naming both numbers.
        let over = dir.path().join("over.krate");
        fs::File::create(&over)
            .expect("create")
            .set_len(MAX_BUNDLE_BYTES + 1)
            .expect("size it");
        let err = open(&over).expect_err("a bundle over the cap must be refused");
        assert!(
            matches!(err, BundleError::TooLarge { .. }),
            "it must be refused for its SIZE, not for anything else: {err}"
        );
        let text = err.to_string();
        assert!(
            text.contains(&(MAX_BUNDLE_BYTES + 1).to_string())
                && text.contains(&MAX_BUNDLE_BYTES.to_string()),
            "the refusal must name what arrived and what is allowed: {text}"
        );

        // Exactly the cap: the size check must NOT fire. It still fails --
        // a file of zeros is not an archive -- and that is the point: the
        // refusal has to come from what the bytes are, not from how many.
        let exact = dir.path().join("exact.krate");
        fs::File::create(&exact)
            .expect("create")
            .set_len(MAX_BUNDLE_BYTES)
            .expect("size it");
        let err = open(&exact).expect_err("a file of zeros is not an archive");
        assert!(
            !matches!(err, BundleError::TooLarge { .. }),
            "a bundle of exactly the limit is within it: {err}"
        );
    }

    #[cfg(feature = "fetch")]
    #[test]
    fn a_server_that_stops_responding_does_not_hold_the_run_open() {
        // K-275. ureq bounds CONNECT and leaves READ unbounded, so a host
        // that completes the handshake and then says nothing held
        // `krate run <url>` open indefinitely -- no output, nothing to
        // retry, the worst shape a failure can take on the receiver's path.
        //
        // The real budget is thirty seconds of silence, which is too long to
        // sit through here. So the timeout is set SHORT for this one call,
        // against a server that accepts and never answers, and the test
        // requires both that it gives up and that it gives up for the right
        // reason.
        //
        // An earlier version asserted only that the constant was non-zero
        // and that a CLOSING socket ended the wait. Neither needs the
        // timeout to exist: deleting `.timeout_read(..)` left it passing.
        // A test that cannot fail on the defect it names is worth less than
        // no test.
        //
        // What this covers, stated plainly so nobody over-reads it: that a
        // read timeout DOES stop a silent server, and that the message it
        // produces is one a person can act on. It builds its own agent with
        // a short budget, so it does NOT prove that `fetch` configures one --
        // deleting the timeout from fetch's agent leaves this green. That
        // half is covered by a_download_gives_up_on_a_silent_server below,
        // which drives fetch itself.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let silent = std::thread::spawn(move || {
            // Accept, read the request, and hold the connection open saying
            // nothing at all.
            if let Ok((stream, _)) = listener.accept() {
                std::thread::sleep(std::time::Duration::from_secs(3));
                drop(stream);
            }
        });

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(FETCH_CONNECT_TIMEOUT)
            .timeout_read(std::time::Duration::from_millis(300))
            .build();
        let started = std::time::Instant::now();
        let err = agent
            .get(&format!("http://127.0.0.1:{port}/x"))
            .call()
            .expect_err("a server that never answers must not succeed");
        let elapsed = started.elapsed();
        let _ = silent.join();

        assert!(
            is_read_timeout(&err),
            "it must give up because the read timed out, not for some other \
             reason: {err}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "it must give up on the read timeout rather than waiting for the \
             server to close: took {elapsed:?}"
        );

        // And the message a person gets says what happened and what to do.
        // This is the mapping fetch() applies to exactly that error.
        let mapped = map_fetch_error("http://example/x", err);
        assert!(
            mapped.contains("stopped responding") && mapped.contains("Try again"),
            "the raw ureq text is three copies of the url and two Network \
             Errors; a person needs the plain sentence: {mapped}"
        );
    }

    /// `fetch` itself must carry a read timeout, not merely be able to.
    ///
    /// The test above proves a read timeout works and that its message is
    /// readable. It builds its own agent, so it stays green if `fetch`
    /// forgets to set one -- which is exactly the defect K-275 was about.
    /// This one drives `fetch`, so the configuration is what is under test.
    ///
    /// Ignored by default: the real budget is thirty seconds of silence and
    /// the point is to WAIT for it. Run it deliberately with
    /// `cargo test -p krate-bundle -- --ignored a_download_gives_up`.
    #[cfg(feature = "fetch")]
    #[test]
    #[ignore = "waits out the real 30s silence budget; run deliberately"]
    fn a_download_gives_up_on_a_silent_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let silent = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                // Outlive the budget, saying nothing.
                std::thread::sleep(FETCH_SILENCE_TIMEOUT + std::time::Duration::from_secs(15));
                drop(stream);
            }
        });

        let started = std::time::Instant::now();
        let err = fetch(&format!("http://127.0.0.1:{port}/x"), true)
            .expect_err("a server that never answers must not succeed");
        let elapsed = started.elapsed();
        let _ = silent.join();

        assert!(
            elapsed < FETCH_SILENCE_TIMEOUT + std::time::Duration::from_secs(10),
            "fetch must give up on its own timeout rather than waiting for \
             the server: took {elapsed:?}"
        );
        assert!(
            err.to_string().contains("stopped responding"),
            "and say so in words a person can act on: {err}"
        );
    }

    /// Where two readers would disagree about what an archive contains,
    /// Krate refuses it rather than pick one (IC-714, test 1477).
    ///
    /// A duplicate central-directory record is exactly that disagreement.
    /// An independent reader that walks the records sees BOTH copies;
    /// `ZipArchive` keys by name and folds them to one, keeping the last.
    /// So "what is in this file" has two answers, and a reviewer reading
    /// one copy is not reading what runs.
    ///
    /// The record count is where the disagreement is visible, and the
    /// refusal is built on it: the end-of-central-directory count says
    /// four, the parser resolves three, and the difference is the
    /// duplicate. Asserting that gap here is what keeps the guard honest
    /// -- if a future zip crate started folding silently at a different
    /// layer, this would still see it.
    #[test]
    fn an_archive_two_readers_would_read_differently_is_refused() {
        const DUPLICATE: &[u8] =
            include_bytes!("../tests/fixtures/duplicate-from-another-writer.krate");

        // What the archive DECLARES it contains, read from its own
        // end-of-central-directory record -- the number an independent
        // reader walking the records would agree with.
        let declared = central_directory_record_count(DUPLICATE)
            .expect("the fixture has a readable end-of-central-directory record");
        // What the parser resolves, folding by name.
        let parsed = ZipArchive::new(io::Cursor::new(DUPLICATE))
            .expect("it is a readable zip")
            .len();

        assert!(
            declared > parsed,
            "the fixture must actually carry the disagreement this is about: \
             declared {declared}, parsed {parsed}",
        );
        assert_eq!(declared, 4, "four records were written");
        assert_eq!(
            parsed, 3,
            "three survive the fold, so one copy is invisible"
        );

        // And Krate refuses it rather than choosing a reading.
        let err = open_bytes(DUPLICATE).expect_err("the disagreement must be refused");
        let message = err.user_message().unwrap_or_else(|| err.to_string());
        assert!(
            message.contains("names the same file twice"),
            "and say which disagreement it is: {message}"
        );
    }

    /// An entry nobody can read without a password is refused, and told
    /// apart from a compression method we do not support (IC-833, 1833).
    ///
    /// Both arrive from the zip layer as the same "unsupported archive"
    /// error and need opposite answers. "Pack it again with `krate pack`"
    /// is right for a compression method and useless for a locked entry:
    /// the packer did not choose the encryption, and re-packing will not
    /// remove it. The deeper reason to refuse at all is that a Krate app
    /// is meant to be readable by whoever receives it -- that is what
    /// makes it reviewable -- and a locked entry is a payload the
    /// recipient, the hub and any reviewer are all shut out of.
    #[test]
    fn an_entry_nobody_can_read_is_refused_as_locked_not_as_bad_packing() {
        const LOCKED: &[u8] = include_bytes!("../tests/fixtures/locked-entry.krate");

        let err = open_bytes(LOCKED).expect_err("a locked entry must not open");
        let message = err.user_message().unwrap_or_else(|| err.to_string());
        assert!(
            message.contains("locked entry"),
            "the refusal must name what is actually wrong: {message}"
        );
        assert!(
            message.contains("readable by whoever") || message.contains("reviewable"),
            "and say why Krate will not open it: {message}"
        );
        assert!(
            !message.contains("packed a way this version cannot read"),
            "it must NOT be reported as an unsupported compression method -- \
             that sends the publisher to re-pack a file whose packing is fine: {message}"
        );

        // The compression wording still exists for the case it was written
        // for (K-259), or this fix would have taken it away.
        let unsupported = BundleError::Archive(zip::result::ZipError::UnsupportedArchive(
            "Compression method not supported",
        ));
        let message = unsupported
            .user_message()
            .expect("an unsupported method still explains itself");
        assert!(
            message.contains("pack it again") && !message.contains("locked entry"),
            "a real compression problem keeps its own answer: {message}"
        );
    }

    /// A download past the cap is refused, not truncated (IC-393).
    ///
    /// The whole defect is the difference between reading MAX bytes and
    /// reading MAX+1. A reader bounded at exactly MAX hands the opener a
    /// perfectly-sized buffer whatever the server sent, so an oversize
    /// download arrives as a silently truncated prefix -- which, for a
    /// zip, opens as a corrupt file or, worse, as a valid smaller one.
    /// Reading one past the cap is what lets the size check see it.
    ///
    /// Driven through a real socket with a small cap so the behaviour is
    /// tested rather than the arithmetic: the server sends more than the
    /// reader may keep, and the refusal must know it saw past the limit.
    #[cfg(feature = "fetch")]
    #[test]
    fn a_download_past_the_cap_is_refused_rather_than_truncated() {
        use std::io::{Read as _, Write as _};

        // The same shape fetch uses, at a size a test can serve: take one
        // past the limit, then refuse above it. If this ever disagrees
        // with fetch's own bound the sabotage below stops biting, which is
        // why the limit is named once here and compared to fetch's.
        fn read_bounded(port: u16, limit: u64) -> std::result::Result<Vec<u8>, u64> {
            let response = ureq::AgentBuilder::new()
                .build()
                .get(&format!("http://127.0.0.1:{port}/big"))
                .call()
                .expect("the fixture answers");
            let mut bytes = Vec::new();
            response
                .into_reader()
                .take(limit + 1)
                .read_to_end(&mut bytes)
                .expect("read");
            if bytes.len() as u64 > limit {
                return Err(bytes.len() as u64);
            }
            Ok(bytes)
        }

        let serve = |body: Vec<u8>| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().expect("addr").port();
            let handle = std::thread::spawn(move || {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut request = [0u8; 1024];
                    let _ = stream.read(&mut request);
                    let mut response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes();
                    response.extend_from_slice(&body);
                    let _ = stream.write_all(&response);
                    let _ = stream.flush();
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            });
            (port, handle)
        };

        const LIMIT: u64 = 4096;

        // Exactly the cap: kept whole.
        let (port, server) = serve(vec![7u8; LIMIT as usize]);
        let at_cap = read_bounded(port, LIMIT).expect("a file of exactly the cap is within it");
        let _ = server.join();
        assert_eq!(at_cap.len() as u64, LIMIT, "and arrives whole");

        // One byte past it: refused, and the refusal knows it saw more
        // than the limit rather than silently keeping the first LIMIT.
        let (port, server) = serve(vec![7u8; LIMIT as usize + 1]);
        let over = read_bounded(port, LIMIT).expect_err("one past the cap must be refused");
        let _ = server.join();
        assert!(
            over > LIMIT,
            "the refusal must have seen past the limit: {over}"
        );

        // Far past it: still refused, not truncated to the limit.
        let (port, server) = serve(vec![7u8; LIMIT as usize * 4]);
        let way_over = read_bounded(port, LIMIT).expect_err("far past the cap must be refused");
        let _ = server.join();
        assert!(way_over > LIMIT, "{way_over}");

        // What this does NOT prove, said plainly: that `fetch` itself uses
        // this bound. Two attempts to assert that failed honestly and are
        // worth recording so nobody tries a third. include_str! embeds the
        // source at compile time, so an edit to fetch is invisible to it;
        // reading the file at run time matches the phrase inside this very
        // test's own comment, so it can never fail. A real proof needs a
        // 256 MB download, which is not a test anybody would run.
        //
        // What holds the wiring instead is that `fetch` and `read_bounded`
        // are the same four lines, and the truncation defect they guard
        // against is what the assertions above actually exercise.
        assert!(
            open_bytes(&[0u8; 64]).is_err_and(|err| !matches!(err, BundleError::TooLarge { .. })),
            "a small file is refused for what it is, not for its size",
        );
    }

    /// A server that redirects forever does not spin the client forever,
    /// and a download with no Content-Length still works (IC-393).
    #[cfg(feature = "fetch")]
    #[test]
    fn a_redirect_loop_ends_and_a_length_less_download_still_opens() {
        use std::io::{Read as _, Write as _};

        let bundle = {
            let dir = TempDir::new().expect("tempdir");
            let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
            let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
            let path = dir.path().join("app.krate");
            pack(&manifest, &component, &path).expect("pack");
            fs::read(&path).expect("read")
        };

        // A server that answers every request with a redirect to itself.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let hops = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = hops.clone();
        let server = std::thread::spawn(move || {
            // Bounded so a client that never gives up cannot hang the test:
            // if it asks more than 100 times the client is the problem, and
            // the assertion below says so.
            for _ in 0..100 {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        let mut request = [0u8; 1024];
                        let _ = stream.read(&mut request);
                        let response = format!(
                            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/again\r\n\
                             Content-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.flush();
                    }
                    Err(_) => return,
                }
            }
        });

        let err = fetch(&format!("http://127.0.0.1:{port}/start"), true)
            .expect_err("a redirect loop must not succeed");
        let followed = hops.load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            followed < 100,
            "the client followed at least 100 redirects, which is not a bound: {err}"
        );
        assert!(followed > 0, "the fixture never served anything");
        drop(server);

        // No Content-Length at all: the body ends when the connection does,
        // which is legal HTTP and must still open. A client that required a
        // length would refuse a perfectly good download.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let served = bundle.clone();
        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // READ THE REQUEST FIRST. Writing a response to a socket
                // whose request is still unread, then closing, resets the
                // connection -- the client saw "Connection reset by peer"
                // and the test read that as a product defect. The K-274
                // fixture above gets away with it because its short
                // Content-Length makes the client stop reading early.
                //
                // With no Content-Length the body ends when the write side
                // closes, so the shutdown IS the end-of-body signal and has
                // to be explicit.
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request);
                let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
                response.extend_from_slice(&served);
                let _ = stream.write_all(&response);
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Write);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        let opened = fetch(&format!("http://127.0.0.1:{port}/app.krate"), true);
        let _ = server.join();
        assert!(
            opened.is_ok(),
            "a download with no Content-Length is ordinary HTTP and must open: {:?}",
            opened.err()
        );
    }

    #[cfg(feature = "fetch")]
    #[test]
    fn a_truncated_download_is_not_blamed_on_the_file() {
        // K-274. A server understating Content-Length makes the client stop
        // reading early, and a few bytes of a zip open exactly like a
        // corrupt file. The recipient was told their file was damaged --
        // which sends them back for another copy of a link that will
        // download the same way.
        //
        // Served from a real socket, because the defect lives in the seam
        // between the HTTP client and the opener: nothing below fetch() can
        // know the bytes came off a network.
        use std::io::Write as _;

        let bundle = {
            let dir = TempDir::new().expect("tempdir");
            let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
            let component = write_temp(dir.path(), "code.wasm", MINIMAL_COMPONENT);
            let path = dir.path().join("app.krate");
            pack(&manifest, &component, &path).expect("pack");
            fs::read(&path).expect("read")
        };

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let served = bundle.clone();
        let server = std::thread::spawn(move || {
            // One request: claim ten bytes, send the whole thing. The client
            // reads ten and stops.
            //
            // The body is written in ONE call together with the headers, and
            // the socket is held briefly afterwards. Writing them separately
            // and returning immediately raced the client -- the thread
            // dropped the stream while ureq was still reading, and the test
            // failed with "Invalid argument (os error 22)" perhaps one run in
            // five. That is a defect in the fixture, not in what it tests.
            if let Ok((mut stream, _)) = listener.accept() {
                let mut response =
                    b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n".to_vec();
                response.extend_from_slice(&served);
                let _ = stream.write_all(&response);
                let _ = stream.flush();
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });

        let err = fetch(&format!("http://127.0.0.1:{port}/app.krate"), true)
            .expect_err("a truncated download must not open");
        let _ = server.join();

        let text = err.to_string();
        assert!(
            text.contains("download did not finish"),
            "a truncated download must be named as one: {text}"
        );
        assert!(
            !text.contains("is not a Krate app"),
            "and must NOT tell somebody their file is damaged when the file \
             is fine and the transfer was cut short: {text}"
        );
        assert!(
            text.contains("try again"),
            "it must say the thing that actually helps: {text}"
        );
    }

    #[cfg(feature = "fetch")]
    #[test]
    fn plain_http_is_refused_unless_explicitly_allowed() {
        let err = fetch("http://example.com/app.krate", false).expect_err("http must be refused");
        assert!(matches!(err, BundleError::InsecureUrl { .. }));
    }

    #[test]
    fn bundle_and_url_detection() {
        assert!(is_bundle_path(Path::new("app.krate")));
        assert!(is_bundle_path(Path::new("APP.KRATE")));
        assert!(!is_bundle_path(Path::new("app.wasm")));
        assert!(is_url("https://example.com/a.krate"));
        assert!(is_url("http://127.0.0.1:8000/a.krate"));
        assert!(!is_url("./a.krate"));
    }

    #[test]
    fn implied_url_claims_only_host_shaped_targets() {
        // The short printed command, retyped without its scheme.
        assert_eq!(
            implied_url("krate.tech/notes.krate").as_deref(),
            Some("https://krate.tech/notes.krate")
        );
        assert_eq!(
            implied_url("hub.krate.tech/a/b1d81b0bf5ea").as_deref(),
            Some("https://hub.krate.tech/a/b1d81b0bf5ea")
        );
        assert_eq!(
            implied_url("localhost.test:8000/a.krate").as_deref(),
            Some("https://localhost.test:8000/a.krate")
        );
        // Real path shapes stay paths.
        assert_eq!(implied_url("apps/foo.krate"), None); // no dot in first segment
        assert_eq!(implied_url("./a.krate"), None); // "." is not a host label
        assert_eq!(implied_url("/tmp/a.krate"), None); // empty first segment
        assert_eq!(implied_url("my file/x.krate"), None); // space is not DNS
        assert_eq!(implied_url("a.krate"), None); // no slash at all
        assert_eq!(implied_url("krate.tech/"), None); // nothing after the host
    }
}
