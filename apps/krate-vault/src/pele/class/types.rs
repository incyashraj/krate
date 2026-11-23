//! Ports diagrams/class/types.ts.

use alloc::string::String;
use alloc::vec::Vec;

use super::members::ClassMember;

/// Ports `type RelationEnd`: a relation type number, or None for 'none'.
pub type RelationEnd = Option<u8>;

/// Ports `interface ClassNode`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClassNode {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub css_classes: String,
    pub methods: Vec<ClassMember>,
    pub members: Vec<ClassMember>,
    pub annotations: Vec<String>,
    pub styles: Vec<String>,
    pub parent: Option<String>,
    pub link: Option<String>,
    pub link_target: Option<String>,
    pub have_callback: bool,
    pub tooltip: Option<String>,
}

/// Ports `relation` of `interface ClassRelation`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Relation {
    pub type1: RelationEnd,
    pub type2: RelationEnd,
    pub line_type: u8,
}

/// Ports `interface ClassRelation`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassRelation {
    pub id1: String,
    pub id2: String,
    pub relation_title1: String,
    pub relation_title2: String,
    pub title: Option<String>,
    pub relation: Relation,
}

/// Ports `interface ClassNote`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassNote {
    pub id: String,
    pub class: Option<String>,
    pub text: String,
    pub index: usize,
    pub parent: Option<String>,
}

/// Ports `interface ClassInterface`.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassInterface {
    pub id: String,
    pub label: String,
    pub class_id: String,
}

/// Ports `interface NamespaceNode`. Its classes, notes and children are
/// kept as the ids of those things.
#[derive(Debug, Clone, PartialEq)]
pub struct NamespaceNode {
    pub id: String,
    pub label: String,
    pub classes: Vec<String>,
    pub notes: Vec<String>,
    pub children: Vec<String>,
    pub parent: Option<String>,
    /// False for the ancestors created on the way to a dotted name such as `A.B.C`.
    pub explicit: bool,
}

/// Ports `interface StyleClass`.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleClass {
    pub id: String,
    pub styles: Vec<String>,
    pub text_styles: Vec<String>,
}
