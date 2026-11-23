//! Ports diagrams/flowchart: Mermaid flowcharts (`graph` and `flowchart`),
//! from the text through the lexer, parser and db to the drawn SVG.

pub mod db;
pub mod graph;
pub mod lexer;
pub mod parser;
pub mod render;
pub mod shapes;
pub mod tokens;
pub mod types;

use crate::json::Value;
use crate::pele::errors::PeleError;
use db::{FlowDb, FlowDbOptions};

/// Ports `flowchart.parse` (diagrams/flowchart/index.ts).
pub fn parse(
    source: &str,
    config: &Value,
    title: Option<&str>,
    max_edges: usize,
) -> Result<FlowDb, PeleError> {
    let inherit = config
        .get("flowchart")
        .and_then(|f| f.get("inheritDir"))
        .is_some_and(|v| *v == Value::Bool(true));
    let mut db = FlowDb::new(FlowDbOptions {
        inherit_dir: inherit,
        max_edges: Some(max_edges),
        direction: None,
    });
    if let Some(t) = title.filter(|t| !t.is_empty()) {
        db.title = Some(t.into());
    }
    parser::parse_flowchart(source, &mut db)?;
    Ok(db)
}
