//! An app's `store.sql` kept the same on every device a person signs in on
//! (IC-981 S4, F-321), by row, with no code in the app.
//!
//! When the run is synced and the app was granted `store.sql`, a thread
//! looks at the app's database at open, every [`PERIOD`] while it runs (and
//! soon after its first write, so a fresh device fills as soon as the app
//! has made its tables) and once more at close, and exchanges the rows that
//! changed with the hub. It reads through a connection of its own: the app
//! keeps its connection, its statements and its answers exactly as before.
//!
//! A row is one entry, named by its table and its key -- the primary key,
//! or the rowid of a table without one -- and a version is the whole row
//! (crates/accountsync `sql`). The rule is `store.kv`'s, per row: one side
//! changed, take it; both changed to the same row, take it; both changed
//! differently, or one deleted what the other changed, a conflict -- the
//! app goes on reading this device's row, the other device's waits beside
//! the store until the person chooses: this device's, the other's, both
//! (the other's as a new row, where the table can hold two), or a merge of
//! the JSON columns.
//!
//! What it never does:
//!
//! - **write over a row the app changed meanwhile.** Rows are put in place
//!   in one transaction that first checks each row is still what the last
//!   look saw; one the app changed is left, and its change goes up next.
//! - **write a row from another version of the app.** A row is taken only
//!   when this device's table has exactly its columns; the rest wait, with
//!   words that say which device to update (`sql::wait_words`).
//! - **run the app's triggers or foreign-key actions** on what it puts in
//!   place: those already ran on the device that made the change, and their
//!   results travel as rows of their own.
//!
//! Beside the store: `<store>.sync/<slot>.sql.json` (the replica, where
//! each large row's body is on the hub, the database's path, what waits)
//! and `<store>.sync/<slot>.sql.held/` (other devices' rows). The toggles
//! are the store's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use krate_accountsync::files::FILE_MAX;
use krate_accountsync::hub::HubError;
use krate_accountsync::replica::{KeyState, Outgoing};
use krate_accountsync::seal::Sealed;
use krate_accountsync::sql::{
    self, decode_row, encode_row, fits, merge_rows, parse_row_key, row_key, Body, FileValue,
    SqlKey, Wait, INLINE_MAX, SEND_MAX,
};
use krate_accountsync::toggles::{sql_held_dir, sql_state_path, sync_wanted};
use krate_accountsync::{Replica, Resolution, ResolveError, Version, VersionVector};
use krate_appsql::rusqlite::{self, types::ValueRef, Connection, OpenFlags};
use krate_appsql::SqlValue;

use crate::account_sync_host::{spawn_loop, AccountSyncContext, Session, Step, Wake, CLOSE_WAIT};

/// Where one version's row is on the hub, when it is a body of its own.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct KnownBody {
    blob: String,
    k: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at: Option<u64>,
}

/// Everything sync keeps about one app's database for one account.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct SqlState {
    replica: Replica,
    /// Per row, per row hash: the body the hub keeps for a large row.
    #[serde(default)]
    bodies: BTreeMap<String, BTreeMap<String, KnownBody>>,
    /// The database as the last run knew it: so a choice made with the app
    /// closed can be put in place.
    #[serde(default)]
    database: Option<PathBuf>,
    /// Tables whose rows from another device wait here, and why.
    #[serde(default)]
    waits: Vec<Wait>,
    /// Rows this device keeps to itself: over [`FILE_MAX`].
    #[serde(default)]
    kept_here: Vec<String>,
}

impl SqlState {
    fn new(device: &str) -> SqlState {
        SqlState {
            replica: Replica::new(device),
            bodies: BTreeMap::new(),
            database: None,
            waits: Vec::new(),
            kept_here: Vec::new(),
        }
    }
}

fn read_state(path: &Path) -> Option<SqlState> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn write_state(path: &Path, state: &SqlState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let text = serde_json::to_string(state).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| e.to_string())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One table as sync reads it.
#[derive(Clone, Debug)]
struct Table {
    /// The columns that name a row: the primary key's, or `rowid`.
    key: Vec<String>,
    /// Every other column, in the table's order.
    values: Vec<String>,
    /// A row is named by its rowid, so a second copy can take a new one
    /// ("keep both").
    by_rowid: bool,
}

impl Table {
    /// Every column a row of it travels with, by name.
    fn columns(&self) -> BTreeSet<String> {
        self.key.iter().chain(&self.values).cloned().collect()
    }
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The app's database, through a connection of sync's own: never created
/// (an app that has not run a statement has none), never running the app's
/// triggers or foreign-key actions, and waiting its turn behind the app.
fn open_db(path: &Path) -> Result<Option<Connection>, String> {
    if !path.is_file() {
        return Ok(None);
    }
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    use rusqlite::config::DbConfig;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)
        .map_err(|e| e.to_string())?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_FKEY, false)
        .map_err(|e| e.to_string())?;
    connection
        .execute_batch("PRAGMA trusted_schema = OFF;")
        .map_err(|e| e.to_string())?;
    Ok(Some(connection))
}

/// Has anyone else committed to the database since this connection last
/// asked? (SQLite's `data_version`: unchanged by this connection's own
/// writes.)
fn data_version(connection: &Connection) -> Option<i64> {
    connection
        .query_row("PRAGMA data_version", [], |row| row.get(0))
        .ok()
}

