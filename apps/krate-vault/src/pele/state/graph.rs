//! Ports diagrams/state/graph.ts: Mermaid's dataFetcher, which walks the
//! documents and resolves them into the nodes and edges that are drawn.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::types::{Description, StateClassDef, StateStmt, Stmt};
use crate::pele::common::ordered::OrderedMap;
use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js::trim;

/// Ports `interface StateNode`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StateNode {
    pub id: String,
    /// rect, rectWithTitle, stateStart, stateEnd, choice, fork, join,
    /// divider, roundedWithTitle, note or noteGroup.
    pub shape: Option<String>,
    pub label: Option<String>,
    pub description: Vec<String>,
    pub css_classes: String,
    pub css_compiled_styles: Vec<String>,
    pub css_styles: Vec<String>,
    pub kind: Option<String>,
    pub is_group: bool,
    pub parent_id: Option<String>,
    /// The direction inside a composite: its last `direction` statement, or TB.
    pub dir: Option<String>,
    pub position: Option<String>,
    pub note_for: Option<String>,
    pub color_index: Option<usize>,
}

/// Ports `interface StateEdge`. Labels are markdown throughout, as in Mermaid.
#[derive(Debug, Clone, PartialEq)]
pub struct StateEdge {
    pub id: String,
    pub start: String,
    pub end: String,
    pub label: String,
    pub classes: &'static str,
    /// `pattern: 'dashed'`.
    pub dashed: bool,
}

/// Ports `interface StateGraph`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StateGraph {
    pub nodes: Vec<StateNode>,
    pub edges: Vec<StateEdge>,
}

/// Ports `interface Draft`.
struct Draft {
    shape: Option<String>,
    description: Description,
    css_classes: String,
    css_styles: Vec<String>,
    kind: Option<String>,
    dir: Option<String>,
    color_index: Option<usize>,
}

/// Ports `direction`.
fn direction(doc: &[Stmt]) -> String {
    let mut dir = "TB";
    for item in doc {
        if let Stmt::Dir { value } = item {
            dir = value;
        }
    }
    dir.into()
}

struct Builder<'a> {
    classes: &'a OrderedMap<StateClassDef>,
    states: &'a OrderedMap<StateStmt>,
    nodes: Vec<StateNode>,
    edges: Vec<StateEdge>,
    by_id: BTreeMap<String, usize>,
    drafts: BTreeMap<String, Draft>,
    slots: BTreeMap<String, Option<usize>>,
    counter: usize,
    next_slot: usize,
}

