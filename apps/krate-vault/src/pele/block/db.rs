//! Ports diagrams/block/db.ts: what the block parser builds, and Mermaid's
//! setHierarchy, which turns the statements into the tree of blocks.
//!
//! Blocks live in an arena (`arena`); Pele's object identity is a place
//! in it.

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::types::{Block, BlockClassDef, Statement};
use crate::pele::common::ordered::OrderedMap;
use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js::{self, trim};

const MAX_SPACES: f64 = 100000.0;

/// Ports `typeStr2Type`.
pub fn type_str2_type(type_str: Option<&str>) -> &'static str {
    match type_str {
        Some("[]") => "square",
        Some("()") => "round",
        Some("(())") => "circle",
        Some(">]") => "rect_left_inv_arrow",
        Some("{}") => "diamond",
        Some("{{}}") => "hexagon",
        Some("([])") => "stadium",
        Some("[[]]") => "subroutine",
        Some("[()]") => "cylinder",
        Some("((()))") => "doublecircle",
        Some("[//]") => "lean_right",
        Some("[\\\\]") => "lean_left",
        Some("[/\\]") => "trapezoid",
        Some("[\\/]") => "inv_trapezoid",
        Some("<[]>") => "block_arrow",
        _ => "na",
    }
}

/// Ports `edgeStrToEdgeData`.
pub fn edge_str_to_edge_data(type_str: &str) -> &'static str {
    match trim(type_str).chars().last() {
        Some('x') => "arrow_cross",
        Some('o') => "arrow_circle",
        Some('>') => "arrow_point",
        _ => "",
    }
}

/// Ports `edgeStrToEdgeStartData`.
pub fn edge_str_to_edge_start_data(type_str: &str) -> &'static str {
    match trim(type_str).chars().next() {
        Some('x') => "arrow_cross",
        Some('o') => "arrow_circle",
        Some('<') => "arrow_point",
        _ => "arrow_open",
    }
}

/// Ports `edgeStrToThickness`.
pub fn edge_str_to_thickness(type_str: &str) -> &'static str {
    if type_str.contains("==") {
        "thick"
    } else {
        "normal"
    }
}

/// Ports `edgeStrToPattern`.
pub fn edge_str_to_pattern(type_str: &str) -> &'static str {
    if type_str.contains(".-") {
        "dotted"
    } else {
        "solid"
    }
}

/// Ports `interface Frame`. `columns` feeds only Mermaid's width warning,
/// which the port does not give.
#[allow(dead_code)]
struct Frame {
    list: Vec<usize>,
    at: usize,
    parent: usize,
    children: Vec<usize>,
    columns: f64,
}

/// Ports `class BlockDb`.
#[derive(Debug, Clone)]
pub struct BlockDb {
    pub title: Option<String>,
    pub arena: Vec<Block>,
    pub root: usize,
    pub blocks: Vec<usize>,
    pub edges: Vec<usize>,
    pub classes: OrderedMap<BlockClassDef>,
    // Mermaid keeps the root in here too, which makes a block called `root` vanish. Here it stays out.
    pub by_id: OrderedMap<usize>,
    // Ids that a class or style statement named before any block had them.
    pending: BTreeSet<String>,
    edge_count: OrderedMap<f64>,
    ids: usize,
    colors: usize,
    spaces: f64,
}

impl Default for BlockDb {
    fn default() -> Self {
        BlockDb {
            title: None,
            arena: alloc::vec![Block {
                id: Some("root".into()),
                kind: Some("composite".into()),
                children: Some(Vec::new()),
                columns: Some(-1.0),
                ..Block::default()
            }],
            root: 0,
            blocks: Vec::new(),
            edges: Vec::new(),
            classes: OrderedMap::new(),
            by_id: OrderedMap::new(),
            pending: BTreeSet::new(),
            edge_count: OrderedMap::new(),
            ids: 0,
            colors: 0,
            spaces: 0.0,
        }
    }
}

impl BlockDb {
    pub fn new() -> BlockDb {
        BlockDb::default()
    }

    /// A new block in the arena.
    pub fn add(&mut self, block: Block) -> usize {
        self.arena.push(block);
        self.arena.len() - 1
    }

    /// Ports `generateId`: Mermaid's ids are random. These are counted, so
    /// the same text always gives the same output.
    pub fn generate_id(&mut self) -> String {
        self.ids += 1;
        format!("id-{}", self.ids)
    }

    /// Ports `addStyleClass`.
    fn add_style_class(&mut self, id: &str, style_attributes: &str) {
        if !self.classes.contains_key(id) {
            self.classes.insert(
                id,
                BlockClassDef {
                    id: id.into(),
                    styles: Vec::new(),
                    text_styles: Vec::new(),
                },
            );
        }
        let found = self.classes.get_mut(id).unwrap();
        for attribute in style_attributes.split(',') {
            let fixed: String = trim(&js::replace_first(attribute, ";", "")).into();
            if attribute.contains("color") {
                found.text_styles.push(js::replace_first(
                    &js::replace_first(&fixed, "fill", "bgFill"),
                    "color",
                    "fill",
                ));
            }
            found.styles.push(fixed);
        }
    }

    /// Ports `addStyle2Node`.
    fn add_style2_node(&mut self, id: &str, styles: &str) {
        let k = self.named(id);
        self.arena[k].styles = Some(styles.split(',').map(String::from).collect());
    }

    /// Ports `setCssClass`: Mermaid looks each id up before trimming it, so
    /// `class a, b hot` misses b. Here it is trimmed first.
    fn set_css_class(&mut self, item_ids: &str, css_class_name: &str) {
        for id in item_ids.split(',') {
            let k = self.named(trim(id));
            self.arena[k]
                .classes
                .get_or_insert_with(Vec::new)
                .push(css_class_name.into());
        }
    }

