//! The merge rule for an organisation's app data (`store.org`, IC-951).
//!
//! Pure: no clock, no disk, no network. The hub implements exactly the same
//! rule in `cloud/worker/src/org-data-merge.js`, and both are tested against
//! one set of vectors (`cloud/worker/test/fixtures/org-data-vectors.json`),
//! so a device and the hub can never disagree about which write won.
//!
//! - **One row per field.** A record is the set of its field rows; a delete
//!   is one more row, the record's tombstone, under the reserved field name
//!   `""`. Two people editing different fields of one record both keep their
//!   change.
//! - **The greater stamp wins, per row.** A stamp is a hybrid logical clock:
//!   wall time in milliseconds, a counter, and the writing device's id. A
//!   device stamps a new write above every stamp it has seen, so an edit
//!   made after seeing a value always beats that value, whatever the clocks
//!   say. Applying the same rows in any order gives the same result.
//! - **A wrong clock does not win.** A push carries the device's own idea of
//!   "now"; the hub moves the push's stamps by the difference between its
//!   clock and the device's ([`rebase`]), so what counts is how long ago the
//!   device made each edit, measured on the device's own clock, which a
//!   wrong setting does not change. A stamp can never be later than the hub
//!   receiving it. Clocks within [`DRIFT_MS`] of the hub are left alone.
//! - **A field is live** while its stamp is above the record's tombstone and
//!   its value is not null; writing null removes one field.

use std::cmp::Ordering;
use std::fmt;

/// Clocks this close to the hub's are taken as right: their stamps are not
/// moved, so ordinary network delay never reorders anything.
pub const DRIFT_MS: u64 = 2_000;

/// The largest record, counting its live fields' names and values.
pub const MAX_RECORD_BYTES: usize = 64 * 1024;

/// The longest collection name, record id or field name, in bytes.
pub const MAX_NAME_BYTES: usize = 128;

/// The field name of a record's tombstone. Never a name an app can use.
pub const TOMBSTONE: &str = "";

/// The largest wall time a stamp may carry: what a JavaScript number holds
/// exactly, so the hub reads every stamp the way the device wrote it.
pub const MAX_WALL: u64 = 9_007_199_254_740_991;

const MAX_COUNTER: u32 = 99_999_999;

/// A hybrid logical clock reading: wall milliseconds, a counter, and the
/// device that made it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Stamp {
    pub wall: u64,
    pub counter: u32,
    pub device: String,
}

impl Ord for Stamp {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.wall, self.counter, &self.device).cmp(&(other.wall, other.counter, &other.device))
    }
}

impl PartialOrd for Stamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Stamp {
    /// `wwwwwwwwwwwwwwww-cccccccc-dddddddddddddddd`: fixed widths, so two
    /// stamps compare as text exactly as they compare as clocks. The hub
    /// relies on that to compare them in SQL.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016}-{:08}-{}", self.wall, self.counter, self.device)
    }
}

/// Is this a device id: 16 lower-case hex digits?
pub fn valid_device(device: &str) -> bool {
    device.len() == 16
        && device
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl Stamp {
    /// Read a stamp in its one text form, or `None`.
    pub fn parse(text: &str) -> Option<Stamp> {
        let bytes = text.as_bytes();
        if bytes.len() != 16 + 1 + 8 + 1 + 16 || bytes[16] != b'-' || bytes[25] != b'-' {
            return None;
        }
        let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
        let (wall, counter, device) = (&text[..16], &text[17..25], &text[26..]);
        if !digits(wall) || !digits(counter) || !valid_device(device) {
            return None;
        }
        let wall: u64 = wall.parse().ok()?;
        if wall > MAX_WALL {
            return None;
        }
        Some(Stamp {
            wall,
            counter: counter.parse().ok()?,
            device: device.to_string(),
        })
    }

    /// The next stamp for a write made now on `device`: above `last` (every
    /// stamp this device has made or seen), and at `phys` once the clock is
    /// past it.
    pub fn next(last: Option<&Stamp>, phys: u64, device: &str) -> Stamp {
        let phys = phys.min(MAX_WALL);
        match last {
            Some(last) if last.wall >= phys => {
                if last.counter >= MAX_COUNTER {
                    Stamp {
                        wall: (last.wall + 1).min(MAX_WALL),
                        counter: 0,
                        device: device.to_string(),
                    }
                } else {
                    Stamp {
                        wall: last.wall,
                        counter: last.counter + 1,
                        device: device.to_string(),
                    }
                }
            }
            _ => Stamp {
                wall: phys,
                counter: 0,
                device: device.to_string(),
            },
        }
    }
}

/// Move a pushed stamp onto the hub's clock: by how far the device's clock
/// is from the hub's, when that is more than [`DRIFT_MS`]; and never past
/// the moment the hub received it (plus the drift it allows). The device
/// applies the same function to its own copy with the same three inputs,
/// so both hold the same stamp afterwards.
pub fn rebase(stamp: &Stamp, device_now: u64, hub_now: u64) -> Stamp {
    let delta = i128::from(hub_now) - i128::from(device_now);
    let shifted = if delta.unsigned_abs() <= u128::from(DRIFT_MS) {
        i128::from(stamp.wall)
    } else {
        i128::from(stamp.wall) + delta
    };
    let ceiling = i128::from(hub_now.saturating_add(DRIFT_MS).min(MAX_WALL));
    let wall = shifted.clamp(0, ceiling) as u64;
    Stamp {
        wall,
        counter: stamp.counter,
        device: stamp.device.clone(),
    }
}

/// One field's value, as the WIT `value` variant has it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Text(String),
    Number(f64),
    Boolean(bool),
    Bytes(Vec<u8>),
}

