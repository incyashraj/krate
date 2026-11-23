//! Ports diagrams/state: Mermaid state diagrams (`stateDiagram` and
//! `stateDiagram-v2`), from the text through the lexer, parser and db to
//! the drawn SVG.

pub mod db;
pub mod graph;
pub mod lexer;
pub mod parser;
pub mod render;
pub mod tokens;
pub mod types;

use crate::pele::errors::PeleError;
use db::StateDb;

/// Ports `state.parse` (diagrams/state/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<StateDb, PeleError> {
    let mut db = StateDb::new();
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        db.title = Some(t.into());
    }
    parser::parse_state(source, &mut db)?;
    Ok(db)
}
