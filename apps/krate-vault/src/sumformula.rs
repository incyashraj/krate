//! A base's own summaries: formulas over `values`, the column's values, as
//! the notes app writes them in a base's `summaries:` section
//! (`values.sum() * 2`, `values.filter(value > 3).length`,
//! `values.mean().round(1)`). The notes app's built-in summaries are
//! formulas of the same kind; Vault works those out directly
//! (`bases::summarize`).

use crate::json::Value;
use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Str(String),
    Id(String),
    Op(&'static str),
}

fn lex(s: &str) -> Vec<Tok> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            let st = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            out.push(Tok::Num(s[st..i].parse().unwrap_or(0.0)));
            continue;
        }
        if c == b'"' || c == b'\'' {
            let q = c;
            i += 1;
            let mut t = String::new();
            while i < b.len() && b[i] != q {
                if b[i] == b'\\' && i + 1 < b.len() {
                    i += 1;
                }
                let ch = s[i..].chars().next().unwrap_or(' ');
                t.push(ch);
                i += ch.len_utf8();
            }
            i += 1;
            out.push(Tok::Str(t));
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let st = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Id(String::from(&s[st..i])));
            continue;
        }
        let two = s.get(i..i + 2).unwrap_or("");
        let op: &'static str = match two {
            "==" => "==",
            "!=" => "!=",
            "<=" => "<=",
            ">=" => ">=",
            "&&" => "&&",
            "||" => "||",
            _ => match c {
                b'(' => "(",
                b')' => ")",
                b'.' => ".",
                b',' => ",",
                b'+' => "+",
                b'-' => "-",
                b'*' => "*",
                b'/' => "/",
                b'%' => "%",
                b'<' => "<",
                b'>' => ">",
                b'!' => "!",
                _ => "?",
            },
        };
        i += op.len();
        out.push(Tok::Op(op));
    }
    out
}

struct P<'a> {
    t: &'a [Tok],
    i: usize,
    values: &'a [Value],
    value: Option<&'a Value>,
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Num(n) => Some(*n),
        Value::Str(s) => s.trim().parse().ok(),
        _ => None,
    }
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

