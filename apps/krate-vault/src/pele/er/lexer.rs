//! Ports diagrams/er/lexer.ts: the ER diagram tokenizer, a hand port of
//! Mermaid's Jison lexer. Positions are character indices of a [`Text`];
//! `starts` are byte offsets, as errors locate them.

#![allow(clippy::collapsible_match, clippy::collapsible_if)]

use alloc::string::String;
use alloc::vec::Vec;

use super::tokens as t;
use crate::pele::common::chars::{is_digit, is_line_end, is_space, is_word, Text, NONE};

/// Ports `interface Tokens`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tokens {
    pub types: Vec<u8>,
    pub texts: Vec<String>,
    pub starts: Vec<usize>,
}

// `const enum S`.
const INITIAL: u8 = 0;
const BLOCK: u8 = 1;
const BLOCK_BQ: u8 = 2;
const ACC_TITLE: u8 = 3;
const ACC_DESCR: u8 = 4;
const ACC_DESCR_MULTILINE: u8 = 5;
const STYLE: u8 = 6;

const TEXT: u8 = 1;
const STYLE_TEXT: u8 = 2;
const ATTR_START: u8 = 4;
const ATTR_REST: u8 = 8;

/// Ports `CLASS`: what each ASCII character can be part of.
fn class(c: u32) -> u8 {
    if c >= 128 {
        return 0;
    }
    let mut k = 0;
    let word = is_word(c);
    if word {
        k |= TEXT | STYLE_TEXT | ATTR_REST;
        if !is_digit(c) {
            k |= ATTR_START;
        }
    }
    match c {
        45 | 42 => k |= TEXT | STYLE_TEXT | ATTR_REST,
        _ => {}
    }
    if c == 46 {
        k |= TEXT | ATTR_REST;
    }
    if c == 42 {
        k |= ATTR_START;
    }
    if matches!(c, 91 | 93 | 40 | 41 | 44) {
        k |= ATTR_REST;
    }
    k
}

/// `c >= 128 || (CLASS[c] & kind) !== 0`, false past the end.
fn text_like(c: u32, kind: u8) -> bool {
    c != NONE && (c >= 128 || class(c) & kind != 0)
}

/// Ports `isAttr`: Mermaid's attribute pattern is case-insensitive, which
/// lets the micro sign into its À-￿ range.
fn is_attr(c: u32, kind: u8) -> bool {
    if c == NONE {
        return false;
    }
    if c < 128 {
        class(c) & kind != 0
    } else {
        c >= 0xc0 || c == 0xb5
    }
}

const DIRECTIONS: [&str; 4] = ["tb", "bt", "rl", "lr"];

struct Lexer<'a> {
    src: &'a Text,
    n: usize,
    out: Tokens,
    state: u8,
    p: usize,
    dir_next: isize,
    dir_failed: usize,
    line_end: isize,
    tilde_next: isize,
    tilde_line: isize,
    tilde_last: isize,
    ws_next: isize,
}

