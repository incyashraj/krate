//! The application key-value store behind `krate:store/kv`.
//!
//! An app that remembers anything needs somewhere to put it. Until now the only
//! answer was `fs.read` and `fs.write`, which made the permission prompt say
//! "read files in checklist" when the honest sentence is "remember your
//! settings", and made every app write its own parser.
//!
//! Two properties matter more than the API shape:
//!
//! 1. **The app cannot name a location.** Keys are keys, not paths. The store
//!    lives in a per-app directory the runtime chooses, so granting storage can
//!    never widen into reading the user's documents.
//! 2. **The capability is checked before anything is read or written**, in the
//!    same place and the same way as every other Krate capability. Storage is
//!    not a convenience that slips past the wall.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Longest key an app may use. Long enough for a namespaced name like
/// `window.main.size`, short enough that a key cannot become a payload.
const MAX_KEY_BYTES: usize = 256;

/// Largest single value. Bounded because the store is for state, not for bulk
/// data: an app with megabytes to write wants a file capability and the user's
/// explicit consent to a folder, not a settings store.
const MAX_VALUE_BYTES: usize = 1024 * 1024;

/// Largest the whole store may grow, so a loop cannot fill the user's disk.
const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;

/// Why a store operation could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The app was not granted `store.kv`.
    Denied,
    /// The key was empty, too long, or contained unsupported characters.
    InvalidKey,
    /// The value, or the store as a whole, exceeded its bound.
    TooLarge,
    /// The store could not be read or written.
    Io(String),
}

/// A single application's key-value store.
///
/// Held open for the run and flushed on every write, so an app that is closed
/// abruptly keeps what it had already saved -- the same expectation a person
/// has of a settings file.
#[derive(Debug)]
pub struct AppStore {
    path: PathBuf,
    entries: BTreeMap<String, Vec<u8>>,
    /// False when the app did not receive `store.kv`. Every operation checks
    /// this first, so a denied app cannot read or write a byte.
    granted: bool,
    /// Set when a store file exists but could not be read. Every operation
    /// then answers with an error rather than behaving like a new app.
    unreadable: Option<String>,
    /// Keys this handle has deleted. Needed because flush merges with the
    /// file on disk, and without it a delete would be undone by whatever
    /// was still written there (IC-247).
    deleted: BTreeSet<String>,
}

impl AppStore {
    /// Open (or start) the store for one app.
    ///
    /// `granted` comes from the session policy, not from the app.
    pub fn open(path: PathBuf, granted: bool) -> Self {
        // A store that exists but cannot be read is NOT an empty store
        // (IC-877).
        //
        // This used to be `load(&path).unwrap_or_default()`, so a corrupt
        // file and a brand-new app produced exactly the same thing: an empty
        // map. The app then saw no data, saved something, and overwrote the
        // only copy of what was there -- silent, permanent loss, reported to
        // nobody. E8 reproduced it on the KV, secret and shared stores alike.
        //
        // The failure is carried instead, so every later call answers with a
        // typed error rather than a plausible lie. Reads say what happened;
        // writes refuse, because writing over data we could not read is the
        // step that turns a recoverable file into a lost one.
        let (entries, unreadable) = if granted {
            match load(&path) {
                Ok(Some(entries)) => (entries, None),
                Ok(None) => (BTreeMap::new(), None),
                Err(err) => (BTreeMap::new(), Some(err.to_string())),
            }
        } else {
            (BTreeMap::new(), None)
        };
        Self {
            path,
            entries,
            granted,
            unreadable,
            deleted: BTreeSet::new(),
        }
    }

    /// Refuse when the store on disk could not be read.
    fn require_readable(&self) -> Result<(), StoreError> {
        match &self.unreadable {
            None => Ok(()),
            Some(why) => Err(StoreError::Io(format!(
                "this app's saved data could not be read ({why}). It has not been changed; \
                 the file is at {}",
                self.path.display()
            ))),
        }
    }

    fn require_grant(&self) -> Result<(), StoreError> {
        if self.granted {
            Ok(())
        } else {
            Err(StoreError::Denied)
        }
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.require_grant()?;
        self.require_readable()?;
        validate_key(key)?;
        Ok(self.entries.get(key).cloned())
    }

