//! Stage S4: an app's `store.sql` on every device of the person, by row,
//! under the same rule as its `store.kv` and its files (F-321).
//!
//! A row is one entry. Its name is the table and the row's key -- the
//! primary key's columns, or the rowid for a table without one -- and a
//! version is the whole row: every other column that is not NULL, by name,
//! with its type kept (an integer is not a real, text is not a blob). The
//! replica
//! ([`crate::Replica`]) holds, per row, a [`FileValue`] -- the SHA-256 and
//! size of the row so encoded -- so two devices that wrote the same row
//! agree, and the merge rule, the conflicts and the four choices are
//! exactly `store.kv`'s:
//!
//! - **keep both** puts the other device's row beside this one as a new row
//!   where the table can hold two -- one keyed by its rowid, which takes the
//!   next free one -- and otherwise is refused in words
//!   ([`ResolveError::OneRowOnly`]);
//! - **merge** is offered when the two rows differ only in columns that
//!   hold JSON records or lists on both sides ([`merge_rows`]).
//!
//! **A schema change** never lets one version of an app write over the
//! other's. A row travels with its columns' names, and a device folds in a
//! row only when its own table has every one of them ([`fits`]); any other
//! row waits -- on the hub and on the device -- with a plain note
//! ([`wait_words`]) until the app here is updated. A NULL column is left
//! out of a row, so a newer version's added (empty) column changes no row,
//! and a row from an older version that fits is taken with the newer
//! columns empty. Nothing is lost that way: a row the older device could
//! not read is one it has not seen, so its own edit of that row is made
//! apart from it and meets it as a conflict, never as a silent overwrite.
//!
//! What leaves the device is sealed as a file entry is ([`SqlKey`]): the
//! hub learns neither the app, nor the tables, nor the rows. A row of up to
//! [`INLINE_MAX`] travels inside its entry; a larger one is a body in the
//! hub's object store, as a file's contents are. A row over
//! [`crate::files::FILE_MAX`] stays on the device it is on.

use std::collections::{BTreeMap, BTreeSet};

use krate_appsql::SqlValue;
use krate_orgkeys::{AppDataKey, KeyError, Keyring, RowKind};
use serde_json::Value;

pub use crate::files::{Body, FileValue};
use crate::replica::Version;
use crate::seal::Sealed;

/// The largest row that travels inside its entry; larger ones are a body
/// of their own on the hub (R2), as a file's contents are.
pub const INLINE_MAX: usize = 64 * 1024;

/// The most rows one exchange sends: the hub takes at most 4,096 writes a
/// request (cloud/worker/src/account-sync.js `WRITES_MAX`); the rest go
/// with the next exchange, two seconds later.
pub const SEND_MAX: usize = 1000;

/// The name the rowid goes by in a row's key, for a table that has no
/// primary key of its own.
pub const ROWID: &str = "rowid";

/// "Rows over 16 MB stay on this device", for the person.
pub fn too_big_words() -> String {
    format!(
        "Rows over {} MB stay on this device.",
        crate::files::FILE_MAX / (1024 * 1024)
    )
}

/// One value as it travels: `null`, `["i", 3]`, `["r", 1.5]`, `["t",
/// "milk"]` or `["b", "<base64>"]`, so a value comes back with the type it
/// left with.
pub fn cell_to_json(value: &SqlValue) -> Value {
    match value {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(n) => serde_json::json!(["i", n]),
        SqlValue::Real(n) => match serde_json::Number::from_f64(*n) {
            Some(n) => serde_json::json!(["r", n]),
            // SQLite stores no NaN (it reads back as NULL); an infinity is
            // kept as its words.
            None => serde_json::json!(["r", n.to_string()]),
        },
        SqlValue::Text(text) => serde_json::json!(["t", text]),
        SqlValue::Blob(bytes) => serde_json::json!(["b", krate_orgkeys::b64_encode(bytes)]),
    }
}