    /// Ports `named`: Mermaid throws on a style for an unknown id, and a
    /// class for one makes the block of that id disappear when it is
    /// declared later. Here both wait for the block.
    fn named(&mut self, id: &str) -> usize {
        if let Some(&k) = self.by_id.get(id) {
            return k;
        }
        let k = self.add(Block {
            id: Some(id.into()),
            kind: Some("na".into()),
            children: Some(Vec::new()),
            ..Block::default()
        });
        self.by_id.insert(id, k);
        self.pending.insert(id.into());
        k
    }

    /// Ports `frame`.
    fn frame(&self, statements: &[Statement], parent: usize) -> Frame {
        let mut list: Vec<usize> = Vec::new();
        for s in statements {
            match s {
                Statement::One(k) => list.push(*k),
                Statement::Chain(ks) => list.extend_from_slice(ks),
            }
        }
        let mut columns = -1.0;
        for &k in &list {
            if self.arena[k].kind.as_deref() == Some("column-setting") {
                columns = self.arena[k].columns.unwrap_or(-1.0);
                break;
            }
        }
        Frame {
            list,
            at: 0,
            parent,
            children: Vec::new(),
            columns,
        }
    }

    /// Ports `setHierarchy`.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    pub fn set_hierarchy(&mut self, statements: Vec<Statement>) -> Result<(), PeleError> {
        let first = self.frame(&statements, self.root);
        let mut stack: Vec<Frame> = alloc::vec![first];
        while let Some(f) = stack.last_mut() {
            if f.at == f.list.len() {
                let f = stack.pop().unwrap();
                self.arena[f.parent].children = Some(f.children);
                continue;
            }
            let b = f.list[f.at];
            f.at += 1;
            let kind = self.arena[b].kind.clone();
            match kind.as_deref() {
                Some("classDef") => {
                    let id = self.arena[b].id.clone().unwrap_or_default();
                    let css = self.arena[b].css.clone().unwrap_or_default();
                    self.add_style_class(&id, &css);
                    continue;
                }
                Some("applyClass") => {
                    let id = self.arena[b].id.clone().unwrap_or_default();
                    let class = self.arena[b].style_class.clone().unwrap_or_default();
                    self.set_css_class(&id, &class);
                    continue;
                }
                Some("applyStyles") => {
                    if let Some(styles) = self.arena[b].styles_str.clone().filter(|s| !s.is_empty())
                    {
                        let id = self.arena[b].id.clone().unwrap_or_default();
                        self.add_style2_node(&id, &styles);
                    }
                    continue;
                }
                Some("column-setting") => {
                    let columns = self.arena[b].columns.unwrap_or(-1.0);
                    let parent = stack.last().unwrap().parent;
                    self.arena[parent].columns = Some(columns);
                    continue;
                }
                Some("edge") => {
                    let id = self.arena[b].id.clone().unwrap_or_default();
                    let count = self.edge_count.get(&id).copied().unwrap_or(0.0) + 1.0;
                    self.edge_count.insert(&id, count);
                    self.arena[b].id = Some(format!("{}-{}", js::num_str(count), id));
                    self.edges.push(b);
                    continue;
                }
                _ => {}
            }

            // A composite whose header was a link statement has no id of its own.
            if self.arena[b].id.is_none() {
                let id = self.generate_id();
                self.arena[b].id = Some(id);
            }
            let id = self.arena[b].id.clone().unwrap();
            if self.arena[b].label.as_deref().is_none_or(str::is_empty) {
                self.arena[b].label = Some(if kind.as_deref() == Some("composite") {
                    String::new()
                } else {
                    id.clone()
                });
            }
            let known = self.by_id.get(&id).copied();
            let fresh = known.is_none() || self.pending.remove(&id);
            if !fresh {
                let known = known.unwrap();
                if kind.as_deref() != Some("na") {
                    self.arena[known].kind = kind.clone();
                }
                if self.arena[b].label.as_deref() != Some(id.as_str()) {
                    self.arena[known].label = self.arena[b].label.clone();
                }
            } else {
                if let Some(known) = known {
                    self.arena[b].classes = self.arena[known].classes.clone();
                    self.arena[b].styles = self.arena[known].styles.clone();
                }
                if kind.as_deref() == Some("composite") {
                    self.arena[b].color_index = Some(self.colors);
                    self.colors += 1;
                }
                self.by_id.insert(&id, b);
            }

            let inner = self.arena[b].stmts.take();
            self.arena[b].children = Some(Vec::new());
            if kind.as_deref() == Some("space") {
                let width = self.arena[b].width.unwrap_or(1.0);
                if !(width <= MAX_SPACES - self.spaces) {
                    return Err(PeleError::of(
                        format!(
                            "A block diagram can hold at most {} space blocks.",
                            js::num_str(MAX_SPACES)
                        ),
                        PeleErrorCode::Limit,
                        "block",
                    ));
                }
                self.spaces += if width > 0.0 { width } else { 0.0 };
                let mut j = 0.0;
                while j < width {
                    let mut copy = self.arena[b].clone();
                    copy.id = Some(format!("{}-{}", id, js::num_str(j)));
                    copy.children = Some(Vec::new());
                    let copy_id = copy.id.clone().unwrap();
                    let k = self.add(copy);
                    self.by_id.insert(&copy_id, k);
                    stack.last_mut().unwrap().children.push(k);
                    j += 1.0;
                }
            } else if fresh {
                stack.last_mut().unwrap().children.push(b);
            }
            if let Some(inner) = inner.filter(|i| !i.is_empty()) {
                let frame = self.frame(&inner, b);
                stack.push(frame);
            }
        }
        self.blocks = self.arena[self.root].children.clone().unwrap_or_default();
        Ok(())
    }
}
