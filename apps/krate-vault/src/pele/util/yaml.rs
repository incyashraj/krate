//! Ports util/yaml.ts: a small YAML reader covering what Mermaid feeds to
//! js-yaml with the JSON schema (front matter, `@{...}` shape data): block
//! and flow collections, quoted and plain scalars, and block scalars.
//!
//! Values are the app's JSON values ([`crate::json::Value`]), whose
//! objects keep their keys in the order they came in.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::json::Value;
use crate::pele::js;

/// Ports `type YamlValue`.
pub type YamlValue = Value;

const MAX_DEPTH: usize = 64;

/// Why reading stopped: an error, or (`Incomplete`) a quoted scalar or
/// flow collection that runs past the text read so far.
#[derive(Debug, Clone, PartialEq)]
pub enum YamlError {
    Error(String),
    Incomplete(String),
}

impl YamlError {
    pub fn message(&self) -> &str {
        match self {
            YamlError::Error(m) | YamlError::Incomplete(m) => m,
        }
    }
}

type R<T> = Result<T, YamlError>;

fn err<T>(m: &str) -> R<T> {
    Err(YamlError::Error(m.to_string()))
}

/// Ports `assign`.
fn assign(target: &mut Vec<(String, Value)>, key: String, value: Value) {
    match target.iter_mut().find(|(k, _)| *k == key) {
        Some(entry) => entry.1 = value,
        None => target.push((key, value)),
    }
}

/// Ports `class Reader`.
struct Reader {
    lines: Vec<String>,
    i: usize,
}

impl Reader {
    fn new(src: &str) -> Reader {
        let unified;
        let src = if src.contains('\r') {
            unified = src.replace("\r\n", "\n").replace('\r', "\n");
            unified.as_str()
        } else {
            src
        };
        Reader {
            lines: src.split('\n').map(|s| s.to_string()).collect(),
            i: 0,
        }
    }

    fn skip_blank(&mut self) {
        while self.i < self.lines.len() {
            let t = js::trim(&self.lines[self.i]);
            if !t.is_empty() && !t.starts_with('#') && t != "---" {
                break;
            }
            self.i += 1;
        }
    }

    fn indent(&self) -> R<usize> {
        let line = self.lines[self.i].as_bytes();
        let mut k = 0;
        while k < line.len() && line[k] == b' ' {
            k += 1;
        }
        if line.get(k) == Some(&b'\t') {
            return err("YAML: tabs are not allowed for indentation");
        }
        Ok(k)
    }
}

fn is_blank(ch: Option<u8>) -> bool {
    matches!(ch, Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r'))
}

/// `RE_KEY_START.test(first)`: /[\s#:,[\]{}&*!|>'"%@`]/.
fn key_start(c: char) -> bool {
    js::is_space(c) || "#:,[]{}&*!|>'\"%@`".contains(c)
}

/// Ports `splitKey`: splits `key: rest` at the start of a line, returning
/// the raw key and the offset where the value starts.
fn split_key(line: &str) -> Option<(String, usize)> {
    let b = line.as_bytes();
    let first = line.chars().next();
    let end;
    let colon;
    if first == Some('"') || first == Some('\'') {
        let q = b[0];
        let mut i = 1;
        loop {
            if i >= b.len() {
                return None;
            }
            let c = b[i];
            if q == b'"' && c == b'\\' {
                i += 2;
            } else if c == q {
                if q == b'"' || b.get(i + 1) != Some(&b'\'') {
                    break;
                }
                i += 2;
            } else {
                i += 1;
            }
        }
        end = i + 1;
        let mut k = end;
        while is_blank(b.get(k).copied()) {
            k += 1;
        }
        if b.get(k) != Some(&b':') {
            return None;
        }
        colon = k;
    } else {
        let first = first?;
        if key_start(first) {
            return None;
        }
        if first == '-' && (b.len() < 2 || is_blank(Some(b[1])) || b[1] == b':' || b[1] == b'#') {
            return None;
        }
        let mut k = 1;
        while k < b.len() && b[k] != b':' && b[k] != b'#' {
            k += 1;
        }
        if b.get(k) != Some(&b':') {
            return None;
        }
        colon = k;
        let mut e = colon;
        while e > 0 && is_blank(Some(b[e - 1])) {
            e -= 1;
        }
        end = e;
    }
    let mut length = colon + 1;
    if length < b.len() && !is_blank(Some(b[length])) {
        return None;
    }
    while is_blank(b.get(length).copied()) {
        length += 1;
    }
    Some((line[..end].to_string(), length))
}

fn all_digits(s: &str, radix: u32) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_digit(radix))
}

