//! The application database behind `krate:store/sql`.
//!
//! The key-value store holds settings. An app with a growing list, a search
//! box, or anything it needs to query rather than read whole needs a database,
//! and rewriting one on top of files is where porting a real app stops being
//! worth doing.
//!
//! The rules -- what an app may run, the limits, SQLite's escapes closed --
//! are crates/appsql, shared with the browser tab (crates/sql-web) so an app
//! meets the same database in both. This module is the desktop's half: the
//! app's own file, opened on first use.
//!
//! ## In the wasm32 runtime build
//!
//! There is no C compiler behind that build, so no SQLite. This module keeps
//! its shapes there so the rest of the runtime compiles unchanged, and every
//! call answers `SqlError::Unsupported`. A browser tab does not use this
//! build for store.sql: the Player runs crates/sql-web beside the app.

use std::path::PathBuf;

use krate_appsql::{check_statement, MAX_TRANSACTION_STATEMENTS};
pub use krate_appsql::{QueryResult, SqlError, SqlValue};

#[cfg(not(target_arch = "wasm32"))]
use krate_appsql::rusqlite::Connection;

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
            // Sync (IC-981 S4) reads and writes the same file through a
            // connection of its own: the app's waits its turn for a moment
            // rather than failing with "database is locked".
            krate_appsql::prepare_connection(
                &connection,
                "PRAGMA journal_mode = WAL; PRAGMA busy_timeout = 5000;",
            )?;
            self.connection = Some(connection);
        }
        Ok(self.connection.as_ref().expect("connection"))
    }

    pub fn query(&mut self, statement: &str, params: &[SqlValue]) -> Result<QueryResult, SqlError> {
        check_statement(statement)?;
        krate_appsql::query(self.connect()?, statement, params)
    }

    pub fn execute(&mut self, statement: &str, params: &[SqlValue]) -> Result<u64, SqlError> {
        check_statement(statement)?;
        krate_appsql::execute(self.connect()?, statement, params)
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
        krate_appsql::transaction(self.connect()?, statements)
    }
}

// The wasm32 build's answer: the same shapes, and an honest refusal.
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

    pub fn query(
        &mut self,
        statement: &str,
        _params: &[SqlValue],
    ) -> Result<QueryResult, SqlError> {
        check_statement(statement)?;
        self.refuse()
    }

    pub fn execute(&mut self, statement: &str, _params: &[SqlValue]) -> Result<u64, SqlError> {
        check_statement(statement)?;
        self.refuse()
    }

    pub fn transaction(&mut self, statements: &[String]) -> Result<(), SqlError> {
        if statements.len() > MAX_TRANSACTION_STATEMENTS {
            return Err(SqlError::TooLarge);
        }
        for statement in statements {
            check_statement(statement)?;
        }
        self.refuse()
    }
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

    /// IC-248. The filter matched the literal string `vacuum into`, and SQL
    /// does not care how much whitespace sits between two keywords. Every
    /// spelling below went straight past it and wrote a copy of the database
    /// wherever it was told; each was reproduced before the fix.
    #[test]
    fn whitespace_cannot_smuggle_a_vacuum_past_the_filter() {
        let (mut db, dir) = database(true);
        let escape = dir.path().join("stolen.db");
        for spelling in [
            format!("VACUUM\nINTO '{}'", escape.display()),
            format!("VACUUM\tINTO '{}'", escape.display()),
            format!("VACUUM  INTO '{}'", escape.display()),
            format!("vacuum\r\ninto '{}'", escape.display()),
        ] {
            let refused = db.execute(&spelling, &[]).is_err();
            assert!(refused, "not refused: {spelling:?}");
            assert!(
                !escape.exists(),
                "a copy of the database was written despite the refusal: {spelling:?}"
            );
        }
    }

    /// The authorizer is the real defence, so it must hold even for a
    /// statement the text filter never looks at. SQLite decides during
    /// prepare, from its own parse, so casing and spacing are already gone.
    #[test]
    fn the_authorizer_refuses_attach_however_it_is_written() {
        let (mut db, _dir) = database(true);
        for spelling in [
            "attach database '/etc/passwd' as leak",
            "ATTACH\nDATABASE '/etc/passwd' AS leak",
            "AtTaCh   DaTaBaSe '/etc/passwd' As leak",
        ] {
            assert!(
                db.execute(spelling, &[]).is_err(),
                "not refused: {spelling}"
            );
        }
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
