//! Ports diagrams/c4: Mermaid C4 diagrams (`C4Context`, `C4Container`,
//! `C4Component`, `C4Dynamic`, `C4Deployment`), from the text through the
//! lexer, parser and db to the laid out and drawn SVG.

pub mod db;
pub mod layout;
pub mod lexer;
pub mod parser;
pub mod render;
pub mod route;
pub mod tokens;
pub mod types;

use crate::pele::errors::PeleError;
use db::C4Db;

/// Ports `c4.parse` (diagrams/c4/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<C4Db, PeleError> {
    let mut db = C4Db::new();
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        db.title = Some(t.into());
    }
    parser::parse_c4(source, &mut db)?;
    Ok(db)
}