impl Value {
    /// What it costs against the record's size: its bytes, 8 for a number,
    /// 1 for a flag, nothing for null.
    pub fn size(&self) -> usize {
        match self {
            Value::Null => 0,
            Value::Text(text) => text.len(),
            Value::Number(_) => 8,
            Value::Boolean(_) => 1,
            Value::Bytes(bytes) => bytes.len(),
        }
    }

    /// The wire form: `null`, `{"t": text}`, `{"n": number}`, `{"b": flag}`
    /// or `{"x": base64}`.
    pub fn to_wire(&self) -> serde_json::Value {
        match self {
            Value::Null => serde_json::Value::Null,
            Value::Text(text) => serde_json::json!({ "t": text }),
            Value::Number(n) => serde_json::json!({ "n": n }),
            Value::Boolean(b) => serde_json::json!({ "b": b }),
            Value::Bytes(bytes) => {
                serde_json::json!({ "x": crate::b64_encode(bytes) })
            }
        }
    }

    /// Read the wire form, or `None` for anything else.
    pub fn from_wire(wire: &serde_json::Value) -> Option<Value> {
        if wire.is_null() {
            return Some(Value::Null);
        }
        let object = wire.as_object()?;
        if object.len() != 1 {
            return None;
        }
        let (kind, inner) = object.iter().next()?;
        match kind.as_str() {
            "t" => inner.as_str().map(|t| Value::Text(t.to_string())),
            "n" => inner.as_f64().filter(|n| n.is_finite()).map(Value::Number),
            "b" => inner.as_bool().map(Value::Boolean),
            "x" => inner.as_str().and_then(crate::b64_decode).map(Value::Bytes),
            _ => None,
        }
    }
}

/// Is `name` a collection name, record id or field name an app may use: 1
/// to [`MAX_NAME_BYTES`] bytes and no control characters? `None` when it
/// is, otherwise what is wrong, in words.
pub fn name_problem(what: &str, name: &str) -> Option<String> {
    if name.is_empty() {
        return Some(format!("a {what} cannot be empty"));
    }
    if name.len() > MAX_NAME_BYTES {
        return Some(format!(
            "a {what} is at most {MAX_NAME_BYTES} bytes, and this one is {}",
            name.len()
        ));
    }
    if name.chars().any(char::is_control) {
        return Some(format!("a {what} cannot contain control characters"));
    }
    None
}

/// One field row as both sides hold it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub value: Value,
    pub stamp: Stamp,
}

/// Does `incoming` replace `held`? Only a strictly greater stamp does, so
/// the same row arriving twice changes nothing.
pub fn wins(incoming: &Stamp, held: Option<&Stamp>) -> bool {
    held.is_none_or(|held| incoming > held)
}

/// Is a field with this stamp alive under the record's tombstone?
pub fn alive(stamp: &Stamp, tombstone: Option<&Stamp>) -> bool {
    tombstone.is_none_or(|t| stamp > t)
}

