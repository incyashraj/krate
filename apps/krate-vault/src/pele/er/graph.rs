//! Ports diagrams/er/graph.ts: a parsed ER diagram resolved into what is
//! drawn, subgraphs as groups, entities as nodes with their class styles,
//! and relationships as edges with a marker type at each end.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::ErDb;
use super::types::ErAttribute;
use crate::pele::js::{self, trim};

/// Ports `interface ErGraphNode`.
#[derive(Debug, Clone, PartialEq)]
pub struct ErGraphNode {
    pub id: String,
    pub label: String,
    pub alias: String,
    pub attributes: Vec<ErAttribute>,
    pub is_group: bool,
    pub parent_id: Option<String>,
    pub css_styles: Vec<String>,
    pub css_compiled_styles: Vec<String>,
    pub css_classes: String,
    pub dir: Option<String>,
    pub label_type: &'static str,
}

/// Ports `interface ErGraphEdge`.
#[derive(Debug, Clone, PartialEq)]
pub struct ErGraphEdge {
    pub id: String,
    pub start: String,
    pub end: String,
    pub label: String,
    pub arrow_type_start: String,
    pub arrow_type_end: String,
    /// `pattern === 'dashed'`.
    pub dashed: bool,
}

/// Ports `interface ErGraph`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ErGraph {
    pub nodes: Vec<ErGraphNode>,
    pub edges: Vec<ErGraphEdge>,
}

/// Ports `buildErGraph`.
pub fn build_er_graph(model: &ErDb) -> ErGraph {
    let mut nodes: Vec<ErGraphNode> = Vec::new();
    let mut edges: Vec<ErGraphEdge> = Vec::new();
    let subgraphs = &model.subgraphs;

    let compiled = |class_names: &[&str]| -> Vec<String> {
        let mut out = Vec::new();
        for name in class_names {
            let Some(def) = model.classes.get(name) else {
                continue;
            };
            for style in &def.styles {
                out.push(String::from(trim(style)));
            }
            for style in &def.text_styles {
                out.push(String::from(trim(style)));
            }
        }
        out
    };

    let mut parent_of: BTreeMap<String, String> = BTreeMap::new();
    let mut subgraph_ids: BTreeSet<String> = BTreeSet::new();
    for subgraph in subgraphs.iter().rev() {
        subgraph_ids.insert(subgraph.id.clone());
        for id in &subgraph.nodes {
            parent_of.insert(id.clone(), subgraph.id.clone());
        }
    }

    for subgraph in subgraphs.iter().rev() {
        let classes: Vec<&str> = subgraph.classes.iter().map(String::as_str).collect();
        nodes.push(ErGraphNode {
            id: subgraph.id.clone(),
            label: subgraph.title.clone(),
            alias: String::new(),
            attributes: Vec::new(),
            is_group: true,
            parent_id: parent_of.get(&subgraph.id).cloned(),
            css_styles: subgraph.css_styles.clone(),
            css_compiled_styles: compiled(&classes),
            css_classes: subgraph.classes.join(" "),
            dir: subgraph.dir.clone(),
            label_type: subgraph.label_type,
        });
    }

    // A name that is also a subgraph id stands for the subgraph. Mermaid drops such an entity and
    // leaves relationships written before the subgraph pointing at nothing; here they follow it.
    let mut moved: BTreeMap<String, String> = BTreeMap::new();
    for (name, entity) in model.entities.iter() {
        if subgraph_ids.contains(name) {
            moved.insert(entity.id.clone(), name.into());
            continue;
        }
        let classes: Vec<&str> = entity.css_classes.split(' ').collect();
        nodes.push(ErGraphNode {
            id: entity.id.clone(),
            label: entity.label.clone(),
            alias: entity.alias.clone(),
            attributes: entity.attributes.clone(),
            is_group: false,
            parent_id: parent_of.get(name).cloned(),
            css_styles: entity.css_styles.clone(),
            css_compiled_styles: compiled(&classes),
            css_classes: entity.css_classes.clone(),
            dir: None,
            label_type: "markdown",
        });
    }

    for (index, relationship) in model.relationships.iter().enumerate() {
        edges.push(ErGraphEdge {
            id: format!(
                "id_{}_{}_{}",
                relationship.entity_a, relationship.entity_b, index
            ),
            start: moved
                .get(&relationship.entity_a)
                .cloned()
                .unwrap_or_else(|| relationship.entity_a.clone()),
            end: moved
                .get(&relationship.entity_b)
                .cloned()
                .unwrap_or_else(|| relationship.entity_b.clone()),
            label: relationship.role_a.clone(),
            arrow_type_start: js::lower(relationship.rel_spec.card_b),
            arrow_type_end: js::lower(relationship.rel_spec.card_a),
            dashed: relationship.rel_spec.rel_type != "IDENTIFYING",
        });
    }

    ErGraph { nodes, edges }
}
