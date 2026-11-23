//! Ports diagrams/common/tokens.ts: the tokenizer for the diagram types
//! Mermaid defines with Langium grammars. Token types are tried in order
//! and the first match wins, as in Chevrotain.
//!
//! Each regular expression is a function that matches what it does,
//! sticky at a character index of a [`Text`] ([`Pattern::Re`]); the
//! common ones are here. Token positions are character indices; errors
//! locate in bytes through [`Text::byte`].

use alloc::boxed::Box;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use super::chars::{is_line_end, is_space, Text, NONE};
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js;

/// Ports `type Scanner`: the end of a match at `at`, or -1. `previous` is
/// the last visible token's type index, or -1 at the start.
pub type Scanner = Box<dyn FnMut(&Text, usize, i32) -> isize>;

/// A sticky regular expression: the end of its match at a position.
pub type Matcher = Rc<dyn Fn(&Text, usize) -> Option<usize>>;

/// A lookahead at a position: whether it matches there.
pub type Lookahead = dyn Fn(&Text, usize) -> bool;

/// A matcher that also sees the token before (`match`).
pub type MatchFn = Rc<dyn Fn(&Text, usize, i32) -> isize>;

/// A token type's `pattern`: a literal string, or a regular expression.
#[derive(Clone)]
pub enum Pattern {
    Lit(String),
    Re(Matcher),
}

/// Ports `interface TokenType`. `longer` holds indices into the table the
/// type is in (Chevrotain's LONGER_ALT, which Pele finds by identity).
#[derive(Clone)]
pub struct TokenType {
    pub name: String,
    pub pattern: Pattern,
    pub hidden: bool,
    /// Start of a construct that needs a closer: when this matches but the
    /// whole pattern does not, the pattern is not tried again.
    pub opener: Option<Rc<Lookahead>>,
    /// Every ASCII character a match of a regular expression can start
    /// with, when that is known.
    pub first: Option<String>,
    pub longer: Vec<usize>,
    /// Makes a matcher that answers as `pattern` does, once per text.
    pub scanner: Option<fn() -> Scanner>,
    /// Matches in place of the pattern (`match`).
    pub matcher: Option<MatchFn>,
}

impl TokenType {
    /// A type with a literal pattern.
    pub fn lit(name: &str, pattern: &str) -> TokenType {
        TokenType::new(name, Pattern::Lit(pattern.into()), None)
    }

    /// A type with a regular expression and the characters it starts with.
    pub fn re(
        name: &str,
        f: impl Fn(&Text, usize) -> Option<usize> + 'static,
        first: Option<&str>,
    ) -> TokenType {
        TokenType::new(name, Pattern::Re(Rc::new(f)), first)
    }

    fn new(name: &str, pattern: Pattern, first: Option<&str>) -> TokenType {
        TokenType {
            name: name.into(),
            pattern,
            hidden: false,
            opener: None,
            first: first.map(String::from),
            longer: Vec::new(),
            scanner: None,
            matcher: None,
        }
    }

    pub fn hidden(mut self) -> TokenType {
        self.hidden = true;
        self
    }
}

/// Ports `interface Tokens`. `kinds` ends with -1 at the end of input.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tokens {
    pub kinds: Vec<i32>,
    pub starts: Vec<usize>,
    pub ends: Vec<usize>,
    pub src: Text,
}

/// Ports `prepare`: for each ASCII character, the token types that can
/// start with it, in matching order.
fn prepare(types: &[TokenType]) -> Vec<Vec<usize>> {
    let mut starting = Vec::with_capacity(128);
    for c in 0u8..128 {
        let ch = c as char;
        starting.push(
            (0..types.len())
                .filter(|&k| match &types[k].pattern {
                    Pattern::Lit(s) => s.starts_with(ch),
                    Pattern::Re(_) => types[k].first.as_ref().is_none_or(|f| f.contains(ch)),
                })
                .collect(),
        );
    }
    starting
}