/// A record's live fields, by name: above the tombstone and not null.
pub fn live_fields<'a, I>(cells: I) -> Vec<(&'a str, &'a Cell)>
where
    I: IntoIterator<Item = (&'a str, &'a Cell)> + Clone,
{
    let tombstone = cells
        .clone()
        .into_iter()
        .find(|(name, _)| *name == TOMBSTONE)
        .map(|(_, cell)| &cell.stamp);
    let mut live: Vec<(&str, &Cell)> = cells
        .into_iter()
        .filter(|(name, cell)| {
            *name != TOMBSTONE && cell.value != Value::Null && alive(&cell.stamp, tombstone)
        })
        .collect();
    live.sort_by(|a, b| a.0.cmp(b.0));
    live
}

/// A record's size as the 64 KiB limit counts it: its live fields' names
/// and values.
pub fn record_size<'a, I>(cells: I) -> usize
where
    I: IntoIterator<Item = (&'a str, &'a Cell)> + Clone,
{
    live_fields(cells)
        .iter()
        .map(|(name, cell)| name.len() + cell.value.size())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const DEV_A: &str = "aaaaaaaaaaaaaaaa";
    const DEV_B: &str = "bbbbbbbbbbbbbbbb";

    fn stamp(wall: u64, counter: u32, device: &str) -> Stamp {
        Stamp {
            wall,
            counter,
            device: device.to_string(),
        }
    }

    /// The vectors the hub's tests read too: one file, two implementations.
    fn vectors() -> serde_json::Value {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../cloud/worker/test/fixtures/org-data-vectors.json"
        );
        let text = std::fs::read_to_string(path).expect("the shared merge vectors");
        serde_json::from_str(&text).expect("vectors are JSON")
    }

    fn s(v: &serde_json::Value) -> Stamp {
        Stamp::parse(v.as_str().expect("stamp text")).expect("stamp parses")
    }

    #[test]
    fn stamps_print_and_parse_and_compare_as_text() {
        let a = stamp(1_760_400_000_000, 7, DEV_A);
        assert_eq!(a.to_string(), "0001760400000000-00000007-aaaaaaaaaaaaaaaa");
        assert_eq!(Stamp::parse(&a.to_string()), Some(a.clone()));
        let v = vectors();
        for case in v["parse"].as_array().unwrap() {
            let text = case["text"].as_str().unwrap();
            assert_eq!(
                Stamp::parse(text).is_some(),
                case["ok"].as_bool().unwrap(),
                "{text}"
            );
        }
        for case in v["compare"].as_array().unwrap() {
            let (x, y) = (s(&case[0]), s(&case[1]));
            let want = case[2].as_i64().unwrap();
            let got = match x.cmp(&y) {
                Ordering::Less => -1,
                Ordering::Equal => 0,
                Ordering::Greater => 1,
            };
            assert_eq!(got, want, "{case}");
            // Text order is clock order.
            assert_eq!(
                x.to_string().cmp(&y.to_string()),
                x.cmp(&y),
                "text order of {case}"
            );
        }
    }

    #[test]
    fn next_climbs_above_everything_seen() {
        let v = vectors();
        for case in v["next"].as_array().unwrap() {
            let last = case["last"].as_str().map(|t| Stamp::parse(t).unwrap());
            let got = Stamp::next(
                last.as_ref(),
                case["phys"].as_u64().unwrap(),
                case["device"].as_str().unwrap(),
            );
            assert_eq!(got.to_string(), case["expect"].as_str().unwrap(), "{case}");
            if let Some(last) = last {
                assert!(got > last, "a new write is above what was seen: {case}");
            }
        }
    }

    #[test]
    fn rebase_matches_the_vectors() {
        let v = vectors();
        for case in v["rebase"].as_array().unwrap() {
            let got = rebase(
                &s(&case["stamp"]),
                case["device_now"].as_u64().unwrap(),
                case["hub_now"].as_u64().unwrap(),
            );
            assert_eq!(
                got.to_string(),
                case["expect"].as_str().unwrap(),
                "{}",
                case["why"]
            );
        }
    }

    /// Apply rows to a store of records with the per-row rule.
    fn apply(store: &mut BTreeMap<(String, String, String), Cell>, rows: &[serde_json::Value]) {
        for row in rows {
            let key = (
                row["c"].as_str().unwrap().to_string(),
                row["i"].as_str().unwrap().to_string(),
                row["f"].as_str().unwrap().to_string(),
            );
            let cell = Cell {
                value: Value::from_wire(&row["v"]).expect("wire value"),
                stamp: s(&row["s"]),
            };
            if wins(&cell.stamp, store.get(&key).map(|c| &c.stamp)) {
                store.insert(key, cell);
            }
        }
    }

    fn view(
        store: &BTreeMap<(String, String, String), Cell>,
        collection: &str,
        id: &str,
    ) -> serde_json::Value {
        let cells: Vec<(&str, &Cell)> = store
            .iter()
            .filter(|((c, i, _), _)| c == collection && i == id)
            .map(|((_, _, f), cell)| (f.as_str(), cell))
            .collect();
        let live = live_fields(cells.iter().copied());
        if live.is_empty() {
            return serde_json::Value::Null;
        }
        let mut out = serde_json::Map::new();
        for (name, cell) in live {
            out.insert(name.to_string(), cell.value.to_wire());
        }
        serde_json::Value::Object(out)
    }

    #[test]
    fn merges_match_the_vectors_in_every_order() {
        let v = vectors();
        for case in v["merge"].as_array().unwrap() {
            let rows = case["rows"].as_array().unwrap();
            let mut forward = BTreeMap::new();
            apply(&mut forward, rows);
            let mut backward = BTreeMap::new();
            let reversed: Vec<serde_json::Value> = rows.iter().rev().cloned().collect();
            apply(&mut backward, &reversed);
            // Twice is once.
            apply(&mut backward, &reversed);
            for (id, want) in case["expect"].as_object().unwrap() {
                assert_eq!(&view(&forward, "c", id), want, "{} {id}", case["name"]);
                assert_eq!(
                    &view(&backward, "c", id),
                    want,
                    "{} {id}, reversed",
                    case["name"]
                );
            }
            if let Some(size) = case.get("size") {
                let cells: Vec<(&str, &Cell)> = forward
                    .iter()
                    .map(|((_, _, f), cell)| (f.as_str(), cell))
                    .collect();
                assert_eq!(
                    record_size(cells.iter().copied()) as u64,
                    size.as_u64().unwrap(),
                    "{}",
                    case["name"]
                );
            }
        }
    }

    /// The device with a clock a day fast: its earlier offline edit does
    /// not beat a later one from a device whose clock is right.
    #[test]
    fn a_device_a_day_fast_does_not_win() {
        let day = 86_400_000u64;
        let t0 = 1_760_400_000_000u64;
        // B edits at t0 by the real clock, but its clock says t0 + 1 day.
        let b_edit = Stamp::next(None, t0 + day, DEV_B);
        // A edits ten seconds later, with a right clock, and pushes first.
        let a_edit = Stamp::next(None, t0 + 10_000, DEV_A);
        let a_stored = rebase(&a_edit, t0 + 20_000, t0 + 20_000);
        // B pushes thirty seconds after its edit, by its own clock.
        let b_stored = rebase(&b_edit, t0 + day + 30_000, t0 + 30_000);
        assert_eq!(b_stored.wall, t0, "B's edit lands at the time it was made");
        assert!(a_stored > b_stored, "the later edit wins");
        assert!(!wins(&b_stored, Some(&a_stored)));
        // Without the rebase the fast clock would have won: that is what it guards.
        assert!(b_edit > a_edit);
    }

    #[test]
    fn values_round_trip_on_the_wire_and_refuse_what_is_not_one() {
        for value in [
            Value::Null,
            Value::Text("Dana".into()),
            Value::Number(12.5),
            Value::Boolean(true),
            Value::Bytes(vec![0, 1, 254, 255]),
        ] {
            assert_eq!(Value::from_wire(&value.to_wire()), Some(value.clone()));
        }
        for bad in [
            serde_json::json!({ "t": 1 }),
            serde_json::json!({ "q": "x" }),
            serde_json::json!({ "t": "a", "n": 1 }),
            serde_json::json!("plain"),
            serde_json::json!({ "x": "not base64!" }),
        ] {
            assert_eq!(Value::from_wire(&bad), None, "{bad}");
        }
    }

    #[test]
    fn names_are_bounded() {
        assert!(name_problem("collection name", "tasks").is_none());
        assert!(name_problem("collection name", "").is_some());
        assert!(name_problem("collection name", &"x".repeat(129)).is_some());
        assert!(name_problem("collection name", &"x".repeat(128)).is_none());
        assert!(name_problem("record id", "a\nb").is_some());
    }
}
