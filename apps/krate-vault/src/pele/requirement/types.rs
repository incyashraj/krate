//! Ports diagrams/requirement/types.ts.

use alloc::string::String;
use alloc::vec::Vec;

/// Ports `interface Requirement`.
#[derive(Debug, Clone, PartialEq)]
pub struct Requirement {
    pub name: String,
    pub kind: String,
    pub requirement_id: String,
    pub text: String,
    pub risk: String,
    pub verify_method: String,
    pub css_styles: Vec<String>,
    pub classes: Vec<String>,
}

/// Ports `interface RequirementElement`.
#[derive(Debug, Clone, PartialEq)]
pub struct RequirementElement {
    pub name: String,
    pub kind: String,
    pub doc_ref: String,
    pub css_styles: Vec<String>,
    pub classes: Vec<String>,
}

/// Ports `interface Relation`.
#[derive(Debug, Clone, PartialEq)]
pub struct Relation {
    pub kind: String,
    pub src: String,
    pub dst: String,
}

/// Ports `interface RequirementClass`.
#[derive(Debug, Clone, PartialEq)]
pub struct RequirementClass {
    pub id: String,
    pub styles: Vec<String>,
    pub text_styles: Vec<String>,
}
