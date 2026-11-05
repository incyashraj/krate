//! An app's `store.kv`, kept the same on every device a person signs in on
//! (IC-981, F-321), with no code in the app.
//!
//! When the run's [`crate::Config`] carries an [`AccountSyncContext`] and
//! the app was granted `store.kv`, the store is wrapped: every get, set,
//! delete, list and clear goes to the very same store file as before, and
//! beside it a replica (crates/accountsync) remembers each key's version
//! vector, what still has to go up, and the conflicts nobody has answered.
//! With no context -- nobody signed in, the toggle off, a browser tab --
//! the store is exactly what it always was.
//!
//! Local first. The app never waits for the network: a thread of the
//! store's own exchanges with the hub at open, every [`PERIOD`] while the
//! app runs and once more at close, and backs off when the hub is away.
//! The machinery is `store.shared`'s (shared_host.rs): the lock held only
//! to read what to send and to fold in the answer, never across the
//! network; a write made while an exchange is away stays to go next time.
//!
//! Beside the store: `<store>.sync/<account slot>.json`, the replica. If it
//! is lost or the store is changed while sync is off, the next exchange
//! compares the store with what the replica last knew and sends the
//! difference as this device's writes -- nothing written here is ever
//! silently overwritten.

use std::path::PathBuf;

use crate::store_host::{AppStore, StoreError};

/// What a run needs to keep its app's `store.kv` on every device of the
/// signed-in person. Decided by the player before the run: who is signed
/// in, where this machine keeps its account-sync files, which app this is
/// (its id and verified publisher, crates/accountsync `publisher_label`)
/// and whether the person turned sync off for it.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountSyncContext {
    /// The hub, e.g. `https://hub.krate.tech`.
    pub hub: String,
    /// The person's sign-in, sent only to `hub`. The hub decides whose
    /// account it is from this alone.
    pub token: String,
    /// This machine's account-sync folder (device keys, the held key).
    pub device_dir: PathBuf,
    /// What the person's device list calls this device.
    pub device_name: String,
    /// The key Krate keeps on this machine for store.secret: the device's
    /// private key is sealed with it at rest.
    pub machine_key: Vec<u8>,
    pub app_id: String,
    pub publisher: String,
    /// The toggle: `false` keeps this device's copy and stops exchanging.
    pub enabled: bool,
    /// A person's run exchanges on its own thread. A test turns this off
    /// and drives [`SyncedKv::sync_now`] itself.
    pub background: bool,
}

impl std::fmt::Debug for AccountSyncContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountSyncContext")
            .field("hub", &self.hub)
            .field("token", &"<sign-in>")
            .field("device_dir", &self.device_dir)
            .field("device_name", &self.device_name)
            .field("app_id", &self.app_id)
            .field("publisher", &self.publisher)
            .field("enabled", &self.enabled)
            .field("background", &self.background)
            .finish()
    }
}

// The toggles (`<store>.sync/off`, `<device dir>/off`) and where the
// replica lives, as every player reads them.
#[cfg(not(target_arch = "wasm32"))]
pub use krate_accountsync::publisher_label;
#[cfg(not(target_arch = "wasm32"))]
pub use krate_accountsync::toggles::{
    app_toggle_path, forget_replicas, global_toggle_path, replica_path, set_all_sync, set_app_sync,
    sync_dir, sync_wanted,
};

/// The app's key-value store for one run: as it always was, or kept in sync
/// across the person's devices.
pub enum KvStore {
    Plain(AppStore),
    #[cfg(not(target_arch = "wasm32"))]
    Synced(SyncedKv),
}