/// Ports `tokenize`.
pub fn tokenize(
    source: &str,
    types: &[TokenType],
    diagram: &'static str,
) -> Result<Tokens, PeleError> {
    let src = Text::new(source);
    let mut kinds: Vec<i32> = Vec::new();
    let mut starts: Vec<usize> = Vec::new();
    let mut ends: Vec<usize> = Vec::new();
    let mut dead = alloc::vec![false; types.len()];
    let starting = prepare(types);
    let every: Vec<usize> = (0..types.len()).collect();
    let mut scanners: Vec<Option<Scanner>> = (0..types.len()).map(|_| None).collect();
    let n = src.len();

    let mut match_at = |k: usize, p: usize, kinds: &Vec<i32>| -> isize {
        if dead[k] {
            return -1;
        }
        let ty = &types[k];
        let previous = kinds.last().copied().unwrap_or(-1);
        if let Some(m) = &ty.matcher {
            return m(&src, p, previous);
        }
        if let Some(make) = ty.scanner {
            let s = scanners[k].get_or_insert_with(make);
            return s(&src, p, previous);
        }
        match &ty.pattern {
            Pattern::Lit(s) => {
                if src.starts_with(s, p) {
                    (p + s.chars().count()) as isize
                } else {
                    -1
                }
            }
            Pattern::Re(f) => {
                if let Some(e) = f(&src, p) {
                    return e as isize;
                }
                if let Some(o) = &ty.opener {
                    if o(&src, p) {
                        dead[k] = true;
                    }
                }
                -1
            }
        }
    };

    let mut p = 0usize;
    'scan: while p < n {
        let c = src.c(p);
        let list = if c < 128 {
            &starting[c as usize]
        } else {
            &every
        };
        for &k in list {
            let mut end = match_at(k, p, &kinds);
            if end <= p as isize {
                continue;
            }
            let mut kind = k;
            for &alt in &types[k].longer {
                let alt_end = match_at(alt, p, &kinds);
                if alt_end > end {
                    kind = alt;
                    end = alt_end;
                    break;
                }
            }
            if !types[kind].hidden {
                kinds.push(kind as i32);
                starts.push(p);
                ends.push(end as usize);
            }
            p = end as usize;
            continue 'scan;
        }
        return Err(syntax_error(diagram, source, src.byte(p), "", true));
    }
    kinds.push(-1);
    starts.push(n);
    ends.push(n);
    Ok(Tokens {
        kinds,
        starts,
        ends,
        src,
    })
}

/// `[\t ]*` from `p`.
pub fn blanks(src: &Text, mut p: usize) -> usize {
    while src.c(p) == 9 || src.c(p) == 32 {
        p += 1;
    }
    p
}

/// `[^\n\r]*?(?=%%)|[^\n\r]*` from `p`: to the first `%%` on the line, or
/// to its end.
pub fn rest_of_line(src: &Text, p: usize) -> usize {
    let mut q = p;
    while q < src.len() && src.c(q) != 10 && src.c(q) != 13 {
        if src.c(q) == 37 && src.c(q + 1) == 37 {
            return q;
        }
        q += 1;
    }
    q
}

/// `[^\n\r]*` from `p`.
fn to_line_end(src: &Text, mut p: usize) -> usize {
    while p < src.len() && src.c(p) != 10 && src.c(p) != 13 {
        p += 1;
    }
    p
}

/// `(?!\S)` at `p`: the end, or whitespace.
pub fn not_followed(src: &Text, p: usize) -> bool {
    p >= src.len() || is_space(src.c(p))
}

/// `[\t ]*accDescr` and then the braced form `\s*{` at `p`, or None.
fn acc_descr_head(src: &Text, p: usize) -> Option<(usize, bool)> {
    let q = blanks(src, p);
    if !src.starts_with("accDescr", q) {
        return None;
    }
    let q = q + 8;
    let r = blanks(src, q);
    if src.c(r) == 58 {
        return Some((rest_of_line(src, r + 1), false));
    }
    let mut r = q;
    while is_space(src.c(r)) {
        r += 1;
    }
    if src.c(r) == 123 {
        return Some((r + 1, true));
    }
    None
}

/// `/[\t ]*accDescr(?:[\t ]*:([^\n\r]*?(?=%%)|[^\n\r]*)|\s*{([^}]*)})/y`.
pub fn acc_descr_re(src: &Text, p: usize) -> Option<usize> {
    let (end, braced) = acc_descr_head(src, p)?;
    if !braced {
        return Some(end);
    }
    src.find_char(125, end).map(|c| c + 1)
}

/// The head of the accDescr scanner: `[\t ]*accDescr(?:[\t ]*:(...)|\s*({))`,
/// with whether it is the braced form.
pub fn acc_descr_open(src: &Text, p: usize) -> Option<(usize, bool)> {
    acc_descr_head(src, p)
}

