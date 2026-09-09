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
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};

pub mod imports;

use krate_manifest::Manifest;
use tempfile::TempDir;
use thiserror::Error;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

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
/// 1 is what every bundle shipped so far implicitly is: manifest.toml and
/// code.wasm required, assets/ source/ sdk/ signature.json optional,
/// unknown entries ignored, paths compared case- and separator-insensitively.
/// Bundles written before this entry existed carry no profile line and are
/// read as generation 1, because that is what they are -- "keep existing
/// files readable as their recorded generation" is the requirement's own
/// words.
pub const PROFILE_VERSION: u32 = 1;

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

/// Conventional file extension.
pub const BUNDLE_EXTENSION: &str = "krate";

/// Largest bundle we will read, compressed. Generous for a format whose
/// reference application is 26 KB, and small enough that a hostile URL cannot
/// stream gigabytes at us.
pub const MAX_BUNDLE_BYTES: u64 = 256 * 1024 * 1024;
/// Largest single entry we will decompress. Bounds the classic zip bomb, where
/// a small archive expands to an enormous file.
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;
/// Largest individual bundled asset after decompression.
pub const MAX_ASSET_BYTES: u64 = 96 * 1024 * 1024;
/// Largest total asset payload after decompression.
pub const MAX_TOTAL_ASSET_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum number of asset files in one bundle.
pub const MAX_ASSET_COUNT: usize = 4096;

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
        "{path} is not a WebAssembly component, so it cannot be packed into \
         an app.\n\n  {detail}\n\n  \
         If this came from a build, check that the build produced a \
         component (`cargo component build`) rather than a plain module."
    )]
    NotAComponent { path: String, detail: String },
    #[error(
        "{path} uses characters outside ASCII.\n\n  \
         Two spellings of one accented name are different bytes but the same \
         file on some systems, so Krate cannot tell a reviewed copy from a \
         substituted one. Rename it to ASCII and pack again."
    )]
    NonAsciiPath { path: String },
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
}

