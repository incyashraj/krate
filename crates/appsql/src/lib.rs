//! The rules behind `krate:store/sql`, in one place for every host.
//!
//! The desktop runtime (crates/runtime/src/sql_host.rs) keeps an app's
//! database in a file; a browser tab (crates/sql-web) keeps it in memory
//! and saves it in the browser's storage. Both open the database through
//! [`prepare_connection`] and run every statement through [`query`],
//! [`execute`] and [`transaction`] here, so an app meets the same SQLite,
//! the same refusals and the same limits wherever it runs.
//!
//! What makes this a capability rather than a hole in the filesystem boundary:
//!
//! 1. **The app never names a file.** It gets SQL over its own database. The
//!    host chooses where it lives from the app's id, so this cannot widen
//!    into reading the user's documents.
//! 2. **SQLite's own escapes are closed.** `ATTACH` would open a second
//!    database anywhere on disk, and SQLite's file-backed pragmas can read and
//!    write outside the sandbox. Both are refused before the statement runs.
//! 3. **Parameters are bound, never pasted.** An app cannot build an injection
//!    out of its user's input by accident, because the text and the values
//!    travel separately all the way to SQLite.

/// Largest result a single query may return, so one `SELECT *` on a large table
/// cannot exhaust memory. An app that needs more should page with LIMIT.
pub const MAX_RESULT_ROWS: usize = 100_000;

/// Largest a single returned value may be.
pub const MAX_VALUE_BYTES: usize = 8 * 1024 * 1024;

/// Statements in one transaction, bounded so a batch cannot run unboundedly.
pub const MAX_TRANSACTION_STATEMENTS: usize = 1_000;

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
    /// This host has no database at all. Distinct from `Denied`, which means
    /// the app was not granted the capability: this one says the capability
    /// cannot exist here, so the words a person reads can be honest about
    /// which it is.
    Unsupported,
}

/// The rows a query returned.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<SqlValue>>,
}

