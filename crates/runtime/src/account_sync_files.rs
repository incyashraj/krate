//! An app's own files -- its sandbox -- kept the same on every device a
//! person signs in on (IC-981 S3, F-321), with no code in the app.
//!
//! When the run's [`AccountSyncContext`] names the app's own folder
//! (`files`), a thread looks at that folder at open, every [`PERIOD`] while
//! the app runs and once more at close, and exchanges what changed with
//! the hub. The app never waits for it: it reads and writes its files
//! exactly as before, through the same calls, and they are on its own disk.
//!
//! A version is a whole file. The rule is `store.kv`'s, per path
//! (crates/accountsync `Replica`): one side changed, take it; both changed
//! to the same bytes, take it; both changed differently, or one deleted
//! what the other changed, a conflict -- the app goes on reading this
//! device's file, the other device's copy waits beside the store (never in
//! the app's folder) until the person chooses: this device's, the other's,
//! both (the other kept as `name (Galaxy S22 Ultra).ext`), or a merge,
//! offered only when both are JSON.
//!
//! What it keeps, beside the app's store (crates/accountsync `toggles`):
//! `<store>.sync/<slot>.files.json` -- the replica, what each file looked
//! like when last read (so a file nobody touched is not read again), and
//! where each version's sealed body is on the hub -- and
//! `<store>.sync/<slot>.held/`, the other devices' copies waiting for a
//! choice. The toggles are the store's: off for the app or for everything
//! stops the files too.
//!
//! Limits, said to the person: a file over 16 MB, or past the first
//! [`MAX_FILES`] of a folder, stays on the device it is on; it is never
//! sent, and never deleted or replaced by sync.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use krate_accountsync::files::{self, Body, FileKey, FileValue, FILE_MAX, MAX_FILES, TEMP_PREFIX};
use krate_accountsync::hub::HubError;
use krate_accountsync::replica::Outgoing;
use krate_accountsync::seal::Sealed;
use krate_accountsync::toggles::{files_state_path, held_dir, sync_wanted};
use krate_accountsync::{Replica, Resolution, ResolveError, Version, VersionVector};

use crate::account_sync_host::{spawn_loop, AccountSyncContext, Session, Step, Wake, CLOSE_WAIT};

/// What sync remembers of one file as it last read it: so a file whose
/// size and time are unchanged is not read again -- unless it was read
/// within two seconds of being written, when a second write could share
/// its time.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Seen {
    size: u64,
    /// Modified, in nanoseconds since 1970.
    modified: u64,
    /// When this was read, the same.
    checked: u64,
    hash: String,
}

/// Where one version's contents are on the hub, and when it was written.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct KnownBody {
    blob: String,
    k: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<u64>,
}

impl KnownBody {
    fn body(&self) -> Body {
        Body {
            blob: self.blob.clone(),
            k: self.k,
            at: self.at,
        }
    }
}

/// Everything sync keeps about one app's files for one account.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct FilesState {
    replica: Replica,
    #[serde(default)]
    index: BTreeMap<String, Seen>,
    /// Per path, per contents hash: the body the hub keeps for it. A
    /// version sent again, or settled on contents the hub already has, is
    /// named without sending the bytes again.
    #[serde(default)]
    bodies: BTreeMap<String, BTreeMap<String, KnownBody>>,
    /// The app's folder as the last run knew it: so a choice made with the
    /// app closed can be put in place.
    #[serde(default)]
    folder: Option<PathBuf>,
    /// Files this device keeps to itself: over [`FILE_MAX`], or past
    /// [`MAX_FILES`].
    #[serde(default)]
    kept_here: Vec<String>,
}

impl FilesState {
    fn new(device: &str) -> FilesState {
        FilesState {
            replica: Replica::new(device),
            index: BTreeMap::new(),
            bodies: BTreeMap::new(),
            folder: None,
            kept_here: Vec::new(),
        }
    }
}

fn read_state(path: &Path) -> Option<FilesState> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn write_state(path: &Path, state: &FilesState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string(state).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| e.to_string())
}

fn nanos(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    nanos(SystemTime::now()) / 1_000_000_000
}

fn modified(meta: &std::fs::Metadata) -> u64 {
    meta.modified().map(nanos).unwrap_or(0)
}

/// What one look at the folder found.
#[derive(Default)]
struct Scanned {
    files: BTreeMap<String, (FileValue, Seen)>,
    /// Too large, or past the first [`MAX_FILES`]: this device's alone.
    kept_here: Vec<String>,
}

