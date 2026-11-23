//! Ports diagrams/block: Mermaid block diagrams (`block` and
//! `block-beta`), from the text through the lexer, parser and db to the
//! laid out and drawn SVG.

pub mod arrows;
pub mod db;
pub mod layout;
pub mod lexer;
pub mod parser;
pub mod render;
pub mod tokens;
pub mod types;

use crate::pele::errors::PeleError;
use db::BlockDb;

/// Ports `block.parse` (diagrams/block/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<BlockDb, PeleError> {
    let mut db = BlockDb::new();
    db.title = title.map(String::from);
    parser::parse_block(source, &mut db)?;
    Ok(db)
}

use alloc::string::String;
