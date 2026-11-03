//! Rust highlighting, one line at a time.
//!
//! A line is tokenized from the state the line before it ended in -- inside a
//! block comment (nested ones count their depth, as Rust's do), inside a
//! string that runs on, inside a raw string with its `#`s -- so the editor
//! only ever tokenizes the lines it draws, plus the states down to them.
//! [`Highlighter`] keeps those line-start states and drops them from the
//! first line an edit touched.

use alloc::vec::Vec;

/// What a run of characters is, for choosing its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Keyword,
    /// `self`, `Self`, `true`, `false`, `crate`, `super`.
    Special,
    Type,
    Function,
    Macro,
    Lifetime,
    Str,
    Char,
    Number,
    Comment,
    DocComment,
    Attribute,
    Punct,
    /// An ALL_CAPS name: a constant or a static.
    Constant,
}

/// Where a line ends up, carried into the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    #[default]
    Code,
    /// Inside `/* */`, `depth` deep. `doc` for `/** */` and `/*! */`.
    Block { depth: u16, doc: bool },
    /// Inside a `"..."` that continues onto the next line.
    Str,
    /// Inside `r#"..."#` with this many `#`s.
    RawStr { hashes: u16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "dyn", "else", "enum", "extern", "fn",
    "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
    "return", "static", "struct", "trait", "type", "union", "unsafe", "use", "where", "while",
    "yield", "macro_rules", "try",
];

const SPECIAL: &[&str] = &["self", "Self", "true", "false", "crate", "super"];

const PRIMITIVES: &[&str] = &[
    "bool", "char", "str", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64",
    "i128", "isize", "f32", "f64",
];

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c >= 0x80
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

struct Out {
    spans: Vec<Span>,
}

impl Out {
    fn push(&mut self, start: usize, end: usize, kind: Kind) {
        if end <= start {
            return;
        }
        if let Some(last) = self.spans.last_mut() {
            if last.kind == kind && last.end == start {
                last.end = end;
                return;
            }
        }
        self.spans.push(Span { start, end, kind });
    }
}

/// Scan the rest of a block comment from `i`. Returns where it stopped and
/// the depth left (0 when it closed on this line).
fn block_comment(s: &[u8], mut i: usize, mut depth: u16) -> (usize, u16) {
    while i < s.len() {
        if s[i] == b'/' && s.get(i + 1) == Some(&b'*') {
            depth = depth.saturating_add(1);
            i += 2;
        } else if s[i] == b'*' && s.get(i + 1) == Some(&b'/') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return (i, 0);
            }
        } else {
            i += 1;
        }
    }
    (i, depth)
}

/// Scan a `"` string body from `i` (after the opening quote). Returns where
/// it stopped and whether it closed.
fn string_body(s: &[u8], mut i: usize) -> (usize, bool) {
    while i < s.len() {
        match s[i] {
            b'\\' => i += 2,
            b'"' => return (i + 1, true),
            _ => i += 1,
        }
    }
    (s.len(), false)
}

/// Scan a raw string body from `i` for `"` followed by `hashes` `#`s.
fn raw_body(s: &[u8], mut i: usize, hashes: u16) -> (usize, bool) {
    while i < s.len() {
        if s[i] == b'"' {
            let mut k = 0usize;
            while k < hashes as usize && s.get(i + 1 + k) == Some(&b'#') {
                k += 1;
            }
            if k == hashes as usize {
                return (i + 1 + k, true);
            }
        }
        i += 1;
    }
    (s.len(), false)
}

