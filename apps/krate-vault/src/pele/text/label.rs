//! Ports text/label.ts: a label's text split into lines of styled spans
//! (inline HTML, Markdown emphasis, icons), wrapped to a width and
//! measured.
//!
//! Math (`$$...$$` with the host's `math` resolver) is not ported: with no
//! resolver Pele draws `$$` as text, and so does this.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::entities::decode_entities;
use super::measurer::{style, Measure};
use crate::pele::js::{self, is_space, jround};

/// The host's icons: markup for an icon name, or None when it has none
/// (`IconResolver`). Pele's core hands diagrams a resolver that decodes
/// entity codes in the name first.
pub type Icons<'a> = Option<&'a dyn Fn(&str) -> Option<String>>;

/// Ports `iconMarkup`: the markup the host has for an icon, or "".
pub fn icon_markup(icons: Icons, name: &str) -> String {
    match icons {
        Some(f) => f(name).unwrap_or_default(),
        None => String::new(),
    }
}

/// Ports `interface Span`.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: u32,
    pub icon: Option<String>,
    pub width: f64,
}

/// Ports `interface Label`. `heights` is present only when math needs
/// extra line height, which without math is never.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub lines: Vec<Vec<Span>>,
    pub widths: Vec<f64>,
    pub width: f64,
    pub height: f64,
    pub size: f64,
    pub line_height: f64,
    pub heights: Option<Vec<f64>>,
}

pub const LINE_HEIGHT: f64 = 1.5;
/// Lines of one wrapped label sit closer, so the label reads as one block.
const WRAPPED_LINE_HEIGHT: f64 = 1.3;

/// Ports `EMPTY`.
pub fn empty_label() -> Label {
    Label {
        lines: Vec::new(),
        widths: Vec::new(),
        width: 0.0,
        height: 0.0,
        size: 0.0,
        line_height: 0.0,
        heights: None,
    }
}

/// Ports `TAG_STYLES`.
fn tag_style(tag: &str) -> u32 {
    match tag {
        "b" | "strong" => style::BOLD,
        "i" | "em" | "cite" | "dfn" | "var" => style::ITALIC,
        "code" | "kbd" | "samp" | "tt" => style::MONO,
        "sub" => style::SUB,
        "sup" => style::SUP,
        "u" | "ins" => style::UNDERLINE,
        _ => 0,
    }
}
const SHIFT: u32 = style::SUB | style::SUP;

const HTML_TAGS: &str = "a abbr address article aside audio b bdi bdo big blockquote body br button caption center cite code col colgroup data dd del details dfn dialog div dl dt em embed fieldset figcaption figure font footer form h1 h2 h3 h4 h5 h6 head header hr html i iframe img input ins kbd label legend li link main map mark meta meter nav noscript object ol optgroup option output p picture pre progress q rp rt ruby s samp script section select small source span strike strong style sub summary sup table tbody td template textarea tfoot th thead time title tr track tt u ul var video wbr";

/// Ports `HTML_TAGS.has`.
fn is_html_tag(tag: &str) -> bool {
    HTML_TAGS.split(' ').any(|t| t == tag)
}

/// `[${CJK}]`: Chinese and Japanese characters (Hangul is left out).
fn is_cjk(c: char) -> bool {
    let c = c as u32;
    (0x2e80..=0x9fff).contains(&c)
        || (0xf900..=0xfaff).contains(&c)
        || (0xff00..=0xff60).contains(&c)
}

/// `[${CLOSE}]`: what may not start a line.
fn is_close(c: char) -> bool {
    "、。，．：；！？）］｝」』】〕〉》〙〗ー…‥・ゝゞぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮ,.:;!?)]}".contains(c)
}

/// `[${OPEN}]`: what may not end a line.
fn is_open(c: char) -> bool {
    "（［｛「『【〔〈《〘〖([{".contains(c)
}

/// `[${HINT}]`: a zero-width space and a soft hyphen.
fn is_hint(c: char) -> bool {
    c == '\u{200b}' || c == '\u{ad}'
}

/// `RE_MARKUP.test(raw)`: /[<\\\n*_]|fa[bklrs]?:fa-/
fn has_markup(raw: &str) -> bool {
    raw.contains(['<', '\\', '\n', '*', '_']) || has_icon_prefix(raw)
}

