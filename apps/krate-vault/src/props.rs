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
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        let inner = t.get(1..t.len() - 1).unwrap_or("");
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    if t.len() >= 2 && t.starts_with('\'') && t.ends_with('\'') {
        return t.get(1..t.len() - 1).unwrap_or("").replace("''", "'");
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

/// A YAML scalar as a notes app writes it: plain when it reads back as
/// the same text, double-quoted otherwise.
pub fn yaml_scalar(s: &str) -> String {
    let risky = s.is_empty()
        || s.starts_with([
            ' ', '[', '{', '#', '&', '*', '!', '|', '>', '\'', '"', '%', '@', '`', '-', '?',
        ])
        || s.ends_with(' ')
        || s.contains(": ")
        || s.contains(" #")
        || matches!(s, "true" | "false" | "null" | "~" | "yes" | "no")
        || is_number(s);
    if !risky {
        return String::from(s);
    }
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

/// The YAML lines a property is written as.
pub fn render(key: &str, kind: Kind, value: &Value) -> String {
    let mut s = yaml_key(key);
    s.push(':');
    match value {
        Value::List(items) => {
            for it in items {
                s.push_str("\n  - ");
                let it = if kind == Kind::Tags {
                    it.trim_start_matches('#')
                } else {
                    it.as_str()
                };
                s.push_str(&yaml_scalar(it));
            }
        }
        Value::Bool(b) => s.push_str(if *b { " true" } else { " false" }),
        Value::Number(n) => {
            s.push(' ');
            s.push_str(n);
        }
        Value::Date(d) => {
            s.push(' ');
            s.push_str(d);
        }
        Value::Text(t) => {
            s.push(' ');
            s.push_str(&yaml_scalar(t));
        }
        Value::Empty => {}
    }
    s
}

fn yaml_key(k: &str) -> String {
    if k.is_empty() || k.contains(':') || k.starts_with([' ', '-', '#', '"', '\'']) {
        yaml_scalar(k)
    } else {
        String::from(k)
    }
}

/// A value after its property's type is changed in the type menu.
pub fn convert(v: &Value, to: Kind) -> Value {
    let text = match v {
        Value::List(items) => items.join(", "),
        other => other.shown_raw(),
    };
    match to {
        Kind::List | Kind::Tags | Kind::Aliases => match v {
            Value::List(items) => Value::List(items.clone()),
            Value::Empty => Value::List(Vec::new()),
            _ if text.is_empty() => Value::List(Vec::new()),
            _ => Value::List(alloc::vec![text]),
        },
        Kind::Number => {
            if is_number(text.trim()) {
                Value::Number(String::from(text.trim()))
            } else {
                Value::Empty
            }
        }
        Kind::Checkbox => Value::Bool(matches!(text.trim(), "true" | "yes" | "1")),
        Kind::Date => {
            let d = text.get(..10).unwrap_or("");
            if is_date(d) {
                Value::Date(String::from(d))
            } else {
                Value::Empty
            }
        }
        Kind::DateTime => {
            if is_datetime(&text) {
                Value::Date(text)
            } else if is_date(&text) {
                let mut t = text;
                t.push_str("T00:00");
                Value::Date(t)
            } else {
                Value::Empty
            }
        }
        Kind::Text => {
            if text.is_empty() {
                Value::Empty
            } else {
                Value::Text(text)
            }
        }
    }
}

/// What typing `text` into a value field means for a property of `kind`.
pub fn from_input(kind: Kind, text: &str) -> Value {
    let t = text.trim();
    if t.is_empty() {
        return Value::Empty;
    }
    match kind {
        Kind::Number if is_number(t) => Value::Number(String::from(t)),
        Kind::Date | Kind::DateTime if is_date(t.get(..10).unwrap_or("")) => {
            Value::Date(String::from(t))
        }
        Kind::Checkbox => Value::Bool(t == "true"),
        _ => Value::Text(String::from(t)),
    }
}

impl Value {
    /// The value as written (dates as YYYY-MM-DD), for editing.
    pub fn shown_raw(&self) -> String {
        match self {
            Value::Date(d) => d.clone(),
            other => other.shown(),
        }
    }
}

/// The name `.obsidian/types.json` gives a kind.
pub fn type_name(k: Kind) -> &'static str {
    match k {
        Kind::Text => "text",
        Kind::List => "multitext",
        Kind::Number => "number",
        Kind::Checkbox => "checkbox",
        Kind::Date => "date",
        Kind::DateTime => "datetime",
        Kind::Tags => "tags",
        Kind::Aliases => "aliases",
    }
}

/// The kind a `.obsidian/types.json` name stands for.
pub fn kind_named(name: &str) -> Option<Kind> {
    Some(match name {
        "text" => Kind::Text,
        "multitext" => Kind::List,
        "number" => Kind::Number,
        "checkbox" => Kind::Checkbox,
        "date" => Kind::Date,
        "datetime" => Kind::DateTime,
        "tags" => Kind::Tags,
        "aliases" => Kind::Aliases,
        _ => return None,
    })
}

/// The types a vault keeps for its property names, read from the
/// `types` object of `.obsidian/types.json`.
pub fn types_from(json: &crate::json::Value) -> Vec<(String, Kind)> {
    match json.get("types") {
        Some(crate::json::Value::Obj(kv)) => kv
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), kind_named(v.str()?)?)))
            .collect(),
        _ => Vec::new(),
    }
}

