//! Ports diagrams/state/db.ts: Mermaid's StateDB in its version 2 form,
//! which is what Mermaid uses for both `stateDiagram` and
//! `stateDiagram-v2`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::graph::{build_state_graph, StateGraph};
use super::types::{
    Description, StateClassDef, StateLink, StateNote, StateRelation, StateStmt, Stmt,
};
use crate::pele::class::db::newline_runs;
use crate::pele::common::ordered::OrderedMap;
use crate::pele::errors::PeleError;
use crate::pele::js::{self, trim};

/// Ports `class StateDb`.
#[derive(Debug, Clone, Default)]
pub struct StateDb {
    pub root_doc: Vec<Stmt>,
    pub states: OrderedMap<StateStmt>,
    pub relations: Vec<StateRelation>,
    pub classes: OrderedMap<StateClassDef>,
    pub links: OrderedMap<StateLink>,
    pub graph: StateGraph,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    divider_count: usize,
}

impl StateDb {
    pub fn new() -> StateDb {
        StateDb::default()
    }

    /// Ports `get direction`: the direction the root document is laid out
    /// in, its last `direction` statement.
    pub fn direction(&self) -> &str {
        let mut dir = "TB";
        for item in &self.root_doc {
            if let Stmt::Dir { value } = item {
                dir = value;
            }
        }
        dir
    }

    /// Ports `setRootDoc`.
    pub fn set_root_doc(&mut self, doc: Vec<Stmt>) -> Result<(), PeleError> {
        self.root_doc = doc;
        self.translate();
        self.extract()
    }

    /// Ports `extract`: only the root document's statements become states,
    /// relations and classes here; the statements inside composites are
    /// read when the graph is built.
    fn extract(&mut self) -> Result<(), PeleError> {
        self.states = OrderedMap::new();
        self.relations = Vec::new();
        self.classes = OrderedMap::new();
        let doc = core::mem::take(&mut self.root_doc);
        for item in &doc {
            match item {
                Stmt::State(s) => {
                    self.add_state(
                        trim(&s.id),
                        s.kind.as_deref(),
                        s.description.as_ref(),
                        s.note.as_ref(),
                        None,
                    );
                }
                Stmt::Relation {
                    state1,
                    state2,
                    description,
                } => self.add_relation(state1, state2, description.as_deref().unwrap_or("")),
                Stmt::ClassDef { id, classes } => self.add_style_class(trim(id), classes),
                Stmt::Style { id, style_class } => {
                    let styles: Vec<String> = style_class
                        .split(',')
                        .map(|s| String::from(trim(&s.replace(';', ""))))
                        .collect();
                    for key in self.named(trim(id)) {
                        if let Some(state) = self.states.get_mut(&key) {
                            state.styles = styles.clone();
                        }
                    }
                }
                Stmt::ApplyClass { id, style_class } => self.set_css_class(trim(id), style_class),
                Stmt::Click { id, url, tooltip } => self.add_link(&id.id, url, tooltip),
                _ => {}
            }
        }
        self.root_doc = doc;
        self.graph = build_state_graph(&self.root_doc, &self.states, &self.classes)?;
        Ok(())
    }

    /// Ports `translate`: Mermaid's docTranslator, which gives each `[*]` an
    /// id scoped to its parent, and turns a document split by `--` into one
    /// region per part.
    fn translate(&mut self) {
        let mut doc = core::mem::take(&mut self.root_doc);
        self.translate_doc("root", &mut doc);
        self.root_doc = doc;
    }

    /// One node of `translate`'s walk: a state.
    fn translate_state(&mut self, parent_id: &str, node: &mut StateStmt, first: bool) {
        if node.id == "[*]" {
            node.id = format!("{}{}", parent_id, if first { "_start" } else { "_end" });
            node.start = Some(first);
        } else {
            node.id = trim(&node.id).into();
        }
        if let Some(mut doc) = node.doc.take() {
            let id = node.id.clone();
            self.translate_doc(&id, &mut doc);
            node.doc = Some(doc);
        }
    }

    /// The regions of a document, and then each of its items in order.
    fn translate_doc(&mut self, id: &str, doc: &mut Vec<Stmt>) {
        let mut regions: Vec<Stmt> = Vec::new();
        let mut current: Vec<Stmt> = Vec::new();
        let mut split = false;
        for item in core::mem::take(doc) {
            match item {
                Stmt::State(s) if s.kind.as_deref() == Some("divider") && s.doc.is_none() => {
                    let mut region = s;
                    region.doc = Some(core::mem::take(&mut current));
                    regions.push(Stmt::State(region));
                    split = true;
                }
                other => current.push(other),
            }
        }
        // Mermaid names the region after the last divider with a random id; here it continues the
        // divider numbering. Mermaid also leaves a document that ends with a divider ungrouped and
        // then fails to draw it; here the regions before that divider are kept.
        if split {
            if !current.is_empty() {
                let rid = self.get_divider_id();
                regions.push(Stmt::State(StateStmt {
                    id: rid,
                    kind: Some("divider".into()),
                    doc: Some(current),
                    ..StateStmt::default()
                }));
            }
            *doc = regions;
        } else {
            *doc = current;
        }
        for item in doc.iter_mut() {
            match item {
                Stmt::State(s) => self.translate_state(id, s, true),
                Stmt::Relation { state1, state2, .. } => {
                    self.translate_state(id, state1, true);
                    self.translate_state(id, state2, false);
                }
                _ => {}
            }
        }
    }

