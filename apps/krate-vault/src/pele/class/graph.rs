//! Ports diagrams/class/graph.ts: a parsed class diagram resolved into the
//! nodes and edges that are drawn.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::{relation_type, ClassDb};
use super::types::RelationEnd;

/// Ports `type NodeShape`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeShape {
    Rect,
    ClassBox,
    Note,
    Interface,
}

/// Ports `interface GraphNode`. `class_node` is the class's place in the
/// db's classes.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    pub is_group: bool,
    pub shape: NodeShape,
    pub parent_id: Option<String>,
    /// Index of the namespace node this one is drawn in, or -1.
    pub parent: i32,
    pub class_node: Option<usize>,
    pub color_index: Option<usize>,
}

/// Ports `interface GraphEdge`.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdge {
    pub id: String,
    pub start: String,
    pub end: String,
    pub source: i32,
    pub target: i32,
    pub label: Option<String>,
    pub arrow_type_start: &'static str,
    pub arrow_type_end: &'static str,
    pub start_label_right: String,
    pub end_label_left: String,
    /// "solid", "dashed" or "dotted".
    pub pattern: &'static str,
    pub classes: &'static str,
}

/// Ports `interface ClassGraph`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClassGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

const MARKERS: [&str; 5] = [
    "aggregation",
    "extension",
    "composition",
    "dependency",
    "lollipop",
];

/// Ports `marker`.
fn marker(kind: RelationEnd) -> &'static str {
    match kind {
        Some(k) if (k as usize) < MARKERS.len() => MARKERS[k as usize],
        _ => "none",
    }
}

/// Ports `buildClassGraph`. With `hierarchical` off, only namespaces the
/// source declares are kept and each is labeled with its full name.
pub fn build_class_graph(model: &ClassDb, hierarchical: bool) -> ClassGraph {
    let mut nodes: Vec<GraphNode> = Vec::new();
    let mut edges: Vec<GraphEdge> = Vec::new();
    let mut namespace_at: BTreeMap<String, usize> = BTreeMap::new();
    let mut class_at: BTreeMap<String, usize> = BTreeMap::new();
    let mut interface_at: BTreeMap<String, usize> = BTreeMap::new();

    let mut declared: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut declared_ancestor = |id: Option<&str>| -> Option<String> {
        if hierarchical {
            return id.map(String::from);
        }
        let id = id?;
        let mut chain: Vec<String> = Vec::new();
        let mut current: Option<String> = Some(id.into());
        let mut found: Option<String> = None;
        while let Some(cur) = current.clone().filter(|c| !c.is_empty()) {
            if let Some(d) = declared.get(&cur) {
                found = d.clone();
                break;
            }
            let Some(ns) = model.namespaces.get(&cur) else {
                break;
            };
            if ns.explicit {
                found = Some(cur);
                break;
            }
            chain.push(cur);
            current = ns.parent.clone();
        }
        for visited in chain {
            declared.insert(visited, found.clone());
        }
        found
    };

    for ns in model.namespaces.values() {
        if !hierarchical && !ns.explicit {
            continue;
        }
        namespace_at.insert(ns.id.clone(), nodes.len());
        nodes.push(GraphNode {
            id: ns.id.clone(),
            label: if hierarchical {
                ns.label.clone()
            } else {
                ns.id.clone()
            },
            is_group: true,
            shape: NodeShape::Rect,
            parent_id: if hierarchical {
                ns.parent.clone()
            } else {
                None
            },
            parent: -1,
            class_node: None,
            color_index: None,
        });
    }
    let group = |namespace_at: &BTreeMap<String, usize>, id: Option<&str>| -> i32 {
        match id {
            None => -1,
            Some(id) => namespace_at.get(id).map_or(-1, |&i| i as i32),
        }
    };
    for node in nodes.iter_mut() {
        node.parent = group(&namespace_at, node.parent_id.as_deref());
    }

    for (color_index, (k, class_node)) in model.classes.iter().enumerate() {
        let parent_id = declared_ancestor(class_node.parent.as_deref());
        class_at.insert(class_node.id.clone(), nodes.len());
        nodes.push(GraphNode {
            id: class_node.id.clone(),
            label: class_node.label.clone(),
            is_group: false,
            shape: NodeShape::ClassBox,
            parent: group(&namespace_at, parent_id.as_deref()),
            parent_id,
            class_node: model.classes.position(k),
            color_index: Some(color_index),
        });
    }

    for note in model.notes.values() {
        let parent_id = declared_ancestor(note.parent.as_deref());
        let target = note.class.as_ref().and_then(|c| class_at.get(c)).copied();
        if let Some(target) = target {
            edges.push(GraphEdge {
                id: format!("edgeNote{}", note.index),
                start: note.id.clone(),
                end: nodes[target].id.clone(),
                source: nodes.len() as i32,
                target: target as i32,
                label: None,
                arrow_type_start: "none",
                arrow_type_end: "none",
                start_label_right: String::new(),
                end_label_left: String::new(),
                pattern: "dotted",
                classes: "relation",
            });
        }
        nodes.push(GraphNode {
            id: note.id.clone(),
            label: note.text.clone(),
            is_group: false,
            shape: NodeShape::Note,
            parent: group(&namespace_at, parent_id.as_deref()),
            parent_id,
            class_node: None,
            color_index: None,
        });
    }

    // Mermaid leaves every lollipop interface outside the namespaces. Here it stays beside its class.
    for item in &model.interfaces {
        let owner = class_at.get(&item.class_id).copied();
        interface_at.insert(item.id.clone(), nodes.len());
        let parent = owner.map_or(-1, |o| nodes[o].parent);
        nodes.push(GraphNode {
            id: item.id.clone(),
            label: item.label.clone(),
            is_group: false,
            shape: NodeShape::Interface,
            parent_id: None,
            parent,
            class_node: None,
            color_index: None,
        });
    }

    for (n, relation) in model.relations.iter().enumerate() {
        let count = n + 1;
        let (type1, type2) = (relation.relation.type1, relation.relation.type2);
        // addRelation swaps in an interface for the lollipop end of a relation with no other marker.
        let start_is_interface = type1 == Some(relation_type::LOLLIPOP) && type2.is_none();
        let end_is_interface =
            !start_is_interface && type2 == Some(relation_type::LOLLIPOP) && type1.is_none();
        let start_at = if start_is_interface {
            &interface_at
        } else {
            &class_at
        };
        let end_at = if end_is_interface {
            &interface_at
        } else {
            &class_at
        };
        edges.push(GraphEdge {
            id: format!("id_{}_{}_{}", relation.id1, relation.id2, count),
            start: relation.id1.clone(),
            end: relation.id2.clone(),
            source: start_at.get(&relation.id1).map_or(-1, |&i| i as i32),
            target: end_at.get(&relation.id2).map_or(-1, |&i| i as i32),
            label: relation.title.clone(),
            arrow_type_start: marker(type1),
            arrow_type_end: marker(type2),
            start_label_right: if relation.relation_title1 == "none" {
                String::new()
            } else {
                relation.relation_title1.clone()
            },
            end_label_left: if relation.relation_title2 == "none" {
                String::new()
            } else {
                relation.relation_title2.clone()
            },
            pattern: if relation.relation.line_type == 1 {
                "dashed"
            } else {
                "solid"
            },
            classes: "relation",
        });
    }

    ClassGraph { nodes, edges }
}
