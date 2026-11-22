//! The part of YAML notes and bases are written in: note properties (the
//! `---` block at the top of a note) and `.base` files. Block maps and
//! lists by indentation, flow `[a, b]` and `{a: b}`, quoted and plain
//! scalars, `|` and `>` blocks, comments. Read into [`json::Value`] so the
//! rest of the app has one shape of data.

use crate::json::Value;
use alloc::string::String;
use alloc::vec::Vec;

struct Line<'a> {
    indent: usize,
    text: &'a str,
}

/// Parse a YAML document. Anything it cannot read becomes a string, never
/// an error: a note's properties should still show what is there.
pub fn parse(text: &str) -> Value {
    let lines: Vec<Line> = text
        .split('\n')
        .map(|l| l.trim_end_matches('\r'))
        .filter_map(|l| {
            let body = l.trim_start_matches(' ');
            let t = body.trim_end();
            if t.is_empty() || t.starts_with('#') || t == "---" || t == "..." {
                return None;
            }
            Some(Line {
                indent: l.len() - body.len(),
                text: t,
            })
        })
        .collect();
    let raw: Vec<&str> = text.split('\n').collect();
    let mut i = 0;
    block(&lines, &mut i, 0, &raw).unwrap_or(Value::Null)
}

/// Where a `key:` ends in a line (the colon followed by a space or the end).
fn key_split(t: &str) -> Option<(&str, &str)> {
    let b = t.as_bytes();
    let mut q: Option<u8> = None;
    for (i, &c) in b.iter().enumerate() {
        match (q, c) {
            (None, b'"' | b'\'') if i == 0 => q = Some(c),
            (Some(x), c) if c == x => q = None,
            (None, b':') if b.get(i + 1).is_none_or(|n| *n == b' ') => {
                let k = t[..i].trim();
                let k = k
                    .strip_prefix('"')
                    .and_then(|k| k.strip_suffix('"'))
                    .or_else(|| k.strip_prefix('\'').and_then(|k| k.strip_suffix('\'')))
                    .unwrap_or(k);
                return Some((k, t[i + 1..].trim()));
            }
            (None, b'{' | b'[') if i == 0 => return None,
            _ => {}
        }
    }
    None
}

fn block(lines: &[Line], i: &mut usize, min: usize, raw: &[&str]) -> Option<Value> {
    let first = lines.get(*i)?;
    if first.indent < min {
        return None;
    }
    let ind = first.indent;
    if first.text == "-" || first.text.starts_with("- ") {
        let mut items = Vec::new();
        while let Some(l) = lines.get(*i) {
            if l.indent != ind || !(l.text == "-" || l.text.starts_with("- ")) {
                break;
            }
            let rest = l.text[1..].trim_start();
            *i += 1;
            if rest.is_empty() {
                items.push(block(lines, i, ind + 1, raw).unwrap_or(Value::Null));
            } else if key_split(rest).is_some() && !rest.starts_with(['"', '\'', '[', '{']) {
                // `- key: value` starts a map whose keys line up after "- ".
                let inner = ind + (l.text.len() - rest.len());
                let mut m = Value::obj();
                map_entry(rest, lines, i, inner, raw, &mut m);
                while let Some(n) = lines.get(*i) {
                    if n.indent != inner {
                        break;
                    }
                    let Some(_) = key_split(n.text) else { break };
                    let t = n.text;
                    *i += 1;
                    map_entry(t, lines, i, inner, raw, &mut m);
                }
                items.push(m);
            } else {
                items.push(scalar(rest));
            }
        }
        return Some(Value::Arr(items));
    }
    if key_split(first.text).is_some() {
        let mut m = Value::obj();
        while let Some(l) = lines.get(*i) {
            if l.indent != ind {
                break;
            }
            if key_split(l.text).is_none() {
                break;
            }
            let t = l.text;
            *i += 1;
            map_entry(t, lines, i, ind, raw, &mut m);
        }
        return Some(m);
    }
    *i += 1;
    Some(scalar(first.text))
}

fn map_entry(t: &str, lines: &[Line], i: &mut usize, ind: usize, raw: &[&str], m: &mut Value) {
    let Some((k, v)) = key_split(t) else {
        return;
    };
    let key = String::from(k);
    let val = if v.is_empty() {
        // A nested block, or a list at the same indent (`tags:\n- a`).
        match lines.get(*i) {
            Some(n) if n.indent > ind => block(lines, i, ind + 1, raw).unwrap_or(Value::Null),
            Some(n) if n.indent == ind && (n.text == "-" || n.text.starts_with("- ")) => {
                block(lines, i, ind, raw).unwrap_or(Value::Null)
            }
            _ => Value::Null,
        }
    } else if v == "|" || v == ">" || v.starts_with("|-") || v.starts_with(">-") {
        let fold = v.starts_with('>');
        let mut parts: Vec<&str> = Vec::new();
        while let Some(n) = lines.get(*i) {
            if n.indent <= ind {
                break;
            }
            parts.push(n.text);
            *i += 1;
        }
        let mut s = parts.join(if fold { " " } else { "\n" });
        if !v.ends_with('-') {
            s.push('\n');
        }
        Value::Str(s)
    } else {
        scalar(v)
    };
    m.set(&key, val);
}

