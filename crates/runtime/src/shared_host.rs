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

/// Mirror of the WIT `shared-error` variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedError {
    Denied,
    NotJoined,
    NoSuchShare,
    InvalidName,
    TooLarge,
    Io(String),
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

pub struct AppShared {
    path: PathBuf,
    hub: String,
    state: State,
    last_sync: Option<std::time::Instant>,
}

impl AppShared {
    /// Open (or start) the mirror at `path`, syncing against `hub`.
    pub fn open(path: PathBuf, hub: String) -> Self {
        let state = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            hub,
            state,
            last_sync: None,
        }
    }

    fn save(&self) -> Result<(), SharedError> {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = serde_json::to_string(&self.state)
            .map_err(|error| SharedError::Io(error.to_string()))?;
        std::fs::write(&self.path, text).map_err(|error| SharedError::Io(error.to_string()))
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
        self.state.code.clone()
    }

    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SharedError> {
        Self::check_key(key)?;
        Ok(self
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
        self.state.kv.insert(
            key.to_string(),
            Entry {
                v: Some(b64_encode(&value)),
                t: Self::now_ms(),
                dirty: true,
            },
        );
        self.save()
    }

    pub fn delete(&mut self, key: &str) -> Result<(), SharedError> {
        Self::check_key(key)?;
        // A tombstone, not an absence: without one, the other machine's next
        // push would resurrect every item ever removed.
        self.state.kv.insert(
            key.to_string(),
            Entry {
                v: None,
                t: Self::now_ms(),
                dirty: true,
            },
        );
        self.save()
    }

    pub fn keys(&self) -> Vec<String> {
        self.state
            .kv
            .iter()
            .filter(|(_, entry)| entry.v.is_some())
            .map(|(key, _)| key.clone())
            .collect()
    }

    pub fn create(&mut self) -> Result<String, SharedError> {
        let text = ureq::post(&format!("{}/share/new", self.hub))
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
        self.state.code = Some(code.clone());
        self.save()?;
        // Whatever was written before the share existed rides up with the
        // first sync -- create-then-fill and fill-then-create both work.
        let _ = self.sync_now();
        Ok(code)
    }

    pub fn join(&mut self, code: &str) -> Result<(), SharedError> {
        let code = code.trim().to_ascii_lowercase();
        if !code.chars().all(|c| c.is_ascii_alphanumeric()) || code.len() != 10 {
            return Err(SharedError::NoSuchShare);
        }
        // Prove it exists before remembering it, so a typo answers now
        // rather than as eternal silent sync failure.
        let response = ureq::get(&format!("{}/share/{}", self.hub, code))
            .timeout(std::time::Duration::from_secs(8))
            .call();
        match response {
            Ok(response) => {
                self.state.code = Some(code);
                let text = response
                    .into_string()
                    .map_err(|error| SharedError::Io(error.to_string()))?;
                let remote: serde_json::Value = serde_json::from_str(&text)
                    .map_err(|error| SharedError::Io(error.to_string()))?;
                self.merge_remote(remote);
                self.save()?;
                let _ = self.sync_now();
                Ok(())
            }
            Err(ureq::Error::Status(404, _)) => Err(SharedError::NoSuchShare),
            Err(error) => Err(SharedError::Io(error.to_string())),
        }
    }

    pub fn leave(&mut self) -> Result<(), SharedError> {
        self.state.code = None;
        self.save()
    }

    /// Exchange changes with the hub. Returns `true` when the LOCAL copy
    /// changed. Offline returns `false`: the queue keeps waiting.
    pub fn sync(&mut self) -> Result<bool, SharedError> {
        if self.state.code.is_none() {
            return Err(SharedError::NotJoined);
        }
        if self.last_sync.is_some_and(|at| at.elapsed() < SYNC_FLOOR) {
            return Ok(false);
        }
        match self.sync_now() {
            Ok(changed) => Ok(changed),
            // The network being away is a normal state for a laptop.
            Err(SharedError::Io(_)) => Ok(false),
            Err(other) => Err(other),
        }
    }

    fn sync_now(&mut self) -> Result<bool, SharedError> {
        let Some(code) = self.state.code.clone() else {
            return Err(SharedError::NotJoined);
        };
        self.last_sync = Some(std::time::Instant::now());
        let dirty: Vec<serde_json::Value> = self
            .state
            .kv
            .iter()
            .filter(|(_, entry)| entry.dirty)
            .map(|(key, entry)| serde_json::json!({ "key": key, "v": entry.v, "t": entry.t }))
            .collect();
        let url = format!("{}/share/{}", self.hub, code);
        let response = if dirty.is_empty() {
            ureq::get(&url)
                .timeout(std::time::Duration::from_secs(8))
                .call()
        } else {
            let payload = serde_json::json!({ "writes": dirty }).to_string();
            ureq::request("PUT", &url)
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
        // The push succeeded; what was dirty is now the hub's copy too.
        for entry in self.state.kv.values_mut() {
            entry.dirty = false;
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

fn b64_encode(bytes: &[u8]) -> String {
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

fn b64_decode(text: &str) -> Option<Vec<u8>> {
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
        let changed = shared.merge_remote(serde_json::json!({
            "kv": {
                "bread": { "v": b64_encode(b"new"), "t": newer },
                "jam": { "v": b64_encode(b"1"), "t": 5 },
            }
        }));
        assert!(changed);
        assert_eq!(shared.get("bread").expect("get"), Some(b"new".to_vec()));
        assert_eq!(shared.get("jam").expect("get"), Some(b"1".to_vec()));
        // An older remote write never beats a newer local one.
        let changed = shared.merge_remote(serde_json::json!({
            "kv": { "bread": { "v": b64_encode(b"stale"), "t": 1 } }
        }));
        assert!(!changed);
        assert_eq!(shared.get("bread").expect("get"), Some(b"new".to_vec()));
        let _ = std::fs::remove_dir_all(&dir);
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
        a.sync_now().expect("push a");

        // Machine B joins by the code and sees both.
        let mut b = AppShared::open(dir.join("b.json"), hub.clone());
        b.join(&code).expect("join");
        assert_eq!(b.get("milk").expect("get"), Some(b"2".to_vec()));
        assert_eq!(b.get("eggs").expect("get"), Some(b"12".to_vec()));

        // B deletes; A sees it gone after a sync.
        b.delete("milk").expect("delete");
        b.sync_now().expect("push b");
        a.sync_now().expect("pull a");
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