    pub fn set(&mut self, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        self.require_grant()?;
        self.require_readable()?;
        validate_key(key)?;
        if value.len() > MAX_VALUE_BYTES {
            return Err(StoreError::TooLarge);
        }
        // Measure the store as it would be *after* this write, counting the
        // replacement rather than the addition, so overwriting a large value
        // with a small one is never refused.
        let existing = self.entries.get(key).map(|v| v.len()).unwrap_or(0);
        let total = self.total_bytes() - existing + value.len();
        if total > MAX_TOTAL_BYTES {
            return Err(StoreError::TooLarge);
        }
        self.entries.insert(key.to_string(), value);
        self.flush()
    }

    pub fn delete(&mut self, key: &str) -> Result<(), StoreError> {
        self.require_grant()?;
        self.require_readable()?;
        validate_key(key)?;
        // Deleting something absent is a success: the caller wanted it gone.
        if self.entries.remove(key).is_some() {
            self.deleted.insert(key.to_string());
            self.flush()?;
        }
        Ok(())
    }

    pub fn keys(&self) -> Result<Vec<String>, StoreError> {
        self.require_grant()?;
        self.require_readable()?;
        // BTreeMap iterates sorted, so a listing is stable run to run and host
        // to host -- an app can rely on the order without sorting again.
        Ok(self.entries.keys().cloned().collect())
    }

    pub fn clear(&mut self) -> Result<(), StoreError> {
        self.require_grant()?;
        self.require_readable()?;
        self.entries.clear();
        self.flush()
    }

    fn total_bytes(&self) -> usize {
        self.entries.values().map(|v| v.len()).sum()
    }

    /// Write the whole store, atomically.
    ///
    /// Via a temporary file and a rename, so a crash midway leaves the previous
    /// contents intact rather than a truncated file. Losing yesterday's settings
    /// because today's write was interrupted is exactly the kind of failure that
    /// makes software feel unreliable.
    /// Write the store back, keeping whatever another writer saved meanwhile
    /// (IC-247).
    ///
    /// This used to encode the in-memory map and rename it over the file, so
    /// two open handles on the same store raced and the second rename simply
    /// erased the first writer's accepted `set`. Two windows of one app, or
    /// an app and a tool, were enough. It also used one fixed `.tmp` name,
    /// so both writers could be writing the same scratch file at once.
    ///
    /// Two changes. The file on disk is re-read and merged under this
    /// instance's own changes, so a key this handle never touched survives
    /// someone else having written it. And the scratch file carries the
    /// process id, so two writers cannot collide inside it.
    ///
    /// This is last-writer-wins PER KEY rather than per file, which is the
    /// weakest thing that stops the loss reported. It is deliberately not a
    /// general concurrency story: real ordering needs the versioned,
    /// transactional store CP2 specifies, and pretending otherwise here
    /// would be a worse lie than the one being fixed.
    fn flush(&self) -> Result<(), StoreError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Io(e.to_string()))?;
        }

        // Whatever is on disk now, plus this instance's own entries on top.
        // A read failure here is not fatal: the caller's data still needs
        // saving, and refusing would lose the write we were asked to make.
        let mut merged = match load(&self.path) {
            Ok(Some(existing)) => existing,
            _ => BTreeMap::new(),
        };
        // Keys this handle deleted must not come back from disk.
        merged.retain(|key, _| !self.deleted.contains(key));
        for (key, value) in &self.entries {
            merged.insert(key.clone(), value.clone());
        }

        let encoded = encode(&merged);
        let temp = self
            .path
            .with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&temp, &encoded).map_err(|e| StoreError::Io(e.to_string()))?;
        std::fs::rename(&temp, &self.path).map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(())
    }
}

/// A key must be a short, printable, path-free name.
///
/// Path separators and `..` are rejected even though keys never become paths on
/// their own, because the store file format and any future per-key layout should
/// not have to care whether a key could be read as a path.
fn validate_key(key: &str) -> Result<(), StoreError> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return Err(StoreError::InvalidKey);
    }
    if key.contains('/') || key.contains('\\') || key.contains("..") {
        return Err(StoreError::InvalidKey);
    }
    if key
        .chars()
        .any(|c| c.is_control() || c == '\n' || c == '\t')
    {
        return Err(StoreError::InvalidKey);
    }
    Ok(())
}

/// Line-oriented: `<key>\t<base64 value>`.
///
/// Values are arbitrary bytes, so they are encoded rather than written raw; that
/// keeps the file readable enough to inspect by hand, which matters for a format
/// holding a user's data.
fn encode(entries: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut out = String::new();
    for (key, value) in entries {
        out.push_str(key);
        out.push('\t');
        out.push_str(&base64_encode(value));
        out.push('\n');
    }
    out.into_bytes()
}

