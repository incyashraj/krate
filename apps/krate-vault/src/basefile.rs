//! A `.base` file as the notes app reads and writes it, for editing: the
//! toolbar's Sort, Group, Filter and Properties menus change this and it is
//! written back the way the notes app writes one -- the same keys in the
//! same order, property ids shortened the same way (`note.status` is
//! written `status`), and the same YAML (the notes app's `stringifyYaml`:
//! plain scalars where they can be, double quotes otherwise, `|-` for text
//! on several lines, lists indented under their key). What it does not
//! know is kept where the notes app keeps it.
//!
//! Proved against files the notes app 1.15.1 itself wrote:
//! `fixtures/obsidian-bases.json`, each an input and the text its own code
//! wrote after the same change (taken through its developer tools).

use crate::json::Value;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

/// A view of a base, as the notes app keeps one.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewDoc {
    pub kind: String,
    pub name: String,
    pub filters: Option<Value>,
    /// The property (full id) and `ASC` or `DESC`.
    pub group_by: Option<(String, String)>,
    pub group_order: Option<Vec<String>>,
    pub order: Option<Vec<String>>,
    pub sort: Option<Vec<(String, String)>>,
    pub limit: Option<f64>,
    pub summaries: Option<Vec<(String, String)>>,
    /// Everything else the view says (card size, image...).
    pub data: Vec<(String, Value)>,
}

/// A whole `.base` file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BaseDoc {
    /// Top-level keys the notes app does not know, kept first.
    pub extra: Vec<(String, Value)>,
    pub filters: Option<Value>,
    pub formulas: Vec<(String, String)>,
    /// The base's own summaries: a name and a formula over `values`.
    pub summaries: Vec<(String, String)>,
    /// Per-property settings (`displayName`, and what else is there).
    pub properties: Vec<(String, Vec<(String, Value)>)>,
    pub views: Vec<ViewDoc>,
    pub new_item_folder: Option<String>,
    pub new_item_template: Option<String>,
}

/// A property id in full, as the notes app holds it: `status` is
/// `note.status`, `file` is `file.file`.
pub fn full_id(p: &str) -> String {
    if p.starts_with("note.") || p.starts_with("formula.") || p.starts_with("file.") {
        String::from(p)
    } else if p == "file" {
        String::from("file.file")
    } else {
        let mut s = String::from("note.");
        s.push_str(p);
        s
    }
}

/// A property id as the notes app writes it: `note.status` as `status`
/// (unless the rest starts with `file.`, `formula.` or `note.`).
pub fn short_id(p: &str) -> String {
    if p.len() > 5 && p.starts_with("note.") {
        let rest = &p[5..];
        let head = rest.split('.').next().unwrap_or("");
        if ["file", "formula", "note"].contains(&head) {
            String::from(p)
        } else {
            String::from(rest)
        }
    } else if p == "file.file" {
        String::from("file")
    } else {
        String::from(p)
    }
}

