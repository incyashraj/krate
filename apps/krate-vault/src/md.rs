//! Reading Markdown one line at a time, the way a live-preview editor needs.
//!
//! Nothing here builds a document tree. A note can be 200,000 lines, and a
//! keystroke must cost the same in it as in a short one, so every question
//! is answered for one line: what kind of block it is ([`classify`]) and how
//! its text is styled ([`inline`]). The one fact that carries from line to
//! line -- whether a line sits inside a fenced code block -- is kept by
//! [`Fences`], which an edit updates in place instead of rescanning.

use alloc::string::String;
use alloc::vec::Vec;

/// What a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Blank,
    Para,
    /// `#`..`######`; `content` is where the heading text starts.
    Heading {
        level: u8,
        content: usize,
    },
    /// `- `, `* `, `+ ` at `indent` columns; content after the marker.
    Bullet {
        indent: usize,
        marker: usize,
        content: usize,
    },
    /// `1. ` or `1) `.
    Ordered {
        indent: usize,
        marker: usize,
        content: usize,
    },
    /// `- [ ] ` / `- [x] `; `box_at` is the byte of the `[`.
    Task {
        indent: usize,
        marker: usize,
        box_at: usize,
        checked: bool,
        content: usize,
    },
    /// `> `.
    Quote {
        content: usize,
    },
    /// A fence line: ``` or ~~~, opening or closing.
    Fence,
    /// A line inside a fenced block.
    Code,
    /// `---`, `***` or `___` alone.
    Rule,
    /// A table row (`| a | b |`).
    Table,
}

impl Block {
    /// Where the line's own text begins after its marker.
    pub fn content(&self) -> usize {
        match *self {
            Block::Heading { content, .. }
            | Block::Bullet { content, .. }
            | Block::Ordered { content, .. }
            | Block::Task { content, .. }
            | Block::Quote { content } => content,
            _ => 0,
        }
    }

    /// The list depth in indentation levels (two spaces or one tab each).
    pub fn level(&self) -> usize {
        match *self {
            Block::Bullet { indent, .. }
            | Block::Ordered { indent, .. }
            | Block::Task { indent, .. } => indent,
            _ => 0,
        }
    }

    pub fn is_list(&self) -> bool {
        matches!(
            self,
            Block::Bullet { .. } | Block::Ordered { .. } | Block::Task { .. }
        )
    }
}

/// Whether a line opens or closes a fence: up to three spaces, then three
/// or more backticks or tildes.
pub fn is_fence(line: &str) -> bool {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return false;
    }
    t.starts_with("```") || t.starts_with("~~~")
}

/// The language named after an opening fence (```rust), if any.
pub fn fence_lang(line: &str) -> &str {
    line.trim_start()
        .trim_start_matches(['`', '~'])
        .split_whitespace()
        .next()
        .unwrap_or("")
}

/// Indentation in levels: a tab is one, two spaces are one.
fn indent_levels(ws: &str) -> usize {
    let mut cols = 0usize;
    for b in ws.bytes() {
        if b == b'\t' {
            cols += 2;
        } else {
            cols += 1;
        }
    }
    cols / 2
}

/// Classify one line. `in_code` says whether it sits inside a fence (the
/// fence lines themselves are [`Block::Fence`] either way).
pub fn classify(line: &str, in_code: bool) -> Block {
    if is_fence(line) {
        return Block::Fence;
    }
    if in_code {
        return Block::Code;
    }
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return Block::Blank;
    }
    let lead = line.len() - trimmed.len();
    let b = trimmed.as_bytes();
    let first = b.first().copied().unwrap_or(0);
    // Headings: 1-6 hashes then a space (or the end).
    if first == b'#' && lead <= 3 {
        let hashes = b.iter().take_while(|c| **c == b'#').count();
        if (1..=6).contains(&hashes) {
            match b.get(hashes) {
                None => {
                    return Block::Heading {
                        level: hashes as u8,
                        content: line.len(),
                    }
                }
                Some(b' ') | Some(b'\t') => {
                    let mut content = lead + hashes + 1;
                    while line.as_bytes().get(content) == Some(&b' ') {
                        content += 1;
                    }
                    return Block::Heading {
                        level: hashes as u8,
                        content,
                    };
                }
                _ => {}
            }
        }
    }
    // A thematic break.
    if lead <= 3 && matches!(first, b'-' | b'*' | b'_') {
        let mut n = 0;
        let mut only = true;
        for c in b {
            if *c == first {
                n += 1;
            } else if *c != b' ' {
                only = false;
                break;
            }
        }
        if only && n >= 3 {
            return Block::Rule;
        }
    }
    let indent = indent_levels(line.get(..lead).unwrap_or(""));
    // Bullets and tasks.
    if matches!(first, b'-' | b'*' | b'+') && matches!(b.get(1), Some(b' ') | None) {
        let marker = lead;
        let after = lead + 2;
        let rest = line.get(after.min(line.len())..).unwrap_or("");
        let rb = rest.as_bytes();
        if rb.len() >= 3 && rb.first() == Some(&b'[') && rb.get(2) == Some(&b']') {
            let mark = rb.get(1).copied().unwrap_or(b' ');
            if matches!(rb.get(3), Some(b' ') | None) {
                let content = (after + 4).min(line.len());
                return Block::Task {
                    indent,
                    marker,
                    box_at: after,
                    checked: mark != b' ',
                    content,
                };
            }
        }
        return Block::Bullet {
            indent,
            marker,
            content: after.min(line.len()),
        };
    }
    // Ordered items: digits then `.` or `)` then a space.
    if first.is_ascii_digit() {
        let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
        if digits <= 9
            && matches!(b.get(digits), Some(b'.') | Some(b')'))
            && matches!(b.get(digits + 1), Some(b' ') | None)
        {
            return Block::Ordered {
                indent,
                marker: lead,
                content: (lead + digits + 2).min(line.len()),
            };
        }
    }
    if first == b'>' {
        let mut content = lead + 1;
        if line.as_bytes().get(content) == Some(&b' ') {
            content += 1;
        }
        return Block::Quote { content };
    }
    if first == b'|' {
        return Block::Table;
    }
    Block::Para
}