impl Builder<'_> {
    /// Ports `insert`. `aside` says the node is a note or its group, which
    /// have a `position` and no `colorIndex` (Object.assign copies the
    /// fields a node has).
    fn insert(&mut self, mut node: StateNode, aside: bool) {
        // The two literal ids are what DOMPurify leaves of `<<fork>>` and `<<choice>>` in Mermaid.
        if node.id.is_empty() || node.id == "</join></fork>" || node.id == "</choice>" {
            return;
        }
        if !node.css_classes.is_empty() {
            // One copy, however many classes.
            let mut compiled: Option<Vec<String>> = None;
            for name in node.css_classes.split(' ') {
                let Some(def) = self.classes.get(name) else {
                    continue;
                };
                let list = compiled.get_or_insert_with(|| node.css_compiled_styles.clone());
                for style in &def.styles {
                    list.push(style.clone());
                }
            }
            if let Some(c) = compiled {
                node.css_compiled_styles = c;
            }
        }
        match self.by_id.get(&node.id) {
            Some(&k) => {
                let existing = &mut self.nodes[k];
                let parent_id = node.parent_id.clone().or(existing.parent_id.clone());
                existing.shape = node.shape;
                existing.label = node.label;
                existing.description = node.description;
                existing.css_classes = node.css_classes;
                existing.css_compiled_styles = node.css_compiled_styles;
                existing.css_styles = node.css_styles;
                existing.kind = node.kind;
                existing.is_group = node.is_group;
                existing.dir = node.dir;
                if aside {
                    existing.position = node.position;
                    if node.note_for.is_some() {
                        existing.note_for = node.note_for;
                    }
                } else {
                    existing.color_index = node.color_index;
                }
                if parent_id.is_some() {
                    existing.parent_id = parent_id;
                }
            }
            None => {
                self.by_id.insert(node.id.clone(), self.nodes.len());
                self.nodes.push(node);
            }
        }
    }

    /// Ports `visit`.
    fn visit(&mut self, parent: Option<&StateStmt>, item: &StateStmt, alt: bool) {
        let id = item.id.clone();
        if id == "root" {
            return;
        }
        let known = self.states.get(&id);
        let class_str = known
            .and_then(|k| k.classes.as_ref())
            .map(|c| c.join(" "))
            .unwrap_or_default();
        let styles: Vec<String> = known.map(|k| k.styles.clone()).unwrap_or_default();

        if !self.drafts.contains_key(&id) {
            let mut shape = Some(String::from("rect"));
            if item.start == Some(true) {
                shape = Some("stateStart".into());
            } else if item.start == Some(false) {
                shape = Some("stateEnd".into());
            }
            if item.kind.as_deref() != Some("default") {
                shape = item.kind.clone();
            }
            self.drafts.insert(
                id.clone(),
                Draft {
                    shape,
                    description: Description::One(id.clone()),
                    css_classes: format!("{} statediagram-state", class_str),
                    css_styles: styles.clone(),
                    kind: None,
                    dir: None,
                    color_index: None,
                },
            );
        }
        let draft = self.drafts.get_mut(&id).unwrap();

        if let Some(desc) = item.description.as_ref().filter(|d| d.truthy()) {
            match &draft.description {
                Description::Many(list) => {
                    draft.shape = Some("rectWithTitle".into());
                    let mut list = list.clone();
                    list.extend(desc.items());
                    draft.description = Description::Many(list);
                }
                Description::One(s) if !s.is_empty() => {
                    draft.shape = Some("rectWithTitle".into());
                    let mut list = if *s == id {
                        Vec::new()
                    } else {
                        alloc::vec![s.clone()]
                    };
                    list.extend(desc.items());
                    draft.description = Description::Many(list);
                }
                Description::One(_) => {
                    draft.shape = Some("rect".into());
                    draft.description = desc.clone();
                }
            }
        }
        if draft.description.len() == 1 && draft.shape.as_deref() == Some("rectWithTitle") {
            draft.shape = Some(
                if draft.kind.as_deref() == Some("group") {
                    "roundedWithTitle"
                } else {
                    "rect"
                }
                .into(),
            );
        }

        if draft.kind.is_none() {
            if let Some(doc) = &item.doc {
                draft.kind = Some("group".into());
                draft.dir = Some(direction(doc));
                draft.shape = Some(
                    if item.kind.as_deref() == Some("divider") {
                        "divider"
                    } else {
                        "roundedWithTitle"
                    }
                    .into(),
                );
                // A container takes the next palette slot unless it is a region, which shares its composite's.
                let region_of = parent.map(|p| p.id.clone()).filter(|pid| {
                    draft.shape.as_deref() == Some("divider") && self.slots.contains_key(pid)
                });
                if let Some(pid) = region_of {
                    draft.color_index = self.slots.get(&pid).copied().flatten();
                } else {
                    let slot = self.next_slot;
                    self.next_slot += 1;
                    draft.color_index = if !trim(&class_str).is_empty() || !styles.is_empty() {
                        None
                    } else {
                        Some(slot)
                    };
                }
                self.slots.insert(id.clone(), draft.color_index);
                draft.css_classes = format!(
                    "{} statediagram-cluster {}",
                    draft.css_classes,
                    if alt { "statediagram-cluster-alt" } else { "" }
                );
            }
        }

        let (label, description) = match &draft.description {
            Description::Many(list) => (
                list.first().cloned(),
                list.iter().skip(1).cloned().collect(),
            ),
            Description::One(s) => (Some(s.clone()), Vec::new()),
        };
        let mut node = StateNode {
            id: id.clone(),
            shape: draft.shape.clone(),
            label,
            description,
            css_classes: draft.css_classes.clone(),
            css_compiled_styles: Vec::new(),
            css_styles: draft.css_styles.clone(),
            kind: draft.kind.clone(),
            is_group: draft.kind.as_deref() == Some("group"),
            dir: draft.dir.clone(),
            color_index: draft.color_index,
            ..StateNode::default()
        };
        let draft_classes = draft.css_classes.clone();
        if node.shape.as_deref() == Some("divider") {
            node.label = Some(String::new());
        }
        if let Some(p) = parent.filter(|p| p.id != "root") {
            node.parent_id = Some(p.id.clone());
        }

        let Some(note) = &item.note else {
            self.insert(node, false);
            return;
        };
        // Mermaid puts each note in a group of its own, named after the state. Pele keeps that
        // group in the model and lays the note out beside its state instead.
        let aside =
            |aside_id: String, shape: &str, css_classes: String, is_group: bool| StateNode {
                id: aside_id,
                shape: Some(shape.into()),
                label: Some(note.text.clone()),
                description: Vec::new(),
                css_classes,
                css_compiled_styles: Vec::new(),
                css_styles: Vec::new(),
                kind: Some(if is_group { "group" } else { "node" }.into()),
                is_group,
                dir: None,
                position: note.position.clone(),
                ..StateNode::default()
            };
        let mut note_node = aside(
            format!("{}----note-{}", id, self.counter),
            "note",
            "statediagram-note".into(),
            false,
        );
        self.counter += 1;
        let group_id = format!("{}----parent", id);
        note_node.parent_id = Some(group_id.clone());
        note_node.note_for = Some(id.clone());
        let note_id = note_node.id.clone();
        self.insert(aside(group_id, "noteGroup", draft_classes, true), true);
        self.insert(note_node, true);
        self.insert(node, false);
        let left = note.position.as_deref() == Some("left of");
        let (from, to) = if left { (note_id, id) } else { (id, note_id) };
        self.edges.push(StateEdge {
            id: format!("{}-{}", from, to),
            start: from,
            end: to,
            label: String::new(),
            classes: "transition note-edge",
            dashed: true,
        });
    }
}