impl KvStore {
    /// The store at `path`; synced when `sync` says so and the app was
    /// granted `store.kv`.
    pub fn open(path: PathBuf, granted: bool, sync: Option<&AccountSyncContext>) -> KvStore {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(context) = sync.filter(|c| c.enabled && granted) {
            return KvStore::Synced(SyncedKv::open(path, context.clone()));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = sync;
        KvStore::Plain(AppStore::open(path, granted))
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
        match self {
            KvStore::Plain(store) => store.get(key),
            #[cfg(not(target_arch = "wasm32"))]
            KvStore::Synced(store) => store.get(key),
        }
    }

    pub fn set(&mut self, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
        match self {
            KvStore::Plain(store) => store.set(key, value),
            #[cfg(not(target_arch = "wasm32"))]
            KvStore::Synced(store) => store.set(key, value),
        }
    }

    pub fn delete(&mut self, key: &str) -> Result<(), StoreError> {
        match self {
            KvStore::Plain(store) => store.delete(key),
            #[cfg(not(target_arch = "wasm32"))]
            KvStore::Synced(store) => store.delete(key),
        }
    }

    pub fn keys(&self) -> Result<Vec<String>, StoreError> {
        match self {
            KvStore::Plain(store) => store.keys(),
            #[cfg(not(target_arch = "wasm32"))]
            KvStore::Synced(store) => store.keys(),
        }
    }

    pub fn clear(&mut self) -> Result<(), StoreError> {
        match self {
            KvStore::Plain(store) => store.clear(),
            #[cfg(not(target_arch = "wasm32"))]
            KvStore::Synced(store) => store.clear(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use synced::*;

#[cfg(not(target_arch = "wasm32"))]
mod synced {
    use super::*;

    use std::collections::BTreeMap;
    use std::path::Path;
    use std::sync::{Arc, Condvar, Mutex, MutexGuard};
    use std::time::{Duration, Instant, SystemTime};

    use krate_accountsync::device::{Account, KeyStatus};
    use krate_accountsync::hub::Hub;
    use krate_accountsync::seal::{AppKey, Sealed};
    use krate_accountsync::{Replica, Resolution, ResolveError, Version, VersionVector};
    use krate_orgkeys::Keyring;

    /// How often a running app's data is exchanged with the hub (F-321:
    /// "every ~30 s while open"). Each exchange that sends something is one
    /// hub write, so this is also what bounds an app's share of the hub's
    /// daily writes.
    pub const PERIOD: Duration = Duration::from_secs(30);
    /// The soonest a follow-up exchange runs, when one exchange left
    /// something to send (two devices' same value joined, a lost write).
    pub const FLOOR: Duration = Duration::from_secs(2);
    /// The longest an app's close waits for its last exchange.
    pub const CLOSE_WAIT: Duration = Duration::from_secs(3);

    /// What the person sees about one app's sync.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct SyncStatus {
        /// Registered, waiting for one of their devices to approve this one
        /// (or for the recovery code).
        pub waiting_for_approval: bool,
        /// The last exchange that reached the hub.
        pub last_sync: Option<SystemTime>,
        /// Why the last exchange did not, in words.
        pub last_error: Option<String>,
        pub conflicts: usize,
        /// Values from other devices this store's own rules refused (too
        /// large, a key it does not allow), by key.
        pub refused: Vec<String>,
    }

    /// A conflict as Krate shows it: this device's side and every other.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct KvConflict {
        pub key: String,
        /// This device's value; `None` when it was deleted here.
        pub mine: Option<Vec<u8>>,
        /// Each other device's value (`None`: deleted there) and its name.
        pub others: Vec<(Option<Vec<u8>>, String)>,
        /// Whether "merge" is offered.
        pub mergeable: bool,
    }

    /// What one exchange did.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Step {
        /// In step with the hub.
        Done,
        /// Something is left to send: exchange again soon.
        More,
        /// This device is waiting for the key.
        Waiting,
    }

    struct Loaded {
        path: PathBuf,
        replica: Replica,
        app: AppKey,
        names: BTreeMap<String, String>,
    }

    impl Loaded {
        fn save(&self) -> Result<(), String> {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let text = serde_json::to_string(&self.replica).map_err(|e| e.to_string())?;
            let temp = self
                .path
                .with_extension(format!("{}.tmp", std::process::id()));
            std::fs::write(&temp, text).map_err(|e| e.to_string())?;
            std::fs::rename(&temp, &self.path).map_err(|e| e.to_string())
        }
    }

    struct Core {
        store: AppStore,
        store_path: PathBuf,
        loaded: Option<Loaded>,
        status: SyncStatus,
    }

    /// The account and key this run holds, serialised across exchanges
    /// (the background thread's and [`SyncedKv::sync_now`]'s), and held
    /// across the network -- never the store's lock.
    struct Session {
        hub: Hub,
        account: Option<Account>,
        ring: Option<Keyring>,
        view: serde_json::Value,
        /// A version came from a device not in `view`'s list.
        names_stale: bool,
    }

    struct Wake {
        flags: Mutex<(bool, bool)>, // (stop, kick)
        cv: Condvar,
    }

    /// One app's store, kept in sync across the person's devices.
    pub struct SyncedKv {
        core: Arc<Mutex<Core>>,
        session: Arc<Mutex<Session>>,
        context: AccountSyncContext,
        wake: Arc<Wake>,
        done: Option<std::sync::mpsc::Receiver<()>>,
    }

    fn lock<T>(m: &Mutex<T>) -> Result<MutexGuard<'_, T>, StoreError> {
        m.lock()
            .map_err(|_| StoreError::Io("the store's sync lock was poisoned".into()))
    }

    impl SyncedKv {
        pub fn open(path: PathBuf, context: AccountSyncContext) -> SyncedKv {
            let store = AppStore::open(path.clone(), true);
            let core = Arc::new(Mutex::new(Core {
                store,
                store_path: path,
                loaded: None,
                status: SyncStatus::default(),
            }));
            let session = Arc::new(Mutex::new(Session {
                hub: Hub::new(&context.hub, &context.token),
                account: None,
                ring: None,
                view: serde_json::Value::Null,
                names_stale: false,
            }));
            let wake = Arc::new(Wake {
                flags: Mutex::new((false, false)),
                cv: Condvar::new(),
            });
            let mut kv = SyncedKv {
                core,
                session,
                context,
                wake,
                done: None,
            };
            if kv.context.background {
                kv.start();
            }
            kv
        }

        fn start(&mut self) {
            let (core, session, context, wake) = (
                self.core.clone(),
                self.session.clone(),
                self.context.clone(),
                self.wake.clone(),
            );
            let (tell, done) = std::sync::mpsc::channel();
            let started = std::thread::Builder::new()
                .name("krate-account-sync".into())
                .spawn(move || {
                    let mut wait = Duration::ZERO;
                    let mut failures = 0u32;
                    let mut last = None::<Instant>;
                    loop {
                        let stop = {
                            let flags = wake.flags.lock().unwrap_or_else(|p| p.into_inner());
                            let (mut flags, _) = wake
                                .cv
                                .wait_timeout_while(flags, wait, |(stop, kick)| !*stop && !*kick)
                                .unwrap_or_else(|p| p.into_inner());
                            flags.1 = false;
                            flags.0
                        };
                        if stop {
                            // At close: only what is still to go up matters.
                            let dirty = core.lock().ok().is_some_and(|c| {
                                c.loaded
                                    .as_ref()
                                    .is_some_and(|l| !l.replica.outgoing().is_empty())
                            });
                            if dirty {
                                let _ = exchange(&core, &session, &context);
                            }
                            break;
                        }
                        if last.is_some_and(|at: Instant| at.elapsed() < FLOOR) {
                            wait = FLOOR;
                            continue;
                        }
                        last = Some(Instant::now());
                        wait = match exchange(&core, &session, &context) {
                            Ok(Step::More) => FLOOR,
                            Ok(Step::Done) | Ok(Step::Waiting) => {
                                failures = 0;
                                PERIOD
                            }
                            Err(_) => {
                                failures = failures.saturating_add(1);
                                // 30 s, 60 s, 2 min, 4 min, then every 5.
                                PERIOD
                                    .saturating_mul(1 << failures.min(4).saturating_sub(1))
                                    .min(Duration::from_secs(300))
                            }
                        };
                    }
                    let _ = tell.send(());
                });
            if started.is_ok() {
                self.done = Some(done);
            }
        }

        /// One exchange now, on this thread: what a test, or a person
        /// pressing "Sync now", does.
        pub fn sync_now(&self) -> Result<Step, String> {
            exchange(&self.core, &self.session, &self.context)
        }

        pub fn status(&self) -> SyncStatus {
            self.core
                .lock()
                .map(|c| {
                    let mut status = c.status.clone();
                    status.conflicts = c
                        .loaded
                        .as_ref()
                        .map(|l| l.replica.conflicts().len())
                        .unwrap_or(0);
                    status
                })
                .unwrap_or_default()
        }

        pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StoreError> {
            lock(&self.core)?.store.get(key)
        }

        pub fn keys(&self) -> Result<Vec<String>, StoreError> {
            lock(&self.core)?.store.keys()
        }

        pub fn set(&mut self, key: &str, value: Vec<u8>) -> Result<(), StoreError> {
            let mut core = lock(&self.core)?;
            core.store.set(key, value.clone())?;
            if let Some(loaded) = core.loaded.as_mut() {
                loaded.replica.local_write(key, Some(&value));
            }
            Ok(())
        }

        pub fn delete(&mut self, key: &str) -> Result<(), StoreError> {
            let mut core = lock(&self.core)?;
            let had = core.store.get(key)?.is_some();
            core.store.delete(key)?;
            if had {
                if let Some(loaded) = core.loaded.as_mut() {
                    loaded.replica.local_write(key, None);
                }
            }
            Ok(())
        }

        pub fn clear(&mut self) -> Result<(), StoreError> {
            let mut core = lock(&self.core)?;
            let keys = core.store.keys()?;
            core.store.clear()?;
            if let Some(loaded) = core.loaded.as_mut() {
                for key in keys {
                    loaded.replica.local_write(&key, None);
                }
            }
            Ok(())
        }

        /// Every key in conflict, with both sides, for Krate to show.
        pub fn conflicts(&self) -> Vec<KvConflict> {
            let Ok(core) = self.core.lock() else {
                return Vec::new();
            };
            let Some(loaded) = core.loaded.as_ref() else {
                return Vec::new();
            };
            conflicts_of(&core.store, &loaded.replica, &loaded.names)
        }

        /// Answer a conflict. It goes up with the next exchange.
        pub fn resolve(&self, key: &str, choice: &Resolution) -> Result<(), ResolveError> {
            {
                let mut core = self.core.lock().map_err(|_| ResolveError::NoConflict)?;
                let core = &mut *core;
                let loaded = core.loaded.as_mut().ok_or(ResolveError::NoConflict)?;
                resolve_in(
                    &mut core.store,
                    &mut loaded.replica,
                    &loaded.names,
                    key,
                    choice,
                )?;
                let _ = loaded.save();
            }
            self.kick();
            Ok(())
        }

        fn kick(&self) {
            if let Ok(mut flags) = self.wake.flags.lock() {
                flags.1 = true;
                self.wake.cv.notify_all();
            }
        }
    }

    impl Drop for SyncedKv {
        /// The app closes: one last exchange of what is still to go up,
        /// waited for at most [`CLOSE_WAIT`]. Past that it is left on its
        /// own thread, and whatever it did not send goes next time -- the
        /// replica on disk still says so.
        fn drop(&mut self) {
            if let Ok(mut flags) = self.wake.flags.lock() {
                flags.0 = true;
                self.wake.cv.notify_all();
            }
            if let Some(done) = self.done.take() {
                let _ = done.recv_timeout(CLOSE_WAIT);
            }
            if let Ok(core) = self.core.lock() {
                if let Some(loaded) = core.loaded.as_ref() {
                    let _ = loaded.save();
                }
            }
        }
    }

    fn conflicts_of(
        store: &AppStore,
        replica: &Replica,
        names: &BTreeMap<String, String>,
    ) -> Vec<KvConflict> {
        replica
            .conflicts()
            .into_iter()
            .map(|c| {
                let mine = store.get(&c.key).ok().flatten();
                KvConflict {
                    mergeable: replica.mergeable(&c.key, mine.as_deref()),
                    key: c.key,
                    mine,
                    others: c
                        .others
                        .into_iter()
                        .map(|v| (v.value, name_or(names, &v.by)))
                        .collect(),
                }
            })
            .collect()
    }

    fn name_or(names: &BTreeMap<String, String>, device: &str) -> String {
        names
            .get(device)
            .filter(|n| !n.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| "other device".to_string())
    }

    fn resolve_in(
        store: &mut AppStore,
        replica: &mut Replica,
        names: &BTreeMap<String, String>,
        key: &str,
        choice: &Resolution,
    ) -> Result<(), ResolveError> {
        let mine = store.get(key).ok().flatten();
        let existing: std::collections::BTreeSet<String> =
            store.keys().unwrap_or_default().into_iter().collect();
        let writes = replica.resolve(
            key,
            choice,
            mine.as_deref(),
            &|device| name_or(names, device),
            &|k| existing.contains(k),
        )?;
        store
            .apply_synced(&writes)
            .map_err(|_| ResolveError::NoConflict)?;
        Ok(())
    }

    /// The conflicts in an app's store while it is not running: for
    /// Krate's home and `krate account sync conflicts`, which read the store
    /// and its replica directly. `names` are the account's device names
    /// (crates/accountsync `Overview::names`), for saying where the other
    /// side came from.
    pub fn conflicts_at(
        store: &Path,
        account: &str,
        names: &BTreeMap<String, String>,
    ) -> Vec<KvConflict> {
        let path = replica_path(store, account);
        let Some(replica) = read_replica(&path) else {
            return Vec::new();
        };
        conflicts_of(&AppStore::open(store.to_path_buf(), true), &replica, names)
    }

    /// When this device last synced an app's store with the hub, in seconds
    /// since 1970; `None` for never.
    pub fn last_sync_at(store: &Path, account: &str) -> Option<u64> {
        read_replica(&replica_path(store, account))?.synced_at
    }

    /// Answer a conflict while the app is not running. `names` are the
    /// account's device names (crates/accountsync `device_names`), for
    /// "keep both". The answer goes up the next time the app runs.
    pub fn resolve_at(
        store: &Path,
        account: &str,
        names: &BTreeMap<String, String>,
        key: &str,
        choice: &Resolution,
    ) -> Result<(), ResolveError> {
        let path = replica_path(store, account);
        let mut replica = read_replica(&path).ok_or(ResolveError::NoConflict)?;
        let mut app_store = AppStore::open(store.to_path_buf(), true);
        resolve_in(&mut app_store, &mut replica, names, key, choice)?;
        let text = serde_json::to_string(&replica).map_err(|_| ResolveError::NoConflict)?;
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&temp, text).map_err(|_| ResolveError::NoConflict)?;
        std::fs::rename(&temp, &path).map_err(|_| ResolveError::NoConflict)?;
        Ok(())
    }