/// Refuse the statements that would reach outside the app's own database.
///
/// SQLite is a capable engine, and some of that capability is filesystem
/// access. `ATTACH` opens a second database at any path; `PRAGMA` can load an
/// extension or point the journal at a file; `VACUUM INTO` writes a copy
/// wherever it is told. None of these are things an app needs to keep its own
/// data, and every one of them is a way around the capability boundary, so they
/// are refused before the statement reaches SQLite.
pub fn check_statement(statement: &str) -> Result<(), SqlError> {
    // Collapse every run of whitespace to one space before matching.
    //
    // Without this, `vacuum into` only matched a single space, and SQL does
    // not care: `VACUUM` + newline + `INTO` wrote a copy of the database to
    // any path and returned success (IC-248). A tab or two spaces did the
    // same. The authorizer installed on the connection is the real defence
    // now, but this stays as a second line and has to at least be right
    // about the spelling it claims to catch.
    let lowered = statement
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
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

#[cfg(feature = "engine")]
pub use engine::*;

#[cfg(feature = "engine")]
mod engine {
    use super::{QueryResult, SqlError, SqlValue, MAX_RESULT_ROWS, MAX_VALUE_BYTES};
    use rusqlite::{types::ValueRef, Connection};

    pub use rusqlite;

    /// Make a freshly opened connection safe to hand to an app.
    ///
    /// `pragmas` are the host's own (the desktop asks for WAL on its file);
    /// they run first, because once the authorizer is installed no pragma
    /// runs at all.
    pub fn prepare_connection(connection: &Connection, pragmas: &str) -> Result<(), SqlError> {
        // Close SQLite's own routes out of the sandbox before the app can
        // run anything. `ATTACH` is rejected per statement as well; these
        // stop a pragma from loading an extension or reaching a file.
        connection
            .execute_batch(&format!(
                "PRAGMA trusted_schema = OFF; PRAGMA foreign_keys = ON; {pragmas}"
            ))
            .map_err(|e| SqlError::Io(e.to_string()))?;
        install_authorizer(connection);
        Ok(())
    }

    /// Run a statement that returns rows.
    pub fn query(
        connection: &Connection,
        statement: &str,
        params: &[SqlValue],
    ) -> Result<QueryResult, SqlError> {
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

    /// Run a statement for its effect; the number of rows it changed.
    pub fn execute(
        connection: &Connection,
        statement: &str,
        params: &[SqlValue],
    ) -> Result<u64, SqlError> {
        let bound = to_rusqlite_params(params);
        connection
            .execute(statement, rusqlite::params_from_iter(bound.iter()))
            .map(|changed| changed as u64)
            .map_err(|e| SqlError::InvalidStatement(e.to_string()))
    }

    /// Run several statements as one unit. Any failure rolls the whole batch
    /// back, so a half-applied change cannot survive a crash mid-migration.
    /// The caller has checked every statement and the batch's length.
    pub fn transaction(connection: &Connection, statements: &[String]) -> Result<(), SqlError> {
        let tx = connection
            .unchecked_transaction()
            .map_err(|e| SqlError::Io(e.to_string()))?;
        for statement in statements {
            tx.execute_batch(statement)
                .map_err(|e| SqlError::InvalidStatement(e.to_string()))?;
        }
        tx.commit().map_err(|e| SqlError::Io(e.to_string()))
    }

    /// Let SQLite itself decide what an app may do (IC-248).
    ///
    /// The text filter ([`crate::check_statement`]) is a second line, not the first
    /// one. It could not be the first: it matched the literal string `vacuum
    /// into`, and SQL does not care how many spaces sit between two keywords.
    /// `VACUUM` + newline + `INTO` went straight past it and wrote a copy of
    /// the database wherever it was told -- reproduced before this fix.
    ///
    /// That is the shape of every text filter. It has to be right about all
    /// the ways a statement can be spelled, while an attacker only has to find
    /// one spelling it missed: a tab, two spaces, a comment between the words,
    /// a different case. So the decision moves to the one component that has
    /// already parsed the statement and knows what it actually does.
    ///
    /// The authorizer runs during prepare, before any row is touched, and sees
    /// typed actions rather than text. Whitespace, comments and casing are gone
    /// by then -- an `Attach` is an `Attach` however it was written.
    fn install_authorizer(connection: &Connection) {
        use rusqlite::hooks::{AuthAction, Authorization};
        connection.authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
            match context.action {
                // A second database at any path on disk. The escape that
                // matters most, and the reason this exists.
                AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
                // A pragma can load an extension or point the journal at a
                // file. The ones Krate itself needs are set on the connection
                // before this is installed, so refusing all of them here
                // costs the app nothing it is entitled to.
                AuthAction::Pragma { .. } => Authorization::Deny,
                // Everything else is ordinary table work inside the app's own
                // database, which is exactly what store.sql is for.
                _ => Authorization::Allow,
            }
        }));
    }

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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_named_like_a_keyword_is_still_allowed() {
        assert_eq!(
            check_statement("SELECT attachment, pragmatic FROM t"),
            Ok(())
        );
        assert!(check_statement("ATTACH DATABASE 'x' AS y").is_err());
        assert!(check_statement("VACUUM\n\tINTO '/tmp/x'").is_err());
    }

    #[cfg(feature = "engine")]
    #[test]
    fn an_in_memory_database_keeps_the_same_rules() {
        let connection = rusqlite::Connection::open_in_memory().expect("memory");
        prepare_connection(&connection, "").expect("prepare");
        execute(&connection, "CREATE TABLE t (a INTEGER)", &[]).expect("create");
        execute(
            &connection,
            "INSERT INTO t VALUES (?1)",
            &[SqlValue::Integer(7)],
        )
        .expect("insert");
        let rows = query(&connection, "SELECT a FROM t", &[]).expect("query");
        assert_eq!(rows.rows, vec![vec![SqlValue::Integer(7)]]);
        // Past the text filter, the authorizer still refuses.
        assert!(matches!(
            execute(&connection, "ATTACH DATABASE ':memory:' AS other", &[]),
            Err(SqlError::InvalidStatement(_))
        ));
        assert!(matches!(
            query(&connection, "PRAGMA journal_mode", &[]),
            Err(SqlError::InvalidStatement(_))
        ));
    }
}
