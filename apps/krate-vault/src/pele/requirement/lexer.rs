//! Ports diagrams/requirement/lexer.ts: the token stream of Mermaid's
//! requirement lexer. Its rules are tried in order and the first match
//! wins, so a line that holds a direction statement anywhere is one token.
//! Positions are character indices of a [`Text`]; `starts` are byte
//! offsets, as errors locate them.

#![allow(clippy::collapsible_match, clippy::collapsible_if)]

use alloc::string::String;
use alloc::vec::Vec;

use super::tokens as t;
use crate::pele::common::chars::{is_line_end, is_space, is_word, Text};
use crate::pele::js::trim;

/// Ports `interface Tokens`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tokens {
    pub types: Vec<u8>,
    pub texts: Vec<String>,
    pub starts: Vec<usize>,
}

// `const enum S`.
const INITIAL: u8 = 0;
const STRING: u8 = 1;
const STYLE: u8 = 2;
const ACC_TITLE: u8 = 3;
const ACC_DESCR: u8 = 4;
const ACC_DESCR_MULTILINE: u8 = 5;

const LONGEST_KEYWORD: usize = 22;

/// Ports `STOP`: characters that end an unquoted string, `:,{<>-=` and
/// line breaks.
fn stop(c: u32) -> bool {
    matches!(c, 58 | 44 | 13 | 10 | 123 | 60 | 62 | 45 | 61)
}

/// Case-insensitive lower-case ASCII `word` at `q`.
fn word_at(src: &Text, q: usize, word: &str) -> bool {
    for (i, k) in word.bytes().enumerate() {
        let c = src.c(q + i);
        let k = k as u32;
        if c != k && !((97..=122).contains(&k) && c == k - 32) {
            return false;
        }
    }
    true
}

