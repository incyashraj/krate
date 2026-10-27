//! The app's shared store: a key-value bucket shared between the machines
//! that hold its invite code.
//!
//! This is how a generated app becomes a household app -- a shopping list two
//! people see, a meal plan a family edits -- without the app author running a
//! backend and without anyone creating an account. Possession of the
//! ten-character code IS the membership, like a shared album link, and the
//! consent wording says so plainly.
//!
//! Local-first, deliberately: `get`/`set`/`delete` always work against the
//! JSON mirror on this machine, and `sync` exchanges changes with the hub
//! when the network allows. Merging is last-writer-wins per key by the
//! writer's clock, with tombstones for deletes -- for a household list the
//! worst concurrent-edit outcome is one item re-added by hand, which is the
//! right price for having no accounts and no locks. Offline is a normal
//! state, never an error.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Mirror of the WIT `shared-error` variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedError {
    Denied,
    NotJoined,
    NoSuchShare,
    InvalidName,
    TooLarge,
    Io(String),
    /// This host cannot reach the sharing hub at all -- a browser preview,
    /// where the browser's own origin rules forbid one app reading
    /// another's storage. Distinct from `Io`, which means the network was
    /// tried and failed, so the words a person reads can say which.
    Unsupported,
}

/// Bounds matching the hub's: a list, not a database.
const MAX_KEY: usize = 128;
const MAX_VALUE: usize = 64 * 1024;
/// The least time between network syncs; faster callers get `false` back
/// from the local answer instead of hammering the hub.
const SYNC_FLOOR: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct State {
    /// The invite code, once created or joined.
    code: Option<String>,
    /// Every key ever seen, tombstones included (`v: None`).
    kv: BTreeMap<String, Entry>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct Entry {
    /// Base64 value, `None` for a tombstone.
    v: Option<String>,
    /// The writer's clock, milliseconds since the epoch. Newest wins.
    t: u64,
    /// Written locally and not yet pushed.
    #[serde(default)]
    dirty: bool,
}

/// The mirror and what the last exchange left for the app to hear.
struct Core {
    path: PathBuf,
    hub: String,
    state: State,
    last_sync: Option<std::time::Instant>,
    /// A background exchange changed the local copy and the app's `sync`
    /// has not said so yet.
    unseen_change: bool,
    /// A background exchange was refused for a reason the app must hear
    /// (the share is gone); the next `sync` answers with it once.
    unseen_error: Option<SharedError>,
}

/// What one exchange sends: the share and the writes as they stood.
struct Outgoing {
    url: String,
    writes: Vec<(String, Option<String>, u64)>,
}

/// One app's shared store for this run.
///
/// **Never in the window's way (K-1121).** A windowed app's code runs where
/// its window is drawn, so in a windowed app `sync` never waits for the hub:
/// it starts the exchange on a thread of its own and answers at once with
/// whether an earlier exchange changed the local copy. A command-line app
/// has no window to freeze, so its `sync` still waits, as a script that
/// syncs and exits expects. Either way the mirror's lock is never held
/// across the network.
pub struct AppShared {
    core: Arc<Mutex<Core>>,
    /// Whether `sync` waits for the exchange: a command-line app's does,
    /// a windowed app's never does.
    waits: AtomicBool,
    /// An exchange is under way on its own thread.
    in_flight: Arc<AtomicBool>,
}

impl AppShared {
    /// Open (or start) the mirror at `path`, syncing against `hub`.
    pub fn open(path: PathBuf, hub: String) -> Self {
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            core: Arc::new(Mutex::new(Core {
                path,
                hub,
                state,
                last_sync: None,
                unseen_change: false,
                unseen_error: None,
            })),
            waits: AtomicBool::new(true),
            in_flight: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A windowed app: `sync` never waits for the network from here on.
    pub fn set_windowed(&self) {
        self.waits.store(false, Ordering::SeqCst);
    }

    fn core(&self) -> Result<std::sync::MutexGuard<'_, Core>, SharedError> {
        lock(&self.core)
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn check_key(key: &str) -> Result<(), SharedError> {
        if key.is_empty() || key.len() > MAX_KEY || key.chars().any(char::is_control) {
            return Err(SharedError::InvalidName);
        }
        Ok(())
    }

    pub fn code(&self) -> Option<String> {
        self.core().ok().and_then(|core| core.state.code.clone())
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SharedError> {
        Self::check_key(key)?;
        Ok(self
            .core()?
            .state
            .kv
            .get(key)
            .and_then(|entry| entry.v.as_deref())
            .and_then(b64_decode))
    }

    pub fn set(&mut self, key: &str, value: Vec<u8>) -> Result<(), SharedError> {
        Self::check_key(key)?;
        if value.len() > MAX_VALUE {
            return Err(SharedError::TooLarge);
        }
        let mut core = self.core()?;
        core.state.kv.insert(
            key.to_string(),
            Entry {
                v: Some(b64_encode(&value)),
                t: Self::now_ms(),
                dirty: true,
            },
        );
        core.save()
    }

    pub fn delete(&mut self, key: &str) -> Result<(), SharedError> {
        Self::check_key(key)?;
        let mut core = self.core()?;
        // A tombstone, not an absence: without one, the other machine's next
        // push would resurrect every item ever removed.
        core.state.kv.insert(
            key.to_string(),
            Entry {
                v: None,
                t: Self::now_ms(),
                dirty: true,
            },
        );
        core.save()
    }

    pub fn keys(&self) -> Vec<String> {
        let Ok(core) = self.core() else {
            return Vec::new();
        };
        core.state
            .kv
            .iter()
            .filter(|(_, entry)| entry.v.is_some())
            .map(|(key, _)| key.clone())
            .collect()
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn create(&mut self) -> Result<String, SharedError> {
        let hub = self.core()?.hub.clone();
        let text = krate_http::post(&format!("{hub}/share/new"))
            .timeout(std::time::Duration::from_secs(8))
            .call()
            .map_err(|error| SharedError::Io(error.to_string()))?
            .into_string()
            .map_err(|error| SharedError::Io(error.to_string()))?;
        let body: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| SharedError::Io(error.to_string()))?;
        let code = body["code"]
            .as_str()
            .ok_or_else(|| SharedError::Io("the hub returned no code".to_string()))?
            .to_string();
        {
            let mut core = self.core()?;
            core.state.code = Some(code.clone());
            core.save()?;
        }
        // Whatever was written before the share existed rides up with the
        // first sync -- create-then-fill and fill-then-create both work.
        self.kick();
        Ok(code)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn join(&mut self, code: &str) -> Result<(), SharedError> {
        let code = code.trim().to_ascii_lowercase();
        if !code.chars().all(|c| c.is_ascii_alphanumeric()) || code.len() != 10 {
            return Err(SharedError::NoSuchShare);
        }
        let hub = self.core()?.hub.clone();
        // Prove it exists before remembering it, so a typo answers now
        // rather than as eternal silent sync failure.
        let response = krate_http::get(&format!("{hub}/share/{code}"))
            .timeout(std::time::Duration::from_secs(8))
            .call();
        match response {
            Ok(response) => {
                let text = response
                    .into_string()
                    .map_err(|error| SharedError::Io(error.to_string()))?;
                let remote: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|error| SharedError::Io(error.to_string()))?;
                {
                    let mut core = self.core()?;
                    core.state.code = Some(code);
                    core.merge_remote(remote);
                    core.save()?;
                }
                self.kick();
                Ok(())
            }
            Err(ureq::Error::Status(404, _)) => Err(SharedError::NoSuchShare),
            Err(error) => Err(SharedError::Io(error.to_string())),
        }
    }

    pub fn leave(&mut self) -> Result<(), SharedError> {
        let mut core = self.core()?;
        core.state.code = None;
        core.save()
    }

    /// Exchange changes with the hub. Returns `true` when the LOCAL copy
    /// changed. Offline returns `false`: the queue keeps waiting.
    ///
    /// In a windowed app the exchange runs on its own thread and `true`
    /// means an earlier exchange changed the copy since the app last asked.
    pub fn sync(&mut self) -> Result<bool, SharedError> {
        {
            let mut core = self.core()?;
            if core.state.code.is_none() {
                return Err(SharedError::NotJoined);
            }
            if !self.waits.load(Ordering::SeqCst) {
                if let Some(error) = core.unseen_error.take() {
                    return Err(error);
                }
                let changed = std::mem::take(&mut core.unseen_change);
                let floor = core.last_sync.is_some_and(|at| at.elapsed() < SYNC_FLOOR);
                drop(core);
                if !floor {
                    self.start_exchange();
                }
                return Ok(changed);
            }
            if core.last_sync.is_some_and(|at| at.elapsed() < SYNC_FLOOR) {
                return Ok(false);
            }
        }
        match exchange(&self.core) {
            Ok(changed) => Ok(changed),
            // The network being away is a normal state for a laptop.
            Err(SharedError::Io(_)) => Ok(false),
            Err(other) => Err(other),
        }
    }

    /// One exchange now: on its own thread in a windowed app, here
    /// otherwise. Its answer reaches the app through the next `sync`.
    #[cfg(not(target_arch = "wasm32"))]
    fn kick(&self) {
        if self.waits.load(Ordering::SeqCst) {
            if let Ok(true) = exchange(&self.core) {
                if let Ok(mut core) = self.core() {
                    core.unseen_change = true;
                }
            }
        } else {
            self.start_exchange();
        }
    }

    /// Start a background exchange unless one is already under way.
    fn start_exchange(&self) {
        if self.in_flight.swap(true, Ordering::SeqCst) {
            return;
        }
        let (core, in_flight) = (self.core.clone(), self.in_flight.clone());
        let started = std::thread::Builder::new()
            .name("krate-shared-sync".into())
            .spawn(move || {
                let outcome = exchange(&core);
                if let Ok(mut core) = lock(&core) {
                    match outcome {
                        Ok(true) => core.unseen_change = true,
                        Ok(false) | Err(SharedError::Io(_)) => {}
                        Err(other) => core.unseen_error = Some(other),
                    }
                }
                in_flight.store(false, Ordering::SeqCst);
            });
        if started.is_err() {
            self.in_flight.store(false, Ordering::SeqCst);
        }
    }

    // The browser's answers. Everything local -- get, set, delete, keys --
    // works exactly as it does anywhere; only reaching the hub is out of
    // reach, because a tab is bound by the browser's origin rules and one
    // app reading another's storage is precisely what those forbid.
    //
    // Refusing beats a silent no-op: an app whose share never syncs, with
    // nothing said, looks broken to the person and to whoever they shared
    // with.
    #[cfg(target_arch = "wasm32")]
    pub fn create(&mut self) -> Result<String, SharedError> {
        Err(SharedError::Unsupported)
    }

    #[cfg(target_arch = "wasm32")]
    pub fn join(&mut self, _code: &str) -> Result<(), SharedError> {
        Err(SharedError::Unsupported)
    }
}

fn lock(core: &Arc<Mutex<Core>>) -> Result<std::sync::MutexGuard<'_, Core>, SharedError> {
    core.lock()
        .map_err(|_| SharedError::Io("the shared store's lock was poisoned".to_string()))
}

#[cfg(target_arch = "wasm32")]
fn exchange(_core: &Arc<Mutex<Core>>) -> Result<bool, SharedError> {
    Err(SharedError::Unsupported)
}

/// One push-and-pull with the hub. The lock is held to read what to send
/// and to fold in the answer, never across the network.
#[cfg(not(target_arch = "wasm32"))]
fn exchange(core: &Arc<Mutex<Core>>) -> Result<bool, SharedError> {
    let outgoing = lock(core)?.outgoing()?;
    let response = if outgoing.writes.is_empty() {
        krate_http::get(&outgoing.url)
            .timeout(std::time::Duration::from_secs(8))
            .call()
    } else {
        let writes: Vec<serde_json::Value> = outgoing
            .writes
            .iter()
            .map(|(key, v, t)| serde_json::json!({ "key": key, "v": v, "t": t }))
            .collect();
        let payload = serde_json::json!({ "writes": writes }).to_string();
        krate_http::request("PUT", &outgoing.url)
            .timeout(std::time::Duration::from_secs(8))
            .set("content-type", "application/json")
            .send_string(&payload)
    };
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(404, _)) => return Err(SharedError::NoSuchShare),
        Err(error) => return Err(SharedError::Io(error.to_string())),
    };
    let text = response
        .into_string()
        .map_err(|error| SharedError::Io(error.to_string()))?;
    let remote: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| SharedError::Io(error.to_string()))?;
    lock(core)?.finish(&outgoing, remote)
}