/// The app's tables, by name: ordinary tables only -- not SQLite's own,
/// not a virtual table or the shadow tables behind one (their rows follow
/// from the table they serve).
fn tables(connection: &Connection) -> Result<BTreeMap<String, Table>, String> {
    let mut list = connection
        .prepare(
            "SELECT name, wr FROM pragma_table_list \
             WHERE schema = 'main' AND type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'",
        )
        .map_err(|e| e.to_string())?;
    let names: Vec<(String, bool)> = list
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0))
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    let mut out = BTreeMap::new();
    for (name, without_rowid) in names {
        let mut info = connection
            .prepare("SELECT name, type, pk, hidden FROM pragma_table_xinfo(?1) ORDER BY cid")
            .map_err(|e| e.to_string())?;
        let columns: Vec<(String, String, i64, i64)> = info
            .query_map([&name], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(|e| e.to_string())?
            .flatten()
            .collect();
        // A generated column follows from the others; a hidden one is not
        // the app's to write.
        let plain: Vec<&(String, String, i64, i64)> = columns.iter().filter(|c| c.3 == 0).collect();
        let mut pk: Vec<&(String, String, i64, i64)> =
            plain.iter().copied().filter(|c| c.2 > 0).collect();
        pk.sort_by_key(|c| c.2);
        let (key, by_rowid) = if pk.is_empty() {
            if without_rowid || plain.iter().any(|c| c.0.eq_ignore_ascii_case(sql::ROWID)) {
                // No key sync can name a row by: kept on this device.
                continue;
            }
            (vec![sql::ROWID.to_string()], true)
        } else {
            let alias = !without_rowid && pk.len() == 1 && pk[0].1.eq_ignore_ascii_case("INTEGER");
            (pk.iter().map(|c| c.0.clone()).collect(), alias)
        };
        let values = plain
            .iter()
            .filter(|c| !key.contains(&c.0))
            .map(|c| c.0.clone())
            .collect();
        out.insert(
            name,
            Table {
                key,
                values,
                by_rowid,
            },
        );
    }
    Ok(out)
}

