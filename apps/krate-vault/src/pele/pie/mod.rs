//! Ports diagrams/pie: Mermaid pie charts.

pub mod model;
pub mod parser;
pub mod render;

use crate::pele::errors::PeleError;
use model::{build_pie, PieModel};

/// Ports `pie.parse` (diagrams/pie/index.ts).
pub fn parse(source: &str, title: Option<&str>) -> Result<PieModel, PeleError> {
    build_pie(parser::parse_pie(source)?, title)
}
