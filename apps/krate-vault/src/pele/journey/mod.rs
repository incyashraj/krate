//! Ports diagrams/journey: Mermaid user journeys.

pub mod db;
pub mod lexer;
pub mod parser;
pub mod render;

use crate::pele::errors::PeleError;
use db::JourneyDb;

/// Ports `journey.parse` (diagrams/journey/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<JourneyDb, PeleError> {
    let mut db = JourneyDb::default();
    db.title = title.map(Into::into);
    parser::parse_journey(source, &mut db)?;
    Ok(db)
}
