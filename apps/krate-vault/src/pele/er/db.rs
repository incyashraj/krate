//! Ports diagrams/er/db.ts: what the ER parser builds, as Mermaid's ErDB
//! keeps it.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::types::{
    DocItem, ErAttribute, ErClassDef, ErEntity, ErRelationship, ErSubgraph, ParsedAttribute,
    RelSpec,
};
use crate::pele::class::db::newline_runs;
use crate::pele::common::ordered::OrderedMap;
use crate::pele::js::{self, trim};

/// Ports `labelType`.
fn label_type(kind: Option<&str>) -> &'static str {
    match kind {
        Some("string") => "string",
        Some("text") => "text",
        _ => "markdown",
    }
}

/// Ports `class ErDb`.
#[derive(Debug, Clone)]
pub struct ErDb {
    pub direction: String,
    pub entities: OrderedMap<ErEntity>,
    pub relationships: Vec<ErRelationship>,
    pub classes: OrderedMap<ErClassDef>,
    pub subgraphs: Vec<ErSubgraph>,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    /// The place of each subgraph in `subgraphs`, by id.
    subgraph_lookup: OrderedMap<usize>,
    claimed: BTreeSet<String>,
}

impl Default for ErDb {
    fn default() -> Self {
        ErDb {
            direction: "TB".into(),
            entities: OrderedMap::new(),
            relationships: Vec::new(),
            classes: OrderedMap::new(),
            subgraphs: Vec::new(),
            title: None,
            acc_title: None,
            acc_descr: None,
            subgraph_lookup: OrderedMap::new(),
            claimed: BTreeSet::new(),
        }
    }
}

impl ErDb {
    pub fn new() -> ErDb {
        ErDb::default()
    }

    /// Ports `addEntity`.
    pub fn add_entity(&mut self, name: &str, alias: &str) -> &mut ErEntity {
        if !self.entities.contains_key(name) {
            let entity = ErEntity {
                id: format!("entity-{}-{}", name, self.entities.len()),
                label: name.into(),
                alias: alias.into(),
                attributes: Vec::new(),
                css_classes: "default".into(),
                css_styles: Vec::new(),
            };
            self.entities.insert(name, entity);
        } else {
            let entity = self.entities.get_mut(name).unwrap();
            if entity.alias.is_empty() && !alias.is_empty() {
                entity.alias = alias.into();
            }
        }
        self.entities.get_mut(name).unwrap()
    }

    /// Ports `addAttributes`: the grammar builds the list back to front, so
    /// the last attribute written comes first.
    pub fn add_attributes(&mut self, name: &str, attributes: Vec<ParsedAttribute>) {
        let entity = self.add_entity(name, "");
        for attribute in attributes.into_iter().rev() {
            entity.attributes.push(ErAttribute {
                kind: attribute.kind,
                name: attribute.name,
                keys: attribute.keys.unwrap_or_default(),
                comment: attribute.comment.unwrap_or_default(),
            });
        }
    }

    /// Ports `addRelationship`.
    pub fn add_relationship(&mut self, ent_a: &str, role_a: &str, ent_b: &str, rel_spec: RelSpec) {
        let entity_a = if self.subgraph_lookup.contains_key(ent_a) {
            String::from(ent_a)
        } else {
            self.add_entity(ent_a, "").id.clone()
        };
        let entity_b = if self.subgraph_lookup.contains_key(ent_b) {
            String::from(ent_b)
        } else {
            self.add_entity(ent_b, "").id.clone()
        };
        self.relationships.push(ErRelationship {
            entity_a,
            role_a: role_a.into(),
            entity_b,
            rel_spec,
        });
    }

    /// Ports `setDirection`.
    pub fn set_direction(&mut self, dir: &str) {
        self.direction = dir.into();
    }

    /// Ports `addCssStyles`.
    pub fn add_css_styles(&mut self, ids: &[String], styles: &[String]) {
        for id in ids {
            if let Some(entity) = self.entities.get_mut(id) {
                entity.css_styles.extend(styles.iter().cloned());
            }
            if let Some(&k) = self.subgraph_lookup.get(id) {
                self.subgraphs[k].css_styles.extend(styles.iter().cloned());
            }
        }
    }

    /// Ports `addClass`.
    pub fn add_class(&mut self, ids: &[String], styles: &[String]) {
        for id in ids {
            if !self.classes.contains_key(id) {
                self.classes.insert(
                    id,
                    ErClassDef {
                        id: id.clone(),
                        styles: Vec::new(),
                        text_styles: Vec::new(),
                    },
                );
            }
            let def = self.classes.get_mut(id).unwrap();
            for style in styles {
                if style.contains("color") {
                    def.text_styles
                        .push(js::replace_first(style, "fill", "bgFill"));
                }
                def.styles.push(style.clone());
            }
        }
    }

    /// Ports `setClass`.
    pub fn set_class(&mut self, ids: &[String], class_names: &[String]) {
        for id in ids {
            if let Some(entity) = self.entities.get_mut(id) {
                for name in class_names {
                    entity.css_classes.push(' ');
                    entity.css_classes.push_str(name);
                }
            }
            if let Some(&k) = self.subgraph_lookup.get(id) {
                self.subgraphs[k]
                    .classes
                    .extend(class_names.iter().cloned());
            }
        }
    }

    /// Ports `addSubGraph`.
    pub fn add_sub_graph(&mut self, id_text: &str, list: &[DocItem], title_text: &str) -> String {
        let id: String = trim(id_text).into();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut nodes: Vec<String> = Vec::new();
        let mut dir: Option<String> = None;
        for item in list {
            let item = match item {
                DocItem::Dir(value) => {
                    dir = Some(value.clone());
                    continue;
                }
                DocItem::Name(name) => name,
            };
            let trimmed = trim(item);
            if trimmed.is_empty() || seen.contains(trimmed) {
                continue;
            }
            seen.insert(trimmed.into());
            // A node belongs to the first subgraph that lists it.
            if !self.claimed.contains(item) {
                nodes.push(item.clone());
            }
        }
        for node in &nodes {
            self.claimed.insert(node.clone());
        }
        let subgraph = ErSubgraph {
            id: id.clone(),
            nodes,
            title: trim(title_text).into(),
            classes: Vec::new(),
            css_styles: Vec::new(),
            dir,
            label_type: label_type(None),
        };
        self.subgraphs.push(subgraph);
        self.subgraph_lookup.insert(&id, self.subgraphs.len() - 1);
        id
    }

    /// Ports `setAccTitle`.
    pub fn set_acc_title(&mut self, text: &str) {
        self.acc_title = Some(js::trim_start(text).into());
    }

    /// Ports `setAccDescription`.
    pub fn set_acc_description(&mut self, text: &str) {
        self.acc_descr = Some(newline_runs(text));
    }
}