/// The next match of /(fa[bklrs]?):fa-([\w-]+)/ at or after `from`, as
/// (start, end of the prefix, end).
fn find_icon(text: &str, from: usize) -> Option<(usize, usize, usize)> {
    let b = text.as_bytes();
    let mut i = from;
    while i + 1 < b.len() {
        if b[i] == b'f' && b[i + 1] == b'a' {
            let mut j = i + 2;
            if j < b.len() && matches!(b[j], b'b' | b'k' | b'l' | b'r' | b's') {
                j += 1;
            }
            let prefix = j;
            if text[j..].starts_with(":fa-") {
                let s = j + 4;
                let mut e = s;
                while e < b.len() && (b[e].is_ascii_alphanumeric() || b[e] == b'_' || b[e] == b'-')
                {
                    e += 1;
                }
                if e > s {
                    return Some((i, prefix, e));
                }
            }
        }
        i += 1;
    }
    None
}

/// `fa[bklrs]?:fa-` anywhere in `raw`, the icon half of `RE_MARKUP`.
fn has_icon_prefix(raw: &str) -> bool {
    let b = raw.as_bytes();
    for i in 0..b.len().saturating_sub(1) {
        if b[i] == b'f' && b[i + 1] == b'a' {
            if raw[i + 2..].starts_with(":fa-") {
                return true;
            }
            if i + 2 < b.len()
                && matches!(b[i + 2], b'b' | b'k' | b'l' | b'r' | b's')
                && raw[i + 3..].starts_with(":fa-")
            {
                return true;
            }
        }
    }
    false
}

/// Ports `dedent`.
fn dedent(text: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut min = usize::MAX;
    for line in &lines {
        if js::trim(line).is_empty() {
            continue;
        }
        let indent = leading_spaces(line);
        if indent < min {
            min = indent;
        }
    }
    if min == usize::MAX || min == 0 {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for (k, line) in lines.iter().enumerate() {
        if k > 0 {
            out.push('\n');
        }
        let cut = min.min(leading_spaces(line));
        let at = line
            .char_indices()
            .nth(cut)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        out.push_str(&line[at..]);
    }
    out
}

/// `line.length - line.trimStart().length`, in characters (whitespace is
/// all in the BMP, so this is UTF-16 units too).
fn leading_spaces(line: &str) -> usize {
    line.chars().take_while(|c| is_space(*c)).count()
}

/// Ports `pushText` (without math marks).
fn push_text(line: &mut Vec<Span>, text: &str, style: u32) {
    if text.is_empty() {
        return;
    }
    push_plain(line, text, style);
}

/// Ports `pushPlain`.
fn push_plain(line: &mut Vec<Span>, text: &str, style: u32) {
    let decoded = decode_entities(text);
    if let Some(last) = line.last_mut() {
        if last.icon.is_none() && last.style == style {
            last.text.push_str(&decoded);
            return;
        }
    }
    line.push(Span {
        text: decoded,
        style,
        icon: None,
        width: 0.0,
    });
}

/// Ports `pushIcons`.
fn push_icons(line: &mut Vec<Span>, text: &str, style: u32, icons: Icons) {
    if !text.contains(":fa-") {
        push_text(line, text, style);
        return;
    }
    let mut last = 0usize;
    let mut from = 0usize;
    while let Some((start, prefix, end)) = find_icon(text, from) {
        let icon = alloc::format!("{}:fa-{}", &text[start..prefix], &text[prefix + 4..end]);
        push_text(line, &text[last..start], style);
        last = end;
        from = end;
        if !icon_markup(icons, &icon).is_empty() {
            line.push(Span {
                text: String::new(),
                style,
                icon: Some(icon),
                width: 0.0,
            });
        } else if text[last..].starts_with(' ') {
            // An icon the host does not have leaves no gap, nor the space that set it off from the text.
            last += 1;
        } else if last == text.len() {
            if let Some(before) = line.last_mut() {
                if before.icon.is_none() {
                    let t = js::trim_end(&before.text).to_string();
                    before.text = t;
                }
            }
        }
    }
    push_text(line, &text[last..], style);
}

/// Ports `interface Open`.
struct Open {
    style: u32,
    /// Sub/sup stack; only the innermost tag shifts text.
    shifts: Vec<u32>,
}

/// Ports `RE_TAG` (sticky at `i`): /<\/?([a-zA-Z][\w-]*)(?:\s[^<>]*)?\/?>/y.
/// Returns the tag name's range and the end of the match.
fn match_tag(s: &str, i: usize) -> Option<(usize, usize, usize)> {
    let b = s.as_bytes();
    let mut j = i + 1;
    if b.get(j) == Some(&b'/') {
        j += 1;
    }
    if !b.get(j).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let name_start = j;
    j += 1;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || b[j] == b'-') {
        j += 1;
    }
    let name_end = j;
    // (?:\s[^<>]*)? then \/?>
    if let Some(c) = s[j..].chars().next() {
        if is_space(c) {
            let mut k = j + c.len_utf8();
            while k < b.len() && b[k] != b'<' && b[k] != b'>' {
                k += 1;
            }
            if k < b.len() && b[k] == b'>' {
                return Some((name_start, name_end, k + 1));
            }
        }
    }
    let mut k = j;
    if b.get(k) == Some(&b'/') {
        k += 1;
    }
    if b.get(k) == Some(&b'>') {
        return Some((name_start, name_end, k + 1));
    }
    None
}

