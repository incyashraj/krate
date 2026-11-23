//! Ports diagrams/common/fit.ts: labels laid out as plain text.

use alloc::string::String;
use alloc::vec::Vec;

use crate::pele::js::{len16, trim};
use crate::pele::text::label::{layout_label, Icons, Label};
use crate::pele::text::measurer::Measure;

/// `/br\s*\/?>/iy` at byte `i` of `s` (after the `<`).
fn br_rest(s: &str, i: usize) -> Option<usize> {
    let b = s.as_bytes();
    if b.len() < i + 2 || !b[i].eq_ignore_ascii_case(&b'b') || !b[i + 1].eq_ignore_ascii_case(&b'r')
    {
        return None;
    }
    let mut j = i + 2;
    for (k, c) in s[j..].char_indices() {
        if !crate::pele::js::is_space(c) {
            j = i + 2 + k;
            break;
        }
        j = i + 2 + k + c.len_utf8();
    }
    if b.get(j) == Some(&b'/') {
        j += 1;
    }
    if b.get(j) == Some(&b'>') {
        Some(j + 1)
    } else {
        None
    }
}

/// `text.replace(/<(?!br\s*\/?>)/gi, '&lt;')`.
fn escape_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, c) in text.char_indices() {
        if c == '<' && br_rest(text, i + 1).is_none() {
            out.push_str("&lt;");
        } else {
            out.push(c);
        }
    }
    out
}

/// Ports `plainLabel`: lays out text the way Mermaid's journey and
/// timeline draw it, as plain text in which only <br> is markup, so other
/// tags and icon names show as written.
pub fn plain_label(
    raw: Option<&str>,
    measurer: &dyn Measure,
    size: f64,
    max: f64,
    base: u32,
    icons: Icons,
) -> Label {
    let mut text: Option<String> = raw.map(String::from);
    if let Some(t) = text.as_mut() {
        if t.contains('<') {
            *t = escape_tags(t);
        }
        if t.contains(":fa-") {
            *t = t.replace(":fa-", "&colon;fa-");
        }
    }
    layout_label(text.as_deref(), false, measurer, size, max, base, icons)
}

/// `/\w+/y` at byte `i`.
fn word_run(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
        j += 1;
    }
    j
}

/// `raw.match(RE_UNIT)`: a line break, an encoded entity, a surrogate
/// pair, or one character, the pieces a word can be cut between.
fn units(raw: &str) -> Vec<&str> {
    let b = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let mut end = None;
        if b[i] == b'<' {
            end = br_rest(raw, i + 1);
        }
        if end.is_none() && raw[i..].starts_with("\u{fb02}\u{b0}") {
            let mut j = i + "\u{fb02}\u{b0}".len();
            if raw[j..].starts_with('\u{b0}') {
                let w = word_run(b, j + 2);
                if w > j + 2 && raw[w..].starts_with("\u{b6}\u{df}") {
                    end = Some(w + "\u{b6}\u{df}".len());
                }
            }
            if end.is_none() {
                let w = word_run(b, j);
                if w > j && raw[w..].starts_with("\u{b6}\u{df}") {
                    j = w;
                    end = Some(j + "\u{b6}\u{df}".len());
                }
            }
        }
        if end.is_none() && b[i] == b'&' {
            let mut j = i + 1;
            if b.get(j) == Some(&b'#') {
                j += 1;
            }
            let w = word_run(b, j);
            if w > j && b.get(w) == Some(&b';') {
                end = Some(w + 1);
            } else if j > i + 1 {
                // `&#` then no word: try `#` as not taken.
                let w = word_run(b, i + 1);
                if w > i + 1 && b.get(w) == Some(&b';') {
                    end = Some(w + 1);
                }
            }
        }
        let e = end.unwrap_or_else(|| i + raw[i..].chars().next().map_or(1, char::len_utf8));
        out.push(&raw[i..e]);
        i = e;
    }
    out
}

/// Ports `fitLabel`: layoutLabel wraps at spaces only; for a box of fixed
/// width, this also cuts any word too long to fit.
pub fn fit_label(
    raw: Option<&str>,
    measurer: &dyn Measure,
    size: f64,
    max: f64,
    base: u32,
    icons: Icons,
) -> Label {
    let label = plain_label(raw, measurer, size, max, base, icons);
    let Some(raw) = raw else {
        return label;
    };
    if label.width <= max {
        return label;
    }
    let mut out = String::new();
    let mut width = 0.0;
    for unit in units(raw) {
        if trim(unit).is_empty() || (len16(unit) > 2 && unit.starts_with('<')) {
            width = 0.0;
        } else {
            let w = measurer.width(if len16(unit) > 2 { "x" } else { unit }, size, base);
            if width > 0.0 && width + w > max {
                out.push('\n');
                width = 0.0;
            }
            width += w;
        }
        out.push_str(unit);
    }
    plain_label(Some(&out), measurer, size, max, base, icons)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_split_as_the_regex_does() {
        assert_eq!(units("a<BR/>b&amp;c"), ["a", "<BR/>", "b", "&amp;", "c"]);
        assert_eq!(units("&#35;x"), ["&#35;", "x"]);
        assert_eq!(units("<b>"), ["<", "b", ">"]);
        assert_eq!(
            units("x\u{fb02}\u{b0}\u{b0}35\u{b6}\u{df}y"),
            ["x", "\u{fb02}\u{b0}\u{b0}35\u{b6}\u{df}", "y"]
        );
        assert_eq!(escape_tags("<b>a<br>b<br />"), "&lt;b>a<br>b<br />");
    }
}
