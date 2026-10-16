//! The unpacked cache: a bundle opened from a file is unpacked once and
//! reused, keyed by the digest of its bytes (IC-944).
//!
//! [`open`](super::open) unpacks every entry into a new temporary folder on
//! every launch. For a small app that is nothing; for Veloren it wrote
//! 687 MB to the temp folder each time (2.5 s of a 4.5 s start), and a run
//! that was killed left all of it behind until the next sweep.
//!
//! The rules:
//!
//! - An entry is used only for the bundle whose bytes it was made from. The
//!   digest is recomputed whenever the file's size or modification time
//!   differs from what was recorded, so the cheap check never vouches for a
//!   file that changed.
//! - An entry is made in a staging folder and renamed into place only after
//!   its marker -- written last -- lists every file with its size and
//!   modification time. A half-written entry has no marker and is never
//!   used; an entry whose files differ from its marker in any way is
//!   removed and made again.
//! - The judging is the same as [`open`](super::open)'s: a miss runs the
//!   whole-archive checks and the unpack, a hit reuses a verdict made over
//!   the same bytes by the same rules ([`RULES`]).
//! - One entry is kept per app id: making a new one removes the others for
//!   that app, except any a running app still holds (a shared lock, kept
//!   beside the entries in `.locks/`).

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};
use zip::ZipArchive;

use super::{
    io_err, lim, preflight_entries, refuse_undeclared_duplicate, sweep_abandoned_dirs, unpack_into,
    with_limits, BundleDir, BundleError, OpenBundle, Result, LOCAL_LIMITS,
};

/// What a marker is judged by: an entry made by another version of these
/// rules is made again rather than trusted. Raise it whenever what `open`
/// accepts or produces changes in a way an old entry would not reflect.
const RULES: &str = concat!("krate-bundle ", env!("CARGO_PKG_VERSION"), " opened.1");

/// The marker, written last; an entry without one was never finished.
const MARKER: &str = ".krate-opened.json";
/// Each entry's lock, held shared by every bundle open from the entry and
/// taken exclusively by a sweep before it removes one. Beside the entries,
/// in `.locks/<digest>.lock`, never inside one: on Windows a folder cannot
/// be renamed or removed while a file in it is open, so a sweep holding a
/// lock that lived in the entry could never remove that entry -- two IC-944
/// tests failed on windows-2022 with the old entry still there.
const LOCKS: &str = ".locks";
/// Where an entry's lock lived before (IC-944's first version): still left
/// out of an entry's file list, so an entry made then is not rebuilt for it.
const LEGACY_LOCK: &str = ".krate-opened.lock";
/// A staging folder's prefix: a run killed while unpacking leaves one, and
/// the sweep removes those an hour old.
const STAGING_PREFIX: &str = ".opening-";
/// Where the cheap check keeps what it last saw of each bundle file.
const SOURCES: &str = ".sources";

#[derive(serde::Serialize, serde::Deserialize)]
struct Marker {
    rules: String,
    digest: String,
    app_id: String,
    profile: u32,
    records: Vec<super::Record>,
    extensions: Vec<super::ExtensionRecord>,
    closure: Option<super::Closure>,
    has_assets: bool,
    has_source: bool,
    has_sdk: bool,
    has_ext: bool,
    /// Every file in the entry but the marker and the lock: path relative
    /// to the entry with `/`, size, modification time in nanoseconds.
    files: Vec<(String, u64, u64)>,
}

/// What the cheap check last saw of one bundle file.
#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
struct Seen {
    path: String,
    size: u64,
    modified: u64,
    digest: String,
}

/// Open the bundle at `bundle_path` through the unpacked cache at `root`,
/// judged by the local tier's limits like [`open`](super::open).
///
/// Errors are the bundle's own. A cache that cannot be used -- unwritable,
/// damaged, raced -- is not an error: the bundle is unpacked as `open`
/// would, into a staging folder, and kept if it can be.
pub fn open_cached(bundle_path: &Path, root: &Path) -> Result<OpenBundle> {
    with_limits(LOCAL_LIMITS, || open_cached_here(bundle_path, root))
}

