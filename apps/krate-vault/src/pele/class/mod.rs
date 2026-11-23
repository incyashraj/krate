//! Ports diagrams/class: Mermaid class diagrams (`classDiagram`), from the
//! text through the lexer, parser and db to the drawn SVG.

pub mod db;
pub mod graph;
pub mod lexer;
pub mod markers;
pub mod members;
pub mod parser;
pub mod render;
pub mod tokens;
pub mod types;

use crate::pele::errors::PeleError;
use db::ClassDb;

/// Ports `classDiagram.parse` (diagrams/class/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<ClassDb, PeleError> {
    let mut db = ClassDb::new();
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        db.title = Some(t.into());
    }
    parser::parse_class_diagram(source, &mut db)?;
    Ok(db)
}