pub fn cell_from_json(value: &Value) -> Option<SqlValue> {
    if value.is_null() {
        return Some(SqlValue::Null);
    }
    let pair = value.as_array().filter(|a| a.len() == 2)?;
    Some(match pair[0].as_str()? {
        "i" => SqlValue::Integer(pair[1].as_i64()?),
        "r" => SqlValue::Real(match &pair[1] {
            Value::Number(n) => n.as_f64()?,
            Value::String(text) => text.parse().ok()?,
            _ => return None,
        }),
        "t" => SqlValue::Text(pair[1].as_str()?.to_string()),
        "b" => SqlValue::Blob(krate_orgkeys::b64_decode(pair[1].as_str()?)?),
        _ => return None,
    })
}

/// A row's name: its table and its key's columns with their values,
/// `notes\n[["id",["i",3]]]`. The same on every device for the same row.
pub fn row_key(table: &str, key: &[(String, SqlValue)]) -> String {
    let cells: Vec<Value> = key
        .iter()
        .map(|(column, value)| serde_json::json!([column, cell_to_json(value)]))
        .collect();
    format!("{table}\n{}", Value::Array(cells))
}

/// A row's name read back: the table and the key's columns.
pub fn parse_row_key(key: &str) -> Option<(String, Vec<(String, SqlValue)>)> {
    let (table, cells) = key.split_once('\n')?;
    if table.is_empty() {
        return None;
    }
    let cells: Value = serde_json::from_str(cells).ok()?;
    let mut out = Vec::new();
    for cell in cells.as_array()? {
        let pair = cell.as_array().filter(|a| a.len() == 2)?;
        out.push((pair[0].as_str()?.to_string(), cell_from_json(&pair[1])?));
    }
    if out.is_empty() {
        return None;
    }
    Some((table.to_string(), out))
}

/// A row's other columns as they travel: one JSON object, its columns in
/// order of name and NULL ones left out, so the same row is the same bytes
/// on every device, and adding an empty column changes no row.
pub fn encode_row(columns: &[(String, SqlValue)]) -> Vec<u8> {
    let map: BTreeMap<&str, Value> = columns
        .iter()
        .filter(|(_, value)| *value != SqlValue::Null)
        .map(|(name, value)| (name.as_str(), cell_to_json(value)))
        .collect();
    serde_json::to_vec(&map).expect("a map of JSON values serialises")
}

pub fn decode_row(bytes: &[u8]) -> Option<BTreeMap<String, SqlValue>> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let mut out = BTreeMap::new();
    for (name, cell) in value.as_object()? {
        out.insert(name.clone(), cell_from_json(cell)?);
    }
    Some(out)
}

/// Two rows of one key merged: every column the two agree on, and each one
/// they do not merged as JSON ([`crate::json_merge`]) -- offered only when
/// every such column holds a JSON record or list on both sides. `None`
/// when a column that differs is a plain value (or empty on one side):
/// picking one is what the other choices do.
pub fn merge_rows(mine: &[u8], theirs: &[u8]) -> Option<Vec<u8>> {
    let mine = decode_row(mine)?;
    let theirs = decode_row(theirs)?;
    let names: BTreeSet<&String> = mine.keys().chain(theirs.keys()).collect();
    let mut out = Vec::new();
    let mut merged_any = false;
    for name in names {
        let (ours, other) = (
            mine.get(name).unwrap_or(&SqlValue::Null),
            theirs.get(name).unwrap_or(&SqlValue::Null),
        );
        if ours == other {
            out.push((name.clone(), ours.clone()));
            continue;
        }
        let (SqlValue::Text(a), SqlValue::Text(b)) = (ours, other) else {
            return None;
        };
        let merged = crate::json_merge::merge_bytes(a.as_bytes(), b.as_bytes())?;
        out.push((
            name.clone(),
            SqlValue::Text(String::from_utf8(merged).ok()?),
        ));
        merged_any = true;
    }
    merged_any.then(|| encode_row(&out))
}

fn cell_words(value: &SqlValue, max: usize) -> String {
    match value {
        SqlValue::Null => "empty".to_string(),
        SqlValue::Integer(n) => n.to_string(),
        SqlValue::Real(n) => n.to_string(),
        SqlValue::Text(text) => crate::readable::preview(text.as_bytes(), max),
        SqlValue::Blob(bytes) => crate::readable::size_in_words(bytes.len() as u64),
    }
}