/// Tokenize one line from `state`. Returns the spans (covering every
/// non-blank byte, in order) and the state the next line starts in.
pub fn highlight_line(line: &str, state: State) -> (Vec<Span>, State) {
    let s = line.as_bytes();
    let mut out = Out { spans: Vec::new() };
    let mut i = 0usize;

    // Finish whatever the previous line left open.
    match state {
        State::Code => {}
        State::Block { depth, doc } => {
            let (end, left) = block_comment(s, 0, depth);
            out.push(0, end, if doc { Kind::DocComment } else { Kind::Comment });
            if left > 0 {
                return (out.spans, State::Block { depth: left, doc });
            }
            i = end;
        }
        State::Str => {
            let (end, closed) = string_body(s, 0);
            out.push(0, end, Kind::Str);
            if !closed {
                return (out.spans, State::Str);
            }
            i = end;
        }
        State::RawStr { hashes } => {
            let (end, closed) = raw_body(s, 0, hashes);
            out.push(0, end, Kind::Str);
            if !closed {
                return (out.spans, State::RawStr { hashes });
            }
            i = end;
        }
    }

    while i < s.len() {
        let c = s[i];
        if c == b' ' || c == b'\t' {
            i += 1;
            continue;
        }
        let start = i;

        // Comments.
        if c == b'/' && s.get(i + 1) == Some(&b'/') {
            let doc = matches!(s.get(i + 2), Some(b'/') | Some(b'!')) && s.get(i + 3) != Some(&b'/');
            out.push(i, s.len(), if doc { Kind::DocComment } else { Kind::Comment });
            return (out.spans, State::Code);
        }
        if c == b'/' && s.get(i + 1) == Some(&b'*') {
            let doc = matches!(s.get(i + 2), Some(b'*') | Some(b'!')) && s.get(i + 3) != Some(&b'/');
            let (end, left) = block_comment(s, i + 2, 1);
            out.push(i, end, if doc { Kind::DocComment } else { Kind::Comment });
            if left > 0 {
                return (out.spans, State::Block { depth: left, doc });
            }
            i = end;
            continue;
        }

        // Attributes.
        if c == b'#' && (s.get(i + 1) == Some(&b'[') || (s.get(i + 1) == Some(&b'!') && s.get(i + 2) == Some(&b'['))) {
            // `#[...]` runs to its closing bracket; nested brackets count.
            let mut attr_depth = 0usize;
            let mut j = i;
            while j < s.len() {
                match s[j] {
                    b'[' => attr_depth += 1,
                    b']' => {
                        attr_depth = attr_depth.saturating_sub(1);
                        if attr_depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    b'"' => {
                        let (end, _) = string_body(s, j + 1);
                        j = end;
                        continue;
                    }
                    _ => {}
                }
                j += 1;
            }
            out.push(i, j.min(s.len()), Kind::Attribute);
            i = j.min(s.len());
            continue;
        }

        // Raw strings and byte/C strings: r"..", r#".."#, br"..", b"..", c"..".
        if c == b'r' || c == b'b' || c == b'c' {
            let mut j = i;
            if (c == b'b' || c == b'c') && s.get(j + 1) == Some(&b'r') {
                j += 1;
            }
            if s.get(j) == Some(&b'r') && matches!(s.get(j + 1), Some(b'"') | Some(b'#')) {
                let mut k = j + 1;
                let mut hashes = 0u16;
                while s.get(k) == Some(&b'#') {
                    hashes += 1;
                    k += 1;
                }
                if s.get(k) == Some(&b'"') {
                    let (end, closed) = raw_body(s, k + 1, hashes);
                    out.push(i, end, Kind::Str);
                    if !closed {
                        return (out.spans, State::RawStr { hashes });
                    }
                    i = end;
                    continue;
                }
            }
            if (c == b'b' || c == b'c') && s.get(i + 1) == Some(&b'"') {
                let (end, closed) = string_body(s, i + 2);
                out.push(i, end, Kind::Str);
                if !closed {
                    return (out.spans, State::Str);
                }
                i = end;
                continue;
            }
            if c == b'b' && s.get(i + 1) == Some(&b'\'') {
                // A byte literal: b'x', b'\n'.
                let end = char_literal_end(s, i + 1).unwrap_or(i + 2);
                out.push(i, end, Kind::Char);
                i = end;
                continue;
            }
        }

        // Strings.
        if c == b'"' {
            let (end, closed) = string_body(s, i + 1);
            out.push(i, end, Kind::Str);
            if !closed {
                return (out.spans, State::Str);
            }
            i = end;
            continue;
        }

        // Chars and lifetimes.
        if c == b'\'' {
            if let Some(end) = char_literal_end(s, i) {
                out.push(i, end, Kind::Char);
                i = end;
                continue;
            }
            if s.get(i + 1).is_some_and(|&n| is_ident_start(n)) {
                let mut j = i + 1;
                while j < s.len() && is_ident(s[j]) {
                    j += 1;
                }
                out.push(i, j, Kind::Lifetime);
                i = j;
                continue;
            }
            out.push(i, i + 1, Kind::Punct);
            i += 1;
            continue;
        }

        // Numbers.
        if c.is_ascii_digit() {
            let mut j = i + 1;
            let hex = c == b'0' && matches!(s.get(i + 1), Some(b'x') | Some(b'o') | Some(b'b'));
            if hex {
                j = i + 2;
            }
            while j < s.len() {
                let d = s[j];
                let more = d.is_ascii_alphanumeric()
                    || d == b'_'
                    || (d == b'.'
                        && !hex
                        && s.get(j + 1).is_some_and(|n| n.is_ascii_digit()))
                    || ((d == b'+' || d == b'-')
                        && !hex
                        && matches!(s.get(j - 1), Some(b'e') | Some(b'E')));
                if !more {
                    break;
                }
                j += 1;
            }
            out.push(i, j, Kind::Number);
            i = j;
            continue;
        }

        // Identifiers, keywords, macros, types, calls.
        if is_ident_start(c) {
            let mut j = i + 1;
            while j < s.len() && is_ident(s[j]) {
                j += 1;
            }
            // A raw identifier: r#match.
            let word = line.get(i..j).unwrap_or("");
            let next = s.get(j).copied();
            let kind = if next == Some(b'!') && s.get(j + 1) != Some(&b'=') {
                j += 1;
                Kind::Macro
            } else if KEYWORDS.contains(&word) {
                Kind::Keyword
            } else if SPECIAL.contains(&word) {
                Kind::Special
            } else if PRIMITIVES.contains(&word) {
                Kind::Type
            } else if word.len() > 1
                && word.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                && word.bytes().any(|b| b.is_ascii_uppercase())
            {
                Kind::Constant
            } else if word.as_bytes().first().is_some_and(u8::is_ascii_uppercase) {
                Kind::Type
            } else if next == Some(b'(')
                || (next == Some(b':') && s.get(j + 1) == Some(&b':') && s.get(j + 2) == Some(&b'<'))
            {
                Kind::Function
            } else if start >= 3 && line.get(..start).is_some_and(|h| h.trim_end().ends_with("fn")) {
                Kind::Function
            } else {
                Kind::Text
            };
            out.push(i, j, kind);
            i = j;
            continue;
        }

        // Everything else is punctuation, one character at a time (it may be
        // multi-byte, so step over the whole character).
        let ch_len = line
            .get(i..)
            .and_then(|r| r.chars().next())
            .map(char::len_utf8)
            .unwrap_or(1);
        out.push(i, i + ch_len, Kind::Punct);
        i += ch_len;
    }
    (out.spans, State::Code)
}

/// If a char literal starts at `i` (`'x'`, `'\n'`, `'\u{1F600}'`, `'é'`),
/// where it ends.
fn char_literal_end(s: &[u8], i: usize) -> Option<usize> {
    if s.get(i) != Some(&b'\'') {
        return None;
    }
    match s.get(i + 1)? {
        b'\\' => {
            // An escape: run to the next quote within a short reach.
            let mut j = i + 2;
            while j < s.len() && j < i + 14 {
                if s[j] == b'\'' {
                    return Some(j + 1);
                }
                j += 1;
            }
            None
        }
        b'\'' => None,
        &first => {
            // One character, which may be several bytes.
            let len = if first < 0x80 {
                1
            } else if first >= 0xF0 {
                4
            } else if first >= 0xE0 {
                3
            } else {
                2
            };
            if s.get(i + 1 + len) == Some(&b'\'') {
                Some(i + 2 + len)
            } else {
                None
            }
        }
    }
}

/// Line-start states for a buffer, kept valid from the top down.
#[derive(Debug, Clone, Default)]
pub struct Highlighter {
    /// `states[i]` is the state line `i` starts in.
    states: Vec<State>,
}

impl Highlighter {
    pub fn new() -> Self {
        Highlighter { states: alloc::vec![State::Code] }
    }

    /// An edit touched line `line`: every state after it may have changed.
    pub fn invalidate_from(&mut self, line: usize) {
        self.states.truncate(line + 1);
        if self.states.is_empty() {
            self.states.push(State::Code);
        }
    }

    /// The state line `line` starts in, computing down to it.
    pub fn state_at(&mut self, line: usize, text: impl Fn(usize) -> alloc::string::String) -> State {
        while self.states.len() <= line {
            let i = self.states.len() - 1;
            let st = self.states.last().copied().unwrap_or_default();
            let (_, next) = highlight_line(&text(i), st);
            self.states.push(next);
        }
        self.states.get(line).copied().unwrap_or_default()
    }

    pub fn known(&self) -> usize {
        self.states.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    fn kinds(line: &str) -> Vec<(String, Kind)> {
        let (spans, _) = highlight_line(line, State::Code);
        spans
            .iter()
            .filter(|s| s.kind != Kind::Punct)
            .map(|s| (String::from(&line[s.start..s.end]), s.kind))
            .collect()
    }

    fn kind_of(line: &str, word: &str) -> Kind {
        let (spans, _) = highlight_line(line, State::Code);
        let at = line.find(word).expect("word in line");
        spans
            .iter()
            .find(|s| s.start <= at && at < s.end)
            .map(|s| s.kind)
            .expect("a span covers the word")
    }

    #[test]
    fn keywords_types_functions_macros() {
        let line = "pub fn main() -> Result<u32, Error> { println!(\"hi\"); }";
        assert_eq!(kind_of(line, "pub"), Kind::Keyword);
        assert_eq!(kind_of(line, "fn"), Kind::Keyword);
        assert_eq!(kind_of(line, "main"), Kind::Function);
        assert_eq!(kind_of(line, "Result"), Kind::Type);
        assert_eq!(kind_of(line, "u32"), Kind::Type);
        assert_eq!(kind_of(line, "println!"), Kind::Macro);
        assert_eq!(kind_of(line, "\"hi\""), Kind::Str);
        assert_eq!(kind_of("let x = foo(1);", "foo"), Kind::Function);
        assert_eq!(kind_of("self.x = true;", "self"), Kind::Special);
        assert_eq!(kind_of("self.x = true;", "true"), Kind::Special);
        assert_eq!(kind_of("const MAX_LEN: usize = 4;", "MAX_LEN"), Kind::Constant);
        assert_eq!(kind_of("x != y", "x"), Kind::Text);
    }

    #[test]
    fn numbers_in_every_shape() {
        for n in ["42", "1_000", "0xFF_u8", "3.14", "1e-9", "2.5f32", "0b1010"] {
            let line = alloc::format!("let a = {n};");
            assert_eq!(kind_of(&line, n), Kind::Number, "{n}");
            let (spans, _) = highlight_line(&line, State::Code);
            let at = line.find(n).unwrap();
            let span = spans.iter().find(|s| s.start == at).unwrap();
            assert_eq!(&line[span.start..span.end], n);
        }
        // A range is two numbers, not one.
        let (spans, _) = highlight_line("0..10", State::Code);
        assert_eq!(spans.iter().filter(|s| s.kind == Kind::Number).count(), 2);
    }

    #[test]
    fn chars_versus_lifetimes() {
        assert_eq!(kind_of("let c = 'x';", "'x'"), Kind::Char);
        assert_eq!(kind_of("let c = '\\n';", "'\\n'"), Kind::Char);
        assert_eq!(kind_of("let c = '\\u{1F600}';", "'\\u{1F600}'"), Kind::Char);
        assert_eq!(kind_of("let c = 'é';", "'é'"), Kind::Char);
        assert_eq!(kind_of("fn f<'a>(x: &'a str) {}", "'a>"), Kind::Lifetime);
        assert_eq!(kind_of("&'static str", "'static"), Kind::Lifetime);
        assert_eq!(kind_of("let b = b'a';", "b'a'"), Kind::Char);
    }

    #[test]
    fn comments_including_nested_blocks_and_docs() {
        assert_eq!(kind_of("let x = 1; // note", "// note"), Kind::Comment);
        assert_eq!(kind_of("/// A doc line", "///"), Kind::DocComment);
        assert_eq!(kind_of("//! Crate doc", "//!"), Kind::DocComment);
        assert_eq!(kind_of("//// not doc", "////"), Kind::Comment);
        // Nested: the inner close does not end the comment.
        let (spans, st) = highlight_line("/* a /* b */ still */ code", State::Code);
        assert_eq!(st, State::Code);
        assert_eq!(spans[0].kind, Kind::Comment);
        assert_eq!(spans[0].end, "/* a /* b */ still */".len());
        // Across lines, with depth carried.
        let (_, st) = highlight_line("x /* one /* two", State::Code);
        assert_eq!(st, State::Block { depth: 2, doc: false });
        let (spans, st) = highlight_line("still */ inside */ fn", State::Block { depth: 2, doc: false });
        assert_eq!(st, State::Code);
        assert_eq!(spans[0].kind, Kind::Comment);
        assert_eq!(spans.last().unwrap().kind, Kind::Keyword);
    }

    #[test]
    fn strings_raw_strings_and_escapes() {
        assert_eq!(kind_of(r#"let s = "a \" b";"#, r#""a \" b""#), Kind::Str);
        let line = r###"let s = r#"has "quotes" inside"#;"###;
        assert_eq!(kind_of(line, "r#"), Kind::Str);
        let (spans, _) = highlight_line(line, State::Code);
        let s = spans.iter().find(|s| s.kind == Kind::Str).unwrap();
        assert_eq!(&line[s.start..s.end], r###"r#"has "quotes" inside"#"###);
        // A string left open carries on.
        let (_, st) = highlight_line("let s = \"first", State::Code);
        assert_eq!(st, State::Str);
        let (spans, st) = highlight_line("second\"; let", State::Str);
        assert_eq!(st, State::Code);
        assert_eq!(spans[0].kind, Kind::Str);
        let (_, st) = highlight_line("let s = r##\"open", State::Code);
        assert_eq!(st, State::RawStr { hashes: 2 });
        let (_, st) = highlight_line("\"# not yet", State::RawStr { hashes: 2 });
        assert_eq!(st, State::RawStr { hashes: 2 });
        let (_, st) = highlight_line("done\"##;", State::RawStr { hashes: 2 });
        assert_eq!(st, State::Code);
        assert_eq!(kind_of("let b = b\"bytes\";", "b\"bytes\""), Kind::Str);
    }

    #[test]
    fn attributes_cover_their_brackets() {
        assert_eq!(kind_of("#[derive(Debug, Clone)]", "Debug"), Kind::Attribute);
        assert_eq!(kind_of("#![no_std]", "no_std"), Kind::Attribute);
        assert_eq!(kind_of("#[cfg(feature = \"x]\")] fn", "fn"), Kind::Keyword);
    }

    #[test]
    fn spans_are_ordered_and_inside_the_line() {
        let line = "impl<'a, T: Clone> Foo<'a, T> { fn é(&self) -> &'a T { &self.0 } }";
        let (spans, _) = highlight_line(line, State::Code);
        let mut at = 0;
        for s in &spans {
            assert!(s.start >= at && s.end <= line.len() && s.start < s.end);
            assert!(line.is_char_boundary(s.start) && line.is_char_boundary(s.end));
            at = s.end;
        }
        assert!(!kinds(line).is_empty());
    }

    #[test]
    fn highlighter_recomputes_from_an_edit() {
        let mut lines = alloc::vec![
            String::from("fn a() {}"),
            String::from("/* open"),
            String::from("still comment"),
            String::from("*/ fn b() {}"),
        ];
        let mut h = Highlighter::new();
        let get = |v: &Vec<String>| {
            let v = v.clone();
            move |i: usize| v.get(i).cloned().unwrap_or_default()
        };
        assert_eq!(h.state_at(2, get(&lines)), State::Block { depth: 1, doc: false });
        assert_eq!(h.state_at(3, get(&lines)), State::Block { depth: 1, doc: false });
        lines[1] = String::from("// closed now");
        h.invalidate_from(1);
        assert_eq!(h.state_at(3, get(&lines)), State::Code);
    }
}
