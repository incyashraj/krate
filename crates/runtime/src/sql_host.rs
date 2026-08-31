//! The application database behind `krate:store/sql`.
//!
//! The key-value store holds settings. An app with a growing list, a search
//! box, or anything it needs to query rather than read whole needs a database,
//! and rewriting one on top of files is where porting a real app stops being
//! worth doing.
//!
//! What makes this a capability rather than a hole in the filesystem boundary:
//!
//! 1. **The app never names a file.** It gets SQL over its own database. The
//!    runtime chooses the path from the app's id, so this cannot widen into
//!    reading the user's documents.
//! 2. **SQLite's own escapes are closed.** `ATTACH` would open a second
//!    database anywhere on disk, and SQLite's file-backed pragmas can read and
//!    write outside the sandbox. Both are refused before the statement runs.
//! 3. **Parameters are bound, never pasted.** An app cannot build an injection
//!    out of its user's input by accident, because the text and the values
//!    travel separately all the way to SQLite.

//! ## In a browser tab
//!
//! There is no SQLite. This module keeps its shapes on wasm32 so the rest
//! of the runtime compiles unchanged, and every call answers
//! `SqlError::Unsupported` -- which the permission wall turns into words a
//! person can act on, rather than an app that opens and then silently
//! loses everything it saved.
//!
//! IndexedDB is deliberately not pretended to be a substitute: it is not
//! SQL, and an app that asked for a database and got a key-value store
//! back would fail in ways nobody could explain.

use std::path::PathBuf;

#[cfg(not(target_arch = "wasm32"))]
use rusqlite::{types::ValueRef, Connection};

/// Largest result a single query may return, so one `SELECT *` on a large table
/// cannot exhaust memory. An app that needs more should page with LIMIT.
const MAX_RESULT_ROWS: usize = 100_000;

/// Largest a single returned value may be.
const MAX_VALUE_BYTES: usize = 8 * 1024 * 1024;

/// Statements in one transaction, bounded so a batch cannot run unboundedly.
const MAX_TRANSACTION_STATEMENTS: usize = 1_000;

/// A value crossing the guest boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// Why a database operation could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlError {
    /// The app was not granted `store.sql`.
    Denied,
    /// The statement could not be parsed or refers to something missing.
    InvalidStatement(String),
    /// The statement is one this interface does not permit.
    Forbidden(String),
    /// The result, or the database, exceeded a bound.
    TooLarge,
    /// The database could not be read or written.
    Io(String),
    /// This host has no database at all -- a browser preview. Distinct from
    /// `Denied`, which means the app was not granted the capability: this
    /// one says the capability cannot exist here, so the words a person
    /// reads can be honest about which it is.
    Unsupported,
}

/// The rows a query returned.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqlValue>>,
}

