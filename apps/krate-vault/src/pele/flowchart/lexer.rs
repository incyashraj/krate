//! Ports diagrams/flowchart/lexer.ts: the flowchart tokenizer, a hand
//! port of Mermaid's Jison lexer. Each regular expression Pele's lexer
//! uses is a function here that matches what it does, sticky at a
//! position.
//!
//! Positions are character indices (JS walks UTF-16 code units; every
//! test the lexer makes is on a character of the Basic Multilingual
//! Plane, and a character beyond it is a lone surrogate to JS, which no
//! test accepts, as none accepts it here).

use alloc::string::String;
use alloc::vec::Vec;

use super::tokens as t;
use crate::pele::util::unicode::is_unicode_letter;

/// Ports `interface Tokens`. `starts` are byte offsets in the source.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tokens {
    pub types: Vec<u8>,
    pub texts: Vec<String>,
    pub starts: Vec<usize>,
}

// `const enum S`.
const INITIAL: u8 = 0;
const STRING: u8 = 1;
const MD_STRING: u8 = 2;
const ACC_TITLE: u8 = 3;
const ACC_DESCR: u8 = 4;
const ACC_DESCR_MULTILINE: u8 = 5;
const DIR: u8 = 6;
const TEXT: u8 = 7;
const ELLIPSE_TEXT: u8 = 8;
const TRAP_TEXT: u8 = 9;
const EDGE_TEXT: u8 = 10;
const THICK_EDGE_TEXT: u8 = 11;
const DOTTED_EDGE_TEXT: u8 = 12;
const CLICK: u8 = 13;
const CALLBACKNAME: u8 = 14;
const CALLBACKARGS: u8 = 15;
const SHAPE_DATA: u8 = 16;
const SHAPE_DATA_STR: u8 = 17;

/// No character: JS's NaN past the end, which no test accepts.
const NONE: u32 = u32::MAX;

/// `NODE_CHAR`.
fn node_char(c: u32) -> bool {
    c < 128 && {
        let b = c as u8;
        b.is_ascii_alphanumeric() || b"!\"#$%&'*+.`?\\_/".contains(&b)
    }
}

/// util/chars.ts `isSpace` on a code.
fn is_space(c: u32) -> bool {
    c != NONE && char::from_u32(c).is_some_and(crate::pele::js::is_space)
}

/// util/chars.ts `isWord`.
fn is_word(c: u32) -> bool {
    c < 128 && ((c as u8).is_ascii_alphanumeric() || c == 95)
}

struct Lexer<'a> {
    src: &'a str,
    ch: Vec<u32>,
    off: Vec<usize>,
    /// Character index of each byte offset that starts a character.
    at_byte: Vec<usize>,
    n: usize,
    types: Vec<u8>,
    texts: Vec<String>,
    starts: Vec<usize>,
    stack: Vec<u8>,
    state: u8,
    p: usize,
    first_graph: bool,
    dir_next: Option<usize>,
    at_next: Option<usize>,
    line_end: isize,
    dir_failed: usize,
    run_end: usize,
    run_at: isize,
}

impl<'a> Lexer<'a> {
    fn c(&self, i: usize) -> u32 {
        self.ch.get(i).copied().unwrap_or(NONE)
    }