/// Ports `inline`: splits inline HTML and, for markdown labels, emphasis
/// markers into styled spans.
fn inline(
    line: &mut Vec<Span>,
    source: &str,
    markdown: bool,
    base: u32,
    open: &mut Open,
    icons: Icons,
) {
    let mut style = open.style;
    let mut marked = 0u32;
    let mut text = String::new();
    let mut i = 0usize;
    let n = source.len();
    let b = source.as_bytes();
    let flush = |line: &mut Vec<Span>, text: &mut String, style: u32, open: &Open| {
        push_icons(
            line,
            text,
            style | open.shifts.last().copied().unwrap_or(0),
            icons,
        );
        text.clear();
    };
    while i < n {
        let ch = source[i..].chars().next().unwrap_or(' ');
        if ch == '<' {
            if let Some((ns, ne, end)) = match_tag(source, i) {
                let tag = source[ns..ne].to_ascii_lowercase();
                let closing = b.get(i + 1) == Some(&b'/');
                let bit = tag_style(&tag);
                if bit & SHIFT != 0 {
                    flush(line, &mut text, style, open);
                    let at = open.shifts.iter().rposition(|s| *s == bit);
                    if !closing {
                        open.shifts.push(bit);
                    } else if let Some(at) = at {
                        open.shifts.remove(at);
                    }
                } else if bit != 0 {
                    flush(line, &mut text, style, open);
                    open.style = if closing {
                        (open.style & !bit) | (base & bit)
                    } else {
                        open.style | bit
                    };
                    style = if closing {
                        (style & !bit) | (base & bit)
                    } else {
                        style | bit
                    };
                }
                // Other HTML tags are dropped and their text kept. Anything else shaped like a tag, such
                // as the type in List<T>, is text.
                if bit != 0 || is_html_tag(&tag) {
                    i = end;
                    continue;
                }
            }
        } else if markdown && (ch == '*' || ch == '_') {
            let double = b.get(i + 1) == Some(&(ch as u8));
            let bit = if double { style::BOLD } else { style::ITALIC };
            let len = if double { 2 } else { 1 };
            let opening = (marked & bit) == 0;
            let prev = if i > 0 {
                source[..i].chars().next_back().unwrap_or(' ')
            } else {
                ' '
            };
            let next = if i + len < n {
                source[i + len..].chars().next().unwrap_or(' ')
            } else {
                ' '
            };
            let pattern: &str = if double {
                if ch == '*' {
                    "**"
                } else {
                    "__"
                }
            } else if ch == '*' {
                "*"
            } else {
                "_"
            };
            let can_open = opening && !is_space(next) && source[i + len..].contains(pattern);
            let can_close = !opening && !is_space(prev);
            let wordy = ch == '_' && js::is_word(prev) && js::is_word(next);
            if (can_open || can_close) && !wordy {
                flush(line, &mut text, style, open);
                marked = if can_open {
                    marked | bit
                } else {
                    marked & !bit
                };
                style = if can_open {
                    style | bit
                } else {
                    (style & !bit) | (base & bit)
                };
                i += len;
                continue;
            }
        }
        text.push(ch);
        i += ch.len_utf8();
    }
    flush(line, &mut text, style, open);
}

/// Ports `measureSpans`.
fn measure_spans(line: &mut [Span], measurer: &dyn Measure, size: f64) -> f64 {
    let mut total = 0.0;
    for span in line.iter_mut() {
        span.width = if span.icon.is_some() {
            size * 1.25
        } else {
            measurer.width(&span.text, size, span.style)
        };
        total += span.width;
    }
    total
}

