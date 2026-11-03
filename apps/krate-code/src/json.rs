//! Just enough JSON for cargo's messages and the two protocols the editor
//! speaks (the Language Server Protocol and the Debug Adapter Protocol).
//!
//! A value tree, a parser that never panics on bad input, and a writer. Plain
//! `alloc`, so `cargo test` proves it natively.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    /// Keys in the order they came, so a writer's output is stable.
    Obj(Vec<(String, Value)>),
}

pub static NULL: Value = Value::Null;

impl Value {
    pub fn get(&self, key: &str) -> &Value {
        match self {
            Value::Obj(items) => items
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v)
                .unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    /// `v.at(&["a", "b"])` is `v.get("a").get("b")`.
    pub fn at(&self, path: &[&str]) -> &Value {
        let mut v = self;
        for k in path {
            v = v.get(k);
        }
        v
    }

    pub fn idx(&self, i: usize) -> &Value {
        match self {
            Value::Arr(items) => items.get(i).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn int(&self) -> Option<i64> {
        self.num().map(|n| n as i64)
    }

    pub fn uint(&self) -> Option<usize> {
        self.num().filter(|n| *n >= 0.0).map(|n| n as usize)
    }

    pub fn bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn arr(&self) -> &[Value] {
        match self {
            Value::Arr(items) => items,
            _ => &[],
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn obj_items(&self) -> &[(String, Value)] {
        match self {
            Value::Obj(items) => items,
            _ => &[],
        }
    }

    /// Set `key` on an object (replacing it), or do nothing on anything else.
    pub fn set(&mut self, key: &str, v: Value) {
        if let Value::Obj(items) = self {
            if let Some(slot) = items.iter_mut().find(|(k, _)| k == key) {
                slot.1 = v;
            } else {
                items.push((String::from(key), v));
            }
        }
    }

    pub fn to_json(&self) -> String {
        let mut out = String::new();
        write(self, &mut out);
        out
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(String::from(s))
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Num(n as f64)
    }
}
impl From<usize> for Value {
    fn from(n: usize) -> Self {
        Value::Num(n as f64)
    }
}
impl From<u64> for Value {
    fn from(n: u64) -> Self {
        Value::Num(n as f64)
    }
}
impl From<i32> for Value {
    fn from(n: i32) -> Self {
        Value::Num(n as f64)
    }
}
impl From<Vec<Value>> for Value {
    fn from(v: Vec<Value>) -> Self {
        Value::Arr(v)
    }
}

/// An object from pairs: `obj([("a", 1.into())])`.
pub fn obj<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Obj(
        pairs
            .into_iter()
            .map(|(k, v)| (String::from(k), v))
            .collect(),
    )
}

fn write(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Num(n) => write_num(*n, out),
        Value::Str(s) => write_str(s, out),
        Value::Arr(items) => {
            out.push('[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(it, out);
            }
            out.push(']');
        }
        Value::Obj(items) => {
            out.push('{');
            for (i, (k, it)) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_str(k, out);
                out.push(':');
                write(it, out);
            }
            out.push('}');
        }
    }
}

fn write_num(n: f64, out: &mut String) {
    if !n.is_finite() {
        out.push_str("null");
        return;
    }
    if n == (n as i64) as f64 && n.abs() < 9.0e15 {
        let i = n as i64;
        if i < 0 {
            out.push('-');
        }
        out.push_str(&crate::text::num(i.unsigned_abs() as usize));
        return;
    }
    // Fractions are rare here (never sent by the editor); a plain
    // fixed-point form is enough.
    let neg = n < 0.0;
    let a = if neg { -n } else { n };
    let whole = a as u64;
    let frac = ((a - whole as f64) * 1_000_000.0) as u64;
    if neg {
        out.push('-');
    }
    out.push_str(&crate::text::num(whole as usize));
    out.push('.');
    let f = crate::text::num(frac as usize);
    for _ in f.len()..6 {
        out.push('0');
    }
    out.push_str(f.trim_end_matches('0'));
    if out.ends_with('.') {
        out.push('0');
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u00");
                let hex = b"0123456789abcdef";
                out.push(hex[(c as usize) >> 4] as char);
                out.push(hex[(c as usize) & 15] as char);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parse one JSON document. `None` on anything malformed.
pub fn parse(text: &str) -> Option<Value> {
    let mut p = Parser {
        s: text.as_bytes(),
        i: 0,
        depth: 0,
    };
    let v = p.value()?;
    p.ws();
    if p.i == p.s.len() {
        Some(v)
    } else {
        None
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while let Some(b) = self.s.get(self.i) {
            if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn lit(&mut self, word: &[u8], v: Value) -> Option<Value> {
        if self.s.get(self.i..self.i + word.len()) == Some(word) {
            self.i += word.len();
            Some(v)
        } else {
            None
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.ws();
        self.depth += 1;
        if self.depth > 256 {
            return None;
        }
        let v = match self.peek()? {
            b'n' => self.lit(b"null", Value::Null),
            b't' => self.lit(b"true", Value::Bool(true)),
            b'f' => self.lit(b"false", Value::Bool(false)),
            b'"' => self.string().map(Value::Str),
            b'[' => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(b']') {
                    self.i += 1;
                } else {
                    loop {
                        items.push(self.value()?);
                        self.ws();
                        match self.peek()? {
                            b',' => self.i += 1,
                            b']' => {
                                self.i += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
                Some(Value::Arr(items))
            }
            b'{' => {
                self.i += 1;
                let mut items = Vec::new();
                self.ws();
                if self.peek() == Some(b'}') {
                    self.i += 1;
                } else {
                    loop {
                        self.ws();
                        if self.peek()? != b'"' {
                            return None;
                        }
                        let k = self.string()?;
                        self.ws();
                        if self.peek()? != b':' {
                            return None;
                        }
                        self.i += 1;
                        let v = self.value()?;
                        items.push((k, v));
                        self.ws();
                        match self.peek()? {
                            b',' => self.i += 1,
                            b'}' => {
                                self.i += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
                Some(Value::Obj(items))
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        };
        self.depth -= 1;
        v
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        while matches!(
            self.peek(),
            Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
        ) {
            self.i += 1;
        }
        let t = core::str::from_utf8(self.s.get(start..self.i)?).ok()?;
        parse_f64(t).map(Value::Num)
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = self.s.get(self.i..self.i + 4)?;
        self.i += 4;
        let mut v = 0u32;
        for b in h {
            v = v * 16 + (*b as char).to_digit(16)?;
        }
        Some(v)
    }

    fn string(&mut self) -> Option<String> {
        self.i += 1; // the opening quote
        let mut out = String::new();
        loop {
            let start = self.i;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' {
                    break;
                }
                self.i += 1;
            }
            out.push_str(core::str::from_utf8(self.s.get(start..self.i)?).ok()?);
            match self.peek()? {
                b'"' => {
                    self.i += 1;
                    return Some(out);
                }
                _ => {
                    self.i += 1;
                    let e = self.peek()?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut c = self.hex4()?;
                            if (0xD800..0xDC00).contains(&c)
                                && self.s.get(self.i..self.i + 2) == Some(b"\\u")
                            {
                                self.i += 2;
                                let lo = self.hex4()?;
                                c = 0x10000
                                    + ((c - 0xD800) << 10)
                                    + (lo.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            out.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                        }
                        _ => return None,
                    }
                }
            }
        }
    }
}

/// A float from JSON's number grammar, without `std`.
fn parse_f64(t: &str) -> Option<f64> {
    let (mant, exp) = match t.find(['e', 'E']) {
        Some(i) => (&t[..i], t[i + 1..].parse::<i32>().ok()?),
        None => (t, 0),
    };
    let neg = mant.starts_with('-');
    let mant = mant.trim_start_matches('-');
    let (whole, frac) = match mant.split_once('.') {
        Some((w, f)) => (w, f),
        None => (mant, ""),
    };
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut v = 0f64;
    for b in whole.bytes() {
        v = v * 10.0 + (b - b'0') as f64;
    }
    let mut scale = 1f64;
    for b in frac.bytes() {
        scale /= 10.0;
        v += (b - b'0') as f64 * scale;
    }
    let mut e = exp;
    while e > 0 {
        v *= 10.0;
        e -= 1;
    }
    while e < 0 {
        v /= 10.0;
        e += 1;
    }
    Some(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let t = r#"{"a":[1,-2,3.5,true,null],"b":"x\"y\n\u00e9\ud83d\ude00","c":{}}"#;
        let v = parse(t).unwrap();
        assert_eq!(v.get("a").idx(1).int(), Some(-2));
        assert_eq!(v.get("a").idx(2).num(), Some(3.5));
        assert_eq!(v.get("b").str(), Some("x\"y\né😀"));
        let again = parse(&v.to_json()).unwrap();
        assert_eq!(v, again);
    }

    #[test]
    fn bad_input_is_none_not_a_panic() {
        for t in [
            "",
            "{",
            "[1,",
            "\"abc",
            "{\"a\" 1}",
            "tru",
            "1 2",
            "{\"a\":\"\\u12\"}",
        ] {
            assert!(parse(t).is_none(), "{t}");
        }
        let deep = "[".repeat(10_000);
        assert!(parse(&deep).is_none());
    }

    #[test]
    fn numbers() {
        assert_eq!(parse("1e3").unwrap().num(), Some(1000.0));
        assert_eq!(parse("-0.25").unwrap().num(), Some(-0.25));
        assert_eq!(Value::from(42usize).to_json(), "42");
        assert_eq!(Value::Num(-7.0).to_json(), "-7");
    }

    #[test]
    fn builder_and_set() {
        let mut v = obj([("id", 1i64.into()), ("m", "x".into())]);
        v.set("m", "y".into());
        v.set("n", Value::Null);
        assert_eq!(v.to_json(), r#"{"id":1,"m":"y","n":null}"#);
    }
}