/// Record the type a person picked for a property name in the
/// `.obsidian/types.json` data, as Obsidian writes it: the three built-in
/// names first when the file is new, then each name in the order it was
/// first typed, an existing name (in any case) changed where it is.
pub fn remember_type(json: &mut crate::json::Value, key: &str, kind: Kind) {
    use crate::json::Value as J;
    if !matches!(json.get("types"), Some(J::Obj(_))) {
        let mut t = J::obj();
        t.set("aliases", J::s("aliases"));
        t.set("cssclasses", J::s("multitext"));
        t.set("tags", J::s("tags"));
        json.set("types", t);
    }
    if let Some(J::Obj(kv)) = json.get_mut("types") {
        let lower = crate::text::lower(key);
        match kv.iter_mut().find(|(k, _)| crate::text::lower(k) == lower) {
            Some((_, v)) => *v = J::s(type_name(kind)),
            None => kv.push((String::from(key), J::s(type_name(kind)))),
        }
    }
}

/// Give each property the type its name has in the vault, as Obsidian
/// does: the kind the vault keeps wins over the one the value suggests,
/// unless the value cannot be shown as that kind.
pub fn apply_types(props: &mut [Prop], types: &[(String, Kind)]) {
    if types.is_empty() {
        return;
    }
    for p in props.iter_mut() {
        let lower = crate::text::lower(&p.key);
        let Some((_, kind)) = types.iter().find(|(k, _)| crate::text::lower(k) == lower) else {
            continue;
        };
        if *kind == p.kind {
            continue;
        }
        let v = convert(&p.value, *kind);
        if v != Value::Empty || p.value == Value::Empty {
            p.kind = *kind;
            p.value = v;
        }
    }
}

/// The day of the week of a date, 0 Monday to 6 Sunday.
pub fn weekday(y: i32, m: u32, d: u32) -> u32 {
    // Days from the civil calendar (Howard Hinnant's), 1970-01-01 a Thursday.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = i64::from(era) * 146_097 + doe - 719_468;
    (((days % 7) + 7 + 3) % 7) as u32
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 {
                29
            } else {
                28
            }
        }
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
    fn values_write_back_as_yaml() {
        assert_eq!(
            render(
                "title",
                Kind::Text,
                &Value::Text(String::from("Render test"))
            ),
            "title: Render test"
        );
        assert_eq!(
            render("t", Kind::Text, &Value::Text(String::from("a: b"))),
            "t: \"a: b\""
        );
        assert_eq!(
            render("t", Kind::Text, &Value::Text(String::from("42"))),
            "t: \"42\""
        );
        assert_eq!(
            render(
                "tags",
                Kind::Tags,
                &Value::List(alloc::vec![String::from("#a"), String::from("b c")])
            ),
            "tags:\n  - a\n  - b c"
        );
        assert_eq!(
            render("n", Kind::Number, &Value::Number(String::from("4"))),
            "n: 4"
        );
        assert_eq!(render("e", Kind::Text, &Value::Empty), "e:");
        // Written and read again, a value is the same.
        let lines = ["q: \"He said \\\"hi\\\"\""];
        assert_eq!(parse(&lines, 1)[0].value.shown(), "He said \"hi\"");
        let back = render(
            "q",
            Kind::Text,
            &Value::Text(String::from("He said \"hi\"")),
        );
        assert_eq!(
            parse(&[back.as_str()], 1)[0].value.shown(),
            "He said \"hi\""
        );
        assert_eq!(
            convert(&Value::Text(String::from("x")), Kind::List),
            Value::List(alloc::vec![String::from("x")])
        );
        assert_eq!(
            convert(&Value::Text(String::from("2026-11-22")), Kind::Date),
            Value::Date(String::from("2026-11-22"))
        );
        assert_eq!(
            convert(&Value::Text(String::from("abc")), Kind::Number),
            Value::Empty
        );
        assert_eq!(
            from_input(Kind::Number, " 7 "),
            Value::Number(String::from("7"))
        );
        assert_eq!(weekday(2026, 11, 22), 6, "a Sunday");
        assert_eq!(weekday(2024, 2, 29), 3, "a Thursday");
        assert_eq!(days_in_month(2024, 2), 29);
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

    #[test]
    fn picked_types_are_kept_as_obsidian_writes_them() {
        // What Obsidian 1.15.1 wrote after status -> Number, due -> Date &
        // time, then MyKey -> Checkbox and status -> Text
        // (evidence: setType through its DevTools, read back from disk).
        let mut j = crate::json::Value::obj();
        remember_type(&mut j, "status", Kind::Number);
        remember_type(&mut j, "due", Kind::DateTime);
        assert_eq!(
            crate::json::pretty(&j, "  "),
            "{\n  \"types\": {\n    \"aliases\": \"aliases\",\n    \"cssclasses\": \"multitext\",\n    \"tags\": \"tags\",\n    \"status\": \"number\",\n    \"due\": \"datetime\"\n  }\n}"
        );
        remember_type(&mut j, "MyKey", Kind::Checkbox);
        remember_type(&mut j, "Status", Kind::Text);
        let t = types_from(&j);
        let names: Vec<&str> = t.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            names,
            ["aliases", "cssclasses", "tags", "status", "due", "MyKey"]
        );
        assert_eq!(t[3].1, Kind::Text);
        // Every note's property of that name takes the kept type.
        let mut ps = parse(
            &["status: 3", "due: 2026-11-22", "mykey: yes", "other: 4"],
            1,
        );
        apply_types(&mut ps, &t);
        assert_eq!(ps[0].kind, Kind::Text);
        assert_eq!(ps[0].value, Value::Text(String::from("3")));
        assert_eq!(ps[1].kind, Kind::DateTime);
        assert_eq!(ps[2].kind, Kind::Checkbox);
        assert_eq!(ps[2].value, Value::Bool(true));
        assert_eq!(ps[3].kind, Kind::Number);
        // A value that cannot be that kind keeps the kind it reads as.
        let mut ps = parse(&["due: soon"], 1);
        apply_types(&mut ps, &t);
        assert_eq!(ps[0].kind, Kind::Text);
    }
}
