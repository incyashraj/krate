//! Ports diagrams/timeline: Mermaid timelines.

pub mod db;
pub mod lexer;
pub mod parser;
pub mod render;

use crate::pele::errors::PeleError;
use db::TimelineDb;

/// Ports `timeline.parse` (diagrams/timeline/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<TimelineDb, PeleError> {
    let mut db = TimelineDb::default();
    db.title = title.map(Into::into);
    parser::parse_timeline(source, &mut db)?;
    Ok(db)
}