/// Look at every file in `folder`: regular files only (never a link), by
/// paths sync may write ([`files::valid_path`]), at most 32 folders deep.
fn scan(folder: &Path, index: &BTreeMap<String, Seen>) -> Scanned {
    let mut out = Scanned::default();
    let now = nanos(SystemTime::now());
    let mut stack = vec![(folder.to_path_buf(), String::new(), 0usize)];
    while let Some((dir, prefix, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if !files::valid_path(&path) {
                continue;
            }
            let Ok(meta) = entry.path().symlink_metadata() else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if depth < 32 {
                    stack.push((entry.path(), path, depth + 1));
                }
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            if meta.len() > FILE_MAX || out.files.len() >= MAX_FILES {
                out.kept_here.push(path);
                continue;
            }
            let at = modified(&meta);
            let known = index.get(&path).filter(|s| {
                s.size == meta.len()
                    && s.modified == at
                    && s.checked > at.saturating_add(2_000_000_000)
            });
            let seen = match known {
                Some(seen) => seen.clone(),
                None => {
                    let Ok(bytes) = std::fs::read(entry.path()) else {
                        continue;
                    };
                    Seen {
                        size: bytes.len() as u64,
                        modified: at,
                        checked: now,
                        hash: krate_accountsync::value_hash(&bytes),
                    }
                }
            };
            let value = FileValue {
                hash: seen.hash.clone(),
                size: seen.size,
            };
            out.files.insert(path, (value, seen));
        }
    }
    out.kept_here.sort();
    out
}

/// The file at `path` in `folder`, as sync may touch it: a regular file
/// (or nothing), every folder above it a real folder, never a link.
fn place(folder: &Path, path: &str, create: bool) -> Result<PathBuf, String> {
    if !files::valid_path(path) {
        return Err(format!("{path:?} is not a name sync writes"));
    }
    let parts: Vec<&str> = path.split('/').collect();
    let mut dir = folder.to_path_buf();
    for part in &parts[..parts.len() - 1] {
        dir.push(part);
        match dir.symlink_metadata() {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => return Err(format!("{path}: something else is where its folder goes")),
            Err(_) if create => std::fs::create_dir(&dir).map_err(|e| format!("{path}: {e}"))?,
            Err(_) => return Ok(dir.join(parts[parts.len() - 1])),
        }
    }
    let target = dir.join(parts[parts.len() - 1]);
    if let Ok(meta) = target.symlink_metadata() {
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(format!("{path}: something other than a file is there"));
        }
    }
    Ok(target)
}

/// Put `bytes` at `path`, whole: written beside it, then moved over it, so
/// the app never reads half a file.
fn put_in_place(folder: &Path, path: &str, bytes: &[u8]) -> Result<Seen, String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let target = place(folder, path, true)?;
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = target.with_file_name(format!("{TEMP_PREFIX}-{}-{n}.tmp", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|e| format!("{path}: {e}"))?;
    if let Err(e) = std::fs::rename(&temp, &target) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("{path}: {e}"));
    }
    let meta = target.metadata().map_err(|e| format!("{path}: {e}"))?;
    Ok(Seen {
        size: bytes.len() as u64,
        modified: modified(&meta),
        checked: nanos(SystemTime::now()),
        hash: krate_accountsync::value_hash(bytes),
    })
}

fn take_away(folder: &Path, path: &str) -> Result<(), String> {
    let target = place(folder, path, false)?;
    match std::fs::remove_file(&target) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{path}: {e}")),
        _ => Ok(()),
    }
}

/// The size and time of the file at `path` now, or `None`.
fn stat(folder: &Path, path: &str) -> Option<(u64, u64)> {
    let target = place(folder, path, false).ok()?;
    let meta = target.symlink_metadata().ok()?;
    meta.is_file().then(|| (meta.len(), modified(&meta)))
}

/// The other device's copy of a file, waiting for a choice.
fn held_body(held: &Path, hash: &str) -> Option<Vec<u8>> {
    let bytes = std::fs::read(held.join(hash)).ok()?;
    (krate_accountsync::value_hash(&bytes) == hash).then_some(bytes)
}

fn hold_body(held: &Path, hash: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(held).map_err(|e| e.to_string())?;
    let temp = held.join(format!("{hash}.{}.tmp", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, held.join(hash)).map_err(|e| e.to_string())
}

/// Keep only the copies a conflict still waits on.
fn prune_held(held: &Path, replica: &Replica) {
    let wanted: BTreeSet<String> = replica
        .conflicts()
        .iter()
        .flat_map(|c| c.others.iter())
        .filter_map(|v| v.value.as_deref().and_then(FileValue::decode))
        .map(|v| v.hash)
        .collect();
    for entry in std::fs::read_dir(held).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !wanted.contains(&name) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// One side of a file conflict, as Krate shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSide {
    pub size: u64,
    /// When it was written, seconds since 1970 (the writer's clock).
    pub at: Option<u64>,
}

/// A file changed on two devices: this device's side and every other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileConflict {
    pub path: String,
    /// This device's file; `None` when it was deleted here.
    pub mine: Option<FileSide>,
    /// Each other device's (`None`: deleted there) and its name.
    pub others: Vec<(Option<FileSide>, String)>,
    /// Both are JSON, and merge into one.
    pub mergeable: bool,
}