    fn slice(&self, s: usize, e: usize) -> &'a str {
        &self.src[self.off[s]..self.off[e]]
    }

    fn starts_with(&self, word: &str, p: usize) -> bool {
        p <= self.n && self.src[self.off[p]..].starts_with(word)
    }

    /// `src.indexOf(needle, from)` as a character index.
    fn index_of(&self, needle: &str, from: usize) -> Option<usize> {
        if from > self.n {
            return None;
        }
        self.src[self.off[from]..]
            .find(needle)
            .map(|k| self.at_byte[self.off[from] + k])
    }

    fn emit(&mut self, kind: u8, s: usize, e: usize) {
        self.types.push(kind);
        self.texts.push(self.slice(s, e).into());
        self.starts.push(self.off[s]);
        self.p = e;
    }

    fn push(&mut self, s: u8) {
        self.stack.push(self.state);
        self.state = s;
    }

    fn pop(&mut self) {
        if let Some(s) = self.stack.pop() {
            self.state = s;
        }
    }

    fn kw(&self, word: &str) -> bool {
        self.starts_with(word, self.p) && !is_word(self.c(self.p + word.chars().count()))
    }

    fn skip_ws(&self, mut q: usize) -> usize {
        while q < self.n && is_space(self.ch[q]) {
            q += 1;
        }
        q
    }

    /// `linkChar`: first character after any run of whitespace starting
    /// at q, skipping one optional x, o or <.
    fn link_char(&self, q: usize) -> u32 {
        let j = self.skip_ws(q);
        let d = self.c(j);
        if d == 120 || d == 111 || d == 60 {
            self.c(j + 1)
        } else {
            d
        }
    }

    fn invis_ahead(&self, q: usize) -> bool {
        self.c(self.skip_ws(q)) == 126
    }

    // ---- The sticky regular expressions -------------------------------

    /// /\s*[xo<]?/ then what `rest` matches.
    fn link_re(&self, q: usize, rest: impl Fn(&Self, usize) -> Option<usize>) -> Option<usize> {
        let j = self.skip_ws(q);
        let d = self.c(j);
        if d == 120 || d == 111 || d == 60 {
            if let Some(e) = rest(self, j + 1) {
                return Some(e);
            }
        }
        rest(self, j)
    }

    /// RE_LINK /\s*[xo<]?--+[-xo>]\s*/y (and thick, with `=`).
    fn re_long_link(&self, q: usize, dash: u32) -> Option<usize> {
        self.link_re(q, |l, j| {
            let mut k = j;
            while l.c(k) == dash {
                k += 1;
            }
            let run = k - j;
            let next = l.c(k);
            if run >= 2 && (next == 120 || next == 111 || next == 62) {
                Some(l.skip_ws(k + 1))
            } else if run >= 3 {
                Some(l.skip_ws(k))
            } else {
                None
            }
        })
    }

    /// RE_START_LINK /\s*[xo<]?--\s*/y (and thick).
    fn re_start_link(&self, q: usize, dash: u32) -> Option<usize> {
        self.link_re(q, |l, j| {
            if l.c(j) == dash && l.c(j + 1) == dash {
                Some(l.skip_ws(j + 2))
            } else {
                None
            }
        })
    }

    /// RE_DOTTED_LINK /\s*[xo<]?-?\.+-[xo>]?\s*/y.
    fn re_dotted_link(&self, q: usize) -> Option<usize> {
        self.link_re(q, |l, j| {
            let mut k = j;
            if l.c(k) == 45 && l.c(k + 1) == 46 {
                k += 1;
            }
            let dots = k;
            while l.c(k) == 46 {
                k += 1;
            }
            if k == dots || l.c(k) != 45 {
                return None;
            }
            k += 1;
            let x = l.c(k);
            if x == 120 || x == 111 || x == 62 {
                k += 1;
            }
            Some(l.skip_ws(k))
        })
    }

    /// RE_DOTTED_START /\s*[xo<]?-\.\s*/y.
    fn re_dotted_start(&self, q: usize) -> Option<usize> {
        self.link_re(q, |l, j| {
            if l.c(j) == 45 && l.c(j + 1) == 46 {
                Some(l.skip_ws(j + 2))
            } else {
                None
            }
        })
    }

    /// RE_INVIS_LINK /\s*~~~+\s*/y.
    fn re_invis_link(&self, q: usize) -> Option<usize> {
        let j = self.skip_ws(q);
        let mut k = j;
        while self.c(k) == 126 {
            k += 1;
        }
        if k - j >= 3 {
            Some(self.skip_ws(k))
        } else {
            None
        }
    }

    /// The link regular expression an edge-text state ends at.
    fn re_link_of(&self, kind: u8, q: usize) -> Option<usize> {
        match kind {
            EDGE_TEXT => self.re_long_link(q, 45),
            THICK_EDGE_TEXT => self.re_long_link(q, 61),
            _ => self.re_dotted_link(q),
        }
    }

    /// /word\s*sep\s*/y for accTitle / accDescr.
    fn re_word_sep(&self, q: usize, word: &str, sep: u32) -> Option<usize> {
        if !self.starts_with(word, q) {
            return None;
        }
        let j = self.skip_ws(q + word.chars().count());
        if self.c(j) != sep {
            return None;
        }
        Some(self.skip_ws(j + 1))
    }

    /// /word\s+/y for call / click.
    fn re_word_space(&self, q: usize, word: &str) -> Option<usize> {
        if !self.starts_with(word, q) {
            return None;
        }
        let j = q + word.chars().count();
        let e = self.skip_ws(j);
        if e > j {
            Some(e)
        } else {
            None
        }
    }

    /// RE_NODIR /(?:\r?\n)*\s*\n/y: the end of the last line break in the
    /// run of whitespace at q.
    fn re_nodir(&self, q: usize) -> Option<usize> {
        let end = self.skip_ws(q);
        (q..end).rev().find(|&k| self.ch[k] == 10).map(|k| k + 1)
    }

    /// RE_DIR /\s*(?:LR|RL|TB|BT|TD|BR)\b/y.
    fn re_dir(&self, q: usize) -> Option<usize> {
        let j = self.skip_ws(q);
        let a = self.c(j);
        let b = self.c(j + 1);
        let pair = |x: u8, y: u8| a == x as u32 && b == y as u32;
        let ok = pair(b'L', b'R')
            || pair(b'R', b'L')
            || pair(b'T', b'B')
            || pair(b'B', b'T')
            || pair(b'T', b'D')
            || pair(b'B', b'R');
        if ok && !is_word(self.c(j + 2)) {
            Some(j + 2)
        } else {
            None
        }
    }

    /// RE_DIR_SYM /\s*[<>^]/y.
    fn re_dir_sym(&self, q: usize) -> Option<usize> {
        let j = self.skip_ws(q);
        let a = self.c(j);
        if a == 60 || a == 62 || a == 94 {
            Some(j + 1)
        } else {
            None
        }
    }

    /// RE_DIR_V /\s*v\b/y.
    fn re_dir_v(&self, q: usize) -> Option<usize> {
        let j = self.skip_ws(q);
        if self.c(j) == 118 && !is_word(self.c(j + 1)) {
            Some(j + 1)
        } else {
            None
        }
    }

    /// RE_DIRECTION[i] /.*direction\s+XY[^\n]*/y: the last "direction" on
    /// this line that a run of whitespace and the two letters follow.
    fn re_direction(&self, q: usize, x: u8, y: u8) -> Option<usize> {
        let mut line = q;
        while line < self.n && !matches!(self.ch[line], 10 | 13 | 0x2028 | 0x2029) {
            line += 1;
        }
        let word = [100u32, 105, 114, 101, 99, 116, 105, 111, 110]; // direction
        let mut k = line as isize - 9;
        while k >= q as isize {
            let ku = k as usize;
            if (0..9).all(|i| self.c(ku + i) == word[i]) {
                let s = ku + 9;
                let j = self.skip_ws(s);
                if j > s && self.c(j) == x as u32 && self.c(j + 1) == y as u32 {
                    let mut e = j + 2;
                    while e < self.n && self.ch[e] != 10 {
                        e += 1;
                    }
                    return Some(e);
                }
            }
            k -= 1;
        }
        None
    }

    /// RE_NEWLINE /(?:\r?\n)+/y.
    fn re_newline(&self, q: usize) -> Option<usize> {
        let mut k = q;
        loop {
            if self.c(k) == 10 {
                k += 1;
            } else if self.c(k) == 13 && self.c(k + 1) == 10 {
                k += 2;
            } else {
                break;
            }
        }
        if k > q {
            Some(k)
        } else {
            None
        }
    }

    /// RE_SPACE /[^\S\n\r]+/y.
    fn re_space(&self, q: usize) -> Option<usize> {
        let mut k = q;
        while k < self.n && is_space(self.ch[k]) && self.ch[k] != 10 && self.ch[k] != 13 {
            k += 1;
        }
        if k > q {
            Some(k)
        } else {
            None
        }
    }

    /// RE_EMPTY_CALL /\(\s*\)/y.
    fn re_empty_call(&self, q: usize) -> Option<usize> {
        if self.c(q) != 40 {
            return None;
        }
        let j = self.skip_ws(q + 1);
        if self.c(j) == 41 {
            Some(j + 1)
        } else {
            None
        }
    }

    // ---- The lexer's closures -----------------------------------------

    fn quote(&mut self) {
        if self.c(self.p + 1) == 96 {
            self.push(MD_STRING);
            self.p += 2;
        } else {
            self.push(STRING);
            self.p += 1;
        }
    }

    fn invis_link(&mut self) -> bool {
        if !self.invis_ahead(self.p) {
            return false;
        }
        match self.re_invis_link(self.p) {
            Some(e) => {
                self.emit(t::LINK, self.p, e);
                true
            }
            None => false,
        }
    }

    fn opener(&mut self, c: u32) -> bool {
        let p = self.p;
        let d = self.c(p + 1);
        if c == 40 {
            if d == 45 {
                self.push(ELLIPSE_TEXT);
                self.emit(t::ELLIPSE_START, p, p + 2);
            } else if d == 91 {
                self.push(TEXT);
                self.emit(t::STADIUMSTART, p, p + 2);
            } else if d == 40 && self.c(p + 2) == 40 {
                self.push(TEXT);
                self.emit(t::DOUBLECIRCLESTART, p, p + 3);
            } else {
                self.push(TEXT);
                self.emit(t::PS, p, p + 1);
            }
            return true;
        }
        if c == 91 {
            if d == 91 {
                self.push(TEXT);
                self.emit(t::SUBROUTINESTART, p, p + 2);
            } else if d == 40 {
                self.push(TEXT);
                self.emit(t::CYLINDERSTART, p, p + 2);
            } else if d == 47 {
                self.push(TRAP_TEXT);
                self.emit(t::TRAPSTART, p, p + 2);
            } else if d == 92 {
                self.push(TRAP_TEXT);
                self.emit(t::INVTRAPSTART, p, p + 2);
            } else {
                self.push(TEXT);
                self.emit(t::SQS, p, p + 1);
            }
            return true;
        }
        if c == 124 {
            self.push(TEXT);
            self.emit(t::PIPE, p, p + 1);
            return true;
        }
        if c == 123 {
            self.push(TEXT);
            self.emit(t::DIAMOND_START, p, p + 1);
            return true;
        }
        false
    }

    fn graph(&mut self, len: usize) {
        if self.first_graph {
            self.first_graph = false;
            self.push(DIR);
        }
        self.emit(t::GRAPH, self.p, self.p + len);
    }

    fn node_string(&mut self) -> bool {
        let p = self.p;
        let mut q = p;
        while q < self.n {
            let ch = self.ch[q];
            if node_char(ch) {
                q += 1;
            } else if ch == 45 && q + 1 < self.n {
                let d = self.ch[q + 1];
                if d == 62 || d == 45 || d == 46 {
                    break;
                }
                q += 1;
            } else {
                break;
            }
        }
        if q == p {
            return false;
        }
        self.emit(t::NODE_STRING, p, q);
        true
    }

    fn link(&mut self, c: u32) -> bool {
        let p = self.p;
        let d = self.link_char(p);
        if d == 45 {
            if let Some(e) = self.re_long_link(p, 45) {
                self.emit(t::LINK, p, e);
                return true;
            }
            if let Some(e) = self.re_start_link(p, 45) {
                self.push(EDGE_TEXT);
                self.emit(t::START_LINK, p, e);
                return true;
            }
            if let Some(e) = self.re_dotted_link(p) {
                self.emit(t::LINK, p, e);
                return true;
            }
            if let Some(e) = self.re_dotted_start(p) {
                self.push(DOTTED_EDGE_TEXT);
                self.emit(t::START_LINK, p, e);
                return true;
            }
        } else if d == 61 {
            if let Some(e) = self.re_long_link(p, 61) {
                self.emit(t::LINK, p, e);
                return true;
            }
            if let Some(e) = self.re_start_link(p, 61) {
                self.push(THICK_EDGE_TEXT);
                self.emit(t::START_LINK, p, e);
                return true;
            }
        } else if d == 46 {
            if let Some(e) = self.re_dotted_link(p) {
                self.emit(t::LINK, p, e);
                return true;
            }
        }
        c != 45 && c != 61 && c != 46 && self.invis_link()
    }

    // The switch keeps the TypeScript's shape: a case whose test fails
    // falls through to the checks after it.
    #[allow(clippy::collapsible_match)]
    fn initial(&mut self, c: u32) -> bool {
        let p = self.p;
        match c {
            97 => {
                if self.starts_with("acc", p) {
                    if let Some(e) = self.re_word_sep(p, "accTitle", 58) {
                        self.push(ACC_TITLE);
                        self.emit(t::acc_title, p, e);
                        return true;
                    }
                    if let Some(e) = self.re_word_sep(p, "accDescr", 58) {
                        self.push(ACC_DESCR);
                        self.emit(t::acc_descr, p, e);
                        return true;
                    }
                    if let Some(e) = self.re_word_sep(p, "accDescr", 123) {
                        self.push(ACC_DESCR_MULTILINE);
                        self.p = e;
                        return true;
                    }
                }
            }
            64 => {
                if self.c(p + 1) == 123 {
                    self.push(SHAPE_DATA);
                    self.types.push(t::SHAPE_DATA);
                    self.texts.push(String::new());
                    self.starts.push(self.off[p]);
                    self.p += 2;
                    return true;
                }
            }
            99 => {
                if let Some(e) = self.re_word_space(p, "call") {
                    self.push(CALLBACKNAME);
                    self.p = e;
                    return true;
                }
                if self.kw("classDef") {
                    self.emit(t::CLASSDEF, p, p + 8);
                    return true;
                }
                if self.kw("class") {
                    self.emit(t::CLASS, p, p + 5);
                    return true;
                }
                if let Some(e) = self.re_word_space(p, "click") {
                    self.push(CLICK);
                    self.p = e;
                    return true;
                }
            }
            34 => {
                self.quote();
                return true;
            }
            115 => {
                if self.kw("style") {
                    self.emit(t::STYLE, p, p + 5);
                    return true;
                }
                if self.kw("swimlane-beta") {
                    self.graph(13);
                    return true;
                }
                if self.kw("subgraph") {
                    self.emit(t::subgraph, p, p + 8);
                    return true;
                }
            }
            100 => {
                if self.kw("default") {
                    self.emit(t::DEFAULT, p, p + 7);
                    return true;
                }
            }
            108 => {
                if self.kw("linkStyle") {
                    self.emit(t::LINKSTYLE, p, p + 9);
                    return true;
                }
            }
            105 => {
                if self.kw("interpolate") {
                    self.emit(t::INTERPOLATE, p, p + 11);
                    return true;
                }
            }
            104 => {
                if self.starts_with("href", p) && is_space(self.c(p + 4)) {
                    self.emit(t::HREF, p, p + 5);
                    return true;
                }
            }
            102 => {
                if self.kw("flowchart-elk") {
                    self.graph(13);
                    return true;
                }
                if self.kw("flowchart") {
                    self.graph(9);
                    return true;
                }
            }
            103 => {
                if self.kw("graph") {
                    self.graph(5);
                    return true;
                }
            }
            101 => {
                if self.kw("end") {
                    let e = self.skip_ws(p + 3);
                    self.emit(t::end, p, e);
                    return true;
                }
            }
            95 => {
                for (word, len) in [("_self", 5), ("_blank", 6), ("_parent", 7), ("_top", 4)] {
                    if self.kw(word) {
                        self.emit(t::LINK_TARGET, p, p + len);
                        return true;
                    }
                }
            }
            _ => {}
        }

        if self.dir_next.is_some() && p >= self.dir_failed {
            if self.dir_next.is_some_and(|d| d < p) {
                self.dir_next = self.index_of("direction", p);
            }
            if let Some(dn) = self.dir_next {
                if self.line_end < p as isize {
                    let mut k = p;
                    while k < self.n && !matches!(self.ch[k], 10 | 13 | 0x2028 | 0x2029) {
                        k += 1;
                    }
                    self.line_end = k as isize;
                }
                if (dn as isize) < self.line_end {
                    for (i, (x, y)) in [
                        (b'T', b'B'),
                        (b'B', b'T'),
                        (b'R', b'L'),
                        (b'L', b'R'),
                        (b'T', b'D'),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if let Some(e) = self.re_direction(p, x, y) {
                            self.emit(t::direction_tb + i as u8, p, e);
                            return true;
                        }
                    }
                    // No direction statement starts anywhere in the rest of this line either.
                    self.dir_failed = self.line_end as usize;
                }
            }
        }

        // A link id is a run of non-space characters up to its last `@` that is not followed by `{` or `"`.
        if self.at_next.is_some() && c != 34 && !is_space(c) {
            if p >= self.run_end {
                if self.at_next.is_some_and(|a| a < p) {
                    self.at_next = self.index_of("@", p);
                }
                let mut q = p;
                while q < self.n {
                    let ch = self.ch[q];
                    if ch == 34 || is_space(ch) {
                        break;
                    }
                    q += 1;
                }
                self.run_end = q;
                self.run_at = -1;
                if let Some(an) = self.at_next.filter(|a| *a < q) {
                    let mut k = q.min(self.n.saturating_sub(1)) as isize - 1;
                    while k >= an as isize {
                        let ku = k as usize;
                        if self.ch[ku] == 64 {
                            let after = self.c(ku + 1);
                            if after != 123 && after != 34 {
                                self.run_at = k;
                                break;
                            }
                        }
                        k -= 1;
                    }
                }
            }
            if self.run_at > p as isize {
                let e = self.run_at as usize + 1;
                self.emit(t::LINK_ID, p, e);
                return true;
            }
        }

        if (48..=57).contains(&c) {
            let mut q = p + 1;
            while q < self.n && (48..=57).contains(&self.ch[q]) {
                q += 1;
            }
            self.emit(t::NUM, p, q);
            return true;
        }

        match c {
            35 => {
                self.emit(t::BRKT, p, p + 1);
                return true;
            }
            58 => {
                if self.c(p + 1) == 58 && self.c(p + 2) == 58 {
                    self.emit(t::STYLE_SEPARATOR, p, p + 3);
                } else {
                    self.emit(t::COLON, p, p + 1);
                }
                return true;
            }
            38 => {
                self.emit(t::AMP, p, p + 1);
                return true;
            }
            59 => {
                self.emit(t::SEMI, p, p + 1);
                return true;
            }
            44 => {
                self.emit(t::COMMA, p, p + 1);
                return true;
            }
            42 => {
                self.emit(t::MULT, p, p + 1);
                return true;
            }
            120 | 111 => return self.link(c) || self.node_string(),
            60 => {
                if self.link(c) {
                    return true;
                }
                self.emit(t::TAGSTART, p, p + 1);
                return true;
            }
            45 => {
                if self.link(c) || self.node_string() {
                    return true;
                }
                self.emit(t::MINUS, p, p + 1);
                return true;
            }
            61 => return self.link(c),
            46 => return self.link(c) || self.node_string(),
            126 => return self.invis_link(),
            40 => return self.opener(c),
            91 => {
                if self.c(p + 1) == 124 {
                    self.emit(t::VERTEX_WITH_PROPS_START, p, p + 2);
                    return true;
                }
                return self.opener(c);
            }
            124 | 123 => return self.opener(c),
            62 => {
                self.push(TEXT);
                self.emit(t::TAGEND, p, p + 1);
                return true;
            }
            94 => {
                self.emit(t::UP, p, p + 1);
                return true;
            }
            92 => {
                if self.c(p + 1) == 124 {
                    self.emit(t::SEP, p, p + 2);
                    return true;
                }
                return self.node_string();
            }
            118 => {
                if !is_word(self.c(p + 1)) {
                    self.emit(t::DOWN, p, p + 1);
                    return true;
                }
                return self.node_string();
            }
            _ => {}
        }

        if is_space(c) {
            if self.link(c) {
                return true;
            }
            if let Some(e) = self.re_newline(p) {
                self.emit(t::NEWLINE, p, e);
                return true;
            }
            if let Some(e) = self.re_space(p) {
                self.emit(t::SPACE, p, e);
                return true;
            }
            return false;
        }

        if c < 128 {
            return self.node_string();
        }
        if is_unicode_letter(c) {
            self.emit(t::UNICODE_TEXT, p, p + 1);
            return true;
        }
        false
    }

    fn edge_text(&mut self, c: u32, kind: u8, a: u32, b: u32) -> bool {
        if c == 34 {
            self.quote();
            return true;
        }
        let p = self.p;
        let d = self.link_char(p);
        if d == a || d == b {
            if let Some(e) = self.re_link_of(kind, p) {
                self.pop();
                self.emit(t::LINK, p, e);
                return true;
            }
        }
        if c == a && (a != 45 || self.c(p + 1) == 45) {
            return false;
        }
        let mut q = p + 1;
        while q < self.n {
            let ch = self.ch[q];
            if ch == 34 {
                break;
            }
            if ch == a {
                if a != 45 || self.c(q + 1) == 45 {
                    break;
                }
            } else if ch == 120 || ch == 111 || ch == 60 || ch == b || is_space(ch) {
                let dd = self.link_char(q);
                if (dd == a || dd == b) && self.re_link_of(kind, q).is_some() {
                    break;
                }
            }
            q += 1;
        }
        self.emit(t::EDGE_TEXT, p, q);
        true
    }

    fn run(mut self) -> Tokens {
        loop {
            let n = self.n;
            if self.p >= n {
                let kind = match self.state {
                    INITIAL => Some(t::EOF),
                    ACC_TITLE => Some(t::acc_title_value),
                    ACC_DESCR => Some(t::acc_descr_value),
                    ACC_DESCR_MULTILINE => Some(t::acc_descr_multiline_value),
                    CLICK => Some(t::CLICK),
                    CALLBACKNAME => Some(t::CALLBACKNAME),
                    CALLBACKARGS => Some(t::CALLBACKARGS),
                    _ => None,
                };
                if let Some(k) = kind {
                    self.emit(k, n, n);
                }
                break;
            }
            let p = self.p;
            let c = self.ch[p];
            let mut ok = true;
            match self.state {
                INITIAL => ok = self.initial(c),
                STRING => {
                    if c == 34 {
                        if self.c(p + 1) == 96 {
                            self.push(MD_STRING);
                            self.p += 2;
                        } else {
                            self.pop();
                            self.p += 1;
                        }
                    } else {
                        let e = self.index_of("\"", p).unwrap_or(n);
                        self.emit(t::STR, p, e);
                    }
                }
                MD_STRING => {
                    if c == 96 {
                        if self.c(p + 1) == 34 {
                            self.pop();
                            self.p += 2;
                        } else {
                            ok = false;
                        }
                    } else if c == 34 {
                        self.quote();
                    } else {
                        let mut q = p + 1;
                        while q < n && self.ch[q] != 96 && self.ch[q] != 34 {
                            q += 1;
                        }
                        self.emit(t::MD_STR, p, q);
                    }
                }
                ACC_TITLE | ACC_DESCR => {
                    let e = self.index_of("\n", p).unwrap_or(n);
                    let kind = if self.state == ACC_TITLE {
                        t::acc_title_value
                    } else {
                        t::acc_descr_value
                    };
                    self.pop();
                    self.emit(kind, p, e);
                }
                ACC_DESCR_MULTILINE => {
                    if c == 125 {
                        self.pop();
                        self.p += 1;
                    } else {
                        let e = self.index_of("}", p).unwrap_or(n);
                        self.emit(t::acc_descr_multiline_value, p, e);
                    }
                }
                DIR => {
                    if c == 34 {
                        self.quote();
                    } else if let Some(e) = self.re_nodir(p) {
                        self.pop();
                        self.emit(t::NODIR, p, e);
                    } else if let Some(e) = self
                        .re_dir(p)
                        .or_else(|| self.re_dir_sym(p))
                        .or_else(|| self.re_dir_v(p))
                    {
                        self.pop();
                        self.emit(t::DIR, p, e);
                    } else {
                        ok = self.invis_link() || self.opener(c);
                    }
                }
                TEXT => {
                    let d = self.c(p + 1);
                    match c {
                        34 => self.quote(),
                        40 | 91 | 123 => {
                            self.opener(c);
                        }
                        93 => {
                            self.pop();
                            if d == 41 {
                                self.emit(t::STADIUMEND, p, p + 2);
                            } else if d == 93 {
                                self.emit(t::SUBROUTINEEND, p, p + 2);
                            } else {
                                self.emit(t::SQE, p, p + 1);
                            }
                        }
                        41 => {
                            self.pop();
                            if d == 93 {
                                self.emit(t::CYLINDEREND, p, p + 2);
                            } else if d == 41 && self.c(p + 2) == 41 {
                                self.emit(t::DOUBLECIRCLEEND, p, p + 3);
                            } else {
                                self.emit(t::PE, p, p + 1);
                            }
                        }
                        124 => {
                            self.pop();
                            self.emit(t::PIPE, p, p + 1);
                        }
                        125 => {
                            self.pop();
                            self.emit(t::DIAMOND_STOP, p, p + 1);
                        }
                        _ => {
                            if (c == 126 || is_space(c)) && self.invis_link() {
                            } else {
                                let mut q = p + 1;
                                while q < n
                                    && !matches!(
                                        self.ch[q],
                                        91 | 93 | 40 | 41 | 123 | 125 | 124 | 34
                                    )
                                {
                                    q += 1;
                                }
                                self.emit(t::TEXT, p, q);
                            }
                        }
                    }
                }
                ELLIPSE_TEXT => {
                    let d = self.c(p + 1);
                    if c == 34 {
                        self.quote();
                    } else if (c == 126 || is_space(c)) && self.invis_link() {
                    } else if (c == 45 || c == 47 || c == 41) && d == 41 {
                        self.pop();
                        self.emit(t::ELLIPSE_END, p, p + 2);
                    } else if c == 40 || c == 91 || c == 123 {
                        self.opener(c);
                    } else if c == 41 || c == 93 || c == 125 {
                        ok = false;
                    } else {
                        let mut q = p + 1;
                        while q < n {
                            let ch = self.ch[q];
                            if matches!(ch, 34 | 40 | 41 | 91 | 93 | 123 | 125) {
                                break;
                            }
                            if (ch == 45 || ch == 47) && self.c(q + 1) == 41 {
                                break;
                            }
                            if (ch == 126 || is_space(ch))
                                && self.invis_ahead(q)
                                && self.re_invis_link(q).is_some()
                            {
                                break;
                            }
                            q += 1;
                        }
                        self.emit(t::TEXT, p, q);
                    }
                }
                TRAP_TEXT => {
                    let d = self.c(p + 1);
                    if c == 34 {
                        self.quote();
                    } else if (c == 126 || is_space(c)) && self.invis_link() {
                    } else {
                        match c {
                            40 => {
                                if d == 45 || d == 91 || (d == 40 && self.c(p + 2) == 40) {
                                    self.opener(c);
                                } else if d == 93 {
                                    self.pop();
                                    self.emit(t::TRAPEND, p, p + 2);
                                } else {
                                    self.opener(c);
                                }
                            }
                            91 | 123 => {
                                self.opener(c);
                            }
                            92 => {
                                if d == 93 {
                                    self.pop();
                                    self.emit(t::TRAPEND, p, p + 2);
                                } else {
                                    self.emit(t::TEXT, p, p + 1);
                                }
                            }
                            47 => {
                                if d == 93 {
                                    self.pop();
                                    self.emit(t::INVTRAPEND, p, p + 2);
                                } else {
                                    self.emit(t::TEXT, p, p + 1);
                                }
                            }
                            93 | 41 => {
                                if d == 93 {
                                    self.pop();
                                    self.emit(t::TRAPEND, p, p + 2);
                                } else {
                                    ok = false;
                                }
                            }
                            125 => ok = false,
                            _ => {
                                if (c == 63 || c == 61) && d == 93 {
                                    self.pop();
                                    self.emit(t::TRAPEND, p, p + 2);
                                } else {
                                    let mut q = p + 1;
                                    while q < n
                                        && !matches!(
                                            self.ch[q],
                                            92 | 91 | 93 | 40 | 41 | 123 | 125 | 47
                                        )
                                    {
                                        q += 1;
                                    }
                                    self.emit(t::TEXT, p, q);
                                }
                            }
                        }
                    }
                }
                EDGE_TEXT => ok = self.edge_text(c, EDGE_TEXT, 45, NONE - 1),
                THICK_EDGE_TEXT => ok = self.edge_text(c, THICK_EDGE_TEXT, 61, NONE - 1),
                DOTTED_EDGE_TEXT => ok = self.edge_text(c, DOTTED_EDGE_TEXT, 46, 45),
                CLICK => {
                    if c == 34 {
                        self.quote();
                    } else if is_space(c) {
                        self.pop();
                        self.p += 1;
                    } else {
                        let mut q = p + 1;
                        while q < n && !is_space(self.ch[q]) {
                            q += 1;
                        }
                        self.emit(t::CLICK, p, q);
                    }
                }
                CALLBACKNAME => {
                    if c == 40 {
                        let e = self.re_empty_call(p);
                        self.pop();
                        match e {
                            Some(e) => self.p = e,
                            None => {
                                self.push(CALLBACKARGS);
                                self.p += 1;
                            }
                        }
                    } else {
                        let e = self.index_of("(", p).unwrap_or(n);
                        self.emit(t::CALLBACKNAME, p, e);
                    }
                }
                CALLBACKARGS => {
                    if c == 41 {
                        self.pop();
                        self.p += 1;
                    } else {
                        let e = self.index_of(")", p).unwrap_or(n);
                        self.emit(t::CALLBACKARGS, p, e);
                    }
                }
                SHAPE_DATA => {
                    if c == 34 {
                        self.push(SHAPE_DATA_STR);
                        self.emit(t::SHAPE_DATA, p, p + 1);
                    } else if c == 125 {
                        self.pop();
                        self.p += 1;
                    } else if c == 94 {
                        ok = false;
                    } else {
                        let mut q = p + 1;
                        while q < n && !matches!(self.ch[q], 125 | 94 | 34) {
                            q += 1;
                        }
                        self.emit(t::SHAPE_DATA, p, q);
                    }
                }
                SHAPE_DATA_STR => {
                    if c == 34 {
                        self.pop();
                        self.emit(t::SHAPE_DATA, p, p + 1);
                    } else {
                        let e = self.index_of("\"", p).unwrap_or(n);
                        self.types.push(t::SHAPE_DATA);
                        self.texts.push(br_lines(self.slice(p, e)));
                        self.starts.push(self.off[p]);
                        self.p = e;
                    }
                }
                _ => {}
            }
            if !ok {
                self.types.push(t::ERROR);
                self.texts.push(String::new());
                self.starts.push(self.off[self.p]);
                return Tokens {
                    types: self.types,
                    texts: self.texts,
                    starts: self.starts,
                };
            }
        }
        self.types.push(t::END);
        self.texts.push(String::new());
        self.starts.push(self.src.len());
        Tokens {
            types: self.types,
            texts: self.texts,
            starts: self.starts,
        }
    }
}