struct Lexer<'a> {
    src: &'a Text,
    n: usize,
    out: Tokens,
    stack: Vec<u8>,
    state: u8,
    p: usize,
    dir_starts: [Vec<usize>; 4],
    dir_ends: [Vec<usize>; 4],
    dir_seen: [usize; 4],
    any_dir: bool,
    line_end: isize,
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

    fn push(&mut self, s: u8) {
        self.stack.push(self.state);
        self.state = s;
    }

    fn pop(&mut self) {
        if let Some(s) = self.stack.pop() {
            self.state = s;
        }
    }

    fn line_break(&self, q: usize) -> usize {
        self.src.find_char(10, q).unwrap_or(self.n)
    }

    /// `/accTitle\s*:\s*/iy` and its siblings: `word`, spaces, `mark`, spaces.
    fn acc(&self, word: &str, mark: u32) -> Option<usize> {
        if !word_at(self.src, self.p, word) {
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

    /// Ports `direction`: `.*direction\s+XX[^\n]*`, the last such statement
    /// that starts on this line decides the token.
    fn direction(&mut self) -> bool {
        let p = self.p;
        if self.line_end < p as isize {
            let mut q = p;
            while q < self.n && !is_line_end(self.c(q)) {
                q += 1;
            }
            self.line_end = q as isize;
            for k in 0..4 {
                let list = &self.dir_starts[k];
                let mut j = self.dir_seen[k];
                while j < list.len() && list[j] as isize <= self.line_end {
                    j += 1;
                }
                self.dir_seen[k] = j;
            }
        }
        for k in 0..4 {
            let j = self.dir_seen[k];
            if j >= 1 && self.dir_starts[k][j - 1] >= p {
                let e = self.line_break(self.dir_ends[k][j - 1]);
                self.emit(t::direction_tb + k as u8, p, e);
                return true;
            }
        }
        false
    }

    /// Ports `initial`.
    fn initial(&mut self, c: u32) -> bool {
        let p = self.p;
        let n = self.n;
        let lower = c | 32;
        if lower == 116 && word_at(self.src, p + 1, "itle") && is_space(self.c(p + 5)) {
            let mut q = p + 6;
            while q < n {
                let d = self.c(q);
                if d == 35 || d == 10 || d == 59 {
                    break;
                }
                q += 1;
            }
            if q > p + 6 {
                self.emit(t::title, p, q);
                return true;
            }
        }
        if lower == 97 && (self.c(p + 1) | 32) == 99 && (self.c(p + 2) | 32) == 99 {
            if let Some(e) = self.acc("acctitle", 58) {
                self.push(ACC_TITLE);
                self.emit(t::acc_title, p, e);
                return true;
            }
            if let Some(e) = self.acc("accdescr", 58) {
                self.push(ACC_DESCR);
                self.emit(t::acc_descr, p, e);
                return true;
            }
            if let Some(e) = self.acc("accdescr", 123) {
                self.push(ACC_DESCR_MULTILINE);
                self.p = e;
                return true;
            }
        }
        if self.any_dir && !is_line_end(c) && self.direction() {
            return true;
        }

        if c == 10 || (c == 13 && self.c(p + 1) == 10) {
            let mut q = p;
            while q < n {
                let d = self.c(q);
                if d == 10 {
                    q += 1;
                } else if d == 13 && self.c(q + 1) == 10 {
                    q += 2;
                } else {
                    break;
                }
            }
            self.emit(t::NEWLINE, p, q);
            return true;
        }
        if is_space(c) {
            self.p += 1;
            while self.p < n && is_space(self.c(self.p)) {
                self.p += 1;
            }
            return true;
        }
        if c == 35 || c == 37 {
            self.p = self.line_break(p);
            return true;
        }

        if is_word(c) {
            let mut q = p + 1;
            while q < n && is_word(self.c(q)) {
                q += 1;
            }
            if q - p <= LONGEST_KEYWORD {
                let word = self.src.slice(p, q).to_ascii_lowercase();
                if let Some(keyword) = t::keyword(&word) {
                    if keyword == t::STYLE || keyword == t::CLASSDEF || keyword == t::CLASS {
                        self.push(STYLE);
                    }
                    self.emit(keyword, p, q);
                    return true;
                }
            }
            while q < n {
                let d = self.c(q);
                if d < 128 && stop(d) {
                    break;
                }
                q += 1;
            }
            self.out.types.push(t::unqString);
            self.out.texts.push(trim(self.src.slice(p, q)).into());
            self.out.starts.push(self.src.byte(p));
            self.p = q;
            return true;
        }

        match c {
            123 => {
                self.emit(t::STRUCT_START, p, p + 1);
                true
            }
            125 => {
                self.emit(t::STRUCT_STOP, p, p + 1);
                true
            }
            58 => {
                if self.c(p + 1) == 58 && self.c(p + 2) == 58 {
                    self.emit(t::STYLE_SEPARATOR, p, p + 3);
                } else {
                    self.emit(t::COLONSEP, p, p + 1);
                }
                true
            }
            60 => {
                if self.c(p + 1) != 45 {
                    return false;
                }
                self.emit(t::END_ARROW_L, p, p + 2);
                true
            }
            45 => {
                if self.c(p + 1) == 62 {
                    self.emit(t::END_ARROW_R, p, p + 2);
                } else {
                    self.emit(t::LINE, p, p + 1);
                }
                true
            }
            34 => {
                self.push(STRING);
                self.p += 1;
                true
            }
            44 => {
                self.emit(t::COMMA, p, p + 1);
                true
            }
            _ => false,
        }
    }

    /// Ports `style`.
    fn style(&mut self, c: u32) -> bool {
        let p = self.p;
        if is_word(c) {
            let mut q = p + 1;
            while q < self.n && is_word(self.c(q)) {
                q += 1;
            }
            self.emit(t::ALPHA, p, q);
            return true;
        }
        match c {
            58 => self.emit(t::COLON, p, p + 1),
            59 => self.emit(t::SEMICOLON, p, p + 1),
            37 => self.emit(t::PERCENT, p, p + 1),
            45 => self.emit(t::MINUS, p, p + 1),
            35 => self.emit(t::BRKT, p, p + 1),
            32 => self.p += 1,
            34 => {
                self.push(STRING);
                self.p += 1;
            }
            10 => {
                self.pop();
                self.p += 1;
            }
            44 => self.emit(t::COMMA, p, p + 1),
            _ => return false,
        }
        true
    }
}

/// `/direction\s+(?:(TB)|(BT)|(RL)|LR)/gi` over the whole text: where each
/// kind of direction statement starts and where its keyword ends.
fn directions(src: &Text) -> ([Vec<usize>; 4], [Vec<usize>; 4]) {
    let mut starts: [Vec<usize>; 4] = Default::default();
    let mut ends: [Vec<usize>; 4] = Default::default();
    let n = src.len();
    let mut i = 0;
    while i < n {
        if word_at(src, i, "direction") {
            let mut q = i + 9;
            while q < n && is_space(src.c(q)) {
                q += 1;
            }
            if q > i + 9 {
                let kind = ["tb", "bt", "rl", "lr"]
                    .iter()
                    .position(|w| word_at(src, q, w));
                if let Some(k) = kind {
                    starts[k].push(i);
                    ends[k].push(q + 2);
                    i = q + 2;
                    continue;
                }
            }
        }
        i += 1;
    }
    (starts, ends)
}

/// Ports `tokenize`.
pub fn tokenize(source: &str) -> Tokens {
    let src = Text::new(source);
    let n = src.len();
    let (dir_starts, dir_ends) = directions(&src);
    let any_dir = dir_starts.iter().any(|l| !l.is_empty());
    let mut lx = Lexer {
        src: &src,
        n,
        out: Tokens::default(),
        stack: Vec::new(),
        state: INITIAL,
        p: 0,
        dir_starts,
        dir_ends,
        dir_seen: [0; 4],
        any_dir,
        line_end: -1,
    };

    while lx.p < n {
        let p = lx.p;
        let c = lx.c(p);
        let mut ok = true;
        match lx.state {
            INITIAL => ok = lx.initial(c),
            STYLE => ok = lx.style(c),
            STRING => {
                if c == 34 {
                    lx.pop();
                    lx.p += 1;
                } else {
                    let e = src.find_char(34, p).unwrap_or(n);
                    lx.emit(t::qString, p, e);
                }
            }
            ACC_TITLE => {
                lx.pop();
                let e = lx.line_break(p);
                lx.emit(t::acc_title_value, p, e);
            }
            ACC_DESCR => {
                lx.pop();
                let e = lx.line_break(p);
                lx.emit(t::acc_descr_value, p, e);
            }
            _ => {
                // ACC_DESCR_MULTILINE
                if c == 125 {
                    lx.pop();
                    lx.p += 1;
                } else {
                    let e = src.find_char(125, p).unwrap_or(n);
                    lx.emit(t::acc_descr_multiline_value, p, e);
                }
            }
        }
        if !ok {
            let at = src.byte(lx.p);
            lx.out.types.push(t::ERROR);
            lx.out.texts.push(String::new());
            lx.out.starts.push(at);
            return lx.out;
        }
    }

    // At the end of input Jison still tries the rules that can match nothing, once.
    let last = match lx.state {
        INITIAL => Some(t::EOF),
        STRING => Some(t::qString),
        ACC_TITLE => Some(t::acc_title_value),
        ACC_DESCR => Some(t::acc_descr_value),
        ACC_DESCR_MULTILINE => Some(t::acc_descr_multiline_value),
        _ => None,
    };
    if let Some(k) = last {
        lx.emit(k, n, n);
    }
    lx.emit(t::END, n, n);
    lx.out
}