/// Which row it is, for the person: `notes, id 3`.
pub fn key_words(key: &str) -> String {
    match parse_row_key(key) {
        Some((table, cells)) => {
            let cells: Vec<String> = cells
                .iter()
                .map(|(name, value)| format!("{name} {}", cell_words(value, 40)))
                .collect();
            format!("{table}, {}", cells.join(", "))
        }
        None => key.to_string(),
    }
}

/// A row in a few words, for a conflict card: `title: Milk, done: 0`. The
/// columns that differ from `against` come first, so the card shows what
/// changed.
pub fn row_words(row: &[u8], against: Option<&[u8]>, max: usize) -> String {
    let Some(row) = decode_row(row) else {
        return String::new();
    };
    let other = against.and_then(decode_row).unwrap_or_default();
    let mut columns: Vec<(&String, &SqlValue)> = row.iter().collect();
    columns.sort_by_key(|(name, value)| other.get(*name) == Some(*value));
    let mut out = String::new();
    for (name, value) in columns {
        let part = format!("{name}: {}", cell_words(value, max));
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(&part);
        if out.chars().count() >= max {
            break;
        }
    }
    if out.chars().count() > max {
        let cut: String = out.chars().take(max.saturating_sub(1)).collect();
        out = format!("{cut}\u{2026}");
    }
    if out.is_empty() {
        "a row".to_string()
    } else {
        out
    }
}

/// A table whose rows from one device wait here: the other device runs a
/// newer version of the app, whose rows have columns this one's table
/// lacks (or a table this one has not made).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Wait {
    pub table: String,
    /// The device whose rows wait.
    pub device: String,
}

/// Whether a row with `theirs` columns (its key's and every other that is
/// not NULL) can be put in a table here that has `here`: every one of them
/// must be a column of it. A row with fewer is an older version's, and its
/// other columns are empty.
pub fn fits(here: Option<&BTreeSet<String>>, theirs: &BTreeSet<String>) -> bool {
    here.is_some_and(|here| theirs.is_subset(here))
}

/// A wait in words: what the person reads on the app's page and in
/// `krate account sync`. `app` is the app's name, `device` the other
/// device's.
pub fn wait_words(app: &str, device: &str, wait: &Wait) -> String {
    format!(
        "Update {app} on this device to keep it in sync: {device} has a newer version that keeps \
         more in its {}. Changes from {device} wait until then -- nothing is lost.",
        wait.table
    )
}

/// One app's database keys, for one account: a bucket and a key of their
/// own, apart from its `store.kv` and its files.
pub struct SqlKey {
    keyring: Keyring,
    scope: String,
    bucket: String,
}

impl std::fmt::Debug for SqlKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SqlKey({})", self.bucket)
    }
}

const BODY_FIELD: &str = "row body";

/// One version of a row as it came off the hub: the row's name, the
/// version, and the row itself when it travelled inside its entry, or the
/// body that holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub key: String,
    pub version: Version,
    pub row: Option<Vec<u8>>,
    pub body: Option<Body>,
    pub at: Option<u64>,
}

