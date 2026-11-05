//! One app's `store.kv` on one device, as sync sees it.
//!
//! The values themselves stay where they always were -- the app's store
//! file -- and the replica keeps, per key, what sync needs beside them: the
//! version vector of the value the store holds, a hash of it (so an edit
//! made while sync was off is noticed), whether it still has to go up, and
//! the versions from other devices it is in conflict with.
//!
//! The rule, per key (F-321):
//!
//! - one side changed: take it;
//! - both changed to the same value: take it (the two vectors joined);
//! - both changed differently, or one deleted what the other changed: a
//!   conflict. Both are kept. The app goes on reading this device's value,
//!   and the other is held here until the person chooses -- keep this
//!   device's, keep the other's, keep both, or merge (JSON). Unanswered,
//!   both stay, here and on the hub.
//!
//! Pure: no disk, clock or network. The runtime (crates/runtime,
//! `account_sync_host`) reads and writes the store and the replica's file
//! and does the exchange.

use std::collections::BTreeMap;

use crate::vv::{Order, VersionVector};

/// One version of one key: a value, or a deletion (`None`), with the
/// vector it was written at and the device that wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub value: Option<Vec<u8>>,
    pub vv: VersionVector,
    pub by: String,
}

/// What the replica keeps per key.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeyState {
    /// The vector of the value the store holds (or of its deletion).
    pub vv: VersionVector,
    /// The device that wrote it.
    pub by: String,
    /// SHA-256 of the value the store holds; `None` when it was deleted.
    pub hash: Option<String>,
    /// Written here (or settled here) and not yet taken by the hub.
    #[serde(default)]
    pub dirty: bool,
    /// The other devices' versions this one is in conflict with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub others: Vec<StoredVersion>,
}

/// A conflicting version as the replica's file keeps it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StoredVersion {
    /// Base64 of the value; `None` for a deletion.
    pub v: Option<String>,
    pub vv: VersionVector,
    pub by: String,
}

impl StoredVersion {
    fn of(version: &Version) -> StoredVersion {
        StoredVersion {
            v: version.value.as_deref().map(krate_orgkeys::b64_encode),
            vv: version.vv.clone(),
            by: version.by.clone(),
        }
    }

    pub fn value(&self) -> Option<Vec<u8>> {
        self.v.as_deref().and_then(krate_orgkeys::b64_decode)
    }

    pub fn hash(&self) -> Option<String> {
        self.value().map(|v| crate::value_hash(&v))
    }
}

/// A key the person has to decide, as Krate shows it: this device's value
/// is the store's (the caller reads it), the others are here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub key: String,
    /// True when this device's side is a deletion.
    pub mine_deleted: bool,
    pub mine_by: String,
    pub others: Vec<Version>,
    /// Whether "merge" can be offered: this device's value and every
    /// other one are JSON objects, or all lists. The caller decides that
    /// with the store's value in hand; see [`Replica::mergeable`].
    pub others_all_values: bool,
}

/// The person's answer to a conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// This device's value (or deletion) stands.
    KeepMine,
    /// The other device's version at this index of [`Conflict::others`].
    KeepOther(usize),
    /// This device's value stands at the key, and each other device's value
    /// is kept beside it under `<key> (<that device's name>)`.
    KeepBoth,
    /// JSON objects or lists, merged ([`crate::json_merge`]).
    Merge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// The key has no conflict (it was settled elsewhere meanwhile).
    NoConflict,
    /// `KeepOther` named a version that is not there.
    NoSuchVersion,
    /// `Merge` on values that are not all JSON objects or all lists.
    NotMergeable,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ResolveError::NoConflict => "there is nothing to decide for that key any more",
            ResolveError::NoSuchVersion => "that version is not one of the two",
            ResolveError::NotMergeable => {
                "these two cannot be merged: merging is for lists and records"
            }
        })
    }
}

impl std::error::Error for ResolveError {}

/// What one merge of a key from the hub means for the store.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Merged {
    /// `Some(value)` when the store must now hold `value` (`None`: delete).
    pub store: Option<Option<Vec<u8>>>,
    /// The key is in conflict after this merge.
    pub conflict: bool,
}

