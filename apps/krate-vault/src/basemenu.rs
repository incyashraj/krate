//! A base's toolbar menus, as the notes app has them: Sort (sort keys in
//! order, each a property and a direction), Group (a property and a
//! direction), Filter (groups of conditions for all views and for this
//! view, each condition a property, an operator for its type and a value),
//! Properties (the columns shown, in order, and their names) and Search.
//! Every change is written into the `.base` file the way the notes app
//! writes it (src/basefile.rs) and the tab reads it back.

use crate::app::{App, Ask, AskFor, Input, Menu, MenuAction, Overlay, View, TICKED};
use crate::basefile::{short_id, BaseDoc};
use crate::json::Value;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

/// Which toolbar menu is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Sort,
    Group,
    Filter,
    Properties,
}

/// A property's type, as the filter builder offers operators for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PType {
    Text,
    Number,
    Date,
    Checkbox,
    List,
    File,
}

/// A press in a base's menus.
#[derive(Debug, Clone, PartialEq)]
pub enum BaseUi {
    Tool(Panel),
    Search,
    Close,
    SortProp(usize),
    SortDir(usize),
    SortRemove(usize),
    SortAdd,
    GroupProp,
    GroupDir,
    GroupRemove,
    /// A column shown or not.
    Column(String),
    /// A column's name, to change.
    Rename(String),
    /// Open or fold "All views" in the filter menu.
    FilterAll,
    /// A group's conjunction, a condition's property, operator or value,
    /// removing one, adding a condition or a group: (all views, path).
    FilterConj(bool, Vec<usize>),
    FilterProp(bool, Vec<usize>),
    FilterOp(bool, Vec<usize>),
    FilterValue(bool, Vec<usize>),
    /// The condition as a formula, to write by hand.
    FilterFormula(bool, Vec<usize>),
    FilterRemove(bool, Vec<usize>),
    FilterAdd(bool, Vec<usize>),
    FilterAddGroup(bool, Vec<usize>),
}

/// A change picked from a dropdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseAct {
    SortProp(usize, String),
    SortDir(usize, String),
    SortAdd(String),
    GroupProp(String),
    GroupDir(String),
    /// From a column's own menu: sort it, hide it, group by it, summarize it.
    ColumnSort(String, String),
    HideColumn(String),
    GroupBy(String),
    Summary(String, Option<String>),
    FilterConj(bool, Vec<usize>, String),
    FilterProp(bool, Vec<usize>, String),
    FilterOp(bool, Vec<usize>, String),
}

/// A condition the simple builder can show: a property, an operator (as
/// written: `==`, `<`, `contains`, `!startsWith`...) and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cond {
    pub prop: String,
    pub op: String,
    pub value: String,
}

/// The notes app's name for a property (`file.ctime` is "created time").
pub fn prop_label(id: &str) -> String {
    let s = match id {
        "file.file" | "file" => "file",
        "file.name" => "name",
        "file.basename" => "file base name",
        "file.fullname" => "file full name",
        "file.path" => "path",
        "file.folder" => "folder",
        "file.ext" => "extension",
        "file.ctime" => "created time",
        "file.mtime" => "modified time",
        "file.size" => "file size",
        "file.links" => "file links",
        "file.embeds" => "file embeds",
        "file.tags" => "file tags",
        "file.backlinks" => "file backlinks",
        _ => "",
    };
    if !s.is_empty() {
        return String::from(s);
    }
    let bare = id.strip_prefix("note.").unwrap_or(id);
    String::from(bare.strip_prefix("formula.").unwrap_or(bare))
}

/// The file's own properties, in the order the notes app lists them.
pub const FILE_PROPS: [&str; 14] = [
    "file.file",
    "file.name",
    "file.basename",
    "file.fullname",
    "file.path",
    "file.folder",
    "file.ext",
    "file.ctime",
    "file.mtime",
    "file.size",
    "file.links",
    "file.embeds",
    "file.tags",
    "file.backlinks",
];

