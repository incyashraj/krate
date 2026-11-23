//! Ports diagrams/er: Mermaid entity relationship diagrams (`erDiagram`),
//! from the text through the lexer, parser and db to the drawn SVG.

pub mod db;
pub mod graph;
pub mod lexer;
pub mod markers;
pub mod parser;
pub mod render;
pub mod tokens;
pub mod types;

use crate::pele::errors::PeleError;
use db::ErDb;

/// Ports `er.parse` (diagrams/er/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<ErDb, PeleError> {
    let mut db = ErDb::new();
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        db.title = Some(t.into());
    }
    parser::parse_er(source, &mut db)?;
    Ok(db)
}