/// The end of the group in `RE_PIECE` that starts at `q`, if any:
/// `[OPEN]*[CJK][CLOSE]*|[^\s CJK HINT]+[HINT]*[CLOSE]*|[HINT]+`.
fn piece_group(s: &str, q: usize) -> Option<usize> {
    let chars: Vec<(usize, char)> = s[q..].char_indices().take_while(|_| true).collect();
    let at = |k: usize| chars.get(k).map(|(_, c)| *c);
    let pos = |k: usize| chars.get(k).map(|(i, _)| q + *i).unwrap_or(s.len());
    // [OPEN]*[CJK][CLOSE]*, backtracking the opens.
    let mut opens = 0;
    while at(opens).is_some_and(is_open) {
        opens += 1;
    }
    let mut j = opens as isize;
    while j >= 0 {
        if at(j as usize).is_some_and(is_cjk) {
            let mut k = j as usize + 1;
            while at(k).is_some_and(is_close) {
                k += 1;
            }
            return Some(pos(k));
        }
        j -= 1;
    }
    // [^\s CJK HINT]+[HINT]*[CLOSE]*
    let other = |c: char| !is_space(c) && !is_cjk(c) && !is_hint(c);
    let mut k = 0;
    while at(k).is_some_and(other) {
        k += 1;
    }
    if k > 0 {
        while at(k).is_some_and(is_hint) {
            k += 1;
        }
        while at(k).is_some_and(is_close) {
            k += 1;
        }
        return Some(pos(k));
    }
    // [HINT]+
    while at(k).is_some_and(is_hint) {
        k += 1;
    }
    if k > 0 {
        return Some(pos(k));
    }
    None
}

fn skip_spaces(s: &str, mut p: usize) -> usize {
    while let Some(c) = s[p..].chars().next() {
        if !is_space(c) {
            break;
        }
        p += c.len_utf8();
    }
    p
}