/// `text.replace(RE_BR, '<br/>')`: RE_BR = /\n\s*/g.
fn br_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\n' {
            out.push_str("<br/>");
            while it.peek().is_some_and(|c| crate::pele::js::is_space(*c)) {
                it.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Ports `tokenize`.
pub fn tokenize(src: &str) -> Tokens {
    let mut ch = Vec::with_capacity(src.len());
    let mut off = Vec::with_capacity(src.len() + 1);
    let mut at_byte = alloc::vec![0usize; src.len() + 1];
    for (i, c) in src.char_indices() {
        at_byte[i] = ch.len();
        ch.push(c as u32);
        off.push(i);
    }
    at_byte[src.len()] = ch.len();
    off.push(src.len());
    let n = ch.len();
    let lexer = Lexer {
        src,
        ch,
        off,
        at_byte,
        n,
        types: Vec::new(),
        texts: Vec::new(),
        starts: Vec::new(),
        stack: Vec::new(),
        state: INITIAL,
        p: 0,
        first_graph: true,
        dir_next: None,
        at_next: None,
        line_end: -1,
        dir_failed: 0,
        run_end: 0,
        run_at: -1,
    };
    let mut lexer = lexer;
    lexer.dir_next = lexer.index_of("direction", 0);
    lexer.at_next = lexer.index_of("@", 0);
    lexer.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_of_a_small_chart() {
        let tk = tokenize("flowchart TD\n    A[Start] -->|Yes| B\n");
        assert_eq!(
            tk.types,
            [
                t::GRAPH,
                t::DIR,
                t::NEWLINE,
                t::SPACE,
                t::NODE_STRING,
                t::SQS,
                t::TEXT,
                t::SQE,
                t::LINK,
                t::PIPE,
                t::TEXT,
                t::PIPE,
                t::SPACE,
                t::NODE_STRING,
                t::NEWLINE,
                t::EOF,
                t::END
            ]
        );
        assert_eq!(tk.texts[8], " -->");
    }
}
