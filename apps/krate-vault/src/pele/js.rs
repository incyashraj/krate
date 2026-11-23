//! What JavaScript does that Rust does differently, for the port of Pele's
//! renderer (everything outside the layout engine): `Number#toString`,
//! `Number(text)` and `parseFloat`, `String#trim` and `\s`, and string
//! lengths counted in UTF-16 code units. Each helper names the JS it
//! stands for.
//!
//! The renderer works on Rust strings (UTF-8). Where Pele reads a string
//! one UTF-16 code unit at a time, the port reads it one `char` at a time;
//! a character outside the Basic Multilingual Plane is two code units in
//! JS (a high and a low surrogate), and the helpers that count say so.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

pub(crate) use super::layered::{jhypot, jmax, jmin, jround};

/// `\s` in a JS regular expression, and what `String#trim` removes
/// (util/chars.ts `isSpace`, for a whole character).
pub fn is_space(c: char) -> bool {
    let c = c as u32;
    if c < 128 {
        return c == 32 || (9..=13).contains(&c);
    }
    c == 0xa0
        || c == 0x1680
        || (0x2000..=0x200a).contains(&c)
        || c == 0x2028
        || c == 0x2029
        || c == 0x202f
        || c == 0x205f
        || c == 0x3000
        || c == 0xfeff
}

/// `\w`: an ASCII letter, digit or underscore.
pub fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `String#trim`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `String#trimStart`.
pub fn trim_start(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `String#trimEnd`.
pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `string.length`: UTF-16 code units.
pub fn len16(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// A byte offset in `s` as a UTF-16 offset, as JS counts positions.
pub fn units_at(s: &str, byte: usize) -> usize {
    len16(&s[..floor_boundary(s, byte)])
}

/// A UTF-16 offset in `s` as a byte offset (the start of the character it
/// falls in).
pub fn byte_at(s: &str, units: usize) -> usize {
    let mut u = 0;
    for (i, c) in s.char_indices() {
        let n = c.len_utf16();
        if u + n > units {
            return i;
        }
        u += n;
    }
    s.len()
}

/// The largest character boundary at or before `byte`.
pub fn floor_boundary(s: &str, byte: usize) -> usize {
    let mut b = byte.min(s.len());
    while !s.is_char_boundary(b) {
        b -= 1;
    }
    b
}

/// An edit `(from, to, length)` in bytes of `text`, with `length` bytes
/// of `replacement`, in UTF-16 units as Pele records it.
pub fn edit16(
    text: &str,
    from: usize,
    to: usize,
    replacement_len16: usize,
) -> (usize, usize, usize) {
    (units_at(text, from), units_at(text, to), replacement_len16)
}

/// `Number.prototype.toString()` (radix 10): the shortest digits that read
/// back as the same number, written plainly for exponents from -7 to 21 and
/// as `1.5e+21` outside them.
pub fn num_str(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_string();
    }
    if x == 0.0 {
        return "0".to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    // Rust's `{:e}` writes the shortest round-trip digits too: `d.ddde-x`.
    let e = format!("{:e}", x.abs());
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // JS's n: the value is 0.digits * 10^n.
    let n = exp + 1;
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    if k <= n && n <= 21 {
        out.push_str(&digits);
        for _ in 0..(n - k) {
            out.push('0');
        }
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        for _ in 0..(-n) {
            out.push('0');
        }
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        let e = n - 1;
        out.push(if e < 0 { '-' } else { '+' });
        out.push_str(&format!("{}", e.abs()));
    }
    out
}

/// A string of decimal digits, optionally signed, with an optional
/// fraction and exponent, read the way JS reads number text. Returns the
/// value and how many bytes it took, or None when nothing numeric starts
/// at the beginning.
fn decimal_prefix(s: &str) -> Option<(f64, usize)> {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    if s[i..].starts_with("Infinity") {
        let v = if b.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
        return Some((v, i + 8));
    }
    let start_digits = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - start_digits;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        let j = i + 1;
        let mut k = j;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        frac_digits = k - j;
        if int_digits > 0 || frac_digits > 0 {
            i = k;
        }
    }
    if int_digits == 0 && frac_digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut k = i + 1;
        if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
            k += 1;
        }
        let d = k;
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        if k > d {
            i = k;
        }
    }
    let text = &s[..i];
    let v: f64 = text.parse().ok()?;
    Some((v, i))
}

/// `parseFloat(text)`.
pub fn parse_float(s: &str) -> f64 {
    let t = trim_start(s);
    match decimal_prefix(t) {
        Some((v, _)) => v,
        None => f64::NAN,
    }
}