fn pairs(v: Option<&Value>) -> Vec<(String, Value)> {
    match v {
        Some(Value::Obj(kv)) => kv.clone(),
        _ => Vec::new(),
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.map(|a| a.arr().iter().map(crate::bases::show).collect())
        .unwrap_or_default()
}

impl BaseDoc {
    pub fn parse(text: &str) -> BaseDoc {
        let v = crate::yaml::parse(text);
        let mut d = BaseDoc::default();
        for (k, val) in pairs(Some(&v)) {
            match k.as_str() {
                "filters" => d.filters = Some(val),
                "formulas" => {
                    d.formulas = pairs(Some(&val))
                        .into_iter()
                        .map(|(k, f)| (k, crate::bases::show(&f)))
                        .collect()
                }
                "summaries" => {
                    d.summaries = pairs(Some(&val))
                        .into_iter()
                        .map(|(k, f)| (k, crate::bases::show(&f)))
                        .collect()
                }
                "properties" => {
                    d.properties = pairs(Some(&val))
                        .into_iter()
                        .map(|(k, p)| {
                            // The notes app keeps what it does not know first
                            // and the display name last.
                            let mut kv = pairs(Some(&p));
                            if let Some(i) = kv
                                .iter()
                                .position(|(k, v)| k == "displayName" && matches!(v, Value::Str(_)))
                            {
                                let dn = kv.remove(i);
                                kv.push(dn);
                            }
                            (k, kv)
                        })
                        .collect()
                }
                "views" => d.views = val.arr().iter().map(ViewDoc::parse).collect(),
                "newItemFolder" => d.new_item_folder = val.str().map(String::from),
                "newItemTemplate" => d.new_item_template = val.str().map(String::from),
                _ => d.extra.push((k, val)),
            }
        }
        d
    }

    /// The file's text, as the notes app writes it.
    pub fn to_text(&self) -> String {
        let mut top: Vec<(String, Value)> = self.extra.clone();
        if let Some(f) = &self.filters {
            top.push((String::from("filters"), f.clone()));
        }
        if !self.formulas.is_empty() {
            top.push((
                String::from("formulas"),
                Value::Obj(
                    self.formulas
                        .iter()
                        .map(|(k, f)| (k.clone(), Value::s(f)))
                        .collect(),
                ),
            ));
        }
        if !self.summaries.is_empty() {
            top.push((
                String::from("summaries"),
                Value::Obj(
                    self.summaries
                        .iter()
                        .map(|(k, f)| (k.clone(), Value::s(f)))
                        .collect(),
                ),
            ));
        }
        let props: Vec<(String, Value)> = self
            .properties
            .iter()
            .filter(|(_, kv)| !kv.is_empty())
            .map(|(k, kv)| (k.clone(), Value::Obj(kv.clone())))
            .collect();
        if !props.is_empty() {
            top.push((String::from("properties"), Value::Obj(props)));
        }
        if !self.views.is_empty() {
            top.push((
                String::from("views"),
                Value::Arr(self.views.iter().map(ViewDoc::to_value).collect()),
            ));
        }
        if let Some(f) = &self.new_item_folder {
            top.push((String::from("newItemFolder"), Value::s(f)));
        }
        if let Some(f) = &self.new_item_template {
            top.push((String::from("newItemTemplate"), Value::s(f)));
        }
        if top.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        emit_map(&mut out, &top, 0);
        out
    }

    /// A property's settings, made when it has none (as the notes app's
    /// `getPropertyConfig` does).
    fn property_mut(&mut self, id: &str) -> &mut Vec<(String, Value)> {
        let i = match self.properties.iter().position(|(k, _)| k == id) {
            Some(i) => i,
            None => {
                self.properties.push((String::from(id), Vec::new()));
                self.properties.len() - 1
            }
        };
        &mut self.properties[i].1
    }

    /// A property's name in the table, or none to use its own.
    pub fn set_display_name(&mut self, id: &str, name: &str) {
        let p = self.property_mut(id);
        p.retain(|(k, _)| k != "displayName");
        if !name.is_empty() {
            p.push((String::from("displayName"), Value::s(name)));
        }
    }

    pub fn display_name(&self, id: &str) -> Option<&str> {
        self.properties
            .iter()
            .find(|(k, _)| k == id || full_id(k) == full_id(id))
            .and_then(|(_, kv)| kv.iter().find(|(k, _)| k == "displayName"))
            .and_then(|(_, v)| v.str())
    }

    /// The base's and a view's filters (none clears them).
    pub fn set_filters(&mut self, global: Option<Value>, view: usize, own: Option<Value>) {
        self.filters = global;
        if let Some(w) = self.views.get_mut(view) {
            w.filters = own;
        }
    }

    /// A summary of the base's own: a name and a formula (none removes it).
    pub fn set_summary_formula(&mut self, name: &str, formula: Option<&str>) {
        match formula {
            Some(f) => match self.summaries.iter_mut().find(|(k, _)| k == name) {
                Some(e) => e.1 = String::from(f),
                None => self.summaries.push((String::from(name), String::from(f))),
            },
            None => self.summaries.retain(|(k, _)| k != name),
        }
    }
}

impl ViewDoc {
    fn parse(v: &Value) -> ViewDoc {
        let mut w = ViewDoc {
            kind: String::from(v.get_str("type").unwrap_or("table")),
            name: String::from(v.get_str("name").unwrap_or("Table")),
            filters: None,
            group_by: None,
            group_order: None,
            order: None,
            sort: None,
            limit: None,
            summaries: None,
            data: Vec::new(),
        };
        for (k, val) in pairs(Some(v)) {
            match k.as_str() {
                "type" | "name" => {}
                "filters" => w.filters = Some(val),
                "groupBy" => {
                    w.group_by = match &val {
                        Value::Str(p) => Some((full_id(p), String::from("ASC"))),
                        _ => val.get_str("property").map(|p| {
                            (
                                full_id(p),
                                String::from(val.get_str("direction").unwrap_or("ASC")),
                            )
                        }),
                    }
                }
                "groupOrder" => w.group_order = Some(strings(Some(&val))),
                "order" => w.order = Some(strings(Some(&val)).iter().map(|p| full_id(p)).collect()),
                "sort" => {
                    w.sort = Some(
                        val.arr()
                            .iter()
                            .filter_map(|e| {
                                Some((
                                    full_id(e.get_str("property")?),
                                    String::from(e.get_str("direction").unwrap_or("ASC")),
                                ))
                            })
                            .collect(),
                    )
                }
                "limit" => w.limit = val.num(),
                "summaries" => {
                    w.summaries = Some(
                        pairs(Some(&val))
                            .into_iter()
                            .map(|(p, n)| (full_id(&p), crate::bases::show(&n)))
                            .collect(),
                    )
                }
                _ => w.data.push((k, val)),
            }
        }
        w
    }

    fn to_value(&self) -> Value {
        let mut kv: Vec<(String, Value)> = alloc::vec![
            (String::from("type"), Value::s(&self.kind)),
            (String::from("name"), Value::s(&self.name)),
        ];
        if let Some(f) = &self.filters {
            kv.push((String::from("filters"), f.clone()));
        }
        if let Some((p, d)) = &self.group_by {
            kv.push((
                String::from("groupBy"),
                Value::Obj(alloc::vec![
                    (String::from("property"), Value::s(&short_id(p))),
                    (String::from("direction"), Value::s(d)),
                ]),
            ));
        }
        if let Some(g) = &self.group_order {
            kv.push((
                String::from("groupOrder"),
                Value::Arr(g.iter().map(|s| Value::s(s)).collect()),
            ));
        }
        if let Some(o) = &self.order {
            kv.push((
                String::from("order"),
                Value::Arr(o.iter().map(|p| Value::s(&short_id(p))).collect()),
            ));
        }
        if let Some(s) = &self.sort {
            kv.push((
                String::from("sort"),
                Value::Arr(
                    s.iter()
                        .map(|(p, d)| {
                            Value::Obj(alloc::vec![
                                (String::from("property"), Value::s(&short_id(p))),
                                (String::from("direction"), Value::s(d)),
                            ])
                        })
                        .collect(),
                ),
            ));
        }
        if let Some(l) = self.limit {
            if l != 0.0 {
                kv.push((String::from("limit"), Value::Num(l)));
            }
        }
        if let Some(s) = &self.summaries {
            kv.push((
                String::from("summaries"),
                Value::Obj(s.iter().map(|(p, n)| (short_id(p), Value::s(n))).collect()),
            ));
        }
        for (k, v) in &self.data {
            if !kv.iter().any(|(x, _)| x == k) {
                kv.push((k.clone(), v.clone()));
            }
        }
        Value::Obj(kv)
    }

    /// The notes app's `setSortProperty`: `ASC`, `DESC` or `NONE` puts the
    /// property first that way (or takes it out); `TOGGLE` goes A to Z, Z
    /// to A, then off.
    pub fn set_sort_property(&mut self, prop: &str, dir: &str) {
        let mut list = self.sort.clone().unwrap_or_default();
        let mut dir = String::from(dir);
        if let Some(i) = list.iter().position(|(p, _)| p == prop) {
            if dir == "TOGGLE" {
                dir = String::from(if list[i].1 == "ASC" { "DESC" } else { "NONE" });
            }
            list.remove(i);
        }
        if dir == "TOGGLE" {
            dir = String::from("ASC");
        }
        if dir == "ASC" || dir == "DESC" {
            list.insert(0, (String::from(prop), dir));
        }
        self.sort = Some(list);
    }

    /// The notes app's `setGroupBy`: a new property starts its group order
    /// again; none takes grouping off.
    pub fn set_group_by(&mut self, to: Option<(String, String)>) {
        match (&to, &self.group_by) {
            (None, None) => {}
            (None, Some(_)) => {
                self.group_by = None;
                self.group_order = None;
            }
            (Some(t), e) => {
                if e.as_ref() == Some(t) {
                    return;
                }
                if e.as_ref().map(|x| &x.0) != Some(&t.0) {
                    self.group_order = self.group_order.as_ref().map(|_| Vec::new());
                }
                self.group_by = to;
            }
        }
    }

    /// The notes app's `setSummaryKey`: a summary under a column, or none.
    pub fn set_summary_key(&mut self, prop: &str, name: Option<&str>) {
        match name {
            None => {
                if let Some(s) = self.summaries.as_mut() {
                    s.retain(|(p, _)| p != prop);
                }
            }
            Some(n) => {
                let s = self.summaries.get_or_insert_with(Vec::new);
                match s.iter_mut().find(|(p, _)| p == prop) {
                    Some(e) => e.1 = String::from(n),
                    None => s.push((String::from(prop), String::from(n))),
                }
            }
        }
    }
}

// ---- YAML, as the notes app's stringifyYaml writes it ------------------------------

fn pad(out: &mut String, n: usize) {
    for _ in 0..n {
        out.push(' ');
    }
}

fn emit_map(out: &mut String, kv: &[(String, Value)], indent: usize) {
    for (i, (k, v)) in kv.iter().enumerate() {
        if i > 0 {
            pad(out, indent);
        }
        out.push_str(&scalar(k));
        out.push(':');
        emit_after_key(out, v, indent);
    }
}

/// A value after `key:`: on the same line, or below it.
fn emit_after_key(out: &mut String, v: &Value, indent: usize) {
    match v {
        Value::Obj(kv) if kv.is_empty() => out.push_str(" {}\n"),
        Value::Arr(a) if a.is_empty() => out.push_str(" []\n"),
        Value::Obj(kv) => {
            out.push('\n');
            pad(out, indent + 2);
            emit_map(out, kv, indent + 2);
        }
        Value::Arr(a) => {
            out.push('\n');
            emit_seq(out, a, indent + 2);
        }
        Value::Str(s) if s.contains('\n') => {
            block(out, s, indent + 2);
        }
        Value::Null => out.push('\n'),
        _ => {
            out.push(' ');
            out.push_str(&leaf(v));
            out.push('\n');
        }
    }
}

fn emit_seq(out: &mut String, a: &[Value], indent: usize) {
    for v in a {
        pad(out, indent);
        out.push_str("- ");
        match v {
            Value::Obj(kv) if !kv.is_empty() => emit_map(out, kv, indent + 2),
            Value::Arr(inner) if !inner.is_empty() => {
                // A list in a list: its first item on the dash's line.
                let mut sub = String::new();
                emit_seq(&mut sub, inner, indent + 2);
                out.push_str(sub.trim_start());
            }
            Value::Obj(_) => out.push_str("{}\n"),
            Value::Arr(_) => out.push_str("[]\n"),
            Value::Str(s) if s.contains('\n') => {
                let mut sub = String::new();
                block(&mut sub, s, indent + 2);
                out.push_str(sub.trim_start());
            }
            _ => {
                out.push_str(&leaf(v));
                out.push('\n');
            }
        }
    }
}

/// Text on several lines: a `|` block (`|-` without a final line break).
fn block(out: &mut String, s: &str, indent: usize) {
    let (body, chomp) = match s.strip_suffix('\n') {
        Some(b) if !b.ends_with('\n') => (b, ""),
        Some(_) => (s.trim_end_matches('\n'), "+"),
        None => (s, "-"),
    };
    out.push_str(" |");
    out.push_str(chomp);
    out.push('\n');
    for line in body.split('\n') {
        if !line.is_empty() {
            pad(out, indent);
            out.push_str(line);
        }
        out.push('\n');
    }
}

fn leaf(v: &Value) -> String {
    match v {
        Value::Str(s) => scalar(s),
        Value::Num(n) => {
            let mut s = String::new();
            crate::json::push_num(&mut s, *n);
            s
        }
        Value::Bool(b) => String::from(if *b { "true" } else { "false" }),
        Value::Null => String::new(),
        _ => crate::json::compact(v),
    }
}

/// Whether text would read back as something other than text: a number,
/// true or false, null.
fn reads_as_other(s: &str) -> bool {
    if matches!(
        s,
        "true" | "True" | "TRUE" | "false" | "False" | "FALSE" | "null" | "Null" | "NULL" | "~"
    ) {
        return true;
    }
    let t = s.strip_prefix(['+', '-']).unwrap_or(s);
    if matches!(t, ".inf" | ".Inf" | ".INF") || matches!(s, ".nan" | ".NaN" | ".NAN") {
        return true;
    }
    if let Some(h) = s.strip_prefix("0x") {
        return !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit());
    }
    if let Some(o) = s.strip_prefix("0o") {
        return !o.is_empty() && o.bytes().all(|b| (b'0'..=b'7').contains(&b));
    }
    // [-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?
    let b = t.as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize| {
        let s = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > s
    };
    let mant = if b.first() == Some(&b'.') {
        i = 1;
        digits(&mut i)
    } else {
        let d = digits(&mut i);
        if d && b.get(i) == Some(&b'.') {
            i += 1;
            digits(&mut i);
        }
        d
    };
    if !mant {
        return false;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if !digits(&mut i) {
            return false;
        }
    }
    i == b.len()
}

