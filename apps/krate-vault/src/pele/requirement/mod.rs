//! Ports diagrams/requirement: Mermaid requirement diagrams
//! (`requirementDiagram`), from the text through the lexer, parser and db
//! to the drawn SVG.

pub mod db;
pub mod graph;
pub mod lexer;
pub mod parser;
pub mod render;
pub mod tokens;
pub mod types;

use crate::pele::errors::PeleError;
use db::RequirementDb;

/// Ports `requirement.parse` (diagrams/requirement/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<RequirementDb, PeleError> {
    let mut db = RequirementDb::new();
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        db.title = Some(t.into());
    }
    parser::parse_requirement(source, &mut db)?;
    Ok(db)
}
