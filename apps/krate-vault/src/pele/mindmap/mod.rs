//! Ports diagrams/mindmap: Mermaid mind maps.

pub mod db;
pub mod render;

use crate::pele::common::outline::parse_outline;
use crate::pele::errors::PeleError;
use db::MindmapDb;

/// Ports `mindmap.parse` (diagrams/mindmap/index.ts and parser.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<MindmapDb, PeleError> {
    let mut db = MindmapDb {
        title: title.map(Into::into),
        ..MindmapDb::default()
    };
    parse_outline(source, &mut db, false)?;
    Ok(db)
}