/// One app's data on one device, as sync keeps it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Replica {
    /// This device's id (crates/orgkeys `device_id`).
    pub device: String,
    #[serde(default)]
    pub keys: BTreeMap<String, KeyState>,
    /// The bucket's revision when this device last read it all, so an
    /// exchange with nothing new costs the hub one read and no bytes.
    #[serde(default)]
    pub rev: Option<u64>,
}

/// One write to send: the key, its vector, its writer, and the hash of the
/// value the store must still hold for it (`None` for a deletion).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outgoing {
    pub key: String,
    pub vv: VersionVector,
    pub by: String,
    pub hash: Option<String>,
}

/// One change the store must make: set `key` to the value, or delete it.
pub type StoreWrite = (String, Option<Vec<u8>>);

fn hash_of(value: Option<&[u8]>) -> Option<String> {
    value.map(crate::value_hash)
}

impl Replica {
    pub fn new(device: &str) -> Replica {
        Replica {
            device: device.to_string(),
            keys: BTreeMap::new(),
            rev: None,
        }
    }

    /// The app wrote `key` here (`None`: deleted it).
    pub fn local_write(&mut self, key: &str, value: Option<&[u8]>) {
        let device = self.device.clone();
        let state = self.keys.entry(key.to_string()).or_insert(KeyState {
            vv: VersionVector::new(),
            by: device.clone(),
            hash: None,
            dirty: false,
            others: Vec::new(),
        });
        state.vv.bump(&device);
        state.by = device;
        state.hash = hash_of(value);
        state.dirty = true;
        // A conflict stays a conflict: the app writing again is not the
        // person deciding it.
    }

    /// Bring the replica in line with what the store holds now. Anything
    /// changed while sync was off -- the toggle, a run with nobody signed
    /// in -- becomes a write of this device's, so it goes up (and may meet
    /// a conflict) rather than being overwritten. Answers how many keys.
    pub fn reconcile(&mut self, store: &BTreeMap<String, Vec<u8>>) -> usize {
        let mut changed = 0;
        for (key, value) in store {
            let hash = Some(crate::value_hash(value));
            if self.keys.get(key).map(|s| &s.hash) != Some(&hash) {
                self.local_write(key, Some(value));
                changed += 1;
            }
        }
        let gone: Vec<String> = self
            .keys
            .iter()
            .filter(|(key, state)| state.hash.is_some() && !store.contains_key(*key))
            .map(|(key, _)| key.clone())
            .collect();
        for key in gone {
            self.local_write(&key, None);
            changed += 1;
        }
        changed
    }

    /// Fold in one key's versions as the hub holds them.
    pub fn merge_remote(&mut self, key: &str, remote: &[Version]) -> Merged {
        let mut out = Merged::default();
        for version in remote {
            let Some(state) = self.keys.get_mut(key) else {
                // A key this device never had: take it as it is.
                self.keys.insert(
                    key.to_string(),
                    KeyState {
                        vv: version.vv.clone(),
                        by: version.by.clone(),
                        hash: hash_of(version.value.as_deref()),
                        dirty: false,
                        others: Vec::new(),
                    },
                );
                out.store = Some(version.value.clone());
                continue;
            };
            match state.vv.compare(&version.vv) {
                Order::Equal | Order::After => {}
                Order::Before => {
                    // Made knowing this device's: take it.
                    state.vv = version.vv.clone();
                    state.by = version.by.clone();
                    state.hash = hash_of(version.value.as_deref());
                    state.dirty = false;
                    let current = state.vv.clone();
                    state.others.retain(|o| !current.covers(&o.vv));
                    out.store = Some(version.value.clone());
                }
                Order::Concurrent => {
                    if hash_of(version.value.as_deref()) == state.hash {
                        // The same value both ways: one version that has
                        // seen both, sent up so the hub keeps one.
                        state.vv.join(&version.vv);
                        state.dirty = true;
                        let current = state.vv.clone();
                        state.others.retain(|o| !current.covers(&o.vv));
                    } else if !state.others.iter().any(|o| o.vv.covers(&version.vv)) {
                        state.others.retain(|o| !version.vv.covers(&o.vv));
                        state.others.push(StoredVersion::of(version));
                    }
                }
            }
        }
        out.conflict = self.keys.get(key).is_some_and(|s| !s.others.is_empty());
        out
    }