/// One application's database.
// The real database, on a machine that has one.
#[cfg(not(target_arch = "wasm32"))]
pub struct AppDatabase {
    path: PathBuf,
    connection: Option<Connection>,
    /// False when the app did not receive `store.sql`. Checked before anything
    /// opens or runs, so a denied app never reaches SQLite at all.
    granted: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl std::fmt::Debug for AppDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppDatabase")
            .field("path", &self.path)
            .field("granted", &self.granted)
            .finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AppDatabase {
    /// Prepare an app's database. The file is not opened until the app runs a
    /// statement, so an app that never queries never creates one.
    pub fn new(path: PathBuf, granted: bool) -> Self {
        Self {
            path,
            connection: None,
            granted,
        }
    }

    fn connect(&mut self) -> Result<&Connection, SqlError> {
        if !self.granted {
            return Err(SqlError::Denied);
        }
        if self.connection.is_none() {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| SqlError::Io(e.to_string()))?;
            }
            let connection =
                Connection::open(&self.path).map_err(|e| SqlError::Io(e.to_string()))?;
            // Close SQLite's own routes out of the sandbox before the app can
            // run anything. `ATTACH` is rejected per statement below; these
            // stop a pragma from loading an extension or reaching a file.
            connection
                .execute_batch(
                    "PRAGMA trusted_schema = OFF; \
                     PRAGMA foreign_keys = ON; \
                     PRAGMA journal_mode = WAL;",
                )
                .map_err(|e| SqlError::Io(e.to_string()))?;
            self.connection = Some(connection);
        }
        Ok(self.connection.as_ref().expect("connection"))
    }

    pub fn query(&mut self, statement: &str, params: &[SqlValue]) -> Result<QueryResult, SqlError> {
        check_statement(statement)?;
        let connection = self.connect()?;
        let mut prepared = connection
            .prepare(statement)
            .map_err(|e| SqlError::InvalidStatement(e.to_string()))?;
        let column_count = prepared.column_count();
        let columns: Vec<String> = (0..column_count)
            .map(|i| {
                prepared
                    .column_name(i)
                    .map(str::to_string)
                    .unwrap_or_default()
            })
            .collect();

        let bound = to_rusqlite_params(params);
        let mut rows = prepared
            .query(rusqlite::params_from_iter(bound.iter()))
            .map_err(|e| SqlError::InvalidStatement(e.to_string()))?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(|e| SqlError::Io(e.to_string()))? {
            if out.len() >= MAX_RESULT_ROWS {
                return Err(SqlError::TooLarge);
            }
            let mut values = Vec::with_capacity(column_count);
            for i in 0..column_count {
                values.push(from_rusqlite_value(
                    row.get_ref(i).map_err(|e| SqlError::Io(e.to_string()))?,
                )?);
            }
            out.push(values);
        }

        Ok(QueryResult { columns, rows: out })
    }

    pub fn execute(&mut self, statement: &str, params: &[SqlValue]) -> Result<u64, SqlError> {
        check_statement(statement)?;
        let connection = self.connect()?;
        let bound = to_rusqlite_params(params);
        connection
            .execute(statement, rusqlite::params_from_iter(bound.iter()))
            .map(|changed| changed as u64)
            .map_err(|e| SqlError::InvalidStatement(e.to_string()))
    }

    /// Run several statements as one unit. Any failure rolls the whole batch
    /// back, so a half-applied change cannot survive a crash mid-migration.
    pub fn transaction(&mut self, statements: &[String]) -> Result<(), SqlError> {
        if statements.len() > MAX_TRANSACTION_STATEMENTS {
            return Err(SqlError::TooLarge);
        }
        for statement in statements {
            check_statement(statement)?;
        }
        let connection = self.connect()?;
        let tx = connection
            .unchecked_transaction()
            .map_err(|e| SqlError::Io(e.to_string()))?;
        for statement in statements {
            tx.execute_batch(statement)
                .map_err(|e| SqlError::InvalidStatement(e.to_string()))?;
        }
        tx.commit().map_err(|e| SqlError::Io(e.to_string()))
    }
}

// The browser's answer: the same shapes, and an honest refusal.
//
// Kept beside the real one rather than behind a runtime branch so the
// wasm build never links SQLite at all, and so the two cannot drift into
// disagreeing about the type the rest of the runtime sees.
#[cfg(target_arch = "wasm32")]
pub struct AppDatabase {
    path: PathBuf,
    granted: bool,
}

#[cfg(target_arch = "wasm32")]
impl std::fmt::Debug for AppDatabase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppDatabase")
            .field("path", &self.path)
            .field("granted", &self.granted)
            .finish_non_exhaustive()
    }
}

#[cfg(target_arch = "wasm32")]
impl AppDatabase {
    pub fn new(path: PathBuf, granted: bool) -> Self {
        Self { path, granted }
    }

    /// Denied beats unsupported. An app that was never granted the
    /// capability should hear the same thing everywhere; only an app that
    /// WAS granted it, and still cannot have it here, learns that this
    /// host has no database.
    fn refuse<T>(&self) -> Result<T, SqlError> {
        if !self.granted {
            return Err(SqlError::Denied);
        }
        Err(SqlError::Unsupported)
    }

    pub fn query(&mut self, statement: &str, _params: &[SqlValue]) -> Result<QueryResult, SqlError> {
        check_statement(statement)?;
        self.refuse()
    }

    pub fn execute(&mut self, statement: &str, _params: &[SqlValue]) -> Result<u64, SqlError> {
        check_statement(statement)?;
        self.refuse()
    }

    pub fn transaction(&mut self, statements: &[String]) -> Result<(), SqlError> {
        for statement in statements {
            check_statement(statement)?;
        }
        self.refuse()
    }
}

