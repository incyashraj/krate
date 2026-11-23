//! Ports diagrams/class/db.ts: what the class parser builds, as Mermaid's
//! ClassDB keeps it.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::members::ClassMember;
use super::types::{
    ClassInterface, ClassNode, ClassNote, ClassRelation, NamespaceNode, StyleClass,
};
use crate::pele::common::ordered::OrderedMap;
use crate::pele::js::{self, trim};
use crate::pele::text::entities::decode_entities;
use crate::pele::util::url::sanitize_url;

/// Ports `RELATION_TYPE`.
pub mod relation_type {
    pub const AGGREGATION: u8 = 0;
    pub const EXTENSION: u8 = 1;
    pub const COMPOSITION: u8 = 2;
    pub const DEPENDENCY: u8 = 3;
    pub const LOLLIPOP: u8 = 4;
}

/// Mermaid looks up ids that start with a digit under this prefix, so they
/// never match a class (`DOM_ID_PREFIX`).
const DOM_ID_PREFIX: &str = "classId-";

/// Ports `splitName`: (className, type).
pub fn split_name(id: &str) -> (String, String) {
    let Some(at) = id.find('~').filter(|&a| a > 0) else {
        return (id.into(), String::new());
    };
    let end = id[at + 1..].find('~').map_or(id.len(), |k| at + 1 + k);
    (id[..at].into(), id[at + 1..end].into())
}

/// Ports `lookupId`.
fn lookup_id(id: &str) -> String {
    if id.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        split_name(&format!("{}{}", DOM_ID_PREFIX, id)).0
    } else {
        split_name(id).0
    }
}

/// Ports `class ClassDb`.
#[derive(Debug, Clone, Default)]
pub struct ClassDb {
    pub direction: String,
    pub classes: OrderedMap<ClassNode>,
    pub relations: Vec<ClassRelation>,
    pub notes: OrderedMap<ClassNote>,
    pub namespaces: OrderedMap<NamespaceNode>,
    pub interfaces: Vec<ClassInterface>,
    pub style_classes: OrderedMap<StyleClass>,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    namespace_stack: Vec<String>,
    /// The ids of the classes tagged with each CSS class, in the order tagged.
    with_css_class: OrderedMap<Vec<String>>,
}

impl ClassDb {
    pub fn new() -> ClassDb {
        ClassDb {
            direction: "TB".into(),
            ..ClassDb::default()
        }
    }

    /// Ports `addClass`.
    pub fn add_class(&mut self, id: &str) {
        let (class_name, kind) = split_name(id);
        if self.classes.contains_key(&class_name) {
            return;
        }
        let node = ClassNode {
            id: class_name.clone(),
            kind,
            label: class_name.clone(),
            css_classes: "default".into(),
            ..ClassNode::default()
        };
        self.classes.insert(&class_name, node);
        self.tag(&class_name, "default");
    }

    /// Ports `tag`.
    fn tag(&mut self, node: &str, css_class: &str) {
        match self.with_css_class.get_mut(css_class) {
            Some(tagged) => {
                if !tagged.iter().any(|n| n == node) {
                    tagged.push(node.into());
                }
            }
            None => self
                .with_css_class
                .insert(css_class, alloc::vec![String::from(node)]),
        }
    }

    /// Ports `ensure`: the class for a name that may carry a generic type,
    /// created if needed.
    fn ensure(&mut self, id: &str) -> &mut ClassNode {
        self.add_class(id);
        let name = split_name(id).0;
        self.classes.get_mut(&name).unwrap()
    }

    /// Ports `setClassLabel`.
    pub fn set_class_label(&mut self, id: &str, label: &str) {
        self.ensure(id).label = label.into();
    }

    /// Ports `addRelation`.
    pub fn add_relation(&mut self, mut relation: ClassRelation) {
        let (type1, type2) = (relation.relation.type1, relation.relation.type2);
        if type1 == Some(relation_type::LOLLIPOP) && type2.is_none() {
            self.add_class(&relation.id2);
            relation.id1 = self.add_interface(&relation.id1, &relation.id2);
        } else if type2 == Some(relation_type::LOLLIPOP) && type1.is_none() {
            self.add_class(&relation.id1);
            relation.id2 = self.add_interface(&relation.id2, &relation.id1);
        } else {
            self.add_class(&relation.id1);
            self.add_class(&relation.id2);
        }
        relation.id1 = split_name(&relation.id1).0;
        relation.id2 = split_name(&relation.id2).0;
        relation.relation_title1 = trim(&relation.relation_title1).into();
        relation.relation_title2 = trim(&relation.relation_title2).into();
        self.relations.push(relation);
    }