/// What the person sees about one app's files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FilesStatus {
    pub waiting_for_approval: bool,
    pub last_sync: Option<SystemTime>,
    pub last_error: Option<String>,
    pub conflicts: usize,
    /// Files this device keeps to itself (too large, or too many).
    pub kept_here: Vec<String>,
    /// Files from other devices that could not be put in place, and why.
    pub refused: Vec<String>,
}

/// The largest file "merge" reads: a JSON file larger than this is offered
/// the other three choices.
const MERGE_MAX: u64 = 4 * 1024 * 1024;

fn name_or(names: &BTreeMap<String, String>, device: &str) -> String {
    names
        .get(device)
        .filter(|n| !n.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "other device".to_string())
}

fn conflicts_of(
    state: &FilesState,
    folder: &Path,
    held: &Path,
    names: &BTreeMap<String, String>,
) -> Vec<FileConflict> {
    state
        .replica
        .conflicts()
        .into_iter()
        .map(|c| {
            let mine = if c.mine_deleted {
                None
            } else {
                stat(folder, &c.key).map(|(size, at)| FileSide {
                    size,
                    at: Some(at / 1_000_000_000),
                })
            };
            let side = |v: &Version| {
                let value = v.value.as_deref().and_then(FileValue::decode)?;
                let at = state
                    .bodies
                    .get(&c.key)
                    .and_then(|b| b.get(&value.hash))
                    .and_then(|b| b.at);
                Some(FileSide {
                    size: value.size,
                    at,
                })
            };
            let others = c
                .others
                .iter()
                .map(|v| (side(v), name_or(names, &v.by)))
                .collect();
            FileConflict {
                mergeable: merged(state, folder, held, &c.key).is_some(),
                path: c.key,
                mine,
                others,
            }
        })
        .collect()
}

/// This device's file merged with every other device's copy, when all are
/// JSON objects (or all lists) and small enough to read.
fn merged(state: &FilesState, folder: &Path, held: &Path, path: &str) -> Option<Vec<u8>> {
    let key = state.replica.keys.get(path)?;
    key.hash.as_ref()?;
    let (size, _) = stat(folder, path)?;
    if size > MERGE_MAX || key.others.is_empty() {
        return None;
    }
    let mut value = std::fs::read(place(folder, path, false).ok()?).ok()?;
    for other in &key.others {
        let theirs = FileValue::decode(&other.value()?)?;
        if theirs.size > MERGE_MAX {
            return None;
        }
        let body = held_body(held, &theirs.hash)?;
        value = krate_accountsync::json_merge::merge_bytes(&value, &body)?;
    }
    Some(value)
}

/// Answer a conflict on `path`: put the chosen file (or files) in place and
/// settle the replica. It goes up with the next exchange.
fn resolve_in(
    state: &mut FilesState,
    folder: &Path,
    held: &Path,
    names: &BTreeMap<String, String>,
    path: &str,
    choice: &Resolution,
) -> Result<(), ResolveError> {
    let key = state
        .replica
        .keys
        .get(path)
        .ok_or(ResolveError::NoConflict)?;
    if key.others.is_empty() {
        return Err(ResolveError::NoConflict);
    }
    let mine_bytes = match key.hash {
        Some(_) => place(folder, path, false)
            .ok()
            .and_then(|p| std::fs::read(p).ok()),
        None => None,
    };
    let mine = mine_bytes.as_deref().map(FileValue::of);
    if let Resolution::Merge = choice {
        let bytes = merged(state, folder, held, path).ok_or(ResolveError::NotMergeable)?;
        let seen = put_in_place(folder, path, &bytes).map_err(|_| ResolveError::NotHereYet)?;
        state.index.insert(path.to_string(), seen);
        state
            .replica
            .settle(path, Some(&FileValue::of(&bytes).encode()))?;
        return Ok(());
    }
    // Every copy the choice puts in place must be on this device first:
    // nothing is settled that cannot be written.
    let needed: Vec<FileValue> = match choice {
        Resolution::KeepOther(i) => key
            .others
            .get(*i)
            .ok_or(ResolveError::NoSuchVersion)?
            .value()
            .and_then(|v| FileValue::decode(&v))
            .into_iter()
            .collect(),
        Resolution::KeepBoth => key
            .others
            .iter()
            .filter_map(|o| o.value().and_then(|v| FileValue::decode(&v)))
            .collect(),
        _ => Vec::new(),
    };
    if needed
        .iter()
        .any(|v| Some(v) != mine.as_ref() && held_body(held, &v.hash).is_none())
    {
        return Err(ResolveError::NotHereYet);
    }
    let known: BTreeSet<String> = state.replica.keys.keys().cloned().collect();
    let taken = |p: &str| known.contains(p) || stat(folder, p).is_some();
    let mine_value = mine.as_ref().map(FileValue::encode);
    let writes = state.replica.resolve_named(
        path,
        choice,
        mine_value.as_deref(),
        &|device| name_or(names, device),
        &taken,
        &files::both_file_name,
    )?;
    for (p, value) in writes {
        match value.as_deref().and_then(FileValue::decode) {
            Some(value) => {
                let bytes = if Some(&value) == mine.as_ref() {
                    mine_bytes.clone()
                } else {
                    held_body(held, &value.hash)
                };
                let Some(bytes) = bytes else { continue };
                if let Ok(seen) = put_in_place(folder, &p, &bytes) {
                    state.index.insert(p, seen);
                }
            }
            None => {
                let _ = take_away(folder, &p);
                state.index.remove(&p);
            }
        }
    }
    Ok(())
}