/// `RE_INT.test(s) || RE_FLOAT.test(s)`.
fn is_decimal(s: &str) -> bool {
    let t = s.strip_prefix(['-', '+']).unwrap_or(s);
    // RE_INT: 0|[1-9][0-9]*
    if t == "0" || (all_digits(t, 10) && !t.starts_with('0')) {
        return true;
    }
    // RE_FLOAT: (?:[0-9][0-9]*(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][-+]?[0-9]+)?
    let (mant, exp) = match t.find(['e', 'E']) {
        Some(k) => (&t[..k], Some(&t[k + 1..])),
        None => (t, None),
    };
    if let Some(e) = exp {
        let e = e.strip_prefix(['-', '+']).unwrap_or(e);
        if !all_digits(e, 10) {
            return false;
        }
    }
    match mant.split_once('.') {
        None => all_digits(mant, 10),
        Some((a, f)) => {
            if a.is_empty() {
                all_digits(f, 10)
            } else {
                all_digits(a, 10) && (f.is_empty() || all_digits(f, 10))
            }
        }
    }
}

/// Ports `scalar`.
fn scalar(raw: &str) -> Value {
    let s = js::trim(raw);
    if matches!(s, "" | "~" | "null" | "Null" | "NULL") {
        return Value::Null;
    }
    if matches!(s, "true" | "True" | "TRUE") {
        return Value::Bool(true);
    }
    if matches!(s, "false" | "False" | "FALSE") {
        return Value::Bool(false);
    }
    let c = s.as_bytes()[0];
    if c != b'-' && c != b'+' && c != b'.' && !c.is_ascii_digit() {
        return Value::Str(s.to_string());
    }
    if is_decimal(s) {
        return Value::Num(js::number(s));
    }
    let t = s.strip_prefix(['-', '+']).unwrap_or(s);
    let radix = if let Some(d) = t.strip_prefix("0x") {
        all_digits(d, 16).then_some((16, d))
    } else if let Some(d) = t.strip_prefix("0o") {
        all_digits(d, 8).then_some((8, d))
    } else if let Some(d) = t.strip_prefix("0b") {
        all_digits(d, 2).then_some((2, d))
    } else {
        None
    };
    if let Some((r, d)) = radix {
        let mut v = 0.0f64;
        for ch in d.chars() {
            v = v * r as f64 + ch.to_digit(r).unwrap_or(0) as f64;
        }
        return Value::Num(if s.starts_with('-') { -v } else { v });
    }
    if matches!(t, ".inf" | ".Inf" | ".INF") {
        return Value::Num(if s.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    if matches!(s, ".nan" | ".NaN" | ".NAN") {
        return Value::Num(f64::NAN);
    }
    Value::Str(s.to_string())
}

/// Ports `ESCAPES`.
fn escape(e: char) -> Option<char> {
    Some(match e {
        '0' => '\0',
        'a' => '\x07',
        'b' => '\x08',
        't' => '\t',
        'n' => '\n',
        'v' => '\x0b',
        'f' => '\x0c',
        'r' => '\r',
        'e' => '\x1b',
        ' ' => ' ',
        '"' => '"',
        '/' => '/',
        '\\' => '\\',
        'N' => '\u{85}',
        '_' => '\u{a0}',
        'L' => '\u{2028}',
        'P' => '\u{2029}',
        _ => return None,
    })
}

/// Ports `fold`: folds the line breaks of a scalar: blanks around a break
/// go, one break becomes a space, and each further break stays.
fn fold(text: &str) -> String {
    if !text.contains('\n') {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len() - 1;
    let mut joined = String::new();
    for (i, line) in lines.iter().enumerate() {
        let mut l: &str = line;
        if i > 0 {
            l = l.trim_start_matches([' ', '\t']);
        }
        if i < last {
            l = l.trim_end_matches([' ', '\t']);
        }
        if i > 0 {
            joined.push('\n');
        }
        joined.push_str(l);
    }
    let mut out = String::new();
    let mut run = 0;
    for c in joined.chars() {
        if c == '\n' {
            run += 1;
            continue;
        }
        if run > 0 {
            if run == 1 {
                out.push(' ');
            } else {
                for _ in 0..run - 1 {
                    out.push('\n');
                }
            }
            run = 0;
        }
        out.push(c);
    }
    if run == 1 {
        out.push(' ');
    } else if run > 1 {
        for _ in 0..run - 1 {
            out.push('\n');
        }
    }
    out
}

/// Ports `unquoteDouble`.
fn unquote_double(body: &str) -> R<String> {
    if !body.contains('\\') {
        return Ok(fold(body));
    }
    // body.replace(/\\\n[ \t]*/g, '')
    let mut joined = String::new();
    let mut it = body.char_indices().peekable();
    while let Some((_, c)) = it.next() {
        if c == '\\' && it.peek().map(|(_, n)| *n) == Some('\n') {
            it.next();
            while matches!(it.peek(), Some((_, ' ')) | Some((_, '\t'))) {
                it.next();
            }
            continue;
        }
        joined.push(c);
    }
    let s: Vec<char> = fold(&joined).chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < s.len() {
        let ch = s[i];
        if ch != '\\' {
            out.push(ch);
            i += 1;
            continue;
        }
        i += 1;
        let e = s.get(i).copied();
        match e {
            Some(x @ ('x' | 'u' | 'U')) => {
                let len = if x == 'x' {
                    2
                } else if x == 'u' {
                    4
                } else {
                    8
                };
                let hex: String = s[(i + 1).min(s.len())..(i + 1 + len).min(s.len())]
                    .iter()
                    .collect();
                let n = js::parse_int(&hex, 16);
                // String.fromCodePoint throws on what is no code point.
                match (n.is_finite() && (0.0..=1114111.0).contains(&n))
                    .then(|| char::from_u32(n as u32))
                    .flatten()
                {
                    Some(c) => out.push(c),
                    None => return err("YAML: invalid code point"),
                }
                i += len + 1;
            }
            Some(x) => match escape(x) {
                Some(c) => {
                    out.push(c);
                    i += 1;
                }
                None => return err("YAML: unknown escape sequence"),
            },
            None => return err("YAML: unknown escape sequence"),
        }
    }
    Ok(out)
}

/// Ports `skip`.
fn skip(s: &str, mut pos: usize, commas: bool) -> usize {
    let b = s.as_bytes();
    while pos < b.len() {
        let c = b[pos];
        if c == b' ' || c == b'\n' || c == b'\t' || c == b'\r' || (commas && c == b',') {
            pos += 1;
        } else if c == b'#' && (pos == 0 || s[..pos].chars().next_back().is_some_and(js::is_space))
        {
            pos = s[pos..].find('\n').map(|k| pos + k).unwrap_or(s.len());
        } else {
            break;
        }
    }
    pos
}

/// Ports `entryStart`: entries of a flow collection are separated by
/// exactly one comma, as js-yaml requires.
fn entry_start(ch: u8, separated: bool) -> R<()> {
    if !separated {
        return err("YAML: missed comma between flow collection entries");
    }
    if ch == b',' {
        return err("YAML: expected the node content, but found ','");
    }
    Ok(())
}

/// `String(key)` for a flow key.
fn key_string(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Num(n) => js::num_str(*n),
        Value::Str(s) => s.clone(),
        Value::Arr(a) => a.iter().map(key_string).collect::<Vec<_>>().join(","),
        Value::Obj(_) => "[object Object]".to_string(),
    }
}

/// Ports `flow`: parses a flow value starting at s[pos]; returns the
/// value and the index after it.
fn flow(s: &str, pos: usize, stops: &str, depth: usize) -> R<(Value, usize)> {
    if depth > MAX_DEPTH {
        return err("YAML: nesting is too deep");
    }
    let b = s.as_bytes();
    let mut pos = skip(s, pos, false);
    let ch = b.get(pos).copied();
    if ch == Some(b'{') {
        let mut out: Vec<(String, Value)> = Vec::new();
        pos += 1;
        let mut separated = true;
        loop {
            pos = skip(s, pos, false);
            if pos >= b.len() {
                return Err(YamlError::Incomplete(
                    "YAML: unexpected end of the stream within a flow collection".to_string(),
                ));
            }
            if b[pos] == b'}' {
                return Ok((Value::Obj(out), pos + 1));
            }
            entry_start(b[pos], separated)?;
            let (key, p) = flow(s, pos, ":,}", depth + 1)?;
            pos = skip(s, p, false);
            let mut value = Value::Null;
            if b.get(pos) == Some(&b':') {
                let (v, p) = flow(s, pos + 1, ",}", depth + 1)?;
                value = v;
                pos = p;
            }
            assign(&mut out, key_string(&key), value);
            pos = skip(s, pos, false);
            separated = b.get(pos) == Some(&b',');
            if separated {
                pos += 1;
            }
        }
    }
    if ch == Some(b'[') {
        let mut out: Vec<Value> = Vec::new();
        pos += 1;
        let mut separated = true;
        loop {
            pos = skip(s, pos, false);
            if pos >= b.len() {
                return Err(YamlError::Incomplete(
                    "YAML: unexpected end of the stream within a flow collection".to_string(),
                ));
            }
            if b[pos] == b']' {
                return Ok((Value::Arr(out), pos + 1));
            }
            entry_start(b[pos], separated)?;
            let (value, p) = flow(s, pos, ",]", depth + 1)?;
            out.push(value);
            pos = skip(s, p, false);
            separated = b.get(pos) == Some(&b',');
            if separated {
                pos += 1;
            }
        }
    }
    if ch == Some(b'"') {
        let mut end = pos + 1;
        while end < b.len() && b[end] != b'"' {
            end += if b[end] == b'\\' { 2 } else { 1 };
        }
        if end >= b.len() {
            return Err(YamlError::Incomplete(
                "YAML: unexpected end of the stream within a double quoted scalar".to_string(),
            ));
        }
        return Ok((Value::Str(unquote_double(&s[pos + 1..end])?), end + 1));
    }
    if ch == Some(b'\'') {
        let mut end = pos + 1;
        while end < b.len() {
            if b[end] == b'\'' {
                if b.get(end + 1) != Some(&b'\'') {
                    break;
                }
                end += 1;
            }
            end += 1;
        }
        if end >= b.len() {
            return Err(YamlError::Incomplete(
                "YAML: unexpected end of the stream within a single quoted scalar".to_string(),
            ));
        }
        return Ok((
            Value::Str(fold(&s[pos + 1..end]).replace("''", "'")),
            end + 1,
        ));
    }
    let mut end = pos;
    while end < b.len() {
        let c = b[end];
        if stops.as_bytes().contains(&c)
            && (c != b':' || end + 1 >= b.len() || {
                let n = s[end + 1..].chars().next().unwrap_or(' ');
                js::is_space(n) || ",[]{}".contains(n)
            })
        {
            break;
        }
        if c == b'#' && end > pos && s[..end].chars().next_back().is_some_and(js::is_space) {
            let nl = s[end..].find('\n').map(|k| end + k).unwrap_or(s.len());
            return Ok((scalar(&fold(js::trim(&s[pos..end]))), nl));
        }
        end += 1;
    }
    Ok((scalar(&fold(js::trim(&s[pos..end]))), end))
}

/// Ports `stripComment`.
fn strip_comment(s: &str) -> &str {
    let b = s.as_bytes();
    let mut quote = 0u8;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if quote != 0 {
            if c == b'\\' && quote == b'"' {
                i += 1;
            } else if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'#' && (i == 0 || b[i - 1] == b' ' || b[i - 1] == b'\t') {
            return &s[..i];
        }
        i += 1;
    }
    s
}

/// `line.length - line.trimStart().length` in characters.
fn lead(line: &str) -> usize {
    line.chars().take_while(|c| js::is_space(*c)).count()
}

/// `line.slice(k)` for k characters.
fn after_chars(line: &str, k: usize) -> &str {
    let at = line
        .char_indices()
        .nth(k)
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    &line[at..]
}

/// Ports `blockScalar`.
fn block_scalar(r: &mut Reader, header: &str, parent_indent: isize) -> String {
    let folded = header.starts_with('>');
    let chomp = if header.contains('-') {
        '-'
    } else if header.contains('+') {
        '+'
    } else {
        ' '
    };
    let explicit = header.bytes().find(|c| (b'1'..=b'9').contains(c));
    let mut indent: isize = match explicit {
        Some(d) => parent_indent + (d - b'0') as isize,
        None => -1,
    };
    let mut out: Vec<String> = Vec::new();
    while r.i < r.lines.len() {
        let line = r.lines[r.i].clone();
        if js::trim(&line).is_empty() {
            out.push(String::new());
            r.i += 1;
            continue;
        }
        let k = lead(&line) as isize;
        if indent == -1 {
            if k <= parent_indent {
                break;
            }
            indent = k;
        }
        if k < indent {
            break;
        }
        out.push(after_chars(&line, indent as usize).to_string());
        r.i += 1;
    }
    let mut trailing = 0;
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
        trailing += 1;
    }
    let text = if folded {
        let mut text = String::new();
        for i in 0..out.len() {
            let cur = &out[i];
            if i > 0 {
                let prev = &out[i - 1];
                if cur.is_empty() {
                    text.push('\n');
                } else if !prev.is_empty() {
                    let sp = |s: &str| s.starts_with([' ', '\t']);
                    text.push(if sp(prev) || sp(cur) { '\n' } else { ' ' });
                }
            }
            text.push_str(cur);
        }
        text
    } else {
        out.join("\n")
    };
    if out.is_empty() {
        return if chomp == '+' {
            "\n".repeat(trailing)
        } else {
            String::new()
        };
    }
    match chomp {
        '-' => text,
        '+' => text + &"\n".repeat(trailing + 1),
        _ => text + "\n",
    }
}

/// `/^[|>][-+0-9]*\s*(?:#.*)?$/.test(rest)`.
fn block_header(rest: &str) -> bool {
    let b = rest.as_bytes();
    let mut i = 1;
    while i < b.len() && (b[i] == b'-' || b[i] == b'+' || b[i].is_ascii_digit()) {
        i += 1;
    }
    let tail = js::trim_start(&rest[i..]);
    tail.is_empty()
        || (tail.starts_with('#') && !tail.contains(['\n', '\r', '\u{2028}', '\u{2029}']))
}

/// Ports `inlineValue`.
fn inline_value(r: &mut Reader, rest: &str, indent: isize) -> R<Value> {
    let first = rest.as_bytes().first().copied();
    if (first == Some(b'|') || first == Some(b'>')) && block_header(rest) {
        return Ok(Value::Str(block_scalar(r, rest, indent)));
    }
    if matches!(first, Some(b'{') | Some(b'[') | Some(b'"') | Some(b'\'')) {
        // The value may continue on later lines. Read them in growing batches until it closes.
        let start = r.i;
        let mut text = rest.to_string();
        let mut taken = 0;
        let mut batch = 1;
        loop {
            match flow(&text, 0, "", 0) {
                Ok((value, end)) => {
                    let used = text[..end.min(text.len())].matches('\n').count();
                    let line_end = text[end.min(text.len())..]
                        .find('\n')
                        .map(|k| end + k)
                        .unwrap_or(text.len());
                    if !js::trim(strip_comment(&text[end.min(text.len())..line_end])).is_empty() {
                        return err("YAML: unexpected content after a value");
                    }
                    r.i = start + used;
                    return Ok(value);
                }
                Err(YamlError::Incomplete(m)) => {
                    if start + taken >= r.lines.len() {
                        return Err(YamlError::Error(m));
                    }
                    let to = (start + taken + batch).min(r.lines.len());
                    let more = r.lines[start + taken..to].join("\n");
                    text.push('\n');
                    text.push_str(&more);
                    taken += to - (start + taken);
                    batch *= 2;
                }
                Err(e) => return Err(e),
            }
        }
    }
    let mut text = js::trim(strip_comment(rest)).to_string();
    while r.i < r.lines.len() {
        let line = r.lines[r.i].clone();
        if js::trim(&line).is_empty() {
            let mut j = r.i;
            while j < r.lines.len() && js::trim(&r.lines[j]).is_empty() {
                j += 1;
            }
            if j >= r.lines.len() || lead(&r.lines[j]) as isize <= indent {
                break;
            }
            text.push_str(&"\n".repeat(j - r.i));
            r.i = j;
            continue;
        }
        if lead(&line) as isize <= indent {
            break;
        }
        let more = js::trim(strip_comment(&line));
        if !text.ends_with('\n') {
            text.push(' ');
        }
        text.push_str(more);
        r.i += 1;
    }
    Ok(scalar(&text))
}

/// Ports `block`.
fn block(r: &mut Reader, min_indent: usize, depth: usize) -> R<Value> {
    if depth > MAX_DEPTH {
        return err("YAML: nesting is too deep");
    }
    r.skip_blank();
    if r.i >= r.lines.len() {
        return Ok(Value::Null);
    }
    let indent = r.indent()?;
    if indent < min_indent {
        return Ok(Value::Null);
    }
    let line = r.lines[r.i][indent..].to_string();

    if line == "-" || line.starts_with("- ") {
        let mut out: Vec<Value> = Vec::new();
        while r.i < r.lines.len() {
            r.skip_blank();
            if r.i >= r.lines.len() || r.indent()? != indent {
                break;
            }
            let cur = r.lines[r.i][indent..].to_string();
            if cur != "-" && !cur.starts_with("- ") {
                break;
            }
            let rest = &cur[1..];
            let pad = lead(rest) + 1;
            let body = js::trim(strip_comment(rest)).to_string();
            if body.is_empty() {
                r.i += 1;
                out.push(block(r, indent + 1, depth + 1)?);
            } else if split_key(&body).is_some() {
                r.lines[r.i] = " ".repeat(indent + pad) + js::trim_start(rest);
                out.push(block(r, indent + pad, depth + 1)?);
            } else {
                r.i += 1;
                out.push(inline_value(r, js::trim(rest), indent as isize)?);
            }
        }
        return Ok(Value::Arr(out));
    }

    if split_key(&line).is_none() {
        r.i += 1;
        return inline_value(r, js::trim(&line), indent as isize - 1);
    }

    let mut out: Vec<(String, Value)> = Vec::new();
    while r.i < r.lines.len() {
        r.skip_blank();
        if r.i >= r.lines.len() {
            break;
        }
        let k = r.indent()?;
        if k < indent {
            break;
        }
        if k > indent {
            return err("YAML: bad indentation of a mapping entry");
        }
        let cur = r.lines[r.i][indent..].to_string();
        let Some((raw_key, length)) = split_key(&cur) else {
            return err("YAML: can not read a block mapping entry");
        };
        let key = key_string(&flow(&raw_key, 0, "", 0)?.0);
        let rest = cur[length..].to_string();
        r.i += 1;
        if js::trim(strip_comment(&rest)).is_empty() {
            r.skip_blank();
            if r.i < r.lines.len() {
                let next = r.indent()?;
                let next_line = &r.lines[r.i][next..];
                let seq = next_line == "-" || next_line.starts_with("- ");
                let value = if next > indent || (seq && next == indent) {
                    block(r, if seq { indent } else { indent + 1 }, depth + 1)?
                } else {
                    Value::Null
                };
                assign(&mut out, key, value);
            } else {
                assign(&mut out, key, Value::Null);
            }
        } else {
            let v = inline_value(r, js::trim(&rest), indent as isize)?;
            assign(&mut out, key, v);
        }
    }
    Ok(Value::Obj(out))
}

/// Ports `parseYaml`.
pub fn parse_yaml(src: &str) -> Result<Value, String> {
    let trimmed = js::trim(src);
    if trimmed.starts_with(['{', '[']) {
        match flow(trimmed, 0, "", 0) {
            Ok((value, end)) => {
                if skip(trimmed, end, false) == trimmed.len() {
                    return Ok(value);
                }
            }
            Err(e) => return Err(e.message().to_string()),
        }
    }
    let mut r = Reader::new(src);
    let value = block(&mut r, 0, 0).map_err(|e| e.message().to_string())?;
    r.skip_blank();
    if r.i < r.lines.len() && js::trim(&r.lines[r.i]) != "..." {
        return Err("YAML: end of the stream or a document separator is expected".to_string());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_front_matter_and_shape_data() {
        let v =
            parse_yaml("title: Hello\nconfig:\n  theme: dark\n  flowchart:\n    curve: linear\n")
                .unwrap();
        assert_eq!(v.get("title"), Some(&Value::s("Hello")));
        assert_eq!(
            v.get("config")
                .and_then(|c| c.get("flowchart"))
                .and_then(|f| f.get("curve")),
            Some(&Value::s("linear"))
        );
        let d = parse_yaml("{\n shape: rect, label: \"Hi there\" \n}").unwrap();
        assert_eq!(d.get("shape"), Some(&Value::s("rect")));
        assert_eq!(d.get("label"), Some(&Value::s("Hi there")));
        assert_eq!(
            parse_yaml("n: 12").unwrap().get("n"),
            Some(&Value::Num(12.0))
        );
    }
}