    /// Ports `addInterface`.
    fn add_interface(&mut self, label: &str, class_id: &str) -> String {
        let id = format!("interface{}", self.interfaces.len());
        self.interfaces.push(ClassInterface {
            id: id.clone(),
            label: label.into(),
            class_id: class_id.into(),
        });
        id
    }

    /// Ports `addAnnotation`.
    pub fn add_annotation(&mut self, class_name: &str, annotation: &str) {
        self.ensure(class_name).annotations.push(annotation.into());
    }

    /// Ports `addMember`.
    pub fn add_member(&mut self, class_name: &str, member: &str) {
        let node = self.ensure(class_name);
        let text = trim(member);
        if text.starts_with("<<") && text.ends_with(">>") {
            // substring(2, length - 2), its ends in either order.
            let chars: Vec<char> = text.chars().collect();
            let (a, b) = (2usize, chars.len().saturating_sub(2));
            let (a, b) = (a.min(b), a.max(b));
            node.annotations.push(chars[a..b].iter().collect());
        } else if text.find(')').is_some_and(|k| k > 0) {
            let parsed = ClassMember::new(text, "method");
            if parsed.member_type == "method" {
                node.methods.push(parsed);
            } else {
                node.members.push(parsed);
            }
        } else if !text.is_empty() {
            node.members.push(ClassMember::new(text, "attribute"));
        }
    }

    /// Ports `addMembers`.
    pub fn add_members(&mut self, class_name: &str, mut members: Vec<String>) {
        members.reverse();
        for member in &members {
            self.add_member(class_name, member);
        }
    }

    /// Ports `addNote`.
    pub fn add_note(&mut self, text: &str, class_name: Option<&str>) -> String {
        let index = self.notes.len();
        let id = format!("note{}", index);
        self.notes.insert(
            &id,
            ClassNote {
                id: id.clone(),
                class: class_name.map(String::from),
                text: text.into(),
                index,
                parent: None,
            },
        );
        id
    }

    /// Ports `cleanupLabel`.
    pub fn cleanup_label(&self, label: &str) -> String {
        trim(label.strip_prefix(':').unwrap_or(label)).into()
    }

    /// Ports `setCssClass`.
    pub fn set_css_class(&mut self, ids: &str, class_name: &str) {
        for id in ids.split(',') {
            let key = lookup_id(id);
            let Some(node) = self.classes.get_mut(&key) else {
                continue;
            };
            node.css_classes.push(' ');
            node.css_classes.push_str(class_name);
            for word in class_name.split(' ') {
                self.tag(&key, word);
            }
        }
    }

    /// Ports `defineClass`.
    pub fn define_class(&mut self, ids: &[String], style: &[String]) {
        for id in ids {
            if !self.style_classes.contains_key(id) {
                self.style_classes.insert(
                    id,
                    StyleClass {
                        id: id.clone(),
                        styles: Vec::new(),
                        text_styles: Vec::new(),
                    },
                );
            }
            let style_class = self.style_classes.get_mut(id).unwrap();
            for s in style {
                if s.contains("color") {
                    style_class
                        .text_styles
                        .push(js::replace_first(s, "fill", "bgFill"));
                }
                style_class.styles.push(s.clone());
            }
            // Mermaid tests `cssClasses.includes(id)`, which also matches part of a longer name.
            // Whole names are compared here.
            let Some(tagged) = self.with_css_class.get(id) else {
                continue;
            };
            let tagged = tagged.clone();
            let parts: Vec<&str> = style.iter().flat_map(|s| s.split(',')).collect();
            for node in &tagged {
                if let Some(n) = self.classes.get_mut(node) {
                    for part in &parts {
                        n.styles.push(String::from(*part));
                    }
                }
            }
        }
    }

    /// Ports `setTooltip`.
    pub fn set_tooltip(&mut self, ids: &str, tooltip: Option<&str>) {
        let Some(tooltip) = tooltip else {
            return;
        };
        for id in ids.split(',') {
            if let Some(node) = self.classes.get_mut(&split_name(id).0) {
                node.tooltip = Some(tooltip.into());
            }
        }
    }

    /// Ports `setLink`.
    pub fn set_link(&mut self, ids: &str, link: &str, target: Option<&str>) {
        // Entity codes are resolved first, so that what is checked is what a browser would follow.
        let decoded = decode_entities(link);
        let url = trim(&decoded);
        for id in ids.split(',') {
            if let Some(node) = self.classes.get_mut(&lookup_id(id)) {
                node.link = if url.is_empty() {
                    None
                } else {
                    Some(sanitize_url(url))
                };
                node.link_target = Some(target.unwrap_or("_blank").into());
            }
        }
        self.set_css_class(ids, "clickable");
    }

