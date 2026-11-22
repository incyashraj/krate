//! JSON, read and written by hand: the vault's settings (`.obsidian/*.json`)
//! and JSON Canvas files. Objects keep their keys in the order they came
//! in, so a file Vault reads and writes back changes only where it was
//! changed.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    pub fn obj() -> Value {
        Value::Obj(Vec::new())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        match self {
            Value::Obj(kv) => kv.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Set `key`, keeping its place if it is there already.
    pub fn set(&mut self, key: &str, v: Value) {
        if !matches!(self, Value::Obj(_)) {
            *self = Value::obj();
        }
        if let Value::Obj(kv) = self {
            match kv.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = v,
                None => kv.push((String::from(key), v)),
            }
        }
    }

    pub fn remove(&mut self, key: &str) {
        if let Value::Obj(kv) = self {
            kv.retain(|(k, _)| k != key);
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

    pub fn bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn arr(&self) -> &[Value] {
        match self {
            Value::Arr(a) => a,
            _ => &[],
        }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::str)
    }

    pub fn get_num(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(Value::num)
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(Value::bool)
    }

    pub fn s(v: &str) -> Value {
        Value::Str(String::from(v))
    }
}

/// Parse JSON text. `None` when it is not JSON.
pub fn parse(text: &str) -> Option<Value> {
    let mut p = Parser {
        b: text.as_bytes(),
        i: 0,
        depth: 0,
    };
    p.ws();
    let v = p.value()?;
    p.ws();
    (p.i == p.b.len()).then_some(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn lit(&mut self, word: &str, v: Value) -> Option<Value> {
        let end = self.i + word.len();
        if self.b.get(self.i..end)? == word.as_bytes() {
            self.i = end;
            Some(v)
        } else {
            None
        }
    }

    fn value(&mut self) -> Option<Value> {
        self.ws();
        self.depth += 1;
        if self.depth > 200 {
            return None;
        }
        let v = match self.peek()? {
            b'{' => {
                self.i += 1;
                let mut kv = Vec::new();
                if !self.eat(b'}') {
                    loop {
                        self.ws();
                        let k = self.string()?;
                        if !self.eat(b':') {
                            return None;
                        }
                        let v = self.value()?;
                        kv.push((k, v));
                        if self.eat(b',') {
                            continue;
                        }
                        if self.eat(b'}') {
                            break;
                        }
                        return None;
                    }
                }
                Value::Obj(kv)
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                if !self.eat(b']') {
                    loop {
                        a.push(self.value()?);
                        if self.eat(b',') {
                            continue;
                        }
                        if self.eat(b']') {
                            break;
                        }
                        return None;
                    }
                }
                Value::Arr(a)
            }
            b'"' => Value::Str(self.string()?),
            b't' => self.lit("true", Value::Bool(true))?,
            b'f' => self.lit("false", Value::Bool(false))?,
            b'n' => self.lit("null", Value::Null)?,
            _ => self.number()?,
        };
        self.depth -= 1;
        Some(v)
    }

    fn number(&mut self) -> Option<Value> {
        let start = self.i;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.i += 1;
        }
        let s = core::str::from_utf8(self.b.get(start..self.i)?).ok()?;
        if s.is_empty() {
            return None;
        }
        s.parse::<f64>().ok().map(Value::Num)
    }

    fn hex4(&mut self) -> Option<u32> {
        let s = core::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(s, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        if self.peek()? != b'"' {
            return None;
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while let Some(c) = self.peek() {
                if c == b'"' || c == b'\\' {
                    break;
                }
                self.i += 1;
            }
            out.push_str(core::str::from_utf8(self.b.get(start..self.i)?).ok()?);
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
                        b'n' => out.push('\n'),
                        b't' => out.push('\t'),
                        b'r' => out.push('\r'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'u' => {
                            let mut c = self.hex4()?;
                            if (0xD800..0xDC00).contains(&c)
                                && self.b.get(self.i) == Some(&b'\\')
                                && self.b.get(self.i + 1) == Some(&b'u')
                            {
                                self.i += 2;
                                let lo = self.hex4()?;
                                c = 0x10000
                                    + ((c - 0xD800) << 10)
                                    + (lo.wrapping_sub(0xDC00) & 0x3FF);
                            }
                            out.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                        }
                        other => out.push(other as char),
                    }
                }
            }
        }
    }
}