    /// After a full read of the bucket: a key whose version the hub does not
    /// hold (a write another device's push raced over, a version made while
    /// apart) goes up again. `hub` is every key the bucket holds, with its
    /// versions' vectors. Answers how many keys.
    pub fn heal(&mut self, hub: &BTreeMap<String, Vec<VersionVector>>) -> usize {
        let mut marked = 0;
        for (key, state) in self.keys.iter_mut() {
            if state.dirty {
                continue;
            }
            let held = hub
                .get(key)
                .is_some_and(|versions| versions.iter().any(|vv| vv.covers(&state.vv)));
            if !held {
                state.dirty = true;
                marked += 1;
            }
        }
        marked
    }

    /// What the next exchange sends.
    pub fn outgoing(&self) -> Vec<Outgoing> {
        self.keys
            .iter()
            .filter(|(_, state)| state.dirty)
            .map(|(key, state)| Outgoing {
                key: key.clone(),
                vv: state.vv.clone(),
                by: state.by.clone(),
                hash: state.hash.clone(),
            })
            .collect()
    }

    /// The hub took `sent`. A key written again while the exchange was away
    /// has a newer vector than what went up, and stays to go next time.
    pub fn mark_sent(&mut self, sent: &[Outgoing]) {
        for write in sent {
            if let Some(state) = self.keys.get_mut(&write.key) {
                if state.vv == write.vv {
                    state.dirty = false;
                }
            }
        }
    }

    /// Every key in conflict, for Krate to show.
    pub fn conflicts(&self) -> Vec<Conflict> {
        self.keys
            .iter()
            .filter(|(_, state)| !state.others.is_empty())
            .map(|(key, state)| Conflict {
                key: key.clone(),
                mine_deleted: state.hash.is_none(),
                mine_by: state.by.clone(),
                others: state
                    .others
                    .iter()
                    .map(|o| Version {
                        value: o.value(),
                        vv: o.vv.clone(),
                        by: o.by.clone(),
                    })
                    .collect(),
                others_all_values: state.others.iter().all(|o| o.v.is_some()),
            })
            .collect()
    }

    /// Whether "merge" can be offered for `key`, given this device's value.
    pub fn mergeable(&self, key: &str, mine: Option<&[u8]>) -> bool {
        let Some(state) = self.keys.get(key) else {
            return false;
        };
        if state.others.is_empty() {
            return false;
        }
        let mut value = match mine {
            Some(mine) => mine.to_vec(),
            None => return false,
        };
        for other in &state.others {
            let Some(theirs) = other.value() else {
                return false;
            };
            match crate::json_merge::merge_bytes(&value, &theirs) {
                Some(merged) => value = merged,
                None => return false,
            }
        }
        true
    }