    /// Ports `setClickEvent`: callbacks are never executed; the class is
    /// only marked clickable, as in Mermaid's strict mode.
    pub fn set_click_event(&mut self, ids: &str) {
        for id in ids.split(',') {
            if let Some(node) = self.classes.get_mut(&split_name(id).0) {
                node.have_callback = true;
            }
        }
        self.set_css_class(ids, "clickable");
    }

    /// Ports `setDirection`.
    pub fn set_direction(&mut self, dir: &str) {
        self.direction = dir.into();
    }

    /// Ports `setAccTitle`.
    pub fn set_acc_title(&mut self, text: &str) {
        self.acc_title = Some(js::trim_start(text).into());
    }

    /// Ports `setAccDescription`: `text.replace(/\n\s+/g, '\n')`.
    pub fn set_acc_description(&mut self, text: &str) {
        self.acc_descr = Some(newline_runs(text));
    }

    /// Ports `addNamespace`.
    pub fn add_namespace(&mut self, id: &str, label: Option<&str>) -> String {
        let prefix = self.namespace_stack.last().cloned();
        let qualified = match prefix.as_deref() {
            Some(p) if !p.is_empty() => format!("{}.{}", p, id),
            _ => id.into(),
        };
        self.namespace_stack.push(qualified.clone());

        if let Some(existing) = self.namespaces.get_mut(&qualified) {
            existing.explicit = true;
            if let Some(l) = label.filter(|l| !l.is_empty()) {
                existing.label = l.into();
            }
            return qualified;
        }

        // The enclosing namespace and its ancestors exist already, so only the parts of `id` are walked.
        let prefix = prefix.filter(|p| !p.is_empty());
        let mut parent_id: Option<String> = prefix.clone();
        let mut from = prefix.as_ref().map_or(0, |p| p.len() + 1);
        loop {
            let dot = qualified[from.min(qualified.len())..]
                .find('.')
                .map(|k| from + k);
            let leaf = dot.is_none();
            let current: String = match dot {
                None => qualified.clone(),
                Some(d) => qualified[..d].into(),
            };
            if !self.namespaces.contains_key(&current) {
                let part: String =
                    qualified[from.min(qualified.len())..dot.unwrap_or(qualified.len())].into();
                let node = NamespaceNode {
                    id: current.clone(),
                    label: match label {
                        Some(l) if leaf && !l.is_empty() => l.into(),
                        _ => part,
                    },
                    classes: Vec::new(),
                    notes: Vec::new(),
                    children: Vec::new(),
                    parent: parent_id.clone(),
                    explicit: leaf,
                };
                self.namespaces.insert(&current, node);
            } else if leaf {
                self.namespaces.get_mut(&current).unwrap().explicit = true;
            }
            if let Some(pid) = parent_id.as_deref() {
                if let Some(parent) = self.namespaces.get_mut(pid) {
                    if !parent.children.contains(&current) {
                        parent.children.push(current.clone());
                    }
                    let pid = String::from(pid);
                    let node = self.namespaces.get_mut(&current).unwrap();
                    if node.parent.is_none() {
                        node.parent = Some(pid);
                    }
                }
            }
            if leaf {
                return qualified;
            }
            parent_id = Some(current);
            from = dot.unwrap() + 1;
        }
    }

    /// Ports `popNamespace`.
    pub fn pop_namespace(&mut self) {
        self.namespace_stack.pop();
    }

    /// Ports `addClassesToNamespace`.
    pub fn add_classes_to_namespace(
        &mut self,
        id: &str,
        class_names: &[String],
        note_names: &[String],
    ) {
        if !self.namespaces.contains_key(id) {
            return;
        }
        for name in class_names {
            let class_name = split_name(name).0;
            let Some(node) = self.classes.get_mut(&class_name) else {
                continue;
            };
            node.parent = Some(id.into());
            let ns = self.namespaces.get_mut(id).unwrap();
            if !ns.classes.contains(&class_name) {
                ns.classes.push(class_name);
            }
        }
        for name in note_names {
            let Some(note) = self.notes.get_mut(name) else {
                continue;
            };
            note.parent = Some(id.into());
            let ns = self.namespaces.get_mut(id).unwrap();
            if !ns.notes.contains(name) {
                ns.notes.push(name.clone());
            }
        }
    }

    /// Ports `setCssStyle`.
    pub fn set_css_style(&mut self, id: &str, styles: &[String]) {
        let Some(node) = self.classes.get_mut(id) else {
            return;
        };
        for s in styles {
            for part in s.split(',') {
                node.styles.push(part.into());
            }
        }
    }
}

/// `text.replace(/\n\s+/g, '\n')`.
pub fn newline_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '\n' {
            while chars.peek().is_some_and(|&c| js::is_space(c)) {
                chars.next();
            }
        }
    }
    out
}