/// The digests of the entries the cache at `root` holds, for tests and for
/// `krate doctor`-style listings.
pub fn opened_cache_entries(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().join(MARKER).is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect();
    found.sort();
    found
}

fn open_cached_here(bundle_path: &Path, root: &Path) -> Result<OpenBundle> {
    let meta = fs::metadata(bundle_path).map_err(|err| io_err(bundle_path, err))?;
    if meta.len() > lim().bundle_bytes {
        return Err(BundleError::TooLarge {
            size: meta.len(),
            limit: lim().bundle_bytes,
        });
    }
    let _ = fs::create_dir_all(root);
    sweep_abandoned_dirs(
        root,
        STAGING_PREFIX,
        std::time::Duration::from_secs(60 * 60),
    );

    let size = meta.len();
    let modified = nanos(&meta);
    let key = source_key(bundle_path);
    // The cheap check: the same file, unchanged since its digest was taken.
    if let Some(digest) = seen_digest(root, &key, bundle_path, size, modified) {
        if let Some(opened) = reuse(root, &digest) {
            return Ok(opened);
        }
    }

    // A miss: read the file once, for its digest and for the checks open
    // makes on the file as a whole.
    let bytes = fs::read(bundle_path).map_err(|err| io_err(bundle_path, err))?;
    let digest = hex(&Sha256::digest(&bytes));
    remember_seen(root, &key, bundle_path, size, modified, &digest);
    if let Some(opened) = reuse(root, &digest) {
        return Ok(opened);
    }
    refuse_undeclared_duplicate(&bytes)?;
    let mut archive = ZipArchive::new(io::Cursor::new(bytes.as_slice()))?;
    preflight_entries(&mut archive)?;
    let staging = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(root);
    let staging = match staging {
        Ok(staging) => staging,
        // No cache to be had: the ordinary temporary unpack.
        Err(_) => return super::open_archive(io::Cursor::new(bytes.as_slice())),
    };
    let unpacked = unpack_into(archive, staging)?;
    Ok(keep(root, &digest, unpacked))
}

/// The lock for the entry `digest`, opened (and made) beside the entries.
fn open_lock(root: &Path, digest: &str) -> Option<File> {
    let dir = root.join(LOCKS);
    fs::create_dir_all(&dir).ok()?;
    File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(format!("{digest}.lock")))
        .ok()
}

/// The entry for `digest`, if it is finished, made by these rules, and
/// exactly as its marker lists it. Anything else there is removed.
fn reuse(root: &Path, digest: &str) -> Option<OpenBundle> {
    let entry = root.join(digest);
    if !entry.exists() {
        return None;
    }
    let lock = open_lock(root, digest);
    if let Some(lock) = &lock {
        let _ = lock.lock_shared();
    }
    let marker = fs::read(entry.join(MARKER))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Marker>(&bytes).ok())
        .filter(|marker| marker.rules == RULES && marker.digest == digest)
        .filter(|marker| files_of(&entry).as_ref() == Some(&marker.files));
    let Some(marker) = marker else {
        // Half-written, made by other rules, or changed since: never used.
        drop(lock);
        remove_entry(&entry);
        return None;
    };
    let manifest_path = entry.join(super::MANIFEST_ENTRY);
    let manifest = fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|text| krate_manifest::Manifest::parse(&text).ok())?;
    let _ = filetime_touch(&entry.join(MARKER));
    Some(OpenBundle {
        manifest_path,
        component_path: entry.join(super::COMPONENT_ENTRY),
        assets_path: marker.has_assets.then(|| entry.join("assets")),
        source_path: marker.has_source.then(|| entry.join("source")),
        sdk_path: marker.has_sdk.then(|| entry.join("sdk")),
        ext_path: marker.has_ext.then(|| entry.join("ext")),
        manifest,
        profile: marker.profile,
        records: marker.records,
        extensions: marker.extensions,
        closure: marker.closure,
        _dir: BundleDir::Kept {
            path: entry,
            _lock: lock,
        },
    })
}

