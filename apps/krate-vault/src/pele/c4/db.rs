//! Ports diagrams/c4/db.ts: what the C4 parser builds, as Mermaid's C4 db
//! keeps it.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use super::types::{C4Attr, C4Element, C4Named, C4Rel};
use crate::pele::class::db::newline_runs;
use crate::pele::common::ordered::OrderedMap;
use crate::pele::js;

/// Ports `type Target`: an element or a relation, which named arguments
/// and positional slots fill.
trait Target {
    fn is_rel(&self) -> bool;
    fn field(&mut self, name: &str) -> Option<&mut Option<String>>;
    fn label(&mut self) -> &mut String;
    fn label_attr(&mut self) -> &mut Option<C4Named>;
    fn extra(&mut self) -> &mut OrderedMap<String>;
    fn offset(&mut self, _key: &str) -> Option<&mut Option<f64>> {
        None
    }
}

impl Target for C4Element {
    fn is_rel(&self) -> bool {
        false
    }
    fn field(&mut self, name: &str) -> Option<&mut Option<String>> {
        C4Element::field(self, name)
    }
    fn label(&mut self) -> &mut String {
        &mut self.label
    }
    fn label_attr(&mut self) -> &mut Option<C4Named> {
        &mut self.label_attr
    }
    fn extra(&mut self) -> &mut OrderedMap<String> {
        &mut self.extra
    }
}

impl Target for C4Rel {
    fn is_rel(&self) -> bool {
        true
    }
    fn field(&mut self, name: &str) -> Option<&mut Option<String>> {
        C4Rel::field(self, name)
    }
    fn label(&mut self) -> &mut String {
        &mut self.label
    }
    fn label_attr(&mut self) -> &mut Option<C4Named> {
        &mut self.label_attr
    }
    fn extra(&mut self) -> &mut OrderedMap<String> {
        &mut self.extra
    }
    fn offset(&mut self, key: &str) -> Option<&mut Option<f64>> {
        match key {
            "offsetX" => Some(&mut self.offset_x),
            "offsetY" => Some(&mut self.offset_y),
            _ => None,
        }
    }
}

/// Ports `named`: Mermaid assigns a named argument to whatever property it
/// names, structural ones included. Here it reaches only the known fields;
/// any other name is kept aside in `extra`.
fn named(target: &mut dyn Target, attr: &C4Named) {
    let (key, value) = (attr.key.as_str(), attr.value.as_str());
    if target.is_rel() {
        if let Some(slot) = target.offset(key) {
            *slot = Some(js::parse_int(value, 10));
            return;
        }
    }
    if key == "label" {
        *target.label() = value.into();
        return;
    }
    if !target.is_rel() || key != "type" {
        if let Some(slot) = target.field(key) {
            *slot = Some(value.into());
            return;
        }
    }
    target.extra().insert(key, value.into());
}

/// Ports `put`: a positional argument fills the field of its slot. A named
/// one fills the field it names, whichever slot it is in.
fn put(target: &mut dyn Target, field: &str, attr: Option<&C4Attr>) {
    match attr {
        None => {}
        Some(C4Attr::Str(s)) => {
            if let Some(slot) = target.field(field) {
                *slot = Some(s.clone());
            }
        }
        Some(C4Attr::Named(n)) => named(target, n),
    }
}

/// Ports `text`.
fn text(target: &mut dyn Target, field: &str, attr: Option<&C4Attr>) {
    match attr {
        None => {
            if let Some(slot) = target.field(field) {
                if slot.is_none() {
                    *slot = Some(String::new());
                }
            }
        }
        Some(_) => put(target, field, attr),
    }
}

/// Ports `label`.
fn label(target: &mut dyn Target, attr: Option<&C4Attr>) {
    *target.label_attr() = None;
    match attr {
        Some(C4Attr::Named(n)) => {
            *target.label() = String::new();
            *target.label_attr() = Some(n.clone());
            named(target, n);
        }
        Some(C4Attr::Str(s)) => *target.label() = s.clone(),
        None => *target.label() = String::new(),
    }
}