fn day(v: &Value) -> Option<i64> {
    let s = v.str()?;
    let b = s.as_bytes();
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let n = |a: usize, z: usize| s.get(a..z)?.parse::<i64>().ok();
    Some(crate::dates::days_from_civil(
        n(0, 4)? as i32,
        n(5, 7)? as u8,
        n(8, 10)? as u8,
    ))
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn eat(&mut self, op: &str) -> bool {
        if self.peek()
            == Some(&Tok::Op(match op {
                "(" => "(",
                ")" => ")",
                "." => ".",
                "," => ",",
                _ => "?",
            }))
        {
            self.i += 1;
            return true;
        }
        false
    }
    fn op(&self) -> Option<&'static str> {
        match self.peek() {
            Some(Tok::Op(o)) => Some(o),
            _ => None,
        }
    }

    fn or(&mut self) -> Value {
        let mut l = self.and();
        while self.op() == Some("||") {
            self.i += 1;
            let r = self.and();
            l = Value::Bool(truthy(&l) || truthy(&r));
        }
        l
    }
    fn and(&mut self) -> Value {
        let mut l = self.cmp();
        while self.op() == Some("&&") {
            self.i += 1;
            let r = self.cmp();
            l = Value::Bool(truthy(&l) && truthy(&r));
        }
        l
    }
    fn cmp(&mut self) -> Value {
        let l = self.add();
        let Some(op) = self
            .op()
            .filter(|o| ["==", "!=", "<", ">", "<=", ">="].contains(o))
        else {
            return l;
        };
        self.i += 1;
        let r = self.add();
        let ord = match (num(&l), num(&r)) {
            (Some(a), Some(b)) if !matches!(l, Value::Str(_)) || !matches!(r, Value::Str(_)) => {
                a.partial_cmp(&b)
            }
            _ => match (day(&l), day(&r)) {
                (Some(a), Some(b)) => Some(a.cmp(&b)),
                _ => Some(crate::bases::show(&l).cmp(&crate::bases::show(&r))),
            },
        };
        use core::cmp::Ordering::*;
        Value::Bool(match op {
            "==" => {
                l == r
                    || ord == Some(Equal)
                        && !matches!((&l, &r), (Value::Null, _) | (_, Value::Null))
            }
            "!=" => {
                !(l == r
                    || ord == Some(Equal)
                        && !matches!((&l, &r), (Value::Null, _) | (_, Value::Null)))
            }
            "<" => ord == Some(Less),
            ">" => ord == Some(Greater),
            "<=" => matches!(ord, Some(Less | Equal)),
            _ => matches!(ord, Some(Greater | Equal)),
        })
    }
    fn add(&mut self) -> Value {
        let mut l = self.mul();
        while let Some(op) = self.op().filter(|o| *o == "+" || *o == "-") {
            self.i += 1;
            let r = self.mul();
            l = match (num(&l), num(&r), op) {
                (Some(a), Some(b), "+") => Value::Num(a + b),
                (Some(a), Some(b), _) => Value::Num(a - b),
                _ => match (day(&l), day(&r), op) {
                    (Some(a), Some(b), "-") => {
                        let n = a - b;
                        Value::Str(alloc::format!(
                            "{n} {}",
                            if n.abs() == 1 { "day" } else { "days" }
                        ))
                    }
                    _ => {
                        let mut s = crate::bases::show(&l);
                        s.push_str(&crate::bases::show(&r));
                        Value::Str(s)
                    }
                },
            };
        }
        l
    }
    fn mul(&mut self) -> Value {
        let mut l = self.unary();
        while let Some(op) = self.op().filter(|o| ["*", "/", "%"].contains(o)) {
            self.i += 1;
            let r = self.unary();
            l = match (num(&l), num(&r)) {
                (Some(a), Some(b)) => Value::Num(match op {
                    "*" => a * b,
                    "/" => a / b,
                    _ => a % b,
                }),
                _ => Value::Null,
            };
        }
        l
    }
    fn unary(&mut self) -> Value {
        match self.op() {
            Some("-") => {
                self.i += 1;
                num(&self.unary()).map_or(Value::Null, |n| Value::Num(-n))
            }
            Some("!") => {
                self.i += 1;
                Value::Bool(!truthy(&self.unary()))
            }
            _ => self.postfix(),
        }
    }
    fn args(&mut self) -> Vec<(usize, usize)> {
        // Each argument as a token range, evaluated by the method itself
        // (a filter's argument is evaluated once per value).
        let mut out = Vec::new();
        if !self.eat("(") {
            return out;
        }
        let mut depth = 0;
        let mut st = self.i;
        while let Some(t) = self.peek() {
            match t {
                Tok::Op("(") => depth += 1,
                Tok::Op(")") if depth == 0 => {
                    if self.i > st {
                        out.push((st, self.i));
                    }
                    self.i += 1;
                    return out;
                }
                Tok::Op(")") => depth -= 1,
                Tok::Op(",") if depth == 0 => {
                    out.push((st, self.i));
                    st = self.i + 1;
                }
                _ => {}
            }
            self.i += 1;
        }
        out
    }
    fn eval_range(&self, (a, b): (usize, usize), value: Option<&Value>) -> Value {
        let mut p = P {
            t: &self.t[a..b],
            i: 0,
            values: self.values,
            value: value.or(self.value),
        };
        p.or()
    }
    fn postfix(&mut self) -> Value {
        let mut v = self.primary();
        while self.eat(".") {
            let Some(Tok::Id(m)) = self.peek().cloned() else {
                return Value::Null;
            };
            self.i += 1;
            let called = self.peek() == Some(&Tok::Op("("));
            let args = if called { self.args() } else { Vec::new() };
            v = self.method(v, &m, &args);
        }
        v
    }
    fn primary(&mut self) -> Value {
        let Some(t) = self.peek().cloned() else {
            return Value::Null;
        };
        self.i += 1;
        match t {
            Tok::Num(n) => Value::Num(n),
            Tok::Str(s) => Value::Str(s),
            Tok::Op("(") => {
                let v = self.or();
                self.eat(")");
                v
            }
            Tok::Id(id) => match id.as_str() {
                "values" => Value::Arr(self.values.to_vec()),
                "value" => self.value.cloned().unwrap_or(Value::Null),
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                "null" => Value::Null,
                "min" | "max" | "number" | "abs" => {
                    let args = self.args();
                    let ns: Vec<f64> = args
                        .iter()
                        .filter_map(|r| num(&self.eval_range(*r, None)))
                        .collect();
                    match id.as_str() {
                        "min" => ns
                            .iter()
                            .copied()
                            .reduce(f64::min)
                            .map_or(Value::Null, Value::Num),
                        "max" => ns
                            .iter()
                            .copied()
                            .reduce(f64::max)
                            .map_or(Value::Null, Value::Num),
                        "abs" => ns.first().map_or(Value::Null, |n| Value::Num(n.abs())),
                        _ => ns.first().map_or(Value::Null, |n| Value::Num(*n)),
                    }
                }
                _ => Value::Null,
            },
            _ => Value::Null,
        }
    }

    fn method(&self, v: Value, m: &str, args: &[(usize, usize)]) -> Value {
        let arg = |i: usize| args.get(i).map(|r| self.eval_range(*r, None));
        let items: Vec<Value> = match &v {
            Value::Arr(a) => a.clone(),
            _ => Vec::new(),
        };
        let nums: Vec<f64> = items.iter().filter_map(num).collect();
        let days: Vec<(i64, &Value)> = items.iter().filter_map(|x| Some((day(x)?, x))).collect();
        let n = |x: f64| Value::Num(x);
        match m {
            "length" => match &v {
                Value::Arr(a) => n(a.len() as f64),
                Value::Str(s) => n(s.chars().count() as f64),
                _ => Value::Null,
            },
            "sum" => n(nums.iter().sum()),
            "mean" | "average" if !nums.is_empty() => {
                n(nums.iter().sum::<f64>() / nums.len() as f64)
            }
            "min" => nums.iter().copied().reduce(f64::min).map_or(Value::Null, n),
            "max" => nums.iter().copied().reduce(f64::max).map_or(Value::Null, n),
            "median" if !nums.is_empty() => {
                let mut s = nums.clone();
                s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
                let k = s.len() / 2;
                n(if s.len().is_multiple_of(2) {
                    (s[k - 1] + s[k]) / 2.0
                } else {
                    s[k]
                })
            }
            "stddev" if !nums.is_empty() => {
                let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                n(
                    (nums.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / nums.len() as f64)
                        .sqrt(),
                )
            }
            "earliest" => days
                .iter()
                .min_by_key(|d| d.0)
                .map_or(Value::Null, |d| d.1.clone()),
            "latest" => days
                .iter()
                .max_by_key(|d| d.0)
                .map_or(Value::Null, |d| d.1.clone()),
            "unique" => {
                let mut out: Vec<Value> = Vec::new();
                for x in items {
                    if !out.contains(&x) {
                        out.push(x);
                    }
                }
                Value::Arr(out)
            }
            "filter" | "map" => {
                let Some(r) = args.first() else {
                    return v;
                };
                let mut out = Vec::new();
                for x in &items {
                    let res = self.eval_range(*r, Some(x));
                    if m == "map" {
                        out.push(res);
                    } else if truthy(&res) {
                        out.push(x.clone());
                    }
                }
                Value::Arr(out)
            }
            "reverse" => Value::Arr(items.into_iter().rev().collect()),
            "isEmpty" => Value::Bool(!truthy(&v)),
            "isType" => {
                let want = arg(0).map(|a| crate::bases::show(&a)).unwrap_or_default();
                Value::Bool(match want.as_str() {
                    "null" => matches!(v, Value::Null),
                    "number" => matches!(v, Value::Num(_)),
                    "string" => matches!(v, Value::Str(_)),
                    "boolean" => matches!(v, Value::Bool(_)),
                    "list" => matches!(v, Value::Arr(_)),
                    _ => false,
                })
            }
            "contains" => {
                let a = arg(0).unwrap_or(Value::Null);
                Value::Bool(
                    items.contains(&a) || crate::bases::show(&v).contains(&crate::bases::show(&a)),
                )
            }
            "join" => {
                let sep = arg(0).map_or(String::from(","), |a| crate::bases::show(&a));
                Value::Str(
                    items
                        .iter()
                        .map(crate::bases::show)
                        .collect::<Vec<_>>()
                        .join(&sep),
                )
            }
            "round" | "toFixed" => {
                let Some(x) = num(&v) else {
                    return Value::Null;
                };
                let places = arg(0).and_then(|a| num(&a)).unwrap_or(0.0).clamp(0.0, 10.0) as i32;
                let f = 10f64.powi(places);
                let r = (x * f).round() / f;
                if m == "toFixed" {
                    Value::Str(alloc::format!("{r:.*}", places as usize))
                } else {
                    n(r)
                }
            }
            "abs" => num(&v).map_or(Value::Null, |x| n(x.abs())),
            "ceil" => num(&v).map_or(Value::Null, |x| n(x.ceil())),
            "floor" => num(&v).map_or(Value::Null, |x| n(x.floor())),
            _ => Value::Null,
        }
    }
}