/// `Number(text)` for a string: blank is 0, `0x`/`0o`/`0b` integers, and
/// otherwise the whole trimmed text must be a decimal number.
pub fn number(s: &str) -> f64 {
    let t = trim(s);
    if t.is_empty() {
        return 0.0;
    }
    let lower = t.as_bytes();
    if lower.len() > 2 && lower[0] == b'0' {
        let radix = match lower[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            let mut v = 0.0f64;
            for c in t[2..].chars() {
                match c.to_digit(radix) {
                    Some(d) => v = v * radix as f64 + d as f64,
                    None => return f64::NAN,
                }
            }
            return v;
        }
    }
    match decimal_prefix(t) {
        Some((v, used)) if used == t.len() => v,
        _ => f64::NAN,
    }
}

/// `parseInt(text, radix)` for the radixes Pele uses (10, 16, 36).
pub fn parse_int(s: &str, radix: u32) -> f64 {
    let t = trim_start(s);
    let (neg, t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let t = if radix == 16 && (t.starts_with("0x") || t.starts_with("0X")) {
        &t[2..]
    } else {
        t
    };
    let mut v = 0.0f64;
    let mut any = false;
    for c in t.chars() {
        match c.to_digit(radix) {
            Some(d) => {
                v = v * radix as f64 + d as f64;
                any = true;
            }
            None => break,
        }
    }
    if !any {
        return f64::NAN;
    }
    if neg {
        -v
    } else {
        v
    }
}

/// JS `Math.sign`.
pub fn sign(x: f64) -> f64 {
    if x.is_nan() || x == 0.0 {
        x
    } else if x > 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// `a || b` for a number.
pub fn or(a: f64, b: f64) -> f64 {
    if a != 0.0 && !a.is_nan() {
        a
    } else {
        b
    }
}

/// `String.prototype.split(sep)` with a string separator.
pub fn split<'a>(s: &'a str, sep: &str) -> Vec<&'a str> {
    if sep.is_empty() {
        return s
            .char_indices()
            .map(|(i, c)| &s[i..i + c.len_utf8()])
            .collect();
    }
    s.split(sep).collect()
}

/// `str.replace(from, to)` with a string pattern: only the first match.
pub fn replace_first(s: &str, from: &str, to: &str) -> String {
    match s.find(from) {
        Some(at) => {
            let mut out = String::with_capacity(s.len() + to.len());
            out.push_str(&s[..at]);
            out.push_str(to);
            out.push_str(&s[at + from.len()..]);
            out
        }
        None => s.to_string(),
    }
}

/// JS `toLowerCase` for the ASCII-only names Pele lowers (tags, props).
/// Non-ASCII letters are lowered as Unicode lowers them, as JS does.
pub fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// `Number.prototype.toFixed(digits)`: the exact value rounded to
/// `digits` decimals, a tie going to the larger number.
pub fn to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() || x.abs() >= 1e21 {
        return num_str(x);
    }
    // Every double's decimal expansion ends within 1074 places.
    let exact = format!("{:.1100}", x.abs());
    let (int, frac) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut ds: Vec<u8> = int.bytes().chain(frac.bytes().take(digits)).collect();
    if frac.as_bytes().get(digits).is_some_and(|d| *d >= b'5') {
        let mut k = ds.len();
        loop {
            if k == 0 {
                ds.insert(0, b'1');
                break;
            }
            k -= 1;
            if ds[k] == b'9' {
                ds[k] = b'0';
            } else {
                ds[k] += 1;
                break;
            }
        }
    }
    let split = ds.len() - digits;
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    out.push_str(core::str::from_utf8(&ds[..split]).unwrap_or("0"));
    if digits > 0 {
        out.push('.');
        out.push_str(core::str::from_utf8(&ds[split..]).unwrap_or(""));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_print_as_js_prints_them() {
        for (x, s) in [
            (1.0, "1"),
            (0.1, "0.1"),
            (-2.5, "-2.5"),
            (1e21, "1e+21"),
            (123456789012345680000.0, "123456789012345680000"),
            (1e-7, "1e-7"),
            (0.000001, "0.000001"),
            (1.5e-10, "1.5e-10"),
            (100.0, "100"),
            (0.3 + 0.6, "0.8999999999999999"),
        ] {
            assert_eq!(num_str(x), s);
        }
    }

    #[test]
    fn number_reads_as_js_reads() {
        assert_eq!(number("  12 "), 12.0);
        assert_eq!(number(""), 0.0);
        assert!(number("12px").is_nan());
        assert_eq!(number("0x1f"), 31.0);
        assert_eq!(parse_float("12.5px"), 12.5);
        assert_eq!(parse_float(".5"), 0.5);
        assert!(parse_float("px").is_nan());
        assert_eq!(parse_int("zz", 36), 1295.0);
    }
}