    /// Answer a conflict. `mine` is the value the store holds for `key`
    /// (`None` when it was deleted here); `name_of` turns a device id into
    /// the name the person knows it by; `taken` says whether a key already
    /// exists, so "keep both" never writes over another value.
    ///
    /// Answers what the store must now hold: `(key, Some(value))` to set,
    /// `(key, None)` to delete. The settled version has seen every side, so
    /// it replaces them all on the hub and on every other device.
    pub fn resolve(
        &mut self,
        key: &str,
        choice: &Resolution,
        mine: Option<&[u8]>,
        name_of: &dyn Fn(&str) -> String,
        taken: &dyn Fn(&str) -> bool,
    ) -> Result<Vec<StoreWrite>, ResolveError> {
        let device = self.device.clone();
        let state = self.keys.get(key).ok_or(ResolveError::NoConflict)?;
        if state.others.is_empty() {
            return Err(ResolveError::NoConflict);
        }
        let mut settled = state.vv.clone();
        for other in &state.others {
            settled.join(&other.vv);
        }
        settled.bump(&device);

        let mut writes = Vec::new();
        let value: Option<Vec<u8>> = match choice {
            Resolution::KeepMine => mine.map(<[u8]>::to_vec),
            Resolution::KeepOther(index) => state
                .others
                .get(*index)
                .ok_or(ResolveError::NoSuchVersion)?
                .value(),
            Resolution::KeepBoth => {
                let mut extra = Vec::new();
                let mut used: Vec<String> = Vec::new();
                for other in &state.others {
                    let Some(theirs) = other.value() else {
                        // Deleted there, kept here: keeping both is
                        // keeping this device's.
                        continue;
                    };
                    let name = both_name(key, &name_of(&other.by), &|k| {
                        taken(k) || used.iter().any(|u| u == k) || k == key
                    });
                    used.push(name.clone());
                    extra.push((name, theirs));
                }
                for (name, theirs) in extra {
                    self.local_write(&name, Some(&theirs));
                    writes.push((name, Some(theirs)));
                }
                mine.map(<[u8]>::to_vec)
            }
            Resolution::Merge => {
                let mut value = mine.ok_or(ResolveError::NotMergeable)?.to_vec();
                for other in &state.others {
                    let theirs = other.value().ok_or(ResolveError::NotMergeable)?;
                    value = crate::json_merge::merge_bytes(&value, &theirs)
                        .ok_or(ResolveError::NotMergeable)?;
                }
                Some(value)
            }
        };
        let state = self.keys.get_mut(key).expect("checked above");
        let changed = hash_of(value.as_deref()) != state.hash;
        state.vv = settled;
        state.by = device;
        state.hash = hash_of(value.as_deref());
        state.dirty = true;
        state.others.clear();
        if changed {
            writes.insert(0, (key.to_string(), value));
        }
        Ok(writes)
    }
}