impl SqlKey {
    pub fn derive(keyring: &Keyring, publisher: &str, app_id: &str) -> SqlKey {
        let scope = format!("account sql\n{publisher}\n{app_id}");
        let first = *keyring.versions().first().expect("a keyring has a version");
        let names = AppDataKey::derive(keyring, first, "account buckets")
            .expect("the keyring holds its own first version");
        let bucket = names.record_key("sql bucket", &format!("{publisher}\n{app_id}"));
        SqlKey {
            keyring: keyring.clone(),
            scope,
            bucket,
        }
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    fn data(&self, version: u32) -> Option<AppDataKey> {
        AppDataKey::derive(&self.keyring, version, &self.scope)
    }

    fn current(&self) -> AppDataKey {
        self.data(self.keyring.current().0)
            .expect("the keyring holds its current version")
    }

    /// The name a row's entry goes by on the hub: 64 hex.
    pub fn name_of(&self, key: &str) -> String {
        let first = *self.keyring.versions().first().expect("a version");
        self.data(first)
            .expect("the keyring holds its first version")
            .record_key("row", key)
    }

    fn bound(version: &Version) -> String {
        format!("{}|{}", version.vv.canonical(), version.by)
    }

    /// Seal a large row as a body of its own.
    pub fn seal_body(&self, key: &str, row: &[u8]) -> Result<(Vec<u8>, Body), KeyError> {
        let data = self.current();
        let sealed = data.seal(&self.name_of(key), BODY_FIELD, RowKind::Value, row)?;
        let raw = krate_orgkeys::b64_decode(&sealed).ok_or(KeyError::Malformed("sealed body"))?;
        let blob = crate::files::blob_id(&raw);
        Ok((
            raw,
            Body {
                blob,
                k: data.version(),
                at: None,
            },
        ))
    }

    /// Open a body of `key`, refusing one whose row is not `value`.
    pub fn open_body(
        &self,
        key: &str,
        body: &Body,
        raw: &[u8],
        value: &FileValue,
    ) -> Result<Vec<u8>, KeyError> {
        if crate::files::blob_id(raw) != body.blob {
            return Err(KeyError::Open);
        }
        let data = self.data(body.k).ok_or(KeyError::Open)?;
        let row = data.open(
            &self.name_of(key),
            BODY_FIELD,
            RowKind::Value,
            &krate_orgkeys::b64_encode(raw),
        )?;
        if FileValue::of(&row) != *value {
            return Err(KeyError::Open);
        }
        Ok(row)
    }

    /// Seal one version of a row: inside the entry (`row`), or naming its
    /// body; neither for a deletion.
    pub fn seal(
        &self,
        key: &str,
        version: &Version,
        row: Option<&[u8]>,
        body: Option<&Body>,
        at: Option<u64>,
    ) -> Result<Sealed, KeyError> {
        let data = self.current();
        let deleted = version.value.is_none();
        if deleted != (row.is_none() && body.is_none()) || (row.is_some() && body.is_some()) {
            return Err(KeyError::Malformed(
                "a row version holds its row or names its body",
            ));
        }
        let value = match &version.value {
            Some(v) => {
                Some(String::from_utf8(v.clone()).map_err(|_| KeyError::Malformed("row value"))?)
            }
            None => None,
        };
        let plain = serde_json::json!({
            "k": key,
            "v": value,
            "r": row.map(krate_orgkeys::b64_encode),
            "b": body.map(|b| b.blob.clone()),
            "bk": body.map(|b| b.k),
            "t": at,
        })
        .to_string();
        let c = data.seal(
            &self.name_of(key),
            &Self::bound(version),
            if deleted {
                RowKind::Tombstone
            } else {
                RowKind::Value
            },
            plain.as_bytes(),
        )?;
        Ok(Sealed {
            vv: version.vv.clone(),
            by: version.by.clone(),
            deleted,
            k: data.version(),
            c,
        })
    }

    /// Open one version the hub keeps under `name`. An inline row is
    /// checked against the version's value; a body is checked when it is
    /// fetched ([`SqlKey::open_body`]).
    pub fn open(&self, name: &str, sealed: &Sealed) -> Result<Opened, KeyError> {
        let data = self.data(sealed.k).ok_or(KeyError::Open)?;
        let probe = Version {
            value: None,
            vv: sealed.vv.clone(),
            by: sealed.by.clone(),
        };
        let plain = data.open(
            name,
            &Self::bound(&probe),
            if sealed.deleted {
                RowKind::Tombstone
            } else {
                RowKind::Value
            },
            &sealed.c,
        )?;
        let bad = || KeyError::Malformed("sealed row entry");
        let entry: Value = serde_json::from_slice(&plain).map_err(|_| bad())?;
        let key = entry["k"].as_str().ok_or_else(bad)?.to_string();
        if self.name_of(&key) != name || parse_row_key(&key).is_none() {
            return Err(KeyError::Open);
        }
        let at = entry["t"].as_u64();
        let (value, row, body) = match (&entry["v"], sealed.deleted) {
            (Value::Null, true) => (None, None, None),
            (Value::String(text), false) => {
                let value = FileValue::decode(text.as_bytes()).ok_or_else(bad)?;
                match (entry["r"].as_str(), entry["b"].as_str()) {
                    (Some(row), None) => {
                        let row = krate_orgkeys::b64_decode(row).ok_or_else(bad)?;
                        if FileValue::of(&row) != value {
                            return Err(KeyError::Open);
                        }
                        (Some(text.as_bytes().to_vec()), Some(row), None)
                    }
                    (None, Some(blob)) => {
                        if blob.len() != 64 || !blob.bytes().all(|b| b.is_ascii_hexdigit()) {
                            return Err(bad());
                        }
                        let k = u32::try_from(entry["bk"].as_u64().ok_or_else(bad)?)
                            .map_err(|_| bad())?;
                        (
                            Some(text.as_bytes().to_vec()),
                            None,
                            Some(Body {
                                blob: blob.to_string(),
                                k,
                                at,
                            }),
                        )
                    }
                    _ => return Err(bad()),
                }
            }
            _ => return Err(bad()),
        };
        Ok(Opened {
            key,
            version: Version {
                value,
                vv: sealed.vv.clone(),
                by: sealed.by.clone(),
            },
            row,
            body,
            at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seal::new_account_key;
    use crate::vv::VersionVector;

    const A: &str = "aaaaaaaaaaaaaaaa";

    fn row(cells: &[(&str, SqlValue)]) -> Vec<u8> {
        encode_row(
            &cells
                .iter()
                .map(|(n, v)| (n.to_string(), v.clone()))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn a_row_is_the_same_bytes_on_every_device_and_keeps_its_types() {
        let one = row(&[
            ("title", SqlValue::Text("Milk".into())),
            ("done", SqlValue::Integer(0)),
            ("weight", SqlValue::Real(1.0)),
            ("photo", SqlValue::Blob(vec![1, 2, 3])),
            ("note", SqlValue::Null),
        ]);
        // Another order of columns is the same row.
        let two = row(&[
            ("note", SqlValue::Null),
            ("weight", SqlValue::Real(1.0)),
            ("photo", SqlValue::Blob(vec![1, 2, 3])),
            ("done", SqlValue::Integer(0)),
            ("title", SqlValue::Text("Milk".into())),
        ]);
        assert_eq!(one, two);
        let back = decode_row(&one).unwrap();
        assert_eq!(back["done"], SqlValue::Integer(0), "an integer stays one");
        assert_eq!(back["weight"], SqlValue::Real(1.0), "1.0 stays a real");
        assert_eq!(back["photo"], SqlValue::Blob(vec![1, 2, 3]));
        assert!(!back.contains_key("note"), "an empty column is left out");
        assert_ne!(
            row(&[("n", SqlValue::Integer(1))]),
            row(&[("n", SqlValue::Real(1.0))])
        );

        let key = row_key("notes", &[("id".into(), SqlValue::Integer(3))]);
        let (table, cells) = parse_row_key(&key).unwrap();
        assert_eq!(table, "notes");
        assert_eq!(cells, vec![("id".to_string(), SqlValue::Integer(3))]);
        assert_eq!(key_words(&key), "notes, id 3");
        assert!(parse_row_key("notes").is_none());
        assert!(parse_row_key("notes\n[]").is_none());
    }

    #[test]
    fn merge_is_for_json_columns_only() {
        let mine = row(&[
            ("title", SqlValue::Text("Trip".into())),
            ("items", SqlValue::Text(r#"["passport"]"#.into())),
        ]);
        let theirs = row(&[
            ("title", SqlValue::Text("Trip".into())),
            ("items", SqlValue::Text(r#"["tickets"]"#.into())),
        ]);
        let merged = decode_row(&merge_rows(&mine, &theirs).unwrap()).unwrap();
        assert_eq!(
            merged["items"],
            SqlValue::Text(r#"["passport","tickets"]"#.into())
        );
        // A plain column that differs: nothing to merge.
        let plain = row(&[
            ("title", SqlValue::Text("Holiday".into())),
            ("items", SqlValue::Text(r#"["tickets"]"#.into())),
        ]);
        assert!(merge_rows(&mine, &plain).is_none());
        // A column set on one side only is a plain difference.
        let wider = row(&[
            ("title", SqlValue::Text("Trip".into())),
            ("items", SqlValue::Text(r#"["tickets"]"#.into())),
            ("color", SqlValue::Text("red".into())),
        ]);
        assert!(merge_rows(&mine, &wider).is_none());
        // The same row: nothing to merge either.
        assert!(merge_rows(&mine, &mine).is_none());
    }

    #[test]
    fn a_row_fits_a_table_that_has_every_column_it_brings() {
        let set = |c: &[&str]| c.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        let here = set(&["id", "title", "done"]);
        assert!(fits(Some(&here), &set(&["id", "done", "title"])));
        assert!(
            fits(Some(&here), &set(&["id", "title"])),
            "an older version's row: its other columns are empty"
        );
        assert!(
            !fits(Some(&here), &set(&["id", "title", "color"])),
            "a newer version's"
        );
        assert!(
            !fits(None, &set(&["id"])),
            "a table this device has not made"
        );
        // An empty column is no column: adding one changes no row.
        assert_eq!(
            row(&[
                ("title", SqlValue::Text("Milk".into())),
                ("color", SqlValue::Null)
            ]),
            row(&[("title", SqlValue::Text("Milk".into()))])
        );
        let words = wait_words(
            "Reminders",
            "Pixel",
            &Wait {
                table: "items".into(),
                device: A.into(),
            },
        );
        assert!(
            words.starts_with("Update Reminders on this device"),
            "{words}"
        );
    }

    #[test]
    fn a_row_seals_inline_or_as_a_body_and_opens_only_as_itself() {
        let ring = new_account_key("u1");
        let key = SqlKey::derive(&ring, "unsigned", "dev.krate.notes");
        let name = row_key("notes", &[("id".into(), SqlValue::Integer(1))]);
        let bytes = row(&[("title", SqlValue::Text("secret milk".into()))]);
        let mut vv = VersionVector::new();
        vv.bump(A);
        let version = Version {
            value: Some(FileValue::of(&bytes).encode()),
            vv,
            by: A.into(),
        };
        let sealed = key
            .seal(&name, &version, Some(&bytes), None, Some(5))
            .unwrap();
        let opened = key.open(&key.name_of(&name), &sealed).unwrap();
        assert_eq!(opened.key, name);
        assert_eq!(opened.row.as_deref(), Some(bytes.as_slice()));
        assert_eq!(opened.version, version);
        // The hub sees none of it.
        let wire = sealed.to_json().to_string();
        for plain in ["secret milk", "notes", "dev.krate.notes"] {
            assert!(!wire.contains(plain));
            assert!(!wire.contains(&krate_orgkeys::b64_encode(plain.as_bytes())));
        }
        // Moved to another row's name, it does not open.
        let other = row_key("notes", &[("id".into(), SqlValue::Integer(2))]);
        assert!(key.open(&key.name_of(&other), &sealed).is_err());
        // Another app's key does not open it either.
        let elsewhere = SqlKey::derive(&ring, "unsigned", "dev.krate.other");
        assert!(elsewhere.open(&key.name_of(&name), &sealed).is_err());
        assert_ne!(elsewhere.bucket(), key.bucket());

        // As a body.
        let (raw, body) = key.seal_body(&name, &bytes).unwrap();
        let sealed = key.seal(&name, &version, None, Some(&body), None).unwrap();
        let opened = key.open(&key.name_of(&name), &sealed).unwrap();
        assert_eq!(opened.body.as_ref().map(|b| &b.blob), Some(&body.blob));
        let value = FileValue::of(&bytes);
        assert_eq!(
            key.open_body(&name, opened.body.as_ref().unwrap(), &raw, &value)
                .unwrap(),
            bytes
        );
        // A row that says it is both, or neither, is refused.
        assert!(key
            .seal(&name, &version, Some(&bytes), Some(&body), None)
            .is_err());
        assert!(key.seal(&name, &version, None, None, None).is_err());
    }
}
