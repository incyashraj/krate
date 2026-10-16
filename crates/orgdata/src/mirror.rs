//! A device's copy of one app's team data in one organisation.
//!
//! Local first: every write lands here, stamped on the device's clock as the
//! hub last measured it, and waits as `pending` until a sync's answer says
//! the hub took it. A sync sends [`Mirror::prepare`]'s writes and folds the
//! hub's answer in with [`Mirror::apply`]; which write wins is
//! [`crate::merge`]'s rule, the one the hub applies too.
//!
//! The device's own stamps not yet taken are on its old clock: an answer
//! moves them by exactly what the hub moved the ones it took, so both hold
//! the same stamps afterwards, and measures the clock again.
//!
//! What leaves the device is sealed ([`seal_writes`], feature `seal`):
//! opaque record and field names, the stamp in the clear, the value and the
//! writer's name inside XChaCha20-Poly1305 under the app's data key. A row
//! coming down that does not open, or whose names are not the ones it was
//! sealed under, is refused and counted ([`open_rows`]), never applied.
//!
//! Nothing here touches a disk, a clock or the network: the caller hands in
//! the time and keeps the copy ([`Mirror::to_json`]) wherever it keeps
//! things -- a file on a computer, IndexedDB in a browser tab.

use std::collections::BTreeMap;

use crate::merge::{self, Stamp, Value};

/// One record as an app reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub id: String,
    pub fields: Vec<(String, Value)>,
    pub changed_by: String,
    pub changed_at: u64,
}

/// One page of a collection's records.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub entries: Vec<Entry>,
    pub removed: Vec<String>,
    pub next: String,
    pub more: bool,
}

/// The signed-in person, as the organisation's last answer named them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub role: String,
    pub org: String,
}

/// What a write or a read was refused for, in the WIT `org-error`'s cases.
#[derive(Debug, Clone, PartialEq)]
pub enum DataError {
    Invalid(String),
    TooLarge,
    OverQuota,
}

/// The most records one `records` page holds.
pub const PAGE_RECORDS: usize = 200;
/// The quota before the hub has said what it is.
pub const DEFAULT_QUOTA: u64 = 50 * 1024 * 1024;
/// What one push carries at most, under the hub's 1 MiB.
pub const PUSH_BYTES: usize = 512 * 1024;
pub const PUSH_WRITES: usize = 1000;

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

/// The person, as the organisation's last answer named them.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct Me {
    pub name: String,
    pub role: String,
    pub org: String,
}

/// This device's copy of one app's data in one organisation.
///
/// The fields a host reads and keeps are public; the records, the stamps
/// and the counters change only through the methods, which hold the rules.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct Mirror {
    v: u32,
    /// This copy's device id: 16 random hex digits.
    pub device: String,
    /// How far this device's clock is behind the hub's, as last measured.
    pub offset_ms: i64,
    /// The greatest stamp made or seen here.
    last: Option<String>,
    /// The hub's cursor: what this device has pulled.
    cursor: Option<String>,
    /// When the hub last took this device's changes.
    pub synced_at: Option<u64>,
    /// The app's quota in the organisation, once the hub has said it.
    pub quota: Option<u64>,
    /// The person, once a sync has said who they are.
    pub me: Option<Me>,
    local_seq: u64,
    generation: u64,
    records: BTreeMap<String, BTreeMap<String, Rec>>,
    /// The keyring, sealed to this device's key: opens only here.
    #[serde(default)]
    pub envelope: Option<String>,
    /// Rows from the hub that did not open or did not match their names:
    /// refused, never applied.
    #[serde(default)]
    pub rejected: u64,
}

/// What one round of a sync sends, and what [`Mirror::apply`] needs back.
#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    /// This device's writes, in plaintext: sealed just before they leave.
    pub writes: Vec<serde_json::Value>,
    pub since: Option<String>,
    pub device: String,
    pub sent_now: u64,
    /// (collection, id, field, generation) of each write sent.
    pub sent: Vec<(String, String, String, u64)>,
    /// Pending writes that did not fit: send another round.
    pub left_behind: bool,
}

