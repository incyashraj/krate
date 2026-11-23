//! Ports preprocess.ts: what Mermaid does to a diagram's text before a
//! parser reads it -- line breaks, HTML attribute quotes, front matter,
//! `%%{init}%%` directives, comment lines -- and `encodeEntities`, which
//! hides `#name;` codes behind placeholders.
//!
//! The `_with` forms record each edit (`steps`), which maps an error's
//! position in the parsed text back to the source.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::json::{self, Value};
use crate::pele::errors::{Edit, PeleError, PeleErrorCode};
use crate::pele::js::{self, is_space};
use crate::pele::text::entities::{CLOSE, OPEN};
use crate::pele::util::yaml::parse_yaml;

/// Ports `type Config`: a JSON object.
pub type Config = Value;

/// Ports `interface Preprocessed`.
#[derive(Debug, Clone, PartialEq)]
pub struct Preprocessed {
    pub text: String,
    pub title: Option<String>,
    pub config: Config,
    pub with_comments: String,
    pub line_offset: usize,
}

fn is_line_end(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// The matches of RE_COMMENT = /^\s*%%(?!{)[^\n]+\n?/gm, as byte ranges.
fn comment_matches(text: &str) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    let mut p = 0;
    let mut line_start = true;
    while p < text.len() {
        if line_start {
            let q = skip_ws(text, p);
            if text[q..].starts_with("%%") && !text[q + 2..].starts_with('{') {
                let body = q + 2;
                let nl = text[body..]
                    .find('\n')
                    .map(|k| body + k)
                    .unwrap_or(text.len());
                if nl > body {
                    let end = if nl < text.len() { nl + 1 } else { nl };
                    found.push((p, end));
                    p = end;
                    line_start = true;
                    continue;
                }
            }
        }
        let c = text[p..].chars().next().unwrap_or('\n');
        line_start = is_line_end(c);
        p += c.len_utf8();
    }
    found
}

/// Ports `removeAll`: the text without the matches, each recorded as an edit.
fn remove_all(text: &str, matches: &[(usize, usize)], steps: Steps) -> String {
    if let Some(st) = steps {
        st.push(
            matches
                .iter()
                .map(|&(a, b)| js::edit16(text, a, b, 0))
                .collect(),
        );
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for &(a, b) in matches {
        out.push_str(&text[last..a]);
        last = b;
    }
    out.push_str(&text[last..]);
    out
}

/// Where preprocessing edits are recorded, to map an error's position in
/// the parsed text back to the source (`steps`).
pub type Steps<'a> = Option<&'a mut Vec<Vec<Edit>>>;

/// Ports `cleanupComments`: drops `%%` comment lines and leading whitespace.
pub fn cleanup_comments(text: &str) -> String {
    cleanup_comments_with(text, None)
}

/// `cleanupComments(text, steps)`.
pub fn cleanup_comments_with(text: &str, mut steps: Steps) -> String {
    let mut text = text.to_string();
    if text.contains("%%") {
        let m = comment_matches(&text);
        text = remove_all(&text, &m, steps.as_deref_mut());
    }
    let trimmed = js::trim_start(&text);
    if let Some(st) = steps {
        st.push(alloc::vec![(0, js::len16(&text) - js::len16(trimmed), 0)]);
    }
    trimmed.to_string()
}

fn skip_ws(s: &str, mut p: usize) -> usize {
    while let Some(c) = s[p..].chars().next() {
        if !is_space(c) {
            break;
        }
        p += c.len_utf8();
    }
    p
}

/// Ports `normalizeLineBreaks`.
pub fn normalize_line_breaks(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_string()
    }
}

fn is_line_break(c: Option<u8>) -> bool {
    matches!(c, Some(b'\n') | Some(b'\r'))
}

/// `isSpace(text[i])` for a byte position.
fn space_at(text: &str, i: usize) -> bool {
    text.get(i..)
        .and_then(|s| s.chars().next())
        .is_some_and(is_space)
}

struct FrontMatter {
    indent: String,
    body: String,
    length: usize,
}