impl Core {
    fn save(&self) -> Result<(), SharedError> {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = serde_json::to_string(&self.state)
            .map_err(|error| SharedError::Io(error.to_string()))?;
        std::fs::write(&self.path, text).map_err(|error| SharedError::Io(error.to_string()))
    }

    /// What the next exchange sends, as the mirror stands now.
    fn outgoing(&mut self) -> Result<Outgoing, SharedError> {
        let Some(code) = self.state.code.clone() else {
            return Err(SharedError::NotJoined);
        };
        self.last_sync = Some(std::time::Instant::now());
        let writes = self
            .state
            .kv
            .iter()
            .filter(|(_, entry)| entry.dirty)
            .map(|(key, entry)| (key.clone(), entry.v.clone(), entry.t))
            .collect();
        Ok(Outgoing {
            url: format!("{}/share/{}", self.hub, code),
            writes,
        })
    }

    /// Fold in the hub's answer to `sent`. Only the writes that went up are
    /// clean now: one the app made while the exchange was on the network is
    /// newer than what was sent, and waits for the next push.
    fn finish(&mut self, sent: &Outgoing, remote: serde_json::Value) -> Result<bool, SharedError> {
        for (key, _, t) in &sent.writes {
            if let Some(entry) = self.state.kv.get_mut(key) {
                if entry.t == *t {
                    entry.dirty = false;
                }
            }
        }
        let changed = self.merge_remote(remote);
        self.save()?;
        Ok(changed)
    }

