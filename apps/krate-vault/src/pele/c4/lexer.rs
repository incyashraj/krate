//! Ports diagrams/c4/lexer.ts: follows the lexer Jison compiles from
//! c4Diagram.jison, its rule order, its start conditions, and what it
//! returns once the input runs out in each of them. Positions are
//! character indices of a [`Text`].

use alloc::vec::Vec;

use super::tokens as t;
use crate::pele::common::chars::{is_line_end, is_space, is_word, Text};

/// Ports `interface Tokens`: token text is `src.slice(starts[i], ends[i])`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tokens {
    pub types: Vec<u8>,
    pub starts: Vec<usize>,
    pub ends: Vec<usize>,
}

// `const enum S`.
const INITIAL: u8 = 0;
const STRUCT: u8 = 1;
const ATTRIBUTE: u8 = 2;
const STRING: u8 = 3;
const STRING_KV: u8 = 4;
const STRING_KV_KEY: u8 = 5;
const STRING_KV_VALUE: u8 = 6;
const ACC: u8 = 7;
const ACC_DESCR_MULTILINE: u8 = 8;

/// The direction words of `RE_DIRECTION`, in order.
const DIRECTIONS: [&str; 4] = ["TB", "BT", "RL", "LR"];

struct Lexer<'a> {
    src: &'a Text,
    n: usize,
    out: Tokens,
    p: usize,
}

impl Lexer<'_> {
    fn c(&self, i: usize) -> u32 {
        self.src.c(i)
    }

    fn emit(&mut self, kind: u8, s: usize, e: usize) {
        self.out.types.push(kind);
        self.out.starts.push(s);
        self.out.ends.push(e);
        self.p = e;
    }

    fn skip_ws(&self, mut q: usize) -> usize {
        while q < self.n && is_space(self.c(q)) {
            q += 1;
        }
        q
    }

    fn up_to(&self, ch: u32, from: usize) -> usize {
        self.src.find_char(ch, from).unwrap_or(self.n)
    }

    /// Ports `statement`: `word\s[^#\n;]+`.
    fn statement(&mut self, word: &str, kind: u8) -> bool {
        let mut q = self.p + word.len();
        if !self.src.starts_with(word, self.p) || !is_space(self.c(q)) {
            return false;
        }
        q += 1;
        let from = q;
        while q < self.n {
            let c = self.c(q);
            if c == 35 || c == 10 || c == 59 {
                break;
            }
            q += 1;
        }
        if q == from {
            return false;
        }
        let p = self.p;
        self.emit(kind, p, q);
        true
    }

    /// `/.*direction\s+XX[^\n]*/y` at `p`.
    fn direction_re(&self, word: &str) -> Option<usize> {
        let p = self.p;
        let mut line = p;
        while line < self.n && !is_line_end(self.c(line)) {
            line += 1;
        }
        let mut q = line;
        loop {
            let mut found = None;
            let mut k = q;
            while k > p {
                k -= 1;
                if k + 9 <= line && self.src.starts_with("direction", k) {
                    found = Some(k);
                    break;
                }
            }
            let k = found?;
            let mut r = k + 9;
            let ws = r;
            while r < self.n && is_space(self.c(r)) {
                r += 1;
            }
            if r > ws && self.src.starts_with(word, r) {
                let mut e = r + 2;
                while e < self.n && self.c(e) != 10 {
                    e += 1;
                }
                return Some(e);
            }
            q = k;
        }
    }

    /// `/title[^\S\r\n]*(?=[\r\n]|$)\s*/y`: a title with nothing after it,
    /// with the line breaks after it.
    fn bare_title(&self) -> Option<usize> {
        if !self.src.starts_with("title", self.p) {
            return None;
        }
        let mut q = self.p + 5;
        while q < self.n && is_space(self.c(q)) && self.c(q) != 13 && self.c(q) != 10 {
            q += 1;
        }
        if q < self.n && self.c(q) != 13 && self.c(q) != 10 {
            return None;
        }
        Some(self.skip_ws(q))
    }
}

