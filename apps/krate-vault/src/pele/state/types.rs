//! Ports diagrams/state/types.ts.

use alloc::string::String;
use alloc::vec::Vec;

/// A state's `description`: `string | string[]`.
#[derive(Debug, Clone, PartialEq)]
pub enum Description {
    One(String),
    Many(Vec<String>),
}

impl Description {
    /// JS truthiness: an empty string is false, any array is true.
    pub fn truthy(&self) -> bool {
        match self {
            Description::One(s) => !s.is_empty(),
            Description::Many(_) => true,
        }
    }

    /// `.length`: UTF-16 units of a string, or items of an array.
    pub fn len(&self) -> usize {
        match self {
            Description::One(s) => crate::pele::js::len16(s),
            Description::Many(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The texts, one or many.
    pub fn items(&self) -> Vec<String> {
        match self {
            Description::One(s) => alloc::vec![s.clone()],
            Description::Many(v) => v.clone(),
        }
    }
}

/// Ports `interface StateNote`.
#[derive(Debug, Clone, PartialEq)]
pub struct StateNote {
    pub position: Option<String>,
    pub text: String,
}

/// Ports `interface StateStmt`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StateStmt {
    pub id: String,
    pub kind: Option<String>,
    pub description: Option<Description>,
    pub descriptions: Vec<String>,
    pub doc: Option<Vec<Stmt>>,
    pub note: Option<StateNote>,
    pub start: Option<bool>,
    pub classes: Option<Vec<String>>,
    pub styles: Vec<String>,
    pub text_styles: Vec<String>,
}

/// Ports `type Stmt`. Mermaid's grammar also leaves the text of statements
/// that have no effect in a document; those are `Text`.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Stmt {
    Text(String),
    State(StateStmt),
    Relation {
        state1: StateStmt,
        state2: StateStmt,
        description: Option<String>,
    },
    ClassDef {
        id: String,
        classes: String,
    },
    Style {
        id: String,
        style_class: String,
    },
    ApplyClass {
        id: String,
        style_class: String,
    },
    Dir {
        value: String,
    },
    Click {
        id: StateStmt,
        url: String,
        tooltip: String,
    },
}

/// Ports `interface StateRelation`.
#[derive(Debug, Clone, PartialEq)]
pub struct StateRelation {
    pub id1: String,
    pub id2: String,
    pub relation_title: String,
}

/// Ports `interface StateClassDef`.
#[derive(Debug, Clone, PartialEq)]
pub struct StateClassDef {
    pub id: String,
    pub styles: Vec<String>,
    pub text_styles: Vec<String>,
}

/// Ports `interface StateLink`.
#[derive(Debug, Clone, PartialEq)]
pub struct StateLink {
    pub url: String,
    pub tooltip: String,
}