/// An element `getC4Shape` finds: a shape or a boundary, by place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ElRef {
    Shape(usize),
    Boundary(usize),
}

/// Ports `class C4Db`.
#[derive(Debug, Clone)]
pub struct C4Db {
    /// The header keyword: C4Context, C4Container, C4Component, C4Dynamic or C4Deployment.
    pub c4_type: Option<String>,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    pub shapes: Vec<C4Element>,
    /// The first boundary is the unnamed one that holds everything at the top level.
    pub boundaries: Vec<C4Element>,
    pub rels: Vec<C4Rel>,
    pub shape_in_row: f64,
    pub boundary_in_row: f64,
    shape_index: BTreeMap<String, usize>,
    // The top-level boundary is not listed, so a boundary a diagram names `global` is its own box.
    boundary_index: BTreeMap<String, usize>,
    rel_index: BTreeMap<String, BTreeMap<String, usize>>,
    current: usize,
    stack: Vec<usize>,
}

impl Default for C4Db {
    fn default() -> Self {
        C4Db {
            c4_type: None,
            title: None,
            acc_title: None,
            acc_descr: None,
            shapes: Vec::new(),
            boundaries: alloc::vec![C4Element {
                alias: "global".into(),
                label: "global".into(),
                kind_type: Some("global".into()),
                ..C4Element::default()
            }],
            rels: Vec::new(),
            shape_in_row: 4.0,
            boundary_in_row: 2.0,
            shape_index: BTreeMap::new(),
            boundary_index: BTreeMap::new(),
            rel_index: BTreeMap::new(),
            current: 0,
            stack: Vec::new(),
        }
    }
}

impl C4Db {
    pub fn new() -> C4Db {
        C4Db::default()
    }

    /// Ports `setC4Type`.
    pub fn set_c4_type(&mut self, kind: &str) {
        self.c4_type = Some(kind.into());
    }

    /// Ports `setTitle`.
    pub fn set_title(&mut self, title: &str) {
        self.title = Some(title.into());
    }

    /// Ports `setAccDescription`.
    pub fn set_acc_description(&mut self, descr: &str) {
        self.acc_descr = Some(newline_runs(descr));
    }

    /// Ports `element`: in the boundaries when `boundary`, else the shapes.
    fn element(&mut self, boundary: bool, alias: &C4Attr, title: Option<&C4Attr>) -> usize {
        let found = match alias {
            C4Attr::Str(a) => {
                if boundary {
                    self.boundary_index.get(a).copied()
                } else {
                    self.shape_index.get(a).copied()
                }
            }
            C4Attr::Named(_) => None,
        };
        let k = match found {
            Some(k) => k,
            None => {
                let list = if boundary {
                    &mut self.boundaries
                } else {
                    &mut self.shapes
                };
                list.push(C4Element::default());
                let k = list.len() - 1;
                match alias {
                    C4Attr::Str(a) => {
                        list[k].alias = a.clone();
                        if boundary {
                            self.boundary_index.insert(a.clone(), k);
                        } else {
                            self.shape_index.insert(a.clone(), k);
                        }
                    }
                    C4Attr::Named(n) => {
                        list[k].alias_attr = Some(n.clone());
                        named(&mut list[k], n);
                    }
                }
                k
            }
        };
        let el = if boundary {
            &mut self.boundaries[k]
        } else {
            &mut self.shapes[k]
        };
        label(el, title);
        k
    }

    /// Ports `addPersonOrSystem`: alias, label, ?descr, ?sprite, ?tags, $link.
    pub fn add_person_or_system(&mut self, kind: &str, attrs: &[C4Attr]) {
        let k = self.element(false, &attrs[0], attrs.get(1));
        text(&mut self.shapes[k], "descr", attrs.get(2));
        self.finish_shape(k, kind, attrs, 3);
    }