    /// Fold the hub's bucket into the mirror. Newest write per key wins;
    /// local dirty entries with newer clocks survive to be pushed next time.
    fn merge_remote(&mut self, remote: serde_json::Value) -> bool {
        let Some(kv) = remote.get("kv").and_then(|kv| kv.as_object()) else {
            return false;
        };
        let mut changed = false;
        for (key, value) in kv {
            let t = value.get("t").and_then(|t| t.as_u64()).unwrap_or(0);
            let v = value.get("v").and_then(|v| v.as_str()).map(str::to_string);
            match self.state.kv.get(key) {
                Some(existing) if existing.t >= t => {}
                _ => {
                    self.state
                        .kv
                        .insert(key.clone(), Entry { v, t, dirty: false });
                    changed = true;
                }
            }
        }
        changed
    }
}

/* ---- a small base64, so the mirror and the wire share one encoding ------ */

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn b64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

pub(crate) fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let clean: Vec<u8> = text.bytes().filter(|&c| c != b'=' && c != b'\n').collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut n: u32 = 0;
        for (i, &c) in chunk.iter().enumerate() {
            n |= value(c)? << (18 - 6 * i as u32);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for case in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar"] {
            assert_eq!(
                b64_decode(&b64_encode(case)).as_deref(),
                Some(case),
                "{case:?}"
            );
        }
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn local_writes_read_back_and_tombstones_hide() {
        let dir = std::env::temp_dir().join(format!("krate-shared-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut shared = AppShared::open(dir.join("shared.json"), "http://invalid".to_string());
        shared.set("milk", b"2".to_vec()).expect("set");
        assert_eq!(shared.get("milk").expect("get"), Some(b"2".to_vec()));
        assert_eq!(shared.keys(), vec!["milk".to_string()]);
        shared.delete("milk").expect("delete");
        assert_eq!(shared.get("milk").expect("get"), None);
        assert!(shared.keys().is_empty(), "tombstone hides the key");
        // The mirror survives a reopen.
        let reopened = AppShared::open(dir.join("shared.json"), "http://invalid".to_string());
        assert_eq!(reopened.get("milk").expect("get"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_prefers_the_newest_write() {
        let dir = std::env::temp_dir().join(format!("krate-shared-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut shared = AppShared::open(dir.join("shared.json"), "http://invalid".to_string());
        shared.set("bread", b"old".to_vec()).expect("set");
        let newer = AppShared::now_ms() + 10_000;
        let changed = shared.core().unwrap().merge_remote(serde_json::json!({
            "kv": {
                "bread": { "v": b64_encode(b"new"), "t": newer },
                "jam": { "v": b64_encode(b"1"), "t": 5 },
            }
        }));
        assert!(changed);
        assert_eq!(shared.get("bread").expect("get"), Some(b"new".to_vec()));
        assert_eq!(shared.get("jam").expect("get"), Some(b"1".to_vec()));
        // An older remote write never beats a newer local one.
        let changed = shared.core().unwrap().merge_remote(serde_json::json!({
            "kv": { "bread": { "v": b64_encode(b"stale"), "t": 1 } }
        }));
        assert!(!changed);
        assert_eq!(shared.get("bread").expect("get"), Some(b"new".to_vec()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A hub on localhost: `answer` decides each response body, `None`
    /// takes the connection and never answers.
    fn local_hub(
        answer: Option<&'static str>,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let served = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = served.clone();
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                count.fetch_add(1, Ordering::SeqCst);
                match answer {
                    None => held.push(stream),
                    Some(body) => {
                        let mut buf = [0u8; 8192];
                        let _ = stream.read(&mut buf);
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                    }
                }
            }
        });
        (url, served)
    }

    fn joined(dir: &std::path::Path, hub: String) -> AppShared {
        let shared = AppShared::open(dir.join("shared.json"), hub);
        shared.core().unwrap().state.code = Some("abcde12345".to_string());
        shared
    }

    /// K-1121: a windowed app's `sync` answers at once while the hub takes
    /// the connection and never answers. Before, it waited out the 8 s
    /// timeout on the app's own thread, which is the window's.
    #[test]
    fn a_windowed_sync_never_waits_on_a_hub_that_never_answers() {
        let dir = tempfile::tempdir().unwrap();
        let (url, served) = local_hub(None);
        let mut shared = joined(dir.path(), url);
        shared.set_windowed();
        shared.set("milk", b"2".to_vec()).unwrap();
        let started = std::time::Instant::now();
        assert_eq!(shared.sync(), Ok(false));
        // Again while the first exchange hangs: still at once, and no
        // second exchange piles up behind it.
        std::thread::sleep(std::time::Duration::from_millis(100));
        shared.core().unwrap().last_sync = None;
        assert_eq!(shared.sync(), Ok(false));
        let took = started.elapsed();
        eprintln!("MEASURE two windowed syncs held the caller for {took:?}");
        assert!(
            took < std::time::Duration::from_millis(400),
            "sync waited {took:?} on the network"
        );
        assert_eq!(served.load(Ordering::SeqCst), 1, "one exchange at a time");
        assert_eq!(shared.get("milk").unwrap(), Some(b"2".to_vec()));
    }

    /// What a background exchange brought down reaches the app through its
    /// next `sync`, as `true`, once.
    #[test]
    fn a_windowed_sync_reports_what_the_background_exchange_changed() {
        let dir = tempfile::tempdir().unwrap();
        let (url, _) = local_hub(Some(r#"{"kv":{"eggs":{"v":"MTI=","t":99999999999999}}}"#));
        let mut shared = joined(dir.path(), url);
        shared.set_windowed();
        assert_eq!(shared.sync(), Ok(false), "nothing heard yet");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while shared.in_flight.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(shared.get("eggs").unwrap(), Some(b"12".to_vec()));
        shared.core().unwrap().last_sync = None;
        assert_eq!(shared.sync(), Ok(true), "the change is reported");
        shared.core().unwrap().last_sync = Some(std::time::Instant::now());
        assert_eq!(shared.sync(), Ok(false), "and only once");
    }

    /// A write the app makes while an exchange is on the network is newer
    /// than what went up, so it stays queued for the next push. Clearing
    /// every dirty mark after a push (the old code, safe only while the
    /// app's own thread did the exchange) would lose it.
    #[test]
    fn a_write_during_an_exchange_waits_for_the_next_push() {
        let dir = tempfile::tempdir().unwrap();
        let mut shared = joined(dir.path(), "http://invalid".to_string());
        shared.set("milk", b"1".to_vec()).unwrap();
        let sent = shared.core().unwrap().outgoing().unwrap();
        assert_eq!(sent.writes.len(), 1);
        // The app writes again while the exchange is away.
        std::thread::sleep(std::time::Duration::from_millis(2));
        shared.set("milk", b"2".to_vec()).unwrap();
        shared.set("jam", b"1".to_vec()).unwrap();
        shared
            .core()
            .unwrap()
            .finish(&sent, serde_json::json!({ "kv": {} }))
            .unwrap();
        let again = shared.core().unwrap().outgoing().unwrap();
        let keys: Vec<&str> = again.writes.iter().map(|(k, _, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["jam", "milk"], "both later writes still go up");
        assert_eq!(shared.get("milk").unwrap(), Some(b"2".to_vec()));
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Two machines, one list: the whole feature end to end against the real
    /// hub. Opt-in (KRATE_SHARED_LIVE_TEST=1) because it needs the network
    /// and writes a throwaway share; run it by hand before shipping.
    #[test]
    fn two_mirrors_share_one_bucket_through_the_hub() {
        if std::env::var("KRATE_SHARED_LIVE_TEST").as_deref() != Ok("1") {
            return;
        }
        let hub =
            std::env::var("KRATE_HUB_URL").unwrap_or_else(|_| "https://hub.krate.tech".to_string());
        let dir = std::env::temp_dir().join(format!("krate-shared-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // Machine A creates the share and writes.
        let mut a = AppShared::open(dir.join("a.json"), hub.clone());
        let code = a.create().expect("create");
        a.set("milk", b"2".to_vec()).expect("set milk");
        a.set("eggs", b"12".to_vec()).expect("set eggs");
        exchange(&a.core).expect("push a");

        // Machine B joins by the code and sees both.
        let mut b = AppShared::open(dir.join("b.json"), hub.clone());
        b.join(&code).expect("join");
        assert_eq!(b.get("milk").expect("get"), Some(b"2".to_vec()));
        assert_eq!(b.get("eggs").expect("get"), Some(b"12".to_vec()));

        // B deletes; A sees it gone after a sync.
        b.delete("milk").expect("delete");
        exchange(&b.core).expect("push b");
        exchange(&a.core).expect("pull a");
        assert_eq!(a.get("milk").expect("get"), None, "delete crossed machines");
        assert_eq!(a.get("eggs").expect("get"), Some(b"12".to_vec()));

        // A typo'd code answers now, not as silent failure.
        let mut c = AppShared::open(dir.join("c.json"), hub);
        assert_eq!(
            c.join("zzzzzzzz99").expect_err("bad code"),
            SharedError::NoSuchShare
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
