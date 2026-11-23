//! Ports diagrams/sequence: Mermaid sequence diagrams.

pub mod actors;
pub mod db;
pub mod lexer;
pub mod parser;
pub mod render;
pub mod tokens;

use crate::json::Value;
use crate::pele::errors::PeleError;
use db::SeqDb;

/// Ports `sequence.parse` (diagrams/sequence/index.ts).
pub fn parse(source: &str, config: &Value, title: Option<&str>) -> Result<SeqDb, PeleError> {
    let wrap = match config.get("wrap") {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    };
    let sequence_wrap =
        config.get("sequence").and_then(|s| s.get("wrap")) == Some(&Value::Bool(true));
    let mut db = SeqDb::new(wrap, sequence_wrap);
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        db.set_diagram_title(t);
    }
    parser::parse_sequence(source, &mut db)?;
    Ok(db)
}
