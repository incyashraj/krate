//! Ports diagrams/xychart: Mermaid XY charts (bars and lines).

pub mod db;
pub mod lexer;
pub mod parser;
pub mod render;
pub mod ticks;
pub mod tokens;

use crate::pele::errors::PeleError;
use db::XyChartDb;

/// Ports `xychart.parse` (diagrams/xychart/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<XyChartDb, PeleError> {
    let mut db = XyChartDb::new();
    db.title = title.map(Into::into);
    parser::parse_xy_chart(source, &mut db)?;
    Ok(db)
}
