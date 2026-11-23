//! Ports diagrams/er/types.ts.

use alloc::string::String;
use alloc::vec::Vec;

/// Ports `interface ErAttribute`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ErAttribute {
    pub kind: String,
    pub name: String,
    pub keys: Vec<String>,
    pub comment: String,
}

/// Ports `interface ParsedAttribute`: keys and comment are present only when written.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedAttribute {
    pub kind: String,
    pub name: String,
    pub keys: Option<Vec<String>>,
    pub comment: Option<String>,
}

/// Ports `interface ErEntity`.
#[derive(Debug, Clone, PartialEq)]
pub struct ErEntity {
    pub id: String,
    pub label: String,
    pub alias: String,
    pub attributes: Vec<ErAttribute>,
    pub css_classes: String,
    pub css_styles: Vec<String>,
}

/// Ports `interface RelSpec`: cardB sits at entityA's end of the line and
/// cardA at entityB's, as in Mermaid. Cardinalities are Mermaid's names
/// (ZERO_OR_ONE, ...); `rel_type` is IDENTIFYING or NON_IDENTIFYING.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RelSpec {
    pub card_a: &'static str,
    pub rel_type: &'static str,
    pub card_b: &'static str,
}

/// Ports `interface ErRelationship`.
#[derive(Debug, Clone, PartialEq)]
pub struct ErRelationship {
    pub entity_a: String,
    pub role_a: String,
    pub entity_b: String,
    pub rel_spec: RelSpec,
}

/// Ports `interface ErClassDef`.
#[derive(Debug, Clone, PartialEq)]
pub struct ErClassDef {
    pub id: String,
    pub styles: Vec<String>,
    pub text_styles: Vec<String>,
}

/// Ports `interface ErSubgraph`.
#[derive(Debug, Clone, PartialEq)]
pub struct ErSubgraph {
    pub id: String,
    pub nodes: Vec<String>,
    pub title: String,
    pub classes: Vec<String>,
    pub css_styles: Vec<String>,
    pub dir: Option<String>,
    pub label_type: &'static str,
}

/// Ports `type DocItem`: a name, or a `direction` statement.
#[derive(Debug, Clone, PartialEq)]
pub enum DocItem {
    Name(String),
    Dir(String),
}