impl Lexer<'_> {
    fn c(&self, i: usize) -> u32 {
        self.src.c(i)
    }

    fn emit(&mut self, kind: u8, s: usize, e: usize) {
        self.out.types.push(kind);
        self.out.texts.push(self.src.slice(s, e).into());
        self.out.starts.push(self.src.byte(s));
        self.p = e;
    }

    /// Case-insensitive `word` (lower case) at `q`.
    fn kw_at(&self, word: &str, q: usize) -> bool {
        for (i, k) in word.bytes().enumerate() {
            let k = k as u32;
            let c = self.c(q + i);
            if c != k && (!(97..=122).contains(&k) || (c | 32) != k) {
                return false;
            }
        }
        true
    }

    /// Ports `kw`: a lower-case keyword at p, ignoring case.
    fn kw(&self, word: &str) -> bool {
        self.kw_at(word, self.p)
    }

    /// Ports `kwb`.
    fn kwb(&self, word: &str) -> bool {
        self.kw(word) && !is_word(self.c(self.p + word.len()))
    }

    /// Ports `lower`.
    fn lower(&self, q: usize) -> u32 {
        let c = self.c(q);
        if (65..=90).contains(&c) {
            c + 32
        } else {
            c
        }
    }

    /// Ports `nextLineEnd`.
    fn next_line_end(&self, q: usize) -> usize {
        let mut k = q;
        while k < self.n && !is_line_end(self.c(k)) {
            k += 1;
        }
        k
    }

    /// Ports `digits`.
    fn digits(&self, mut q: usize) -> usize {
        while is_digit(self.c(q)) {
            q += 1;
        }
        q
    }

    /// `/accTitle\s*:\s*/iy` and its siblings: `word`, spaces, `mark`, spaces.
    fn acc(&self, word: &str, mark: u32) -> Option<usize> {
        if !self.kw(word) {
            return None;
        }
        let mut q = self.p + word.len();
        while is_space(self.c(q)) {
            q += 1;
        }
        if self.c(q) != mark {
            return None;
        }
        q += 1;
        while is_space(self.c(q)) {
            q += 1;
        }
        Some(q)
    }

    /// `/.*direction\s+XX[^\n]*/iy` at `p`.
    fn direction_re(&self, word: &str) -> Option<usize> {
        let p = self.p;
        let line = self.next_line_end(p);
        let mut q = line;
        loop {
            let mut found = None;
            let mut k = q;
            while k > p {
                k -= 1;
                if k + 9 <= line && self.kw_at("direction", k) {
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
            if r > ws && self.kw_at(word, r) {
                let mut e = r + 2;
                while e < self.n && self.c(e) != 10 {
                    e += 1;
                }
                return Some(e);
            }
            q = k;
        }
    }

    /// Ports `direction`: a direction statement swallows its whole line,
    /// from wherever the lexer stands on it.
    fn direction(&mut self) -> bool {
        let p = self.p;
        if self.dir_next == -1 {
            return false;
        }
        if self.dir_next < p as isize {
            self.dir_next = (p..self.n)
                .find(|&k| self.kw_at("direction", k))
                .map_or(-1, |k| k as isize);
            if self.dir_next == -1 {
                return false;
            }
        }
        if p < self.dir_failed {
            return false;
        }
        if self.line_end < p as isize {
            self.line_end = self.next_line_end(p) as isize;
        }
        if self.dir_next >= self.line_end {
            return false;
        }
        for (i, word) in DIRECTIONS.iter().enumerate() {
            if let Some(e) = self.direction_re(word) {
                self.emit(t::direction_tb + i as u8, p, e);
                return true;
            }
        }
        self.dir_failed = self.line_end as usize;
        false
    }

    /// Ports `quoted`.
    fn quoted(&mut self) {
        let p = self.p;
        let Some(e) = self.src.find_char(34, p + 1) else {
            self.emit(t::CHAR, p, p + 1);
            return;
        };
        let mut name = e > p + 1;
        let mut q = p + 1;
        while name && q < e {
            let c = self.c(q);
            if c == 37 || c == 92 || c == 13 || c == 10 || c == 11 || c == 8 {
                name = false;
            }
            q += 1;
        }
        self.emit(if name { t::ENTITY_NAME } else { t::WORD }, p, e + 1);
    }

    /// Ports `number`.
    fn number(&mut self, c: u32) {
        let p = self.p;
        let d = self.c(p + 1);
        if d == 43 && (c == 49 || c == 48) {
            self.emit(
                if c == 49 {
                    t::ONE_OR_MORE
                } else {
                    t::ZERO_OR_MORE
                },
                p,
                p + 2,
            );
            return;
        }
        let q = self.digits(p + 1);
        if self.c(q) == 46 && is_digit(self.c(q + 1)) {
            let e = self.digits(q + 2);
            self.emit(t::DECIMAL_NUM, p, e);
            return;
        }
        if c == 49 {
            if is_space(d) {
                let mut r = p + 2;
                while is_space(self.c(r)) {
                    r += 1;
                }
                let f = self.c(r);
                if is_word(f) || f == 34 || f == 39 {
                    self.emit(t::ONLY_ONE, p, p + 1);
                    return;
                }
            } else if d == 45 || d == 46 {
                let f = self.c(p + 2);
                if f == 45 || f == 46 {
                    self.emit(t::ONLY_ONE, p, p + 1);
                    return;
                }
            }
            if !is_word(d) {
                self.emit(t::ENTITY_ONE, p, p + 1);
                return;
            }
        }
        self.emit(t::NUM, p, q);
    }

    /// Ports `keyword`.
    fn keyword(&mut self, lc: u32) -> bool {
        let p = self.p;
        let d = self.c(p + 1);
        match lc {
            101 => {
                if self.kwb("erdiagram") {
                    self.emit(t::ER_DIAGRAM, p, p + 9);
                } else if self.kwb("end") {
                    let mut e = p + 3;
                    while is_space(self.c(e)) {
                        e += 1;
                    }
                    self.emit(t::END_KW, p, e);
                } else {
                    return false;
                }
                true
            }
            115 => {
                if self.kwb("style") {
                    self.state = STYLE;
                    self.emit(t::STYLE, p, p + 5);
                } else if self.kwb("subgraph") {
                    self.emit(t::SUBGRAPH, p, p + 8);
                } else {
                    return false;
                }
                true
            }
            99 => {
                if self.kwb("classdef") {
                    self.state = STYLE;
                    self.emit(t::CLASSDEF, p, p + 8);
                } else if self.kwb("class") {
                    self.emit(t::CLASS, p, p + 5);
                } else {
                    return false;
                }
                true
            }
            111 => {
                if self.kwb("one or zero") {
                    self.emit(t::ZERO_OR_ONE, p, p + 11);
                } else if self.kwb("one or more") || self.kwb("one or many") {
                    self.emit(t::ONE_OR_MORE, p, p + 11);
                } else if self.kwb("one") {
                    self.emit(t::ONLY_ONE, p, p + 3);
                } else if self.kwb("only one") {
                    self.emit(t::ONLY_ONE, p, p + 8);
                } else if d == 124 {
                    self.emit(t::ZERO_OR_ONE, p, p + 2);
                } else if d == 123 {
                    self.emit(t::ZERO_OR_MORE, p, p + 2);
                } else if self.kwb("optionally to") {
                    self.emit(t::NON_IDENTIFYING, p, p + 13);
                } else {
                    return false;
                }
                true
            }
            122 => {
                if self.kwb("zero or one") {
                    self.emit(t::ZERO_OR_ONE, p, p + 11);
                } else if self.kwb("zero or more") || self.kwb("zero or many") {
                    self.emit(t::ZERO_OR_MORE, p, p + 12);
                } else {
                    return false;
                }
                true
            }
            109 => {
                if self.kw("many(0)") {
                    self.emit(t::ZERO_OR_MORE, p, p + 7);
                } else if self.kw("many(1)") {
                    self.emit(t::ONE_OR_MORE, p, p + 7);
                } else if self.kwb("many") {
                    self.emit(t::ZERO_OR_MORE, p, p + 4);
                } else {
                    return false;
                }
                true
            }
            116 => {
                if !self.kwb("to") {
                    return false;
                }
                self.emit(t::IDENTIFYING, p, p + 2);
                true
            }
            117 => {
                if d != 46 && d != 45 && d != 124 {
                    return false;
                }
                self.emit(t::MD_PARENT, p, p + 1);
                true
            }
            124 => {
                if self.lower(p + 1) == 111 && !is_word(self.c(p + 2)) {
                    self.emit(t::ZERO_OR_ONE, p, p + 2);
                } else if d == 124 {
                    self.emit(t::ONLY_ONE, p, p + 2);
                } else if d == 123 {
                    self.emit(t::ONE_OR_MORE, p, p + 2);
                } else {
                    return false;
                }
                true
            }
            125 => {
                if self.lower(p + 1) == 111 && !is_word(self.c(p + 2)) {
                    self.emit(t::ZERO_OR_MORE, p, p + 2);
                } else if d == 124 {
                    self.emit(t::ONE_OR_MORE, p, p + 2);
                } else {
                    return false;
                }
                true
            }
            46 => {
                if d != 46 && d != 45 {
                    return false;
                }
                self.emit(t::NON_IDENTIFYING, p, p + 2);
                true
            }
            45 => {
                if d == 45 {
                    self.emit(t::IDENTIFYING, p, p + 2);
                } else if d == 46 {
                    self.emit(t::NON_IDENTIFYING, p, p + 2);
                } else {
                    return false;
                }
                true
            }
            _ => false,
        }
    }

    /// Ports `initial`.
    fn initial(&mut self, c: u32) {
        let p = self.p;
        if (c == 97 || c == 65) && self.kw("acc") {
            if let Some(e) = self.acc("acctitle", 58) {
                self.state = ACC_TITLE;
                self.emit(t::acc_title, p, e);
                return;
            }
            if let Some(e) = self.acc("accdescr", 58) {
                self.state = ACC_DESCR;
                self.emit(t::acc_descr, p, e);
                return;
            }
            if let Some(e) = self.acc("accdescr", 123) {
                self.state = ACC_DESCR_MULTILINE;
                self.p = e;
                return;
            }
        }
        if self.direction() {
            return;
        }

        match c {
            32 | 9 | 13 => {
                let mut e = p + 1;
                while matches!(self.c(e), 32 | 9 | 13) {
                    e += 1;
                }
                self.p = e;
                return;
            }
            10 => {
                let mut e = p + 1;
                while self.c(e) == 10 {
                    e += 1;
                }
                self.emit(t::NEWLINE, p, e);
                return;
            }
            34 => {
                self.quoted();
                return;
            }
            123 => {
                self.state = BLOCK;
                self.emit(t::BLOCK_START, p, p + 1);
                return;
            }
            35 => {
                self.emit(t::BRKT, p, p + 1);
                return;
            }
            44 => {
                self.emit(t::COMMA, p, p + 1);
                return;
            }
            58 => {
                if self.c(p + 1) == 58 && self.c(p + 2) == 58 {
                    self.emit(t::STYLE_SEPARATOR, p, p + 3);
                } else {
                    self.emit(t::COLON, p, p + 1);
                }
                return;
            }
            91 => {
                self.emit(t::SQS, p, p + 1);
                return;
            }
            93 => {
                self.emit(t::SQE, p, p + 1);
                return;
            }
            _ => {}
        }
        if is_digit(c) {
            self.number(c);
            return;
        }
        if self.keyword(if (65..=90).contains(&c) { c + 32 } else { c }) {
            return;
        }
        if text_like(c, TEXT) {
            let mut e = p + 1;
            while text_like(self.c(e), TEXT) {
                e += 1;
            }
            self.emit(t::UNICODE_TEXT, p, e);
            return;
        }
        self.emit(t::CHAR, p, p + 1);
    }

    /// Ports `generic`: Mermaid's generic type rule, a run of non-space
    /// characters holding a `~`, through the last `~` on the line.
    fn generic(&mut self) -> bool {
        let p = self.p;
        if self.tilde_next == -1 {
            return false;
        }
        if self.tilde_next < p as isize {
            self.tilde_next = self.src.find_char(126, p).map_or(-1, |k| k as isize);
            if self.tilde_next == -1 {
                return false;
            }
        }
        if self.ws_next < p as isize {
            let mut q = p;
            while q < self.n && !is_space(self.c(q)) {
                q += 1;
            }
            self.ws_next = q as isize;
        }
        if self.tilde_next >= self.ws_next {
            return false;
        }
        if self.tilde_line < p as isize {
            self.tilde_line = self.next_line_end(p) as isize;
            self.tilde_last = if self.tilde_line > 0 {
                self.src
                    .last_char(126, self.tilde_line as usize - 1)
                    .map_or(-1, |k| k as isize)
            } else if self.c(0) == 126 {
                0
            } else {
                -1
            };
        }
        if self.tilde_last <= self.tilde_next {
            return false;
        }
        let mut e = self.tilde_last as usize + 1;
        while e < self.n && !is_space(self.c(e)) {
            e += 1;
        }
        self.emit(t::ATTRIBUTE_WORD, p, e);
        true
    }

    /// Ports `block`.
    fn block(&mut self, c: u32) {
        let p = self.p;
        if is_space(c) {
            self.p += 1;
            while is_space(self.c(self.p)) {
                self.p += 1;
            }
            return;
        }
        let lc = c | 32;
        if (lc == 112 || lc == 102 || lc == 117)
            && (self.c(p + 1) | 32) == 107
            && !is_word(self.c(p + 2))
        {
            self.emit(t::ATTRIBUTE_KEY, p, p + 2);
            return;
        }
        if self.generic() {
            return;
        }
        if is_attr(c, ATTR_START) {
            let mut e = p + 1;
            while e < self.n && is_attr(self.c(e), ATTR_REST) {
                e += 1;
            }
            self.emit(t::ATTRIBUTE_WORD, p, e);
            return;
        }
        if c == 96 {
            self.state = BLOCK_BQ;
            self.p += 1;
            return;
        }
        if c == 34 {
            if let Some(e) = self.src.find_char(34, p + 1) {
                self.emit(t::COMMENT, p, e + 1);
                return;
            }
        } else if c == 125 {
            self.state = INITIAL;
            self.emit(t::BLOCK_STOP, p, p + 1);
            return;
        }
        self.emit(t::CHAR, p, p + 1);
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
        state: INITIAL,
        p: 0,
        dir_next: -2,
        dir_failed: 0,
        line_end: -1,
        tilde_next: -2,
        tilde_line: -1,
        tilde_last: -1,
        ws_next: -1,
    };

    while lx.p < n {
        let p = lx.p;
        let c = lx.c(p);
        match lx.state {
            INITIAL => lx.initial(c),
            BLOCK => lx.block(c),
            BLOCK_BQ => {
                if c == 96 {
                    lx.state = BLOCK;
                    lx.p += 1;
                } else {
                    let e = src.find_char(96, p).unwrap_or(n);
                    lx.emit(t::ATTRIBUTE_WORD, p, e);
                }
            }
            ACC_TITLE | ACC_DESCR => {
                let e = src.find_char(10, p).unwrap_or(n);
                let kind = if lx.state == ACC_TITLE {
                    t::acc_title_value
                } else {
                    t::acc_descr_value
                };
                lx.emit(kind, p, e);
                lx.state = INITIAL;
            }
            ACC_DESCR_MULTILINE => {
                if c == 125 {
                    lx.state = INITIAL;
                    lx.p += 1;
                } else {
                    let e = src.find_char(125, p).unwrap_or(n);
                    lx.emit(t::acc_descr_multiline_value, p, e);
                }
            }
            _ => {
                // STYLE
                if c == 10 {
                    let mut e = p + 1;
                    while lx.c(e) == 10 {
                        e += 1;
                    }
                    lx.state = INITIAL;
                    lx.emit(t::NEWLINE, p, e);
                } else if is_space(c) {
                    lx.p += 1;
                    while is_space(lx.c(lx.p)) {
                        lx.p += 1;
                    }
                } else if c == 58 {
                    lx.emit(t::COLON, p, p + 1);
                } else if c == 44 {
                    lx.emit(t::COMMA, p, p + 1);
                } else if c == 35 {
                    lx.emit(t::BRKT, p, p + 1);
                } else if c == 59 {
                    lx.emit(t::SEMI, p, p + 1);
                } else if text_like(c, STYLE_TEXT) {
                    let mut e = p + 1;
                    while text_like(lx.c(e), STYLE_TEXT) {
                        e += 1;
                    }
                    lx.emit(t::STYLE_TEXT, p, e);
                } else {
                    lx.out.types.push(t::ERROR);
                    lx.out.texts.push(String::new());
                    lx.out.starts.push(src.byte(p));
                    return lx.out;
                }
            }
        }
    }

    // At the end of input Jison's INITIAL state yields EOF, the title and description states yield
    // an empty value, and the block and style states yield nothing.
    let last = match lx.state {
        INITIAL => Some(t::EOF),
        ACC_TITLE => Some(t::acc_title_value),
        ACC_DESCR => Some(t::acc_descr_value),
        ACC_DESCR_MULTILINE => Some(t::acc_descr_multiline_value),
        _ => None,
    };
    if let Some(k) = last {
        lx.out.types.push(k);
        lx.out.texts.push(String::new());
        lx.out.starts.push(source.len());
    }
    lx.out.types.push(t::END);
    lx.out.texts.push(String::new());
    lx.out.starts.push(source.len());
    lx.out
}
