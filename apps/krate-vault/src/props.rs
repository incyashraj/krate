//! A note's properties: the YAML front matter between the `---` lines at
//! its top, read into typed values the way the Properties block shows them,
//! and the edits the block makes written back as YAML.
//!
//! The YAML a notes app writes is a small, flat subset -- `key: value`,
//! lists as `- item` lines or `[a, b]`, quoted strings -- and that is what
//! is read here. A line this cannot read is kept as text, never dropped.

use alloc::string::String;
use alloc::vec::Vec;

/// What kind of value a property holds; it picks the icon and the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    List,
    Number,
    Checkbox,
    Date,
    DateTime,
    Tags,
    Aliases,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Text(String),
    List(Vec<String>),
    Number(String),
    Bool(bool),
    Date(String),
    Empty,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    pub key: String,
    pub kind: Kind,
    pub value: Value,
    /// The note line the key is on (line 0 is the opening `---`).
    pub line: usize,
    /// The last line its value takes (a block list runs past the key).
    pub last: usize,
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
    {
        return String::from(t.get(1..t.len() - 1).unwrap_or(""));
    }
    String::from(t)
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

fn is_datetime(s: &str) -> bool {
    s.len() >= 16
        && is_date(s.get(..10).unwrap_or(""))
        && matches!(s.as_bytes().get(10), Some(b'T' | b' '))
}

fn is_number(s: &str) -> bool {
    let t = s.strip_prefix('-').unwrap_or(s);
    !t.is_empty()
        && t.bytes().filter(|b| *b == b'.').count() <= 1
        && t.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && !t.starts_with('.')
}

fn inline_list(s: &str) -> Option<Vec<String>> {
    let inner = s.trim().strip_prefix('[')?.strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }
    Some(inner.split(',').map(unquote).collect())
}

/// Read the front matter from its lines (the lines between the `---`s;
/// `first` is the note line of the first of them).
pub fn parse(lines: &[&str], first: usize) -> Vec<Prop> {
    let mut out: Vec<Prop> = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let l = lines.get(i).copied().unwrap_or("");
        if l.trim().is_empty() || l.starts_with('#') || l.starts_with(' ') || l.starts_with('-') {
            i += 1;
            continue;
        }
        let Some((key, rest)) = l.split_once(':') else {
            i += 1;
            continue;
        };
        let key = unquote(key);
        let rest = rest.trim();
        let mut last = i;
        let value = if rest.is_empty() {
            // A block list follows, or nothing.
            let mut items = Vec::new();
            let mut j = i + 1;
            while let Some(next) = lines.get(j) {
                let t = next.trim_start();
                if let Some(item) = t
                    .strip_prefix("- ")
                    .or(if t == "-" { Some("") } else { None })
                {
                    items.push(unquote(item));
                    last = j;
                    j += 1;
                } else {
                    break;
                }
            }
            if items.is_empty() {
                Value::Empty
            } else {
                Value::List(items)
            }
        } else if let Some(items) = inline_list(rest) {
            Value::List(items)
        } else {
            let v = unquote(rest);
            if rest == "true" || rest == "false" {
                Value::Bool(rest == "true")
            } else if is_number(rest) {
                Value::Number(v)
            } else if is_date(&v) || is_datetime(&v) {
                Value::Date(v)
            } else {
                Value::Text(v)
            }
        };
        let lower = crate::text::lower(&key);
        let kind = match (lower.as_str(), &value) {
            ("tags" | "tag", _) => Kind::Tags,
            ("aliases" | "alias", _) => Kind::Aliases,
            ("cssclasses", _) => Kind::List,
            (_, Value::List(_)) => Kind::List,
            (_, Value::Number(_)) => Kind::Number,
            (_, Value::Bool(_)) => Kind::Checkbox,
            (_, Value::Date(d)) if d.len() > 10 => Kind::DateTime,
            (_, Value::Date(_)) => Kind::Date,
            _ => Kind::Text,
        };
        // Tags and aliases written on one line are still lists.
        let value = match (kind, value) {
            (Kind::Tags | Kind::Aliases, Value::Text(t)) => Value::List(
                t.split([',', ' '])
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect(),
            ),
            (_, v) => v,
        };
        out.push(Prop {
            key,
            kind,
            value,
            line: first + i,
            last: first + last,
        });
        i = last + 1;
    }
    out
}