/// Ports `matchFrontMatter`: a `---` fenced YAML block at the start of the
/// text, found the way Mermaid's front matter regex finds it.
fn match_front_matter(text: &str) -> Option<FrontMatter> {
    let b = text.as_bytes();
    let mut cursor = 0;
    while cursor < text.len() && space_at(text, cursor) && !is_line_break(b.get(cursor).copied()) {
        cursor += text[cursor..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
    }
    let indent = text[..cursor].to_string();
    if !text[cursor..].starts_with("---") {
        return None;
    }
    let mut opening_ends: Vec<usize> = Vec::new();
    let mut i = cursor + 3;
    while i < text.len() && space_at(text, i) {
        if is_line_break(b.get(i).copied()) {
            opening_ends.push(i);
        }
        i += text[i..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    if opening_ends.is_empty() {
        return None;
    }
    let fence = format!("{}---", indent);
    let closing_end = |i: usize| -> Option<usize> {
        if !is_line_break(b.get(i).copied()) || !text[i + 1..].starts_with(fence.as_str()) {
            return None;
        }
        let mut last = None;
        let mut j = i + 1 + fence.len();
        while j < text.len() && space_at(text, j) {
            if is_line_break(b.get(j).copied()) {
                last = Some(j);
            }
            j += text[j..].chars().next().map(char::len_utf8).unwrap_or(1);
        }
        last
    };
    let needle = format!("\n{}", fence);
    let mut last_fence = None;
    let mut search_end = text.len();
    while let Some(i) = text[..search_end.min(text.len())].rfind(needle.as_str()) {
        if i <= opening_ends[0] {
            break;
        }
        if closing_end(i).is_some() {
            last_fence = Some(i);
            break;
        }
        // lastIndexOf(needle, i - 1): a match starting at or before i - 1.
        search_end = i - 1 + needle.len();
        if i == 0 {
            break;
        }
    }
    let last_fence = last_fence?;
    let mut opening_end = opening_ends[0];
    for &candidate in &opening_ends {
        if candidate < last_fence {
            opening_end = candidate;
        }
    }
    let body_start = opening_end + 1;
    let first_at = if body_start > 0 { body_start - 1 } else { 0 };
    let mut from = first_at;
    while let Some(k) = text[from..].find(needle.as_str()) {
        let i = from + k;
        if i > last_fence {
            break;
        }
        if i >= body_start {
            if let Some(end) = closing_end(i) {
                return Some(FrontMatter {
                    indent,
                    body: text[body_start..i].to_string(),
                    length: end + 1,
                });
            }
        }
        from = i + 1;
    }
    None
}

/// Ports `isRecord`.
pub fn is_record(value: &Value) -> bool {
    matches!(value, Value::Obj(_))
}

/// Ports `merge`.
pub fn merge(target: &mut Value, source: &Value) {
    let Value::Obj(src) = source else { return };
    for (key, value) in src {
        if matches!(key.as_str(), "__proto__" | "constructor" | "prototype") {
            continue;
        }
        let current_is_record = target.get(key).is_some_and(is_record);
        if is_record(value) && current_is_record {
            if let Some(current) = target.get_mut(key) {
                merge(current, value);
            }
        } else if is_record(value) {
            let mut fresh = Value::obj();
            merge(&mut fresh, value);
            target.set(key, fresh);
        } else {
            target.set(key, value.clone());
        }
    }
}

/// A match of RE_DIRECTIVE at `i`:
/// /%{2}{\s*(?:(\w+)\s*:|(\w+))\s*(?:(\w+)|((?:(?!}%{2}).|\r?\n)*))?\s*(?:}%{2})?/gi.
/// Returns (end, type, group 3, group 4).
fn directive_at(s: &str, i: usize) -> Option<(usize, String, Option<String>, Option<String>)> {
    if !s[i..].starts_with("%%{") {
        return None;
    }
    let b = s.as_bytes();
    let word = |p: usize| {
        let mut e = p;
        while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
            e += 1;
        }
        e
    };
    let p = skip_ws(s, i + 3);
    let we = word(p);
    if we == p {
        return None;
    }
    let kind = s[p..we].to_string();
    let after_word = skip_ws(s, we);
    let mut q = if b.get(after_word) == Some(&b':') {
        after_word + 1
    } else {
        we
    };
    q = skip_ws(s, q);
    let mut g3 = None;
    let mut g4 = None;
    let w3 = word(q);
    if w3 > q {
        g3 = Some(s[q..w3].to_string());
        q = w3;
    } else {
        let start = q;
        let mut k = q;
        loop {
            if s[k..].starts_with("}%%") || k >= s.len() {
                break;
            }
            if s[k..].starts_with("\r\n") {
                k += 2;
                continue;
            }
            let c = s[k..].chars().next().unwrap_or('\n');
            if c == '\n' {
                k += 1;
                continue;
            }
            if is_line_end(c) {
                break;
            }
            k += c.len_utf8();
        }
        g4 = Some(s[start..k].to_string());
        q = k;
    }
    q = skip_ws(s, q);
    if s[q..].starts_with("}%%") {
        q += 3;
    }
    Some((q, kind, g3, g4))
}

/// Ports `directives`.
fn directives(text: &str, config: &mut Config, steps: Steps) -> String {
    if !text.contains("%%{") {
        return text.to_string();
    }
    let normalized = js::trim(text).replace('\'', "\"");
    let mut i = 0;
    while let Some(k) = normalized[i..].find("%%{") {
        let at = i + k;
        match directive_at(&normalized, at) {
            Some((end, kind, _g3, g4)) => {
                if kind == "wrap" {
                    config.set("wrap", Value::Bool(true));
                } else if kind == "init" || kind == "initialize" {
                    if let Some(body) = g4.filter(|b| !b.is_empty()) {
                        // Mermaid ignores an init directive whose body is not valid JSON.
                        if let Some(args) = json::parse(js::trim(&body)) {
                            if is_record(&args) {
                                merge(config, &args);
                            }
                        }
                    }
                }
                i = end.max(at + 1);
            }
            None => i = at + 1,
        }
    }
    let mut found = Vec::new();
    let mut i = 0;
    while let Some(k) = text[i..].find("%%{") {
        let at = i + k;
        match directive_at(text, at) {
            Some((end, ..)) => {
                found.push((at, end));
                i = end.max(at + 1);
            }
            None => i = at + 1,
        }
    }
    remove_all(text, &found, steps)
}

/// `text.replace(RE_HTML_TAG, ...)`: /<(\w+)([^>]*)>/g, with each
/// attribute's double quotes made single (`="([^"]*)"` to `='$1'`).
fn quote_attrs(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'<' {
            let mut e = i + 1;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            if e > i + 1 {
                if let Some(k) = text[e..].find('>') {
                    let close = e + k;
                    out.push_str(&text[last..e]);
                    let attrs = &text[e..close];
                    let mut a = 0;
                    while let Some(q) = attrs[a..].find("=\"") {
                        let s = a + q;
                        match attrs[s + 2..].find('"') {
                            Some(r) => {
                                out.push_str(&attrs[a..s]);
                                out.push_str("='");
                                out.push_str(&attrs[s + 2..s + 2 + r]);
                                out.push('\'');
                                a = s + 2 + r + 1;
                            }
                            None => break,
                        }
                    }
                    out.push_str(&attrs[a..]);
                    out.push('>');
                    last = close + 1;
                    i = close + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&text[last..]);
    out
}

/// `String(value)`.
pub fn to_js_string(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Num(n) => js::num_str(*n),
        Value::Str(s) => s.clone(),
        Value::Arr(a) => a.iter().map(to_js_string).collect::<Vec<_>>().join(","),
        Value::Obj(_) => "[object Object]".to_string(),
    }
}

/// JS truthiness of a YAML value.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Num(n)) => *n != 0.0 && !n.is_nan(),
        Some(Value::Str(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// Ports `preprocess`.
pub fn preprocess(source: &str) -> Result<Preprocessed, PeleError> {
    preprocess_with(source, None)
}

/// `preprocess(source, steps)`: `steps` maps normalised source to `with_comments`.
pub fn preprocess_with(source: &str, mut steps: Steps) -> Result<Preprocessed, PeleError> {
    let mut text = normalize_line_breaks(source);
    // A tag needs a closing bracket, so nothing after the last one can match.
    let last_close = if text.contains('<') {
        text.rfind('>')
    } else {
        None
    };
    if let Some(lc) = last_close.filter(|l| *l > 0) {
        text = quote_attrs(&text[..lc + 1]) + &text[lc + 1..];
    }

    let mut config = Value::obj();
    let mut title = None;
    let mut line_offset = 0;

    let front = if text.contains("---") {
        match_front_matter(&text)
    } else {
        None
    };
    if let Some(front) = front {
        let body = if !front.indent.is_empty() {
            front
                .body
                .split('\n')
                .map(|line| line.strip_prefix(front.indent.as_str()).unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            front.body.clone()
        };
        let parsed = parse_yaml(&body).map_err(|m| {
            PeleError::new(
                format!("Front matter is not valid YAML. {}", m),
                PeleErrorCode::Syntax,
            )
        })?;
        if is_record(&parsed) {
            if truthy(parsed.get("title")) {
                title = parsed.get("title").map(to_js_string);
            }
            if let Some(c) = parsed.get("config").filter(|c| is_record(c)) {
                merge(&mut config, c);
            }
            if truthy(parsed.get("displayMode")) {
                let mode = to_js_string(parsed.get("displayMode").unwrap_or(&Value::Null));
                if !config.get("gantt").is_some_and(is_record) {
                    config.set("gantt", Value::obj());
                }
                if let Some(g) = config.get_mut("gantt") {
                    g.set("displayMode", Value::Str(mode));
                }
            }
        }
        let cut = front.length.min(text.len());
        line_offset = text[..cut].matches('\n').count();
        if let Some(st) = steps.as_deref_mut() {
            st.push(alloc::vec![(0, js::len16(&text[..cut]), 0)]);
        }
        text = text[cut..].to_string();
    }

    let text = directives(&text, &mut config, steps);
    Ok(Preprocessed {
        text: cleanup_comments(&text),
        title,
        config,
        with_comments: text,
        line_offset,
    })
}

/// Does what Mermaid's `text.replace(/keyword.*:\S*#.*;/g, drop the last
/// character)` does, in one pass (`dropColorSemicolons`).
fn drop_color_semicolons(text: &str, keyword: &str, steps: Steps) -> String {
    let mut edits: Vec<Edit> = Vec::new();
    let out = drop_color_semicolons_into(text, keyword, &mut edits);
    if let Some(st) = steps {
        st.push(edits);
    }
    out
}

fn drop_color_semicolons_into(text: &str, keyword: &str, edits: &mut Vec<Edit>) -> String {
    let Some(mut from) = text.find(keyword) else {
        return text.to_string();
    };
    let mut out = String::new();
    let mut last = 0;
    loop {
        let line_end = text[from..]
            .char_indices()
            .find(|(_, c)| is_line_end(*c))
            .map(|(k, _)| from + k)
            .unwrap_or(text.len());
        let mut after_colon = false;
        let mut hash = None;
        for (k, c) in text[from + keyword.len()..line_end].char_indices() {
            let j = from + keyword.len() + k;
            if c == ':' {
                after_colon = true;
            } else if c == '#' {
                if after_colon {
                    hash = Some(j);
                    break;
                }
            } else if is_space(c) {
                after_colon = false;
            }
        }
        if let Some(hash) = hash {
            if let Some(semi) = text[..line_end].rfind(';') {
                if semi > hash {
                    edits.push(js::edit16(text, semi, semi + 1, 0));
                    out.push_str(&text[last..semi]);
                    last = semi + 1;
                }
            }
        }
        if line_end >= text.len() {
            break;
        }
        let next = line_end
            + text[line_end..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        match text[next..].find(keyword) {
            Some(k) => from = next + k,
            None => break,
        }
    }
    if last == 0 {
        return text.to_string();
    }
    out.push_str(&text[last..]);
    out
}

/// Ports `encodeEntities`: Mermaid hides `#name;` entity codes behind
/// placeholder characters so `#` and `;` survive parsing.
pub fn encode_entities(text: &str) -> String {
    encode_entities_with(text, None)
}

/// `encodeEntities(text, steps)`.
pub fn encode_entities_with(text: &str, mut steps: Steps) -> String {
    if !text.contains('#') {
        return text.to_string();
    }
    let first = drop_color_semicolons(text, "style", steps.as_deref_mut());
    let text = drop_color_semicolons(&first, "classDef", steps.as_deref_mut());
    let mut edits: Vec<Edit> = Vec::new();
    // RE_ENTITY = /#\w+;/g
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'#' {
            let s = i + 1;
            let mut e = s;
            while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_') {
                e += 1;
            }
            if e > s && b.get(e) == Some(&b';') {
                let inner = &text[s..e];
                out.push_str(&text[last..i]);
                // RE_INT = /^\+?\d+$/
                let numeric = inner.bytes().all(|c| c.is_ascii_digit());
                edits.push(js::edit16(
                    &text,
                    i,
                    e + 1,
                    js::len16(OPEN) + numeric as usize + js::len16(inner) + js::len16(CLOSE),
                ));
                out.push_str(OPEN);
                if numeric {
                    out.push('\u{b0}');
                }
                out.push_str(inner);
                out.push_str(CLOSE);
                last = e + 1;
                i = e + 1;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&text[last..]);
    if let Some(st) = steps {
        st.push(edits);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter_directives_and_comments() {
        let p = preprocess("---\ntitle: Hi\nconfig:\n  flowchart:\n    curve: linear\n---\nflowchart TD\n%% note\nA-->B\n").unwrap();
        assert_eq!(p.title.as_deref(), Some("Hi"));
        assert_eq!(p.text, "flowchart TD\nA-->B\n");
        assert_eq!(
            p.config.get("flowchart").and_then(|f| f.get("curve")),
            Some(&Value::s("linear"))
        );
        let d =
            preprocess("%%{init: {'flowchart': {'curve': 'step'}}}%%\ngraph LR\nA-->B").unwrap();
        assert_eq!(d.text, "graph LR\nA-->B");
        assert_eq!(
            d.config.get("flowchart").and_then(|f| f.get("curve")),
            Some(&Value::s("step"))
        );
        assert_eq!(
            encode_entities("A[#35; and #quot;]"),
            alloc::format!("A[{o}\u{b0}35{c} and {o}quot{c}]", o = OPEN, c = CLOSE)
        );
        assert_eq!(
            drop_color_semicolons("style A fill:#f9f;", "style", None),
            "style A fill:#f9f"
        );
        assert_eq!(quote_attrs("<span class=\"x\">"), "<span class='x'>");
    }
}
