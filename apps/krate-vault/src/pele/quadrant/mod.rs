//! Ports diagrams/quadrant: Mermaid quadrant charts.

pub mod db;
pub mod lexer;
pub mod parser;
pub mod render;
pub mod tokens;

use crate::pele::errors::PeleError;
use db::QuadrantDb;

/// Ports `quadrantChart.parse` (diagrams/quadrant/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<QuadrantDb, PeleError> {
    let mut db = QuadrantDb {
        title: title.map(Into::into),
        ..QuadrantDb::default()
    };
    parser::parse_quadrant(source, &mut db)?;
    Ok(db)
}
