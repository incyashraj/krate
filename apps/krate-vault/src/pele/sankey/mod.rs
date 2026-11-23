//! Ports diagrams/sankey: Mermaid sankey diagrams.
//!
//! The lexer and the row preparation read the text as UTF-16 code units,
//! as Pele does, so that the offsets their errors are located by match
//! Pele's.

pub mod db;
pub mod layout;
pub mod lexer;
pub mod parser;
pub mod render;

use crate::pele::errors::PeleError;
use db::SankeyDb;

/// Ports `sankey.parse` (diagrams/sankey/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<SankeyDb, PeleError> {
    let mut db = SankeyDb::default();
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        db.title = Some(t.into());
    }
    let units: alloc::vec::Vec<u16> = source.encode_utf16().collect();
    let prepared = parser::prepare_text_for_parsing(&units, None);
    parser::parse_sankey(&prepared, &mut db, source)?;
    Ok(db)
}