/// Ports `buildStateGraph`.
pub fn build_state_graph(
    root_doc: &[Stmt],
    states: &OrderedMap<StateStmt>,
    classes: &OrderedMap<StateClassDef>,
) -> Result<StateGraph, PeleError> {
    let mut b = Builder {
        classes,
        states,
        nodes: Vec::new(),
        edges: Vec::new(),
        by_id: BTreeMap::new(),
        drafts: BTreeMap::new(),
        slots: BTreeMap::new(),
        counter: 0,
        next_slot: 0,
    };

    // Frames: the composite whose document it is, the document, the next item, and `alt`.
    let mut stack: Vec<(Option<&StateStmt>, &[Stmt], usize, bool)> =
        alloc::vec![(None, root_doc, 0, true)];
    while let Some(frame) = stack.last_mut() {
        if frame.2 >= frame.1.len() {
            stack.pop();
            continue;
        }
        let item = &frame.1[frame.2];
        frame.2 += 1;
        let (parent, alt) = (frame.0, frame.3);
        match item {
            Stmt::State(s) => {
                b.visit(parent, s, alt);
                if let Some(doc) = &s.doc {
                    stack.push((Some(s), doc, 0, !alt));
                }
            }
            Stmt::Relation {
                state1,
                state2,
                description,
            } => {
                b.visit(parent, state1, alt);
                b.visit(parent, state2, alt);
                b.edges.push(StateEdge {
                    id: format!("edge{}", b.counter),
                    start: state1.id.clone(),
                    end: state2.id.clone(),
                    label: description.clone().unwrap_or_default(),
                    classes: "transition",
                    dashed: false,
                });
                b.counter += 1;
            }
            _ => {}
        }
    }

    for node in &b.nodes {
        if node.is_group && !node.description.is_empty() {
            return Err(PeleError::of(
                format!(
                    "Group nodes can only have label. Remove the additional description for node [{}]",
                    node.id
                ),
                PeleErrorCode::Semantic,
                "state",
            ));
        }
    }
    Ok(StateGraph {
        nodes: b.nodes,
        edges: b.edges,
    })
}