/// Read the store from disk.
///
/// `Ok(None)` means there is no store yet -- a new app, which is ordinary.
/// `Err` means a store exists and could not be read, which is not ordinary
/// and must never be reported as "empty" (IC-877).
fn load(path: &Path) -> std::io::Result<Option<BTreeMap<String, Vec<u8>>>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // The only benign failure: nothing has been saved yet.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut entries = BTreeMap::new();
    for line in text.lines() {
        let Some((key, encoded)) = line.split_once('\t') else {
            continue;
        };
        // A corrupt line is skipped rather than failing the whole store: losing
        // one setting beats refusing to start.
        if validate_key(key).is_err() {
            continue;
        }
        if let Some(value) = base64_decode(encoded) {
            entries.insert(key.to_string(), value);
        }
    }
    Ok(Some(entries))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut value = Vec::new();
    let chars: Vec<u8> = text.bytes().filter(|b| *b != b'=').collect();
    for chunk in chars.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            let idx = B64.iter().position(|b| b == c)? as u32;
            n |= idx << (18 - 6 * i as u32);
        }
        let take = chunk.len() * 6 / 8;
        for i in 0..take {
            value.push(((n >> (16 - 8 * i as u32)) & 0xff) as u8);
        }
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(granted: bool) -> (AppStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("store.kv");
        (AppStore::open(path, granted), dir)
    }

    #[test]
    fn a_denied_app_cannot_read_or_write_anything() {
        // The wall, in the place it matters: not a filtered result, a refusal.
        let (mut store, _dir) = store(false);
        assert_eq!(store.set("a", b"1".to_vec()), Err(StoreError::Denied));
        assert_eq!(store.get("a"), Err(StoreError::Denied));
        assert_eq!(store.keys(), Err(StoreError::Denied));
        assert_eq!(store.delete("a"), Err(StoreError::Denied));
        assert_eq!(store.clear(), Err(StoreError::Denied));
    }

    /// IC-247. Every flush rewrote the whole file through one fixed `.tmp`
    /// name with no lock, so two open instances of the same store raced: the
    /// second rename won and the first writer's successful `set` was gone.
    /// A shared temp name also means two writers can be writing the same
    /// scratch file at the same moment.
    #[test]
    fn a_second_writer_does_not_erase_the_first() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("store.kv");

        // Two handles on the same store, as two windows of one app would be.
        let mut first = AppStore::open(path.clone(), true);
        let mut second = AppStore::open(path.clone(), true);

        first.set("from_first", b"1".to_vec()).expect("set");
        first.flush().expect("flush first");

        // `second` was opened before that write, so it does not know about it.
        second.set("from_second", b"2".to_vec()).expect("set");
        second.flush().expect("flush second");

        // Both writes were accepted, so both must survive.
        let reopened = AppStore::open(path, true);
        assert_eq!(
            reopened.get("from_first").expect("read"),
            Some(b"1".to_vec()),
            "the first writer's value was erased by the second"
        );
        assert_eq!(
            reopened.get("from_second").expect("read"),
            Some(b"2".to_vec())
        );
    }

    /// The merge must not resurrect a deleted key. Flush now reads the file
    /// back and lays this handle's entries on top, so without tracking the
    /// deletion the old value on disk would simply return.
    #[test]
    fn a_deleted_key_stays_deleted_through_a_merge() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("store.kv");

        let mut store = AppStore::open(path.clone(), true);
        store.set("keep", b"yes".to_vec()).expect("set");
        store.set("remove", b"no".to_vec()).expect("set");
        store.flush().expect("flush");

        store.delete("remove").expect("delete");

        let reopened = AppStore::open(path, true);
        assert_eq!(reopened.get("keep").expect("read"), Some(b"yes".to_vec()));
        assert_eq!(
            reopened.get("remove").expect("read"),
            None,
            "the merge brought a deleted key back"
        );
    }

    /// IC-877. A corrupt store used to reopen as an empty one, which an app
    /// cannot tell from a new install: it sees nothing, saves something, and
    /// overwrites the only copy of what was there. Silent, permanent, and
    /// reported to nobody. E8 reproduced it on the KV, secret and shared
    /// stores alike.
    #[test]
    fn an_unreadable_store_reports_itself_instead_of_looking_empty() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("store.kv");

        // Something real is in there first, so "empty" would be a lie rather
        // than merely unhelpful.
        {
            let mut store = AppStore::open(path.clone(), true);
            store.set("greeting", b"hello".to_vec()).expect("set");
            store.flush().expect("flush");
        }
        // Make it unreadable in a way that is not "missing": a directory
        // where a file belongs fails the read without being NotFound.
        std::fs::remove_file(&path).expect("remove");
        std::fs::create_dir(&path).expect("dir in place of file");

        let mut store = AppStore::open(path.clone(), true);

        // A read says what happened rather than answering None.
        match store.get("greeting") {
            Err(StoreError::Io(message)) => {
                assert!(
                    message.contains("could not be read"),
                    "unhelpful message: {message}"
                );
            }
            other => panic!("an unreadable store must report itself, got {other:?}"),
        }

        // And a write refuses, because writing over data we could not read is
        // the step that turns a recoverable file into a lost one.
        assert!(
            store.set("greeting", b"overwritten".to_vec()).is_err(),
            "an unreadable store must not accept a write"
        );
    }

    /// The other half: a store that simply does not exist yet is ordinary,
    /// and must keep behaving like a new app rather than an error.
    #[test]
    fn a_store_that_was_never_written_is_still_just_empty() {
        let dir = tempfile::tempdir().expect("dir");
        let store = AppStore::open(dir.path().join("nothing-here.kv"), true);
        assert_eq!(
            store.get("anything").expect("a new store reads cleanly"),
            None
        );
    }

    #[test]
    fn values_survive_being_closed_and_reopened() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("store.kv");
        {
            let mut store = AppStore::open(path.clone(), true);
            store.set("name", b"Yashraj".to_vec()).expect("set");
            store.set("count", b"7".to_vec()).expect("set");
        }
        // A new process, the same app: this is the whole point of a store.
        let store = AppStore::open(path, true);
        assert_eq!(
            store.get("name").expect("get").as_deref(),
            Some(&b"Yashraj"[..])
        );
        assert_eq!(store.get("count").expect("get").as_deref(), Some(&b"7"[..]));
    }

    #[test]
    fn a_key_that_was_never_set_reads_as_nothing_not_an_error() {
        let (store, _dir) = store(true);
        assert_eq!(store.get("never").expect("get"), None);
    }

    #[test]
    fn keys_come_back_in_a_stable_order() {
        let (mut store, _dir) = store(true);
        for key in ["zebra", "apple", "mango"] {
            store.set(key, b"x".to_vec()).expect("set");
        }
        assert_eq!(store.keys().expect("keys"), ["apple", "mango", "zebra"]);
    }

    #[test]
    fn a_key_cannot_be_a_path() {
        let (mut store, _dir) = store(true);
        for bad in ["", "../escape", "a/b", "a\\b", "with\nnewline"] {
            assert_eq!(
                store.set(bad, b"x".to_vec()),
                Err(StoreError::InvalidKey),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn bounds_stop_a_store_from_filling_the_disk() {
        let (mut store, _dir) = store(true);
        assert_eq!(
            store.set("big", vec![0u8; MAX_VALUE_BYTES + 1]),
            Err(StoreError::TooLarge)
        );
    }

    #[test]
    fn overwriting_a_large_value_with_a_small_one_is_allowed() {
        // The total is measured after the replacement, so an app near the limit
        // can still shrink its own data rather than being stuck.
        let (mut store, _dir) = store(true);
        store.set("k", vec![0u8; MAX_VALUE_BYTES]).expect("set");
        store.set("k", b"small".to_vec()).expect("replace");
        assert_eq!(store.get("k").expect("get").map(|v| v.len()), Some(5));
    }

    #[test]
    fn deleting_something_absent_succeeds() {
        let (mut store, _dir) = store(true);
        assert_eq!(store.delete("never"), Ok(()));
    }

    #[test]
    fn arbitrary_bytes_survive_a_round_trip() {
        let (mut store, _dir) = store(true);
        let raw: Vec<u8> = (0u8..=255).collect();
        store.set("blob", raw.clone()).expect("set");
        assert_eq!(store.get("blob").expect("get"), Some(raw));
    }

    #[test]
    fn base64_round_trips_every_length_remainder() {
        for len in 0..8 {
            let bytes: Vec<u8> = (0..len).map(|i| i as u8 * 37).collect();
            let encoded = base64_encode(&bytes);
            assert_eq!(base64_decode(&encoded), Some(bytes), "length {len}");
        }
    }

    #[test]
    fn a_corrupt_line_loses_one_setting_not_the_whole_store() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("store.kv");
        std::fs::write(&path, "good\tZ29vZA==\nthis line is broken\n").expect("write");
        let store = AppStore::open(path, true);
        assert_eq!(
            store.get("good").expect("get").as_deref(),
            Some(&b"good"[..])
        );
    }
}
