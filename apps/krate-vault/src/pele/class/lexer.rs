//! Ports diagrams/class/lexer.ts: the class diagram tokenizer, a hand port
//! of Mermaid's Jison lexer. Positions are character indices of a
//! [`Text`]; `starts` are byte offsets, as errors locate them.

#![allow(clippy::collapsible_match)]

use alloc::string::String;
use alloc::vec::Vec;

use super::tokens as t;
use crate::pele::common::chars::{is_line_end, is_space, is_word, Text};
use crate::pele::util::unicode::is_unicode_letter;

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
const BQSTRING: u8 = 2;
const GENERIC: u8 = 3;
const CALLBACK_NAME: u8 = 4;
const CALLBACK_ARGS: u8 = 5;
const ACC_TITLE: u8 = 6;
const ACC_DESCR: u8 = 7;
const ACC_DESCR_MULTILINE: u8 = 8;
const CLS: u8 = 9;
const CLASS_BODY: u8 = 10;
const NAMESPACE: u8 = 11;
const NAMESPACE_BODY: u8 = 12;

/// The token each state gives at the end of input, before the parser sees
/// the end itself (`AT_END`).
const AT_END: [u8; 13] = [
    t::EOF,
    t::STR,
    t::EOF,
    t::GENERICTYPE,
    t::CALLBACK_NAME,
    t::CALLBACK_ARGS,
    t::acc_title_value,
    t::acc_descr_value,
    t::acc_descr_multiline_value,
    t::EOF,
    t::EOF_IN_STRUCT,
    t::EOF,
    t::EOF_IN_STRUCT,
];

/// The direction words of `RE_DIRECTION`, in order.
const DIRECTIONS: [&str; 4] = ["TB", "BT", "RL", "LR"];

struct Lexer<'a> {
    src: &'a Text,
    n: usize,
    out: Tokens,
    stack: Vec<u8>,
    state: u8,
    p: usize,
    dir_next: Option<usize>,
    line_end: isize,
    dir_failed: usize,
}