    /// Ports `addContainer`: alias, label, ?techn, ?descr, ?sprite, ?tags, $link.
    pub fn add_container(&mut self, kind: &str, attrs: &[C4Attr]) {
        let k = self.element(false, &attrs[0], attrs.get(1));
        text(&mut self.shapes[k], "techn", attrs.get(2));
        text(&mut self.shapes[k], "descr", attrs.get(3));
        self.finish_shape(k, kind, attrs, 4);
    }

    /// Ports `addComponent`.
    pub fn add_component(&mut self, kind: &str, attrs: &[C4Attr]) {
        self.add_container(kind, attrs);
    }

    /// Ports `finishShape`.
    fn finish_shape(&mut self, k: usize, kind: &str, attrs: &[C4Attr], at: usize) {
        let current = self.current;
        let shape = &mut self.shapes[k];
        put(shape, "sprite", attrs.get(at));
        put(shape, "tags", attrs.get(at + 1));
        put(shape, "link", attrs.get(at + 2));
        shape.kind = kind.into();
        shape.parent = Some(current);
    }

    /// Ports `boundary`.
    fn boundary(&mut self, attrs: &[C4Attr], fallback: &str) -> usize {
        let k = self.element(true, &attrs[0], attrs.get(1));
        let b = &mut self.boundaries[k];
        match attrs.get(2) {
            None => b.kind_type = Some(fallback.into()),
            Some(a) => put(b, "type", Some(a)),
        }
        k
    }

    /// Ports `open`.
    fn open(&mut self, k: usize) {
        self.boundaries[k].parent = Some(self.current);
        self.stack.push(self.current);
        self.current = k;
    }

    /// Ports `addPersonOrSystemBoundary`: alias, label, ?type, ?tags, $link.
    pub fn add_person_or_system_boundary(&mut self, attrs: &[C4Attr], fallback: &str) {
        let k = self.boundary(attrs, fallback);
        let b = &mut self.boundaries[k];
        put(b, "tags", attrs.get(3));
        put(b, "link", attrs.get(4));
        self.open(k);
    }

    /// Ports `addContainerBoundary`.
    pub fn add_container_boundary(&mut self, attrs: &[C4Attr]) {
        self.add_person_or_system_boundary(attrs, "container");
    }

    /// Ports `addDeploymentNode`: alias, label, ?type, ?descr, ?sprite, ?tags, $link.
    pub fn add_deployment_node(&mut self, node_type: &str, attrs: &[C4Attr]) {
        let k = self.boundary(attrs, "node");
        let b = &mut self.boundaries[k];
        text(b, "descr", attrs.get(3));
        // Mermaid drops whatever is in the sprite slot, a named argument included.
        put(b, "sprite", attrs.get(4));
        put(b, "tags", attrs.get(5));
        put(b, "link", attrs.get(6));
        b.node_type = Some(node_type.into());
        self.open(k);
    }

    /// Ports `popBoundaryParseStack`.
    pub fn pop_boundary_parse_stack(&mut self) {
        self.current = self.stack.pop().unwrap_or(0);
    }

    /// Ports `addRel`: from, to, label, ?techn, ?descr, ?sprite, ?tags, $link.
    pub fn add_rel(&mut self, kind: &str, attrs: &[C4Attr]) {
        let (Some(from), Some(to), Some(_)) = (attrs.first(), attrs.get(1), attrs.get(2)) else {
            return;
        };
        let known = match (from, to) {
            (C4Attr::Str(f), C4Attr::Str(t)) => Some((f.clone(), t.clone())),
            _ => None,
        };
        let found = known
            .as_ref()
            .and_then(|(f, t)| self.rel_index.get(f).and_then(|m| m.get(t)))
            .copied();
        let k = match found {
            Some(k) => k,
            None => {
                self.rels.push(C4Rel {
                    kind: kind.into(),
                    ..C4Rel::default()
                });
                let k = self.rels.len() - 1;
                if let Some((f, t)) = known {
                    self.rel_index.entry(f).or_default().insert(t, k);
                }
                let rel = &mut self.rels[k];
                match from {
                    C4Attr::Str(s) => rel.from = s.clone(),
                    C4Attr::Named(n) => rel.from_attr = Some(n.clone()),
                }
                match to {
                    C4Attr::Str(s) => rel.to = s.clone(),
                    C4Attr::Named(n) => rel.to_attr = Some(n.clone()),
                }
                k
            }
        };
        let rel = &mut self.rels[k];
        rel.kind = kind.into();
        label(rel, attrs.get(2));
        text(rel, "techn", attrs.get(3));
        text(rel, "descr", attrs.get(4));
        put(rel, "sprite", attrs.get(5));
        put(rel, "tags", attrs.get(6));
        put(rel, "link", attrs.get(7));
    }