/// `text.match(RE_PIECE) ?? []`: the pieces a line may break between.
fn pieces(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < s.len() {
        let q = skip_spaces(s, p);
        if q < s.len() {
            if let Some(e) = piece_group(s, q) {
                let e2 = skip_spaces(s, e);
                out.push(&s[p..e2]);
                p = e2;
                continue;
            }
        }
        if q > p {
            out.push(&s[p..q]);
            p = q;
            continue;
        }
        // No match here: the regex moves on one character.
        p += s[p..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    out
}

/// `str.slice(0, -1)` for a string that ends in a BMP character.
fn drop_last(s: &str) -> &str {
    match s.char_indices().next_back() {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Ports `wrap`.
fn wrap(
    line: &[Span],
    measurer: &dyn Measure,
    size: f64,
    max: f64,
    out: &mut Vec<Vec<Span>>,
    widths: &mut Vec<f64>,
) {
    let mut current: Vec<Span> = Vec::new();
    let mut width = 0.0f64;
    let commit = |current: &mut Vec<Span>,
                  width: &mut f64,
                  out: &mut Vec<Vec<Span>>,
                  widths: &mut Vec<f64>,
                  hyphen: bool| {
        if let Some(last) = current.last_mut() {
            if last.icon.is_none() {
                let mut trimmed = js::trim_end(&last.text).to_string();
                if hyphen && last.text.ends_with('\u{ad}') {
                    trimmed = alloc::format!("{}-", drop_last(&trimmed));
                }
                if trimmed != last.text {
                    let w = measurer.width(&trimmed, size, last.style);
                    *width += w - last.width;
                    last.text = trimmed;
                    last.width = w;
                }
            }
        }
        out.push(core::mem::take(current));
        widths.push(*width);
        *width = 0.0;
    };
    let line_len = line.len();
    for (si, span) in line.iter().enumerate() {
        if span.icon.is_some() {
            let w = size * 1.25;
            if width > 0.0 && width + w > max {
                commit(&mut current, &mut width, out, widths, false);
            }
            let mut s = span.clone();
            s.width = w;
            current.push(s);
            width += w;
            continue;
        }
        let pieces = pieces(&span.text);
        // A piece ending in a soft hyphen fits if its '-' does, or if the rest of the word does, which
        // hides the hyphen. The line only grows along a word, so no later hyphen in it fits either,
        // and the remaining width of each word makes this one check.
        let soft = span.text.contains('\u{ad}');
        let n = if soft { pieces.len() } else { 0 };
        let mut own = alloc::vec![0.0f64; n];
        let mut rest = alloc::vec![0.0f64; n + 1];
        let mut q = n as isize - 1;
        while q >= 0 {
            let qu = q as usize;
            // A soft hyphen before a space is no break of its own: the space is.
            let shy = pieces[qu].ends_with('\u{ad}');
            let end = js::trim_end(pieces[qu]);
            own[qu] = measurer.width(if shy { drop_last(end) } else { end }, size, span.style);
            // Word width beyond this span is unknown.
            rest[qu] = if !shy {
                own[qu]
            } else if qu == n - 1 {
                if si == line_len - 1 {
                    own[qu]
                } else {
                    f64::INFINITY
                }
            } else {
                own[qu] + rest[qu + 1]
            };
            q -= 1;
        }
        for (p, piece) in pieces.iter().enumerate() {
            let mut part: &str = piece;
            let part_width = if soft {
                own[p]
            } else {
                measurer.width(js::trim_end(part), size, span.style)
            };
            let fits = if !soft || !part.ends_with('\u{ad}') {
                width + part_width <= max
            } else {
                let hyphened = alloc::format!("{}-", drop_last(part));
                width + measurer.width(&hyphened, size, span.style) <= max || width + rest[p] <= max
            };
            if width > 0.0 && !fits {
                // Breaking at whitespace leaves a preceding soft hyphen hidden.
                let hyphen = js::trim_start(part) == part;
                commit(&mut current, &mut width, out, widths, hyphen);
                part = js::trim_start(part);
            }
            let merge =
                matches!(current.last(), Some(l) if l.icon.is_none() && l.style == span.style);
            if merge {
                let last = current.last_mut().unwrap_or_else(|| unreachable!());
                last.text.push_str(part);
                // Remeasuring for each zero-width piece would take quadratic time.
                if part_width > 0.0 || js::trim_end(part) != part {
                    let before = last.width;
                    last.width = measurer.width(&last.text, size, last.style);
                    width += last.width - before;
                }
            } else {
                let w = measurer.width(part, size, span.style);
                current.push(Span {
                    text: part.to_string(),
                    style: span.style,
                    icon: None,
                    width: w,
                });
                width += w;
            }
        }
    }
    if !current.is_empty() || out.is_empty() {
        commit(&mut current, &mut width, out, widths, false);
    }
}

/// Ports `layoutLabel`: lays out a label's text in lines no wider than
/// `max_width` where it can break, in the `base` style flags.
pub fn layout_label(
    raw: Option<&str>,
    markdown: bool,
    measurer: &dyn Measure,
    size: f64,
    max_width: f64,
    base: u32,
    icons: Icons,
) -> Label {
    let raw = match raw {
        None | Some("") => return empty_label(),
        Some(r) => r,
    };
    let line_height = jround(size * LINE_HEIGHT);

    if !has_markup(raw) && !raw.contains('&') && !raw.contains('\u{fb02}') {
        let text = js::trim(raw);
        let width = measurer.width(text, size, base);
        if width <= max_width
            || !(text.contains(' ') || text.chars().any(is_cjk) || text.chars().any(is_hint))
        {
            return Label {
                lines: alloc::vec![alloc::vec![Span {
                    text: text.to_string(),
                    style: base,
                    icon: None,
                    width,
                }]],
                widths: alloc::vec![width],
                width,
                height: line_height,
                size,
                line_height,
                heights: None,
            };
        }
        let mut lines = Vec::new();
        let mut widths = Vec::new();
        let one = [Span {
            text: text.to_string(),
            style: base,
            icon: None,
            width,
        }];
        wrap(&one, measurer, size, max_width, &mut lines, &mut widths);
        let mut widest = 0.0;
        for &w in &widths {
            if w > widest {
                widest = w;
            }
        }
        let snug = if lines.len() > 1 {
            jround(size * WRAPPED_LINE_HEIGHT)
        } else {
            line_height
        };
        let height = lines.len() as f64 * snug;
        return Label {
            lines,
            widths,
            width: widest,
            height,
            size,
            line_height: snug,
            heights: None,
        };
    }
    layout_lines(
        raw,
        markdown,
        measurer,
        size,
        max_width,
        base,
        line_height,
        icons,
    )
}

/// `source.split(RE_BREAK)`: RE_BREAK = /<br\s*\/?>|\\n|\n/gi.
fn split_breaks(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\n' {
            out.push(&s[last..i]);
            i += 1;
            last = i;
            continue;
        }
        if b[i] == b'\\' && b.get(i + 1) == Some(&b'n') {
            out.push(&s[last..i]);
            i += 2;
            last = i;
            continue;
        }
        if b[i] == b'<'
            && i + 2 < b.len()
            && b[i + 1].eq_ignore_ascii_case(&b'b')
            && b[i + 2].eq_ignore_ascii_case(&b'r')
        {
            let j = skip_spaces(s, i + 3);
            let mut k = j;
            if b.get(k) == Some(&b'/') {
                k += 1;
            }
            if b.get(k) == Some(&b'>') {
                out.push(&s[last..i]);
                i = k + 1;
                last = i;
                continue;
            }
        }
        i += 1;
    }
    out.push(&s[last..]);
    out
}

/// Ports `layoutLines`.
#[allow(clippy::too_many_arguments)]
fn layout_lines(
    raw: &str,
    markdown: bool,
    measurer: &dyn Measure,
    size: f64,
    max_width: f64,
    base: u32,
    line_height: f64,
    icons: Icons,
) -> Label {
    let source = if markdown {
        let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
        js::trim(&dedent(&unified)).to_string()
    } else {
        js::trim(raw).to_string()
    };
    let mut lines: Vec<Vec<Span>> = Vec::new();
    let mut widths: Vec<f64> = Vec::new();
    let mut width = 0.0f64;
    // HTML tags can span explicit breaks; Markdown emphasis stays local to each line.
    let mut open = Open {
        style: base,
        shifts: Vec::new(),
    };
    for part in split_breaks(&source) {
        let mut line: Vec<Span> = Vec::new();
        inline(&mut line, js::trim(part), markdown, base, &mut open, icons);
        let w = measure_spans(&mut line, measurer, size);
        if w > max_width {
            wrap(&line, measurer, size, max_width, &mut lines, &mut widths);
        } else {
            lines.push(line);
            widths.push(w);
        }
    }
    for &w in &widths {
        if w > width {
            width = w;
        }
    }
    let snug = if lines.len() > 1 {
        jround(size * WRAPPED_LINE_HEIGHT)
    } else {
        line_height
    };
    let height = lines.len() as f64 * snug;
    Label {
        lines,
        widths,
        width,
        height,
        size,
        line_height: snug,
        heights: None,
    }
}

/// Ports `lineMiddle`: how far down the label the middle of line `i` is.
pub fn line_middle(label: &Label, i: usize) -> f64 {
    match &label.heights {
        None => (i as f64 + 0.5) * label.line_height,
        Some(heights) => {
            let mut top = 0.0;
            for h in &heights[..i] {
                top += h;
            }
            top + heights[i] / 2.0
        }
    }
}

/// Ports `labelLine`: line `i` of a label as a label of its own.
pub fn label_line(label: &Label, i: usize) -> Label {
    let width = label.widths[i];
    let height = match &label.heights {
        Some(h) => h[i],
        None => label.line_height,
    };
    Label {
        lines: alloc::vec![label.lines[i].clone()],
        widths: alloc::vec![width],
        width,
        height,
        size: label.size,
        line_height: label.line_height,
        heights: if height != label.line_height {
            Some(alloc::vec![height])
        } else {
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pele::text::measurer::METRICS;

    #[test]
    fn pieces_split_as_the_regex_does() {
        assert_eq!(pieces("a long  label"), ["a ", "long  ", "label"]);
        assert_eq!(pieces("中文。字"), ["中", "文。", "字"]);
        assert_eq!(pieces(" x"), [" x"]);
        assert_eq!(pieces("「中」"), ["「中」"]);
    }

    #[test]
    fn tags_and_breaks() {
        assert_eq!(
            split_breaks("a<br>b<BR />c\\nd\ne"),
            ["a", "b", "c", "d", "e"]
        );
        assert_eq!(match_tag("<b>x", 0), Some((1, 2, 3)));
        assert_eq!(match_tag("<span class='a'>", 0), Some((1, 5, 16)));
        assert_eq!(match_tag("<T", 0), None);
        let l = layout_label(
            Some("<b>bold</b> plain"),
            false,
            &METRICS,
            16.0,
            200.0,
            0,
            None,
        );
        assert_eq!(l.lines[0].len(), 2);
        assert_eq!(l.lines[0][0].style, style::BOLD);
        assert!(!has_icon_prefix("plain"));
    }
}
