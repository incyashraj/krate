//! Ports diagrams/requirement/graph.ts: the nodes and edges to lay out,
//! requirements first, then elements, as Mermaid orders them.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::{values, RequirementDb};
use super::types::{Requirement, RequirementElement};

/// Ports `interface GraphNode`.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub requirement: Option<Requirement>,
    pub element: Option<RequirementElement>,
    pub css_styles: Vec<String>,
    pub css_classes: String,
}

/// Ports `interface GraphEdge`.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdge {
    pub id: String,
    pub start: Option<String>,
    pub end: Option<String>,
    pub kind: String,
    pub contains: bool,
}

/// Ports `interface RequirementGraph`.
#[derive(Debug, Clone, PartialEq)]
pub struct RequirementGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub direction: String,
}

/// Ports `buildRequirementGraph`.
pub fn build_requirement_graph(model: &RequirementDb) -> RequirementGraph {
    let mut nodes: Vec<GraphNode> = Vec::new();
    for requirement in model.requirements.values() {
        nodes.push(GraphNode {
            id: requirement.name.clone(),
            requirement: Some(requirement.clone()),
            element: None,
            css_styles: requirement.css_styles.clone(),
            css_classes: requirement.classes.join(" "),
        });
    }
    for element in model.elements.values() {
        nodes.push(GraphNode {
            id: element.name.clone(),
            requirement: None,
            element: Some(element.clone()),
            css_styles: element.css_styles.clone(),
            css_classes: element.classes.join(" "),
        });
    }
    let has = |id: &str| -> Option<String> {
        if model.requirements.contains_key(id) || model.elements.contains_key(id) {
            Some(id.into())
        } else {
            None
        }
    };
    let edges = model
        .relations
        .iter()
        .enumerate()
        .map(|(index, relation)| GraphEdge {
            id: format!("{}-{}-{}", relation.src, relation.dst, index),
            start: has(&relation.src),
            end: has(&relation.dst),
            kind: relation.kind.clone(),
            contains: relation.kind == values::CONTAINS,
        })
        .collect();
    RequirementGraph {
        nodes,
        edges,
        direction: model.direction.clone(),
    }
}
