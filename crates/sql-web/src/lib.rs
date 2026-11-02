//! `krate:store/sql` in a browser tab (IC-974, K-1149).
//!
//! The same SQLite the desktop links (rusqlite's bundled copy) and the same
//! rules (crates/appsql), compiled to WebAssembly and run in the app's
//! worker beside the app. The database lives in memory; after every write
//! the Player takes [`image`] of it and keeps that in the browser's storage
//! for this app, and hands it back to [`open`] the next time the app opens.
//!
//! The Player talks to it through a handful of plain exports (no bindings
//! generator), with JSON in and out:
//!
//! - `krate_sql_open(granted, ptr, len)`: the app's grant and its saved
//!   database (empty for a new one);
//! - `krate_sql_call(ptr, len)`: one request, `{"op":"query"|"execute",
//!   "statement","params"}` or `{"op":"transaction","statements"}`;
//! - `krate_sql_dirty()`: whether a write committed since the last image;
//! - `krate_sql_image()`: the database as bytes, for the browser's storage.
//!
//! Answers are `{"ok":...}` or `{"err":{"tag","val"}}` in the WIT's own
//! words (store.wit `sql-error`), read from `krate_sql_out_ptr/len`.
//! Integers travel as strings, so an i64 is never rounded by JavaScript;
//! blobs as hex.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use krate_appsql::rusqlite::{ffi, Connection, OpenFlags};
use krate_appsql::{check_statement, SqlError, SqlValue, MAX_TRANSACTION_STATEMENTS};
use serde_json::{json, Value};

/// One app's database in a tab.
pub struct TabDatabase {
    granted: bool,
    connection: Result<Connection, String>,
    dirty: Arc<AtomicBool>,
}

impl TabDatabase {
    /// Open the app's database from what the browser kept (`image`, empty
    /// for an app that has none yet). A denied app gets no connection at
    /// all, as on desktop; an image SQLite cannot read is reported on every
    /// call rather than replaced, so nothing the person kept is thrown away.
    pub fn open(granted: bool, image: &[u8]) -> Self {
        let dirty = Arc::new(AtomicBool::new(false));
        let connection = if granted {
            open_connection(image, dirty.clone())
        } else {
            Err("denied".to_string())
        };
        Self {
            granted,
            connection,
            dirty,
        }
    }

    fn connect(&self) -> Result<&Connection, SqlError> {
        if !self.granted {
            return Err(SqlError::Denied);
        }
        self.connection
            .as_ref()
            .map_err(|why| SqlError::Io(why.clone()))
    }

    /// One request; the answer, in the Player's JSON.
    pub fn call(&self, request: &Value) -> Value {
        match self.run(request) {
            Ok(ok) => json!({ "ok": ok }),
            Err(err) => json!({ "err": error_json(err) }),
        }
    }