impl Lexer<'_> {
    fn c(&self, i: usize) -> u32 {
        self.src.c(i)
    }

    fn emit(&mut self, kind: u8, s: usize, e: usize) -> bool {
        self.out.types.push(kind);
        self.out.texts.push(self.src.slice(s, e).into());
        self.out.starts.push(self.src.byte(s));
        self.p = e;
        true
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
        self.src.starts_with(word, self.p) && !is_word(self.c(self.p + word.len()))
    }

    fn skip_ws(&self, mut q: usize) -> usize {
        while q < self.n && is_space(self.c(q)) {
            q += 1;
        }
        q
    }

    fn until(&self, ch: u32) -> usize {
        self.src.find_char(ch, self.p).unwrap_or(self.n)
    }

    /// Ports `space`: a run of whitespace is a NEWLINE up to its last line
    /// feed; the rest is skipped.
    fn space(&mut self, leaves: bool) {
        let mut q = self.p;
        let mut k: Option<usize> = None;
        while q < self.n && is_space(self.c(q)) {
            if self.c(q) == 10 {
                k = Some(q);
            }
            q += 1;
        }
        let Some(k) = k else {
            self.p = q;
            return;
        };
        if leaves {
            self.pop();
        }
        self.emit(t::NEWLINE, self.p, k + 1);
    }

    /// Ports `keyword`: the keywords that every state recognizes, even
    /// inside generics and backtick names.
    fn keyword(&mut self, c: u32) -> bool {
        let p = self.p;
        match c {
            99 => {
                if self.kw("cssClass") {
                    return self.emit(t::CSSCLASS, p, p + 8);
                }
                if self.kw("callback") {
                    return self.emit(t::CALLBACK, p, p + 8);
                }
                if self.kw("click") {
                    return self.emit(t::CLICK, p, p + 5);
                }
            }
            108 => {
                if self.kw("link") {
                    return self.emit(t::LINK, p, p + 4);
                }
            }
            110 => {
                if self.kw("note for") {
                    return self.emit(t::NOTE_FOR, p, p + 8);
                }
                if self.kw("note") {
                    return self.emit(t::NOTE, p, p + 4);
                }
            }
            60 => {
                if self.c(p + 1) == 60 {
                    return self.emit(t::ANNOTATION_START, p, p + 2);
                }
            }
            62 => {
                if self.c(p + 1) == 62 {
                    return self.emit(t::ANNOTATION_END, p, p + 2);
                }
            }
            104 => {
                if self.kw("href") {
                    return self.emit(t::HREF, p, p + 4);
                }
            }
            _ => {}
        }
        false
    }

    /// Ports `common`.
    fn common(&mut self, c: u32) -> bool {
        if self.keyword(c) {
            return true;
        }
        let p = self.p;
        let d = self.c(p + 1);
        match c {
            34 => {
                self.push(STRING);
                self.p += 1;
                return true;
            }
            126 => {
                self.push(GENERIC);
                self.p += 1;
                return true;
            }
            96 => {
                self.push(BQSTRING);
                self.p += 1;
                return true;
            }
            95 => {
                if self.kw("_self") {
                    return self.emit(t::LINK_TARGET, p, p + 5);
                }
                if self.kw("_blank") {
                    return self.emit(t::LINK_TARGET, p, p + 6);
                }
                if self.kw("_parent") {
                    return self.emit(t::LINK_TARGET, p, p + 7);
                }
                if self.kw("_top") {
                    return self.emit(t::LINK_TARGET, p, p + 4);
                }
            }
            60 => {
                return if d == 124 {
                    self.emit(t::EXTENSION, p, p + 2)
                } else {
                    self.emit(t::DEPENDENCY, p, p + 1)
                };
            }
            124 => return d == 62 && self.emit(t::EXTENSION, p, p + 2),
            62 => return self.emit(t::DEPENDENCY, p, p + 1),
            42 => return self.emit(t::COMPOSITION, p, p + 1),
            111 => {
                if !is_word(d) {
                    return self.emit(t::AGGREGATION, p, p + 1);
                }
            }
            40 => return d == 41 && self.emit(t::LOLLIPOP, p, p + 2),
            45 => {
                return if d == 45 {
                    self.emit(t::LINE, p, p + 2)
                } else {
                    self.emit(t::MINUS, p, p + 1)
                };
            }
            46 => {
                return if d == 46 {
                    self.emit(t::DOTTED_LINE, p, p + 2)
                } else {
                    self.emit(t::DOT, p, p + 1)
                };
            }
            58 => {
                let mut q = p + 1;
                while q < self.n {
                    let ch = self.c(q);
                    if ch == 58 || ch == 10 || ch == 59 {
                        break;
                    }
                    q += 1;
                }
                if q > p + 1 {
                    return self.emit(t::LABEL, p, q);
                }
                return d == 58 && self.c(p + 2) == 58 && self.emit(t::STYLE_SEPARATOR, p, p + 3);
            }
            43 => return self.emit(t::PLUS, p, p + 1),
            37 => return self.emit(t::PCT, p, p + 1),
            61 => return self.emit(t::EQUALS, p, p + 1),
            91 => return self.emit(t::SQS, p, p + 1),
            93 => return self.emit(t::SQE, p, p + 1),
            33 | 35 | 36 | 38 | 39 | 44 | 63 | 92 | 47 => {
                return self.emit(t::PUNCTUATION, p, p + 1)
            }
            _ => {}
        }
        if is_word(c) {
            let mut q = p + 1;
            while is_word(self.c(q)) {
                q += 1;
            }
            return self.emit(t::ALPHA, p, q);
        }
        is_unicode_letter(c) && self.emit(t::UNICODE_TEXT, p, p + 1)
    }

    /// `/.*direction\s+XX[^\n]*/y` at `p` for the direction `word`: the end
    /// of the match, through the last `direction` on the line it can use.
    fn direction_re(&self, word: &str) -> Option<usize> {
        let p = self.p;
        let mut line = p;
        while line < self.n && !is_line_end(self.c(line)) {
            line += 1;
        }
        let mut q = line;
        loop {
            // The last `direction` that starts before `q` and lies on the line.
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

    /// Ports `direction`.
    fn direction(&mut self) -> bool {
        let p = self.p;
        if p < self.dir_failed {
            return false;
        }
        let Some(mut next) = self.dir_next else {
            return false;
        };
        if next < p {
            match self.src.index_of("direction", p) {
                Some(d) => next = d,
                None => {
                    self.dir_next = None;
                    return false;
                }
            }
            self.dir_next = Some(next);
        }
        if self.line_end < p as isize {
            let mut q = p;
            while q < self.n && !is_line_end(self.c(q)) {
                q += 1;
            }
            self.line_end = q as isize;
        }
        if next as isize >= self.line_end {
            return false;
        }
        for (i, word) in DIRECTIONS.iter().enumerate() {
            if let Some(e) = self.direction_re(word) {
                return self.emit(t::direction_tb + i as u8, p, e);
            }
        }
        // No direction statement starts anywhere in the rest of this line either.
        self.dir_failed = self.line_end as usize;
        false
    }

    /// Ports `initial`.
    fn initial(&mut self, c: u32) -> bool {
        if self.direction() {
            return true;
        }
        if is_space(c) {
            self.space(false);
            return true;
        }
        let p = self.p;
        match c {
            37 => {
                if self.c(p + 1) == 37 {
                    let mut e = self.until(10);
                    while e < self.n && (self.c(e) == 10 || self.c(e) == 13) {
                        e += 1;
                    }
                    self.p = e;
                    return true;
                }
            }
            97 => {
                let title = self.src.starts_with("accTitle", p);
                if title || self.src.starts_with("accDescr", p) {
                    let q = self.skip_ws(p + 8);
                    let d = self.c(q);
                    if d == 58 {
                        let e = self.skip_ws(q + 1);
                        self.emit(if title { t::acc_title } else { t::acc_descr }, p, e);
                        self.push(if title { ACC_TITLE } else { ACC_DESCR });
                        return true;
                    }
                    if d == 123 && !title {
                        self.p = self.skip_ws(q + 1);
                        self.push(ACC_DESCR_MULTILINE);
                        return true;
                    }
                }
            }
            99 => {
                if self.kw("classDiagram-v2") {
                    return self.emit(t::CLASS_DIAGRAM, p, p + 15);
                }
                if self.kw("classDiagram") {
                    return self.emit(t::CLASS_DIAGRAM, p, p + 12);
                }
                if self.src.starts_with("call", p) && is_space(self.c(p + 4)) {
                    self.p = self.skip_ws(p + 4);
                    self.push(CALLBACK_NAME);
                    return true;
                }
                if self.kw("classDef") {
                    return self.emit(t::CLASSDEF, p, p + 8);
                }
                if self.kw("class") {
                    self.push(CLS);
                    return self.emit(t::CLASS, p, p + 5);
                }
            }
            91 => {
                if self.src.starts_with("*]", p + 1) {
                    return self.emit(t::EDGE_STATE, p, p + 3);
                }
            }
            115 => {
                if self.kw("style") {
                    return self.emit(t::STYLE, p, p + 5);
                }
            }
            110 => {
                if self.kw("namespace") {
                    self.push(NAMESPACE);
                    return self.emit(t::NAMESPACE, p, p + 9);
                }
            }
            44 => return self.emit(t::COMMA, p, p + 1),
            35 => return self.emit(t::BRKT, p, p + 1),
            _ => {}
        }
        self.common(c) || (c == 58 && self.emit(t::COLON, p, p + 1))
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
        stack: Vec::new(),
        state: INITIAL,
        p: 0,
        dir_next: src.index_of("direction", 0),
        line_end: -1,
        dir_failed: 0,
    };

    loop {
        if lx.p >= n {
            lx.out.types.push(AT_END[lx.state as usize]);
            lx.out.texts.push(String::new());
            lx.out.starts.push(source.len());
            break;
        }
        let c = lx.c(lx.p);
        let p = lx.p;
        let mut ok = true;
        match lx.state {
            INITIAL => ok = lx.initial(c),
            CLS => {
                if is_space(c) {
                    lx.space(true);
                } else if c == 125 {
                    lx.pop();
                    lx.pop();
                    lx.emit(t::STRUCT_STOP, p, p + 1);
                } else if c == 123 {
                    lx.push(CLASS_BODY);
                    lx.emit(t::STRUCT_START, p, p + 1);
                } else {
                    ok = lx.common(c);
                }
            }
            CLASS_BODY => {
                if c == 34 {
                    lx.push(STRING);
                    lx.p += 1;
                } else if c == 125 {
                    lx.pop();
                    lx.emit(t::STRUCT_STOP, p, p + 1);
                } else if c == 91 && src.starts_with("*]", p + 1) {
                    lx.emit(t::EDGE_STATE, p, p + 3);
                } else if c == 123 {
                    lx.emit(t::OPEN_IN_STRUCT, p, p + 1);
                } else if c == 10 {
                    lx.p += 1;
                } else {
                    let mut q = p + 1;
                    while q < n {
                        let ch = lx.c(q);
                        if ch == 123 || ch == 125 || ch == 10 {
                            break;
                        }
                        q += 1;
                    }
                    lx.emit(t::MEMBER, p, q);
                }
            }
            NAMESPACE => {
                if is_space(c) {
                    lx.space(true);
                } else if c == 123 {
                    lx.push(NAMESPACE_BODY);
                    lx.emit(t::STRUCT_START, p, p + 1);
                } else if c == 125 {
                    // The brace is read again in the state the namespace was opened from.
                    lx.pop();
                } else if c == 110 && lx.kw("namespace") {
                    lx.push(NAMESPACE);
                    lx.emit(t::NAMESPACE, p, p + 9);
                } else {
                    ok = lx.common(c);
                }
            }
            NAMESPACE_BODY => {
                if is_space(c) {
                    lx.space(false);
                } else if c == 125 {
                    lx.pop();
                    lx.emit(t::STRUCT_STOP, p, p + 1);
                } else if c == 110 && lx.kw("namespace") {
                    lx.push(NAMESPACE);
                    lx.emit(t::NAMESPACE, p, p + 9);
                } else if c == 91 && src.starts_with("*]", p + 1) {
                    lx.emit(t::EDGE_STATE, p, p + 3);
                } else if c == 99 && lx.kw("class") {
                    lx.push(CLS);
                    lx.emit(t::CLASS, p, p + 5);
                } else {
                    ok = lx.common(c);
                }
            }
            STRING => {
                if c == 34 {
                    lx.pop();
                    lx.p += 1;
                } else {
                    let e = lx.until(34);
                    lx.emit(t::STR, p, e);
                }
            }
            GENERIC => {
                if c == 34 {
                    lx.push(STRING);
                    lx.p += 1;
                } else if lx.keyword(c) {
                } else if c == 126 {
                    lx.pop();
                    lx.p += 1;
                } else {
                    let e = lx.until(126);
                    lx.emit(t::GENERICTYPE, p, e);
                }
            }
            BQSTRING => {
                if c == 34 {
                    lx.push(STRING);
                    lx.p += 1;
                } else if lx.keyword(c) {
                } else if c == 126 {
                    lx.push(GENERIC);
                    lx.p += 1;
                } else if c == 96 {
                    lx.pop();
                    lx.p += 1;
                } else {
                    let e = lx.until(96);
                    lx.emit(t::BQUOTE_STR, p, e);
                }
            }
            CALLBACK_NAME => {
                if c == 40 {
                    let q = lx.skip_ws(p + 1);
                    lx.pop();
                    if lx.c(q) == 41 {
                        lx.p = q + 1;
                    } else {
                        lx.push(CALLBACK_ARGS);
                        lx.p += 1;
                    }
                } else {
                    let e = lx.until(40);
                    lx.emit(t::CALLBACK_NAME, p, e);
                }
            }
            CALLBACK_ARGS => {
                if c == 41 {
                    lx.pop();
                    lx.p += 1;
                } else {
                    let e = lx.until(41);
                    lx.emit(t::CALLBACK_ARGS, p, e);
                }
            }
            ACC_TITLE | ACC_DESCR => {
                let e = lx.until(10);
                let kind = if lx.state == ACC_TITLE {
                    t::acc_title_value
                } else {
                    t::acc_descr_value
                };
                lx.emit(kind, p, e);
                lx.pop();
            }
            _ => {
                // ACC_DESCR_MULTILINE
                if c == 125 {
                    lx.pop();
                    lx.p += 1;
                } else {
                    let e = lx.until(125);
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

    lx.out.types.push(t::END);
    lx.out.texts.push(String::new());
    lx.out.starts.push(source.len());
    lx.out
}