struct Loaded {
    path: PathBuf,
    held: PathBuf,
    state: FilesState,
    names: BTreeMap<String, String>,
}

struct Core {
    loaded: Option<Loaded>,
    status: FilesStatus,
}

fn lock<T>(m: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
    m.lock()
        .map_err(|_| "the files' sync lock was poisoned".to_string())
}

/// One app's folder, kept in sync across the person's devices.
pub struct SyncedFiles {
    core: Arc<Mutex<Core>>,
    session: Arc<Mutex<Session>>,
    context: AccountSyncContext,
    folder: PathBuf,
    store: PathBuf,
    wake: Arc<Wake>,
    done: Option<std::sync::mpsc::Receiver<()>>,
}

impl SyncedFiles {
    /// The app's `folder`, with sync's own files beside its store at
    /// `store`. With `context.background`, exchanges start at once on a
    /// thread of their own.
    pub fn open(folder: PathBuf, store: PathBuf, context: AccountSyncContext) -> SyncedFiles {
        let mut synced = SyncedFiles {
            core: Arc::new(Mutex::new(Core {
                loaded: None,
                status: FilesStatus::default(),
            })),
            session: Arc::new(Mutex::new(Session::new(&context))),
            context,
            folder,
            store,
            wake: Wake::new(),
            done: None,
        };
        if synced.context.background {
            let run = synced.parts();
            let close = synced.parts();
            synced.done = spawn_loop(
                "krate-files-sync",
                synced.wake.clone(),
                move || run.exchange(false),
                move || {
                    let _ = close.exchange(true);
                },
            );
        }
        synced
    }

    fn parts(&self) -> Parts {
        Parts {
            core: self.core.clone(),
            session: self.session.clone(),
            context: self.context.clone(),
            folder: self.folder.clone(),
            store: self.store.clone(),
        }
    }

    /// One exchange now, on this thread: what a test, or "Sync now", does.
    pub fn sync_now(&self) -> Result<Step, String> {
        self.parts().exchange(false)
    }

    pub fn status(&self) -> FilesStatus {
        self.core
            .lock()
            .map(|c| {
                let mut status = c.status.clone();
                if let Some(loaded) = c.loaded.as_ref() {
                    status.conflicts = loaded.state.replica.conflicts().len();
                    status.kept_here = loaded.state.kept_here.clone();
                }
                status
            })
            .unwrap_or_default()
    }

    /// Every file in conflict, both sides in words' worth of facts.
    pub fn conflicts(&self) -> Vec<FileConflict> {
        let Ok(core) = self.core.lock() else {
            return Vec::new();
        };
        let Some(loaded) = core.loaded.as_ref() else {
            return Vec::new();
        };
        conflicts_of(&loaded.state, &self.folder, &loaded.held, &loaded.names)
    }

    /// Answer a conflict; it goes up with the next exchange.
    pub fn resolve(&self, path: &str, choice: &Resolution) -> Result<(), ResolveError> {
        {
            let mut core = self.core.lock().map_err(|_| ResolveError::NoConflict)?;
            let loaded = core.loaded.as_mut().ok_or(ResolveError::NoConflict)?;
            resolve_in(
                &mut loaded.state,
                &self.folder,
                &loaded.held.clone(),
                &loaded.names.clone(),
                path,
                choice,
            )?;
            let _ = write_state(&loaded.path, &loaded.state);
        }
        self.wake.kick();
        Ok(())
    }
}

impl Drop for SyncedFiles {
    /// The app closes: one last look, sent if anything changed, waited for
    /// at most [`CLOSE_WAIT`]; whatever it did not send goes next time.
    fn drop(&mut self) {
        self.wake.stop();
        if let Some(done) = self.done.take() {
            let _ = done.recv_timeout(CLOSE_WAIT);
        }
        if let Ok(core) = self.core.lock() {
            if let Some(loaded) = core.loaded.as_ref() {
                let _ = write_state(&loaded.path, &loaded.state);
            }
        }
    }
}