impl BundleError {
    /// A plain, single-sentence explanation for a person, with no zip/EOCD/io
    /// jargon and no repeated wrapped error. Callers print this at the process
    /// boundary instead of the raw error chain. Returns `None` when the
    /// variant's own message is already user-facing enough to print as-is.
    pub fn user_message(&self) -> Option<String> {
        match self {
            // A missing/unreadable file: say which and why, once.
            BundleError::Io { path, source } => Some(if source.kind() == io::ErrorKind::NotFound {
                format!("no file at {}", path.display())
            } else {
                format!("could not read {}: {}", path.display(), plain_io(source))
            }),
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
    // Parsing here is the same work `open` already does at run time, moved
    // to where it is actionable.
    imports::component_imports(&component).map_err(|detail| {
        // wasmparser's own words run to several lines of hex for the
        // commonest case, a file that is not wasm at all. Say that plainly
        // and keep the detail only when it adds something.
        let detail = if detail.contains("magic header not detected") {
            "it does not start with the WebAssembly magic number".to_string()
        } else {
            detail
                .trim()
                .lines()
                .next()
                .unwrap_or("unreadable")
                .to_string()
        };
        BundleError::NotAComponent {
            path: component_path.display().to_string(),
            detail,
        }
    })?;

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
fn write_bundle_into(
    staging: &Path,
    manifest_text: &str,
    component: &[u8],
    assets_dir: Option<&Path>,
    source_dir: Option<&Path>,
    sdk_dir: Option<&Path>,
) -> Result<()> {
    let output_path = staging;
    let file = File::create(staging).map_err(|err| io_err(staging, err))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    // First entry, so a reader meets it before anything else (IC-208).
    zip.start_file(PROFILE_ENTRY, options)?;
    zip.write_all(PROFILE_VERSION.to_string().as_bytes())
        .map_err(|err| io_err(output_path, err))?;

    zip.start_file(MANIFEST_ENTRY, options)?;
    zip.write_all(manifest_text.as_bytes())
        .map_err(|err| io_err(output_path, err))?;
    zip.start_file(COMPONENT_ENTRY, options)?;
    zip.write_all(component)
        .map_err(|err| io_err(output_path, err))?;
    if let Some(assets_dir) = assets_dir.filter(|path| path.is_dir()) {
        for (entry_name, source) in collect_assets(assets_dir)? {
            zip.start_file(entry_name, options)?;
            let mut input = File::open(&source).map_err(|err| io_err(&source, err))?;
            io::copy(&mut input, &mut zip).map_err(|err| io_err(output_path, err))?;
        }
    }
    if let Some(source_dir) = source_dir.filter(|path| path.is_dir()) {
        for (entry_name, source) in collect_source(source_dir)? {
            zip.start_file(&entry_name, options)?;
            // Cargo.toml points at the SDK by absolute path, because that is
            // where this machine materialised it. Shipped as-is, the source in
            // a bundle only rebuilds on the machine that made it -- which
            // defeats the point of shipping source at all. Rewriting to a
            // placeholder lets any Krate install substitute its own SDK.
            if entry_name.ends_with("Cargo.toml") {
                let text = fs::read_to_string(&source).map_err(|err| io_err(&source, err))?;
                let rewritten = rewrite_sdk_paths(&text);
                zip.write_all(rewritten.as_bytes())
                    .map_err(|err| io_err(output_path, err))?;
                continue;
            }
            let mut input = File::open(&source).map_err(|err| io_err(&source, err))?;
            io::copy(&mut input, &mut zip).map_err(|err| io_err(output_path, err))?;
        }
    }
    if let Some(sdk_dir) = sdk_dir.filter(|path| path.is_dir()) {
        for (entry_name, source) in collect_tree(sdk_dir, SDK_PREFIX)? {
            zip.start_file(entry_name, options)?;
            let mut input = File::open(&source).map_err(|err| io_err(&source, err))?;
            io::copy(&mut input, &mut zip).map_err(|err| io_err(output_path, err))?;
        }
    }
    zip.finish()?;
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
    manifest: Manifest,
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

    /// The raw envelope, when the bundle carries one.
    pub fn signature_envelope(&self) -> Result<Option<signing::SignatureEnvelope>> {
        let path = self._dir.path().join(SIGNATURE_ENTRY);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&path).map_err(|err| io_err(&path, err))?;
        Ok(serde_json::from_slice(&bytes).ok())
    }

    /// Every entry this layer covers, read from the extracted tree.
    fn entries_for_digest(
        &self,
        layer: provenance::Layer,
    ) -> Result<std::collections::BTreeMap<String, Vec<u8>>> {
        let mut entries = std::collections::BTreeMap::new();
        entries.insert(
            MANIFEST_ENTRY.to_string(),
            fs::read(&self.manifest_path).map_err(|err| io_err(&self.manifest_path, err))?,
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
        for (root, prefix) in [
            (self.source_path.as_deref(), SOURCE_PREFIX),
            (self.sdk_path.as_deref(), SDK_PREFIX),
        ] {
            let Some(root) = root else { continue };
            for (entry_name, source) in collect_tree(root, prefix)? {
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
        let mut writer = ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
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
            writer.start_file(name.clone(), options)?;
            writer.write_all(&bytes).map_err(|err| BundleError::Io {
                path: PathBuf::from(&name),
                source: err,
            })?;
        }
        writer.start_file(SIGNATURE_ENTRY, options)?;
        writer.write_all(&json).map_err(|err| BundleError::Io {
            path: PathBuf::from(SIGNATURE_ENTRY),
            source: err,
        })?;
        writer.finish()?;
    }
    fs::rename(&temporary, bundle_path).map_err(|err| io_err(bundle_path, err))?;
    Ok(envelope)
}

/// Open a bundle from disk, extracting it into a temporary directory.
pub fn open(bundle_path: &Path) -> Result<OpenBundle> {
    let size = fs::metadata(bundle_path)
        .map_err(|err| io_err(bundle_path, err))?
        .len();
    if size > MAX_BUNDLE_BYTES {
        return Err(BundleError::TooLarge { size });
    }
    // Compare what the FILE claims against what the parser will see.
    //
    // `ZipArchive` keys entries by name, so two records with one path become
    // one entry and the LAST wins -- silently. Reading the end-of-central-
    // directory count is the only way to notice, and noticing matters: what
    // a person reviews is then not what Krate extracts (K-252).
    //
    // Only done here, where the bytes are on disk. `open_reader` takes any
    // reader and cannot re-read it without consuming the stream.
    let bytes = fs::read(bundle_path).map_err(|err| io_err(bundle_path, err))?;
    if let Some(declared) = central_directory_record_count(&bytes) {
        let parsed = ZipArchive::new(io::Cursor::new(&bytes))
            .map(|archive| archive.len())
            .unwrap_or(declared);
        if declared > parsed {
            return Err(BundleError::DuplicateEntry {
                // Name it when the records can be walked; fall back to the
                // counts only when they cannot, because a vague refusal
                // still beats opening a file whose contents are ambiguous.
                path: first_duplicate_record_name(&bytes).unwrap_or_else(|| {
                    format!("{declared} entries but only {parsed} distinct names")
                }),
            });
        }
    }

    let file = File::open(bundle_path).map_err(|err| io_err(bundle_path, err))?;
    open_reader(file)
}

/// Open a bundle from any reader that can seek.
pub fn open_reader<R: Read + io::Seek>(reader: R) -> Result<OpenBundle> {
    let mut archive = ZipArchive::new(reader)?;

    // Judge the archive as a whole before writing any of it to disk (IC-209).
    preflight_entries(&mut archive)?;

    let dir = TempDir::new().map_err(|err| io_err(Path::new("<tempdir>"), err))?;
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
    check_profile(&mut archive)?;

    extract_entry(&mut archive, MANIFEST_ENTRY, &manifest_path)?;
    extract_entry(&mut archive, COMPONENT_ENTRY, &component_path)?;
    // Optional, and by exact name like the two above. A bundle without one is
    // unsigned, which is a state Krate supports rather than an error.
    let signature_path = dir.path().join(SIGNATURE_ENTRY);
    let _ = extract_entry(&mut archive, SIGNATURE_ENTRY, &signature_path);
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

    let manifest_text =
        fs::read_to_string(&manifest_path).map_err(|err| io_err(&manifest_path, err))?;
    let manifest =
        Manifest::parse(&manifest_text).map_err(|err| BundleError::Manifest(err.to_string()))?;

    let declared = manifest.app.entry.display().to_string();
    if declared != COMPONENT_ENTRY {
        return Err(BundleError::EntryMismatch { declared });
    }

    Ok(OpenBundle {
        _dir: dir,
        manifest_path,
        component_path,
        assets_path: (!asset_names.is_empty()).then_some(assets_path),
        source_path: (!source_names.is_empty()).then_some(source_path),
        sdk_path: (!sdk_names.is_empty()).then_some(sdk_path),
        manifest,
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
        match sdk_root_in(line) {
            Some(root) => out.push_str(&line.replace(&root, SDK_PLACEHOLDER)),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    out
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

fn collect_source(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    collect_files(root, &|name| {
        matches!(
            name,
            "target"
                | "Cargo.lock"
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
fn check_profile<R: Read + io::Seek>(archive: &mut ZipArchive<R>) -> Result<()> {
    let mut entry = match archive.by_name(PROFILE_ENTRY) {
        Ok(entry) => entry,
        Err(_) => return Ok(()), // generation 1
    };
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|err| BundleError::Io {
            path: PathBuf::from(PROFILE_ENTRY),
            source: err,
        })?;
    let found = text.trim();
    match found.parse::<u32>() {
        Ok(version) if version <= PROFILE_VERSION => Ok(()),
        _ => Err(BundleError::UnsupportedProfile {
            found: found.chars().take(32).collect(),
            supported: PROFILE_VERSION,
        }),
    }
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

        // One logical path, however it is spelled. Backslashes are folded to
        // forward slashes because a zip may carry either and both name the
        // same file once extracted.
        let logical = name.replace('\\', "/").to_lowercase();
        if !seen.insert(logical) {
            return Err(BundleError::DuplicateEntry { path: name });
        }

        if name.starts_with(SOURCE_PREFIX) || name.starts_with(SDK_PREFIX) {
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

/// Fetch a bundle over the network and open it.
///
/// HTTPS is required unless `allow_insecure_http` is set, which exists so CI
/// and local development can serve a bundle from `127.0.0.1` without a
/// certificate. Fetching grants no capability: the returned bundle goes through
/// the same policy resolution as one opened from disk.
pub fn fetch(url: &str, allow_insecure_http: bool) -> Result<OpenBundle> {
    if url.starts_with("http://") && !allow_insecure_http {
        return Err(BundleError::InsecureUrl {
            url: url.to_string(),
        });
    }

    let response = ureq::get(url).call().map_err(|err| BundleError::Fetch {
        url: url.to_string(),
        message: err.to_string(),
    })?;

    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_BUNDLE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| BundleError::Fetch {
            url: url.to_string(),
            message: err.to_string(),
        })?;

    if bytes.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(BundleError::TooLarge {
            size: bytes.len() as u64,
        });
    }

    open_reader(io::Cursor::new(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

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
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");

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
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");

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
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
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

        // Now change the component inside the signed archive.
        let tampered = dir.path().join("tampered.krate");
        rewrite_entry(&bundle, &tampered, COMPONENT_ENTRY, b"\0asm-evil");
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
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
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
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
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
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
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
            writer
                .start_file("source\\lib.rs", options)
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
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
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
        let component = write_temp(dir.path(), "real.wasm", b"\0asm\x01\0\0\0");
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
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
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

    /// A profile from a future Krate is refused, and says what to do.
    #[test]
    fn a_future_container_profile_is_refused_with_words_a_person_can_act_on() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
        let bundle = dir.path().join("ok.krate");
        pack(&manifest, &component, &bundle).expect("pack");

        for claimed in ["99", "not-a-number", ""] {
            let future = dir.path().join(format!("future-{}.krate", claimed.len()));
            rewrite_entry(&bundle, &future, PROFILE_ENTRY, claimed.as_bytes());
            let err = open(&future)
                .expect_err("a profile this build does not understand must be refused");
            let text = err.to_string();
            assert!(
                text.contains("newer .krate format"),
                "must name the real problem for {claimed:?}: {text}"
            );
            assert!(
                text.contains("krate.tech/open"),
                "and what to do about it: {text}"
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
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
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
            (COMPONENT_ENTRY.to_string(), b"\0asm\x01\0\0\0".to_vec()),
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
            assert!(
                text.contains("outside ASCII"),
                "{spelling:?}: {text}"
            );
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
            let opts =
                SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            writer.start_file(MANIFEST_ENTRY, opts).expect("manifest");
            writer.write_all(MANIFEST.as_bytes()).expect("write");
            writer.start_file(COMPONENT_ENTRY, opts).expect("component");
            writer.write_all(b"\0asm\x01\0\0\0").expect("write");
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
        assert_ne!(
            forged, buf,
            "the fixture must really declare forged sizes"
        );
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
                let namelen = u16::from_le_bytes([data[found + len_at], data[found + len_at + 1]])
                    as usize;
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
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
        let bundle = dir.path().join("demo.krate");

        let size = pack(&manifest, &component, &bundle).expect("pack");
        assert!(size > 0, "bundle should not be empty");

        let opened = open(&bundle).expect("open");
        assert_eq!(opened.manifest().app.id, "com.example.demo");
        assert_eq!(
            fs::read(opened.component_path()).expect("read component"),
            b"\0asm\x01\0\0\0"
        );
        assert!(opened.assets_path().is_none());
    }

    #[test]
    fn pack_then_open_round_trips_nested_assets() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
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

    #[test]
    fn packs_the_source_so_an_app_can_be_changed_later() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");

        // A crate-shaped directory, including the two things that must NOT be
        // packed: build output, and a lock file that may not resolve elsewhere.
        write_temp(dir.path(), "Cargo.toml", b"[package]\nname = \"demo\"\n");
        write_temp(dir.path(), "Cargo.lock", b"# pinned");
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
        // The point of shipping source is rebuilding, and neither of these
        // helps with that: one is output, the other pins versions.
        assert!(!source.join("Cargo.lock").exists());
        assert!(!source.join("target").exists());
    }

    #[test]
    fn a_bundle_without_source_still_opens() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(dir.path(), "manifest.toml", MANIFEST.as_bytes());
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
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
            zip.write_all(b"\0asm\x01\0\0\0").expect("write wasm");
            zip.start_file("assets/../../evil", opts)
                .expect("start hostile asset");
            zip.write_all(b"pwned").expect("write hostile asset");
            zip.finish().expect("finish");
        }
        buffer.set_position(0);

        let err = open_reader(buffer).expect_err("asset traversal must fail");
        assert!(matches!(err, BundleError::UnsafeAssetPath { .. }));
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

    #[test]
    fn pack_refuses_a_manifest_whose_entry_is_not_the_bundle_component() {
        let dir = TempDir::new().expect("tempdir");
        let manifest = write_temp(
            dir.path(),
            "manifest.toml",
            MANIFEST.replace("code.wasm", "other.wasm").as_bytes(),
        );
        let component = write_temp(dir.path(), "code.wasm", b"\0asm\x01\0\0\0");
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
            zip.write_all(b"\0asm\x01\0\0\0").expect("write wasm");
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