/// The operators for a type: what the menu says and what is written.
pub fn operators(t: PType) -> Vec<(&'static str, &'static str)> {
    let text = [
        ("is", "=="),
        ("is not", "!="),
        ("starts with", "startsWith"),
        ("ends with", "endsWith"),
        ("is empty", "isEmpty"),
        ("contains", "contains"),
        ("contains any of", "containsAny"),
        ("contains all of", "containsAll"),
        ("does not start with", "!startsWith"),
        ("does not end with", "!endsWith"),
        ("is not empty", "!isEmpty"),
        ("does not contain", "!contains"),
        ("does not contain any of", "!containsAny"),
        ("does not contain all of", "!containsAll"),
    ];
    match t {
        PType::Text => text.to_vec(),
        PType::Number => alloc::vec![
            ("=", "=="),
            ("\u{2260}", "!="),
            ("<", "<"),
            ("\u{2264}", "<="),
            (">", ">"),
            ("\u{2265}", ">="),
            ("is empty", "isEmpty"),
            ("is not empty", "!isEmpty"),
        ],
        PType::Date => alloc::vec![
            ("on", "=="),
            ("not on", "!="),
            ("before", "<"),
            ("on or before", "<="),
            ("after", ">"),
            ("on or after", ">="),
            ("is empty", "isEmpty"),
            ("is not empty", "!isEmpty"),
        ],
        PType::Checkbox => alloc::vec![("is", "=="), ("is not", "!=")],
        PType::List => alloc::vec![
            ("is exactly", "=="),
            ("is not exactly", "!="),
            ("is empty", "isEmpty"),
            ("contains", "contains"),
            ("contains any of", "containsAny"),
            ("contains all of", "containsAll"),
            ("is not empty", "!isEmpty"),
            ("does not contain", "!contains"),
            ("does not contain any of", "!containsAny"),
            ("does not contain all of", "!containsAll"),
        ],
        PType::File => alloc::vec![
            ("links to", "hasLink"),
            ("in folder", "inFolder"),
            ("has tag", "hasTag"),
            ("has property", "hasProperty"),
            ("does not link to", "!hasLink"),
            ("is not in folder", "!inFolder"),
            ("does not have tag", "!hasTag"),
            ("does not have property", "!hasProperty"),
        ],
    }
}

/// An operator that takes no value.
fn no_value(op: &str) -> bool {
    op.trim_start_matches('!') == "isEmpty"
}

fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A condition as the filter builder writes it: `status == "open"`,
/// `priority >= 2`, `!status.contains("x")`, `file.hasTag("task")`.
pub fn cond_text(c: &Cond, t: PType) -> String {
    let neg = c.op.starts_with('!') && c.op != "!=";
    let op = if neg { &c.op[1..] } else { c.op.as_str() };
    let p = short_id(&c.prop);
    let lit = |v: &str| -> String {
        match t {
            PType::Number if v.trim().parse::<f64>().is_ok() => String::from(v.trim()),
            PType::Checkbox => String::from(if v == "true" { "true" } else { "false" }),
            _ => quote(v),
        }
    };
    let body = match op {
        "==" | "!=" | "<" | "<=" | ">" | ">=" => format!("{p} {op} {}", lit(&c.value)),
        "isEmpty" => format!("{p}.isEmpty()"),
        "hasLink" | "inFolder" | "hasTag" | "hasProperty" => {
            format!("file.{op}({})", quote(&c.value))
        }
        "containsAny" | "containsAll" => {
            let args: Vec<String> = c
                .value
                .split(',')
                .map(|a| a.trim())
                .filter(|a| !a.is_empty())
                .map(quote)
                .collect();
            format!("{p}.{op}({})", args.join(", "))
        }
        f => format!("{p}.{f}({})", quote(&c.value)),
    };
    if neg {
        format!("!{body}")
    } else {
        body
    }
}

/// The string between the quotes of a quoted literal, or the bare text.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2
        && (s.starts_with('"') && s.ends_with('"') || s.starts_with('\'') && s.ends_with('\''))
    {
        let mut out = String::new();
        let mut esc = false;
        for c in s[1..s.len() - 1].chars() {
            if esc {
                out.push(c);
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else {
                out.push(c);
            }
        }
        out
    } else {
        String::from(s)
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-' || b >= 0x80)
}

/// A condition read back from what the builder writes, or none when it is
/// a formula the simple builder cannot show.
pub fn parse_cond(text: &str) -> Option<Cond> {
    let t = text.trim();
    let (neg, body) = match t.strip_prefix('!') {
        Some(b) if !b.starts_with('=') => (true, b.trim()),
        _ => (false, t),
    };
    // A method: `prop.fn(args)`.
    if let Some(open) = body.find('(') {
        if body.ends_with(')') {
            let head = &body[..open];
            let args = &body[open + 1..body.len() - 1];
            if let Some((recv, f)) = head.rsplit_once('.') {
                let known = [
                    "startsWith",
                    "endsWith",
                    "isEmpty",
                    "contains",
                    "containsAny",
                    "containsAll",
                    "hasLink",
                    "inFolder",
                    "hasTag",
                    "hasProperty",
                ];
                if known.contains(&f) && is_ident(recv) {
                    let file_fn = ["hasLink", "inFolder", "hasTag", "hasProperty"].contains(&f);
                    if file_fn && recv != "file" {
                        return None;
                    }
                    let value = args
                        .split(',')
                        .map(unquote)
                        .filter(|a| !a.is_empty())
                        .collect::<Vec<_>>()
                        .join(", ");
                    let prop = if file_fn {
                        String::from("file.file")
                    } else {
                        crate::basefile::full_id(recv)
                    };
                    let op = if neg {
                        format!("!{f}")
                    } else {
                        String::from(f)
                    };
                    return Some(Cond { prop, op, value });
                }
            }
            return None;
        }
    }
    if neg {
        return None;
    }
    for op in [">=", "<=", "==", "!=", "<", ">"] {
        if let Some(i) = body.find(op) {
            let lhs = body[..i].trim();
            let rhs = body[i + op.len()..].trim();
            if is_ident(lhs) && !rhs.is_empty() {
                return Some(Cond {
                    prop: crate::basefile::full_id(lhs),
                    op: String::from(op),
                    value: unquote(rhs),
                });
            }
            return None;
        }
    }
    None
}

/// The children of the group at `path` (each index a child that is a
/// group) under a filter root `{and|or|not: [...]}`.
fn group_mut<'a>(root: &'a mut Value, path: &[usize]) -> Option<&'a mut Vec<Value>> {
    let Value::Obj(kv) = root else {
        return None;
    };
    let (_, list) = kv.first_mut()?;
    let Value::Arr(items) = list else {
        return None;
    };
    match path.split_first() {
        None => Some(items),
        Some((i, rest)) => group_mut(items.get_mut(*i)?, rest),
    }
}

