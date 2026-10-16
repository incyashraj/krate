//! An organisation's team data for one app, on this device (`store.org`,
//! IC-951).
//!
//! Local first: every call reads and writes this device's copy, a JSON file
//! per app, per organisation, per person under `~/.krate/org-data/`, written
//! atomically. Syncing exchanges changes with the organisation's store on its
//! hub (`POST /orgs/<slug>/data/<digest>/sync`): this device's writes go up,
//! everyone else's come down by cursor. The runtime syncs in the background
//! every few seconds while the app is open and once when it closes; offline
//! is never an error.
//!
//! **Never in the window's way (K-1107).** Every exchange runs on the
//! background thread. In a windowed app `sync` only asks that thread to go
//! now and answers at once with where this device stands; [`AppOrg::status`]
//! says the same without asking for anything. A command-line app has no
//! window to freeze, so its `sync` still waits for the exchange, as a script
//! that syncs and exits expects.
//!
//! Which write wins is [`crate::org_merge`]'s rule, the one the hub applies
//! too. The part only a device does is keep its stamps on the hub's clock:
//! after a sync it knows how far its clock is from the hub's
//! ([`Mirror::offset_ms`]), and its own changes that the hub has not taken
//! yet are moved by exactly what the hub moved them by, so both hold the
//! same stamps.
//!
//! The CLI decides whether this run may reach any of it -- the bundle is an
//! organisation's marked, signed app that just checked in, or opened offline
//! on a lease -- and hands over an [`OrgAccess`]. A refusal from the hub
//! ends access for the rest of the run, and when the person is no longer a
//! member at all the organisation's data leaves this device.
//!
//! **End to end (F-316).** The hub holds ciphertext only. Every sync first
//! brings this device's hold of the organisation key up to date
//! ([`krate_orgkeys::agent::Agent::refresh`]: register, take the keyring
//! from its envelope, pass it to waiting devices, rotate after a removal).
//! Each write then goes up sealed under the app's data key -- opaque record
//! and field names, the stamp in the clear, the value and the writer's name
//! inside XChaCha20-Poly1305 -- and each row coming down is opened and
//! checked against its names before it is merged. A row that does not open
//! is refused and counted, never applied. Until this device holds the key
//! its writes wait, as they do offline.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::org_merge::{self, Stamp, Value};

/// What the caller decided about this run (only the CLI knows the bundle's
/// mark, the check-in and the sign-in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgAccess {
    /// An organisation's private app, opened as its release by a member.
    Granted(Box<OrgGrant>),
    /// Anything else: an unmarked app, a loose component, a picture run.
    NotAnOrgApp,
}

/// Where this app's organisation data lives and how to reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgGrant {
    /// The organisation's slug, from the bundle's signed mark.
    pub slug: String,
    /// The hub the open checked in with.
    pub hub: String,
    /// The release's sha256: the hub finds the app id from it.
    pub digest: String,
    /// `Bearer <token>` for the signed-in person, sent only to that hub.
    pub authorization: Option<String>,
    /// How the organisation names the person, for writes made here before
    /// the first sync says so (a GitHub login or an email address).
    pub me: Option<String>,
    /// This device's copy.
    pub mirror: PathBuf,
    /// Moves this device's clock, for tests of a wrong clock. Zero in use.
    pub clock_skew_ms: i64,
    /// The app id the signed manifest names: the app's data key is derived
    /// from it.
    pub app_id: String,
    /// This device's X25519 private key (the keyring is sealed to it), kept
    /// by the CLI encrypted at rest and handed over for the run.
    pub device_secret: DeviceSecret,
    /// What the organisation's device list calls this device.
    pub device_name: String,
    /// The organisation's name as the check-in gave it, for the words an
    /// app shows before the first sync names it (the slug when unknown).
    pub org_name: String,
}

/// A device's private key, never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceSecret(pub [u8; 32]);

impl std::fmt::Debug for DeviceSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DeviceSecret(..)")
    }
}

/// The errors, mirroring the WIT `org-error`.
#[derive(Debug, Clone, PartialEq)]
pub enum OrgError {
    Denied,
    NotAnOrgApp,
    Invalid(String),
    TooLarge,
    OverQuota,
    Refused { reason: String, message: String },
    Unsupported,
    Io(String),
}

/// One record as an app reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub fields: Vec<(String, Value)>,
    pub changed_by: String,
    pub changed_at: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub entries: Vec<Entry>,
    pub removed: Vec<String>,
    pub next: String,
    pub more: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncState {
    pub online: bool,
    pub changed: bool,
    pub pending: u32,
    pub last_synced: Option<u64>,
}

/// Where this device stands with the organisation's store, as the runtime
/// last saw it (K-1107, K-1108).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// The last exchange reached the organisation and nothing waits here.
    Synced,
    /// An exchange is under way, or due: something written here is on its
    /// way, or this run has not reached the organisation yet.
    Syncing,
    /// The last try did not reach the organisation.
    Offline,
    /// This device does not hold the organisation's key yet. Writes wait
    /// here until another member's computer passes the key on.
    WaitingForKey,
}

/// [`Link`], with what an app shows beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub link: Link,
    pub pending: u32,
    /// When this device last reached the organisation's store, in
    /// milliseconds since the Unix epoch; reading counts, not only sending.
    pub reached_at: Option<u64>,
    /// The organisation's name, for words like "someone in Acme".
    pub org: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub role: String,
    pub org: String,
}

/// The most records one `records` page holds.
pub const PAGE_RECORDS: usize = 200;
/// The quota before the hub has said what it is.
pub const DEFAULT_QUOTA: u64 = 50 * 1024 * 1024;
/// How often the runtime syncs while the app is open.
pub const BACKGROUND_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
/// The least time between syncs an app asks for.
const SYNC_FLOOR: std::time::Duration = std::time::Duration::from_secs(2);
/// What one push carries at most, under the hub's 1 MiB.
const PUSH_BYTES: usize = 512 * 1024;
const PUSH_WRITES: usize = 1000;
/// Rounds of push and pull in one sync before it stops for now.
const MAX_ROUNDS: usize = 64;
/// How long closing a windowed app waits for its last sync before letting
/// it finish on its own (K-1107): what was written is kept here and goes up
/// on the next open.
const CLOSE_LIMIT: std::time::Duration = std::time::Duration::from_secs(5);

/// One field as this device holds it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
struct MCell {
    /// The wire value: null, {t}, {n}, {b} or {x}.
    v: serde_json::Value,
    /// The stamp, in its text form.
    s: String,
    /// Who wrote it, as the organisation names them.
    by: String,
    /// Written here and not yet taken by the hub.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pending: bool,
    /// The local generation of a pending write, so a sync knows whether
    /// the field was written again while it was away.
    #[serde(default, skip_serializing_if = "is_zero")]
    g: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
struct Rec {
    /// The local change number, for `records` cursors.
    seq: u64,
    cells: BTreeMap<String, MCell>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
struct Me {
    name: String,
    role: String,
    org: String,
}

/// This device's copy of one app's data in one organisation.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
struct Mirror {
    v: u32,
    /// This copy's device id: 16 random hex digits.
    device: String,
    /// How far this device's clock is behind the hub's, as last measured.
    offset_ms: i64,
    /// The greatest stamp made or seen here.
    last: Option<String>,
    /// The hub's cursor: what this device has pulled.
    cursor: Option<String>,
    /// When the hub last took this device's changes.
    synced_at: Option<u64>,
    quota: Option<u64>,
    me: Option<Me>,
    local_seq: u64,
    generation: u64,
    records: BTreeMap<String, BTreeMap<String, Rec>>,
    /// The keyring, sealed to this device's key: opens only here.
    #[serde(default)]
    envelope: Option<String>,
    /// Rows from the hub that did not open or did not match their names:
    /// refused, never applied.
    #[serde(default)]
    rejected: u64,
}

impl Mirror {
    fn fresh() -> Mirror {
        Mirror {
            v: 1,
            device: random_device(),
            ..Default::default()
        }
    }

    fn last(&self) -> Option<Stamp> {
        self.last.as_deref().and_then(Stamp::parse)
    }

    fn observe(&mut self, stamp: &Stamp) {
        if self.last().is_none_or(|last| *stamp > last) {
            self.last = Some(stamp.to_string());
        }
    }

    /// What this device's copy costs against the app's quota, counted as
    /// the hub counts it: each field's sealed bytes and bookkeeping.
    fn used(&self) -> u64 {
        let mut used = 0;
        for (c, recs) in &self.records {
            for (i, rec) in recs {
                for (f, cell) in &rec.cells {
                    used += sealed_size(c, i, f, cell);
                }
            }
        }
        used
    }