/// What one exchange needs, shareable with the sync thread.
struct Parts {
    core: Arc<Mutex<Core>>,
    session: Arc<Mutex<Session>>,
    context: AccountSyncContext,
    folder: PathBuf,
    store: PathBuf,
}

/// One change to send: the replica's write, and the file's value and body
/// as far as they are known before the network.
struct ToSend {
    out: Outgoing,
    value: Option<FileValue>,
    body: Option<Body>,
    at: Option<u64>,
}

impl Parts {
    fn exchange(&self, only_if_changed: bool) -> Result<Step, String> {
        let outcome = self.exchange_inner(only_if_changed);
        if let Ok(mut core) = self.core.lock() {
            match &outcome {
                Ok(Step::Waiting) => {}
                Ok(_) => {
                    core.status.last_sync = Some(SystemTime::now());
                    core.status.last_error = None;
                }
                Err(error) => core.status.last_error = Some(error.clone()),
            }
        }
        outcome
    }

    fn exchange_inner(&self, only_if_changed: bool) -> Result<Step, String> {
        let mut session = lock(&self.session)?;
        let session = &mut *session;
        if !session.hold(&self.context)? {
            lock(&self.core)?.status.waiting_for_approval = true;
            return Ok(Step::Waiting);
        }
        let account = session.account.as_ref().expect("held");
        let ring = session.ring.as_ref().expect("held");
        let key = FileKey::derive(ring, &self.context.publisher, &self.context.app_id);
        let me = account.device.id();

        // 1. Look at the folder: what changed here since is this device's
        // to send.
        let (since, sends, held, kept_here) = {
            let mut core = lock(&self.core)?;
            core.status.waiting_for_approval = false;
            if core.loaded.is_none() {
                let path = files_state_path(&self.store, &account.id);
                let mut state = read_state(&path).unwrap_or_else(|| FilesState::new(&me));
                if state.replica.device != me {
                    state.replica.device = me.clone();
                    state.replica.rev = None;
                }
                core.loaded = Some(Loaded {
                    path,
                    held: held_dir(&self.store, &account.id),
                    state,
                    names: krate_accountsync::device::device_names(&session.view),
                });
            }
            let loaded = core.loaded.as_mut().expect("loaded above");
            let st = &mut loaded.state;
            let scanned = scan(&self.folder, &st.index);
            // A file kept here is never sent as deleted, nor replaced.
            for path in &scanned.kept_here {
                st.replica.keys.remove(path);
            }
            st.kept_here = scanned.kept_here.clone();
            st.index = scanned
                .files
                .iter()
                .map(|(p, (_, seen))| (p.clone(), seen.clone()))
                .collect();
            let current: BTreeMap<String, Vec<u8>> = scanned
                .files
                .iter()
                .map(|(p, (v, _))| (p.clone(), v.encode()))
                .collect();
            st.replica.reconcile(&current);
            let mut sends = Vec::new();
            for out in st.replica.outgoing() {
                let Some(_) = out.hash else {
                    sends.push(ToSend {
                        out,
                        value: None,
                        body: None,
                        at: Some(now_secs()),
                    });
                    continue;
                };
                let Some((value, seen)) = scanned.files.get(&out.key) else {
                    continue;
                };
                if out.hash.as_deref() != Some(&krate_accountsync::value_hash(&value.encode())) {
                    continue;
                }
                let known = st
                    .bodies
                    .get(&out.key)
                    .and_then(|b| b.get(&value.hash))
                    .map(KnownBody::body);
                sends.push(ToSend {
                    at: known
                        .as_ref()
                        .and_then(|b| b.at)
                        .or(Some(seen.modified / 1_000_000_000)),
                    out,
                    value: Some(value.clone()),
                    body: known,
                });
            }
            (
                st.replica.rev,
                sends,
                loaded.held.clone(),
                scanned.kept_here.into_iter().collect::<BTreeSet<_>>(),
            )
        };
        if only_if_changed && sends.is_empty() {
            return Ok(Step::Done);
        }

        // 2. The network, with no lock held: each new body, then the
        // entries in one request.
        let mut writes = Vec::new();
        let mut sent = Vec::new();
        let mut uploaded = Vec::new();
        for send in sends {
            let body = match (&send.value, send.body) {
                (None, _) => None,
                (Some(_), Some(body)) => Some(body),
                (Some(value), None) => {
                    let Ok(target) = place(&self.folder, &send.out.key, false) else {
                        continue;
                    };
                    let Ok(bytes) = std::fs::read(target) else {
                        continue;
                    };
                    // Changed since the look: it goes with the next one.
                    if FileValue::of(&bytes) != *value {
                        continue;
                    }
                    let (raw, mut body) = key
                        .seal_body(&send.out.key, &bytes)
                        .map_err(|e| e.to_string())?;
                    drop(bytes);
                    body.at = send.at;
                    session
                        .hub
                        .put_blob(key.bucket(), &body.blob, &raw)
                        .map_err(|e| e.to_string())?;
                    uploaded.push((send.out.key.clone(), value.hash.clone(), body.clone()));
                    Some(body)
                }
            };
            let version = Version {
                value: send.value.as_ref().map(FileValue::encode),
                vv: send.out.vv.clone(),
                by: send.out.by.clone(),
            };
            let sealed = key
                .seal(&send.out.key, &version, body.as_ref(), send.at)
                .map_err(|e| e.to_string())?;
            let mut wire = sealed.to_json();
            wire["name"] = serde_json::Value::String(key.name_of(&send.out.key));
            if let Some(body) = &body {
                wire["blob"] = serde_json::Value::String(body.blob.clone());
            }
            writes.push(wire);
            sent.push(send.out);
        }
        let answer = if writes.is_empty() {
            session.hub.bucket(key.bucket(), since)
        } else {
            session.hub.put(key.bucket(), &writes)
        };
        let answer = match answer {
            Ok(answer) => answer,
            // A body the hub let go of meanwhile: forget it, and send the
            // bytes again next time.
            Err(HubError::Refused { reason, .. }) if reason == "blob-missing" => {
                let mut core = lock(&self.core)?;
                if let Some(loaded) = core.loaded.as_mut() {
                    for out in &sent {
                        loaded.state.bodies.remove(&out.key);
                    }
                }
                return Ok(Step::More);
            }
            Err(e) => return Err(e.to_string()),
        };

        // 3. What the hub holds, opened.
        let unchanged = answer["unchanged"].as_bool() == Some(true);
        let mut by_path: BTreeMap<String, Vec<(Version, Option<Body>)>> = BTreeMap::new();
        for (name, versions) in answer["entries"].as_object().into_iter().flatten() {
            for wire in versions.as_array().into_iter().flatten() {
                let Some(sealed) = Sealed::from_json(wire) else {
                    continue;
                };
                // What does not open is not this device's to read.
                let Ok((path, version, body, _)) = key.open(name, &sealed) else {
                    continue;
                };
                by_path.entry(path).or_default().push((version, body));
            }
        }

        // 4. The other devices' contents this device will need -- to put in
        // place, or to hold for a choice -- fetched with no lock held.
        let mut skipped: BTreeSet<String> = BTreeSet::new();
        if !unchanged {
            let known: BTreeMap<String, (VersionVector, Option<String>)> = {
                let core = lock(&self.core)?;
                let st = &core.loaded.as_ref().expect("loaded").state;
                by_path
                    .keys()
                    .filter_map(|p| {
                        let state = st.replica.keys.get(p)?;
                        Some((
                            p.clone(),
                            (state.vv.clone(), st.index.get(p).map(|s| s.hash.clone())),
                        ))
                    })
                    .collect()
            };
            for (path, versions) in &by_path {
                if kept_here.contains(path) {
                    continue;
                }
                for (version, body) in versions {
                    let (Some(value), Some(body)) =
                        (version.value.as_deref().and_then(FileValue::decode), body)
                    else {
                        continue;
                    };
                    if let Some((vv, here)) = known.get(path) {
                        if vv.covers(&version.vv) || here.as_deref() == Some(&value.hash) {
                            continue;
                        }
                    }
                    if held_body(&held, &value.hash).is_some() {
                        continue;
                    }
                    let fetched = session
                        .hub
                        .blob(key.bucket(), &body.blob, FILE_MAX + 64)
                        .ok()
                        .and_then(|raw| key.open_body(path, body, &raw, &value).ok());
                    match fetched {
                        Some(bytes) if hold_body(&held, &value.hash, &bytes).is_ok() => {}
                        _ => {
                            skipped.insert(path.clone());
                        }
                    }
                }
            }
        }

        // 5. Fold it in.
        let mut core = lock(&self.core)?;
        let core = &mut *core;
        let loaded = core.loaded.as_mut().expect("loaded");
        let st = &mut loaded.state;
        st.replica.mark_sent(&sent);
        for (path, hash, body) in uploaded {
            st.bodies.entry(path).or_default().insert(
                hash,
                KnownBody {
                    blob: body.blob,
                    k: body.k,
                    at: body.at,
                },
            );
        }
        st.replica.app = Some((self.context.app_id.clone(), self.context.publisher.clone()));
        st.folder = Some(self.folder.clone());
        st.replica.synced_at = Some(now_secs());
        if unchanged {
            write_state(&loaded.path, st)?;
            return Ok(if st.replica.outgoing().is_empty() {
                Step::Done
            } else {
                Step::More
            });
        }
        let mut bodies: BTreeMap<String, BTreeMap<String, KnownBody>> = BTreeMap::new();
        let mut held_vv: BTreeMap<String, Vec<VersionVector>> = BTreeMap::new();
        for (path, versions) in &by_path {
            for (version, body) in versions {
                held_vv
                    .entry(path.clone())
                    .or_default()
                    .push(version.vv.clone());
                let (Some(value), Some(body)) =
                    (version.value.as_deref().and_then(FileValue::decode), body)
                else {
                    continue;
                };
                bodies.entry(path.clone()).or_default().insert(
                    value.hash,
                    KnownBody {
                        blob: body.blob.clone(),
                        k: body.k,
                        at: body.at,
                    },
                );
            }
        }
        st.bodies = bodies;
        let mut refused = Vec::new();
        for (path, versions) in &by_path {
            if kept_here.contains(path) || skipped.contains(path) {
                continue;
            }
            // The app changed it since the look: its change goes up next,
            // and meets this one there.
            let seen = st.index.get(path).map(|s| (s.size, s.modified));
            if stat(&self.folder, path) != seen {
                skipped.insert(path.clone());
                continue;
            }
            let was = st.replica.keys.get(path).cloned();
            let versions: Vec<Version> = versions.iter().map(|(v, _)| v.clone()).collect();
            let Some(target) = st.replica.merge_remote(path, &versions).store else {
                continue;
            };
            let done = match target.as_deref().map(FileValue::decode) {
                None => take_away(&self.folder, path).map(|()| {
                    st.index.remove(path);
                }),
                Some(None) => Err(format!("{path}: not a file's version")),
                Some(Some(value)) => {
                    if st.index.get(path).map(|s| &s.hash) == Some(&value.hash) {
                        Ok(())
                    } else {
                        match held_body(&held, &value.hash) {
                            Some(bytes) => put_in_place(&self.folder, path, &bytes).map(|seen| {
                                st.index.insert(path.clone(), seen);
                            }),
                            None => Err(format!("{path}: its contents did not arrive")),
                        }
                    }
                }
            };
            if let Err(why) = done {
                // Not taken: the replica goes back to what the folder still
                // holds, so this is never sent on as a change of this
                // device's.
                match was {
                    Some(was) => {
                        st.replica.keys.insert(path.clone(), was);
                    }
                    None => {
                        st.replica.keys.remove(path);
                    }
                }
                refused.push(why);
            }
        }
        core.status.refused = refused;
        st.replica.heal(&held_vv);
        st.replica.rev = if skipped.is_empty() && core.status.refused.is_empty() {
            answer["rev"].as_u64()
        } else {
            None
        };
        if by_path
            .values()
            .flatten()
            .any(|(v, _)| v.by != st.replica.device && !loaded.names.contains_key(&v.by))
        {
            if let Ok(view) = session.hub.view(Some(&me)) {
                loaded.names = krate_accountsync::device::device_names(&view);
            }
        }
        prune_held(&held, &st.replica);
        write_state(&loaded.path, st)?;
        // A copy that did not arrive is read again at the next exchange
        // (`rev` is cleared), not hurried: the hub is not asked every
        // two seconds for something it may not have.
        Ok(if st.replica.outgoing().is_empty() {
            Step::Done
        } else {
            Step::More
        })
    }
}

