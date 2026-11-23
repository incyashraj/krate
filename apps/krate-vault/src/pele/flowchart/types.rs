//! Ports diagrams/flowchart/types.ts: the parsed flowchart.

use alloc::string::String;
use alloc::vec::Vec;

use crate::json::Value;

/// Ports `type LabelType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelType {
    Text,
    String,
    Markdown,
}

impl LabelType {
    pub fn as_str(self) -> &'static str {
        match self {
            LabelType::Text => "text",
            LabelType::String => "string",
            LabelType::Markdown => "markdown",
        }
    }
}

/// Ports `interface FlowText`. `same_as_title` stands for the identity
/// test `idText === titleText` in `addSubGraph`: a subgraph header whose
/// id is also its title.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowText {
    pub text: String,
    pub kind: LabelType,
    pub same_as_title: bool,
}

impl FlowText {
    pub fn new(text: String, kind: LabelType) -> FlowText {
        FlowText {
            text,
            kind,
            same_as_title: false,
        }
    }
}

/// Ports `interface FlowNode`. `kind` is `type`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowNode {
    pub id: String,
    pub text: Option<String>,
    pub label_type: LabelType,
    pub kind: Option<String>,
    pub styles: Vec<String>,
    pub classes: Vec<String>,
    pub dir: Option<String>,
    pub props: Option<Vec<(String, String)>>,
    pub link: Option<String>,
    pub link_target: Option<String>,
    pub icon: Option<String>,
    pub form: Option<String>,
    pub pos: Option<String>,
    pub img: Option<String>,
    pub constraint: Option<String>,
    pub asset_width: Option<f64>,
    pub asset_height: Option<f64>,
}

/// Ports `interface FlowEdge`. `kind` is `type`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowEdge {
    pub start: String,
    pub end: String,
    pub kind: Option<String>,
    pub text: String,
    pub label_type: LabelType,
    pub stroke: Option<String>,
    pub length: Option<f64>,
    pub style: Option<Vec<String>>,
    pub classes: Vec<String>,
    pub id: Option<String>,
    pub is_user_defined_id: bool,
    pub interpolate: Option<String>,
    pub animate: Option<Value>,
    pub animation: Option<Value>,
}

/// Ports `interface FlowClassDef`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowClassDef {
    pub id: String,
    pub styles: Vec<String>,
    pub text_styles: Vec<String>,
}

/// Ports `interface FlowSubgraph`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlowSubgraph {
    pub id: String,
    pub nodes: Vec<String>,
    pub title: String,
    pub classes: Vec<String>,
    pub dir: Option<String>,
    pub label_type: LabelType,
    pub metadata: Option<Vec<(String, Value)>>,
}

/// Ports `interface LinkInfo`. `kind` is `type`.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkInfo {
    pub kind: String,
    pub stroke: String,
    pub length: Option<f64>,
    pub text: Option<FlowText>,
    pub id: Option<String>,
}

/// Ports `type DocItem`.
#[derive(Debug, Clone, PartialEq)]
pub enum DocItem {
    Text(String),
    List(Vec<String>),
    Dir(String),
}