impl Value {
    /// The value as the block shows it (lists are shown item by item).
    pub fn shown(&self) -> String {
        match self {
            Value::Text(s) | Value::Number(s) => s.clone(),
            Value::Date(d) => format_date(d),
            Value::Bool(b) => String::from(if *b { "true" } else { "false" }),
            Value::List(v) => v.join(", "),
            Value::Empty => String::new(),
        }
    }
}

/// `2026-11-22` as the Properties block shows a date: `22/11/2026` (the
/// day-first order this machine's locale gives Obsidian's date input).
pub fn format_date(d: &str) -> String {
    if !is_date(d.get(..10).unwrap_or("")) {
        return String::from(d);
    }
    let mut s = String::new();
    s.push_str(d.get(8..10).unwrap_or(""));
    s.push('/');
    s.push_str(d.get(5..7).unwrap_or(""));
    s.push('/');
    s.push_str(d.get(..4).unwrap_or(""));
    if let Some(time) = d.get(11..16) {
        s.push_str(", ");
        s.push_str(time);
    }
    s
}

/// The YAML line a checkbox property becomes when it is ticked or unticked.
pub fn toggled_line(line: &str) -> Option<String> {
    let (key, rest) = line.split_once(':')?;
    let v = rest.trim();
    let flipped = match v {
        "true" => "false",
        "false" => "true",
        _ => return None,
    };
    let mut s = String::from(key);
    s.push_str(": ");
    s.push_str(flipped);
    Some(s)
}

/// A name for a new property that is not taken yet.
pub fn new_key(props: &[Prop]) -> String {
    let mut n = 0usize;
    loop {
        let mut k = String::from("property");
        if n > 0 {
            k.push(' ');
            crate::text::push_num(&mut k, n);
        }
        if !props.iter().any(|p| p.key == k) {
            return k;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FM: [&str; 13] = [
        "title: Render test",
        "tags:",
        "  - render",
        "  - test",
        "aliases:",
        "  - Render page",
        "status: \"draft\"",
        "rating: 4",
        "published: false",
        "created: 2026-11-22",
        "when: 2026-11-22T09:30",
        "cssclasses: [wide, dark]",
        "empty:",
    ];

    #[test]
    fn properties_are_typed_like_the_block_shows_them() {
        let p = parse(&FM, 1);
        let kinds: Vec<(&str, Kind)> = p.iter().map(|x| (x.key.as_str(), x.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("title", Kind::Text),
                ("tags", Kind::Tags),
                ("aliases", Kind::Aliases),
                ("status", Kind::Text),
                ("rating", Kind::Number),
                ("published", Kind::Checkbox),
                ("created", Kind::Date),
                ("when", Kind::DateTime),
                ("cssclasses", Kind::List),
                ("empty", Kind::Text),
            ]
        );
        assert_eq!(
            p[1].value,
            Value::List(alloc::vec![String::from("render"), String::from("test")])
        );
        assert_eq!((p[1].line, p[1].last), (2, 4));
        assert_eq!(p[3].value.shown(), "draft");
        assert_eq!(p[5].value, Value::Bool(false));
        assert_eq!(p[6].value.shown(), "22/11/2026");
        assert_eq!(p[7].value.shown(), "22/11/2026, 09:30");
        assert_eq!(p[8].value.shown(), "wide, dark");
        let one_line = parse(&["tags: a, b c"], 1);
        assert_eq!(one_line[0].value.shown(), "a, b, c");
    }

    #[test]
    fn checkboxes_toggle_and_new_keys_are_free() {
        assert_eq!(
            toggled_line("published: false").as_deref(),
            Some("published: true")
        );
        assert_eq!(
            toggled_line("published:true").as_deref(),
            Some("published: false")
        );
        assert_eq!(toggled_line("title: x"), None);
        let p = parse(&["property: 1", "property 1: 2"], 1);
        assert_eq!(new_key(&p), "property 2");
        assert!(is_number("-3.5") && !is_number("1.2.3") && !is_number("v1"));
    }
}