/// The conjunction of the group at `path`: `and`, `or` or `not`.
pub fn group_conj(root: &Value, path: &[usize]) -> String {
    let mut v = root;
    for i in path {
        let Value::Obj(kv) = v else {
            return String::from("and");
        };
        let Some((_, Value::Arr(items))) = kv.first() else {
            return String::from("and");
        };
        let Some(c) = items.get(*i) else {
            return String::from("and");
        };
        v = c;
    }
    match v {
        Value::Obj(kv) => kv.first().map_or(String::from("and"), |(k, _)| k.clone()),
        _ => String::from("and"),
    }
}

/// A root that is a single condition, as an `and` group of it.
fn as_group(v: Option<Value>) -> Value {
    match v {
        Some(Value::Obj(kv)) if !kv.is_empty() => Value::Obj(kv),
        Some(Value::Str(s)) => Value::Obj(alloc::vec![(
            String::from("and"),
            Value::Arr(alloc::vec![Value::Str(s)])
        )]),
        _ => Value::Obj(alloc::vec![(String::from("and"), Value::Arr(Vec::new()))]),
    }
}

/// A filter root with nothing in it is no filter at all.
fn tidy(v: Value) -> Option<Value> {
    match &v {
        Value::Obj(kv) => match kv.first() {
            Some((_, Value::Arr(a))) if a.is_empty() => None,
            _ => Some(v),
        },
        _ => Some(v),
    }
}

impl App {
    /// The base tab in focus: its file, the view, the parsed file.
    fn base_doc(&mut self) -> Option<(String, usize, BaseDoc)> {
        let (path, view) = match self.view() {
            Some(View::Base(b)) => (b.path.clone(), b.view),
            _ => return None,
        };
        let text = self.vault.as_mut()?.fs.read(&path).ok()?;
        Some((path, view, BaseDoc::parse(&text)))
    }

    /// Change the base in focus and write it back; the tab reads it again.
    pub fn base_apply(&mut self, f: impl FnOnce(&mut BaseDoc, usize)) {
        let Some((path, view, mut doc)) = self.base_doc() else {
            return;
        };
        f(&mut doc, view);
        let text = doc.to_text();
        if let Some(v) = self.vault.as_mut() {
            if let Err(e) = v.fs.write(&path, &text) {
                self.say(&e);
                return;
            }
        }
        if let Some(View::Base(b)) = self.view_mut() {
            b.base = crate::bases::Base::parse(&text);
            b.source = Some(text);
            b.generation = 0;
        }
    }

    /// Every property a base can show, as full ids: the note properties in
    /// the vault, the base's formulas, then the file's own.
    pub fn base_props(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some(View::Base(b)) = self.view() {
            for (f, _) in &b.base.formulas {
                out.push(format!("formula.{f}"));
            }
        }
        let mut notes: Vec<String> = self
            .all_properties()
            .into_iter()
            .map(|(k, _)| format!("note.{k}"))
            .collect();
        notes.sort();
        out.extend(notes);
        out.extend(FILE_PROPS.iter().map(|s| String::from(*s)));
        out
    }