    /// Ports `addState`.
    pub fn add_state(
        &mut self,
        id: &str,
        kind: Option<&str>,
        descr: Option<&Description>,
        note: Option<&StateNote>,
        classes: Option<&[String]>,
    ) {
        let kind = kind.unwrap_or("default");
        let trimmed: String = trim(id).into();
        if !self.states.contains_key(&trimmed) {
            self.states.insert(
                &trimmed,
                StateStmt {
                    id: trimmed.clone(),
                    kind: Some(kind.into()),
                    note: note.cloned(),
                    classes: Some(Vec::new()),
                    ..StateStmt::default()
                },
            );
        } else {
            let state = self.states.get_mut(&trimmed).unwrap();
            if state.kind.as_deref().is_none_or(str::is_empty) {
                state.kind = Some(kind.into());
            }
        }
        if let Some(d) = descr.filter(|d| d.truthy()) {
            for text in d.items() {
                self.add_description(&trimmed, trim(&text));
            }
        }
        if let Some(n) = note {
            self.states.get_mut(&trimmed).unwrap().note = Some(n.clone());
        }
        if let Some(cs) = classes {
            for name in cs {
                self.set_css_class(&trimmed, trim(name));
            }
        }
    }

    /// Ports `addLink`.
    pub fn add_link(&mut self, state_id: &str, url: &str, tooltip: &str) {
        self.links.insert(
            state_id,
            StateLink {
                url: url.into(),
                tooltip: tooltip.into(),
            },
        );
    }

    /// Ports `addRelation`.
    pub fn add_relation(&mut self, item1: &StateStmt, item2: &StateStmt, title: &str) {
        for item in [item1, item2] {
            self.add_state(
                &item.id,
                item.kind.as_deref(),
                item.description.as_ref(),
                item.note.as_ref(),
                item.classes.as_deref(),
            );
        }
        self.relations.push(StateRelation {
            id1: trim(&item1.id).into(),
            id2: trim(&item2.id).into(),
            relation_title: title.into(),
        });
    }

    /// Ports `addDescription`.
    pub fn add_description(&mut self, id: &str, descr: &str) {
        if let Some(state) = self.states.get_mut(id) {
            let text = if descr.starts_with(':') {
                trim(&js::replace_first(descr, ":", "")).into()
            } else {
                String::from(descr)
            };
            state.descriptions.push(text);
        }
    }

    /// Ports `getDividerId`.
    pub fn get_divider_id(&mut self) -> String {
        self.divider_count += 1;
        format!("divider-id-{}", self.divider_count)
    }

    /// Ports `addStyleClass`.
    pub fn add_style_class(&mut self, id: &str, style_attributes: &str) {
        if !self.classes.contains_key(id) {
            self.classes.insert(
                id,
                StateClassDef {
                    id: id.into(),
                    styles: Vec::new(),
                    text_styles: Vec::new(),
                },
            );
        }
        if style_attributes.is_empty() {
            return;
        }
        let found = self.classes.get_mut(id).unwrap();
        for attrib in style_attributes.split(',') {
            let fixed: String = trim(&js::replace_first(attrib, ";", "")).into();
            if attrib.contains("color") {
                found.text_styles.push(js::replace_first(
                    &js::replace_first(&fixed, "fill", "bgFill"),
                    "color",
                    "fill",
                ));
            }
            found.styles.push(fixed);
        }
    }

    /// Ports `setCssClass`.
    pub fn set_css_class(&mut self, item_ids: &str, css_class_name: &str) {
        for key in self.named(item_ids) {
            if let Some(state) = self.states.get_mut(&key) {
                if let Some(classes) = state.classes.as_mut() {
                    classes.push(css_class_name.into());
                }
            }
        }
    }

    /// Ports `setDirection`. The grammar calls this while it is still
    /// building the document, so the statement lands in a root document
    /// that `setRootDoc` then replaces; the direction is read from the
    /// parsed `dir` statements instead, and this changes nothing drawn.
    pub fn set_direction(&mut self, _dir: &str) {}

    /// Ports `trimColon`.
    pub fn trim_colon(&self, text: &str) -> String {
        match text.strip_prefix(':') {
            Some(rest) => trim(rest).into(),
            None => trim(text).into(),
        }
    }

    /// Ports `setAccTitle`.
    pub fn set_acc_title(&mut self, text: &str) {
        self.acc_title = Some(js::trim_start(text).into());
    }

    /// Ports `setAccDescription`.
    pub fn set_acc_description(&mut self, text: &str) {
        self.acc_descr = Some(newline_runs(text));
    }

    /// Ports `named`: the states a comma-separated list of ids names,
    /// created where they do not exist yet. As in Mermaid, an id is looked
    /// up as written and trimmed only when it has to be created.
    fn named(&mut self, ids: &str) -> Vec<String> {
        let mut out = Vec::new();
        for id in ids.split(',') {
            if self.states.contains_key(id) {
                out.push(String::from(id));
                continue;
            }
            self.add_state(id, None, None, None, None);
            let key = trim(id);
            if self.states.contains_key(key) {
                out.push(key.into());
            }
        }
        out
    }
}