    fn run(&self, request: &Value) -> Result<Value, SqlError> {
        let op = request["op"].as_str().unwrap_or_default();
        match op {
            "query" | "execute" => {
                let statement = request["statement"].as_str().unwrap_or_default();
                let params = params_from_json(&request["params"])?;
                check_statement(statement)?;
                let connection = self.connect()?;
                if op == "query" {
                    let result = krate_appsql::query(connection, statement, &params)?;
                    Ok(json!({
                        "columns": result.columns,
                        "rows": result
                            .rows
                            .iter()
                            .map(|row| row.iter().map(value_to_json).collect::<Vec<_>>())
                            .collect::<Vec<_>>(),
                    }))
                } else {
                    let changed = krate_appsql::execute(connection, statement, &params)?;
                    Ok(Value::String(changed.to_string()))
                }
            }
            "transaction" => {
                let statements: Vec<String> = request["statements"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .map(|s| s.as_str().unwrap_or_default().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                if statements.len() > MAX_TRANSACTION_STATEMENTS {
                    return Err(SqlError::TooLarge);
                }
                for statement in &statements {
                    check_statement(statement)?;
                }
                krate_appsql::transaction(self.connect()?, &statements)?;
                Ok(Value::Null)
            }
            other => Err(SqlError::Io(format!("the Player asked for {other:?}"))),
        }
    }

    /// Whether a write committed since the last [`TabDatabase::image`].
    pub fn dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
    }

    /// The whole database as bytes, for the browser to keep.
    pub fn image(&self) -> Result<Vec<u8>, String> {
        let connection = self.connection.as_ref().map_err(String::clone)?;
        let mut size: ffi::sqlite3_int64 = 0;
        // SAFETY: the handle is this connection's own; with no flags SQLite
        // returns a fresh sqlite3_malloc'd copy, freed below.
        unsafe {
            let ptr = ffi::sqlite3_serialize(connection.handle(), c"main".as_ptr(), &mut size, 0);
            if ptr.is_null() {
                // An empty database serializes to nothing at all.
                if size == 0 {
                    self.dirty.store(false, Ordering::Relaxed);
                    return Ok(Vec::new());
                }
                return Err("SQLite could not copy the database".to_string());
            }
            let bytes = std::slice::from_raw_parts(ptr, size as usize).to_vec();
            ffi::sqlite3_free(ptr.cast());
            self.dirty.store(false, Ordering::Relaxed);
            Ok(bytes)
        }
    }
}

fn open_connection(image: &[u8], dirty: Arc<AtomicBool>) -> Result<Connection, String> {
    // SQLite's memdb store, not a plain :memory: database, and a private
    // one (no leading "/", which would make it shared): only then does
    // sqlite3_serialize copy the bytes directly, rather than through a
    // PRAGMA the authorizer below refuses.
    let connection = Connection::open_with_flags(
        "file:app.db?vfs=memdb",
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| e.to_string())?;
    if !image.is_empty() {
        // SAFETY: the buffer is sqlite3_malloc64'd and handed to SQLite with
        // FREEONCLOSE, so SQLite owns it from here on, success or not.
        unsafe {
            let size = image.len() as ffi::sqlite3_int64;
            let buf = ffi::sqlite3_malloc64(image.len() as u64) as *mut u8;
            if buf.is_null() {
                return Err("not enough memory to open the saved database".to_string());
            }
            std::ptr::copy_nonoverlapping(image.as_ptr(), buf, image.len());
            let rc = ffi::sqlite3_deserialize(
                connection.handle(),
                c"main".as_ptr(),
                buf,
                size,
                size,
                ffi::SQLITE_DESERIALIZE_FREEONCLOSE | ffi::SQLITE_DESERIALIZE_RESIZEABLE,
            );
            if rc != ffi::SQLITE_OK {
                return Err(format!(
                    "the saved database could not be read (SQLite {rc})"
                ));
            }
        }
        // A saved image that is not a database fails here, not later.
        connection
            .query_row("SELECT count(*) FROM sqlite_schema", [], |_| Ok(()))
            .map_err(|e| format!("the saved database could not be read ({e})"))?;
    }
    krate_appsql::prepare_connection(&connection, "").map_err(|e| format!("{e:?}"))?;
    // Every committed write, DDL included, marks the database for saving.
    connection.commit_hook(Some(move || {
        dirty.store(true, Ordering::Relaxed);
        false
    }));
    Ok(connection)
}

fn error_json(error: SqlError) -> Value {
    match error {
        SqlError::Denied => json!({ "tag": "denied" }),
        SqlError::InvalidStatement(m) => json!({ "tag": "invalid-statement", "val": m }),
        SqlError::Forbidden(m) => json!({ "tag": "forbidden", "val": m }),
        SqlError::TooLarge => json!({ "tag": "too-large" }),
        SqlError::Io(m) => json!({ "tag": "io", "val": m }),
        // Never made here: this host has a database.
        SqlError::Unsupported => json!({ "tag": "io", "val": "no database here" }),
    }
}

fn value_to_json(value: &SqlValue) -> Value {
    match value {
        SqlValue::Null => json!({ "t": "null" }),
        SqlValue::Integer(n) => json!({ "t": "integer", "v": n.to_string() }),
        SqlValue::Real(n) if n.is_finite() => json!({ "t": "real", "v": n }),
        SqlValue::Real(n) => {
            json!({ "t": "real", "v": if *n > 0.0 { "Infinity" } else { "-Infinity" } })
        }
        SqlValue::Text(s) => json!({ "t": "text", "v": s }),
        SqlValue::Blob(b) => json!({ "t": "blob", "v": hex(b) }),
    }
}

fn params_from_json(params: &Value) -> Result<Vec<SqlValue>, SqlError> {
    let bad =
        |why: &str| SqlError::InvalidStatement(format!("a parameter could not be read: {why}"));
    let Some(list) = params.as_array() else {
        return Ok(Vec::new());
    };
    list.iter()
        .map(|p| {
            Ok(match p["t"].as_str().unwrap_or_default() {
                "null" => SqlValue::Null,
                "integer" => SqlValue::Integer(
                    p["v"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| bad("integer"))?,
                ),
                "real" => SqlValue::Real(match &p["v"] {
                    Value::Number(n) => n.as_f64().ok_or_else(|| bad("real"))?,
                    Value::String(s) => s.parse().map_err(|_| bad("real"))?,
                    _ => return Err(bad("real")),
                }),
                "text" => SqlValue::Text(p["v"].as_str().ok_or_else(|| bad("text"))?.to_string()),
                "blob" => SqlValue::Blob(
                    unhex(p["v"].as_str().unwrap_or_default()).ok_or_else(|| bad("blob"))?,
                ),
                other => return Err(bad(other)),
            })
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

// ---- the exports the Player calls

thread_local! {
    static DATABASE: RefCell<Option<TabDatabase>> = const { RefCell::new(None) };
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn set_out(bytes: Vec<u8>) {
    OUT.with(|out| *out.borrow_mut() = bytes);
}

/// Room for the Player to write a request or an image into.
#[no_mangle]
pub extern "C" fn krate_sql_alloc(len: u32) -> *mut u8 {
    let mut buf = Vec::<u8>::with_capacity(len as usize);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Give back what [`krate_sql_alloc`] lent.
///
/// # Safety
/// `ptr` and `len` must be exactly one earlier `krate_sql_alloc` call's.
#[no_mangle]
pub unsafe extern "C" fn krate_sql_free(ptr: *mut u8, len: u32) {
    drop(Vec::from_raw_parts(ptr, 0, len as usize));
}

/// Open the app's database: its grant and its saved image.
///
/// # Safety
/// `ptr..ptr+len` must be readable (a `krate_sql_alloc` buffer, or len 0).
#[no_mangle]
pub unsafe extern "C" fn krate_sql_open(granted: u32, ptr: *const u8, len: u32) -> u32 {
    let image = if len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(ptr, len as usize)
    };
    let db = TabDatabase::open(granted != 0, image);
    let failed = granted != 0 && db.connection.is_err();
    if let Err(why) = &db.connection {
        set_out(why.clone().into_bytes());
    }
    DATABASE.with(|d| *d.borrow_mut() = Some(db));
    u32::from(failed)
}

/// One request; the answer is in the out buffer.
///
/// # Safety
/// `ptr..ptr+len` must be readable.
#[no_mangle]
pub unsafe extern "C" fn krate_sql_call(ptr: *const u8, len: u32) {
    let bytes = std::slice::from_raw_parts(ptr, len as usize);
    let answer = match serde_json::from_slice::<Value>(bytes) {
        Ok(request) => DATABASE.with(|d| match d.borrow().as_ref() {
            Some(db) => db.call(&request),
            None => json!({ "err": { "tag": "io", "val": "the database is not open" } }),
        }),
        Err(e) => json!({ "err": { "tag": "io", "val": format!("the Player sent {e}") } }),
    };
    set_out(answer.to_string().into_bytes());
}

/// 1 when a write committed since the last image.
#[no_mangle]
pub extern "C" fn krate_sql_dirty() -> u32 {
    DATABASE.with(|d| u32::from(d.borrow().as_ref().is_some_and(TabDatabase::dirty)))
}

/// The database's bytes into the out buffer; 0 on success, else 1 with
/// the reason there.
#[no_mangle]
pub extern "C" fn krate_sql_image() -> u32 {
    let result = DATABASE.with(|d| match d.borrow().as_ref() {
        Some(db) => db.image(),
        None => Err("the database is not open".to_string()),
    });
    match result {
        Ok(bytes) => {
            set_out(bytes);
            0
        }
        Err(why) => {
            set_out(why.into_bytes());
            1
        }
    }
}

#[no_mangle]
pub extern "C" fn krate_sql_out_ptr() -> *const u8 {
    OUT.with(|out| out.borrow().as_ptr())
}

#[no_mangle]
pub extern "C" fn krate_sql_out_len() -> u32 {
    OUT.with(|out| out.borrow().len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(db: &TabDatabase, statement: &str, params: Value) -> Value {
        db.call(&json!({ "op": "execute", "statement": statement, "params": params }))
    }
    fn query(db: &TabDatabase, statement: &str) -> Value {
        db.call(&json!({ "op": "query", "statement": statement, "params": [] }))
    }

    #[test]
    fn a_database_survives_its_image_and_every_value_round_trips() {
        let db = TabDatabase::open(true, &[]);
        assert!(!db.dirty());
        exec(
            &db,
            "CREATE TABLE t (i INTEGER, r REAL, s TEXT, b BLOB, n)",
            json!([]),
        );
        assert!(db.dirty(), "a CREATE TABLE is a write to keep");
        let answer = exec(
            &db,
            "INSERT INTO t VALUES (?1, ?2, ?3, ?4, ?5)",
            json!([
                { "t": "integer", "v": "-9007199254740993" },
                { "t": "real", "v": 1.5 },
                { "t": "text", "v": "héllo" },
                { "t": "blob", "v": "00ff10" },
                { "t": "null" }
            ]),
        );
        assert_eq!(answer, json!({ "ok": "1" }));
        let image = db.image().expect("image");
        assert!(!db.dirty());
        query(&db, "SELECT * FROM t");
        assert!(!db.dirty(), "a read is nothing to save");

        let again = TabDatabase::open(true, &image);
        assert_eq!(
            query(&again, "SELECT * FROM t"),
            json!({ "ok": { "columns": ["i", "r", "s", "b", "n"], "rows": [[
                { "t": "integer", "v": "-9007199254740993" },
                { "t": "real", "v": 1.5 },
                { "t": "text", "v": "héllo" },
                { "t": "blob", "v": "00ff10" },
                { "t": "null" }
            ]] } })
        );
    }

    #[test]
    fn the_desktop_rules_hold_in_a_tab() {
        let denied = TabDatabase::open(false, &[]);
        assert_eq!(
            query(&denied, "SELECT 1"),
            json!({ "err": { "tag": "denied" } })
        );
        let db = TabDatabase::open(true, &[]);
        assert_eq!(
            query(&db, "ATTACH DATABASE 'x' AS y")["err"]["tag"],
            "forbidden"
        );
        assert_eq!(
            query(&db, "SELECT * FROM nothing")["err"]["tag"],
            "invalid-statement"
        );
        let failed = db.call(&json!({ "op": "transaction", "statements": [
            "CREATE TABLE a (x)", "INSERT INTO a VALUES (1)", "INSERT INTO missing VALUES (1)"
        ] }));
        assert_eq!(failed["err"]["tag"], "invalid-statement");
        assert_eq!(
            query(&db, "SELECT * FROM a")["err"]["tag"],
            "invalid-statement",
            "the batch rolled back"
        );
    }

    #[test]
    fn an_image_that_is_not_a_database_is_reported_not_replaced() {
        let db = TabDatabase::open(true, b"this is not a database at all, not even close");
        assert_eq!(query(&db, "SELECT 1")["err"]["tag"], "io");
        assert!(db.image().is_err());
    }
}
