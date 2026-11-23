//! Ports diagrams/c4/types.ts.

use alloc::string::String;

use crate::pele::common::ordered::OrderedMap;

/// Ports `interface C4Named`: a `$key="value"` argument.
#[derive(Debug, Clone, PartialEq)]
pub struct C4Named {
    pub key: String,
    pub value: String,
}

/// Ports `type C4Attr`.
#[derive(Debug, Clone, PartialEq)]
pub enum C4Attr {
    Str(String),
    Named(C4Named),
}

/// Ports `interface C4Element`, with `C4Shape.kind` and
/// `C4Boundary.nodeType`. `parent` is the place of a boundary in the db's
/// boundaries.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct C4Element {
    pub alias: String,
    pub label: String,
    /// A named argument written where the alias or the label belongs.
    pub alias_attr: Option<C4Named>,
    pub label_attr: Option<C4Named>,
    pub parent: Option<usize>,
    pub kind_type: Option<String>,
    pub descr: Option<String>,
    pub techn: Option<String>,
    pub sprite: Option<String>,
    pub tags: Option<String>,
    pub link: Option<String>,
    pub bg_color: Option<String>,
    pub font_color: Option<String>,
    pub border_color: Option<String>,
    pub shadowing: Option<String>,
    pub shape: Option<String>,
    pub legend_text: Option<String>,
    pub legend_sprite: Option<String>,
    /// Named arguments that set no known field.
    pub extra: OrderedMap<String>,
    /// Mermaid's typeC4Shape: person, external_system_db, container_queue and so on.
    pub kind: String,
    /// Set on deployment nodes: node, nodeL or nodeR.
    pub node_type: Option<String>,
}

impl C4Element {
    /// The field `name` of `ELEMENT_FIELDS` (and the slots `put` fills).
    pub fn field(&mut self, name: &str) -> Option<&mut Option<String>> {
        Some(match name {
            "type" => &mut self.kind_type,
            "descr" => &mut self.descr,
            "techn" => &mut self.techn,
            "sprite" => &mut self.sprite,
            "tags" => &mut self.tags,
            "link" => &mut self.link,
            "bgColor" => &mut self.bg_color,
            "fontColor" => &mut self.font_color,
            "borderColor" => &mut self.border_color,
            "shadowing" => &mut self.shadowing,
            "shape" => &mut self.shape,
            "legendText" => &mut self.legend_text,
            "legendSprite" => &mut self.legend_sprite,
            _ => return None,
        })
    }
}

/// Ports `interface C4Rel`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct C4Rel {
    pub kind: String,
    pub from: String,
    pub to: String,
    pub label: String,
    pub from_attr: Option<C4Named>,
    pub to_attr: Option<C4Named>,
    pub label_attr: Option<C4Named>,
    pub techn: Option<String>,
    pub descr: Option<String>,
    pub sprite: Option<String>,
    pub tags: Option<String>,
    pub link: Option<String>,
    pub text_color: Option<String>,
    pub line_color: Option<String>,
    pub offset_x: Option<f64>,
    pub offset_y: Option<f64>,
    pub extra: OrderedMap<String>,
}

impl C4Rel {
    /// The field `name` of `REL_FIELDS` (and the slots `put` fills).
    pub fn field(&mut self, name: &str) -> Option<&mut Option<String>> {
        Some(match name {
            "descr" => &mut self.descr,
            "techn" => &mut self.techn,
            "sprite" => &mut self.sprite,
            "tags" => &mut self.tags,
            "link" => &mut self.link,
            "textColor" => &mut self.text_color,
            "lineColor" => &mut self.line_color,
            _ => return None,
        })
    }
}