impl Round {
    /// The body of `POST /orgs/<slug>/data/<digest>/sync`, with the writes
    /// as sealed.
    pub fn body(&self, sealed: Vec<serde_json::Value>) -> serde_json::Value {
        serde_json::json!({
            "v": 1,
            "device": self.device,
            "now": self.sent_now,
            "since": self.since,
            "writes": sealed,
        })
    }
}

/// The plaintext sealed for one field: its names, value and writer.
pub fn payload(c: &str, i: &str, f: &str, v: &serde_json::Value, by: &str) -> String {
    serde_json::json!({ "c": c, "i": i, "f": f, "v": v, "w": by }).to_string()
}

/// A nonce and a tag (40 bytes) on the payload, and the hub's 160 bytes
/// of bookkeeping per row (cloud/worker/src/org-data.js ROW_OVERHEAD).
fn sealed_size(c: &str, i: &str, f: &str, cell: &MCell) -> u64 {
    (payload(c, i, f, &cell.v, &cell.by).len() + 40 + 160) as u64
}

/// The cells of a record as the merge rule reads them.
fn cells_of(rec: &Rec) -> Vec<(String, merge::Cell)> {
    rec.cells
        .iter()
        .filter_map(|(name, cell)| {
            Some((
                name.clone(),
                merge::Cell {
                    value: Value::from_wire(&cell.v)?,
                    stamp: Stamp::parse(&cell.s)?,
                },
            ))
        })
        .collect()
}

fn entry_of(id: &str, rec: &Rec) -> Option<Entry> {
    let cells = cells_of(rec);
    let live = merge::live_fields(cells.iter().map(|(n, c)| (n.as_str(), c)));
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
        .get(merge::TOMBSTONE)
        .and_then(|c| Stamp::parse(&c.s))
    else {
        return;
    };
    rec.cells.retain(|name, cell| {
        name == merge::TOMBSTONE || Stamp::parse(&cell.s).is_some_and(|s| s > tomb)
    });
}

fn names_problem(collection: &str, id: &str) -> Option<String> {
    merge::name_problem("collection name", collection)
        .or_else(|| merge::name_problem("record id", id))
}

impl Mirror {
    /// An empty copy for the device `device` (16 random hex digits, drawn
    /// by the caller).
    pub fn fresh(device: String) -> Mirror {
        Mirror {
            v: 1,
            device,
            ..Default::default()
        }
    }