/// How a run of a line's text looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub mark: bool,
    pub code: bool,
    /// Part of a link: the text a person clicks.
    pub link: bool,
    pub tag: bool,
    /// Markdown syntax (`**`, `[[`, the `](url)` of a link): shown on the line
    /// the caret is on, hidden everywhere else in live preview.
    pub syntax: bool,
    /// Inline TeX between `$` signs (the source; drawn as math).
    pub math: bool,
    /// `<sup>` and `<sub>`.
    pub sup: bool,
    pub sub: bool,
    /// `<u>`.
    pub underline: bool,
    /// `<kbd>`: a key cap.
    pub kbd: bool,
    /// A footnote reference `[^1]`, an inline footnote `^[...]`, or a
    /// definition's label `[^1]:`.
    pub footnote: bool,
    /// `%%comment%%`: dimmed while editing, gone in reading view.
    pub comment: bool,
    /// A block id `^id` at the end of a line: dimmed, gone in reading view.
    pub blockid: bool,
}

/// What a link points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `[[name]]`, `[[name#heading]]`, `[[name|alias]]`: the byte range of
    /// `name` in the line.
    Wiki(usize, usize),
    /// `[text](url)` or a bare `https://` address: the url's range.
    Url(usize, usize),
    /// `#tag`: the tag's range, `#` included.
    Tag(usize, usize),
    /// `[^id]` or `^[text]`: the label's or the text's range.
    Footnote(usize, usize),
}

/// One styled stretch of a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seg {
    pub start: usize,
    pub end: usize,
    pub style: Style,
    pub target: Option<Target>,
}

fn push(out: &mut Vec<Seg>, start: usize, end: usize, style: Style, target: Option<Target>) {
    if end <= start {
        return;
    }
    if let Some(last) = out.last_mut() {
        if last.end == start && last.style == style && last.target == target {
            last.end = end;
            return;
        }
    }
    out.push(Seg {
        start,
        end,
        style,
        target,
    });
}