    fn pending(&self) -> u32 {
        self.records
            .values()
            .flat_map(|recs| recs.values())
            .flat_map(|rec| rec.cells.values())
            .filter(|cell| cell.pending)
            .count() as u32
    }
}

fn random_device() -> String {
    let mut bytes = [0u8; 8];
    #[cfg(not(target_arch = "wasm32"))]
    {
        use ring::rand::SecureRandom;
        let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
    }
    #[cfg(target_arch = "wasm32")]
    {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        bytes = nanos.to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The plaintext sealed for one field: its names, value and writer.
fn payload(c: &str, i: &str, f: &str, v: &serde_json::Value, by: &str) -> String {
    serde_json::json!({ "c": c, "i": i, "f": f, "v": v, "w": by }).to_string()
}

/// A nonce and a tag (40 bytes) on the payload, and the hub's 160 bytes
/// of bookkeeping per row (cloud/worker/src/org-data.js ROW_OVERHEAD).
fn sealed_size(c: &str, i: &str, f: &str, cell: &MCell) -> u64 {
    (payload(c, i, f, &cell.v, &cell.by).len() + 40 + 160) as u64
}

/// The cells of a record as the merge rule reads them.
fn cells_of(rec: &Rec) -> Vec<(String, org_merge::Cell)> {
    rec.cells
        .iter()
        .filter_map(|(name, cell)| {
            Some((
                name.clone(),
                org_merge::Cell {
                    value: Value::from_wire(&cell.v)?,
                    stamp: Stamp::parse(&cell.s)?,
                },
            ))
        })
        .collect()
}

fn entry_of(id: &str, rec: &Rec) -> Option<Entry> {
    let cells = cells_of(rec);
    let live = org_merge::live_fields(cells.iter().map(|(n, c)| (n.as_str(), c)));
    if live.is_empty() {
        return None;
    }
    let newest = live
        .iter()
        .max_by(|a, b| a.1.stamp.cmp(&b.1.stamp))
        .map(|(name, cell)| (name.to_string(), cell.stamp.wall))?;
    let changed_by = rec
        .cells
        .get(&newest.0)
        .map(|cell| cell.by.clone())
        .unwrap_or_default();
    Some(Entry {
        id: id.to_string(),
        fields: live
            .iter()
            .map(|(name, cell)| (name.to_string(), cell.value.clone()))
            .collect(),
        changed_by,
        changed_at: newest.1,
    })
}

/// Drop every field the record's tombstone has outlived.
fn bury(rec: &mut Rec) {
    let Some(tomb) = rec
        .cells
        .get(org_merge::TOMBSTONE)
        .and_then(|c| Stamp::parse(&c.s))
    else {
        return;
    };
    rec.cells.retain(|name, cell| {
        name == org_merge::TOMBSTONE || Stamp::parse(&cell.s).is_some_and(|s| s > tomb)
    });
}

struct Inner {
    grant: OrgGrant,
    mirror: Mirror,
    /// The hub refused: the reason and the words. Nothing is served after.
    refused: Option<(String, String)>,
    last_online: bool,
    /// When the last sync finished, so an app calling faster than the
    /// floor is answered from here.
    last_sync: Option<std::time::Instant>,
    /// A sync (the background one, say) changed what the app reads, and
    /// the app's own `sync` has not said so yet.
    unseen_change: bool,
    /// The organisation's keyring, while this device holds it.
    #[cfg(not(target_arch = "wasm32"))]
    held: Option<krate_orgkeys::agent::Held>,
    /// Whether this device holds the organisation's key, as the last try
    /// found (K-1108).
    key: KeyHold,
    /// A try at the organisation has finished this run, reached or not.
    tried: bool,
    /// The background thread is in an exchange now.
    syncing: bool,
    /// When this device last reached the organisation, in hub milliseconds.
    reached_at: Option<u64>,
}

/// This device's hold of the organisation's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyHold {
    /// Not asked yet this run, and no keyring on disk.
    Unknown,
    Holds,
    /// The organisation has no key yet, or another member's device has not
    /// passed it to this one.
    Waiting,
}

impl Inner {
    fn now_ms(&self) -> u64 {
        let local = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        (local + self.grant.clock_skew_ms + self.mirror.offset_ms).max(0) as u64
    }

    fn save(&self) -> Result<(), OrgError> {
        let path = &self.grant.mirror;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| OrgError::Io(e.to_string()))?;
        }
        let text = serde_json::to_string(&self.mirror).map_err(|e| OrgError::Io(e.to_string()))?;
        let staged = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&staged, text).map_err(|e| OrgError::Io(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&staged, path).map_err(|e| {
            let _ = std::fs::remove_file(&staged);
            OrgError::Io(e.to_string())
        })
    }

    fn check(&self) -> Result<(), OrgError> {
        match &self.refused {
            Some((reason, message)) => Err(OrgError::Refused {
                reason: reason.clone(),
                message: message.clone(),
            }),
            None => Ok(()),
        }
    }

    fn me_name(&self) -> String {
        self.mirror
            .me
            .as_ref()
            .map(|me| me.name.clone())
            .or_else(|| self.grant.me.clone())
            .unwrap_or_default()
    }

    /// Write cells into one record, as one change.
    fn write(
        &mut self,
        collection: &str,
        id: &str,
        fields: Vec<(String, Value)>,
    ) -> Result<(), OrgError> {
        self.check()?;
        if let Some(problem) = org_merge::name_problem("collection name", collection)
            .or_else(|| org_merge::name_problem("record id", id))
        {
            return Err(OrgError::Invalid(problem));
        }
        for (name, value) in &fields {
            if name != org_merge::TOMBSTONE {
                if let Some(problem) = org_merge::name_problem("field name", name) {
                    return Err(OrgError::Invalid(problem));
                }
            }
            if let Value::Number(n) = value {
                if !n.is_finite() {
                    return Err(OrgError::Invalid(format!(
                        "the field {name} is not a finite number"
                    )));
                }
            }
        }
        let device = self.mirror.device.clone();
        let phys = self.now_ms();
        let by = self.me_name();
        let generation = self.mirror.generation + 1;
        let mut last = self.mirror.last();
        let mut rec = self
            .mirror
            .records
            .get(collection)
            .and_then(|recs| recs.get(id))
            .cloned()
            .unwrap_or_default();
        for (name, value) in fields {
            let stamp = Stamp::next(last.as_ref(), phys, &device);
            rec.cells.insert(
                name,
                MCell {
                    v: value.to_wire(),
                    s: stamp.to_string(),
                    by: by.clone(),
                    pending: true,
                    g: generation,
                },
            );
            last = Some(stamp);
        }
        bury(&mut rec);
        let cells = cells_of(&rec);
        if org_merge::record_size(cells.iter().map(|(n, c)| (n.as_str(), c)))
            > org_merge::MAX_RECORD_BYTES
        {
            return Err(OrgError::TooLarge);
        }
        let size_of = |rec: &Rec| -> u64 {
            rec.cells
                .iter()
                .map(|(f, cell)| sealed_size(collection, id, f, cell))
                .sum()
        };
        let old = self
            .mirror
            .records
            .get(collection)
            .and_then(|recs| recs.get(id))
            .map(size_of)
            .unwrap_or(0);
        let new = size_of(&rec);
        let quota = self.mirror.quota.unwrap_or(DEFAULT_QUOTA);
        if new > old && self.mirror.used() - old + new > quota {
            return Err(OrgError::OverQuota);
        }
        self.mirror
            .records
            .entry(collection.to_string())
            .or_default()
            .insert(id.to_string(), rec);
        self.mirror.generation = generation;
        if let Some(last) = last {
            self.mirror.observe(&last);
        }
        self.mirror.local_seq += 1;
        let seq = self.mirror.local_seq;
        if let Some(rec) = self
            .mirror
            .records
            .get_mut(collection)
            .and_then(|recs| recs.get_mut(id))
        {
            rec.seq = seq;
        }
        self.save()
    }
}

/// What the app asks of the background thread.
#[derive(Default)]
struct Asks {
    /// Sync now, not at the next tick.
    now: bool,
    /// The app is closing: one last sync, then stop.
    stop: bool,
}

/// One app's organisation data for this run.
pub struct AppOrg {
    access: Option<Arc<Mutex<Inner>>>,
    /// Serialises syncs: the app's own and the background one.
    sync_lock: Arc<Mutex<()>>,
    /// Wakes the background thread early.
    asks: Arc<(Mutex<Asks>, std::sync::Condvar)>,
    /// Whether `sync` waits for the exchange: a command-line app's does,
    /// a windowed app's never does (K-1107).
    waits: std::sync::atomic::AtomicBool,
    background: Option<std::thread::JoinHandle<()>>,
}

impl AppOrg {
    /// Open this run's organisation data. `None` means the app was not
    /// granted `store.org`, which every call answers as `denied`.
    pub fn open(access: OrgAccess) -> AppOrg {
        let mut org = Self::open_quiet(access);
        org.start_background();
        org
    }

