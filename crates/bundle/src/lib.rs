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

/// The manifest entry name inside a bundle.
pub mod provenance;

pub const MANIFEST_ENTRY: &str = "manifest.toml";
/// The component entry name inside a bundle.
pub const COMPONENT_ENTRY: &str = "code.wasm";
/// Root for optional portable resources inside the bundle.
pub const ASSETS_PREFIX: &str = "assets/";
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
/// `manifest.toml` in its first local-file-header. A raw `.wasm` (which starts
/// with `\0asm`) never matches, so the two are never confused.
fn looks_like_bundle_file(path: &Path) -> bool {
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    // 4-byte ZIP local-file-header signature + 26 header bytes + the file name.
    // "manifest.toml" is 13 bytes; read enough to see it if it is the first
    // entry (which `pack` always writes first).
    let mut head = [0u8; 64];
    let n = match std::io::Read::read(&mut file, &mut head) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let head = &head[..n];
    // ZIP local file header magic.
    if !head.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
        return false;
    }
    // The file name follows the 30-byte fixed local header. Look for
    // "manifest.toml" anywhere in the bytes we read (it is right there when it
    // is the first entry, and this avoids parsing header length fields).
    head.windows(b"manifest.toml".len())
        .any(|w| w == b"manifest.toml")
}

/// Whether a run target is a URL rather than a filesystem path.
pub fn is_url(target: &str) -> bool {
    target.starts_with("https://") || target.starts_with("http://")
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

    let file = File::create(output_path).map_err(|err| io_err(output_path, err))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    zip.start_file(MANIFEST_ENTRY, options)?;
    zip.write_all(manifest_text.as_bytes())
        .map_err(|err| io_err(output_path, err))?;
    zip.start_file(COMPONENT_ENTRY, options)?;
    zip.write_all(&component)
        .map_err(|err| io_err(output_path, err))?;
    if let Some(assets_dir) = assets_dir.filter(|path| path.is_dir()) {
        for (entry_name, source) in collect_assets(assets_dir)? {
            zip.start_file(entry_name, options)?;
            let mut input = File::open(&source).map_err(|err| io_err(&source, err))?;
            io::copy(&mut input, &mut zip).map_err(|err| io_err(output_path, err))?;
        }
    }
    zip.finish()?;

    let size = fs::metadata(output_path)
        .map_err(|err| io_err(output_path, err))?
        .len();
    Ok(size)
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
        Ok(provenance::digest_entries(&entries))
    }
}

/// Open a bundle from disk, extracting it into a temporary directory.
pub fn open(bundle_path: &Path) -> Result<OpenBundle> {
    let size = fs::metadata(bundle_path)
        .map_err(|err| io_err(bundle_path, err))?
        .len();
    if size > MAX_BUNDLE_BYTES {
        return Err(BundleError::TooLarge { size });
    }
    let file = File::open(bundle_path).map_err(|err| io_err(bundle_path, err))?;
    open_reader(file)
}

/// Open a bundle from any reader that can seek.
pub fn open_reader<R: Read + io::Seek>(reader: R) -> Result<OpenBundle> {
    let mut archive = ZipArchive::new(reader)?;

    let dir = TempDir::new().map_err(|err| io_err(Path::new("<tempdir>"), err))?;
    let manifest_path = dir.path().join(MANIFEST_ENTRY);
    let component_path = dir.path().join(COMPONENT_ENTRY);
    let assets_path = dir.path().join("assets");

    // Reading by exact name rather than iterating entries is what makes path
    // traversal unrepresentable: any other entry in the archive is ignored, and
    // neither name can escape the temp directory.
    extract_entry(&mut archive, MANIFEST_ENTRY, &manifest_path)?;
    extract_entry(&mut archive, COMPONENT_ENTRY, &component_path)?;
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
        manifest,
    })
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

    fn write_temp(dir: &Path, name: &str, contents: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).expect("write fixture");
        path
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
}
