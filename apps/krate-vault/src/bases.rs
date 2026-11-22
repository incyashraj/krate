//! Bases: `.base` files, a table over the vault's notes. A base says which
//! notes (filters on their properties, tags, folder and name), which
//! columns (`order`), how to sort, and what to call each column. Read in
//! the notes app's own format, so the same `.base` file shows the same rows
//! in both apps.

use crate::json::Value;
use crate::text::lower;
use crate::vault::Vault;
use crate::vfs::{file_name, parent};
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    And(Vec<Filter>),
    Or(Vec<Filter>),
    Not(Vec<Filter>),
    Expr(String),
}

impl Filter {
    fn from(v: &Value) -> Option<Filter> {
        match v {
            Value::Str(s) => Some(Filter::Expr(s.clone())),
            Value::Obj(_) => {
                let list = |k: &str| -> Option<Vec<Filter>> {
                    Some(v.get(k)?.arr().iter().filter_map(Filter::from).collect())
                };
                if let Some(a) = list("and") {
                    Some(Filter::And(a))
                } else if let Some(o) = list("or") {
                    Some(Filter::Or(o))
                } else {
                    list("not").map(Filter::Not)
                }
            }
            _ => None,
        }
    }

    pub fn test(&self, row: &Row) -> bool {
        match self {
            Filter::And(a) => a.iter().all(|f| f.test(row)),
            Filter::Or(o) => o.is_empty() || o.iter().any(|f| f.test(row)),
            Filter::Not(n) => !n.iter().any(|f| f.test(row)),
            Filter::Expr(e) => truthy(&eval(e, row)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub kind: String,
    pub name: String,
    pub filters: Option<Filter>,
    pub order: Vec<String>,
    /// (property, descending)
    pub sort: Vec<(String, bool)>,
    pub limit: Option<usize>,
    /// Rows grouped by this property (`groupBy`), descending when true.
    pub group_by: Option<(String, bool)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Base {
    pub filters: Option<Filter>,
    /// Column names the base gives properties (`displayName`).
    pub names: Vec<(String, String)>,
    pub views: Vec<View>,
    /// `formulas`: name and expression; a column `formula.name` shows it.
    pub formulas: Vec<(String, String)>,
    /// Today, `YYYY-MM-DD`, set by the app before rows are worked out.
    pub today: String,
}

/// One note, as a base sees it.
pub struct Row<'a> {
    pub path: &'a str,
    pub tags: &'a [String],
    pub links: &'a [(String, u32)],
    pub props: Option<&'a Value>,
    /// The base's formulas, for `formula.name`.
    pub formulas: &'a [(String, String)],
    /// Today, `YYYY-MM-DD`, for `today()` and `now()`.
    pub today: &'a str,
}

impl Base {
    pub fn parse(text: &str) -> Base {
        let v = crate::yaml::parse(text);
        let names = match v.get("properties") {
            Some(Value::Obj(kv)) => kv
                .iter()
                .filter_map(|(k, p)| Some((k.clone(), String::from(p.get_str("displayName")?))))
                .collect(),
            _ => Vec::new(),
        };
        let mut views: Vec<View> = v
            .get("views")
            .map(|a| {
                a.arr()
                    .iter()
                    .map(|w| View {
                        kind: String::from(w.get_str("type").unwrap_or("table")),
                        name: String::from(w.get_str("name").unwrap_or("Table")),
                        filters: w.get("filters").and_then(Filter::from),
                        order: w
                            .get("order")
                            .map(|o| {
                                o.arr()
                                    .iter()
                                    .filter_map(|s| s.str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        sort: w
                            .get("sort")
                            .map(|s| {
                                s.arr()
                                    .iter()
                                    .filter_map(|e| {
                                        Some((
                                            String::from(e.get_str("property")?),
                                            e.get_str("direction")
                                                .is_some_and(|d| d.eq_ignore_ascii_case("desc")),
                                        ))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        limit: w.get_num("limit").map(|n| n as usize),
                        group_by: w.get("groupBy").and_then(|g| {
                            let p = match g {
                                Value::Str(p) => p.clone(),
                                _ => String::from(g.get_str("property")?),
                            };
                            let desc = g
                                .get_str("direction")
                                .is_some_and(|d| d.eq_ignore_ascii_case("desc"));
                            Some((p, desc))
                        }),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if views.is_empty() {
            views.push(View {
                kind: String::from("table"),
                name: String::from("Table"),
                filters: None,
                order: alloc::vec![String::from("file.name")],
                sort: Vec::new(),
                limit: None,
                group_by: None,
            });
        }
        let formulas = match v.get("formulas") {
            Some(Value::Obj(kv)) => kv.iter().map(|(k, e)| (k.clone(), show(e))).collect(),
            _ => Vec::new(),
        };
        Base {
            filters: v.get("filters").and_then(Filter::from),
            names,
            views,
            formulas,
            today: String::new(),
        }
    }

    /// What a column is called: its display name, else the property name
    /// without `note.` (`file.name` shows as "name").
    pub fn column_name(&self, prop: &str) -> String {
        let bare = prop.strip_prefix("note.").unwrap_or(prop);
        for (k, n) in &self.names {
            if k == prop || k.strip_prefix("note.").unwrap_or(k) == bare {
                return n.clone();
            }
        }
        let bare = bare.strip_prefix("formula.").unwrap_or(bare);
        String::from(bare.strip_prefix("file.").unwrap_or(bare))
    }

    /// The rows of view `view`: paths of the notes it shows, in order.
    pub fn rows(&self, v: &Vault, view: usize) -> Vec<String> {
        let Some(w) = self.views.get(view) else {
            return Vec::new();
        };
        let none: Vec<String> = Vec::new();
        let nolinks: Vec<(String, u32)> = Vec::new();
        let mut out: Vec<(String, Vec<Value>)> = Vec::new();
        for n in &v.notes {
            if !n.path.ends_with(".md") {
                continue;
            }
            let meta = v.meta.get(&n.path);
            let row = Row {
                path: &n.path,
                tags: meta.map_or(&none, |m| &m.tags),
                links: meta.map_or(&nolinks, |m| &m.links),
                props: meta.and_then(|m| m.props.as_ref()),
                formulas: &self.formulas,
                today: &self.today,
            };
            let ok = self.filters.as_ref().is_none_or(|f| f.test(&row))
                && w.filters.as_ref().is_none_or(|f| f.test(&row));
            if ok {
                let keys = w.sort.iter().map(|(p, _)| value(p, &row)).collect();
                out.push((n.path.clone(), keys));
            }
        }
        out.sort_by(|a, b| {
            for (i, (_, desc)) in w.sort.iter().enumerate() {
                let (x, y) = (&a.1[i], &b.1[i]);
                let o = compare(x, y);
                // Empty values go last either way.
                let empty = matches!(x, Value::Null) || matches!(y, Value::Null);
                let o = if *desc && !empty { o.reverse() } else { o };
                if o != core::cmp::Ordering::Equal {
                    return o;
                }
            }
            a.0.cmp(&b.0)
        });
        if let Some(l) = w.limit {
            out.truncate(l);
        }
        out.into_iter().map(|(p, _)| p).collect()
    }
}

/// A property's value for a row (`file.name`, `note.status`, `status`).
pub fn value(prop: &str, row: &Row) -> Value {
    let name = file_name(row.path);
    let base = name.rsplit_once('.').map_or(name, |(b, _)| b);
    match prop {
        "file.name" | "file.basename" => return Value::s(base),
        "file.path" => return Value::s(row.path),
        "file.folder" => return Value::s(parent(row.path)),
        "file.ext" => {
            return Value::s(name.rsplit_once('.').map_or("", |(_, e)| e));
        }
        "file.tags" => {
            return Value::Arr(row.tags.iter().map(|t| Value::s(t)).collect());
        }
        _ => {}
    }
    if let Some(f) = prop.strip_prefix("formula.") {
        // A formula sees the note's properties, not other formulas.
        let Some((_, e)) = row.formulas.iter().find(|(n, _)| n == f) else {
            return Value::Null;
        };
        let inner = Row {
            formulas: &[],
            ..*row
        };
        return eval(e, &inner);
    }
    let key = prop.strip_prefix("note.").unwrap_or(prop);
    if let Some(v) = row.props.and_then(|p| p.get(key)) {
        return v.clone();
    }
    // A date's parts: `due.year`, `due.month`, `due.day`.
    if let Some((obj, part)) = key.rsplit_once('.') {
        if let Some(d) = as_date(&value(obj, row)) {
            return match part {
                "year" => Value::Num(f64::from(d.year)),
                "month" => Value::Num(f64::from(d.month)),
                "day" => Value::Num(f64::from(d.day)),
                _ => Value::Null,
            };
        }
        if part == "length" {
            return match value(obj, row) {
                Value::Str(s) => Value::Num(s.chars().count() as f64),
                Value::Arr(a) => Value::Num(a.len() as f64),
                _ => Value::Null,
            };
        }
    }
    Value::Null
}

/// A value that is a date (`YYYY-MM-DD`, with or without a time).
fn as_date(v: &Value) -> Option<crate::dates::When> {
    let s = v.str()?;
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |a: usize, z: usize| s.get(a..z)?.parse::<u32>().ok();
    let mut w = crate::dates::When::date(num(0, 4)? as i32, num(5, 7)? as u8, num(8, 10)? as u8);
    if b.len() >= 16 && (b[10] == b'T' || b[10] == b' ') {
        w.hour = num(11, 13).unwrap_or(0) as u8;
        w.minute = num(14, 16).unwrap_or(0) as u8;
    }
    Some(w)
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Num(n) => *n != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Arr(a) => !a.is_empty(),
        Value::Obj(_) => true,
    }
}

/// Order two values: numbers by size, text case-insensitively, empty last.
pub fn compare(a: &Value, b: &Value) -> core::cmp::Ordering {
    use core::cmp::Ordering::*;
    match (a, b) {
        (Value::Null, Value::Null) => Equal,
        (Value::Null, _) => Greater,
        (_, Value::Null) => Less,
        (Value::Num(x), Value::Num(y)) => x.partial_cmp(y).unwrap_or(Equal),
        _ => lower(&show(a)).cmp(&lower(&show(b))),
    }
}

/// A value as a cell shows it.
pub fn show(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Bool(b) => String::from(if *b { "true" } else { "false" }),
        Value::Num(n) => {
            let mut s = String::new();
            crate::json::push_num(&mut s, *n);
            s
        }
        Value::Str(s) => s.clone(),
        Value::Arr(a) => {
            let parts: Vec<String> = a.iter().map(show).collect();
            parts.join(", ")
        }
        Value::Obj(_) => crate::json::compact(v),
    }
}

fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Num(x), Value::Num(y)) => (x - y).abs() < 1e-9,
        (Value::Null, Value::Null) => true,
        (Value::Null, Value::Str(s)) | (Value::Str(s), Value::Null) => s.is_empty(),
        (Value::Arr(x), y) | (y, Value::Arr(x)) if !matches!(y, Value::Arr(_)) => {
            x.iter().any(|i| equal(i, y))
        }
        _ => lower(&show(a)) == lower(&show(b)),
    }
}

// ---- expressions ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Ident(String),
    Op(&'static str),
}

fn lex(s: &str) -> Vec<Tok> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            let mut t = String::new();
            while j < b.len() && b[j] != c {
                if b[j] == b'\\' && j + 1 < b.len() {
                    j += 1;
                }
                let ch = s[j..].chars().next().unwrap_or(' ');
                t.push(ch);
                j += ch.len_utf8();
            }
            out.push(Tok::Str(t));
            i = j + 1;
            continue;
        }
        if c.is_ascii_digit() {
            let mut j = i;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b'.') {
                j += 1;
            }
            out.push(Tok::Num(s[i..j].parse().unwrap_or(0.0)));
            i = j;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            let mut j = i;
            while j < b.len()
                && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'.' || b[j] >= 0x80)
            {
                j += 1;
            }
            out.push(Tok::Ident(String::from(&s[i..j])));
            i = j;
            continue;
        }
        let two = s.get(i..i + 2).unwrap_or("");
        let op: &'static str = match two {
            "==" => "==",
            "!=" => "!=",
            ">=" => ">=",
            "<=" => "<=",
            "&&" => "&&",
            "||" => "||",
            _ => match c {
                b'>' => ">",
                b'<' => "<",
                b'!' => "!",
                b'(' => "(",
                b')' => ")",
                b',' => ",",
                b'+' => "+",
                b'-' => "-",
                b'*' => "*",
                b'/' => "/",
                b'[' => "[",
                b']' => "]",
                _ => "?",
            },
        };
        i += op.len().max(1);
        out.push(Tok::Op(op));
    }
    out
}

/// Evaluate one filter expression for a row.
pub fn eval(src: &str, row: &Row) -> Value {
    let toks = lex(src);
    let mut p = Ex {
        t: &toks,
        i: 0,
        row,
    };
    p.or()
}

struct Ex<'a, 'r> {
    t: &'a [Tok],
    i: usize,
    row: &'a Row<'r>,
}

impl Ex<'_, '_> {
    fn peek_op(&self) -> Option<&'static str> {
        match self.t.get(self.i) {
            Some(Tok::Op(o)) => Some(o),
            _ => None,
        }
    }

    fn or(&mut self) -> Value {
        let mut v = self.and();
        while self.peek_op() == Some("||") {
            self.i += 1;
            let r = self.and();
            v = Value::Bool(truthy(&v) || truthy(&r));
        }
        v
    }

    fn and(&mut self) -> Value {
        let mut v = self.cmp();
        while self.peek_op() == Some("&&") {
            self.i += 1;
            let r = self.cmp();
            v = Value::Bool(truthy(&v) && truthy(&r));
        }
        v
    }

    fn cmp(&mut self) -> Value {
        let a = self.sum();
        let Some(op) = self.peek_op() else {
            return a;
        };
        if !matches!(op, "==" | "!=" | ">" | "<" | ">=" | "<=") {
            return a;
        }
        self.i += 1;
        let b = self.sum();
        use core::cmp::Ordering::*;
        Value::Bool(match op {
            "==" => equal(&a, &b),
            "!=" => !equal(&a, &b),
            _ if matches!(a, Value::Null) || matches!(b, Value::Null) => false,
            ">" => compare(&a, &b) == Greater,
            "<" => compare(&a, &b) == Less,
            ">=" => compare(&a, &b) != Less,
            _ => compare(&a, &b) != Greater,
        })
    }

    fn product(&mut self) -> Value {
        let mut v = self.unary();
        while let Some(op @ ("*" | "/")) = self.peek_op() {
            self.i += 1;
            let r = self.unary();
            v = match (&v, &r, op) {
                (Value::Num(a), Value::Num(b), "*") => Value::Num(a * b),
                (Value::Num(a), Value::Num(b), _) if *b != 0.0 => Value::Num(a / b),
                _ => Value::Null,
            };
        }
        v
    }

    fn sum(&mut self) -> Value {
        let mut v = self.product();
        while let Some(op @ ("+" | "-")) = self.peek_op() {
            self.i += 1;
            let r = self.product();
            v = match (&v, &r, op) {
                (Value::Num(a), Value::Num(b), "+") => Value::Num(a + b),
                (Value::Num(a), Value::Num(b), _) => Value::Num(a - b),
                // Two dates apart: days between them.
                (_, _, "-") if as_date(&v).is_some() && as_date(&r).is_some() => {
                    let (a, b) = (
                        as_date(&v).map(|d| d.day_number()),
                        as_date(&r).map(|d| d.day_number()),
                    );
                    // A duration, shown as the notes app shows one ("9 days").
                    let n = a.unwrap_or(0) - b.unwrap_or(0);
                    let mut s = String::new();
                    if n < 0 {
                        s.push('-');
                    }
                    crate::text::push_num(&mut s, n.unsigned_abs() as usize);
                    s.push_str(if n.abs() == 1 { " day" } else { " days" });
                    Value::Str(s)
                }
                (_, _, "+") => {
                    let mut s = show(&v);
                    s.push_str(&show(&r));
                    Value::Str(s)
                }
                _ => Value::Null,
            };
        }
        v
    }

    fn unary(&mut self) -> Value {
        match self.peek_op() {
            Some("!") => {
                self.i += 1;
                Value::Bool(!truthy(&self.unary()))
            }
            Some("-") => {
                self.i += 1;
                match self.unary() {
                    Value::Num(n) => Value::Num(-n),
                    _ => Value::Null,
                }
            }
            _ => self.atom(),
        }
    }

    fn args(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        if self.peek_op() != Some("(") {
            return out;
        }
        self.i += 1;
        if self.peek_op() == Some(")") {
            self.i += 1;
            return out;
        }
        loop {
            out.push(self.or());
            match self.peek_op() {
                Some(",") => self.i += 1,
                Some(")") => {
                    self.i += 1;
                    break;
                }
                _ => break,
            }
        }
        out
    }

    fn atom(&mut self) -> Value {
        let Some(t) = self.t.get(self.i).cloned() else {
            return Value::Null;
        };
        self.i += 1;
        match t {
            Tok::Num(n) => Value::Num(n),
            Tok::Str(s) => Value::Str(s),
            Tok::Op("(") => {
                let v = self.or();
                if self.peek_op() == Some(")") {
                    self.i += 1;
                }
                v
            }
            Tok::Op("[") => {
                let mut a = Vec::new();
                while self.peek_op() != Some("]") && self.i < self.t.len() {
                    a.push(self.or());
                    if self.peek_op() == Some(",") {
                        self.i += 1;
                    }
                }
                self.i += 1;
                Value::Arr(a)
            }
            Tok::Ident(id) => self.ident(&id),
            _ => Value::Null,
        }
    }

    fn ident(&mut self, id: &str) -> Value {
        match id {
            "true" => return Value::Bool(true),
            "false" => return Value::Bool(false),
            "null" => return Value::Null,
            _ => {}
        }
        let called = self.peek_op() == Some("(");
        if !called {
            return value(id, self.row);
        }
        let args = self.args();
        let arg_str = |i: usize| args.get(i).map(show).unwrap_or_default();
        let row = self.row;
        let num = |i: usize| match args.get(i) {
            Some(Value::Num(n)) => Some(*n),
            Some(Value::Str(s)) => s.trim().parse::<f64>().ok(),
            _ => None,
        };
        match id {
            "if" => {
                return if truthy(args.first().unwrap_or(&Value::Null)) {
                    args.get(1).cloned().unwrap_or(Value::Null)
                } else {
                    args.get(2).cloned().unwrap_or(Value::Null)
                };
            }
            "today" | "now" => {
                return Value::s(row.today);
            }
            "date" => {
                return match args.first().and_then(as_date) {
                    Some(d) => Value::Str(d.format("YYYY-MM-DD")),
                    None => Value::Null,
                };
            }
            "number" => return num(0).map_or(Value::Null, Value::Num),
            "abs" => return num(0).map_or(Value::Null, |n| Value::Num(n.abs())),
            "min" | "max" => {
                let ns: Vec<f64> = (0..args.len()).filter_map(num).collect();
                let pick = if id == "min" {
                    ns.iter().copied().reduce(f64::min)
                } else {
                    ns.iter().copied().reduce(f64::max)
                };
                return pick.map_or(Value::Null, Value::Num);
            }
            "concat" => {
                let mut out = String::new();
                for a in &args {
                    out.push_str(&show(a));
                }
                return Value::Str(out);
            }
            _ => {}
        }
        match id {
            "file.hasTag" => {
                let has = |t: &str| {
                    let t = lower(t.trim_start_matches('#'));
                    row.tags.iter().any(|x| {
                        let x = lower(x);
                        x == t
                            || x.starts_with(&{
                                let mut s = t.clone();
                                s.push('/');
                                s
                            })
                    })
                };
                Value::Bool(args.iter().any(|a| has(&show(a))))
            }
            "file.inFolder" => {
                let f = arg_str(0);
                let f = f.trim_matches('/');
                Value::Bool(
                    row.path.starts_with(f) && row.path.as_bytes().get(f.len()) == Some(&b'/')
                        || f.is_empty(),
                )
            }
            "file.hasLink" => {
                let l = lower(arg_str(0).trim_end_matches(".md"));
                let l = String::from(file_name(&l));
                Value::Bool(row.links.iter().any(|(t, _)| file_name(t) == l))
            }
            "file.hasProperty" => Value::Bool(row.props.and_then(|p| p.get(&arg_str(0))).is_some()),
            _ => {
                // A method on a value: `status.contains("x")`.
                let Some((obj, method)) = id.rsplit_once('.') else {
                    return Value::Null;
                };
                let v = value(obj, row);
                let s = lower(&show(&v));
                let a = lower(&arg_str(0));
                match method {
                    "contains" => match &v {
                        Value::Arr(items) => {
                            Value::Bool(items.iter().any(|i| lower(&show(i)) == a))
                        }
                        _ => Value::Bool(s.contains(a.as_str())),
                    },
                    "containsAny" => Value::Bool(args.iter().any(|x| s.contains(&lower(&show(x))))),
                    "startsWith" => Value::Bool(s.starts_with(a.as_str())),
                    "endsWith" => Value::Bool(s.ends_with(a.as_str())),
                    "isEmpty" => Value::Bool(!truthy(&v)),
                    "lower" => Value::Str(s),
                    "upper" => Value::Str(show(&v).to_uppercase()),
                    "title" => Value::Str(
                        show(&v)
                            .split(' ')
                            .map(|w| {
                                let mut c = w.chars();
                                match c.next() {
                                    Some(f) => {
                                        let mut t: String = f.to_uppercase().collect();
                                        t.push_str(c.as_str());
                                        t
                                    }
                                    None => String::new(),
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    "trim" => Value::Str(String::from(show(&v).trim())),
                    "replace" => Value::Str(show(&v).replace(&arg_str(0), &arg_str(1))),
                    "format" => match as_date(&v) {
                        Some(d) => Value::Str(d.format(&arg_str(0))),
                        None => v.clone(),
                    },
                    "round" | "toFixed" => match &v {
                        Value::Num(n) => {
                            let places = num(0).unwrap_or(0.0).clamp(0.0, 10.0) as i32;
                            let f = 10f64.powi(places);
                            let r = (n * f + 0.5 * n.signum()).trunc() / f;
                            if method == "toFixed" {
                                Value::Str(fixed(r, places as usize))
                            } else {
                                Value::Num(r)
                            }
                        }
                        _ => Value::Null,
                    },
                    "relative" => match (as_date(&v), as_date(&Value::s(row.today))) {
                        (Some(d), Some(t)) => {
                            let n = d.day_number() - t.day_number();
                            Value::Str(match n {
                                0 => String::from("today"),
                                1 => String::from("tomorrow"),
                                -1 => String::from("yesterday"),
                                n if n > 0 => {
                                    let mut s = String::from("in ");
                                    crate::text::push_num(&mut s, n as usize);
                                    s.push_str(" days");
                                    s
                                }
                                n => {
                                    let mut s = String::new();
                                    crate::text::push_num(&mut s, (-n) as usize);
                                    s.push_str(" days ago");
                                    s
                                }
                            })
                        }
                        _ => Value::Null,
                    },
                    _ => Value::Null,
                }
            }
        }
    }
}

/// A number with `places` digits after the point.
fn fixed(n: f64, places: usize) -> String {
    let neg = n < 0.0;
    let f = 10f64.powi(places as i32);
    let scaled = (n.abs() * f + 0.5).trunc() as u64;
    let whole = scaled / (f as u64).max(1);
    let frac = scaled % (f as u64).max(1);
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    crate::text::push_num(&mut s, whole as usize);
    if places > 0 {
        s.push('.');
        let mut fs = String::new();
        crate::text::push_num(&mut fs, frac as usize);
        for _ in fs.len()..places {
            s.push('0');
        }
        s.push_str(&fs);
    }
    s
}

/// The rows of a view in groups, by its `groupBy` property: each group's
/// value as shown and its notes, in order. One unnamed group without it.
impl Base {
    pub fn groups(&self, v: &Vault, view: usize) -> Vec<(String, Vec<String>)> {
        let rows = self.rows(v, view);
        let Some((prop, desc)) = self.views.get(view).and_then(|w| w.group_by.clone()) else {
            return alloc::vec![(String::new(), rows)];
        };
        let mut groups: Vec<(Value, String, Vec<String>)> = Vec::new();
        let none: Vec<String> = Vec::new();
        let nolinks: Vec<(String, u32)> = Vec::new();
        for p in rows {
            let meta = v.meta.get(&p);
            let row = Row {
                path: &p,
                tags: meta.map_or(&none, |m| &m.tags),
                links: meta.map_or(&nolinks, |m| &m.links),
                props: meta.and_then(|m| m.props.as_ref()),
                formulas: &self.formulas,
                today: &self.today,
            };
            let key = value(&prop, &row);
            let label = show(&key);
            match groups.iter_mut().find(|g| g.1 == label) {
                Some(g) => g.2.push(p.clone()),
                None => groups.push((key, label, alloc::vec![p.clone()])),
            }
        }
        groups.sort_by(|a, b| {
            let o = compare(&a.0, &b.0);
            if desc {
                o.reverse()
            } else {
                o
            }
        });
        groups.into_iter().map(|(_, l, r)| (l, r)).collect()
    }
}

/// A new, empty base, as the app writes one.
pub fn new_base_text() -> &'static str {
    "views:\n  - type: table\n    name: Table\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::mem_vault;

    fn vault() -> Vault {
        let task = |st: &str, pr: u32, due: &str| {
            let mut s = String::from("---\nstatus: ");
            s.push_str(st);
            s.push_str("\npriority: ");
            crate::text::push_num(&mut s, pr as usize);
            s.push_str("\ndue: ");
            s.push_str(due);
            s.push_str("\ntags:\n  - task\n---\nFor [[Home]].\n");
            s
        };
        let t1 = task("done", 3, "2026-12-01");
        let t3 = task("open", 2, "2026-12-03");
        let t4 = task("open", 4, "2026-12-04");
        let t5 = task("open", 1, "2026-12-05");
        let mut v = mem_vault(
            "B",
            &[
                ("Home.md", "# Home"),
                ("Tasks/Task 1.md", &t1),
                ("Tasks/Task 3.md", &t3),
                ("Tasks/Task 4.md", &t4),
                ("Tasks/Task 5.md", &t5),
                ("Other/Note.md", "#task in the body"),
            ],
        );
        v.index_some(&mut || true);
        v
    }

    const BASE: &str = "filters:\n  and:\n    - file.hasTag(\"task\")\nproperties:\n  note.status:\n    displayName: Status\nviews:\n  - type: table\n    name: Open tasks\n    filters:\n      and:\n        - status != \"done\"\n    order:\n      - file.name\n      - status\n      - priority\n      - due\n    sort:\n      - property: priority\n        direction: DESC\n  - type: table\n    name: In Tasks\n    filters:\n      and:\n        - file.inFolder(\"Tasks\")\n        - 'priority >= 2 && !status.contains(\"don\")'\n    order:\n      - file.name\n";

    #[test]
    fn filters_sort_and_columns() {
        let v = vault();
        let b = Base::parse(BASE);
        assert_eq!(b.views.len(), 2);
        assert_eq!(b.column_name("status"), "Status");
        assert_eq!(b.column_name("file.name"), "name");
        assert_eq!(b.column_name("due"), "due");
        let rows = b.rows(&v, 0);
        assert_eq!(
            rows,
            [
                "Tasks/Task 4.md",
                "Tasks/Task 3.md",
                "Tasks/Task 5.md",
                "Other/Note.md"
            ]
            .map(String::from),
            "priority 4, 2, 1, and the note with none last"
        );
    }

    #[test]
    fn formulas_and_groups() {
        let v = vault();
        let mut b = Base::parse("formulas:\n  left: '(due - today())'\n  label: 'concat(file.name, \" (\", status, \")\")'\nviews:\n  - type: table\n    name: G\n    filters:\n      and:\n        - file.inFolder(\"Tasks\")\n    groupBy:\n      property: status\n      direction: ASC\n    order:\n      - file.name\n      - formula.left\n");
        b.today = String::from("2026-11-22");
        assert_eq!(b.formulas.len(), 2);
        let g = b.groups(&v, 0);
        assert_eq!(
            g.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
            ["done", "open"]
        );
        assert_eq!(g[1].1.len(), 3);
        let meta = v.meta.get("Tasks/Task 3.md").unwrap();
        let row = Row {
            path: "Tasks/Task 3.md",
            tags: &meta.tags,
            links: &meta.links,
            props: meta.props.as_ref(),
            formulas: &b.formulas,
            today: "2026-11-22",
        };
        assert_eq!(value("formula.left", &row), Value::s("11 days"));
        assert_eq!(value("formula.label", &row), Value::s("Task 3 (open)"));
    }

    #[test]
    fn expressions() {
        let v = vault();
        let b = Base::parse(BASE);
        assert_eq!(
            b.rows(&v, 1),
            ["Tasks/Task 3.md", "Tasks/Task 4.md"].map(String::from)
        );
        let meta = v.meta.get("Tasks/Task 1.md").unwrap();
        let row = Row {
            path: "Tasks/Task 1.md",
            tags: &meta.tags,
            links: &meta.links,
            props: meta.props.as_ref(),
            formulas: &[],
            today: "2026-11-22",
        };
        assert_eq!(eval("priority + 2 == 5", &row), Value::Bool(true));
        assert_eq!(eval("priority * 2 - 1", &row), Value::Num(5.0));
        assert_eq!(
            eval("if(status == \"done\", \"yes\", \"no\")", &row),
            Value::s("yes")
        );
        assert_eq!(eval("due - today()", &row), Value::s("9 days"));
        assert_eq!(
            eval("due.format(\"DD.MM.YYYY\")", &row),
            Value::s("01.12.2026")
        );
        assert_eq!(eval("due.month", &row), Value::Num(12.0));
        assert_eq!(eval("status.upper() + \"!\"", &row), Value::s("DONE!"));
        assert_eq!(eval("priority.toFixed(2)", &row), Value::s("3.00"));
        assert_eq!(eval("max(priority, 7)", &row), Value::Num(7.0));
        assert_eq!(eval("due.relative()", &row), Value::s("in 9 days"));
        assert_eq!(eval("file.hasLink(\"Home\")", &row), Value::Bool(true));
        assert_eq!(eval("file.name", &row), Value::s("Task 1"));
        assert_eq!(eval("due > \"2026-11-30\"", &row), Value::Bool(true));
        assert_eq!(eval("missing == null", &row), Value::Bool(true));
        assert_eq!(show(&value("file.tags", &row)), "task");
    }
}
