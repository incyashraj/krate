//! Ports diagrams/sankey/db.ts.

use alloc::string::String;
use alloc::vec::Vec;

/// Ports `interface SankeyNode`.
#[derive(Debug, Clone, PartialEq)]
pub struct SankeyNode {
    pub id: String,
    pub index: usize,
}

/// Ports `interface SankeyLink`: the nodes by index.
#[derive(Debug, Clone, PartialEq)]
pub struct SankeyLink {
    pub source: usize,
    pub target: usize,
    pub value: f64,
}

/// Ports `class SankeyDb`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SankeyDb {
    pub nodes: Vec<SankeyNode>,
    pub links: Vec<SankeyLink>,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    lookup: alloc::collections::BTreeMap<String, usize>,
}

impl SankeyDb {
    /// Ports `findOrCreateNode`.
    pub fn find_or_create_node(&mut self, id: &str) -> usize {
        if let Some(&i) = self.lookup.get(id) {
            return i;
        }
        let index = self.nodes.len();
        self.lookup.insert(id.into(), index);
        self.nodes.push(SankeyNode {
            id: id.into(),
            index,
        });
        index
    }

    /// Ports `addLink`.
    pub fn add_link(&mut self, source: usize, target: usize, value: f64) {
        self.links.push(SankeyLink {
            source,
            target,
            value,
        });
    }
}