    /// A copy as [`Mirror::to_json`] wrote it, or `None` when it is not one
    /// (then the caller starts a fresh one).
    pub fn from_json(text: &str) -> Option<Mirror> {
        serde_json::from_str::<Mirror>(text)
            .ok()
            .filter(|m| merge::valid_device(&m.device))
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| e.to_string())
    }

    /// Keep `envelope` with the copy, so the key works offline. True when
    /// it changed.
    pub fn set_envelope(&mut self, envelope: Option<String>) -> bool {
        if self.envelope == envelope {
            return false;
        }
        self.envelope = envelope;
        true
    }

    /// The app's quota in the organisation, as the hub last said it.
    pub fn quota(&self) -> u64 {
        self.quota.unwrap_or(DEFAULT_QUOTA)
    }

    /// The time to stamp with: this device's clock (`local_ms`, moved by
    /// `skew_ms` for tests), moved by how far the hub last found it off.
    pub fn clock(&self, local_ms: i64, skew_ms: i64) -> u64 {
        (local_ms + skew_ms + self.offset_ms).max(0) as u64
    }

    /// The name a write made here carries: the organisation's for the
    /// person once a sync said it, `fallback` before that.
    pub fn me_name(&self, fallback: Option<&str>) -> String {
        self.me
            .as_ref()
            .map(|me| me.name.clone())
            .or_else(|| fallback.map(str::to_string))
            .unwrap_or_default()
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
    pub fn used(&self) -> u64 {
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

    /// Changes made here that the hub has not taken yet.
    pub fn pending(&self) -> u32 {
        self.records
            .values()
            .flat_map(|recs| recs.values())
            .flat_map(|rec| rec.cells.values())
            .filter(|cell| cell.pending)
            .count() as u32
    }

    /// Whether `collection`/`id` holds a live record here.
    pub fn has(&self, collection: &str, id: &str) -> bool {
        self.records
            .get(collection)
            .and_then(|recs| recs.get(id))
            .is_some_and(|rec| entry_of(id, rec).is_some())
    }

    /// `store.org.put`: write the named fields of one record as one change,
    /// stamped at `phys` (milliseconds, from [`Mirror::clock`]) and signed
    /// `by`. No fields is no change (`Ok(false)`).
    pub fn put(
        &mut self,
        collection: &str,
        id: &str,
        fields: Vec<(String, Value)>,
        phys: u64,
        by: &str,
    ) -> Result<bool, DataError> {
        if fields.is_empty() {
            return Ok(false);
        }
        if fields.iter().any(|(name, _)| name.is_empty()) {
            return Err(DataError::Invalid("a field name cannot be empty".into()));
        }
        self.write(collection, id, fields, phys, by).map(|()| true)
    }

    /// `store.org.delete`: one more row, the record's tombstone. Deleting a
    /// record that is not here is no change (`Ok(false)`).
    pub fn delete(
        &mut self,
        collection: &str,
        id: &str,
        phys: u64,
        by: &str,
    ) -> Result<bool, DataError> {
        if !self.has(collection, id) {
            if let Some(problem) = names_problem(collection, id) {
                return Err(DataError::Invalid(problem));
            }
            return Ok(false);
        }
        self.write(
            collection,
            id,
            vec![(merge::TOMBSTONE.to_string(), Value::Null)],
            phys,
            by,
        )
        .map(|()| true)
    }

    /// Write cells into one record, as one change.
    pub fn write(
        &mut self,
        collection: &str,
        id: &str,
        fields: Vec<(String, Value)>,
        phys: u64,
        by: &str,
    ) -> Result<(), DataError> {
        if let Some(problem) = names_problem(collection, id) {
            return Err(DataError::Invalid(problem));
        }
        for (name, value) in &fields {
            if name != merge::TOMBSTONE {
                if let Some(problem) = merge::name_problem("field name", name) {
                    return Err(DataError::Invalid(problem));
                }
            }
            if let Value::Number(n) = value {
                if !n.is_finite() {
                    return Err(DataError::Invalid(format!(
                        "the field {name} is not a finite number"
                    )));
                }
            }
        }
        let device = self.device.clone();
        let generation = self.generation + 1;
        let mut last = self.last();
        let mut rec = self
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
                    by: by.to_string(),
                    pending: true,
                    g: generation,
                },
            );
            last = Some(stamp);
        }
        bury(&mut rec);
        let cells = cells_of(&rec);
        if merge::record_size(cells.iter().map(|(n, c)| (n.as_str(), c))) > merge::MAX_RECORD_BYTES
        {
            return Err(DataError::TooLarge);
        }
        let size_of = |rec: &Rec| -> u64 {
            rec.cells
                .iter()
                .map(|(f, cell)| sealed_size(collection, id, f, cell))
                .sum()
        };
        let old = self
            .records
            .get(collection)
            .and_then(|recs| recs.get(id))
            .map(size_of)
            .unwrap_or(0);
        let new = size_of(&rec);
        let quota = self.quota.unwrap_or(DEFAULT_QUOTA);
        if new > old && self.used() - old + new > quota {
            return Err(DataError::OverQuota);
        }
        self.records
            .entry(collection.to_string())
            .or_default()
            .insert(id.to_string(), rec);
        self.generation = generation;
        if let Some(last) = last {
            self.observe(&last);
        }
        self.local_seq += 1;
        let seq = self.local_seq;
        if let Some(rec) = self
            .records
            .get_mut(collection)
            .and_then(|recs| recs.get_mut(id))
        {
            rec.seq = seq;
        }
        Ok(())
    }

    /// `store.org.get`.
    pub fn get(&self, collection: &str, id: &str) -> Result<Option<Entry>, DataError> {
        if let Some(problem) = names_problem(collection, id) {
            return Err(DataError::Invalid(problem));
        }
        Ok(self
            .records
            .get(collection)
            .and_then(|recs| recs.get(id))
            .and_then(|rec| entry_of(id, rec)))
    }

    /// `store.org.records`: from the start, or what changed after a cursor
    /// an earlier page gave.
    pub fn records(&self, collection: &str, since: Option<&str>) -> Result<Page, DataError> {
        if let Some(problem) = merge::name_problem("collection name", collection) {
            return Err(DataError::Invalid(problem));
        }
        let after = match since {
            None => None,
            Some(text) => Some(text.parse::<u64>().map_err(|_| {
                DataError::Invalid("that cursor is not one an earlier page gave".into())
            })?),
        };
        let mut recs: Vec<(&String, &Rec)> = self
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
            page.more = self
                .records
                .get(collection)
                .is_some_and(|recs| recs.values().any(|rec| rec.seq > last));
        }
        Ok(page)
    }

    /// `store.org.who`: the person, once a sync has said who they are.
    pub fn who(&self) -> Option<Member> {
        self.me.as_ref().map(|me| Member {
            name: me.name.clone(),
            role: me.role.clone(),
            org: me.org.clone(),
        })
    }

    /// The next push: this device's pending writes, as many as fit, sent at
    /// `sent_now` on this device's clock ([`Mirror::clock`]).
    pub fn prepare(&self, sent_now: u64) -> Round {
        let mut writes = Vec::new();
        let mut sent = Vec::new();
        let mut bytes = 0usize;
        let mut left_behind = false;
        'all: for (c, recs) in &self.records {
            for (i, rec) in recs {
                for (f, cell) in &rec.cells {
                    if !cell.pending {
                        continue;
                    }
                    let size = c.len() + i.len() + f.len() + cell.v.to_string().len() + 96;
                    if writes.len() >= PUSH_WRITES
                        || (bytes + size > PUSH_BYTES && !writes.is_empty())
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
        Round {
            writes,
            since: self.cursor.clone(),
            device: self.device.clone(),
            sent_now,
            sent,
            left_behind,
        }
    }

    /// Fold the hub's answer (its rows already opened, [`open_rows`]) into
    /// the copy. True when what an app reads changed.
    pub fn apply(&mut self, round: &Round, answer: &serde_json::Value) -> bool {
        let hub_now = answer["now"].as_u64().unwrap_or(round.sent_now);
        // This device's own stamps not yet taken are on its old clock: move
        // them exactly as the hub moved the ones it took, then measure the
        // clock again.
        for recs in self.records.values_mut() {
            for rec in recs.values_mut() {
                for cell in rec.cells.values_mut() {
                    if cell.pending {
                        if let Some(stamp) = Stamp::parse(&cell.s) {
                            cell.s = merge::rebase(&stamp, round.sent_now, hub_now).to_string();
                        }
                    }
                }
            }
        }
        if let Some(last) = self.last() {
            self.last = Some(merge::rebase(&last, round.sent_now, hub_now).to_string());
        }
        let delta = i128::from(hub_now) - i128::from(round.sent_now);
        if delta.unsigned_abs() > u128::from(merge::DRIFT_MS) {
            self.offset_ms = self.offset_ms.saturating_add(delta as i64);
        }
        // What was sent and not written again since is the hub's now.
        for (c, i, f, g) in &round.sent {
            if let Some(cell) = self
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
            self.synced_at = Some(hub_now);
        }
        let mut changed = false;
        // A device behind the hub's oldest tombstone starts over from the
        // hub's copy, keeping only its own changes not yet taken.
        if answer["full"].as_bool() == Some(true) {
            for recs in self.records.values_mut() {
                for rec in recs.values_mut() {
                    let before = rec.cells.len();
                    rec.cells.retain(|_, cell| cell.pending);
                    changed |= rec.cells.len() != before;
                }
                recs.retain(|_, rec| !rec.cells.is_empty());
            }
            self.records.retain(|_, recs| !recs.is_empty());
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
            self.observe(&s);
            let rec = self
                .records
                .entry(c.to_string())
                .or_default()
                .entry(i.to_string())
                .or_default();
            let before = entry_of(i, rec);
            let held = rec.cells.get(f).and_then(|cell| Stamp::parse(&cell.s));
            if !merge::wins(&s, held.as_ref()) {
                continue;
            }
            let tomb = rec
                .cells
                .get(merge::TOMBSTONE)
                .and_then(|cell| Stamp::parse(&cell.s));
            if f != merge::TOMBSTONE && !merge::alive(&s, tomb.as_ref()) {
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
                self.local_seq += 1;
                rec.seq = self.local_seq;
                changed = true;
            }
        }
        if let Some(cursor) = answer["cursor"].as_str() {
            self.cursor = Some(cursor.to_string());
        }
        if let Some(limit) = answer["quota"]["limit"].as_u64() {
            self.quota = Some(limit);
        }
        if let (Some(name), Some(role)) =
            (answer["me"]["name"].as_str(), answer["me"]["role"].as_str())
        {
            self.me = Some(Me {
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
}

/// Seal one round's writes under the current version of the app's data key.
#[cfg(feature = "seal")]
pub fn seal_writes(
    writes: &[serde_json::Value],
    keyring: &krate_orgkeys::Keyring,
    app_id: &str,
) -> Result<Vec<serde_json::Value>, String> {
    use krate_orgkeys::{AppDataKey, RowKind};
    let (version, _) = keyring.current();
    let dk = AppDataKey::derive(keyring, version, app_id)
        .ok_or_else(|| "this device's keyring has no current key".to_string())?;
    writes
        .iter()
        .map(|w| {
            let (c, i, f) = (
                w["c"].as_str().unwrap_or(""),
                w["i"].as_str().unwrap_or(""),
                w["f"].as_str().unwrap_or(""),
            );
            let kind = if f == merge::TOMBSTONE {
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
                .map_err(|e| e.to_string())?;
            Ok(serde_json::json!({
                "r": r, "k": k, "kind": kind.as_str(), "s": w["s"], "ver": version, "ct": ct,
            }))
        })
        .collect()
}

/// Open the hub's sealed rows into the plaintext rows [`Mirror::apply`]
/// merges. A row that does not open, or whose names are not the ones it was
/// sealed under, is refused: left out and counted.
#[cfg(feature = "seal")]
pub fn open_rows(
    answer: &serde_json::Value,
    keyring: &krate_orgkeys::Keyring,
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
                .or_insert_with(|| AppDataKey::derive(keyring, version, app_id))
                .as_ref()?;
            let plain = dk.open(r, k, kind, row["ct"].as_str()?).ok()?;
            let p: serde_json::Value = serde_json::from_slice(&plain).ok()?;
            let (c, i, f) = (p["c"].as_str()?, p["i"].as_str()?, p["f"].as_str()?);
            let kind_matches = match kind {
                RowKind::Tombstone => f == merge::TOMBSTONE,
                RowKind::Null => f != merge::TOMBSTONE && p["v"].is_null(),
                RowKind::Value => f != merge::TOMBSTONE && !p["v"].is_null(),
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
