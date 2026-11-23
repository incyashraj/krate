//! Ports svg/theme.ts: the root element's private colour aliases, the
//! fonts, and Mermaid style declarations split into what applies to a
//! shape and what applies to its text.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::builder::{esc, has_invalid_xml};
use crate::pele::js;

/// Private aliases set once on the root element, so each use is short and
/// the public --pele-* tokens keep their fallbacks (`ROOT_STYLE`).
pub const ROOT_STYLE: &str = concat!(
    "--_bg:var(--pele-bg,#fff);--_fg:var(--pele-fg,#1f1f1f);--_m:var(--pele-muted,#6e6e6e);",
    "--_l:var(--pele-line,#1f1f1f);--_s:var(--pele-surface,#f3f3f3);--_a:var(--pele-surface-alt,#e6e6e6);",
    "--_b:var(--pele-border,#8a8a8a);--_c:var(--pele-accent,#5b7bd5);color:var(--_fg)"
);

pub const FONT_MONO: &str = "var(--_fm)";
pub const RADIUS: &str = "var(--pele-radius,4px)";

/// Use CSS for rx variables to support older browsers (`ROUNDED`).
pub const ROUNDED: &str = " style=\"rx:var(--_r)\"";

/// Ports `setsFill`: expects a shape style from resolve_style(). RE_FILL = /[";]fill:/.
pub fn sets_fill(shape: &str) -> bool {
    shape.contains("\"fill:") || shape.contains(";fill:")
}

/// Ports `rounded`.
pub fn rounded(attrs: &str) -> String {
    match attrs.find(" style=\"") {
        None => format!("{}{}", attrs, ROUNDED),
        Some(at) => format!("{}rx:var(--_r);{}", &attrs[..at + 8], &attrs[at + 8..]),
    }
}

/// The colour of what a diagram marks as critical (`CRITICAL`).
pub const CRITICAL: &str = "var(--pele-critical,#e45756)";

/// One `FAMILY` of RE_FAMILIES at `p`, returning where it ends:
/// `"[^"'\\\n<>{}();:@]*"|'[^"'\\\n<>{}();:@]*'|[\p{L}\p{N}_-][\p{L}\p{N}_ .-]*`.
fn family(s: &str, p: usize) -> Option<usize> {
    let bad = |c: char| "\"'\\\n<>{}();:@".contains(c);
    let mut it = s[p..].char_indices();
    let (_, first) = it.next()?;
    if first == '"' || first == '\'' {
        for (i, c) in it {
            if c == first {
                return Some(p + i + 1);
            }
            if bad(c) {
                return None;
            }
        }
        return None;
    }
    let word = |c: char| c.is_alphabetic() || c.is_numeric() || c == '_' || c == '-';
    if !word(first) {
        return None;
    }
    let mut end = p + first.len_utf8();
    for (i, c) in it {
        if word(c) || c == ' ' || c == '.' {
            end = p + i + c.len_utf8();
        } else {
            break;
        }
    }
    Some(end)
}

fn skip_ws(s: &str, mut p: usize) -> usize {
    while let Some(c) = s[p..].chars().next() {
        if !js::is_space(c) {
            break;
        }
        p += c.len_utf8();
    }
    p
}

/// `RE_FAMILIES.test(list)`: a list of font names, quoted or not, and
/// nothing that could end the declaration it is put in.
fn is_families(list: &str) -> bool {
    let mut p = skip_ws(list, 0);
    loop {
        match family(list, p) {
            Some(e) => p = e,
            None => return false,
        }
        p = skip_ws(list, p);
        if p == list.len() {
            return true;
        }
        if list.as_bytes()[p] != b',' {
            return false;
        }
        p = skip_ws(list, p + 1);
    }
}

/// Ports `families`.
fn families<'a>(list: Option<&'a str>, generic: &'a str) -> &'a str {
    match list {
        Some(l) if !l.is_empty() && is_families(l) => l,
        _ => generic,
    }
}

/// Ports `fontStyle`: the fonts the labels were measured in are the ones
/// they are drawn in, unless the page names others.
pub fn font_style(size: f64, family: Option<&str>, mono: Option<&str>) -> String {
    format!(
        ";--_fm:var(--pele-font-mono,{});font:{}px sans-serif;letter-spacing:normal;word-spacing:normal;text-transform:none;direction:ltr;font-family:var(--pele-font,{})",
        families(mono, "monospace"),
        js::num_str(size),
        families(family, "sans-serif")
    )
}

const SHAPE_PROPS: [&str; 12] = [
    "fill",
    "fill-opacity",
    "stroke",
    "stroke-width",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-opacity",
    "opacity",
    "rx",
    "ry",
];

const TEXT_PROPS: [&str; 9] = [
    "color",
    "font-size",
    "font-family",
    "font-weight",
    "font-style",
    "text-decoration",
    "letter-spacing",
    "word-spacing",
    "opacity",
];

/// `RE_UNSAFE.test(value)`:
/// /url\s*\(|expression\s*\(|javascript:|[<>{};\\]|@import|image-set|\/\*/i
fn unsafe_value(value: &str) -> bool {
    if value.contains(['<', '>', '{', '}', ';', '\\']) || value.contains("/*") {
        return true;
    }
    let lower = value.to_ascii_lowercase();
    if lower.contains("javascript:") || lower.contains("@import") || lower.contains("image-set") {
        return true;
    }
    for word in ["url", "expression"] {
        let mut from = 0;
        while let Some(k) = lower[from..].find(word) {
            let after = skip_ws(&lower, from + k + word.len());
            if lower[after..].starts_with('(') {
                return true;
            }
            from += k + word.len();
        }
    }
    false
}