/// Make the staging folder `unpacked` lives in the entry for `digest`:
/// marker last, then one rename. On any failure the bundle comes back as it
/// was, still a temporary unpack, so the run goes on.
fn keep(root: &Path, digest: &str, unpacked: OpenBundle) -> OpenBundle {
    let BundleDir::Temp(_) = &unpacked._dir else {
        return unpacked;
    };
    let staging = unpacked._dir.path().to_path_buf();
    let Some(files) = files_of(&staging) else {
        return unpacked;
    };
    let marker = Marker {
        rules: RULES.to_string(),
        digest: digest.to_string(),
        app_id: unpacked.manifest.app.id.clone(),
        profile: unpacked.profile,
        records: unpacked.records.clone(),
        extensions: unpacked.extensions.clone(),
        closure: unpacked.closure.clone(),
        has_assets: unpacked.assets_path.is_some(),
        has_source: unpacked.source_path.is_some(),
        has_sdk: unpacked.sdk_path.is_some(),
        has_ext: unpacked.ext_path.is_some(),
        files,
    };
    let Ok(json) = serde_json::to_vec(&marker) else {
        return unpacked;
    };
    if fs::write(staging.join(MARKER), json).is_err() {
        return unpacked;
    }
    let entry = root.join(digest);
    // Another run may have made it meanwhile; theirs is as good as ours.
    if fs::rename(&staging, &entry).is_err() {
        return match reuse(root, digest) {
            Some(theirs) => theirs,
            None => unpacked,
        };
    }
    // The staging folder is gone (renamed), so the TempDir must not try to
    // remove it; the bundle now points into the entry.
    let OpenBundle {
        _dir,
        manifest_path,
        component_path,
        assets_path,
        source_path,
        sdk_path,
        ext_path,
        manifest,
        profile,
        records,
        extensions,
        closure,
    } = unpacked;
    if let BundleDir::Temp(dir) = _dir {
        let _ = dir.keep();
    }
    let moved = |path: PathBuf| match path.strip_prefix(&staging) {
        Ok(rest) => entry.join(rest),
        Err(_) => path,
    };
    let lock = open_lock(root, digest);
    if let Some(lock) = &lock {
        let _ = lock.lock_shared();
    }
    let kept = OpenBundle {
        _dir: BundleDir::Kept {
            path: entry.clone(),
            _lock: lock,
        },
        manifest_path: moved(manifest_path),
        component_path: moved(component_path),
        assets_path: assets_path.map(moved),
        source_path: source_path.map(moved),
        sdk_path: sdk_path.map(moved),
        ext_path: ext_path.map(moved),
        manifest,
        profile,
        records,
        extensions,
        closure,
    };
    sweep_app(root, &kept.manifest.app.id, digest);
    kept
}

/// Remove this app's other entries, except one a running app holds.
fn sweep_app(root: &Path, app_id: &str, keep: &str) {
    for entry in fs::read_dir(root).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == keep || name.starts_with('.') || !path.is_dir() {
            continue;
        }
        let same_app = fs::read(path.join(MARKER))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Marker>(&bytes).ok())
            .is_some_and(|marker| marker.app_id == app_id);
        if !same_app {
            continue;
        }
        let Some(lock) = open_lock(root, name) else {
            continue;
        };
        // In use: a running copy of the older version keeps its files.
        if lock.try_lock().is_err() {
            continue;
        }
        // Held through the removal, so no open can take the entry meanwhile;
        // the lock is not in the entry, so holding it does not pin it.
        remove_entry(&path);
        // Remove the lock file while we still hold it exclusively. Safe on
        // Unix (unlink while open is fine); on Windows an open file cannot
        // be deleted, so this may fail -- stale locks are cleaned later.
        let lock_path = root.join(LOCKS).join(format!("{name}.lock"));
        let _ = fs::remove_file(&lock_path);
        drop(lock);
    }
    // Clean any leftover lock files whose entries are gone and that are not
    // currently held (covers the Windows case and any races).
    clean_stale_locks(root);
}