    /// Send an answer given with the app closed now, rather than at the
    /// app's next run: one exchange of the store, the way its run makes
    /// them. `context` is this device's (the app's id and publisher are the
    /// replica's own, from the run that last synced it). Offline is no
    /// failure the person sees: the answer stays on disk and goes next time.
    pub fn sync_closed(
        store: &Path,
        account: &str,
        mut context: AccountSyncContext,
    ) -> Result<Step, String> {
        let replica = read_replica(&replica_path(store, account))
            .ok_or_else(|| "this app has not synced on this device yet".to_string())?;
        let (app_id, publisher) = replica.app.ok_or_else(|| {
            "this app has not synced since Krate learned to send answers at once".to_string()
        })?;
        if !sync_wanted(store, &context.device_dir) {
            return Err("sync is off for this app on this device".to_string());
        }
        context.app_id = app_id;
        context.publisher = publisher;
        context.enabled = true;
        context.background = false;
        let kv = SyncedKv::open(store.to_path_buf(), context);
        kv.sync_now()
    }

    fn read_replica(path: &Path) -> Option<Replica> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    fn set_error(core: &Arc<Mutex<Core>>, error: Option<String>) {
        if let Ok(mut core) = core.lock() {
            if error.is_none() {
                core.status.last_sync = Some(SystemTime::now());
            }
            core.status.last_error = error;
        }
    }