/// The files in conflict for an app that is not running, read from disk:
/// for Krate's home and `krate account sync conflicts`. `store` is the
/// app's store (sync keeps its files beside it); `names` the account's
/// device names.
pub fn file_conflicts_at(
    store: &Path,
    account: &str,
    names: &BTreeMap<String, String>,
) -> Vec<FileConflict> {
    let Some(state) = read_state(&files_state_path(store, account)) else {
        return Vec::new();
    };
    let Some(folder) = state.folder.clone() else {
        return Vec::new();
    };
    conflicts_of(&state, &folder, &held_dir(store, account), names)
}

/// The files this device keeps to itself for an app (too large, or too
/// many), as its last sync found them.
pub fn files_kept_here_at(store: &Path, account: &str) -> Vec<String> {
    read_state(&files_state_path(store, account))
        .map(|s| s.kept_here)
        .unwrap_or_default()
}

/// When this device last synced an app's files, in seconds since 1970.
pub fn files_last_sync_at(store: &Path, account: &str) -> Option<u64> {
    read_state(&files_state_path(store, account))?
        .replica
        .synced_at
}

/// Answer a file conflict while the app is not running. It goes up the
/// next time the app syncs, or at once with [`sync_files_closed`].
pub fn resolve_file_at(
    store: &Path,
    account: &str,
    names: &BTreeMap<String, String>,
    path: &str,
    choice: &Resolution,
) -> Result<(), ResolveError> {
    let state_path = files_state_path(store, account);
    let mut state = read_state(&state_path).ok_or(ResolveError::NoConflict)?;
    let folder = state.folder.clone().ok_or(ResolveError::NoConflict)?;
    resolve_in(
        &mut state,
        &folder,
        &held_dir(store, account),
        names,
        path,
        choice,
    )?;
    write_state(&state_path, &state).map_err(|_| ResolveError::NoConflict)
}