    /// Ports `getC4Shape`.
    pub fn get_c4_shape(&self, alias: &str) -> Option<ElRef> {
        self.shape_index
            .get(alias)
            .map(|&k| ElRef::Shape(k))
            .or_else(|| self.boundary_index.get(alias).map(|&k| ElRef::Boundary(k)))
    }

    /// Ports `updateElStyle`: elementName, ?bgColor, ?fontColor,
    /// ?borderColor, ?shadowing, ?shape, ?sprite, ?techn, ?legendText,
    /// ?legendSprite.
    pub fn update_el_style(&mut self, attrs: &[C4Attr]) {
        let el = match attrs.first() {
            Some(C4Attr::Str(a)) => self.get_c4_shape(a),
            _ => None,
        };
        let Some(el) = el else {
            return;
        };
        let el = match el {
            ElRef::Shape(k) => &mut self.shapes[k],
            ElRef::Boundary(k) => &mut self.boundaries[k],
        };
        for (field, at) in [
            ("bgColor", 1),
            ("fontColor", 2),
            ("borderColor", 3),
            ("shadowing", 4),
            ("shape", 5),
            ("sprite", 6),
            ("techn", 7),
            ("legendText", 8),
            ("legendSprite", 9),
        ] {
            put(el, field, attrs.get(at));
        }
    }

    /// Ports `updateRelStyle`: from, to, ?textColor, ?lineColor, ?offsetX, ?offsetY.
    pub fn update_rel_style(&mut self, attrs: &[C4Attr]) {
        let k = match (attrs.first(), attrs.get(1)) {
            (Some(C4Attr::Str(f)), Some(C4Attr::Str(t))) => {
                self.rel_index.get(f).and_then(|m| m.get(t)).copied()
            }
            _ => None,
        };
        let Some(k) = k else {
            return;
        };
        let rel = &mut self.rels[k];
        put(rel, "textColor", attrs.get(2));
        put(rel, "lineColor", attrs.get(3));
        match attrs.get(4) {
            Some(C4Attr::Str(x)) => rel.offset_x = Some(js::parse_int(x, 10)),
            Some(C4Attr::Named(n)) => named(rel, n),
            None => {}
        }
        match attrs.get(5) {
            Some(C4Attr::Str(y)) => rel.offset_y = Some(js::parse_int(y, 10)),
            Some(C4Attr::Named(n)) => named(rel, n),
            None => {}
        }
    }

    /// Ports `updateLayoutConfig`: ?c4ShapeInRow, ?c4BoundaryInRow. As in
    /// Mermaid, the slot decides which one a value sets, not its name.
    pub fn update_layout_config(&mut self, attrs: &[C4Attr]) {
        let count = |attr: Option<&C4Attr>| -> f64 {
            match attr {
                None => f64::NAN,
                Some(C4Attr::Str(s)) => js::parse_int(s, 10),
                Some(C4Attr::Named(n)) => js::parse_int(&n.value, 10),
            }
        };
        let shapes = count(attrs.first());
        let boundaries = count(attrs.get(1));
        if shapes >= 1.0 {
            self.shape_in_row = shapes;
        }
        if boundaries >= 1.0 {
            self.boundary_in_row = boundaries;
        }
    }
}