fn find_from(s: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    if pat.is_empty() || from >= s.len() {
        return None;
    }
    let mut i = from;
    while i + pat.len() <= s.len() {
        if s.get(i..i + pat.len()) == Some(pat) {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/')
}

/// A closing emphasis delimiter for `delim` after `from`: preceded by a
/// non-space, and for `_` not followed by a word character.
fn closing(s: &[u8], from: usize, delim: &[u8]) -> Option<usize> {
    let mut i = from;
    while let Some(at) = find_from(s, i, delim) {
        let before = if at == 0 {
            b' '
        } else {
            s.get(at - 1).copied().unwrap_or(b' ')
        };
        let after = s.get(at + delim.len()).copied().unwrap_or(b' ');
        let ok_before = before != b' ' && before != b'\t';
        let ok_after = delim.first() != Some(&b'_') || !after.is_ascii_alphanumeric();
        // `**` must not be read as a closing `*`.
        let single_star = delim == b"*" && (after == b'*' || before == b'*');
        if ok_before && ok_after && !single_star && at > from {
            return Some(at);
        }
        i = at + 1;
    }
    None
}

/// The inline HTML tags a note may use that change how text looks, and the
/// flag each sets. `br` and `span` are recognised and hidden but style
/// nothing.
fn html_tag(name: &[u8]) -> Option<u8> {
    Some(match name {
        b"u" | b"ins" => 1,
        b"mark" => 2,
        b"sub" => 3,
        b"sup" => 4,
        b"kbd" => 5,
        b"b" | b"strong" => 6,
        b"i" | b"em" => 7,
        b"s" | b"del" | b"strike" => 8,
        b"span" | b"font" | b"small" | b"big" | b"abbr" | b"cite" | b"q" => 0,
        _ => return None,
    })
}

fn set_html(st: &mut Style, kind: u8, on: bool) {
    match kind {
        1 => st.underline = on,
        2 => st.mark = on,
        3 => st.sub = on,
        4 => st.sup = on,
        5 => st.kbd = on,
        6 => st.bold = on,
        7 => st.italic = on,
        8 => st.strike = on,
        _ => {}
    }
}

/// An opening tag `<name ...>` at `i`: its name and where it ends.
fn open_tag(s: &[u8], i: usize) -> Option<(&[u8], usize)> {
    let rest = s.get(i + 1..)?;
    let n = rest.iter().take_while(|b| b.is_ascii_alphabetic()).count();
    if n == 0 {
        return None;
    }
    let name = rest.get(..n)?;
    let close = rest.iter().position(|b| *b == b'>')?;
    if close < n
        || rest
            .get(n)
            .is_some_and(|b| !matches!(*b, b'>' | b' ' | b'/'))
    {
        return None;
    }
    Some((name, i + 1 + close + 1))
}

/// Whether `s[i]` starts a block id: `^` after a space, then letters,
/// digits and dashes to the end of the line.
fn block_id_at(s: &[u8], i: usize) -> bool {
    if s.get(i) != Some(&b'^') || (i > 0 && s.get(i - 1) != Some(&b' ')) {
        return false;
    }
    let rest = s.get(i + 1..).unwrap_or(&[]);
    let trimmed = rest
        .iter()
        .rposition(|b| *b != b' ')
        .map_or(&[][..], |e| rest.get(..=e).unwrap_or(&[]));
    !trimmed.is_empty()
        && trimmed
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
}

/// The closing `$` of inline math opened at `open` (the byte after it is
/// where the TeX starts): not after a space or a backslash, and not before
/// a digit, so `$5 and $6` stays text.
fn math_close(s: &[u8], from: usize) -> Option<usize> {
    let first = *s.get(from)?;
    if first == b' ' || first == b'$' {
        return None;
    }
    let mut j = from + 1;
    while j < s.len() {
        let c = s.get(j).copied().unwrap_or(0);
        if c == b'\\' {
            j += 2;
            continue;
        }
        if c == b'$' {
            let before = s.get(j - 1).copied().unwrap_or(b' ');
            let after = s.get(j + 1).copied().unwrap_or(b' ');
            if before != b' ' && before != b'$' && after != b'$' && !after.is_ascii_digit() {
                return Some(j);
            }
        }
        j += 1;
    }
    None
}

/// Style the text of a line from byte `from` (past its block marker).
/// Every byte from `from` to the end lands in exactly one segment, in order.
pub fn inline(line: &str, from: usize) -> Vec<Seg> {
    let s = line.as_bytes();
    let mut out: Vec<Seg> = Vec::new();
    let mut st = Style::default();
    // Open emphasis runs: where each one closes.
    let mut bold_end: Option<usize> = None;
    let mut ital_end: Option<usize> = None;
    let mut strike_end: Option<usize> = None;
    let mut mark_end: Option<usize> = None;
    // Open HTML tags: where each closing tag starts and ends, and its kind.
    let mut html_open: Vec<(usize, usize, u8)> = Vec::new();
    let mut i = from.min(s.len());
    let mut text_start = i;
    let syntax = |st: Style| Style { syntax: true, ..st };
    macro_rules! flush {
        ($to:expr) => {
            push(&mut out, text_start, $to, st, None);
        };
    }
    while i < s.len() {
        let c = s.get(i).copied().unwrap_or(0);
        let prev = if i == 0 {
            b' '
        } else {
            s.get(i - 1).copied().unwrap_or(b' ')
        };
        // Closing delimiters of open runs.
        if bold_end == Some(i) {
            flush!(i);
            push(&mut out, i, i + 2, syntax(st), None);
            st.bold = false;
            bold_end = None;
            i += 2;
            text_start = i;
            continue;
        }
        if strike_end == Some(i) {
            flush!(i);
            push(&mut out, i, i + 2, syntax(st), None);
            st.strike = false;
            strike_end = None;
            i += 2;
            text_start = i;
            continue;
        }
        if mark_end == Some(i) {
            flush!(i);
            push(&mut out, i, i + 2, syntax(st), None);
            st.mark = false;
            mark_end = None;
            i += 2;
            text_start = i;
            continue;
        }
        if ital_end == Some(i) {
            flush!(i);
            push(&mut out, i, i + 1, syntax(st), None);
            st.italic = false;
            ital_end = None;
            i += 1;
            text_start = i;
            continue;
        }
        if let Some(k) = html_open.iter().position(|(at, _, _)| *at == i) {
            let (_, end, kind) = html_open.remove(k);
            flush!(i);
            push(&mut out, i, end, syntax(st), None);
            set_html(&mut st, kind, false);
            i = end;
            text_start = i;
            continue;
        }
        match c {
            b'$' => {
                let double = s.get(i + 1) == Some(&b'$');
                let open = if double { 2 } else { 1 };
                let close = if double {
                    find_from(s, i + 2, b"$$").filter(|c| *c > i + 2)
                } else {
                    math_close(s, i + 1)
                };
                if let Some(close) = close {
                    flush!(i);
                    let m = Style {
                        math: true,
                        ..Style::default()
                    };
                    push(&mut out, i, i + open, syntax(m), None);
                    push(&mut out, i + open, close, m, None);
                    push(&mut out, close, close + open, syntax(m), None);
                    i = close + open;
                    text_start = i;
                    continue;
                }
            }
            b'%' if s.get(i + 1) == Some(&b'%') => {
                let cm = Style {
                    comment: true,
                    ..Style::default()
                };
                flush!(i);
                match find_from(s, i + 2, b"%%") {
                    Some(close) => {
                        push(&mut out, i, i + 2, syntax(cm), None);
                        push(&mut out, i + 2, close, cm, None);
                        push(&mut out, close, close + 2, syntax(cm), None);
                        i = close + 2;
                    }
                    None => {
                        // A comment that runs past the end of the line.
                        push(&mut out, i, i + 2, syntax(cm), None);
                        push(&mut out, i + 2, s.len(), cm, None);
                        i = s.len();
                    }
                }
                text_start = i;
                continue;
            }
            b'[' if s.get(i + 1) == Some(&b'^') => {
                if let Some(close) = find_from(s, i + 2, b"]") {
                    let label = s.get(i + 2..close).unwrap_or(b"");
                    if !label.is_empty() && !label.contains(&b' ') && !label.contains(&b'[') {
                        flush!(i);
                        let def = i == 0 && s.get(close + 1) == Some(&b':');
                        let f = Style {
                            footnote: true,
                            ..st
                        };
                        let target = Some(Target::Footnote(i + 2, close));
                        let end = if def { close + 2 } else { close + 1 };
                        push(&mut out, i, i + 2, syntax(f), target);
                        push(&mut out, i + 2, close, f, target);
                        push(&mut out, close, end, syntax(f), target);
                        i = end;
                        text_start = i;
                        continue;
                    }
                }
            }
            b'^' if s.get(i + 1) == Some(&b'[') => {
                // An inline footnote: `^[text]`, brackets may nest.
                let mut depth = 0usize;
                let mut j = i + 1;
                let mut close = None;
                while j < s.len() {
                    match s.get(j) {
                        Some(b'[') => depth += 1,
                        Some(b']') => {
                            depth -= 1;
                            if depth == 0 {
                                close = Some(j);
                                break;
                            }
                        }
                        _ => {}
                    }
                    j += 1;
                }
                if let Some(close) = close.filter(|c| *c > i + 2) {
                    flush!(i);
                    let f = Style {
                        footnote: true,
                        ..st
                    };
                    let target = Some(Target::Footnote(i + 2, close));
                    push(&mut out, i, i + 2, syntax(f), target);
                    push(&mut out, i + 2, close, f, target);
                    push(&mut out, close, close + 1, syntax(f), target);
                    i = close + 1;
                    text_start = i;
                    continue;
                }
            }
            b'^' if block_id_at(s, i) => {
                flush!(i);
                push(
                    &mut out,
                    i,
                    s.len(),
                    Style {
                        blockid: true,
                        ..Style::default()
                    },
                    None,
                );
                i = s.len();
                text_start = i;
                continue;
            }
            b'<' => {
                if let Some((name, end)) = open_tag(s, i) {
                    let lname: Vec<u8> = name.iter().map(u8::to_ascii_lowercase).collect();
                    if lname == b"br" {
                        flush!(i);
                        push(&mut out, i, end, syntax(st), None);
                        i = end;
                        text_start = i;
                        continue;
                    }
                    if let Some(kind) = html_tag(&lname) {
                        let mut closer: Vec<u8> = Vec::from(&b"</"[..]);
                        closer.extend_from_slice(name);
                        closer.push(b'>');
                        if let Some(at) = find_from(s, end, &closer) {
                            flush!(i);
                            push(&mut out, i, end, syntax(st), None);
                            set_html(&mut st, kind, true);
                            html_open.push((at, at + closer.len(), kind));
                            i = end;
                            text_start = i;
                            continue;
                        }
                    }
                }
            }
            b'\\' if i + 1 < s.len() => {
                // An escaped character is plain text; the backslash is syntax.
                flush!(i);
                push(&mut out, i, i + 1, syntax(st), None);
                let next = line
                    .get(i + 1..)
                    .and_then(|r| r.chars().next())
                    .map_or(1, char::len_utf8);
                push(&mut out, i + 1, i + 1 + next, st, None);
                i += 1 + next;
                text_start = i;
                continue;
            }
            b'`' => {
                let ticks = s
                    .get(i..)
                    .map_or(0, |r| r.iter().take_while(|b| **b == b'`').count());
                let pat = s.get(i..i + ticks).unwrap_or(b"`");
                if let Some(close) = find_from(s, i + ticks, pat) {
                    flush!(i);
                    let code = Style {
                        code: true,
                        ..Style::default()
                    };
                    push(&mut out, i, i + ticks, syntax(code), None);
                    push(&mut out, i + ticks, close, code, None);
                    push(&mut out, close, close + ticks, syntax(code), None);
                    i = close + ticks;
                    text_start = i;
                    continue;
                }
            }
            b'!' if s.get(i + 1) == Some(&b'[') && s.get(i + 2) == Some(&b'[') => {
                // An embed `![[x]]` reads as a link with its `!` as syntax.
                if let Some(close) = find_from(s, i + 3, b"]]") {
                    flush!(i);
                    push(&mut out, i, i + 1, syntax(st), None);
                    i += 1;
                    text_start = i;
                    let _ = close;
                    continue;
                }
            }
            b'[' if s.get(i + 1) == Some(&b'[') => {
                if let Some(close) = find_from(s, i + 2, b"]]") {
                    let inner_start = i + 2;
                    let inner = s.get(inner_start..close).unwrap_or(b"");
                    if !inner.is_empty() && !inner.contains(&b'[') {
                        flush!(i);
                        let pipe = inner
                            .iter()
                            .position(|b| *b == b'|')
                            .map(|p| inner_start + p);
                        let name_end = inner
                            .iter()
                            .position(|b| *b == b'|' || *b == b'#')
                            .map_or(close, |p| inner_start + p);
                        let target = Some(Target::Wiki(inner_start, name_end));
                        let link = Style { link: true, ..st };
                        push(&mut out, i, inner_start, syntax(link), target);
                        match pipe {
                            Some(p) => {
                                push(&mut out, inner_start, p + 1, syntax(link), target);
                                push(&mut out, p + 1, close, link, target);
                            }
                            None => push(&mut out, inner_start, close, link, target),
                        }
                        push(&mut out, close, close + 2, syntax(link), target);
                        i = close + 2;
                        text_start = i;
                        continue;
                    }
                }
            }
            b'[' => {
                // [text](url)
                if let Some(mid) = find_from(s, i + 1, b"](") {
                    if let Some(close) = find_from(s, mid + 2, b")") {
                        let text_ok = !s.get(i + 1..mid).unwrap_or(b"").contains(&b'[');
                        if text_ok && mid > i + 1 {
                            flush!(i);
                            let target = Some(Target::Url(mid + 2, close));
                            let link = Style { link: true, ..st };
                            push(&mut out, i, i + 1, syntax(link), target);
                            push(&mut out, i + 1, mid, link, target);
                            push(&mut out, mid, close + 1, syntax(link), target);
                            i = close + 1;
                            text_start = i;
                            continue;
                        }
                    }
                }
            }
            b'h' if (prev == b' ' || i == from)
                && (s.get(i..i + 8) == Some(b"https://")
                    || s.get(i..i + 7) == Some(b"http://")) =>
            {
                let end = s.get(i..).map_or(i, |r| {
                    i + r.iter().take_while(|b| !b.is_ascii_whitespace()).count()
                });
                let end = {
                    // A sentence's full stop or a closing bracket is not the url's.
                    let mut e = end;
                    while e > i && matches!(s.get(e - 1), Some(b'.' | b',' | b')' | b';')) {
                        e -= 1;
                    }
                    e
                };
                flush!(i);
                push(
                    &mut out,
                    i,
                    end,
                    Style { link: true, ..st },
                    Some(Target::Url(i, end)),
                );
                i = end;
                text_start = i;
                continue;
            }
            b'#' if prev == b' ' || prev == b'\t' || i == from => {
                let rest = line.get(i + 1..).unwrap_or("");
                let len: usize = rest
                    .chars()
                    .take_while(|c| is_tag_char(*c))
                    .map(char::len_utf8)
                    .sum();
                let all_digits = rest
                    .get(..len)
                    .is_some_and(|t| t.bytes().all(|b| b.is_ascii_digit()));
                if len > 0 && !all_digits {
                    flush!(i);
                    push(
                        &mut out,
                        i,
                        i + 1 + len,
                        Style { tag: true, ..st },
                        Some(Target::Tag(i, i + 1 + len)),
                    );
                    i += 1 + len;
                    text_start = i;
                    continue;
                }
            }
            b'*' | b'_' => {
                let double = s.get(i + 1) == Some(&c);
                let next = s
                    .get(i + if double { 2 } else { 1 })
                    .copied()
                    .unwrap_or(b' ');
                let opens =
                    next != b' ' && next != b'\t' && (c == b'*' || !prev.is_ascii_alphanumeric());
                if double && !st.bold && opens {
                    let pat: &[u8] = if c == b'*' { b"**" } else { b"__" };
                    if let Some(close) = closing(s, i + 2, pat) {
                        flush!(i);
                        push(&mut out, i, i + 2, syntax(st), None);
                        st.bold = true;
                        bold_end = Some(close);
                        i += 2;
                        text_start = i;
                        continue;
                    }
                }
                if !double && !st.italic && opens {
                    let pat: &[u8] = if c == b'*' { b"*" } else { b"_" };
                    if let Some(close) = closing(s, i + 1, pat) {
                        if bold_end.is_none_or(|b| close < b) {
                            flush!(i);
                            push(&mut out, i, i + 1, syntax(st), None);
                            st.italic = true;
                            ital_end = Some(close);
                            i += 1;
                            text_start = i;
                            continue;
                        }
                    }
                }
            }
            b'~' if s.get(i + 1) == Some(&b'~') && !st.strike => {
                if let Some(close) = closing(s, i + 2, b"~~") {
                    flush!(i);
                    push(&mut out, i, i + 2, syntax(st), None);
                    st.strike = true;
                    strike_end = Some(close);
                    i += 2;
                    text_start = i;
                    continue;
                }
            }
            b'=' if s.get(i + 1) == Some(&b'=') && !st.mark => {
                if let Some(close) = closing(s, i + 2, b"==") {
                    flush!(i);
                    push(&mut out, i, i + 2, syntax(st), None);
                    st.mark = true;
                    mark_end = Some(close);
                    i += 2;
                    text_start = i;
                    continue;
                }
            }
            _ => {}
        }
        // An ordinary character: step over all of it.
        let len = line
            .get(i..)
            .and_then(|r| r.chars().next())
            .map_or(1, char::len_utf8);
        i += len;
    }
    flush!(s.len());
    out
}

/// The links, embeds and tags a line holds, for the vault's index: wiki
/// link targets (the note name, before any `#` or `|`) and tags (without
/// the `#`). Code spans are skipped.
pub fn refs(line: &str, mut on_link: impl FnMut(&str), mut on_tag: impl FnMut(&str)) {
    let b = line.as_bytes();
    // Fast path: most lines hold neither.
    if !b.contains(&b'[') && !b.contains(&b'#') {
        return;
    }
    for seg in inline(line, 0) {
        match seg.target {
            // Report each link once: on the segment that opens it.
            Some(Target::Wiki(a, z)) if seg.style.syntax && seg.start + 2 == a => {
                if let Some(name) = line.get(a..z) {
                    on_link(name.trim());
                }
            }
            Some(Target::Tag(a, z)) => {
                if let Some(t) = line.get(a + 1..z) {
                    on_tag(t);
                }
            }
            _ => {}
        }
    }
}

/// The fence lines of a note, in order, kept as the note is edited.
///
/// Line `n` is inside a code block when an odd number of fence lines come
/// before it, so a binary search answers it; an edit touches only the lines
/// it changed and shifts the numbers after them.
#[derive(Debug, Clone, Default)]
pub struct Fences {
    lines: Vec<usize>,
}

impl Fences {
    pub fn build<'a>(lines: impl Iterator<Item = &'a str>) -> Self {
        let mut out = Vec::new();
        for (i, l) in lines.enumerate() {
            if is_fence(l) {
                out.push(i);
            }
        }
        Fences { lines: out }
    }

    /// How many fence lines come before line `n`.
    fn before(&self, n: usize) -> usize {
        self.lines.partition_point(|l| *l < n)
    }

    pub fn is_fence(&self, n: usize) -> bool {
        self.lines.binary_search(&n).is_ok()
    }

    /// Whether line `n` is inside a fenced block (fence lines are not).
    pub fn in_code(&self, n: usize) -> bool {
        !self.is_fence(n) && self.before(n) % 2 == 1
    }

    /// The fence line that opens the code block line `n` is in.
    pub fn opener(&self, n: usize) -> Option<usize> {
        if !self.in_code(n) {
            return None;
        }
        let k = self.before(n);
        self.lines.get(k.checked_sub(1)?).copied()
    }

    /// Whether fence line `n` opens a block (an even number before it).
    pub fn opens(&self, n: usize) -> bool {
        self.is_fence(n) && self.before(n).is_multiple_of(2)
    }

    /// Lines `first .. first + old` were replaced by `first .. first + new`;
    /// `line` reads the text after the edit.
    pub fn edit<'a>(
        &mut self,
        first: usize,
        old: usize,
        new: usize,
        line: impl Fn(usize) -> &'a str,
    ) {
        let lo = self.before(first);
        let hi = self.before(first + old);
        let delta = new as isize - old as isize;
        let mut fresh: Vec<usize> = Vec::new();
        for n in first..first + new {
            if is_fence(line(n)) {
                fresh.push(n);
            }
        }
        let tail: Vec<usize> = self
            .lines
            .get(hi..)
            .unwrap_or(&[])
            .iter()
            .map(|l| (*l as isize + delta).max(0) as usize)
            .collect();
        self.lines.truncate(lo);
        self.lines.extend(fresh);
        self.lines.extend(tail);
    }

    pub fn count(&self) -> usize {
        self.lines.len()
    }
}