/// A text scalar on one line: plain when it reads back as the same text,
/// else quoted -- double quotes, or single when it holds a double quote
/// and no single one.
pub fn scalar(s: &str) -> String {
    let first = s.chars().next();
    let second = s.chars().nth(1);
    let plain = !s.is_empty()
        && !reads_as_other(s)
        && !s.starts_with(' ')
        && !s.ends_with(' ')
        && !s.contains(": ")
        && !s.contains(" #")
        && !s.ends_with(':')
        && !s.contains('\t')
        && !s.contains('\n')
        && !matches!(
            first,
            Some(
                ',' | '['
                    | ']'
                    | '{'
                    | '}'
                    | '#'
                    | '&'
                    | '*'
                    | '!'
                    | '|'
                    | '>'
                    | '\''
                    | '"'
                    | '%'
                    | '@'
                    | '`'
            )
        )
        && !(matches!(first, Some('-' | '?' | ':')) && matches!(second, None | Some(' ')));
    if plain {
        return String::from(s);
    }
    if s.contains('"') && !s.contains('\'') {
        let mut out = String::from("'");
        out.push_str(s);
        out.push('\'');
        return out;
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same change made to the notes app's own query in its own code.
    fn apply(d: &mut BaseDoc, case: &str) {
        let s = |x: &str| String::from(x);
        match case {
            "identity" => {}
            "sort-add-status" => d.views[0].set_sort_property("note.status", "ASC"),
            "sort-toggle-priority" => d.views[0].set_sort_property("note.priority", "TOGGLE"),
            "sort-remove-name" => d.views[0].set_sort_property("file.name", "NONE"),
            "group-status-desc" => d.views[1].set_group_by(Some((s("note.status"), s("DESC")))),
            "group-none" => d.views[0].set_group_by(None),
            "group-change" => d.views[0].set_group_by(Some((s("note.due"), s("ASC")))),
            "order-two" => d.views[0].order = Some(alloc::vec![s("file.name"), s("note.due")]),
            "display-name" => d.set_display_name("note.priority", "Prio"),
            "filters" => {
                let g = crate::yaml::parse("and:\n  - file.hasTag(\"task\")\n  - priority > 2\n");
                let v = crate::yaml::parse(
                    "or:\n  - status == \"open\"\n  - not:\n      - due < today()\n",
                );
                d.set_filters(Some(g), 0, Some(v));
            }
            "filters-clear-view" => {
                let g = crate::yaml::parse("and:\n  - file.hasTag(\"task\")\n");
                d.set_filters(Some(g), 0, None);
            }
            "summary-key" => d.views[0].set_summary_key("note.due", Some("Latest")),
            "summary-remove" => d.views[0].set_summary_key("note.status", None),
            "custom-summary" => {
                d.set_summary_formula("Twice", Some("values.sum() * 2"));
                d.views[0].set_summary_key("note.priority", Some("Twice"));
            }
            "limit" => d.views[0].limit = Some(10.0),
            "names" => {
                for (p, n) in [
                    ("note.a", "a: b"),
                    ("note.b", "#hash"),
                    ("note.c", "true"),
                    ("note.status", "12"),
                    ("note.priority", "say \"hi\""),
                    ("note.due", "it's"),
                    ("file.name", " pad"),
                    ("note.x", "both ' and \""),
                    ("note.y", "- dash"),
                    ("note.z", "line\nbreak"),
                    ("note.w", "émoji ✓"),
                    ("note.v", "null"),
                    ("note.u", "@at"),
                    ("note.t", "a #b"),
                    ("note.s", "[x]"),
                ] {
                    d.set_display_name(p, n);
                }
            }
            other => panic!("no such case {other}"),
        }
    }

    #[test]
    fn writes_bases_exactly_as_the_notes_app_writes_them() {
        let all = crate::json::parse(include_str!("../fixtures/obsidian-bases.json")).unwrap();
        let Value::Obj(cases) = all else {
            panic!("fixtures")
        };
        assert!(cases.len() >= 20);
        for (name, c) in cases {
            let input = c.get_str("input").unwrap();
            let want = c.get_str("output").unwrap();
            let case = name.split("--").nth(1).unwrap();
            let mut d = BaseDoc::parse(input);
            apply(&mut d, case);
            let got = d.to_text();
            assert_eq!(got, want, "case {name}\n--- got\n{got}\n--- want\n{want}");
        }
    }

    #[test]
    fn a_changed_file_reads_back_the_same() {
        let all = crate::json::parse(include_str!("../fixtures/obsidian-bases.json")).unwrap();
        let Value::Obj(cases) = all else {
            panic!("fixtures")
        };
        for (name, c) in cases {
            let out = c.get_str("output").unwrap();
            let again = BaseDoc::parse(out).to_text();
            assert_eq!(again, out, "case {name}");
        }
    }
}