    /// One exchange: hold the key, send what is dirty, fold in the answer.
    fn exchange(
        core: &Arc<Mutex<Core>>,
        session: &Arc<Mutex<Session>>,
        context: &AccountSyncContext,
    ) -> Result<Step, String> {
        let outcome = exchange_inner(core, session, context);
        match &outcome {
            Ok(Step::Waiting) => {}
            Ok(_) => set_error(core, None),
            Err(error) => set_error(core, Some(error.clone())),
        }
        outcome
    }

    fn exchange_inner(
        core: &Arc<Mutex<Core>>,
        session: &Arc<Mutex<Session>>,
        context: &AccountSyncContext,
    ) -> Result<Step, String> {
        let mut session = session.lock().map_err(|_| "sync session poisoned")?;
        let session = &mut *session;

        // 1. The account and its key: once a run, then from memory.
        if session.account.is_none() {
            let id =
                krate_accountsync::device::account_of(&session.hub).map_err(|e| e.to_string())?;
            let account = Account::load(
                &context.device_dir,
                &id,
                &context.machine_key,
                &context.device_name,
            )
            .map_err(|e| e.to_string())?;
            session.account = Some(account);
        }
        let account = session.account.as_ref().expect("set above");
        if session.ring.is_none() {
            let (status, view) = account
                .refresh_with_view(&session.hub)
                .map_err(|e| e.to_string())?;
            session.view = view;
            match status {
                KeyStatus::Holds(ring) => session.ring = Some(ring),
                KeyStatus::Waiting => {
                    if let Ok(mut core) = core.lock() {
                        core.status.waiting_for_approval = true;
                    }
                    return Ok(Step::Waiting);
                }
            }
        }
        // A version from a device this run has not heard of: its name, for
        // "keep both", from the account's device list.
        if session.names_stale {
            if let Ok(view) = session.hub.view(Some(&account.device.id())) {
                session.names_stale = false;
                if let Ok(mut core) = core.lock() {
                    if let Some(loaded) = core.loaded.as_mut() {
                        loaded.names = krate_accountsync::device::device_names(&view);
                    }
                }
                session.view = view;
            }
        }
        let ring = session.ring.as_ref().expect("set above");

        // 2. The replica, the first time: what the store holds now that the
        // replica does not know is this device's to send.
        let (bucket, since, sent, writes) = {
            let mut core = core.lock().map_err(|_| "store poisoned")?;
            core.status.waiting_for_approval = false;
            let core = &mut *core;
            if core.loaded.is_none() {
                let path = replica_path(&core.store_path, &account.id);
                let mut replica =
                    read_replica(&path).unwrap_or_else(|| Replica::new(&account.device.id()));
                if replica.device != account.device.id() {
                    replica.device = account.device.id();
                    replica.rev = None;
                }
                replica.reconcile(&core.store.snapshot());
                core.loaded = Some(Loaded {
                    path,
                    replica,
                    app: AppKey::derive(ring, &context.publisher, &context.app_id),
                    names: krate_accountsync::device::device_names(&session.view),
                });
            }
            let loaded = core.loaded.as_mut().expect("loaded above");
            // A write the app made with the replica not yet loaded, or an
            // edit from outside the run, is caught here as well.
            loaded.replica.reconcile(&core.store.snapshot());
            let sent = loaded.replica.outgoing();
            let mut writes = Vec::with_capacity(sent.len());
            for out in &sent {
                let value = match &out.hash {
                    Some(_) => core.store.get(&out.key).map_err(|e| format!("{e:?}"))?,
                    None => None,
                };
                let sealed = loaded
                    .app
                    .seal(
                        &out.key,
                        &Version {
                            value,
                            vv: out.vv.clone(),
                            by: out.by.clone(),
                        },
                    )
                    .map_err(|e| e.to_string())?;
                let mut wire = sealed.to_json();
                wire["name"] = serde_json::Value::String(loaded.app.name_of(&out.key));
                writes.push(wire);
            }
            (
                loaded.app.bucket().to_string(),
                loaded.replica.rev,
                sent,
                writes,
            )
        };

        // 3. The network, with no lock on the store.
        let answer = if writes.is_empty() {
            session.hub.bucket(&bucket, since)
        } else {
            session.hub.put(&bucket, &writes)
        }
        .map_err(|e| e.to_string())?;

        // 4. Fold in the answer.
        let mut core = core.lock().map_err(|_| "store poisoned")?;
        let core = &mut *core;
        let loaded = core.loaded.as_mut().expect("loaded above");
        loaded.replica.mark_sent(&sent);
        loaded.replica.app = Some((context.app_id.clone(), context.publisher.clone()));
        loaded.replica.synced_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs());
        if answer["unchanged"].as_bool() == Some(true) {
            loaded.save()?;
            return Ok(if loaded.replica.outgoing().is_empty() {
                Step::Done
            } else {
                Step::More
            });
        }
        let rev = answer["rev"].as_u64();
        let mut by_key: BTreeMap<String, Vec<Version>> = BTreeMap::new();
        let mut held: BTreeMap<String, Vec<VersionVector>> = BTreeMap::new();
        for (name, versions) in answer["entries"].as_object().into_iter().flatten() {
            for wire in versions.as_array().into_iter().flatten() {
                let Some(sealed) = Sealed::from_json(wire) else {
                    continue;
                };
                // What does not open -- another account's, changed on the
                // way -- is not this device's to read, and is left alone.
                let Ok((key, version)) = loaded.app.open(name, &sealed) else {
                    continue;
                };
                held.entry(key.clone())
                    .or_default()
                    .push(version.vv.clone());
                by_key.entry(key).or_default().push(version);
            }
        }
        let mut store_writes = Vec::new();
        let mut before = BTreeMap::new();
        for (key, versions) in &by_key {
            let was = loaded.replica.keys.get(key).cloned();
            if let Some(value) = loaded.replica.merge_remote(key, versions).store {
                before.insert(key.clone(), was);
                store_writes.push((key.clone(), value));
            }
        }
        let refused = core
            .store
            .apply_synced(&store_writes)
            .map_err(|e| format!("{e:?}"))?;
        // A value this store's rules refused was not taken: the replica goes
        // back to what the store still holds, so the refusal is never sent
        // on as this device deleting or changing it.
        for key in &refused {
            match before.remove(key).flatten() {
                Some(was) => {
                    loaded.replica.keys.insert(key.clone(), was);
                }
                None => {
                    loaded.replica.keys.remove(key);
                }
            }
        }
        core.status.refused = refused;
        if by_key
            .values()
            .flatten()
            .any(|v| v.by != loaded.replica.device && !loaded.names.contains_key(&v.by))
        {
            session.names_stale = true;
        }
        loaded.replica.heal(&held);
        loaded.replica.rev = rev;
        loaded.save()?;
        Ok(if loaded.replica.outgoing().is_empty() {
            Step::Done
        } else {
            Step::More
        })
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
                app_id: "dev.krate.reminders".into(),
                publisher: "unsigned".into(),
                enabled: true,
                background: true,
            }
        }

        /// Offline is normal: every store call answers at once, as a plain
        /// store would, and closing the app does not wait on the hub.
        #[test]
        fn with_the_hub_away_the_store_is_the_store() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("reminders.kv");
            let started = Instant::now();
            {
                let mut kv = KvStore::open(path.clone(), true, Some(&context(dir.path())));
                assert!(matches!(kv, KvStore::Synced(_)));
                kv.set("r1", b"Buy milk".to_vec()).unwrap();
                kv.set("r2", b"Call mum".to_vec()).unwrap();
                kv.delete("r2").unwrap();
                assert_eq!(kv.get("r1").unwrap(), Some(b"Buy milk".to_vec()));
                assert_eq!(kv.keys().unwrap(), vec!["r1".to_string()]);
            }
            let took = started.elapsed();
            assert!(took < Duration::from_secs(5), "closing waited {took:?}");
            // The same file a plain store reads.
            let plain = AppStore::open(path, true);
            assert_eq!(plain.get("r1").unwrap(), Some(b"Buy milk".to_vec()));
            assert_eq!(plain.get("r2").unwrap(), None);
        }

        #[test]
        fn no_context_the_toggle_off_or_no_grant_is_the_plain_store() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("reminders.kv");
            assert!(matches!(
                KvStore::open(path.clone(), true, None),
                KvStore::Plain(_)
            ));
            let mut off = context(dir.path());
            off.enabled = false;
            assert!(matches!(
                KvStore::open(path.clone(), true, Some(&off)),
                KvStore::Plain(_)
            ));
            let kv = KvStore::open(path.clone(), false, Some(&context(dir.path())));
            assert!(matches!(kv, KvStore::Plain(_)));
            assert_eq!(kv.get("a"), Err(StoreError::Denied), "the wall holds");
        }
    }
}
