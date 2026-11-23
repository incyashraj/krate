//! Ports diagrams/block/types.ts.

use alloc::string::String;
use alloc::vec::Vec;

/// Ports `type Statement`: a block, or a statement with links, which is a
/// list (block, edge, block, and so on). Blocks are places in the db's
/// arena.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    One(usize),
    Chain(Vec<usize>),
}

/// Ports `interface Block`: one shape for blocks, edges, and the
/// statements that set columns, classes, and styles, as in Mermaid.
/// `stmts` holds the statements a composite was written with, which
/// `setHierarchy` turns into `children`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Block {
    pub id: Option<String>,
    pub kind: Option<String>,
    pub label: Option<String>,
    pub stmts: Option<Vec<Statement>>,
    pub children: Option<Vec<usize>>,
    pub columns: Option<f64>,
    pub width_in_columns: Option<f64>,
    /// Number of cells a `space` statement asked for.
    pub width: Option<f64>,
    pub directions: Option<Vec<String>>,
    pub classes: Option<Vec<String>>,
    pub styles: Option<Vec<String>>,
    pub color_index: Option<usize>,
    pub start: Option<String>,
    pub end: Option<String>,
    pub arrow_type_start: Option<String>,
    pub arrow_type_end: Option<String>,
    pub thickness: Option<String>,
    pub pattern: Option<String>,
    pub css: Option<String>,
    pub style_class: Option<String>,
    pub styles_str: Option<String>,
}

/// Ports `interface BlockClassDef`.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockClassDef {
    pub id: String,
    pub styles: Vec<String>,
    pub text_styles: Vec<String>,
}