/// `/[\t ]*accTitle[\t ]*:(?:[^\n\r]*?(?=%%)|[^\n\r]*)/y`.
pub fn acc_title_re(src: &Text, p: usize) -> Option<usize> {
    let q = blanks(src, p);
    if !src.starts_with("accTitle", q) {
        return None;
    }
    let r = blanks(src, q + 8);
    if src.c(r) != 58 {
        return None;
    }
    Some(rest_of_line(src, r + 1))
}

/// `[\t ]*title(?:[\t ][^\n\r]*?(?=%%)|[\t ][^\n\r]*|(?!rest))`, where
/// `rest` is a lookahead, or none for the plain `TITLE`.
pub fn title_re(src: &Text, p: usize, rest: Option<&Lookahead>) -> Option<usize> {
    let q = blanks(src, p);
    if !src.starts_with("title", q) {
        return None;
    }
    let q = q + 5;
    if src.c(q) == 9 || src.c(q) == 32 {
        return Some(rest_of_line(src, q + 1));
    }
    match rest {
        Some(f) if f(src, q) => None,
        _ => Some(q),
    }
}

/// Ports `ACC_DESCR`.
pub fn acc_descr_type() -> TokenType {
    TokenType::re("ACC_DESCR", acc_descr_re, Some("\t a"))
}

/// Ports `ACC_TITLE`.
pub fn acc_title_type() -> TokenType {
    TokenType::re("ACC_TITLE", acc_title_re, Some("\t a"))
}

/// Ports `TITLE`.
pub fn title_type() -> TokenType {
    TokenType::re("TITLE", |s, p| title_re(s, p, None), Some("\t t"))
}

/// Ports `titleBefore`: `rest` matches a name suffix (as a lookahead) so
/// `title` does not split longer names.
pub fn title_before(rest: fn(&Text, usize) -> bool) -> TokenType {
    TokenType::re(
        "TITLE",
        move |s, p| title_re(s, p, Some(&rest)),
        Some("\t t"),
    )
}

/// `/"([^"\\]|\\.)*"|'([^'\\]|\\.)*'/y`.
pub fn string_re(src: &Text, p: usize) -> Option<usize> {
    let q = src.c(p);
    if q != 34 && q != 39 {
        return None;
    }
    let mut i = p + 1;
    loop {
        let c = src.c(i);
        if c == NONE {
            return None;
        }
        if c == q {
            return Some(i + 1);
        }
        if c == 92 {
            let d = src.c(i + 1);
            if d == NONE || is_line_end(d) {
                return None;
            }
            i += 2;
        } else {
            i += 1;
        }
    }
}

/// Ports `STRING`.
pub fn string_type() -> TokenType {
    TokenType::re("STRING", string_re, Some("\"'"))
}

/// `/\r?\n/y`.
pub fn newline_re(src: &Text, p: usize) -> Option<usize> {
    if src.c(p) == 10 {
        Some(p + 1)
    } else if src.c(p) == 13 && src.c(p + 1) == 10 {
        Some(p + 2)
    } else {
        None
    }
}

/// Ports `NEWLINE`.
pub fn newline_type() -> TokenType {
    TokenType::re("NEWLINE", newline_re, Some("\r\n"))
}

/// Ports `WHITESPACE`.
pub fn whitespace_type() -> TokenType {
    TokenType::re(
        "WHITESPACE",
        |s, p| {
            let e = blanks(s, p);
            (e > p).then_some(e)
        },
        Some("\t "),
    )
    .hidden()
}

/// `(?:\r?\n|(?!\S))` at `p`.
fn line_or_space(src: &Text, p: usize) -> Option<usize> {
    newline_re(src, p).or_else(|| not_followed(src, p).then_some(p))
}

/// `/---[\t ]*\r?\n/y`.
fn yaml_open(src: &Text, p: usize) -> Option<usize> {
    if !src.starts_with("---", p) {
        return None;
    }
    newline_re(src, blanks(src, p + 3))
}

/// `/---[\t ]*\r?\n(?:[\S\s]*?\r?\n)?---(?:\r?\n|(?!\S))/y`.
pub fn yaml_re(src: &Text, p: usize) -> Option<usize> {
    let q = yaml_open(src, p)?;
    let close = |at: usize| -> Option<usize> {
        if src.starts_with("---", at) {
            line_or_space(src, at + 3)
        } else {
            None
        }
    };
    let mut j = q;
    while let Some(nl) = src.find_char(10, j) {
        if let Some(e) = close(nl + 1) {
            return Some(e);
        }
        j = nl + 1;
    }
    close(q)
}

