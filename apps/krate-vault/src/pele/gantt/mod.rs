//! Ports diagrams/gantt: Mermaid gantt charts, with the date code
//! (dayjs and d3-time as Mermaid uses them) Pele carries for them.

pub mod axis;
pub mod date;
pub mod dates;
pub mod db;
pub mod lexer;
pub mod parser;
pub mod render;
pub mod tokens;

use crate::json::Value;
use crate::pele::errors::PeleError;
use db::GanttDb;

/// Ports `gantt.parse` (diagrams/gantt/index.ts).
pub fn parse(source: &str, config: &Value, title: Option<&str>) -> Result<GanttDb, PeleError> {
    let mut db = GanttDb::new();
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        db.title = Some(t.into());
    }
    if let Some(Value::Str(mode)) = config.get("gantt").and_then(|g| g.get("displayMode")) {
        db.display_mode = mode.clone();
    }
    parser::parse_gantt(source, &mut db)?;
    Ok(db)
}