/// One heading for the outline pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub line: usize,
    pub level: u8,
    pub text: String,
}

/// The text of a heading with its inline syntax taken out, as the outline
/// and the reading view show it.
pub fn plain(line: &str, from: usize) -> String {
    let mut out = String::new();
    for seg in inline(line, from) {
        if seg.style.syntax {
            continue;
        }
        out.push_str(line.get(seg.start..seg.end).unwrap_or(""));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(line: &str) -> String {
        plain(line, 0)
    }

    #[test]
    fn blocks_are_classified() {
        assert_eq!(
            classify("## Plan", false),
            Block::Heading {
                level: 2,
                content: 3
            }
        );
        assert_eq!(classify("#tag at start", false), Block::Para);
        assert_eq!(classify("####### seven", false), Block::Para);
        assert!(matches!(
            classify("  - item", false),
            Block::Bullet {
                indent: 1,
                content: 4,
                ..
            }
        ));
        assert!(matches!(
            classify("- [x] done", false),
            Block::Task {
                checked: true,
                content: 6,
                box_at: 2,
                ..
            }
        ));
        assert!(matches!(
            classify("- [ ] todo", false),
            Block::Task { checked: false, .. }
        ));
        assert!(matches!(
            classify("12. twelfth", false),
            Block::Ordered { content: 4, .. }
        ));
        assert_eq!(classify("> said", false), Block::Quote { content: 2 });
        assert_eq!(classify("---", false), Block::Rule);
        assert_eq!(classify("- - -", false), Block::Rule);
        assert_eq!(classify("```rust", false), Block::Fence);
        assert_eq!(classify("# not a heading", true), Block::Code);
        assert_eq!(classify("", false), Block::Blank);
        assert_eq!(classify("| a | b |", false), Block::Table);
        assert_eq!(classify("plain words", false), Block::Para);
        assert_eq!(fence_lang("```rust ignore"), "rust");
    }

    #[test]
    fn segments_cover_every_byte_in_order() {
        for line in [
            "plain",
            "a **bold** and *it* and ~~gone~~ and ==lit==",
            "[[Note|alias]] then [site](https://x.y) and #tag/sub",
            "`code with **stars**` after",
            "unclosed **bold and *it",
            "snake_case_names stay plain",
            "é **ü** 漢字 [[日本]]",
            "\\*escaped\\* text",
        ] {
            let segs = inline(line, 0);
            let mut at = 0;
            for s in &segs {
                assert_eq!(s.start, at, "gap in {line:?}: {segs:?}");
                assert!(s.end > s.start);
                at = s.end;
            }
            assert_eq!(at, line.len(), "{line:?}");
        }
    }

    #[test]
    fn styles_and_syntax_are_found() {
        assert_eq!(shown("a **bold** b"), "a bold b");
        assert_eq!(shown("an *italic* word"), "an italic word");
        assert_eq!(shown("**bold *both* bold**"), "bold both bold");
        assert_eq!(shown("~~old~~ ==new=="), "old new");
        assert_eq!(shown("see [[Other note]]"), "see Other note");
        assert_eq!(shown("see [[Other note|that one]]"), "see that one");
        assert_eq!(shown("[Krate](https://krate.tech)"), "Krate");
        assert_eq!(shown("`a*b*c`"), "a*b*c");
        assert_eq!(shown("2 * 3 * 4"), "2 * 3 * 4");
        assert_eq!(shown("snake_case_name"), "snake_case_name");
        assert_eq!(shown("\\*not italic\\*"), "*not italic*");
        let segs = inline("x **b** y", 0);
        assert!(segs.iter().any(|s| s.style.bold && !s.style.syntax));
        let tag = inline("ship it #project/alpha now", 0);
        assert!(tag
            .iter()
            .any(|s| s.style.tag && matches!(s.target, Some(Target::Tag(8, 22)))));
        assert!(!inline("issue #42", 0).iter().any(|s| s.style.tag));
        let url = inline("go https://krate.tech.", 0);
        assert!(url
            .iter()
            .any(|s| matches!(s.target, Some(Target::Url(3, 21)))));
    }

    #[test]
    fn refs_find_links_and_tags_once() {
        let mut links = Vec::new();
        let mut tags = Vec::new();
        refs(
            "[[Alpha]] and [[Beta#Part|b]] #one `[[not]]` ![[Gamma]] #two",
            |l| links.push(String::from(l)),
            |t| tags.push(String::from(t)),
        );
        assert_eq!(links, ["Alpha", "Beta", "Gamma"]);
        assert_eq!(tags, ["one", "two"]);
    }

    #[test]
    fn fences_answer_and_follow_edits() {
        let text = ["a", "```", "code", "```", "b", "~~~", "x"];
        let mut f = Fences::build(text.iter().copied());
        assert!(!f.in_code(0));
        assert!(f.is_fence(1) && f.opens(1));
        assert!(f.in_code(2));
        assert!(f.is_fence(3) && !f.opens(3));
        assert!(!f.in_code(4));
        assert!(f.in_code(6));
        // Insert two plain lines after line 0: everything shifts by two.
        let after = ["a", "n1", "n2", "```", "code", "```", "b", "~~~", "x"];
        f.edit(0, 1, 3, |n| after.get(n).copied().unwrap_or(""));
        assert!(f.in_code(4) && f.is_fence(3) && f.is_fence(5) && f.is_fence(7));
        assert!(!f.in_code(6));
        // Delete the opening fence: the block closes nothing now.
        let gone = ["a", "n1", "n2", "code", "```", "b", "~~~", "x"];
        f.edit(3, 1, 0, |n| gone.get(n).copied().unwrap_or(""));
        assert_eq!(f.count(), 2);
        assert!(f.in_code(5) && !f.in_code(3));
        let rebuilt = Fences::build(gone.iter().copied());
        assert_eq!(rebuilt.lines, f.lines);
    }
}

#[cfg(test)]
mod obsidian_inline {
    use super::*;

    fn find(line: &str, pred: impl Fn(&Style) -> bool) -> Vec<&str> {
        inline(line, 0)
            .into_iter()
            .filter(|s| pred(&s.style) && !s.style.syntax)
            .map(|s| &line[s.start..s.end])
            .collect()
    }

    #[test]
    fn math_footnotes_comments_html_and_block_ids() {
        let l = "Inline $e^{i\\pi}$ and $5 and $6 and $$x$$ end";
        assert_eq!(find(l, |s| s.math), ["e^{i\\pi}", "x"]);
        assert_eq!(plain("costs $5 and $6", 0), "costs $5 and $6");
        let f = "Here[^1] and ^[inline [nested] note] done";
        assert_eq!(find(f, |s| s.footnote), ["1", "inline [nested] note"]);
        let def = inline("[^1]: The text.", 0);
        assert!(def
            .iter()
            .any(|s| s.style.footnote && matches!(s.target, Some(Target::Footnote(2, 3)))));
        let c = "a %%hidden%% b %%open";
        assert_eq!(find(c, |s| s.comment), ["hidden", "open"]);
        let h = "x<sup>2</sup> H<sub>2</sub>O <u>u</u> <mark>m</mark> <kbd>Cmd</kbd> a<br>b <b>bold</b>";
        assert_eq!(find(h, |s| s.sup), ["2"]);
        assert_eq!(find(h, |s| s.sub), ["2"]);
        assert_eq!(find(h, |s| s.underline), ["u"]);
        assert_eq!(find(h, |s| s.mark), ["m"]);
        assert_eq!(find(h, |s| s.kbd), ["Cmd"]);
        assert_eq!(find(h, |s| s.bold), ["bold"]);
        assert_eq!(plain(h, 0), "x2 H2O u m Cmd ab bold");
        assert_eq!(plain("a <notatag> b", 0), "a <notatag> b");
        assert_eq!(find("A block. ^abc-1", |s| s.blockid), ["^abc-1"]);
        assert!(find("x^2 is not an id", |s| s.blockid).is_empty());
        for line in [l, f, c, h, "<u>unclosed", "$", "%%", "[^", "^[", "a ^"] {
            let segs = inline(line, 0);
            let mut at = 0;
            for s in &segs {
                assert_eq!(s.start, at, "{line:?}");
                at = s.end;
            }
            assert_eq!(at, line.len());
        }
    }
}