/// Write a string as a JSON string literal.
pub fn push_str(out: &mut String, s: &str) {
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
                let n = c as u32;
                for d in [n >> 4, n & 15] {
                    out.push(char::from_digit(d, 16).unwrap_or('0'));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A number the way JavaScript's `JSON.stringify` writes it: whole numbers
/// without a point.
pub fn push_num(out: &mut String, n: f64) {
    if !n.is_finite() {
        out.push_str("null");
        return;
    }
    if n == (n as i64) as f64 && n.abs() < 9.0e15 {
        let mut s = String::new();
        let v = n as i64;
        if v < 0 {
            out.push('-');
        }
        crate::text::push_num(&mut s, v.unsigned_abs() as usize);
        out.push_str(&s);
    } else {
        use core::fmt::Write;
        let _ = write!(out, "{}", n);
    }
}

/// One line, no spaces: `{"a":1,"b":[true]}`.
pub fn compact(v: &Value) -> String {
    let mut out = String::new();
    write(&mut out, v, None, 0);
    out
}

/// Indented the way `JSON.stringify(v, null, indent)` writes it.
pub fn pretty(v: &Value, indent: &str) -> String {
    let mut out = String::new();
    write(&mut out, v, Some(indent), 0);
    out
}

fn newline(out: &mut String, indent: Option<&str>, depth: usize) {
    if let Some(ind) = indent {
        out.push('\n');
        for _ in 0..depth {
            out.push_str(ind);
        }
    }
}

fn write(out: &mut String, v: &Value, indent: Option<&str>, depth: usize) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Num(n) => push_num(out, *n),
        Value::Str(s) => push_str(out, s),
        Value::Arr(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, indent, depth + 1);
                write(out, x, indent, depth + 1);
            }
            newline(out, indent, depth);
            out.push(']');
        }
        Value::Obj(kv) => {
            if kv.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, x)) in kv.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                newline(out, indent, depth + 1);
                push_str(out, k);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write(out, x, indent, depth + 1);
            }
            newline(out, indent, depth);
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_writes_back() {
        let t = "{\"a\": 1, \"b\": [true, false, null], \"c\": {\"d\": \"x\\ny \\u00e9\"}, \"e\": -2.5}";
        let v = parse(t).unwrap();
        assert_eq!(v.get_num("a"), Some(1.0));
        assert_eq!(v.get("c").unwrap().get_str("d"), Some("x\ny \u{e9}"));
        assert_eq!(
            compact(&v),
            "{\"a\":1,\"b\":[true,false,null],\"c\":{\"d\":\"x\\ny \u{e9}\"},\"e\":-2.5}"
        );
        assert_eq!(
            pretty(&parse("{\"a\":{\"b\":[1,2]},\"c\":{}}").unwrap(), "  "),
            "{\n  \"a\": {\n    \"b\": [\n      1,\n      2\n    ]\n  },\n  \"c\": {}\n}"
        );
        assert!(parse("{\"a\":").is_none());
        assert!(parse("[1,]").is_none());
        assert_eq!(
            parse("\"\\ud83d\\ude00\"").unwrap().str(),
            Some("\u{1F600}")
        );
    }

    #[test]
    fn set_keeps_key_order() {
        let mut v = parse("{\"x\":1,\"y\":2}").unwrap();
        v.set("x", Value::Num(5.0));
        v.set("z", Value::Bool(true));
        assert_eq!(compact(&v), "{\"x\":5,\"y\":2,\"z\":true}");
    }
}