/// Ports `YAML`.
pub fn yaml_type() -> TokenType {
    let mut t = TokenType::re("YAML", yaml_re, Some("-")).hidden();
    t.opener = Some(Rc::new(|s, p| yaml_open(s, p).is_some()));
    t
}

/// `/[\t ]*%%{/y`.
fn directive_open(src: &Text, p: usize) -> Option<usize> {
    let q = blanks(src, p);
    src.starts_with("%%{", q).then_some(q + 3)
}

/// `/[\t ]*%%{[\S\s]*?}%%(?:\r?\n|(?!\S))/y`.
pub fn directive_re(src: &Text, p: usize) -> Option<usize> {
    let mut j = directive_open(src, p)?;
    while let Some(c) = src.index_of("}%%", j) {
        if let Some(e) = line_or_space(src, c + 3) {
            return Some(e);
        }
        j = c + 1;
    }
    None
}

/// Ports `DIRECTIVE`.
pub fn directive_type() -> TokenType {
    let mut t = TokenType::re("DIRECTIVE", directive_re, Some("\t %")).hidden();
    t.opener = Some(Rc::new(|s, p| directive_open(s, p).is_some()));
    t
}

/// `/[\t ]*%%[^\n\r]*/y`.
pub fn comment_re(src: &Text, p: usize) -> Option<usize> {
    let q = blanks(src, p);
    if !src.starts_with("%%", q) {
        return None;
    }
    Some(to_line_end(src, q + 2))
}

/// Ports `SINGLE_LINE_COMMENT`.
pub fn single_line_comment_type() -> TokenType {
    TokenType::re("SINGLE_LINE_COMMENT", comment_re, Some("\t %")).hidden()
}

/// Ports `keyword`: Mermaid ends a keyword only at whitespace, a comment,
/// or the end of input.
pub fn keyword(word: &'static str) -> TokenType {
    let first: String = word.chars().take(1).collect();
    let mut t = TokenType::re(
        word,
        move |s, p| {
            if !s.starts_with(word, p) {
                return None;
            }
            let e = p + word.chars().count();
            (s.starts_with("%%", e) || not_followed(s, e)).then_some(e)
        },
        None,
    );
    t.first = Some(first);
    t
}

