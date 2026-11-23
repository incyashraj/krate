//! Ports diagrams/git: Mermaid git graphs.

pub mod db;
pub mod parser;
pub mod render;

use crate::json::Value;
use crate::pele::errors::PeleError;
use db::{build_git, GitModel};

/// Ports `gitGraph.parse` (diagrams/git/index.ts).
pub fn parse(source: &str, config: &Value, title: Option<&str>) -> Result<GitModel, PeleError> {
    build_git(parser::parse_git(source)?, config, title)
}