/// `<key> (<device name>)`, made a valid store key (no slashes, no `..`, no
/// control characters, at most 256 bytes) and numbered when taken.
pub fn both_name(key: &str, device_name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let clean = |text: &str| -> String {
        let text: String = text
            .chars()
            .map(|c| {
                if c.is_control() || c == '/' || c == '\\' {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        let mut text = text.replace("..", ".");
        while text.contains("..") {
            text = text.replace("..", ".");
        }
        text.trim().to_string()
    };
    let mut name = clean(device_name);
    if name.is_empty() {
        name = "other device".to_string();
    }
    for n in 1.. {
        let suffix = if n == 1 {
            format!(" ({name})")
        } else {
            format!(" ({name} {n})")
        };
        let room = crate::replica::MAX_KEY_BYTES.saturating_sub(suffix.len());
        let mut base = key.to_string();
        while base.len() > room {
            base.pop();
        }
        let candidate = format!("{base}{suffix}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!("1.. never ends")
}

/// The store's longest key (crates/adapter-common `kv::MAX_KEY_BYTES`).
pub const MAX_KEY_BYTES: usize = 256;

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccc";

    /// Three devices and the hub between them, in memory: the hub keeps
    /// every version no other version covers, as the worker does.
    #[derive(Default)]
    struct Hub {
        keys: BTreeMap<String, Vec<Version>>,
    }

    impl Hub {
        fn take(&mut self, key: &str, version: Version) {
            let versions = self.keys.entry(key.to_string()).or_default();
            if versions.iter().any(|v| v.vv.covers(&version.vv)) {
                return;
            }
            versions.retain(|v| !version.vv.covers(&v.vv));
            versions.push(version);
        }

        fn vectors(&self) -> BTreeMap<String, Vec<VersionVector>> {
            self.keys
                .iter()
                .map(|(k, vs)| (k.clone(), vs.iter().map(|v| v.vv.clone()).collect()))
                .collect()
        }
    }

    /// A device: its store (the values) and its replica.
    struct Device {
        store: BTreeMap<String, Vec<u8>>,
        replica: Replica,
    }

    impl Device {
        fn new(id: &str) -> Device {
            Device {
                store: BTreeMap::new(),
                replica: Replica::new(id),
            }
        }

        fn set(&mut self, key: &str, value: &str) {
            self.store
                .insert(key.to_string(), value.as_bytes().to_vec());
            self.replica.local_write(key, Some(value.as_bytes()));
        }

        fn delete(&mut self, key: &str) {
            self.store.remove(key);
            self.replica.local_write(key, None);
        }

        fn get(&self, key: &str) -> Option<String> {
            self.store
                .get(key)
                .map(|v| String::from_utf8(v.clone()).unwrap())
        }

        /// Push what is dirty, read everything back, fold it in, heal.
        fn sync(&mut self, hub: &mut Hub) {
            let sent = self.replica.outgoing();
            for write in &sent {
                let value = write.hash.as_ref().map(|_| self.store[&write.key].clone());
                hub.take(
                    &write.key,
                    Version {
                        value,
                        vv: write.vv.clone(),
                        by: write.by.clone(),
                    },
                );
            }
            self.replica.mark_sent(&sent);
            for (key, versions) in hub.keys.clone() {
                let merged = self.replica.merge_remote(&key, &versions);
                match merged.store {
                    Some(Some(value)) => {
                        self.store.insert(key.clone(), value);
                    }
                    Some(None) => {
                        self.store.remove(&key);
                    }
                    None => {}
                }
            }
            self.replica.heal(&hub.vectors());
        }

        fn conflict(&self, key: &str) -> Option<Conflict> {
            self.replica.conflicts().into_iter().find(|c| c.key == key)
        }
    }

    #[test]
    fn one_side_changed_takes_it_both_ways() {
        let mut hub = Hub::default();
        let (mut mac, mut phone) = (Device::new(A), Device::new(B));
        mac.set("r1", "Buy milk");
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        assert_eq!(phone.get("r1").as_deref(), Some("Buy milk"), "Mac to phone");
        phone.set("r2", "Call mum");
        phone.set("r1", "Buy oat milk");
        phone.sync(&mut hub);
        mac.sync(&mut hub);
        assert_eq!(mac.get("r2").as_deref(), Some("Call mum"), "phone to Mac");
        assert_eq!(
            mac.get("r1").as_deref(),
            Some("Buy oat milk"),
            "an edit of a synced key"
        );
        assert!(mac.replica.conflicts().is_empty() && phone.replica.conflicts().is_empty());
        // A delete crosses too.
        mac.delete("r2");
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        assert_eq!(phone.get("r2"), None);
        assert_eq!(hub.keys["r2"].len(), 1, "one version on the hub");
        assert!(phone.replica.outgoing().is_empty(), "nothing left to send");
    }

    #[test]
    fn both_changed_to_the_same_value_is_not_a_conflict() {
        let mut hub = Hub::default();
        let (mut mac, mut phone) = (Device::new(A), Device::new(B));
        mac.set("theme", "dark");
        phone.set("theme", "dark");
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        mac.sync(&mut hub);
        assert!(phone.conflict("theme").is_none() && mac.conflict("theme").is_none());
        // The joined version goes up with the next exchange.
        phone.sync(&mut hub);
        mac.sync(&mut hub);
        assert_eq!(
            hub.keys["theme"].len(),
            1,
            "the joined version replaced both"
        );
        assert!(mac.replica.outgoing().is_empty() && phone.replica.outgoing().is_empty());
        assert_eq!(mac.replica.keys["theme"].vv, phone.replica.keys["theme"].vv);
    }

    #[test]
    fn concurrent_edits_are_one_conflict_holding_both_and_the_app_reads_its_own() {
        let mut hub = Hub::default();
        let (mut mac, mut phone) = (Device::new(A), Device::new(B));
        mac.set("r1", "Buy milk");
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        // Both offline, both edit.
        mac.set("r1", "Buy milk and eggs");
        phone.set("r1", "Buy bread");
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        mac.sync(&mut hub);
        assert_eq!(
            mac.get("r1").as_deref(),
            Some("Buy milk and eggs"),
            "Mac reads its own"
        );
        assert_eq!(
            phone.get("r1").as_deref(),
            Some("Buy bread"),
            "phone reads its own"
        );
        let on_phone = phone.conflict("r1").expect("a conflict on the phone");
        assert_eq!(on_phone.others.len(), 1);
        assert_eq!(
            on_phone.others[0].value.as_deref(),
            Some(&b"Buy milk and eggs"[..])
        );
        assert_eq!(on_phone.others[0].by, A);
        let on_mac = mac.conflict("r1").expect("a conflict on the Mac");
        assert_eq!(on_mac.others[0].value.as_deref(), Some(&b"Buy bread"[..]));
        assert_eq!(
            hub.keys["r1"].len(),
            2,
            "the hub keeps both while nobody decides"
        );
        // Syncing again changes nothing: still one conflict, both kept.
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        assert_eq!(mac.replica.conflicts().len(), 1);
        assert_eq!(mac.conflict("r1").unwrap().others.len(), 1);
        // The app writing again is not the person deciding.
        phone.set("r1", "Buy bread and jam");
        phone.sync(&mut hub);
        mac.sync(&mut hub);
        assert!(
            phone.conflict("r1").is_some(),
            "still a conflict on the phone"
        );
        let on_mac = mac.conflict("r1").unwrap();
        assert_eq!(
            on_mac.others.len(),
            1,
            "the newer phone version replaced the older"
        );
        assert_eq!(
            on_mac.others[0].value.as_deref(),
            Some(&b"Buy bread and jam"[..])
        );
    }

    #[test]
    fn delete_against_edit_is_a_conflict_both_ways() {
        let mut hub = Hub::default();
        let (mut mac, mut phone) = (Device::new(A), Device::new(B));
        mac.set("r1", "Buy milk");
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        mac.delete("r1");
        phone.set("r1", "Buy milk today");
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        mac.sync(&mut hub);
        assert_eq!(
            mac.get("r1"),
            None,
            "deleted on the Mac, still deleted there"
        );
        assert_eq!(phone.get("r1").as_deref(), Some("Buy milk today"));
        let on_mac = mac.conflict("r1").expect("deleted here, changed there");
        assert!(on_mac.mine_deleted);
        assert_eq!(
            on_mac.others[0].value.as_deref(),
            Some(&b"Buy milk today"[..])
        );
        let on_phone = phone.conflict("r1").expect("changed here, deleted there");
        assert!(!on_phone.mine_deleted);
        assert_eq!(on_phone.others[0].value, None);
        assert!(!on_phone.others_all_values);
    }

    #[test]
    fn three_devices_converge_and_a_three_way_conflict_settles_everywhere() {
        let mut hub = Hub::default();
        let (mut a, mut b, mut c) = (Device::new(A), Device::new(B), Device::new(C));
        a.set("list", "one");
        a.sync(&mut hub);
        b.sync(&mut hub);
        c.sync(&mut hub);
        // B edits on top of A; C sees B's later: a chain, no conflict.
        b.set("list", "two");
        b.sync(&mut hub);
        c.sync(&mut hub);
        a.sync(&mut hub);
        for d in [&a, &b, &c] {
            assert_eq!(d.get("list").as_deref(), Some("two"));
            assert!(d.replica.conflicts().is_empty());
        }
        // All three apart.
        a.set("list", "from a");
        b.set("list", "from b");
        c.set("list", "from c");
        for _ in 0..2 {
            a.sync(&mut hub);
            b.sync(&mut hub);
            c.sync(&mut hub);
        }
        assert_eq!(hub.keys["list"].len(), 3);
        let on_a = a.conflict("list").unwrap();
        assert_eq!(on_a.others.len(), 2, "A holds B's and C's");
        // A keeps C's; every device takes the settled version.
        let index = on_a.others.iter().position(|v| v.by == C).unwrap();
        let mine = a.store.get("list").cloned();
        let writes = a
            .replica
            .resolve(
                "list",
                &Resolution::KeepOther(index),
                mine.as_deref(),
                &|_| "C".into(),
                &|_| false,
            )
            .unwrap();
        assert_eq!(writes, vec![("list".to_string(), Some(b"from c".to_vec()))]);
        for (key, value) in writes {
            a.store.insert(key, value.unwrap());
        }
        a.sync(&mut hub);
        b.sync(&mut hub);
        c.sync(&mut hub);
        assert_eq!(
            hub.keys["list"].len(),
            1,
            "the settled version replaced all three"
        );
        for d in [&a, &b, &c] {
            assert_eq!(d.get("list").as_deref(), Some("from c"));
            assert!(
                d.replica.conflicts().is_empty(),
                "settled on {}",
                d.replica.device
            );
        }
    }

    fn two_way_conflict() -> (Hub, Device, Device) {
        let mut hub = Hub::default();
        let (mut mac, mut phone) = (Device::new(A), Device::new(B));
        mac.set("r1", r#"{"title":"Milk","done":false}"#);
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        mac.set("r1", r#"{"title":"Milk","done":true}"#);
        phone.set("r1", r#"{"title":"Oat milk","due":"fri"}"#);
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        mac.sync(&mut hub);
        (hub, mac, phone)
    }

    fn resolve_on(device: &mut Device, choice: Resolution) -> Vec<StoreWrite> {
        let mine = device.store.get("r1").cloned();
        let store = device.store.clone();
        let writes = device
            .replica
            .resolve(
                "r1",
                &choice,
                mine.as_deref(),
                &|id| {
                    if id == B {
                        "Galaxy S22".into()
                    } else {
                        "Mac".into()
                    }
                },
                &|k| store.contains_key(k),
            )
            .unwrap();
        for (key, value) in &writes {
            match value {
                Some(v) => {
                    device.store.insert(key.clone(), v.clone());
                }
                None => {
                    device.store.remove(key);
                }
            }
        }
        writes
    }

    #[test]
    fn keep_mine_settles_on_this_devices_value_everywhere() {
        let (mut hub, mut mac, mut phone) = two_way_conflict();
        let writes = resolve_on(&mut mac, Resolution::KeepMine);
        assert!(writes.is_empty(), "the store already holds it");
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        assert_eq!(
            phone.get("r1").as_deref(),
            Some(r#"{"title":"Milk","done":true}"#)
        );
        assert!(phone.conflict("r1").is_none() && mac.conflict("r1").is_none());
    }

    #[test]
    fn keep_other_takes_the_other_devices_value() {
        let (mut hub, mut mac, mut phone) = two_way_conflict();
        resolve_on(&mut mac, Resolution::KeepOther(0));
        assert_eq!(
            mac.get("r1").as_deref(),
            Some(r#"{"title":"Oat milk","due":"fri"}"#)
        );
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        assert_eq!(
            phone.get("r1").as_deref(),
            Some(r#"{"title":"Oat milk","due":"fri"}"#)
        );
        assert!(phone.conflict("r1").is_none());
        let mine = mac.store.get("r1").cloned();
        assert_eq!(
            mac.replica.resolve(
                "r1",
                &Resolution::KeepMine,
                mine.as_deref(),
                &|_| String::new(),
                &|_| false
            ),
            Err(ResolveError::NoConflict),
            "nothing left to decide"
        );
    }

    #[test]
    fn keep_both_puts_the_others_value_beside_it_under_the_devices_name() {
        let (mut hub, mut mac, mut phone) = two_way_conflict();
        let writes = resolve_on(&mut mac, Resolution::KeepBoth);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, "r1 (Galaxy S22)");
        assert_eq!(
            mac.get("r1").as_deref(),
            Some(r#"{"title":"Milk","done":true}"#)
        );
        assert_eq!(
            mac.get("r1 (Galaxy S22)").as_deref(),
            Some(r#"{"title":"Oat milk","due":"fri"}"#)
        );
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        assert_eq!(
            phone.get("r1").as_deref(),
            Some(r#"{"title":"Milk","done":true}"#)
        );
        assert_eq!(
            phone.get("r1 (Galaxy S22)").as_deref(),
            Some(r#"{"title":"Oat milk","due":"fri"}"#)
        );
        assert!(phone.replica.conflicts().is_empty());
    }

    #[test]
    fn keep_both_never_writes_over_a_key_that_exists() {
        let taken = |k: &str| k == "r1 (Phone)" || k == "r1 (Phone 2)";
        assert_eq!(both_name("r1", "Phone", &taken), "r1 (Phone 3)");
        assert_eq!(both_name("r1", "a/b\\c..d", &|_| false), "r1 (a b c.d)");
        assert_eq!(both_name("r1", "  ", &|_| false), "r1 (other device)");
        let long = "k".repeat(300);
        let name = both_name(&long, "Phone", &|_| false);
        assert!(name.len() <= MAX_KEY_BYTES && name.ends_with(" (Phone)"));
    }

    #[test]
    fn merge_unions_the_two_records_with_the_others_fields_winning() {
        let (mut hub, mut mac, mut phone) = two_way_conflict();
        let mine = mac.store.get("r1").cloned();
        assert!(mac.replica.mergeable("r1", mine.as_deref()));
        resolve_on(&mut mac, Resolution::Merge);
        let merged: serde_json::Value = serde_json::from_slice(&mac.store["r1"]).unwrap();
        assert_eq!(
            merged,
            serde_json::json!({ "title": "Oat milk", "done": true, "due": "fri" })
        );
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        assert_eq!(phone.store["r1"], mac.store["r1"]);
        assert!(phone.conflict("r1").is_none());
    }

    #[test]
    fn merge_is_refused_for_plain_text_and_for_a_deletion() {
        let mut hub = Hub::default();
        let (mut mac, mut phone) = (Device::new(A), Device::new(B));
        mac.set("note", "a");
        phone.set("note", "b");
        mac.set("gone", "[1]");
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        mac.sync(&mut hub);
        let mine = mac.store.get("note").cloned();
        assert!(!mac.replica.mergeable("note", mine.as_deref()));
        assert_eq!(
            mac.replica.resolve(
                "note",
                &Resolution::Merge,
                mine.as_deref(),
                &|_| String::new(),
                &|_| false
            ),
            Err(ResolveError::NotMergeable)
        );
        assert!(
            mac.conflict("note").is_some(),
            "a refused merge decides nothing"
        );
        assert_eq!(
            mac.replica.resolve(
                "note",
                &Resolution::KeepOther(7),
                mine.as_deref(),
                &|_| String::new(),
                &|_| false
            ),
            Err(ResolveError::NoSuchVersion)
        );
    }

    #[test]
    fn an_edit_made_while_sync_was_off_goes_up_and_may_conflict() {
        let mut hub = Hub::default();
        let (mut mac, mut phone) = (Device::new(A), Device::new(B));
        mac.set("r1", "Milk");
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        // The toggle off on the phone: its store changes with no replica.
        phone
            .store
            .insert("r1".into(), b"Milk (phone, offline)".to_vec());
        phone.store.insert("r9".into(), b"new while off".to_vec());
        mac.set("r1", "Milk (mac)");
        mac.sync(&mut hub);
        // On again.
        let store = phone.store.clone();
        assert_eq!(phone.replica.reconcile(&store), 2);
        assert_eq!(phone.replica.reconcile(&store), 0, "once");
        phone.sync(&mut hub);
        mac.sync(&mut hub);
        assert_eq!(mac.get("r9").as_deref(), Some("new while off"));
        assert!(phone.conflict("r1").is_some() && mac.conflict("r1").is_some());
        // And a key removed while off is a deletion.
        phone.store.remove("r9");
        let store = phone.store.clone();
        assert_eq!(phone.replica.reconcile(&store), 1);
        phone.sync(&mut hub);
        mac.sync(&mut hub);
        assert_eq!(mac.get("r9"), None);
    }

    #[test]
    fn a_write_the_hub_lost_goes_up_again() {
        let mut hub = Hub::default();
        let (mut mac, mut phone) = (Device::new(A), Device::new(B));
        mac.set("r1", "Milk");
        mac.sync(&mut hub);
        // Two pushes raced on the hub and this one was overwritten.
        hub.keys.clear();
        phone.sync(&mut hub);
        assert_eq!(phone.get("r1"), None);
        mac.sync(&mut hub);
        assert!(mac.replica.outgoing().len() == 1, "healed: queued again");
        mac.sync(&mut hub);
        phone.sync(&mut hub);
        assert_eq!(phone.get("r1").as_deref(), Some("Milk"));
    }

    #[test]
    fn a_write_during_an_exchange_stays_for_the_next() {
        let mut replica = Replica::new(A);
        replica.local_write("k", Some(b"1"));
        let sent = replica.outgoing();
        replica.local_write("k", Some(b"2"));
        replica.mark_sent(&sent);
        assert_eq!(replica.outgoing().len(), 1, "the newer write still goes up");
    }

    #[test]
    fn the_replica_survives_its_file() {
        let (_, mac, _) = two_way_conflict();
        let text = serde_json::to_string(&mac.replica).unwrap();
        let back: Replica = serde_json::from_str(&text).unwrap();
        assert_eq!(back, mac.replica);
        assert_eq!(back.conflicts().len(), 1);
    }
}