/// A summary formula over the column's values, shown as a cell shows it.
pub fn summary(formula: &str, values: &[Value]) -> Option<String> {
    let t = lex(formula);
    let mut p = P {
        t: &t,
        i: 0,
        values,
        value: None,
    };
    let v = p.or();
    match v {
        Value::Null => None,
        Value::Arr(a) => Some(
            a.iter()
                .map(crate::bases::show)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        v => Some(crate::bases::show(&v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bases_own_summaries_are_worked_out() {
        let v = [
            Value::Num(3.0),
            Value::Num(5.0),
            Value::Num(2.0),
            Value::Null,
        ];
        assert_eq!(summary("values.sum() * 2", &v).as_deref(), Some("20"));
        assert_eq!(
            summary("values.filter(value > 2).length", &v).as_deref(),
            Some("2")
        );
        assert_eq!(
            summary("values.mean().round(1)", &v).as_deref(),
            Some("3.3")
        );
        assert_eq!(
            summary("values.max() - values.min()", &v).as_deref(),
            Some("3")
        );
        assert_eq!(
            summary("values.filter(!value.isType(\"null\")).length", &v).as_deref(),
            Some("3")
        );
        assert_eq!(summary("values.unique().length", &v).as_deref(), Some("4"));
        let d = [Value::s("2026-12-03"), Value::s("2026-12-01")];
        assert_eq!(
            summary("values.earliest()", &d).as_deref(),
            Some("2026-12-01")
        );
        assert_eq!(
            summary("values.latest() - values.earliest()", &d).as_deref(),
            Some("2 days")
        );
    }
}
