//! The `.krate` bundle: one file that carries an application and the
//! permissions it is asking for.
//!
//! A bundle is a zip container holding exactly two entries at the root:
//!
//! ```text
//! app.krate
//! ├── manifest.toml   # krate-manifest schema, unchanged
//! └── code.wasm       # the component
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
//! * entry names are matched exactly against the two permitted names, which
//!   makes zip path traversal (`../../etc/passwd`) unrepresentable rather than
//!   merely filtered;
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
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use krate_manifest::Manifest;
use tempfile::TempDir;
use thiserror::Error;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

/// The manifest entry name inside a bundle.
pub const MANIFEST_ENTRY: &str = "manifest.toml";
/// The component entry name inside a bundle.
pub const COMPONENT_ENTRY: &str = "code.wasm";
/// Conventional file extension.
pub const BUNDLE_EXTENSION: &str = "krate";

/// Largest bundle we will read, compressed. Generous for a format whose
/// reference application is 26 KB, and small enough that a hostile URL cannot
/// stream gigabytes at us.
pub const MAX_BUNDLE_BYTES: u64 = 256 * 1024 * 1024;
/// Largest single entry we will decompress. Bounds the classic zip bomb, where
/// a small archive expands to an enormous file.
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024 * 1024;

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
    #[error(
        "bundle manifest declares entry `{declared}`, but a bundle always runs `{COMPONENT_ENTRY}`"
    )]
    EntryMismatch { declared: String },
    #[error("bundle is {size} bytes, larger than the {MAX_BUNDLE_BYTES} byte limit")]
    TooLarge { size: u64 },
    #[error("bundle entry `{entry}` expands to more than {MAX_ENTRY_BYTES} bytes")]
    EntryTooLarge { entry: String },
    #[error("refusing to fetch over plain HTTP: {url}\nuse https, or pass --insecure-http for a local test server")]
    InsecureUrl { url: String },
    #[error("could not fetch {url}: {message}")]
    Fetch { url: String, message: String },
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
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(BUNDLE_EXTENSION))
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
    let manifest_text =
        fs::read_to_string(manifest_path).map_err(|err| io_err(manifest_path, err))?;
    let manifest = Manifest::parse(&manifest_text).map_err(|err| BundleError::Manifest(err.to_string()))?;

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

    /// The parsed manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
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

    // Reading by exact name rather than iterating entries is what makes path
    // traversal unrepresentable: any other entry in the archive is ignored, and
    // neither name can escape the temp directory.
    extract_entry(&mut archive, MANIFEST_ENTRY, &manifest_path)?;
    extract_entry(&mut archive, COMPONENT_ENTRY, &component_path)?;

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
        manifest,
    })
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
            zip.start_file(MANIFEST_ENTRY, opts).expect("start manifest");
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