/// `/[\t ]{2,}/g` replaced with one space.
fn squeeze_blanks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\t' || chars[i] == ' ' {
            let mut j = i;
            while j < chars.len() && (chars[j] == '\t' || chars[j] == ' ') {
                j += 1;
            }
            if j - i >= 2 {
                out.push(' ');
            } else {
                out.push(chars[i]);
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Ports `single`.
fn single(text: &str) -> String {
    squeeze_blanks(js::trim(text))
}

/// Ports `titleValue`.
pub fn title_value(text: &str) -> String {
    let at = text.find("title").map_or(text.len(), |k| k + 5);
    single(&text[at..])
}

/// Ports `accTitleValue`.
pub fn acc_title_value(text: &str) -> String {
    let at = text.find("accTitle").map_or(0, |k| k + 8);
    let rest = &text[at..];
    let colon = rest.find(':').map_or(rest.len(), |k| k + 1);
    let value = &rest[colon..];
    let end = value.find(['\n', '\r']).unwrap_or(value.len());
    single(&value[..end])
}

/// `/^\s*/gm` replaced with nothing.
fn strip_line_starts(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let start = i == 0 || is_line_end(chars[i - 1] as u32);
        if start && js::is_space(chars[i]) {
            while i < chars.len() && js::is_space(chars[i]) {
                i += 1;
            }
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `/\s+$/gm` replaced with nothing.
fn strip_line_ends(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        if js::is_space(chars[i]) {
            let mut b = i;
            while b < n && js::is_space(chars[b]) {
                b += 1;
            }
            if b == n {
                break;
            }
            if let Some(j) = (i + 1..b).rev().find(|&j| is_line_end(chars[j] as u32)) {
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// `/[\n\r]{2,}/g` replaced with `\n`.
fn squeeze_breaks(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\n' || chars[i] == '\r' {
            let mut j = i;
            while j < chars.len() && (chars[j] == '\n' || chars[j] == '\r') {
                j += 1;
            }
            if j - i >= 2 {
                out.push('\n');
            } else {
                out.push(chars[i]);
            }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Ports `accDescrValue`.
pub fn acc_descr_value(text: &str) -> String {
    let at = text.find("accDescr").map_or(0, |k| k + 8);
    let rest = &text[at..];
    let t = Text::new(rest);
    let r = blanks(&t, 0);
    if t.c(r) == 58 {
        let value = t.slice(r + 1, t.len());
        let end = value.find(['\n', '\r']).unwrap_or(value.len());
        return single(&value[..end]);
    }
    let open = rest.find('{').map_or(rest.len(), |k| k + 1);
    let body = &rest[open..];
    let close = body.find('}').unwrap_or(body.len());
    squeeze_breaks(&squeeze_blanks(&strip_line_ends(&strip_line_starts(
        &body[..close],
    ))))
}

/// Ports `stringValue`.
pub fn string_value(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 2 {
        return String::new();
    }
    if !text.contains('\\') {
        return chars[1..chars.len() - 1].iter().collect();
    }
    let mut out = String::new();
    let mut i = 1;
    while i < chars.len() - 1 {
        let c = chars[i];
        if c == '\\' {
            i += 1;
            let next = chars[i];
            out.push(match next {
                'b' => '\u{8}',
                'f' => '\u{c}',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'v' => '\u{b}',
                '0' => '\0',
                other => other,
            });
        } else {
            out.push(c);
        }
        i += 1;
    }
    out
}

/// Ports `class Reader`: reads tokens for a hand-written parser, with
/// Chevrotain-style messages on failure.
pub struct Reader<'a> {
    pub i: usize,
    pub tokens: Tokens,
    pub types: &'a [TokenType],
    pub diagram: &'static str,
}

impl<'a> Reader<'a> {
    pub fn new(tokens: Tokens, types: &'a [TokenType], diagram: &'static str) -> Reader<'a> {
        Reader {
            i: 0,
            tokens,
            types,
            diagram,
        }
    }

    /// `kind`: the current token's type, -1 at the end.
    pub fn kind(&self) -> i32 {
        self.tokens.kinds.get(self.i).copied().unwrap_or(-1)
    }

    /// The kind of token `index`.
    pub fn kind_at(&self, index: usize) -> i32 {
        self.tokens.kinds.get(index).copied().unwrap_or(-1)
    }

    /// `text(index)`.
    pub fn text_at(&self, index: usize) -> &str {
        self.tokens
            .src
            .slice(self.tokens.starts[index], self.tokens.ends[index])
    }

    /// `text()`.
    pub fn text(&self) -> &str {
        self.text_at(self.i)
    }

    /// `take`.
    pub fn take(&mut self) -> String {
        let s = String::from(self.text_at(self.i));
        self.i += 1;
        s
    }

    /// `accept`.
    pub fn accept(&mut self, kind: i32) -> bool {
        if self.kind() != kind {
            return false;
        }
        self.i += 1;
        true
    }

    /// `expect`.
    pub fn expect(&mut self, kind: i32) -> Result<String, PeleError> {
        if self.kind() != kind {
            let name = format!("token of type '{}'", self.types[kind as usize].name);
            return Err(self.fail(&name));
        }
        Ok(self.take())
    }

    /// `fail`: the error to throw.
    pub fn fail(&self, expected: &str) -> PeleError {
        let found = if self.kind() == -1 {
            String::from("end of input")
        } else {
            format!("`{}`", self.text())
        };
        let Tokens {
            src, starts, ends, ..
        } = &self.tokens;
        let mut at = starts[self.i];
        if self.kind() == -1 {
            // Locate EOF errors after the last token, before trailing line breaks and hidden comments.
            let mut k = self.i;
            while k > 0 && js::trim(src.slice(starts[k - 1], ends[k - 1])).is_empty() {
                k -= 1;
            }
            if k > 0 {
                at = ends[k - 1];
            }
        }
        syntax_error(
            self.diagram,
            &src.src,
            src.byte(at),
            &format!("Expecting {} but found {}.", expected, found),
            false,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_read_as_pele_reads_them() {
        assert_eq!(title_value("  title   A   long  one "), "A long one");
        assert_eq!(acc_title_value("accTitle : x  y"), "x y");
        assert_eq!(acc_descr_value("accDescr: hello"), "hello");
        assert_eq!(
            acc_descr_value("accDescr {\n   one  two  \n\n\n   three\n}"),
            "one two\nthree"
        );
        assert_eq!(string_value("\"a\\nb\""), "a\nb");
        let t = Text::new("---\na: 1\n---\nrest");
        assert_eq!(yaml_re(&t, 0), Some(13));
        let t = Text::new("%%{init: {}}%%\nx");
        assert_eq!(directive_re(&t, 0), Some(15));
    }
}