    /// A property's type, read from its values in the vault.
    pub fn prop_type(&self, id: &str) -> PType {
        match id {
            "file.file" | "file" => return PType::File,
            "file.ctime" | "file.mtime" => return PType::Date,
            "file.size" => return PType::Number,
            "file.links" | "file.embeds" | "file.tags" | "file.backlinks" => return PType::List,
            "note.tags" | "note.aliases" | "note.cssclasses" => return PType::List,
            _ if id.starts_with("file.") || id.starts_with("formula.") => return PType::Text,
            _ => {}
        }
        let key = id.strip_prefix("note.").unwrap_or(id);
        let Some(v) = self.vault.as_ref() else {
            return PType::Text;
        };
        for m in v.meta.values() {
            let Some(val) = m.props.as_ref().and_then(|p| p.get(key)) else {
                continue;
            };
            return match val {
                Value::Num(_) => PType::Number,
                Value::Bool(_) => PType::Checkbox,
                Value::Arr(_) => PType::List,
                Value::Str(s)
                    if s.len() >= 10 && s.as_bytes()[4] == b'-' && s.as_bytes()[7] == b'-' =>
                {
                    PType::Date
                }
                Value::Null => continue,
                _ => PType::Text,
            };
        }
        PType::Text
    }

    fn base_menu_at(&mut self, items: Vec<(String, MenuAction)>) {
        let (x, y) = self.pointer;
        self.overlay = Some(Overlay::Menu(Menu {
            x: x - 10.0,
            y: y + 12.0,
            items,
        }));
    }

    fn prop_menu(&mut self, make: impl Fn(String) -> BaseAct, current: Option<&str>) {
        let items = self
            .base_props()
            .into_iter()
            .filter(|p| p != "note.tags" && p != "file.backlinks")
            .map(|p| {
                let mut label = prop_label(&p);
                if current == Some(p.as_str()) {
                    label.push_str(TICKED);
                }
                (label, MenuAction::Base(make(p)))
            })
            .collect();
        self.base_menu_at(items);
    }

    /// Directions as the menus name them for a property's type.
    fn dir_menu(&mut self, prop: &str, make: impl Fn(String) -> BaseAct, current: &str) {
        let (a, z) = dir_labels(self.prop_type(prop));
        let items = [(a, "ASC"), (z, "DESC")]
            .into_iter()
            .map(|(l, d)| {
                let mut label = String::from(l);
                if d == current {
                    label.push_str(TICKED);
                }
                (label, MenuAction::Base(make(String::from(d))))
            })
            .collect();
        self.base_menu_at(items);
    }