    /// [`AppOrg::open`] without the background thread, for tests that set
    /// the state themselves.
    fn open_quiet(access: OrgAccess) -> AppOrg {
        let access = match access {
            OrgAccess::NotAnOrgApp => None,
            OrgAccess::Granted(grant) => {
                let mirror = std::fs::read_to_string(&grant.mirror)
                    .ok()
                    .and_then(|text| serde_json::from_str::<Mirror>(&text).ok())
                    .filter(|m| org_merge::valid_device(&m.device))
                    .unwrap_or_else(Mirror::fresh);
                // The keyring this device was given, so it works offline.
                #[cfg(not(target_arch = "wasm32"))]
                let held = mirror.envelope.as_ref().and_then(|envelope| {
                    let device =
                        krate_orgkeys::KeyPair::from_secret(&grant.device_secret.0).ok()?;
                    let keyring =
                        krate_orgkeys::Keyring::open(envelope, &device, &grant.slug).ok()?;
                    Some(krate_orgkeys::agent::Held {
                        keyring,
                        envelope: envelope.clone(),
                    })
                });
                #[cfg(not(target_arch = "wasm32"))]
                let key = if held.is_some() {
                    KeyHold::Holds
                } else {
                    KeyHold::Unknown
                };
                #[cfg(target_arch = "wasm32")]
                let key = KeyHold::Unknown;
                Some(Arc::new(Mutex::new(Inner {
                    grant: *grant,
                    mirror,
                    refused: None,
                    last_online: false,
                    last_sync: None,
                    unseen_change: false,
                    #[cfg(not(target_arch = "wasm32"))]
                    held,
                    key,
                    tried: false,
                    syncing: false,
                    reached_at: None,
                })))
            }
        };
        AppOrg {
            access,
            sync_lock: Arc::new(Mutex::new(())),
            asks: Arc::new((Mutex::new(Asks::default()), std::sync::Condvar::new())),
            waits: std::sync::atomic::AtomicBool::new(true),
            background: None,
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start_background(&mut self) {
        let Some(inner) = self.access.clone() else {
            return;
        };
        let (lock, asks) = (self.sync_lock.clone(), self.asks.clone());
        self.background = std::thread::Builder::new()
            .name("krate-org-sync".into())
            .spawn(move || {
                let mut asked = false;
                loop {
                    let refused = inner.lock().map(|i| i.refused.is_some()).unwrap_or(true);
                    if refused {
                        return;
                    }
                    // Asked by the app faster than the floor, with nothing of
                    // its own to send: answered from here, so a timer loop
                    // cannot hammer the hub.
                    let floor = asked
                        && inner.lock().is_ok_and(|i| {
                            i.last_sync.is_some_and(|at| at.elapsed() < SYNC_FLOOR)
                                && i.mirror.pending() == 0
                        });
                    if !floor {
                        let _ = sync_now(&inner, &lock);
                    }
                    let (asks_lock, wake) = &*asks;
                    let Ok(mut ask) = asks_lock.lock() else {
                        return;
                    };
                    let until = std::time::Instant::now() + BACKGROUND_EVERY;
                    while !ask.now && !ask.stop {
                        let left = until.saturating_duration_since(std::time::Instant::now());
                        if left.is_zero() {
                            break;
                        }
                        ask = match wake.wait_timeout(ask, left) {
                            Ok((ask, _)) => ask,
                            Err(_) => return,
                        };
                    }
                    if ask.stop {
                        drop(ask);
                        // Once more on close, so what the person did last
                        // reaches the team.
                        let refused = inner.lock().map(|i| i.refused.is_some()).unwrap_or(true);
                        if !refused {
                            let _ = sync_now(&inner, &lock);
                        }
                        return;
                    }
                    asked = std::mem::take(&mut ask.now);
                }
            })
            .ok();
    }

    /// A windowed app: `sync` never waits for the network from here on.
    pub fn set_windowed(&self) {
        self.waits.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Wake the background thread for a sync now.
    fn ask_now(&self) {
        let (asks, wake) = &*self.asks;
        if let Ok(mut ask) = asks.lock() {
            ask.now = true;
        }
        wake.notify_all();
    }

    #[cfg(target_arch = "wasm32")]
    fn start_background(&mut self) {}

    fn inner(&self) -> Result<std::sync::MutexGuard<'_, Inner>, OrgError> {
        let inner = self.access.as_ref().ok_or(OrgError::NotAnOrgApp)?;
        inner
            .lock()
            .map_err(|_| OrgError::Io("the organisation store is unavailable".into()))
    }

    pub fn put(
        &self,
        collection: &str,
        id: &str,
        fields: Vec<(String, Value)>,
    ) -> Result<(), OrgError> {
        let mut inner = self.inner()?;
        if fields.is_empty() {
            inner.check()?;
            return Ok(());
        }
        if fields.iter().any(|(name, _)| name.is_empty()) {
            return Err(OrgError::Invalid("a field name cannot be empty".into()));
        }
        inner.write(collection, id, fields)
    }

    pub fn delete(&self, collection: &str, id: &str) -> Result<(), OrgError> {
        let mut inner = self.inner()?;
        let exists = inner
            .mirror
            .records
            .get(collection)
            .and_then(|recs| recs.get(id))
            .is_some_and(|rec| entry_of(id, rec).is_some());
        if !exists {
            inner.check()?;
            if let Some(problem) = org_merge::name_problem("collection name", collection)
                .or_else(|| org_merge::name_problem("record id", id))
            {
                return Err(OrgError::Invalid(problem));
            }
            return Ok(());
        }
        inner.write(
            collection,
            id,
            vec![(org_merge::TOMBSTONE.to_string(), Value::Null)],
        )
    }

    pub fn get(&self, collection: &str, id: &str) -> Result<Option<Entry>, OrgError> {
        let inner = self.inner()?;
        inner.check()?;
        if let Some(problem) = org_merge::name_problem("collection name", collection)
            .or_else(|| org_merge::name_problem("record id", id))
        {
            return Err(OrgError::Invalid(problem));
        }
        Ok(inner
            .mirror
            .records
            .get(collection)
            .and_then(|recs| recs.get(id))
            .and_then(|rec| entry_of(id, rec)))
    }

    pub fn records(&self, collection: &str, since: Option<&str>) -> Result<Page, OrgError> {
        let inner = self.inner()?;
        inner.check()?;
        if let Some(problem) = org_merge::name_problem("collection name", collection) {
            return Err(OrgError::Invalid(problem));
        }
        let after = match since {
            None => None,
            Some(text) => Some(text.parse::<u64>().map_err(|_| {
                OrgError::Invalid("that cursor is not one an earlier page gave".into())
            })?),
        };
        let mut recs: Vec<(&String, &Rec)> = inner
            .mirror
            .records
            .get(collection)
            .map(|recs| recs.iter().collect())
            .unwrap_or_default();
        recs.retain(|(_, rec)| after.is_none_or(|after| rec.seq > after));
        recs.sort_by_key(|(id, rec)| (rec.seq, (*id).clone()));
        let mut page = Page {
            entries: Vec::new(),
            removed: Vec::new(),
            next: after.unwrap_or(0).to_string(),
            more: false,
        };
        let mut taken = 0;
        for (id, rec) in recs {
            match entry_of(id, rec) {
                Some(entry) => page.entries.push(entry),
                // From the start, a removed record is simply absent.
                None if after.is_none() => continue,
                None => page.removed.push(id.clone()),
            }
            taken += 1;
            page.next = rec.seq.to_string();
            if taken == PAGE_RECORDS {
                page.more = true;
                break;
            }
        }
        if page.more {
            // Only when something really is left.
            let last: u64 = page.next.parse().unwrap_or(0);
            page.more = inner
                .mirror
                .records
                .get(collection)
                .is_some_and(|recs| recs.values().any(|rec| rec.seq > last));
        }
        Ok(page)
    }

    pub fn who(&self) -> Result<Option<Member>, OrgError> {
        let inner = self.inner()?;
        inner.check()?;
        Ok(inner.mirror.me.as_ref().map(|me| Member {
            name: me.name.clone(),
            role: me.role.clone(),
            org: me.org.clone(),
        }))
    }

    /// Where this device stands, from here: it never reaches the network and
    /// never waits behind a sync (K-1107, K-1108).
    pub fn status(&self) -> Result<Status, OrgError> {
        let inner = self.inner()?;
        inner.check()?;
        let asked = self.asks.0.lock().is_ok_and(|ask| ask.now);
        let pending = inner.mirror.pending();
        let link = if inner.tried && !inner.last_online {
            Link::Offline
        } else if inner.key == KeyHold::Waiting {
            Link::WaitingForKey
        } else if !inner.tried || inner.syncing || asked || pending > 0 {
            Link::Syncing
        } else {
            Link::Synced
        };
        let org = inner
            .mirror
            .me
            .as_ref()
            .map(|me| me.org.clone())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| {
                if inner.grant.org_name.is_empty() {
                    inner.grant.slug.clone()
                } else {
                    inner.grant.org_name.clone()
                }
            });
        Ok(Status {
            link,
            pending,
            reached_at: inner.reached_at,
            org,
        })
    }

    /// Sync now, for the app. In a windowed app it asks the background
    /// thread to go now and answers at once with where this device stands;
    /// `changed` says so on a later call once that sync brings something
    /// (K-1107). In a command-line app it waits for the exchange. Faster
    /// than every two seconds answers from here.
    pub fn sync(&self) -> Result<SyncState, OrgError> {
        let inner_arc = self.access.as_ref().ok_or(OrgError::NotAnOrgApp)?.clone();
        if !self.waits.load(std::sync::atomic::Ordering::SeqCst) {
            let mut inner = self.inner()?;
            inner.check()?;
            self.ask_now();
            let changed = std::mem::take(&mut inner.unseen_change);
            return Ok(SyncState {
                online: inner.last_online,
                changed,
                pending: inner.mirror.pending(),
                last_synced: inner.mirror.synced_at,
            });
        }
        let _one = self
            .sync_lock
            .lock()
            .map_err(|_| OrgError::Io("the organisation store is unavailable".into()))?;
        {
            let mut inner = self.inner()?;
            inner.check()?;
            // Faster than the floor with nothing of its own to send: answered
            // from here, so a timer loop cannot hammer the hub.
            if inner.last_sync.is_some_and(|at| at.elapsed() < SYNC_FLOOR)
                && inner.mirror.pending() == 0
            {
                let changed = std::mem::take(&mut inner.unseen_change);
                return Ok(SyncState {
                    online: inner.last_online,
                    changed,
                    pending: inner.mirror.pending(),
                    last_synced: inner.mirror.synced_at,
                });
            }
        }
        let state = sync_locked(&inner_arc)?;
        let changed = std::mem::take(&mut self.inner()?.unseen_change);
        Ok(SyncState { changed, ..state })
    }
}

impl Drop for AppOrg {
    /// Once more on close, so what the person did last reaches the team: the
    /// background thread's last sync. A command-line app waits for it; a
    /// windowed one gives it [`CLOSE_LIMIT`] and lets it finish on its own,
    /// so a slow network never holds the close up for long. A write that has
    /// not gone up is kept here and goes on the next open.
    fn drop(&mut self) {
        {
            let (asks, wake) = &*self.asks;
            if let Ok(mut ask) = asks.lock() {
                ask.stop = true;
            }
            wake.notify_all();
        }
        let Some(handle) = self.background.take() else {
            return;
        };
        if self.waits.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = handle.join();
            return;
        }
        let until = std::time::Instant::now() + CLOSE_LIMIT;
        while !handle.is_finished() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn sync_now(_inner: &Arc<Mutex<Inner>>, _lock: &Arc<Mutex<()>>) -> Result<SyncState, OrgError> {
    // A tab cannot reach an organisation's store yet (IC-956): said, not
    // pretended.
    Err(OrgError::Unsupported)
}

/// What one round sends.
struct Round {
    /// This device's writes, in plaintext: sealed just before they leave.
    writes: Vec<serde_json::Value>,
    since: Option<String>,
    device: String,
    sent_now: u64,
    /// (collection, id, field, generation) of each write sent.
    sent: Vec<(String, String, String, u64)>,
    url: String,
    authorization: Option<String>,
    left_behind: bool,
}

/// Every refusal that means the person is no longer in the organisation:
/// its data leaves this device.
fn ends_membership(reason: &str) -> bool {
    matches!(reason, "removed" | "not-a-member")
}

/// The hub refused this person or this version: access ends for the run,
/// and someone no longer in the organisation keeps none of its data.
#[cfg(not(target_arch = "wasm32"))]
fn refused(inner: &mut Inner, reason: String, message: String) -> OrgError {
    if ends_membership(&reason) {
        let _ = std::fs::remove_file(&inner.grant.mirror);
        inner.mirror = Mirror::fresh();
        inner.held = None;
    }
    let reason = if reason.is_empty() {
        "not-a-member".to_string()
    } else {
        reason
    };
    inner.refused = Some((reason.clone(), message.clone()));
    inner.last_online = true;
    OrgError::Refused { reason, message }
}

/// Seal one round's writes under the current version of the app's data key.
#[cfg(not(target_arch = "wasm32"))]
fn seal_writes(
    writes: &[serde_json::Value],
    held: &krate_orgkeys::agent::Held,
    app_id: &str,
) -> Result<Vec<serde_json::Value>, OrgError> {
    use krate_orgkeys::{AppDataKey, RowKind};
    let (version, _) = held.keyring.current();
    let dk = AppDataKey::derive(&held.keyring, version, app_id)
        .ok_or_else(|| OrgError::Io("this device's keyring has no current key".into()))?;
    writes
        .iter()
        .map(|w| {
            let (c, i, f) = (
                w["c"].as_str().unwrap_or(""),
                w["i"].as_str().unwrap_or(""),
                w["f"].as_str().unwrap_or(""),
            );
            let kind = if f == org_merge::TOMBSTONE {
                RowKind::Tombstone
            } else if w["v"].is_null() {
                RowKind::Null
            } else {
                RowKind::Value
            };
            let (r, k) = (dk.record_key(c, i), dk.field_key(c, i, f));
            let plain = payload(c, i, f, &w["v"], w["by"].as_str().unwrap_or(""));
            let ct = dk
                .seal(&r, &k, kind, plain.as_bytes())
                .map_err(|e| OrgError::Io(e.to_string()))?;
            Ok(serde_json::json!({
                "r": r, "k": k, "kind": kind.as_str(), "s": w["s"], "ver": version, "ct": ct,
            }))
        })
        .collect()
}

/// Open the hub's sealed rows into the plaintext rows [`apply`] merges.
/// A row that does not open, or whose names are not the ones it was sealed
/// under, is refused: left out and counted.
#[cfg(not(target_arch = "wasm32"))]
fn open_rows(
    answer: &serde_json::Value,
    held: &krate_orgkeys::agent::Held,
    app_id: &str,
) -> (serde_json::Value, u64) {
    use krate_orgkeys::{AppDataKey, RowKind};
    let mut keys: BTreeMap<u32, Option<AppDataKey>> = BTreeMap::new();
    let mut rejected = 0;
    let mut rows = Vec::new();
    for row in answer["rows"].as_array().into_iter().flatten() {
        let opened = (|| {
            let (r, k) = (row["r"].as_str()?, row["k"].as_str()?);
            let kind = RowKind::parse(row["kind"].as_str()?)?;
            let version = u32::try_from(row["ver"].as_u64()?).ok()?;
            let dk = keys
                .entry(version)
                .or_insert_with(|| AppDataKey::derive(&held.keyring, version, app_id))
                .as_ref()?;
            let plain = dk.open(r, k, kind, row["ct"].as_str()?).ok()?;
            let p: serde_json::Value = serde_json::from_slice(&plain).ok()?;
            let (c, i, f) = (p["c"].as_str()?, p["i"].as_str()?, p["f"].as_str()?);
            let kind_matches = match kind {
                RowKind::Tombstone => f == org_merge::TOMBSTONE,
                RowKind::Null => f != org_merge::TOMBSTONE && p["v"].is_null(),
                RowKind::Value => f != org_merge::TOMBSTONE && !p["v"].is_null(),
            };
            (dk.record_key(c, i) == r && dk.field_key(c, i, f) == k && kind_matches).then(|| {
                serde_json::json!({ "c": c, "i": i, "f": f, "v": p["v"], "s": row["s"], "by": p["w"] })
            })
        })();
        match opened {
            Some(plain) => rows.push(plain),
            None => rejected += 1,
        }
    }
    let mut plain = answer.clone();
    plain["rows"] = serde_json::Value::Array(rows);
    (plain, rejected)
}

#[cfg(not(target_arch = "wasm32"))]
fn sync_now(inner_arc: &Arc<Mutex<Inner>>, lock: &Arc<Mutex<()>>) -> Result<SyncState, OrgError> {
    let _one = lock
        .lock()
        .map_err(|_| OrgError::Io("the organisation store is unavailable".into()))?;
    sync_locked(inner_arc)
}

/// One sync, with the sync lock already held. Notes when it finished and
/// whether it changed anything the app has not been told about.
#[cfg(not(target_arch = "wasm32"))]
fn sync_locked(inner_arc: &Arc<Mutex<Inner>>) -> Result<SyncState, OrgError> {
    if let Ok(mut inner) = inner_arc.lock() {
        inner.syncing = true;
    }
    let result = sync_rounds(inner_arc);
    if let Ok(mut inner) = inner_arc.lock() {
        inner.syncing = false;
        inner.tried = true;
        inner.last_sync = Some(std::time::Instant::now());
        if let Ok(state) = &result {
            inner.unseen_change |= state.changed;
        }
    }
    result
}

#[cfg(target_arch = "wasm32")]
fn sync_locked(_inner: &Arc<Mutex<Inner>>) -> Result<SyncState, OrgError> {
    Err(OrgError::Unsupported)
}

#[cfg(not(target_arch = "wasm32"))]
fn sync_rounds(inner_arc: &Arc<Mutex<Inner>>) -> Result<SyncState, OrgError> {
    let lock_inner = || {
        inner_arc
            .lock()
            .map_err(|_| OrgError::Io("the organisation store is unavailable".into()))
    };
    let waiting = |inner: &Inner, online: bool| SyncState {
        online,
        changed: false,
        pending: inner.mirror.pending(),
        last_synced: inner.mirror.synced_at,
    };
    // The key first: this device's hold of it, and a holder's duties.
    let mut key_tries = 0;
    let mut held = match refresh_key(inner_arc)? {
        Some(held) => held,
        None => {
            let inner = lock_inner()?;
            return Ok(waiting(&inner, inner.last_online));
        }
    };
    let mut changed = false;
    let mut online = false;
    let mut rounds = 0;
    while rounds < MAX_ROUNDS {
        rounds += 1;
        let (round, app_id) = {
            let inner = lock_inner()?;
            inner.check()?;
            (prepare(&inner), inner.grant.app_id.clone())
        };
        let sealed = seal_writes(&round.writes, &held, &app_id)?;
        let body = serde_json::json!({
            "v": 1,
            "device": round.device,
            "now": round.sent_now,
            "since": round.since,
            "writes": sealed,
        })
        .to_string();
        let agent = krate_http::builder(&round.url)
            .timeout_connect(std::time::Duration::from_secs(4))
            .timeout_read(std::time::Duration::from_secs(10))
            .build();
        let mut request = agent
            .post(&round.url)
            .set("content-type", "application/json")
            .set("accept", "application/json");
        if let Some(value) = &round.authorization {
            request = request.set("authorization", value);
        }
        let answer = match request.send_string(&body) {
            Ok(response) => response
                .into_string()
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()),
            Err(ureq::Error::Status(status, response)) => {
                let body: serde_json::Value =
                    serde_json::from_str(&response.into_string().unwrap_or_default())
                        .unwrap_or_default();
                let reason = body["error"].as_str().unwrap_or("").to_string();
                let message = body["message"].as_str().unwrap_or("").to_string();
                match status {
                    401 | 403 | 404 | 410 => {
                        let mut inner = lock_inner()?;
                        return Err(refused(&mut inner, reason, message));
                    }
                    // The key moved on (a member was removed): take the new
                    // version, then send again under it.
                    409 if reason == "key-rotated" && key_tries < 2 => {
                        key_tries += 1;
                        match refresh_key(inner_arc)? {
                            Some(next) => {
                                held = next;
                                continue;
                            }
                            None => {
                                let inner = lock_inner()?;
                                return Ok(waiting(&inner, true));
                            }
                        }
                    }
                    409 => {
                        let inner = lock_inner()?;
                        return Ok(waiting(&inner, true));
                    }
                    413 if reason == "over-quota" => return Err(OrgError::OverQuota),
                    413 => return Err(OrgError::TooLarge),
                    400 => return Err(OrgError::Io(message)),
                    // Busy, rate limited, down: offline for now.
                    _ => None,
                }
            }
            Err(ureq::Error::Transport(_)) => None,
        };
        let Some(answer) = answer else {
            let mut inner = lock_inner()?;
            inner.last_online = false;
            return Ok(SyncState {
                online: false,
                changed,
                pending: inner.mirror.pending(),
                last_synced: inner.mirror.synced_at,
            });
        };
        online = true;
        let (plain, rejected) = open_rows(&answer, &held, &app_id);
        let mut inner = lock_inner()?;
        inner.last_online = true;
        inner.reached_at = answer["now"].as_u64().or_else(|| Some(inner.now_ms()));
        inner.mirror.rejected += rejected;
        changed |= apply(&mut inner.mirror, &round, &plain);
        inner.save()?;
        let again = answer["more"].as_bool().unwrap_or(false) || round.left_behind;
        if !again {
            break;
        }
    }
    let inner = lock_inner()?;
    Ok(SyncState {
        online,
        changed,
        pending: inner.mirror.pending(),
        last_synced: inner.mirror.synced_at,
    })
}

/// Bring this device's hold of the organisation key up to date, and do a
/// holder's duties. `None` when there is no key yet, this device is waiting
/// for one, or the hub is out of reach.
#[cfg(not(target_arch = "wasm32"))]
fn refresh_key(
    inner_arc: &Arc<Mutex<Inner>>,
) -> Result<Option<krate_orgkeys::agent::Held>, OrgError> {
    use krate_orgkeys::agent::{Agent, AgentError, Status};
    let unavailable = || OrgError::Io("the organisation store is unavailable".into());
    let (agent, held) = {
        let inner = inner_arc.lock().map_err(|_| unavailable())?;
        let device = krate_orgkeys::KeyPair::from_secret(&inner.grant.device_secret.0)
            .map_err(|e| OrgError::Io(format!("this device's key: {e}")))?;
        let agent = Agent::new(
            &inner.grant.hub,
            &inner.grant.slug,
            inner.grant.authorization.clone(),
            device,
            &inner.grant.device_name,
        );
        (agent, inner.held.clone())
    };
    let status = agent.refresh(held.as_ref());
    let mut inner = inner_arc.lock().map_err(|_| unavailable())?;
    match status {
        Ok(Status::Holds(held)) => {
            if inner.mirror.envelope.as_deref() != Some(held.envelope.as_str()) {
                inner.mirror.envelope = Some(held.envelope.clone());
                inner.save()?;
            }
            inner.held = Some(held.clone());
            inner.key = KeyHold::Holds;
            inner.last_online = true;
            inner.reached_at = Some(inner.now_ms());
            Ok(Some(held))
        }
        Ok(Status::NoKey) | Ok(Status::Waiting) => {
            inner.key = KeyHold::Waiting;
            inner.last_online = true;
            inner.reached_at = Some(inner.now_ms());
            Ok(None)
        }
        Err(AgentError::Offline) => {
            inner.last_online = false;
            Ok(None)
        }
        Err(AgentError::Refused {
            status: 401 | 403 | 404 | 410,
            reason,
            message,
        }) => Err(refused(&mut inner, reason, message)),
        Err(other) => Err(OrgError::Io(format!("the organisation's key: {other}"))),
    }
}

/// The next push: this device's pending writes, as many as fit.
fn prepare(inner: &Inner) -> Round {
    let mirror = &inner.mirror;
    let mut writes = Vec::new();
    let mut sent = Vec::new();
    let mut bytes = 0usize;
    let mut left_behind = false;
    'all: for (c, recs) in &mirror.records {
        for (i, rec) in recs {
            for (f, cell) in &rec.cells {
                if !cell.pending {
                    continue;
                }
                let size = c.len() + i.len() + f.len() + cell.v.to_string().len() + 96;
                if writes.len() >= PUSH_WRITES || (bytes + size > PUSH_BYTES && !writes.is_empty())
                {
                    left_behind = true;
                    break 'all;
                }
                bytes += size;
                writes.push(
                    serde_json::json!({ "c": c, "i": i, "f": f, "v": cell.v, "s": cell.s, "by": cell.by }),
                );
                sent.push((c.clone(), i.clone(), f.clone(), cell.g));
            }
        }
    }
    let sent_now = inner.now_ms();
    let hub = inner.grant.hub.trim_end_matches('/');
    Round {
        writes,
        since: mirror.cursor.clone(),
        device: mirror.device.clone(),
        sent_now,
        sent,
        url: format!(
            "{hub}/orgs/{}/data/{}/sync",
            inner.grant.slug, inner.grant.digest
        ),
        authorization: inner.grant.authorization.clone(),
        left_behind,
    }
}

/// Fold the hub's answer into the mirror. True when what an app reads
/// changed.
fn apply(mirror: &mut Mirror, round: &Round, answer: &serde_json::Value) -> bool {
    let hub_now = answer["now"].as_u64().unwrap_or(round.sent_now);
    // This device's own stamps not yet taken are on its old clock: move them
    // exactly as the hub moved the ones it took, then measure the clock
    // again.
    for recs in mirror.records.values_mut() {
        for rec in recs.values_mut() {
            for cell in rec.cells.values_mut() {
                if cell.pending {
                    if let Some(stamp) = Stamp::parse(&cell.s) {
                        cell.s = org_merge::rebase(&stamp, round.sent_now, hub_now).to_string();
                    }
                }
            }
        }
    }
    if let Some(last) = mirror.last() {
        mirror.last = Some(org_merge::rebase(&last, round.sent_now, hub_now).to_string());
    }
    let delta = i128::from(hub_now) - i128::from(round.sent_now);
    if delta.unsigned_abs() > u128::from(org_merge::DRIFT_MS) {
        mirror.offset_ms = mirror.offset_ms.saturating_add(delta as i64);
    }
    // What was sent and not written again since is the hub's now.
    for (c, i, f, g) in &round.sent {
        if let Some(cell) = mirror
            .records
            .get_mut(c)
            .and_then(|recs| recs.get_mut(i))
            .and_then(|rec| rec.cells.get_mut(f))
        {
            if cell.pending && cell.g == *g {
                cell.pending = false;
                cell.g = 0;
            }
        }
    }
    if !round.sent.is_empty() {
        mirror.synced_at = Some(hub_now);
    }
    let mut changed = false;
    // A device behind the hub's oldest tombstone starts over from the
    // hub's copy, keeping only its own changes not yet taken.
    if answer["full"].as_bool() == Some(true) {
        for recs in mirror.records.values_mut() {
            for rec in recs.values_mut() {
                let before = rec.cells.len();
                rec.cells.retain(|_, cell| cell.pending);
                changed |= rec.cells.len() != before;
            }
            recs.retain(|_, rec| !rec.cells.is_empty());
        }
        mirror.records.retain(|_, recs| !recs.is_empty());
    }
    for row in answer["rows"].as_array().into_iter().flatten() {
        let (Some(c), Some(i), Some(f), Some(s)) = (
            row["c"].as_str(),
            row["i"].as_str(),
            row["f"].as_str(),
            row["s"].as_str().and_then(Stamp::parse),
        ) else {
            continue;
        };
        if Value::from_wire(&row["v"]).is_none() {
            continue;
        }
        mirror.observe(&s);
        let rec = mirror
            .records
            .entry(c.to_string())
            .or_default()
            .entry(i.to_string())
            .or_default();
        let before = entry_of(i, rec);
        let held = rec.cells.get(f).and_then(|cell| Stamp::parse(&cell.s));
        if !org_merge::wins(&s, held.as_ref()) {
            continue;
        }
        let tomb = rec
            .cells
            .get(org_merge::TOMBSTONE)
            .and_then(|cell| Stamp::parse(&cell.s));
        if f != org_merge::TOMBSTONE && !org_merge::alive(&s, tomb.as_ref()) {
            continue;
        }
        rec.cells.insert(
            f.to_string(),
            MCell {
                v: row["v"].clone(),
                s: s.to_string(),
                by: row["by"].as_str().unwrap_or("").to_string(),
                pending: false,
                g: 0,
            },
        );
        bury(rec);
        if entry_of(i, rec) != before {
            mirror.local_seq += 1;
            rec.seq = mirror.local_seq;
            changed = true;
        }
    }
    if let Some(cursor) = answer["cursor"].as_str() {
        mirror.cursor = Some(cursor.to_string());
    }
    if let Some(limit) = answer["quota"]["limit"].as_u64() {
        mirror.quota = Some(limit);
    }
    if let (Some(name), Some(role)) = (answer["me"]["name"].as_str(), answer["me"]["role"].as_str())
    {
        mirror.me = Some(Me {
            name: name.to_string(),
            role: role.to_string(),
            org: answer["org"]["name"]
                .as_str()
                .or(answer["org"]["slug"].as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(dir: &std::path::Path, name: &str, skew: i64) -> Box<OrgGrant> {
        Box::new(OrgGrant {
            slug: "acme".into(),
            hub: "http://127.0.0.1:9".into(),
            digest: "0".repeat(64),
            authorization: None,
            me: Some(name.into()),
            mirror: dir.join(format!("{name}.json")),
            clock_skew_ms: skew,
            app_id: "dev.acme.tasks".into(),
            device_secret: DeviceSecret(*krate_orgkeys::KeyPair::generate().secret()),
            device_name: format!("{name}'s laptop"),
            org_name: "Acme".into(),
        })
    }

    fn text(v: &str) -> Value {
        Value::Text(v.into())
    }

    /// The hub, in miniature: the same rule, applied to what two devices
    /// push. The real one is cloud/worker/src/org-data.js; both pass the
    /// shared vectors.
    /// A stored row: wire value, stamp, writer, seq.
    type MiniRow = (serde_json::Value, String, String, u64);

    #[derive(Default)]
    struct MiniHub {
        rows: BTreeMap<(String, String, String), MiniRow>,
        seq: u64,
    }

    impl MiniHub {
        fn take(&mut self, round: &Round, by: &str, hub_now: u64) -> serde_json::Value {
            let device_now = round.sent_now;
            let since: u64 = round
                .since
                .as_deref()
                .map(|s| s.parse().unwrap())
                .unwrap_or(0);
            let mut lost = Vec::new();
            for w in &round.writes {
                let key = (
                    w["c"].as_str().unwrap().to_string(),
                    w["i"].as_str().unwrap().to_string(),
                    w["f"].as_str().unwrap().to_string(),
                );
                let s = org_merge::rebase(
                    &Stamp::parse(w["s"].as_str().unwrap()).unwrap(),
                    device_now,
                    hub_now,
                );
                let held = self.rows.get(&key).map(|r| Stamp::parse(&r.1).unwrap());
                if org_merge::wins(&s, held.as_ref()) {
                    self.seq += 1;
                    self.rows.insert(
                        key,
                        (w["v"].clone(), s.to_string(), by.to_string(), self.seq),
                    );
                } else {
                    lost.push(key);
                }
            }
            let mut rows: Vec<_> = self
                .rows
                .iter()
                .filter(|(_, r)| r.3 > since)
                .map(|(k, r)| (r.3, k, r))
                .collect();
            rows.sort_by_key(|r| r.0);
            let mut out: Vec<serde_json::Value> = rows
                .iter()
                .map(|(_, k, r)| serde_json::json!({ "c": k.0, "i": k.1, "f": k.2, "v": r.0, "s": r.1, "by": r.2 }))
                .collect();
            for k in lost {
                let r = &self.rows[&k];
                out.push(serde_json::json!({ "c": k.0, "i": k.1, "f": k.2, "v": r.0, "s": r.1, "by": r.2 }));
            }
            serde_json::json!({
                "now": hub_now, "rows": out, "cursor": self.seq.to_string(), "more": false,
                "full": false, "me": { "name": by, "role": "developer" }, "org": { "slug": "acme", "name": "Acme" },
                "quota": { "limit": DEFAULT_QUOTA },
            })
        }
    }

    fn sync_with(org: &AppOrg, hub: &mut MiniHub, by: &str, hub_now: u64) {
        let inner = org.access.as_ref().unwrap();
        let mut inner = inner.lock().unwrap();
        let round = prepare(&inner);
        let answer = hub.take(&round, by, hub_now);
        apply(&mut inner.mirror, &round, &answer);
    }

    fn real_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    #[test]
    fn local_writes_read_back_survive_a_reopen_and_deletes_hide() {
        let dir = tempfile::tempdir().unwrap();
        let g = grant(dir.path(), "carol", 0);
        {
            let org = AppOrg::open(OrgAccess::Granted(g.clone()));
            org.put(
                "tasks",
                "t1",
                vec![
                    ("title".into(), text("Order chairs")),
                    ("n".into(), Value::Number(3.0)),
                ],
            )
            .unwrap();
            org.put("tasks", "t1", vec![("n".into(), Value::Null)])
                .unwrap();
            let got = org.get("tasks", "t1").unwrap().unwrap();
            assert_eq!(
                got.fields,
                vec![("title".to_string(), text("Order chairs"))]
            );
            assert_eq!(got.changed_by, "carol");
            org.put("tasks", "t2", vec![("title".into(), text("Book room"))])
                .unwrap();
            org.delete("tasks", "t2").unwrap();
            assert_eq!(org.get("tasks", "t2").unwrap(), None);
        }
        let org = AppOrg::open(OrgAccess::Granted(g));
        assert_eq!(
            org.get("tasks", "t1").unwrap().unwrap().fields.len(),
            1,
            "kept on disk"
        );
        let page = org.records("tasks", None).unwrap();
        assert_eq!(
            page.entries
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec!["t1"]
        );
        assert!(
            page.removed.is_empty(),
            "from the start, removed records are just absent"
        );
        // Three waiting to go up: t1's title, the removal of t1's n, and t2's
        // tombstone (its title went with it).
        assert_eq!(org.inner().unwrap().mirror.pending(), 3);
    }

    #[test]
    fn records_pages_and_cursors() {
        let dir = tempfile::tempdir().unwrap();
        let org = AppOrg::open(OrgAccess::Granted(grant(dir.path(), "carol", 0)));
        for n in 0..250 {
            org.put(
                "items",
                &format!("i{n:03}"),
                vec![("n".into(), Value::Number(n as f64))],
            )
            .unwrap();
        }
        let first = org.records("items", None).unwrap();
        assert_eq!((first.entries.len(), first.more), (200, true));
        let second = org.records("items", Some(&first.next)).unwrap();
        assert_eq!((second.entries.len(), second.more), (50, false));
        // Later, only what changed after the cursor: one edit and one delete.
        org.put("items", "i007", vec![("n".into(), Value::Number(70.0))])
            .unwrap();
        org.delete("items", "i008").unwrap();
        let changes = org.records("items", Some(&second.next)).unwrap();
        assert_eq!(
            changes
                .entries
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            vec!["i007"]
        );
        assert_eq!(changes.removed, vec!["i008".to_string()]);
        assert!(matches!(
            org.records("items", Some("nope")),
            Err(OrgError::Invalid(_))
        ));
    }

    #[test]
    fn limits_are_enforced_on_the_device() {
        let dir = tempfile::tempdir().unwrap();
        let org = AppOrg::open(OrgAccess::Granted(grant(dir.path(), "carol", 0)));
        assert!(matches!(
            org.put("", "t1", vec![("a".into(), text("x"))]),
            Err(OrgError::Invalid(_))
        ));
        assert!(matches!(
            org.put(&"c".repeat(129), "t1", vec![("a".into(), text("x"))]),
            Err(OrgError::Invalid(_))
        ));
        assert!(matches!(
            org.put("c", &"i".repeat(129), vec![("a".into(), text("x"))]),
            Err(OrgError::Invalid(_))
        ));
        assert!(org
            .put(
                &"c".repeat(128),
                &"i".repeat(128),
                vec![("a".into(), text("x"))]
            )
            .is_ok());
        assert!(matches!(
            org.put("c", "t1", vec![("".into(), text("x"))]),
            Err(OrgError::Invalid(_))
        ));
        assert!(matches!(
            org.put("c", "t1", vec![("a\u{7}".into(), text("x"))]),
            Err(OrgError::Invalid(_))
        ));
        assert!(matches!(
            org.put("c", "t1", vec![("a".into(), Value::Number(f64::NAN))]),
            Err(OrgError::Invalid(_))
        ));
        let big = "y".repeat(40 * 1024);
        org.put("c", "big", vec![("a".into(), text(&big))]).unwrap();
        assert_eq!(
            org.put("c", "big", vec![("b".into(), text(&big))]),
            Err(OrgError::TooLarge)
        );
        // The quota the hub announced.
        org.inner().unwrap().mirror.quota = Some(60 * 1024);
        assert_eq!(
            org.put("c", "more", vec![("a".into(), text(&big))]),
            Err(OrgError::OverQuota)
        );
        // Nothing of a refused write was kept.
        assert_eq!(org.get("c", "more").unwrap(), None);
        assert_eq!(org.get("c", "big").unwrap().unwrap().fields.len(), 1);
    }

    #[test]
    fn not_an_org_app_and_a_refusal_are_said() {
        let org = AppOrg::open(OrgAccess::NotAnOrgApp);
        assert_eq!(org.get("c", "i"), Err(OrgError::NotAnOrgApp));
        assert_eq!(org.sync(), Err(OrgError::NotAnOrgApp));
        let dir = tempfile::tempdir().unwrap();
        let org = AppOrg::open(OrgAccess::Granted(grant(dir.path(), "carol", 0)));
        org.inner().unwrap().refused = Some(("removed".into(), "you were removed".into()));
        assert!(matches!(org.get("c", "i"), Err(OrgError::Refused { .. })));
        assert!(matches!(
            org.put("c", "i", vec![("a".into(), text("x"))]),
            Err(OrgError::Refused { .. })
        ));
    }

    #[test]
    fn offline_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        // Port 9 (discard) on localhost: nothing listens.
        let org = AppOrg::open(OrgAccess::Granted(grant(dir.path(), "carol", 0)));
        org.put("c", "i", vec![("a".into(), text("x"))]).unwrap();
        let state = org.inner().map(|_| ()).and_then(|_| {
            let inner = org.access.clone().unwrap();
            sync_now(&inner, &org.sync_lock)
        });
        let state = state.expect("offline answers, not fails");
        assert!(!state.online);
        assert_eq!(state.pending, 1, "the write waits");
    }

    /// Two devices, one record, edited offline in different fields: after
    /// both sync, both edits are there on both.
    #[test]
    fn two_devices_offline_different_fields_both_survive() {
        let dir = tempfile::tempdir().unwrap();
        let carol = AppOrg::open(OrgAccess::Granted(grant(dir.path(), "carol", 0)));
        let dan = AppOrg::open(OrgAccess::Granted(grant(dir.path(), "dan", 0)));
        let mut hub = MiniHub::default();
        carol
            .put("tasks", "t1", vec![("title".into(), text("Order chairs"))])
            .unwrap();
        dan.put("tasks", "t1", vec![("done".into(), Value::Boolean(true))])
            .unwrap();
        let now = real_now();
        sync_with(&carol, &mut hub, "carol", now);
        sync_with(&dan, &mut hub, "dan", now + 10);
        sync_with(&carol, &mut hub, "carol", now + 20);
        for org in [&carol, &dan] {
            let entry = org.get("tasks", "t1").unwrap().unwrap();
            assert_eq!(
                entry.fields,
                vec![
                    ("done".to_string(), Value::Boolean(true)),
                    ("title".to_string(), text("Order chairs"))
                ]
            );
            assert_eq!(org.inner().unwrap().mirror.pending(), 0);
        }
        assert_eq!(carol.who().unwrap().unwrap().org, "Acme");
    }

    /// A device whose clock is a day fast edits first, offline; a device
    /// with a right clock edits later. The later edit wins on both, and the
    /// fast device learns its clock is wrong.
    #[test]
    fn a_device_a_day_fast_does_not_win_and_learns_its_clock() {
        let day = 86_400_000i64;
        let dir = tempfile::tempdir().unwrap();
        let fast = AppOrg::open(OrgAccess::Granted(grant(dir.path(), "bob", day)));
        let right = AppOrg::open(OrgAccess::Granted(grant(dir.path(), "carol", 0)));
        let mut hub = MiniHub::default();
        fast.put("tasks", "t1", vec![("title".into(), text("Bob's"))])
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        right
            .put("tasks", "t1", vec![("title".into(), text("Carol's"))])
            .unwrap();
        sync_with(&right, &mut hub, "carol", real_now());
        sync_with(&fast, &mut hub, "bob", real_now());
        sync_with(&right, &mut hub, "carol", real_now());
        for org in [&fast, &right] {
            assert_eq!(
                org.get("tasks", "t1").unwrap().unwrap().fields,
                vec![("title".to_string(), text("Carol's"))]
            );
        }
        let offset = fast.inner().unwrap().mirror.offset_ms;
        assert!(
            (offset + day).abs() < 1_000,
            "learned its clock is a day fast: {offset}"
        );
        // From now on its edits are on the hub's clock, and a new one wins.
        fast.put("tasks", "t1", vec![("title".into(), text("Bob's again"))])
            .unwrap();
        sync_with(&fast, &mut hub, "bob", real_now());
        sync_with(&right, &mut hub, "carol", real_now());
        assert_eq!(
            right.get("tasks", "t1").unwrap().unwrap().fields,
            vec![("title".to_string(), text("Bob's again"))]
        );
    }

    /// What goes up is sealed: no name, id or value of the record is in it,
    /// and what comes back opens to the same rows. A changed ciphertext,
    /// or one moved to another field, is refused and counted.
    #[test]
    fn writes_leave_sealed_and_rows_that_do_not_open_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let org = AppOrg::open(OrgAccess::Granted(grant(dir.path(), "carol", 0)));
        org.put("tasks", "t1", vec![("title".into(), text("Order chairs"))])
            .unwrap();
        org.delete("tasks", "t9").unwrap();
        let held = krate_orgkeys::agent::Held {
            keyring: krate_orgkeys::Keyring::create("acme"),
            envelope: String::new(),
        };
        let round = prepare(&org.inner().unwrap());
        let sealed = seal_writes(&round.writes, &held, "dev.acme.tasks").unwrap();
        let wire = serde_json::to_string(&sealed).unwrap();
        // Not as text, and not as the bytes inside a ciphertext either:
        // base64 of the plaintext would hide nothing.
        let mut inside = Vec::new();
        for w in &sealed {
            inside.extend(krate_orgkeys::b64_decode(w["ct"].as_str().unwrap()).unwrap());
        }
        for plain in ["Order chairs", "tasks", "title", "carol"] {
            assert!(
                !inside.windows(plain.len()).any(|x| x == plain.as_bytes()),
                "{plain} is readable inside a ciphertext"
            );
        }
        for plain in ["Order chairs", "tasks", "\"t1\"", "title", "carol"] {
            assert!(!wire.contains(plain), "{plain} left the device: {wire}");
        }
        assert_eq!(sealed[0]["kind"], "v");
        // The hub sends them back as rows: they open to what was written.
        let rows: Vec<serde_json::Value> = sealed
            .iter()
            .map(|w| {
                let mut row = w.clone();
                row["by"] = serde_json::json!("carol");
                row
            })
            .collect();
        let answer = serde_json::json!({ "rows": rows });
        let (plain, rejected) = open_rows(&answer, &held, "dev.acme.tasks");
        assert_eq!(rejected, 0);
        assert_eq!(
            plain["rows"][0]["v"],
            serde_json::json!({ "t": "Order chairs" })
        );
        assert_eq!(plain["rows"][0]["by"], "carol");
        // Tampered: one byte of the ciphertext.
        let mut bad = rows[0].clone();
        let mut bytes = krate_orgkeys::b64_decode(bad["ct"].as_str().unwrap()).unwrap();
        bytes[30] ^= 1;
        bad["ct"] = serde_json::json!(krate_orgkeys::b64_encode(&bytes));
        // Moved: the ciphertext under another field's opaque name.
        let mut moved = rows[0].clone();
        moved["k"] = serde_json::json!("f".repeat(64));
        // Another app's key cannot open them either.
        let (plain, rejected) = open_rows(
            &serde_json::json!({ "rows": [bad, moved] }),
            &held,
            "dev.acme.tasks",
        );
        assert_eq!((plain["rows"].as_array().unwrap().len(), rejected), (0, 2));
        // A member's device that seals one record's content under another
        // record's names is refused too: what opens must be what it is
        // named.
        let dk = krate_orgkeys::AppDataKey::derive(&held.keyring, 1, "dev.acme.tasks").unwrap();
        let (r, k) = (
            dk.record_key("tasks", "t1"),
            dk.field_key("tasks", "t1", "title"),
        );
        let lie = payload(
            "tasks",
            "t2",
            "title",
            &serde_json::json!({ "t": "moved" }),
            "mallory",
        );
        let liar = serde_json::json!({
            "r": r, "k": k, "kind": "v", "s": rows[0]["s"], "ver": 1, "by": "mallory",
            "ct": dk.seal(&r, &k, krate_orgkeys::RowKind::Value, lie.as_bytes()).unwrap(),
        });
        let (plain, rejected) = open_rows(
            &serde_json::json!({ "rows": [liar] }),
            &held,
            "dev.acme.tasks",
        );
        assert_eq!((plain["rows"].as_array().unwrap().len(), rejected), (0, 1));
        let (_, rejected) = open_rows(&answer, &held, "dev.acme.other");
        assert_eq!(rejected, rows.len() as u64, "another app's key opens none");
    }

    #[test]
    fn a_write_made_while_a_sync_was_away_stays_pending() {
        let dir = tempfile::tempdir().unwrap();
        let org = AppOrg::open(OrgAccess::Granted(grant(dir.path(), "carol", 0)));
        let mut hub = MiniHub::default();
        org.put("tasks", "t1", vec![("title".into(), text("one"))])
            .unwrap();
        let round = prepare(&org.inner().unwrap());
        // Written again before the answer came back.
        org.put("tasks", "t1", vec![("title".into(), text("two"))])
            .unwrap();
        let answer = hub.take(&round, "carol", real_now());
        apply(&mut org.inner().unwrap().mirror, &round, &answer);
        assert_eq!(
            org.inner().unwrap().mirror.pending(),
            1,
            "the second write still goes up"
        );
        assert_eq!(
            org.get("tasks", "t1").unwrap().unwrap().fields[0].1,
            text("two")
        );
        sync_with(&org, &mut hub, "carol", real_now());
        assert_eq!(org.inner().unwrap().mirror.pending(), 0);
        assert_eq!(
            hub.rows[&("tasks".into(), "t1".into(), "title".into())].0,
            serde_json::json!({ "t": "two" })
        );
    }

    /// A hub that takes the connection and never answers: the slow train.
    fn hanging_hub() -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut held = Vec::new();
            listener.set_nonblocking(true).unwrap();
            let until = std::time::Instant::now() + std::time::Duration::from_secs(40);
            while std::time::Instant::now() < until {
                if let Ok((stream, _)) = listener.accept() {
                    held.push(stream);
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        (url, handle)
    }

    /// K-1107: a windowed app's `sync` and `status` answer at once while
    /// the hub takes the connection and never answers, and closing the app
    /// is held up for at most CLOSE_LIMIT. Before, `sync` waited behind the
    /// background sync and then on the network itself: 30 s measured here.
    #[test]
    fn a_windowed_sync_never_waits_on_a_hub_that_never_answers() {
        let dir = tempfile::tempdir().unwrap();
        let (url, _hub) = hanging_hub();
        let mut g = grant(dir.path(), "carol", 0);
        g.hub = url;
        let org = AppOrg::open(OrgAccess::Granted(g));
        org.set_windowed();
        org.put("c", "i", vec![("a".into(), text("x"))]).unwrap();
        // Let the background sync get stuck on the network first.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let started = std::time::Instant::now();
        let state = org.sync().expect("answers");
        let status = org.status().expect("answers");
        let took = started.elapsed();
        eprintln!("MEASURE sync()+status() held the caller for {took:?}");
        assert!(
            took < std::time::Duration::from_millis(250),
            "sync and status waited {took:?} on the network"
        );
        assert_eq!(state.pending, 1, "the write waits here");
        assert_eq!(status.link, Link::Syncing, "a sync is under way");
        assert_eq!(status.org, "Acme");
        let closing = std::time::Instant::now();
        drop(org);
        let closed = closing.elapsed();
        eprintln!("MEASURE closing waited {closed:?}");
        assert!(
            closed < CLOSE_LIMIT + std::time::Duration::from_secs(1),
            "closing waited {closed:?}"
        );
    }

    /// K-1108: a device the organisation knows and has not given the key
    /// says so, and an unreachable hub says offline; neither is an error.
    #[test]
    fn status_says_waiting_for_the_key_and_offline() {
        let dir = tempfile::tempdir().unwrap();
        let org = AppOrg::open_quiet(OrgAccess::Granted(grant(dir.path(), "carol", 0)));
        org.set_windowed();
        assert_eq!(org.status().unwrap().link, Link::Syncing, "not reached yet");
        {
            let mut inner = org.inner().unwrap();
            inner.tried = true;
            inner.last_online = true;
            inner.key = KeyHold::Waiting;
        }
        let status = org.status().unwrap();
        assert_eq!(status.link, Link::WaitingForKey);
        assert_eq!(status.org, "Acme");
        org.inner().unwrap().last_online = false;
        assert_eq!(org.status().unwrap().link, Link::Offline);
        {
            let mut inner = org.inner().unwrap();
            inner.last_online = true;
            inner.key = KeyHold::Holds;
        }
        assert_eq!(org.status().unwrap().link, Link::Synced);
        org.put("c", "i", vec![("a".into(), text("x"))]).unwrap();
        assert_eq!(
            org.status().unwrap().link,
            Link::Syncing,
            "a write is on its way"
        );
    }
}