/// Refuse the statements that would reach outside the app's own database.
///
/// SQLite is a capable engine, and some of that capability is filesystem
/// access. `ATTACH` opens a second database at any path; `PRAGMA` can load an
/// extension or point the journal at a file; `VACUUM INTO` writes a copy
/// wherever it is told. None of these are things an app needs to keep its own
/// data, and every one of them is a way around the capability boundary, so they
/// are refused before the statement reaches SQLite.
fn check_statement(statement: &str) -> Result<(), SqlError> {
    let lowered = statement.to_lowercase();
    for (needle, reason) in [
        ("attach", "attaching another database"),
        ("detach", "detaching a database"),
        ("pragma", "pragmas"),
        ("vacuum into", "writing a copy to a path"),
        ("load_extension", "loading an extension"),
        ("readfile", "reading a file"),
        ("writefile", "writing a file"),
    ] {
        // Word-boundary match so a column called `attachment` is not refused.
        if contains_word(&lowered, needle) {
            return Err(SqlError::Forbidden(format!(
                "{reason} is not allowed: an app's database is its own, and this would reach \
                 outside it"
            )));
        }
    }
    Ok(())
}

/// True when `needle` appears in `haystack` bounded by non-identifier characters.
fn contains_word(haystack: &str, needle: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut start = 0;
    while let Some(at) = haystack[start..].find(needle) {
        let at = start + at;
        let before_ok = at == 0 || !haystack[..at].chars().next_back().is_some_and(is_ident);
        let after = at + needle.len();
        let after_ok =
            after >= haystack.len() || !haystack[after..].chars().next().is_some_and(is_ident);
        if before_ok && after_ok {
            return true;
        }
        start = at + 1;
        if start >= haystack.len() {
            break;
        }
    }
    false
}