fn strip_comment(s: &str) -> &str {
    match s.find(" #") {
        Some(i) => s[..i].trim_end(),
        None => s,
    }
}

/// One value written on a line.
pub fn scalar(s: &str) -> Value {
    let s = s.trim();
    if let Some(inner) = s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        if !inner.contains('"') || inner.contains("\\\"") {
            return Value::Str(inner.replace("\\\"", "\"").replace("\\n", "\n"));
        }
    }
    if let Some(inner) = s.strip_prefix('\'').and_then(|r| r.strip_suffix('\'')) {
        return Value::Str(inner.replace("''", "'"));
    }
    if s.starts_with('[') && s.ends_with(']') {
        return Value::Arr(
            split_flow(&s[1..s.len() - 1])
                .into_iter()
                .filter(|p| !p.trim().is_empty())
                .map(scalar)
                .collect(),
        );
    }
    if s.starts_with('{') && s.ends_with('}') {
        let mut m = Value::obj();
        for part in split_flow(&s[1..s.len() - 1]) {
            if let Some((k, v)) = part.split_once(':') {
                m.set(k.trim().trim_matches('"'), scalar(v));
            }
        }
        return m;
    }
    let s = strip_comment(s);
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        _ => {}
    }
    let numeric = s
        .bytes()
        .all(|c| c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.' | b'e' | b'E'))
        && s.bytes().any(|c| c.is_ascii_digit())
        && !s[1..].contains('-');
    if numeric {
        if let Ok(n) = s.parse::<f64>() {
            return Value::Num(n);
        }
    }
    Value::Str(String::from(s))
}

/// Split `a, [b, c], "d, e"` at the top-level commas.
fn split_flow(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut q: Option<char> = None;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match (q, c) {
            (Some(x), c) if c == x => q = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => q = Some(c),
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => depth -= 1,
            (None, ',') if depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// A note's properties: the `---` block at its very top, and the line the
/// note's body starts on. `None` when the note has none.
pub fn front_matter(text: &str) -> Option<(Value, usize)> {
    let mut lines = text.split('\n');
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let mut body = String::new();
    for (n, l) in lines.enumerate() {
        let t = l.trim_end();
        if t == "---" || t == "..." {
            let v = parse(&body);
            let v = if matches!(v, Value::Obj(_)) {
                v
            } else {
                Value::obj()
            };
            return Some((v, n + 2));
        }
        body.push_str(l);
        body.push('\n');
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::compact;

    #[test]
    fn properties_read_like_the_notes_app_reads_them() {
        let t = "---\ntitle: \"A: note\"\nstatus: done\npriority: 3\ndue: 2026-12-01\ntags:\n  - task\n  - work/deep\naliases: [one, \"two, three\"]\ndone: true\nempty:\n---\n# Body";
        let (v, body) = front_matter(t).unwrap();
        assert_eq!(body, 12);
        assert_eq!(
            compact(&v),
            "{\"title\":\"A: note\",\"status\":\"done\",\"priority\":3,\"due\":\"2026-12-01\",\"tags\":[\"task\",\"work/deep\"],\"aliases\":[\"one\",\"two, three\"],\"done\":true,\"empty\":null}"
        );
        assert!(front_matter("# No properties").is_none());
        // A list at the same indent as its key.
        let (v, _) = front_matter("---\ntags:\n- a\n- b\n---\n").unwrap();
        assert_eq!(compact(&v), "{\"tags\":[\"a\",\"b\"]}");
    }

    #[test]
    fn a_base_file_reads() {
        let t = "filters:\n  and:\n    - file.hasTag(\"task\")\n    - 'status != \"done\"'\nproperties:\n  note.status:\n    displayName: Status\nviews:\n  - type: table\n    name: Open tasks\n    order:\n      - file.name\n      - status\n    sort:\n      - property: priority\n        direction: DESC\n  - type: cards\n    name: All\n";
        let v = parse(t);
        assert_eq!(
            compact(v.get("filters").unwrap()),
            "{\"and\":[\"file.hasTag(\\\"task\\\")\",\"status != \\\"done\\\"\"]}"
        );
        let views = v.get("views").unwrap().arr();
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].get_str("name"), Some("Open tasks"));
        assert_eq!(
            compact(views[0].get("sort").unwrap()),
            "[{\"property\":\"priority\",\"direction\":\"DESC\"}]"
        );
        assert_eq!(views[1].get_str("type"), Some("cards"));
        assert_eq!(
            v.get("properties")
                .unwrap()
                .get("note.status")
                .unwrap()
                .get_str("displayName"),
            Some("Status")
        );
    }
}