    /// A press in a base's toolbar or one of its menus.
    pub fn base_ui(&mut self, ui: BaseUi) {
        let Some(View::Base(b)) = self.view() else {
            return;
        };
        let w = b.base.views.get(b.view).cloned();
        let panel = b.panel;
        match ui {
            BaseUi::Tool(p) => {
                if let Some(View::Base(b)) = self.view_mut() {
                    b.panel = if panel == Some(p) { None } else { Some(p) };
                    b.search_on = false;
                    b.draft = None;
                }
            }
            BaseUi::Search => {
                if let Some(View::Base(b)) = self.view_mut() {
                    b.panel = None;
                    b.search_on = !b.search_on;
                    if !b.search_on {
                        b.search.clear();
                        b.generation = 0;
                    }
                }
            }
            BaseUi::Close => {
                if let Some(View::Base(b)) = self.view_mut() {
                    b.panel = None;
                    b.draft = None;
                }
            }
            BaseUi::SortProp(i) => {
                let cur = w.and_then(|w| w.sort.get(i).map(|s| s.0.clone()));
                self.prop_menu(move |p| BaseAct::SortProp(i, p), cur.as_deref());
            }
            BaseUi::SortDir(i) => {
                if let Some((p, desc)) = w.and_then(|w| w.sort.get(i).cloned()) {
                    self.dir_menu(
                        &p,
                        move |d| BaseAct::SortDir(i, d),
                        if desc { "DESC" } else { "ASC" },
                    );
                }
            }
            BaseUi::SortRemove(i) => self.base_apply(|d, v| {
                if let Some(s) = d.views.get_mut(v).and_then(|w| w.sort.as_mut()) {
                    if i < s.len() {
                        s.remove(i);
                    }
                }
            }),
            BaseUi::SortAdd => self.prop_menu(BaseAct::SortAdd, None),
            BaseUi::GroupProp => {
                let cur = w.and_then(|w| w.group_by.map(|g| g.0));
                self.prop_menu(BaseAct::GroupProp, cur.as_deref());
            }
            BaseUi::GroupDir => {
                if let Some((p, desc)) = w.and_then(|w| w.group_by) {
                    self.dir_menu(&p, BaseAct::GroupDir, if desc { "DESC" } else { "ASC" });
                }
            }
            BaseUi::GroupRemove => self.base_apply(|d, v| {
                if let Some(w) = d.views.get_mut(v) {
                    w.set_group_by(None);
                }
            }),
            BaseUi::Column(p) => self.base_apply(|d, v| {
                if let Some(w) = d.views.get_mut(v) {
                    let mut o = w.order.clone().unwrap_or_default();
                    match o.iter().position(|x| *x == p) {
                        Some(i) => {
                            o.remove(i);
                        }
                        None => o.push(p),
                    }
                    w.order = Some(o);
                }
            }),
            BaseUi::Rename(p) => {
                let cur = self
                    .base_doc()
                    .and_then(|(_, _, d)| d.display_name(&p).map(String::from))
                    .unwrap_or_else(|| prop_label(&p));
                let mut input = Input::with(&cur);
                input.select_all();
                self.overlay = Some(Overlay::Ask(Ask {
                    title: String::from("Display name"),
                    input,
                    purpose: AskFor::BaseRename(p),
                }));
            }
            BaseUi::FilterAll => {
                if let Some(View::Base(b)) = self.view_mut() {
                    b.filter_all_open = !b.filter_all_open;
                }
            }
            BaseUi::FilterConj(g, path) => {
                let root = self.filter_root(g);
                let cur = group_conj(&root, &path);
                let items = [
                    ("All the following are true", "and"),
                    ("Any of the following are true", "or"),
                    ("None of the following are true", "not"),
                ]
                .into_iter()
                .map(|(l, c)| {
                    let mut label = String::from(l);
                    if c == cur {
                        label.push_str(TICKED);
                    }
                    (
                        label,
                        MenuAction::Base(BaseAct::FilterConj(g, path.clone(), String::from(c))),
                    )
                })
                .collect();
                self.base_menu_at(items);
            }
            BaseUi::FilterProp(g, path) => {
                let cur = self.cond_at(g, &path).map(|c| c.prop);
                self.prop_menu(
                    move |p| BaseAct::FilterProp(g, path.clone(), p),
                    cur.as_deref(),
                );
            }
            BaseUi::FilterOp(g, path) => {
                let (prop, op) = match self.cond_at(g, &path) {
                    Some(c) => (c.prop, c.op),
                    None => match self.draft_of(g, &path) {
                        Some(p) => (p, String::new()),
                        None => return,
                    },
                };
                let items = operators(self.prop_type(&prop))
                    .into_iter()
                    .map(|(l, o)| {
                        let mut label = String::from(l);
                        if o == op {
                            label.push_str(TICKED);
                        }
                        (
                            label,
                            MenuAction::Base(BaseAct::FilterOp(g, path.clone(), String::from(o))),
                        )
                    })
                    .collect();
                self.base_menu_at(items);
            }
            BaseUi::FilterValue(g, path) => {
                let Some(c) = self.cond_at(g, &path) else {
                    return;
                };
                if no_value(&c.op) {
                    return;
                }
                let t = self.prop_type(&c.prop);
                if t == PType::Checkbox {
                    let flipped = if c.value == "true" { "false" } else { "true" };
                    let nc = Cond {
                        value: String::from(flipped),
                        ..c
                    };
                    self.set_cond(g, &path, &cond_text(&nc, t));
                    return;
                }
                let mut input = Input::with(&c.value);
                input.select_all();
                self.overlay = Some(Overlay::Ask(Ask {
                    title: prop_label(&c.prop),
                    input,
                    purpose: AskFor::BaseFilterValue(g, path),
                }));
            }
            BaseUi::FilterFormula(g, path) => {
                let mut root = self.filter_root(g);
                let cur = path
                    .split_last()
                    .and_then(|(last, parent)| group_mut(&mut root, parent)?.get(*last).cloned());
                let text = match cur {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let mut input = Input::with(&text);
                input.select_all();
                self.overlay = Some(Overlay::Ask(Ask {
                    title: String::from("Formula"),
                    input,
                    purpose: AskFor::BaseFilterFormula(g, path),
                }));
            }
            BaseUi::FilterRemove(g, path) => {
                if self.draft_of(g, &path).is_some() {
                    if let Some(View::Base(b)) = self.view_mut() {
                        b.draft = None;
                    }
                    return;
                }
                self.edit_filters(g, |root| {
                    let (last, parent) = match path.split_last() {
                        Some(x) => x,
                        None => return,
                    };
                    if let Some(items) = group_mut(root, parent) {
                        if *last < items.len() {
                            items.remove(*last);
                        }
                    }
                });
            }
            BaseUi::FilterAdd(g, path) => {
                // A row to fill in: it is written once it says something.
                let n = self.filter_root(g).clone();
                let mut root = n;
                let len = group_mut(&mut root, &path).map_or(0, |i| i.len());
                let mut p = path.clone();
                p.push(len);
                if let Some(View::Base(b)) = self.view_mut() {
                    b.draft = Some((g, p, None));
                }
            }
            BaseUi::FilterAddGroup(g, path) => self.edit_filters(g, |root| {
                if let Some(items) = group_mut(root, &path) {
                    items.push(Value::Obj(alloc::vec![(
                        String::from("and"),
                        Value::Arr(Vec::new())
                    )]));
                }
            }),
        }
    }

    /// The filter root of all views (`g`) or this view, as a group.
    pub fn filter_root(&self, g: bool) -> Value {
        let Some(View::Base(b)) = self.view() else {
            return as_group(None);
        };
        let text = b.source.clone().unwrap_or_default();
        let d = BaseDoc::parse(&text);
        as_group(if g {
            d.filters
        } else {
            d.views.get(b.view).and_then(|w| w.filters.clone())
        })
    }

    fn cond_at(&self, g: bool, path: &[usize]) -> Option<Cond> {
        let mut root = self.filter_root(g);
        let (last, parent) = path.split_last()?;
        let items = group_mut(&mut root, parent)?;
        match items.get(*last)? {
            Value::Str(s) => parse_cond(s),
            _ => None,
        }
    }

    fn draft_of(&self, g: bool, path: &[usize]) -> Option<String> {
        match self.view() {
            Some(View::Base(b)) => match &b.draft {
                Some((dg, dp, prop)) if *dg == g && dp.as_slice() == path => {
                    Some(prop.clone().unwrap_or_default())
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn edit_filters(&mut self, g: bool, f: impl FnOnce(&mut Value)) {
        self.base_apply(|d, v| {
            let cur = if g {
                d.filters.clone()
            } else {
                d.views.get(v).and_then(|w| w.filters.clone())
            };
            let mut root = as_group(cur);
            f(&mut root);
            let root = tidy(root);
            if g {
                d.filters = root;
            } else if let Some(w) = d.views.get_mut(v) {
                w.filters = root;
            }
        });
    }

    fn set_cond(&mut self, g: bool, path: &[usize], text: &str) {
        let text = String::from(text);
        let path = path.to_vec();
        self.edit_filters(g, |root| {
            let Some((last, parent)) = path.split_last() else {
                return;
            };
            if let Some(items) = group_mut(root, parent) {
                if *last < items.len() {
                    items[*last] = Value::Str(text);
                } else {
                    items.push(Value::Str(text));
                }
            }
        });
    }

    /// A change picked from one of a base menu's dropdowns.
    pub fn base_act(&mut self, a: BaseAct) {
        match a {
            BaseAct::SortProp(i, p) => self.base_apply(|d, v| {
                if let Some(s) = d.views.get_mut(v).and_then(|w| w.sort.as_mut()) {
                    if let Some(e) = s.get_mut(i) {
                        e.0 = p;
                    }
                }
            }),
            BaseAct::SortDir(i, dir) => self.base_apply(|d, v| {
                if let Some(s) = d.views.get_mut(v).and_then(|w| w.sort.as_mut()) {
                    if let Some(e) = s.get_mut(i) {
                        e.1 = dir;
                    }
                }
            }),
            BaseAct::SortAdd(p) => self.base_apply(|d, v| {
                if let Some(w) = d.views.get_mut(v) {
                    let s = w.sort.get_or_insert_with(Vec::new);
                    s.retain(|(x, _)| *x != p);
                    s.push((p, String::from("ASC")));
                }
            }),
            BaseAct::GroupProp(p) => self.base_apply(|d, v| {
                if let Some(w) = d.views.get_mut(v) {
                    let dir = w
                        .group_by
                        .as_ref()
                        .map_or(String::from("ASC"), |g| g.1.clone());
                    w.set_group_by(Some((p, dir)));
                }
            }),
            BaseAct::GroupDir(dir) => self.base_apply(|d, v| {
                if let Some(w) = d.views.get_mut(v) {
                    if let Some((p, _)) = w.group_by.clone() {
                        w.set_group_by(Some((p, dir)));
                    }
                }
            }),
            BaseAct::ColumnSort(p, dir) => self.base_apply(|d, v| {
                if let Some(w) = d.views.get_mut(v) {
                    w.set_sort_property(&p, &dir);
                }
            }),
            BaseAct::HideColumn(p) => self.base_apply(|d, v| {
                if let Some(w) = d.views.get_mut(v) {
                    if let Some(o) = w.order.as_mut() {
                        o.retain(|x| *x != p);
                    }
                }
            }),
            BaseAct::GroupBy(p) => self.base_apply(|d, v| {
                if let Some(w) = d.views.get_mut(v) {
                    w.set_group_by(Some((p, String::from("ASC"))));
                }
            }),
            BaseAct::Summary(p, name) => self.base_apply(|d, v| {
                if let Some(w) = d.views.get_mut(v) {
                    w.set_summary_key(&p, name.as_deref());
                }
            }),
            BaseAct::FilterConj(g, path, c) => self.edit_filters(g, |root| {
                // Find the group and rename its key.
                let mut node: &mut Value = root;
                for i in &path {
                    let Value::Obj(kv) = node else {
                        return;
                    };
                    let Some((_, Value::Arr(items))) = kv.first_mut() else {
                        return;
                    };
                    let Some(n) = items.get_mut(*i) else {
                        return;
                    };
                    node = n;
                }
                if let Value::Obj(kv) = node {
                    if let Some(first) = kv.first_mut() {
                        first.0 = c;
                    }
                }
            }),
            BaseAct::FilterProp(g, path, p) => {
                let t = self.prop_type(&p);
                let ops = operators(t);
                match self.cond_at(g, &path) {
                    Some(c) => {
                        let op = if ops.iter().any(|(_, o)| *o == c.op) {
                            c.op
                        } else {
                            String::from(ops[0].1)
                        };
                        let nc = Cond {
                            prop: p,
                            op,
                            value: c.value,
                        };
                        self.set_cond(g, &path, &cond_text(&nc, t));
                    }
                    None => {
                        if let Some(View::Base(b)) = self.view_mut() {
                            b.draft = Some((g, path, Some(p)));
                        }
                    }
                }
            }
            BaseAct::FilterOp(g, path, op) => {
                let (prop, value) = match self.cond_at(g, &path) {
                    Some(c) => (c.prop, c.value),
                    None => match self.draft_of(g, &path) {
                        Some(p) if !p.is_empty() => (p, String::new()),
                        _ => return,
                    },
                };
                let t = self.prop_type(&prop);
                let nc = Cond { prop, op, value };
                let complete = no_value(&nc.op) || !nc.value.is_empty() || t == PType::Checkbox;
                if complete {
                    if let Some(View::Base(b)) = self.view_mut() {
                        b.draft = None;
                    }
                    let nc = if t == PType::Checkbox && nc.value.is_empty() {
                        Cond {
                            value: String::from("true"),
                            ..nc
                        }
                    } else {
                        nc
                    };
                    self.set_cond(g, &path, &cond_text(&nc, t));
                } else {
                    // Ask for the value now; the row is written with it.
                    if let Some(View::Base(b)) = self.view_mut() {
                        b.draft_op = Some(nc.op.clone());
                    }
                    self.overlay = Some(Overlay::Ask(Ask {
                        title: prop_label(&nc.prop),
                        input: Input::default(),
                        purpose: AskFor::BaseFilterValue(g, path),
                    }));
                }
            }
        }
    }

    /// A condition written by hand as a formula.
    pub fn base_filter_formula(&mut self, g: bool, path: Vec<usize>, text: &str) {
        if let Some(View::Base(b)) = self.view_mut() {
            b.draft = None;
        }
        self.set_cond(g, &path, text);
    }

    /// The value typed for a filter condition.
    pub fn base_filter_value(&mut self, g: bool, path: Vec<usize>, value: &str) {
        let (prop, op) = match self.cond_at(g, &path) {
            Some(c) => (c.prop, c.op),
            None => {
                let prop = self.draft_of(g, &path).unwrap_or_default();
                let op = match self.view() {
                    Some(View::Base(b)) => b.draft_op.clone().unwrap_or_default(),
                    _ => String::new(),
                };
                if prop.is_empty() || op.is_empty() {
                    return;
                }
                (prop, op)
            }
        };
        if let Some(View::Base(b)) = self.view_mut() {
            b.draft = None;
            b.draft_op = None;
        }
        let t = self.prop_type(&prop);
        let nc = Cond {
            prop,
            op,
            value: String::from(value),
        };
        self.set_cond(g, &path, &cond_text(&nc, t));
    }

    /// A column's own menu (a press on its name), as the notes app has it.
    pub fn base_column_menu(&mut self, col: usize) {
        let Some(View::Base(b)) = self.view() else {
            return;
        };
        let Some(w) = b.base.views.get(b.view) else {
            return;
        };
        let Some(p) = w.order.get(col).cloned() else {
            return;
        };
        let p = crate::basefile::full_id(&p);
        let t = self.prop_type(&p);
        let (a, z) = dir_labels(t);
        let mut items: Vec<(String, MenuAction)> = alloc::vec![
            (
                format!("Sort {a}"),
                MenuAction::Base(BaseAct::ColumnSort(p.clone(), String::from("ASC")))
            ),
            (
                format!("Sort {z}"),
                MenuAction::Base(BaseAct::ColumnSort(p.clone(), String::from("DESC")))
            ),
            (
                String::from("Clear sort"),
                MenuAction::Base(BaseAct::ColumnSort(p.clone(), String::from("NONE")))
            ),
            (
                String::from("Group by this property"),
                MenuAction::Base(BaseAct::GroupBy(p.clone()))
            ),
        ];
        let current = w
            .summaries
            .iter()
            .find(|(k, _)| crate::basefile::full_id(k) == p)
            .map(|(_, n)| n.clone());
        let mut names: Vec<&str> = summary_names(t).to_vec();
        let customs: Vec<String> = b
            .base
            .custom_summaries
            .iter()
            .map(|(k, _)| k.clone())
            .collect();
        for n in &customs {
            names.push(n.as_str());
        }
        for n in names {
            let mut label = format!("Summarize: {n}");
            if current.as_deref() == Some(n) {
                label.push_str(TICKED);
            }
            items.push((
                label,
                MenuAction::Base(BaseAct::Summary(p.clone(), Some(String::from(n)))),
            ));
        }
        if current.is_some() {
            items.push((
                String::from("Hide summary"),
                MenuAction::Base(BaseAct::Summary(p.clone(), None)),
            ));
        }
        items.push((
            String::from("Hide column"),
            MenuAction::Base(BaseAct::HideColumn(p)),
        ));
        self.base_menu_at(items);
    }

    /// Typing into a base's search box. True when it took the text.
    pub fn base_search_text(&mut self, s: &str) -> bool {
        match self.view_mut() {
            Some(View::Base(b)) if b.search_on => {
                b.search.push_str(s);
                b.generation = 0;
                true
            }
            _ => false,
        }
    }

    /// A key while a base's search box is open. True when it took it.
    pub fn base_search_key(&mut self, key: &str) -> bool {
        match self.view_mut() {
            Some(View::Base(b)) if b.search_on => {
                match key {
                    "Backspace" => {
                        b.search.pop();
                    }
                    "Escape" => {
                        b.search_on = false;
                        b.search.clear();
                    }
                    _ => return false,
                }
                b.generation = 0;
                true
            }
            _ => false,
        }
    }
}

/// The two directions' names for a property's type.
pub fn dir_labels(t: PType) -> (&'static str, &'static str) {
    match t {
        PType::Number => ("0 \u{2192} 1", "1 \u{2192} 0"),
        PType::Date => ("Old to new", "New to old"),
        _ => ("A \u{2192} Z", "Z \u{2192} A"),
    }
}

/// The built-in summaries offered for a type, as the notes app offers them.
pub fn summary_names(t: PType) -> &'static [&'static str] {
    match t {
        PType::Number => &[
            "Average", "Min", "Max", "Sum", "Range", "Median", "Stddev", "Empty", "Filled",
            "Unique",
        ],
        PType::Date => &["Earliest", "Latest", "Range", "Empty", "Filled", "Unique"],
        PType::Checkbox => &["Checked", "Unchecked", "Empty", "Filled", "Unique"],
        _ => &["Empty", "Filled", "Unique"],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_are_written_as_the_filter_builder_writes_them() {
        // What the notes app 1.15.1's builder wrote for each choice (taken
        // through its developer tools).
        let c = |p: &str, op: &str, v: &str| Cond {
            prop: String::from(p),
            op: String::from(op),
            value: String::from(v),
        };
        let cases = [
            (
                c("note.status", "==", "open"),
                PType::Text,
                "status == \"open\"",
            ),
            (
                c("note.status", "!=", "open"),
                PType::Text,
                "status != \"open\"",
            ),
            (
                c("note.status", "startsWith", "op"),
                PType::Text,
                "status.startsWith(\"op\")",
            ),
            (
                c("note.status", "!contains", "pe"),
                PType::Text,
                "!status.contains(\"pe\")",
            ),
            (
                c("note.status", "isEmpty", ""),
                PType::Text,
                "status.isEmpty()",
            ),
            (
                c("note.status", "!isEmpty", ""),
                PType::Text,
                "!status.isEmpty()",
            ),
            (
                c("note.status", "containsAny", "open"),
                PType::Text,
                "status.containsAny(\"open\")",
            ),
            (
                c("note.priority", "==", "2"),
                PType::Number,
                "priority == 2",
            ),
            (
                c("note.priority", ">=", "2"),
                PType::Number,
                "priority >= 2",
            ),
            (
                c("note.due", "<", "2026-12-01"),
                PType::Date,
                "due < \"2026-12-01\"",
            ),
            (
                c("note.due", ">=", "2026-12-01"),
                PType::Date,
                "due >= \"2026-12-01\"",
            ),
            (
                c("file.file", "hasTag", "task"),
                PType::File,
                "file.hasTag(\"task\")",
            ),
            (
                c("file.file", "!hasTag", "task"),
                PType::File,
                "!file.hasTag(\"task\")",
            ),
        ];
        for (cond, t, want) in cases {
            assert_eq!(cond_text(&cond, t), want);
            assert_eq!(parse_cond(want), Some(cond.clone()), "{want}");
        }
        assert_eq!(
            parse_cond("(due - today()) > 3"),
            None,
            "a formula stays a formula"
        );
    }
}