#[cfg(not(target_arch = "wasm32"))]
fn to_rusqlite_params(params: &[SqlValue]) -> Vec<rusqlite::types::Value> {
    params
        .iter()
        .map(|value| match value {
            SqlValue::Null => rusqlite::types::Value::Null,
            SqlValue::Integer(n) => rusqlite::types::Value::Integer(*n),
            SqlValue::Real(n) => rusqlite::types::Value::Real(*n),
            SqlValue::Text(text) => rusqlite::types::Value::Text(text.clone()),
            SqlValue::Blob(bytes) => rusqlite::types::Value::Blob(bytes.clone()),
        })
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn from_rusqlite_value(value: ValueRef<'_>) -> Result<SqlValue, SqlError> {
    Ok(match value {
        ValueRef::Null => SqlValue::Null,
        ValueRef::Integer(n) => SqlValue::Integer(n),
        ValueRef::Real(n) => SqlValue::Real(n),
        ValueRef::Text(bytes) => {
            if bytes.len() > MAX_VALUE_BYTES {
                return Err(SqlError::TooLarge);
            }
            SqlValue::Text(String::from_utf8_lossy(bytes).into_owned())
        }
        ValueRef::Blob(bytes) => {
            if bytes.len() > MAX_VALUE_BYTES {
                return Err(SqlError::TooLarge);
            }
            SqlValue::Blob(bytes.to_vec())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database(granted: bool) -> (AppDatabase, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("app.sqlite");
        (AppDatabase::new(path, granted), dir)
    }

    #[test]
    fn a_denied_app_never_reaches_the_database() {
        let (mut db, _dir) = database(false);
        assert_eq!(
            db.execute("CREATE TABLE t (a INTEGER)", &[]),
            Err(SqlError::Denied)
        );
        assert_eq!(db.query("SELECT 1", &[]), Err(SqlError::Denied));
        assert_eq!(
            db.transaction(&["CREATE TABLE t (a INTEGER)".to_string()]),
            Err(SqlError::Denied)
        );
    }

    #[test]
    fn an_app_can_keep_and_query_its_own_data() {
        let (mut db, _dir) = database(true);
        db.execute(
            "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)",
            &[],
        )
        .expect("create");
        db.execute(
            "INSERT INTO notes (body) VALUES (?1)",
            &[SqlValue::Text("first".into())],
        )
        .expect("insert");
        db.execute(
            "INSERT INTO notes (body) VALUES (?1)",
            &[SqlValue::Text("second".into())],
        )
        .expect("insert");

        let result = db
            .query("SELECT body FROM notes ORDER BY id", &[])
            .expect("query");
        assert_eq!(result.columns, ["body"]);
        assert_eq!(
            result.rows,
            vec![
                vec![SqlValue::Text("first".into())],
                vec![SqlValue::Text("second".into())]
            ]
        );
    }

    #[test]
    fn data_survives_being_closed_and_reopened() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("app.sqlite");
        {
            let mut db = AppDatabase::new(path.clone(), true);
            db.execute("CREATE TABLE t (a TEXT)", &[]).expect("create");
            db.execute("INSERT INTO t VALUES ('kept')", &[])
                .expect("insert");
        }
        let mut db = AppDatabase::new(path, true);
        let result = db.query("SELECT a FROM t", &[]).expect("query");
        assert_eq!(result.rows, vec![vec![SqlValue::Text("kept".into())]]);
    }

    #[test]
    fn attaching_another_database_is_refused() {
        // The escape that matters most: ATTACH would open any file on disk as a
        // second database, which is precisely the authority store.sql exists
        // not to grant.
        let (mut db, _dir) = database(true);
        let err = db
            .execute("ATTACH DATABASE '/etc/passwd' AS leak", &[])
            .expect_err("must refuse");
        assert!(matches!(err, SqlError::Forbidden(_)));
    }

    #[test]
    fn pragmas_and_file_functions_are_refused() {
        let (mut db, _dir) = database(true);
        for statement in [
            "PRAGMA journal_mode = DELETE",
            "SELECT load_extension('evil.so')",
            "SELECT readfile('/etc/passwd')",
            "SELECT writefile('/tmp/x', 'data')",
            "VACUUM INTO '/tmp/copy.db'",
            "DETACH DATABASE other",
        ] {
            assert!(
                matches!(db.execute(statement, &[]), Err(SqlError::Forbidden(_))),
                "{statement:?} must be refused"
            );
        }
    }

    #[test]
    fn a_column_named_like_a_keyword_is_still_allowed() {
        // The refusal is word-bounded, so ordinary schemas are not collateral
        // damage: a table of email attachments must still work.
        let (mut db, _dir) = database(true);
        db.execute("CREATE TABLE mail (attachment TEXT)", &[])
            .expect("create");
        db.execute("INSERT INTO mail VALUES ('report.pdf')", &[])
            .expect("insert");
        let result = db.query("SELECT attachment FROM mail", &[]).expect("query");
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn parameters_are_bound_not_pasted() {
        // The classic injection: if the value were substituted into the text,
        // this would drop the table. Bound, it is just an odd note.
        let (mut db, _dir) = database(true);
        db.execute("CREATE TABLE t (body TEXT)", &[])
            .expect("create");
        db.execute(
            "INSERT INTO t VALUES (?1)",
            &[SqlValue::Text("'); DROP TABLE t; --".into())],
        )
        .expect("insert");
        let result = db.query("SELECT body FROM t", &[]).expect("table survived");
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn a_failed_transaction_leaves_nothing_behind() {
        let (mut db, _dir) = database(true);
        db.execute("CREATE TABLE t (a INTEGER PRIMARY KEY)", &[])
            .expect("create");
        let err = db.transaction(&[
            "INSERT INTO t VALUES (1)".to_string(),
            "INSERT INTO t VALUES (1)".to_string(), // duplicate key
        ]);
        assert!(err.is_err());
        let result = db.query("SELECT COUNT(*) FROM t", &[]).expect("query");
        assert_eq!(result.rows, vec![vec![SqlValue::Integer(0)]]);
    }

    #[test]
    fn every_value_type_survives_a_round_trip() {
        let (mut db, _dir) = database(true);
        db.execute("CREATE TABLE t (a, b, c, d, e)", &[])
            .expect("create");
        db.execute(
            "INSERT INTO t VALUES (?1, ?2, ?3, ?4, ?5)",
            &[
                SqlValue::Null,
                SqlValue::Integer(-42),
                SqlValue::Real(1.5),
                SqlValue::Text("text".into()),
                SqlValue::Blob(vec![0, 1, 255]),
            ],
        )
        .expect("insert");
        let result = db.query("SELECT a, b, c, d, e FROM t", &[]).expect("query");
        assert_eq!(
            result.rows[0],
            vec![
                SqlValue::Null,
                SqlValue::Integer(-42),
                SqlValue::Real(1.5),
                SqlValue::Text("text".into()),
                SqlValue::Blob(vec![0, 1, 255]),
            ]
        );
    }
}