/// Ports `tokenize`.
pub fn tokenize(source: &str) -> Tokens {
    let src = Text::new(source);
    let n = src.len();
    let mut lx = Lexer {
        src: &src,
        n,
        out: Tokens::default(),
        p: 0,
    };
    let mut state = INITIAL;
    let mut acc_value = t::acc_title_value;
    let mut done = false;
    let mut dir_next: isize = -2;
    let mut line_end: isize = -1;
    let mut dir_failed = 0usize;

    loop {
        // Jison notes the end of input one call before it reports it, so a rule that matches
        // nothing gets one last turn.
        if done {
            lx.emit(t::END, n, n);
            break;
        }
        let p = lx.p;
        if p >= n {
            done = true;
        }
        let c = lx.c(p);

        if state == INITIAL {
            if p >= n {
                lx.emit(t::EOF, n, n);
                continue;
            }
            if dir_next != -1 && p >= dir_failed {
                if dir_next < p as isize {
                    dir_next = src.index_of("direction", p).map_or(-1, |k| k as isize);
                }
                if dir_next != -1 {
                    if line_end < p as isize {
                        let mut q = p;
                        while q < n && !is_line_end(lx.c(q)) {
                            q += 1;
                        }
                        line_end = q as isize;
                    }
                    if dir_next < line_end {
                        let mut matched = false;
                        for (i, word) in DIRECTIONS.iter().enumerate() {
                            if let Some(e) = lx.direction_re(word) {
                                lx.emit(t::direction_tb + i as u8, p, e);
                                matched = true;
                                break;
                            }
                        }
                        if matched {
                            continue;
                        }
                        // No direction statement starts anywhere in the rest of this line either.
                        dir_failed = line_end as usize;
                    }
                }
            }
            if c == 116 {
                if let Some(e) = lx.bare_title() {
                    lx.p = e;
                    continue;
                }
                if lx.statement("title", t::title) {
                    continue;
                }
            }
            if c == 97 {
                if lx.statement("accDescription", t::accDescription) {
                    continue;
                }
                let title = src.starts_with("accTitle", p);
                if title || src.starts_with("accDescr", p) {
                    let q = lx.skip_ws(p + 8);
                    let d = lx.c(q);
                    if d == 58 {
                        let e = lx.skip_ws(q + 1);
                        lx.emit(if title { t::acc_title } else { t::acc_descr }, p, e);
                        acc_value = if title {
                            t::acc_title_value
                        } else {
                            t::acc_descr_value
                        };
                        state = ACC;
                        continue;
                    }
                    if d == 123 && !title {
                        lx.p = lx.skip_ws(q + 1);
                        state = ACC_DESCR_MULTILINE;
                        continue;
                    }
                }
            }
            if c == 37 && lx.c(p + 1) == 37 {
                // A comment takes the line breaks after it, so no NEWLINE follows.
                let mut q = lx.up_to(10, p + 2);
                while q < n && (lx.c(q) == 10 || lx.c(q) == 13) {
                    q += 1;
                }
                lx.p = q;
                continue;
            }
            if is_space(c) {
                let mut q = p;
                let mut last: Option<usize> = None;
                while q < n {
                    let d = lx.c(q);
                    if !is_space(d) {
                        break;
                    }
                    if d == 10 {
                        last = Some(q);
                    }
                    q += 1;
                }
                match last {
                    Some(l) => lx.emit(t::NEWLINE, p, l + 1),
                    None => lx.p = q,
                }
                continue;
            }
            if c == 123 {
                lx.emit(t::LBRACE, p, p + 1);
                continue;
            }
            if c == 125 {
                lx.emit(t::RBRACE, p, p + 1);
                continue;
            }
            if is_word(c) {
                let mut q = p + 1;
                while q < n && is_word(lx.c(q)) {
                    q += 1;
                }
                let kind = if q - p <= 19 {
                    t::keyword(src.slice(p, q))
                } else {
                    None
                };
                if let Some(kind) = kind {
                    if kind > t::C4_DEPLOYMENT {
                        state = STRUCT;
                    }
                    lx.emit(kind, p, q);
                    continue;
                }
            }
            lx.emit(t::ERROR, p, p);
            break;
        }

        if state == STRUCT {
            if p >= n {
                lx.emit(t::EOF_IN_STRUCT, n, n);
            } else if c == 40 {
                let mut q = p + 1;
                while lx.c(q) == 32 {
                    q += 1;
                }
                if lx.c(q) == 44 {
                    lx.emit(t::ATTRIBUTE_EMPTY, p, q + 1);
                } else {
                    lx.p += 1;
                }
                state = ATTRIBUTE;
            } else if c == 41 {
                lx.p += 1;
                state = INITIAL;
            } else {
                lx.emit(t::ERROR, p, p);
                break;
            }
            continue;
        }

        if state == ATTRIBUTE {
            if p >= n {
                continue;
            }
            if c == 41 {
                lx.p += 1;
                state = INITIAL;
            } else if c == 44 {
                if lx.c(p + 1) == 44 {
                    lx.emit(t::ATTRIBUTE_EMPTY, p, p + 2);
                } else {
                    lx.p += 1;
                }
            } else {
                let mut q = p;
                while lx.c(q) == 32 {
                    q += 1;
                }
                let d = lx.c(q);
                if d == 34 {
                    if lx.c(q + 1) == 34 {
                        lx.emit(t::ATTRIBUTE_EMPTY, p, q + 2);
                    } else {
                        lx.p = q + 1;
                        state = STRING;
                    }
                } else if d == 36 {
                    lx.p = q + 1;
                    state = STRING_KV;
                } else {
                    // An unquoted value runs to the next comma, through closing parentheses and line breaks.
                    let e = lx.up_to(44, p);
                    lx.emit(t::STR, p, e);
                }
            }
            continue;
        }

        if state == STRING {
            if c == 34 {
                lx.p += 1;
                state = ATTRIBUTE;
            } else {
                let e = lx.up_to(34, p);
                lx.emit(t::STR, p, e);
            }
            continue;
        }

        if state == STRING_KV {
            let e = lx.up_to(61, p);
            lx.emit(t::STR_KEY, p, e);
            state = STRING_KV_KEY;
            continue;
        }

        if state == STRING_KV_KEY {
            if p >= n {
                continue;
            }
            let mut q = p + 1;
            if c == 61 {
                while lx.c(q) == 32 {
                    q += 1;
                }
            }
            if c != 61 || lx.c(q) != 34 {
                lx.emit(t::ERROR, p, p);
                break;
            }
            lx.p = q + 1;
            state = STRING_KV_VALUE;
            continue;
        }

        if state == STRING_KV_VALUE {
            if p >= n {
                continue;
            }
            if c == 34 {
                lx.p += 1;
                state = ATTRIBUTE;
            } else {
                let e = lx.up_to(34, p);
                lx.emit(t::STR_VALUE, p, e);
            }
            continue;
        }

        if state == ACC {
            let e = lx.up_to(10, p);
            lx.emit(acc_value, p, e);
            state = INITIAL;
            continue;
        }

        // ACC_DESCR_MULTILINE
        debug_assert_eq!(state, ACC_DESCR_MULTILINE);
        if c == 125 {
            lx.p += 1;
            state = INITIAL;
        } else {
            let e = lx.up_to(125, p);
            lx.emit(t::acc_descr_multiline_value, p, e);
        }
    }
    lx.out
}