fn cell(value: ValueRef<'_>) -> SqlValue {
    match value {
        ValueRef::Null => SqlValue::Null,
        ValueRef::Integer(n) => SqlValue::Integer(n),
        ValueRef::Real(n) => SqlValue::Real(n),
        ValueRef::Text(bytes) => SqlValue::Text(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => SqlValue::Blob(bytes.to_vec()),
    }
}

fn bind(value: &SqlValue) -> rusqlite::types::Value {
    match value {
        SqlValue::Null => rusqlite::types::Value::Null,
        SqlValue::Integer(n) => rusqlite::types::Value::Integer(*n),
        SqlValue::Real(n) => rusqlite::types::Value::Real(*n),
        SqlValue::Text(text) => rusqlite::types::Value::Text(text.clone()),
        SqlValue::Blob(bytes) => rusqlite::types::Value::Blob(bytes.clone()),
    }
}

fn select_list(table: &Table) -> String {
    table
        .key
        .iter()
        .chain(&table.values)
        .map(|c| {
            if c == sql::ROWID && table.by_rowid && table.key.len() == 1 && table.key[0] == *c {
                "rowid".to_string()
            } else {
                quote(c)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn key_column(table: &Table, column: &str) -> String {
    if column == sql::ROWID && table.key == [sql::ROWID] {
        "rowid".to_string()
    } else {
        quote(column)
    }
}

/// Every row of `name`: its key and its encoded row.
fn read_rows(
    connection: &Connection,
    name: &str,
    table: &Table,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let statement = format!("SELECT {} FROM {}", select_list(table), quote(name));
    let mut prepared = connection.prepare(&statement).map_err(|e| e.to_string())?;
    let mut rows = prepared.query([]).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let k = table.key.len();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut key = Vec::with_capacity(k);
        for (i, column) in table.key.iter().enumerate() {
            key.push((
                column.clone(),
                cell(row.get_ref(i).map_err(|e| e.to_string())?),
            ));
        }
        let mut values = Vec::with_capacity(table.values.len());
        for (i, column) in table.values.iter().enumerate() {
            values.push((
                column.clone(),
                cell(row.get_ref(k + i).map_err(|e| e.to_string())?),
            ));
        }
        out.push((row_key(name, &key), encode_row(&values)));
    }
    Ok(out)
}

fn where_key(table: &Table, key: &[(String, SqlValue)], first: usize) -> String {
    key.iter()
        .enumerate()
        .map(|(i, (column, _))| format!("{} IS ?{}", key_column(table, column), first + i))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// One row as it is now, encoded; `None` when there is none.
fn read_row(connection: &Connection, row: &str) -> Option<Vec<u8>> {
    let (name, key) = parse_row_key(row)?;
    let all = tables(connection).ok()?;
    let table = all.get(&name)?;
    read_row_in(connection, &name, table, &key)
}

fn read_row_in(
    connection: &Connection,
    name: &str,
    table: &Table,
    key: &[(String, SqlValue)],
) -> Option<Vec<u8>> {
    if key.iter().map(|(c, _)| c).ne(table.key.iter()) {
        return None;
    }
    let columns = if table.values.is_empty() {
        "1".to_string()
    } else {
        table
            .values
            .iter()
            .map(|c| quote(c))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let statement = format!(
        "SELECT {columns} FROM {} WHERE {}",
        quote(name),
        where_key(table, key, 1)
    );
    let params: Vec<_> = key.iter().map(|(_, v)| bind(v)).collect();
    connection
        .query_row(&statement, rusqlite::params_from_iter(params), |row| {
            let mut values = Vec::new();
            for (i, column) in table.values.iter().enumerate() {
                values.push((column.clone(), cell(row.get_ref(i)?)));
            }
            Ok(encode_row(&values))
        })
        .ok()
}

/// Put a row in place (`None`: delete it). The caller has checked that the
/// row has exactly the table's columns.
fn write_row(
    connection: &Connection,
    name: &str,
    table: &Table,
    key: &[(String, SqlValue)],
    row: Option<&BTreeMap<String, SqlValue>>,
) -> Result<(), String> {
    let key_params: Vec<_> = key.iter().map(|(_, v)| bind(v)).collect();
    let Some(row) = row else {
        let statement = format!(
            "DELETE FROM {} WHERE {}",
            quote(name),
            where_key(table, key, 1)
        );
        connection
            .execute(&statement, rusqlite::params_from_iter(key_params))
            .map_err(|e| e.to_string())?;
        return Ok(());
    };
    // A column the row does not name is empty (NULL): a row from an older
    // version of the app, or one whose column is empty.
    const EMPTY: SqlValue = SqlValue::Null;
    let values: Vec<(&String, &SqlValue)> = table
        .values
        .iter()
        .map(|c| (c, row.get(c).unwrap_or(&EMPTY)))
        .collect();
    let exists = read_row_in(connection, name, table, key).is_some();
    if exists {
        if values.is_empty() {
            return Ok(());
        }
        let set = values
            .iter()
            .enumerate()
            .map(|(i, (c, _))| format!("{} = ?{}", quote(c), i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let statement = format!(
            "UPDATE {} SET {set} WHERE {}",
            quote(name),
            where_key(table, key, values.len() + 1)
        );
        let params: Vec<_> = values
            .iter()
            .map(|(_, v)| bind(v))
            .chain(key_params)
            .collect();
        connection
            .execute(&statement, rusqlite::params_from_iter(params))
            .map_err(|e| e.to_string())?;
    } else {
        let columns: Vec<String> = key
            .iter()
            .map(|(c, _)| key_column(table, c))
            .chain(values.iter().map(|(c, _)| quote(c)))
            .collect();
        let marks: Vec<String> = (1..=columns.len()).map(|i| format!("?{i}")).collect();
        let statement = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            quote(name),
            columns.join(", "),
            marks.join(", ")
        );
        let params: Vec<_> = key_params
            .into_iter()
            .chain(values.iter().map(|(_, v)| bind(v)))
            .collect();
        connection
            .execute(&statement, rusqlite::params_from_iter(params))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// One row's write, as one savepoint: a row the database refuses (a
/// constraint) is left, and the rest still land.
fn write_one(
    connection: &Connection,
    all: &BTreeMap<String, Table>,
    row: &str,
    bytes: Option<&[u8]>,
) -> Result<(), String> {
    let (name, key) = parse_row_key(row).ok_or("not a row's name")?;
    let Some(table) = all.get(&name) else {
        // Deleting a row of a table this device does not have.
        return match bytes {
            None => Ok(()),
            Some(_) => Err(format!("{name}: no such table here")),
        };
    };
    let decoded = match bytes {
        Some(bytes) => Some(decode_row(bytes).ok_or("not a row")?),
        None => None,
    };
    connection
        .execute_batch("SAVEPOINT krate_sync_row")
        .map_err(|e| e.to_string())?;
    match write_row(connection, &name, table, &key, decoded.as_ref()) {
        Ok(()) => connection
            .execute_batch("RELEASE krate_sync_row")
            .map_err(|e| e.to_string()),
        Err(why) => {
            let _ = connection.execute_batch("ROLLBACK TO krate_sync_row; RELEASE krate_sync_row");
            Err(format!("{}: {why}", sql::key_words(row)))
        }
    }
}

/// A row from another device, waiting here.
fn held_row(held: &Path, hash: &str) -> Option<Vec<u8>> {
    let bytes = std::fs::read(held.join(hash)).ok()?;
    (krate_accountsync::value_hash(&bytes) == hash).then_some(bytes)
}

fn hold_row(held: &Path, hash: &str, bytes: &[u8]) -> Result<(), String> {
    if held_row(held, hash).is_some() {
        return Ok(());
    }
    std::fs::create_dir_all(held).map_err(|e| e.to_string())?;
    let temp = held.join(format!("{hash}.{}.tmp", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&temp, held.join(hash)).map_err(|e| e.to_string())
}

/// Keep only the rows a conflict still waits on.
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

/// A row changed on two devices, in words: which row, this device's side
/// and every other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowConflict {
    /// The row's name in sync (what [`resolve_row_at`] takes).
    pub key: String,
    /// The table it is in.
    pub table: String,
    /// Which row, for the person: "notes, id 3".
    pub place: String,
    /// This device's row in a few words; `None` when deleted here.
    pub mine: Option<String>,
    /// Each other device's (`None`: deleted there) and its name.
    pub others: Vec<(Option<String>, String)>,
    /// Every column that differs holds JSON on both sides.
    pub mergeable: bool,
    /// The table can hold the other's row beside this one.
    pub both: bool,
}

/// What the person sees about one app's database.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SqlStatus {
    pub waiting_for_approval: bool,
    pub last_sync: Option<SystemTime>,
    pub last_error: Option<String>,
    pub conflicts: usize,
    /// Tables whose rows from another device wait for an update.
    pub waits: Vec<Wait>,
    /// Rows this device keeps to itself (too large).
    pub kept_here: Vec<String>,
    /// Rows from other devices the database refused, and why.
    pub refused: Vec<String>,
}

/// How long a side of a conflict card may be, in characters.
const WORDS: usize = 140;

fn name_or(names: &BTreeMap<String, String>, device: &str) -> String {
    names
        .get(device)
        .filter(|n| !n.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| "other device".to_string())
}

fn conflicts_of(
    state: &SqlState,
    connection: Option<&Connection>,
    held: &Path,
    names: &BTreeMap<String, String>,
) -> Vec<RowConflict> {
    let all = connection.and_then(|c| tables(c).ok()).unwrap_or_default();
    state
        .replica
        .conflicts()
        .into_iter()
        .map(|c| {
            let mine = if c.mine_deleted {
                None
            } else {
                connection.and_then(|db| read_row(db, &c.key))
            };
            let theirs: Vec<Option<Vec<u8>>> = c
                .others
                .iter()
                .map(|v| {
                    let value = v.value.as_deref().and_then(FileValue::decode)?;
                    held_row(held, &value.hash)
                })
                .collect();
            let first = theirs.first().cloned().flatten();
            let others = c
                .others
                .iter()
                .zip(&theirs)
                .map(|(v, row)| {
                    (
                        row.as_deref()
                            .map(|r| sql::row_words(r, mine.as_deref(), WORDS)),
                        name_or(names, &v.by),
                    )
                })
                .collect();
            let table = parse_row_key(&c.key).map(|(t, _)| t).unwrap_or_default();
            RowConflict {
                place: sql::key_words(&c.key),
                mine: mine
                    .as_deref()
                    .map(|r| sql::row_words(r, first.as_deref(), WORDS)),
                others,
                mergeable: merged(mine.as_deref(), &theirs).is_some(),
                both: all.get(&table).is_some_and(|t| t.by_rowid),
                table,
                key: c.key,
            }
        })
        .collect()
}

/// This device's row merged with every other device's, when every column
/// that differs holds JSON.
fn merged(mine: Option<&[u8]>, theirs: &[Option<Vec<u8>>]) -> Option<Vec<u8>> {
    let mut value = mine?.to_vec();
    if theirs.is_empty() {
        return None;
    }
    for other in theirs {
        value = merge_rows(&value, other.as_deref()?)?;
    }
    Some(value)
}

/// Answer a conflict on `row`: put the chosen row (or rows) in place and
/// settle the replica. It goes up with the next exchange.
fn resolve_in(
    state: &mut SqlState,
    connection: &Connection,
    held: &Path,
    names: &BTreeMap<String, String>,
    row: &str,
    choice: &Resolution,
) -> Result<(), ResolveError> {
    let key_state = state
        .replica
        .keys
        .get(row)
        .ok_or(ResolveError::NoConflict)?
        .clone();
    if key_state.others.is_empty() {
        return Err(ResolveError::NoConflict);
    }
    let all = tables(connection).map_err(|_| ResolveError::NotHereYet)?;
    let (name, _) = parse_row_key(row).ok_or(ResolveError::NoConflict)?;
    let mine_bytes = match key_state.hash {
        Some(_) => read_row(connection, row),
        None => None,
    };
    let theirs: Vec<Option<Vec<u8>>> = key_state
        .others
        .iter()
        .map(|o| {
            let value = FileValue::decode(&o.value()?)?;
            held_row(held, &value.hash)
        })
        .collect();
    let in_transaction = |writes: &[(String, Option<Vec<u8>>)]| -> Result<(), ResolveError> {
        connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| ResolveError::NotHereYet)?;
        for (key, bytes) in writes {
            if write_one(connection, &all, key, bytes.as_deref()).is_err() {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(ResolveError::NotHereYet);
            }
        }
        connection
            .execute_batch("COMMIT")
            .map_err(|_| ResolveError::NotHereYet)
    };
    if let Resolution::Merge = choice {
        let bytes = merged(mine_bytes.as_deref(), &theirs).ok_or(ResolveError::NotMergeable)?;
        in_transaction(&[(row.to_string(), Some(bytes.clone()))])?;
        state
            .replica
            .settle(row, Some(&FileValue::of(&bytes).encode()))?;
        return Ok(());
    }
    // Every row the choice puts in place must be on this device first.
    let needed: Vec<usize> = match choice {
        Resolution::KeepOther(i) => {
            if *i >= key_state.others.len() {
                return Err(ResolveError::NoSuchVersion);
            }
            vec![*i]
        }
        Resolution::KeepBoth => {
            if !all.get(&name).is_some_and(|t| t.by_rowid) {
                return Err(ResolveError::OneRowOnly);
            }
            (0..key_state.others.len()).collect()
        }
        _ => Vec::new(),
    };
    if needed
        .iter()
        .any(|i| key_state.others[*i].v.is_some() && theirs[*i].is_none())
    {
        return Err(ResolveError::NotHereYet);
    }
    let mut by_hash: BTreeMap<String, Vec<u8>> = theirs
        .iter()
        .flatten()
        .map(|r| (FileValue::of(r).hash, r.clone()))
        .collect();
    if let Some(mine) = &mine_bytes {
        by_hash.insert(FileValue::of(mine).hash, mine.clone());
    }
    // "Keep both": the other's row under the next free rowid, past every
    // row this device has and every one sync knows of.
    let next = std::cell::Cell::new({
        let mut top: i64 = connection
            .query_row(
                &format!("SELECT coalesce(max(rowid), 0) FROM {}", quote(&name)),
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        for known in state.replica.keys.keys() {
            if let Some((t, key)) = parse_row_key(known) {
                if let [(_, SqlValue::Integer(n))] = key.as_slice() {
                    if t == name {
                        top = top.max(*n);
                    }
                }
            }
        }
        top
    });
    let key_name = all
        .get(&name)
        .map(|t| t.key.first().cloned().unwrap_or_default())
        .unwrap_or_default();
    let both = |_: &str, _: &str, taken: &dyn Fn(&str) -> bool| -> String {
        loop {
            next.set(next.get().saturating_add(1));
            let candidate = row_key(&name, &[(key_name.clone(), SqlValue::Integer(next.get()))]);
            if !taken(&candidate) {
                return candidate;
            }
        }
    };
    let known: BTreeSet<String> = state.replica.keys.keys().cloned().collect();
    let taken = |k: &str| known.contains(k);
    let mine_value = mine_bytes.as_deref().map(|b| FileValue::of(b).encode());
    let mut trial = state.replica.clone();
    let writes = trial.resolve_named(
        row,
        choice,
        mine_value.as_deref(),
        &|device| name_or(names, device),
        &taken,
        &both,
    )?;
    let mut rows = Vec::new();
    for (key, value) in writes {
        match value.as_deref().and_then(FileValue::decode) {
            Some(value) => {
                let bytes = by_hash.get(&value.hash).ok_or(ResolveError::NotHereYet)?;
                rows.push((key, Some(bytes.clone())));
            }
            None => rows.push((key, None)),
        }
    }
    in_transaction(&rows)?;
    state.replica = trial;
    Ok(())
}

struct Loaded {
    path: PathBuf,
    held: PathBuf,
    state: SqlState,
    names: BTreeMap<String, String>,
}

struct Core {
    loaded: Option<Loaded>,
    status: SqlStatus,
    connection: Option<Connection>,
    /// `data_version` when the database was last read whole.
    looked_at: Option<i64>,
}

fn lock<T>(m: &Mutex<T>) -> Result<MutexGuard<'_, T>, String> {
    m.lock()
        .map_err(|_| "the database's sync lock was poisoned".to_string())
}

/// One app's database, kept in sync across the person's devices.
pub struct SyncedSql {
    core: Arc<Mutex<Core>>,
    session: Arc<Mutex<Session>>,
    context: AccountSyncContext,
    database: PathBuf,
    store: PathBuf,
    wake: Arc<Wake>,
    wrote: AtomicBool,
    done: Option<std::sync::mpsc::Receiver<()>>,
}

impl SyncedSql {
    /// The app's `database`, with sync's own files beside its store at
    /// `store`. With `context.background`, exchanges start at once on a
    /// thread of their own.
    pub fn open(database: PathBuf, store: PathBuf, context: AccountSyncContext) -> SyncedSql {
        let mut synced = SyncedSql {
            core: Arc::new(Mutex::new(Core {
                loaded: None,
                status: SqlStatus::default(),
                connection: None,
                looked_at: None,
            })),
            session: Arc::new(Mutex::new(Session::new(&context))),
            context,
            database,
            store,
            wake: Wake::new(),
            wrote: AtomicBool::new(false),
            done: None,
        };
        if synced.context.background {
            let run = synced.parts();
            let close = synced.parts();
            synced.done = spawn_loop(
                "krate-sql-sync",
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
            database: self.database.clone(),
            store: self.store.clone(),
        }
    }

    /// The app wrote to its database. The first write of a run wakes sync
    /// at once: on a fresh device it is the app making its tables, and the
    /// person's rows can come in now rather than half a minute later.
    pub fn app_wrote(&self) {
        if !self.wrote.swap(true, Ordering::SeqCst) {
            self.wake.kick();
        }
    }

    /// One exchange now, on this thread: what a test, or "Sync now", does.
    pub fn sync_now(&self) -> Result<Step, String> {
        self.parts().exchange(false)
    }

    pub fn status(&self) -> SqlStatus {
        self.core
            .lock()
            .map(|c| {
                let mut status = c.status.clone();
                if let Some(loaded) = c.loaded.as_ref() {
                    status.conflicts = loaded.state.replica.conflicts().len();
                    status.waits = loaded.state.waits.clone();
                    status.kept_here = loaded.state.kept_here.clone();
                }
                status
            })
            .unwrap_or_default()
    }

    /// Every row in conflict, both sides in words.
    pub fn conflicts(&self) -> Vec<RowConflict> {
        let Ok(core) = self.core.lock() else {
            return Vec::new();
        };
        let Some(loaded) = core.loaded.as_ref() else {
            return Vec::new();
        };
        conflicts_of(
            &loaded.state,
            core.connection.as_ref(),
            &loaded.held,
            &loaded.names,
        )
    }

    /// Answer a conflict; it goes up with the next exchange.
    pub fn resolve(&self, row: &str, choice: &Resolution) -> Result<(), ResolveError> {
        {
            let mut core = self.core.lock().map_err(|_| ResolveError::NoConflict)?;
            let core = &mut *core;
            let connection = core.connection.as_ref().ok_or(ResolveError::NoConflict)?;
            let loaded = core.loaded.as_mut().ok_or(ResolveError::NoConflict)?;
            resolve_in(
                &mut loaded.state,
                connection,
                &loaded.held,
                &loaded.names,
                row,
                choice,
            )?;
            let _ = write_state(&loaded.path, &loaded.state);
        }
        self.wake.kick();
        Ok(())
    }
}

impl Drop for SyncedSql {
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
    database: PathBuf,
    store: PathBuf,
}

/// One row to send: the replica's write, and the row as it was read.
struct ToSend {
    out: Outgoing,
    value: Option<FileValue>,
    row: Option<Vec<u8>>,
    body: Option<Body>,
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
        let key = SqlKey::derive(ring, &self.context.publisher, &self.context.app_id);
        let me = account.device.id();

        // 1. Look at the database: the rows changed here since are this
        // device's to send.
        let (since, sends, held) = {
            let mut core = lock(&self.core)?;
            let core = &mut *core;
            core.status.waiting_for_approval = false;
            if core.loaded.is_none() {
                let path = sql_state_path(&self.store, &account.id);
                let mut state = read_state(&path).unwrap_or_else(|| SqlState::new(&me));
                if state.replica.device != me {
                    state.replica.device = me.clone();
                    state.replica.rev = None;
                }
                core.loaded = Some(Loaded {
                    path,
                    held: sql_held_dir(&self.store, &account.id),
                    state,
                    names: krate_accountsync::device::device_names(&session.view),
                });
            }
            if core.connection.is_none() {
                core.connection = open_db(&self.database)?;
                core.looked_at = None;
            }
            let loaded = core.loaded.as_mut().expect("loaded above");
            let st = &mut loaded.state;
            if let Some(db) = core.connection.as_ref() {
                let version = data_version(db);
                if version.is_none() || version != core.looked_at {
                    let all = tables(db)?;
                    let mut current = BTreeMap::new();
                    let mut kept_here = Vec::new();
                    for (name, table) in &all {
                        for (row, bytes) in read_rows(db, name, table)? {
                            if bytes.len() as u64 > FILE_MAX {
                                kept_here.push(row);
                                continue;
                            }
                            current.insert(row, FileValue::of(&bytes).encode());
                        }
                    }
                    // A row kept here is never sent as deleted, nor replaced.
                    for row in &kept_here {
                        st.replica.keys.remove(row);
                    }
                    st.kept_here = kept_here;
                    st.replica.reconcile(&current);
                    core.looked_at = version;
                }
            }
            let all = match core.connection.as_ref() {
                Some(db) => tables(db)?,
                None => BTreeMap::new(),
            };
            let read = |row: &str| -> Option<Vec<u8>> {
                let db = core.connection.as_ref()?;
                let (name, key) = parse_row_key(row)?;
                read_row_in(db, &name, all.get(&name)?, &key)
            };
            let mut sends = Vec::new();
            for out in st.replica.outgoing().into_iter().take(SEND_MAX) {
                if out.hash.is_none() {
                    sends.push(ToSend {
                        out,
                        value: None,
                        row: None,
                        body: None,
                    });
                    continue;
                }
                let Some(row) = read(&out.key) else {
                    continue;
                };
                let value = FileValue::of(&row);
                // Changed since the look: it goes with the next one.
                if out.hash.as_deref() != Some(&krate_accountsync::value_hash(&value.encode())) {
                    continue;
                }
                let body = st
                    .bodies
                    .get(&out.key)
                    .and_then(|b| b.get(&value.hash))
                    .map(|b| Body {
                        blob: b.blob.clone(),
                        k: b.k,
                        at: b.at,
                    });
                sends.push(ToSend {
                    out,
                    value: Some(value),
                    row: Some(row),
                    body,
                });
            }
            // Rows that wait for an update are read again every time: they
            // are taken the moment this device's tables match.
            let since = if st.waits.is_empty() {
                st.replica.rev
            } else {
                None
            };
            (since, sends, loaded.held.clone())
        };
        if only_if_changed && sends.is_empty() {
            return Ok(Step::Done);
        }

        // 2. The network, with no lock held: each new large row, then the
        // entries in one request.
        let at = Some(now_secs());
        let mut writes = Vec::new();
        let mut sent = Vec::new();
        let mut uploaded = Vec::new();
        for send in sends {
            let (inline, body) = match (&send.value, send.row, send.body) {
                (None, _, _) => (None, None),
                (Some(_), Some(row), _) if row.len() <= INLINE_MAX => (Some(row), None),
                (Some(_), _, Some(body)) => (None, Some(body)),
                (Some(value), Some(row), None) => {
                    let (raw, mut body) = key
                        .seal_body(&send.out.key, &row)
                        .map_err(|e| e.to_string())?;
                    body.at = at;
                    session
                        .hub
                        .put_blob(key.bucket(), &body.blob, &raw)
                        .map_err(|e| e.to_string())?;
                    uploaded.push((send.out.key.clone(), value.hash.clone(), body.clone()));
                    (None, Some(body))
                }
                (Some(_), None, None) => continue,
            };
            let version = Version {
                value: send.value.as_ref().map(FileValue::encode),
                vv: send.out.vv.clone(),
                by: send.out.by.clone(),
            };
            let sealed = key
                .seal(
                    &send.out.key,
                    &version,
                    inline.as_deref(),
                    body.as_ref(),
                    at,
                )
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
            // row again next time.
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
        let mut by_row: BTreeMap<String, Vec<sql::Opened>> = BTreeMap::new();
        for (name, versions) in answer["entries"].as_object().into_iter().flatten() {
            for wire in versions.as_array().into_iter().flatten() {
                let Some(sealed) = Sealed::from_json(wire) else {
                    continue;
                };
                // What does not open is not this device's to read.
                let Ok(opened) = key.open(name, &sealed) else {
                    continue;
                };
                by_row.entry(opened.key.clone()).or_default().push(opened);
            }
        }

        // 4. The other devices' rows this device will need -- to put in
        // place, or to hold for a choice -- with no lock held.
        let mut skipped: BTreeSet<String> = BTreeSet::new();
        if !unchanged {
            let known: BTreeMap<String, VersionVector> = {
                let core = lock(&self.core)?;
                let st = &core.loaded.as_ref().expect("loaded").state;
                by_row
                    .keys()
                    .filter_map(|r| Some((r.clone(), st.replica.keys.get(r)?.vv.clone())))
                    .collect()
            };
            for (row, versions) in &by_row {
                for opened in versions {
                    let Some(value) = opened.version.value.as_deref().and_then(FileValue::decode)
                    else {
                        continue;
                    };
                    if known
                        .get(row)
                        .is_some_and(|vv| vv.covers(&opened.version.vv))
                    {
                        continue;
                    }
                    if held_row(&held, &value.hash).is_some() {
                        continue;
                    }
                    let bytes = match (&opened.row, &opened.body) {
                        (Some(row), _) => Some(row.clone()),
                        (None, Some(body)) => session
                            .hub
                            .blob(key.bucket(), &body.blob, FILE_MAX + 64)
                            .ok()
                            .and_then(|raw| key.open_body(row, body, &raw, &value).ok()),
                        (None, None) => None,
                    };
                    match bytes {
                        Some(bytes) if hold_row(&held, &value.hash, &bytes).is_ok() => {}
                        _ => {
                            skipped.insert(row.clone());
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
        for (row, hash, body) in uploaded {
            st.bodies.entry(row).or_default().insert(
                hash,
                KnownBody {
                    blob: body.blob,
                    k: body.k,
                    at: body.at,
                },
            );
        }
        st.replica.app = Some((self.context.app_id.clone(), self.context.publisher.clone()));
        st.database = Some(self.database.clone());
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
        for (row, versions) in &by_row {
            for opened in versions {
                held_vv
                    .entry(row.clone())
                    .or_default()
                    .push(opened.version.vv.clone());
                let (Some(value), Some(body)) = (
                    opened.version.value.as_deref().and_then(FileValue::decode),
                    &opened.body,
                ) else {
                    continue;
                };
                bodies.entry(row.clone()).or_default().insert(
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

        // With no database yet, the app has not made its tables: every row
        // waits, quietly, for its first statement.
        let all = match core.connection.as_ref() {
            Some(db) => Some(tables(db)?),
            None => None,
        };
        let kept_here: BTreeSet<String> = st.kept_here.iter().cloned().collect();
        let mut waits: Vec<Wait> = Vec::new();
        let mut targets: Vec<(String, Option<Vec<u8>>, Option<KeyState>)> = Vec::new();
        for (row, versions) in &by_row {
            if kept_here.contains(row) || skipped.contains(row) {
                continue;
            }
            let Some((name, row_key_cells)) = parse_row_key(row) else {
                continue;
            };
            let mine = st.replica.keys.get(row).map(|s| s.vv.clone());
            // A row written by a newer version of the app -- a column this
            // table lacks, or a table this device has not made -- waits.
            let table = all.as_ref().and_then(|a| a.get(&name));
            let key_fits =
                table.is_some_and(|t| row_key_cells.iter().map(|(c, _)| c).eq(t.key.iter()));
            let mut waiting = false;
            for opened in versions {
                if mine
                    .as_ref()
                    .is_some_and(|vv| vv.covers(&opened.version.vv))
                {
                    continue;
                }
                let Some(value) = opened.version.value.as_deref() else {
                    // A deletion: of a row this device can name, or of one
                    // in a table it does not have (nothing to do).
                    if table.is_some() && !key_fits {
                        waiting = true;
                    }
                    continue;
                };
                let cells = FileValue::decode(value)
                    .and_then(|v| held_row(&held, &v.hash))
                    .and_then(|bytes| decode_row(&bytes));
                let Some(cells) = cells else {
                    waiting = true;
                    continue;
                };
                // No database yet: the app has not made its tables. Its rows
                // wait, quietly, for its first statement.
                if all.is_none() {
                    waiting = true;
                    continue;
                }
                let theirs: BTreeSet<String> = row_key_cells
                    .iter()
                    .map(|(c, _)| c.clone())
                    .chain(cells.into_keys())
                    .collect();
                if !key_fits || !fits(table.map(Table::columns).as_ref(), &theirs) {
                    waiting = true;
                    let wait = Wait {
                        table: name.clone(),
                        device: opened.version.by.clone(),
                    };
                    if !waits.contains(&wait) {
                        waits.push(wait);
                    }
                }
            }
            if waiting {
                skipped.insert(row.clone());
                continue;
            }
            let was = st.replica.keys.get(row).cloned();
            let versions: Vec<Version> = versions.iter().map(|o| o.version.clone()).collect();
            let Some(target) = st.replica.merge_remote(row, &versions).store else {
                continue;
            };
            let bytes = match target.as_deref().map(FileValue::decode) {
                None => None,
                Some(Some(value)) => match held_row(&held, &value.hash) {
                    Some(bytes) => Some(bytes),
                    None => {
                        put_back(&mut st.replica, row, was);
                        skipped.insert(row.clone());
                        continue;
                    }
                },
                Some(None) => {
                    put_back(&mut st.replica, row, was);
                    continue;
                }
            };
            targets.push((row.clone(), bytes, was));
        }

        // Put the rows in place, in one transaction: a row the app changed
        // since the look is left (its change goes up next, and meets this
        // one there), and a row the database refuses is named.
        let mut refused = Vec::new();
        if !targets.is_empty() {
            match (core.connection.as_ref(), all.as_ref()) {
                (Some(db), Some(all)) => {
                    let begun = db.execute_batch("BEGIN IMMEDIATE");
                    if let Err(e) = begun {
                        for (row, _, was) in targets.drain(..) {
                            put_back(&mut st.replica, &row, was);
                            skipped.insert(row);
                        }
                        refused.push(format!("the database was busy: {e}"));
                    }
                    for (row, bytes, was) in targets {
                        let now = parse_row_key(&row)
                            .and_then(|(name, key)| read_row_in(db, &name, all.get(&name)?, &key))
                            .map(|b| FileValue::of(&b).encode());
                        let expected = was.as_ref().and_then(|s| s.hash.clone());
                        let now_hash = now.as_deref().map(krate_accountsync::value_hash);
                        if now_hash != expected {
                            put_back(&mut st.replica, &row, was);
                            skipped.insert(row);
                            continue;
                        }
                        if let Err(why) = write_one(db, all, &row, bytes.as_deref()) {
                            put_back(&mut st.replica, &row, was);
                            refused.push(why);
                        }
                    }
                    if !db.is_autocommit() {
                        db.execute_batch("COMMIT").map_err(|e| e.to_string())?;
                    }
                }
                _ => {
                    // No database: only deletions of rows it never had.
                }
            }
        }
        core.status.refused = refused;
        st.waits = waits;
        st.replica.heal(&held_vv);
        st.replica.rev = if skipped.is_empty() && core.status.refused.is_empty() {
            answer["rev"].as_u64()
        } else {
            None
        };
        if by_row
            .values()
            .flatten()
            .any(|o| o.version.by != st.replica.device && !loaded.names.contains_key(&o.version.by))
        {
            if let Ok(view) = session.hub.view(Some(&me)) {
                loaded.names = krate_accountsync::device::device_names(&view);
            }
        }
        prune_held(&held, &st.replica);
        write_state(&loaded.path, st)?;
        Ok(if st.replica.outgoing().is_empty() {
            Step::Done
        } else {
            Step::More
        })
    }
}

/// A row not taken: the replica goes back to what the database still holds,
/// so it is never sent on as a change of this device's.
fn put_back(replica: &mut Replica, row: &str, was: Option<KeyState>) {
    match was {
        Some(was) => {
            replica.keys.insert(row.to_string(), was);
        }
        None => {
            replica.keys.remove(row);
        }
    }
}

/// The rows in conflict for an app that is not running, read from disk:
/// for Krate's home and `krate account sync conflicts`.
pub fn row_conflicts_at(
    store: &Path,
    account: &str,
    names: &BTreeMap<String, String>,
) -> Vec<RowConflict> {
    let Some(state) = read_state(&sql_state_path(store, account)) else {
        return Vec::new();
    };
    if state.replica.conflicts().is_empty() {
        return Vec::new();
    }
    let connection = state
        .database
        .as_deref()
        .and_then(|p| open_db(p).ok().flatten());
    conflicts_of(
        &state,
        connection.as_ref(),
        &sql_held_dir(store, account),
        names,
    )
}

/// The tables whose rows wait on this device for an update, as its last
/// sync found them.
pub fn sql_waits_at(store: &Path, account: &str) -> Vec<Wait> {
    read_state(&sql_state_path(store, account))
        .map(|s| s.waits)
        .unwrap_or_default()
}

/// The rows this device keeps to itself (too large).
pub fn sql_kept_here_at(store: &Path, account: &str) -> Vec<String> {
    read_state(&sql_state_path(store, account))
        .map(|s| s.kept_here)
        .unwrap_or_default()
}

/// When this device last synced an app's database, in seconds since 1970.
pub fn sql_last_sync_at(store: &Path, account: &str) -> Option<u64> {
    read_state(&sql_state_path(store, account))?
        .replica
        .synced_at
}

/// Answer a row conflict while the app is not running. It goes up the next
/// time the app syncs, or at once with [`sync_sql_closed`].
pub fn resolve_row_at(
    store: &Path,
    account: &str,
    names: &BTreeMap<String, String>,
    row: &str,
    choice: &Resolution,
) -> Result<(), ResolveError> {
    let state_path = sql_state_path(store, account);
    let mut state = read_state(&state_path).ok_or(ResolveError::NoConflict)?;
    let database = state.database.clone().ok_or(ResolveError::NoConflict)?;
    let connection = open_db(&database)
        .ok()
        .flatten()
        .ok_or(ResolveError::NotHereYet)?;
    resolve_in(
        &mut state,
        &connection,
        &sql_held_dir(store, account),
        names,
        row,
        choice,
    )?;
    write_state(&state_path, &state).map_err(|_| ResolveError::NoConflict)
}

/// Send an answer given with the app closed now: one exchange of its
/// database, the way its run makes them.
pub fn sync_sql_closed(
    store: &Path,
    account: &str,
    mut context: AccountSyncContext,
) -> Result<Step, String> {
    let not_yet = || "this app's database has not synced on this device yet".to_string();
    let state = read_state(&sql_state_path(store, account)).ok_or_else(not_yet)?;
    let (app_id, publisher) = state.replica.app.ok_or_else(not_yet)?;
    let database = state.database.ok_or_else(not_yet)?;
    if !sync_wanted(store, &context.device_dir) {
        return Err("sync is off for this app on this device".to_string());
    }
    context.app_id = app_id;
    context.publisher = publisher;
    context.enabled = true;
    context.background = false;
    SyncedSql::open(database, store.to_path_buf(), context).sync_now()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(dir: &Path) -> (PathBuf, Connection) {
        let path = dir.join("app.sqlite");
        let app = Connection::open(&path).unwrap();
        krate_appsql::prepare_connection(&app, "PRAGMA journal_mode = WAL;").unwrap();
        (path, app)
    }

    /// The rows sync reads are every table's, by its key; what it puts in
    /// place lands whole, never runs the app's triggers, and a row the
    /// database refuses is named and leaves the rest.
    #[test]
    fn rows_are_read_by_key_and_written_without_the_apps_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let (path, app) = db(dir.path());
        // Made as Krate's own connection could (an app's authorizer refuses
        // a virtual table), to prove sync passes one by.
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE notes (id INTEGER PRIMARY KEY, title TEXT, done INTEGER);
             CREATE TABLE tags (name TEXT PRIMARY KEY, color TEXT UNIQUE);
             CREATE TABLE log (line TEXT);
             CREATE TABLE audit (n INTEGER);
             CREATE TRIGGER t AFTER INSERT ON notes BEGIN INSERT INTO audit VALUES (1); END;
             CREATE VIRTUAL TABLE IF NOT EXISTS search USING fts5(body);
             INSERT INTO notes (title, done) VALUES ('Milk', 0);
             INSERT INTO tags VALUES ('home', 'blue');
             INSERT INTO log VALUES ('opened');",
            )
            .unwrap();
        let sync = open_db(&path).unwrap().unwrap();
        let all = tables(&sync).unwrap();
        assert_eq!(
            all.keys().cloned().collect::<Vec<_>>(),
            vec!["audit", "log", "notes", "tags"],
            "the full-text table and its shadow tables are not the app's rows"
        );
        assert!(all["notes"].by_rowid && all["log"].by_rowid && !all["tags"].by_rowid);
        let rows = read_rows(&sync, "notes", &all["notes"]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(sql::key_words(&rows[0].0), "notes, id 1");
        let log = read_rows(&sync, "log", &all["log"]).unwrap();
        assert_eq!(sql::key_words(&log[0].0), "log, rowid 1");

        // A row from another device: in place, and no trigger ran.
        let row = row_key("notes", &[("id".into(), SqlValue::Integer(7))]);
        let bytes = encode_row(&[
            ("title".into(), SqlValue::Text("Eggs".into())),
            ("done".into(), SqlValue::Integer(1)),
        ]);
        write_one(&sync, &all, &row, Some(&bytes)).unwrap();
        assert_eq!(read_row(&sync, &row).as_deref(), Some(bytes.as_slice()));
        let audit: i64 = app
            .query_row("SELECT count(*) FROM audit", [], |r| r.get(0))
            .unwrap();
        assert_eq!(audit, 1, "only the app's own insert ran its trigger");
        // An edit and a delete.
        let edited = encode_row(&[
            ("title".into(), SqlValue::Text("Eggs".into())),
            ("done".into(), SqlValue::Integer(0)),
        ]);
        write_one(&sync, &all, &row, Some(&edited)).unwrap();
        assert_eq!(read_row(&sync, &row).as_deref(), Some(edited.as_slice()));
        write_one(&sync, &all, &row, None).unwrap();
        assert_eq!(read_row(&sync, &row), None);
        // A row the database refuses (its color is another tag's) is named.
        let clash = row_key("tags", &[("name".into(), SqlValue::Text("work".into()))]);
        let blue = encode_row(&[("color".into(), SqlValue::Text("blue".into()))]);
        let why = write_one(&sync, &all, &clash, Some(&blue)).unwrap_err();
        assert!(why.starts_with("tags, name work"), "{why}");
        // The app sees another connection's commit; sync sees the app's.
        let before = data_version(&sync);
        app.execute("INSERT INTO log VALUES ('again')", []).unwrap();
        assert_ne!(data_version(&sync), before);
    }
}