/// Ports `interface ResolvedStyle`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedStyle {
    pub shape: String,
    pub line: String,
    pub text: String,
    pub stroke: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub font_size: Option<f64>,
}

/// An insertion-ordered map's `set`: a key keeps its first place.
fn set(map: &mut Vec<(String, String)>, key: &str, value: &str) {
    match map.iter_mut().find(|(k, _)| k == key) {
        Some(entry) => entry.1 = value.to_string(),
        None => map.push((key.to_string(), value.to_string())),
    }
}

fn get<'a>(map: &'a [(String, String)], key: &str) -> Option<&'a str> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// `value.replace(/!important/gi, '')`.
fn drop_important(v: &str) -> String {
    let lower = v.to_ascii_lowercase();
    if !lower.contains("!important") {
        return v.to_string();
    }
    let mut out = String::new();
    let mut i = 0;
    while i < v.len() {
        if lower[i..].starts_with("!important") {
            i += 10;
            continue;
        }
        let c = v[i..].chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `/^[\d.]+(px)?$/.test(size)`.
fn plain_px(size: &str) -> bool {
    let body = size.strip_suffix("px").unwrap_or(size);
    !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit() || b == b'.')
}

/// Ports `resolveStyle`: splits Mermaid style declarations into what
/// applies to a shape and what applies to its text.
pub fn resolve_style<S: AsRef<str>>(declarations: &[S]) -> ResolvedStyle {
    if declarations.is_empty() {
        return ResolvedStyle::default();
    }
    let mut shape: Vec<(String, String)> = Vec::new();
    let mut text: Vec<(String, String)> = Vec::new();
    for decl in declarations {
        let decl = decl.as_ref();
        let at = match decl.find(':') {
            Some(a) if a > 0 => a,
            _ => continue,
        };
        let prop = js::trim(&decl[..at]).to_lowercase();
        let value_owned = drop_important(&decl[at + 1..]);
        let value = js::trim(&value_owned);
        if value.is_empty() || unsafe_value(value) || has_invalid_xml(value) {
            continue;
        }
        if SHAPE_PROPS.contains(&prop.as_str()) {
            set(&mut shape, &prop, value);
        }
        if TEXT_PROPS.contains(&prop.as_str()) {
            set(
                &mut text,
                if prop == "color" { "fill" } else { &prop },
                value,
            );
        }
    }
    if shape.is_empty() && text.is_empty() {
        return ResolvedStyle::default();
    }
    let mut shape_css = String::new();
    let mut line_css = String::new();
    for (prop, value) in &shape {
        shape_css.push_str(&format!("{}:{};", prop, value));
        if prop != "fill" && prop != "fill-opacity" {
            line_css.push_str(&format!("{}:{};", prop, value));
        }
    }
    let mut text_css = String::new();
    for (prop, value) in &text {
        text_css.push_str(&format!("{}:{};", prop, value));
    }
    let weight = get(&text, "font-weight");
    let size = get(&text, "font-size");
    let px = match size {
        Some(s) if !s.is_empty() && plain_px(s) => Some(js::parse_float(s)),
        _ => None,
    };
    let wrap = |css: &str| {
        if css.is_empty() {
            String::new()
        } else {
            format!(" style=\"{}\"", esc(css))
        }
    };
    ResolvedStyle {
        shape: wrap(&shape_css),
        line: wrap(&line_css),
        text: wrap(&text_css),
        stroke: get(&shape, "stroke").map(|s| s.to_string()),
        bold: matches!(weight, Some("bold") | Some("bolder"))
            || weight.is_some_and(|w| js::number(w) >= 600.0),
        italic: get(&text, "font-style") == Some("italic"),
        font_size: px.filter(|p| *p > 0.0 && *p <= 200.0),
    }
}

/// Ports `classNames`: raw.replace(/[^\w-]+/g, ' ').trim(), with a
/// leading space.
pub fn class_names(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    let mut in_run = false;
    for c in raw.chars() {
        if js::is_word(c) || c == '-' {
            cleaned.push(c);
            in_run = false;
        } else if !in_run {
            cleaned.push(' ');
            in_run = true;
        }
    }
    let t = js::trim(&cleaned);
    if t.is_empty() {
        String::new()
    } else {
        format!(" {}", t)
    }
}

const SERIES: [&str; 8] = [
    "#4c78a8", "#f58518", "#54a24b", "#e45756", "#72b7b2", "#eeca3b", "#b279a2", "#9d755d",
];

/// Ports `seriesColor`: colour for the nth data series in a chart,
/// cycling through eight tokens.
pub fn series_color(index: i64) -> String {
    let k = index.rem_euclid(8);
    format!("var(--pele-series-{},{})", k + 1, SERIES[k as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles() {
        let s = resolve_style(&[
            "fill:#f9f",
            "stroke:#333",
            "stroke-width:4px",
            "color:red",
            "font-weight:bold",
        ]);
        assert_eq!(
            s.shape,
            " style=\"fill:#f9f;stroke:#333;stroke-width:4px;\""
        );
        assert_eq!(s.line, " style=\"stroke:#333;stroke-width:4px;\"");
        assert_eq!(s.text, " style=\"fill:red;font-weight:bold;\"");
        assert!(s.bold);
        assert!(unsafe_value("url (x)"));
        assert_eq!(class_names("default foo  bar"), " default foo bar");
        assert!(is_families("Inter, \"Segoe UI\", sans-serif"));
        assert!(!is_families("a;b"));
    }
}
