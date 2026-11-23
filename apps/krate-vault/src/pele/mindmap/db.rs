//! Ports diagrams/mindmap/db.ts.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::pele::common::outline::{get_type, OutlineDb};
use crate::pele::errors::{PeleError, PeleErrorCode};

/// Ports `interface MindmapNode`; children are node ids.
#[derive(Debug, Clone, PartialEq)]
pub struct MindmapNode {
    pub id: usize,
    pub node_id: String,
    pub level: i64,
    pub descr: String,
    pub kind: u32,
    pub children: Vec<usize>,
    pub is_root: bool,
    /// Index of the first-level branch the node belongs to, wrapping after eleven as in Mermaid.
    pub section: Option<usize>,
    pub class: Option<String>,
    pub icon: Option<String>,
}

const SECTIONS: usize = 11;

/// Ports `class MindmapDb`. `nodes` are in source order, which is
/// depth-first; the first is the root.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MindmapDb {
    pub title: Option<String>,
    pub nodes: Vec<MindmapNode>,
    pub base_level: i64,
    /// The chain of nodes that can still take children, with strictly increasing levels.
    pub open: Vec<usize>,
}

impl OutlineDb for MindmapDb {
    fn get_type(&self, start: &str, end: &str) -> u32 {
        get_type(start, end)
    }

    /// Ports `addNode`.
    fn add_node(
        &mut self,
        level: usize,
        id: &str,
        descr: &str,
        kind: u32,
        _shape_data: Option<&str>,
    ) -> Result<(), PeleError> {
        let is_root = self.nodes.is_empty();
        let mut level = level as i64;
        if is_root {
            self.base_level = level;
        }
        level -= self.base_level;

        // Mermaid's parent is the last node with a smaller indentation, wherever it is in the tree.
        let mut depth = self.open.len();
        while depth > 0 && self.nodes[self.open[depth - 1]].level >= level {
            depth -= 1;
        }
        let parent = if depth > 0 {
            Some(self.open[depth - 1])
        } else {
            None
        };
        if parent.is_none() && !is_root {
            return Err(PeleError::of(
                format!(
                    "There can be only one root. No parent could be found for (\"{}\")",
                    descr
                ),
                PeleErrorCode::Semantic,
                "mindmap",
            ));
        }
        let index = self.nodes.len();
        let mut node = MindmapNode {
            id: index,
            node_id: id.into(),
            level,
            descr: descr.into(),
            kind,
            children: Vec::new(),
            is_root,
            section: None,
            class: None,
            icon: None,
        };
        if let Some(p) = parent {
            let pn = &self.nodes[p];
            node.section = if pn.is_root {
                Some(pn.children.len() % SECTIONS)
            } else {
                pn.section
            };
            self.nodes[p].children.push(index);
        }
        self.open.truncate(depth);
        self.open.push(index);
        self.nodes.push(node);
        Ok(())
    }

    /// Ports `decorateNode`: Mermaid throws a TypeError when a decoration
    /// comes before any node; here it is ignored.
    fn decorate_node(&mut self, icon: Option<&str>, class: Option<&str>) -> Result<(), PeleError> {
        let Some(node) = self.nodes.last_mut() else {
            return Ok(());
        };
        if let Some(i) = icon.filter(|i| !i.is_empty()) {
            node.icon = Some(i.into());
        }
        if let Some(c) = class.filter(|c| !c.is_empty()) {
            node.class = Some(c.into());
        }
        Ok(())
    }
}