/// Send an answer given with the app closed now: one exchange of its files,
/// the way its run makes them. `context` is this device's; the app's id,
/// publisher and folder are the ones its last run synced with.
pub fn sync_files_closed(
    store: &Path,
    account: &str,
    mut context: AccountSyncContext,
) -> Result<Step, String> {
    let state = read_state(&files_state_path(store, account))
        .ok_or_else(|| "this app's files have not synced on this device yet".to_string())?;
    let (app_id, publisher) = state
        .replica
        .app
        .ok_or_else(|| "this app's files have not synced on this device yet".to_string())?;
    let folder = state
        .folder
        .ok_or_else(|| "this app's files have not synced on this device yet".to_string())?;
    if !sync_wanted(store, &context.device_dir) {
        return Err("sync is off for this app on this device".to_string());
    }
    context.app_id = app_id;
    context.publisher = publisher;
    context.enabled = true;
    context.background = false;
    context.files = Some(folder.clone());
    SyncedFiles::open(folder, store.to_path_buf(), context).sync_now()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(dir: &Path) -> AccountSyncContext {
        AccountSyncContext {
            // A closed port: the hub is away, as on a plane.
            hub: "http://127.0.0.1:9".into(),
            token: "gho_nobody".into(),
            device_dir: dir.join("devices"),
            device_name: "Test Mac".into(),
            machine_key: vec![3; 32],
            app_id: "dev.krate.notes".into(),
            publisher: "unsigned".into(),
            enabled: true,
            background: true,
            files: Some(dir.join("folder")),
        }
    }

    /// Offline is normal: the app's files are its files, and closing does
    /// not wait on the hub.
    #[test]
    fn with_the_hub_away_the_folder_is_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let started = std::time::Instant::now();
        {
            let files = SyncedFiles::open(
                folder.clone(),
                dir.path().join("notes.kv"),
                context(dir.path()),
            );
            std::fs::write(folder.join("a.txt"), b"hello").unwrap();
            assert!(files.conflicts().is_empty());
        }
        let took = started.elapsed();
        assert!(
            took < std::time::Duration::from_secs(8),
            "closing waited {took:?}"
        );
        assert_eq!(std::fs::read(folder.join("a.txt")).unwrap(), b"hello");
    }

    #[test]
    fn a_look_reads_only_what_changed_and_never_follows_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        std::fs::create_dir_all(folder.join("notes")).unwrap();
        std::fs::write(folder.join("notes/a.txt"), b"one").unwrap();
        std::fs::write(folder.join(".krate-sync-1-2.tmp"), b"half").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", folder.join("outside")).unwrap();
        let first = scan(folder, &BTreeMap::new());
        assert_eq!(
            first.files.keys().cloned().collect::<Vec<_>>(),
            vec!["notes/a.txt".to_string()],
            "no temporary file, no link"
        );
        let (value, seen) = &first.files["notes/a.txt"];
        assert_eq!(*value, FileValue::of(b"one"));
        // Read just now: a second look reads it again (a write in the same
        // tick would share its time); one well after does not.
        let mut index: BTreeMap<String, Seen> = BTreeMap::new();
        let mut old = seen.clone();
        old.checked = old.modified + 5_000_000_000;
        old.hash = "f".repeat(64);
        index.insert("notes/a.txt".into(), old);
        let again = scan(folder, &index);
        assert_eq!(
            again.files["notes/a.txt"].0.hash,
            "f".repeat(64),
            "not read again"
        );
        index.get_mut("notes/a.txt").unwrap().checked = seen.modified;
        let again = scan(folder, &index);
        assert_eq!(
            again.files["notes/a.txt"].0,
            FileValue::of(b"one"),
            "read again"
        );
    }

    #[test]
    fn sync_writes_only_inside_the_folder_and_never_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        put_in_place(&folder, "deep/er/a.txt", b"x").unwrap();
        assert_eq!(std::fs::read(folder.join("deep/er/a.txt")).unwrap(), b"x");
        for bad in ["../escape.txt", "/etc/x", "a//b", "a\\b"] {
            assert!(put_in_place(&folder, bad, b"x").is_err(), "{bad}");
        }
        #[cfg(unix)]
        {
            let outside = dir.path().join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, folder.join("link")).unwrap();
            assert!(put_in_place(&folder, "link/a.txt", b"x").is_err());
            assert!(take_away(&folder, "link/a.txt").is_err());
            assert!(!outside.join("a.txt").exists());
        }
        take_away(&folder, "deep/er/a.txt").unwrap();
        take_away(&folder, "deep/er/a.txt").unwrap();
        assert!(!folder.join("deep/er/a.txt").exists());
    }
}