/// Remove an entry: renamed aside first, so nothing ever sees it half gone
/// under its own name.
fn remove_entry(entry: &Path) {
    let Some(root) = entry.parent() else {
        return;
    };
    let aside = root.join(format!(
        "{STAGING_PREFIX}gone-{}",
        entry
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("entry")
    ));
    let _ = fs::remove_dir_all(&aside);
    if fs::rename(entry, &aside).is_ok() {
        let _ = fs::remove_dir_all(&aside);
    }
}

/// Remove lock files that have no corresponding entry and are not held.
/// Covers the case where removal while open failed (Windows) and any
/// other leftovers.
fn clean_stale_locks(root: &Path) {
    let locks_dir = root.join(LOCKS);
    let Ok(entries) = fs::read_dir(&locks_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !name.ends_with(".lock") {
            continue;
        }
        let digest = &name[..name.len() - 5];
        // Still has an entry: leave it.
        if root.join(digest).join(MARKER).is_file() {
            continue;
        }
        // Try to take the lock exclusively; if we can, nobody holds it.
        if let Ok(file) = File::options().read(true).write(true).open(&path) {
            if file.try_lock().is_ok() {
                drop(file);
                let _ = fs::remove_file(&path);
            }
        }
    }
}

/// Every file below `dir` but the marker and the lock, sorted, with size
/// and modification time. `None` when the tree cannot be read.
fn files_of(dir: &Path) -> Option<Vec<(String, u64, u64)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            let kind = entry.file_type().ok()?;
            if kind.is_symlink() {
                // Nothing open writes makes one; one here was put here.
                return None;
            }
            if kind.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path.strip_prefix(dir).ok()?;
            let name = relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if name == MARKER || name == LEGACY_LOCK {
                continue;
            }
            let meta = entry.metadata().ok()?;
            out.push((name, meta.len(), nanos(&meta)));
        }
    }
    out.sort();
    Some(out)
}

fn nanos(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A file name for what the cheap check saw of one bundle path.
fn source_key(bundle_path: &Path) -> String {
    let path = fs::canonicalize(bundle_path).unwrap_or_else(|_| bundle_path.to_path_buf());
    hex(&Sha256::digest(path.to_string_lossy().as_bytes()))[..32].to_string()
}

fn seen_digest(
    root: &Path,
    key: &str,
    bundle_path: &Path,
    size: u64,
    modified: u64,
) -> Option<String> {
    let bytes = fs::read(root.join(SOURCES).join(format!("{key}.json"))).ok()?;
    let seen: Seen = serde_json::from_slice(&bytes).ok()?;
    let path = fs::canonicalize(bundle_path).ok()?;
    (seen.path == path.to_string_lossy()
        && seen.size == size
        && seen.modified == modified
        && modified != 0
        && seen.digest.len() == 64)
        .then_some(seen.digest)
}

fn remember_seen(
    root: &Path,
    key: &str,
    bundle_path: &Path,
    size: u64,
    modified: u64,
    digest: &str,
) {
    let Ok(path) = fs::canonicalize(bundle_path) else {
        return;
    };
    let seen = Seen {
        path: path.to_string_lossy().into_owned(),
        size,
        modified,
        digest: digest.to_string(),
    };
    let dir = root.join(SOURCES);
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(json) = serde_json::to_vec(&seen) {
        // Written beside and renamed, so a reader never sees half of it.
        let tmp = dir.join(format!("{key}.{}.tmp", std::process::id()));
        if fs::write(&tmp, json).is_ok()
            && fs::rename(&tmp, dir.join(format!("{key}.json"))).is_err()
        {
            let _ = fs::remove_file(&tmp);
        }
    }
}

/// Mark an entry as used now, so a person reading the folder can tell the
/// live one from the rest. Best effort.
fn filetime_touch(path: &Path) -> io::Result<()> {
    let file = File::options().write(true).open(path)?;
    file.set_modified(std::time::SystemTime::now())
}
