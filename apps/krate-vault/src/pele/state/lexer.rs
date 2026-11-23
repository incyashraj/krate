//! Ports diagrams/state/lexer.ts: the state diagram tokenizer, a hand port
//! of Mermaid's Jison lexer. Positions are character indices of a
//! [`Text`]; `starts` are byte offsets, as errors locate them.

#![allow(clippy::collapsible_match, clippy::collapsible_if)]

use alloc::format;
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
    /// Set when lexing stopped on the grammar's own error rather than on
    /// unrecognized text.
    pub error: Option<String>,
}

// `const enum S`.
const INITIAL: u8 = 0;
const STRUCT: u8 = 1;
const STATE: u8 = 2;
const STATE_STRING: u8 = 3;
const STATE_ID: u8 = 4;
const SCALE: u8 = 5;
const ACC_TITLE: u8 = 6;
const ACC_DESCR: u8 = 7;
const ACC_DESCR_MULTILINE: u8 = 8;
const CLASSDEF: u8 = 9;
const CLASSDEFID: u8 = 10;
const CLASS: u8 = 11;
const CLASS_STYLE: u8 = 12;
const STYLE: u8 = 13;
const STYLEDEF_STYLES: u8 = 14;
const NOTE: u8 = 15;
const NOTE_ID: u8 = 16;
const NOTE_TEXT: u8 = 17;
const FLOATING_NOTE: u8 = 18;
const FLOATING_NOTE_ID: u8 = 19;

/// The patterns the lexer looks ahead for.
#[derive(Clone, Copy)]
enum Re {
    /// `/direction/gi`
    Direction,
    /// `/<<(?:fork|join|choice)>>|\[\[(?:fork|join|choice)\]\]/gi`
    Stereotype,
    /// `/[\n\r  ]/g`
    LineEnd,
    /// `/%%/g`
    Percent,
    /// `/\{/g`
    Brace,
}

/// Case-insensitive match of a lowercase ASCII word, as the grammar's /i
/// flag gives (`word`).
fn word(src: &Text, w: &str, q: usize) -> bool {
    for (i, a) in w.bytes().enumerate() {
        let a = a as u32;
        let c = src.c(q + i);
        if c != a && !((97..=122).contains(&a) && c == a - 32) {
            return false;
        }
    }
    true
}

/// Whether `re` matches at `q`.
fn matches_at(src: &Text, re: Re, q: usize) -> bool {
    match re {
        Re::Direction => word(src, "direction", q),
        Re::Stereotype => {
            let (open, close) = match src.c(q) {
                60 => ("<<", ">>"),
                91 => ("[[", "]]"),
                _ => return false,
            };
            if !src.starts_with(open, q) {
                return false;
            }
            ["fork", "join", "choice"]
                .iter()
                .any(|w| word(src, w, q + 2) && src.starts_with(close, q + 2 + w.len()))
        }
        Re::LineEnd => is_line_end(src.c(q)),
        Re::Percent => src.c(q) == 37 && src.c(q + 1) == 37,
        Re::Brace => src.c(q) == 123,
    }
}

/// Ports `finder`: finds the next match of a pattern at or after a
/// position, remembering the answer so that asking at each token costs one
/// scan of the text in total.
struct Finder {
    re: Re,
    from: usize,
    next: isize,
}

impl Finder {
    fn new(re: Re) -> Finder {
        Finder {
            re,
            from: 0,
            next: -2,
        }
    }

    fn find(&mut self, src: &Text, q: usize) -> isize {
        if self.next == -2 || q < self.from || (self.next != -1 && q as isize > self.next) {
            self.next = (q..src.len())
                .find(|&k| matches_at(src, self.re, k))
                .map_or(-1, |k| k as isize);
            self.from = q;
        }
        self.next
    }
}

struct Lexer<'a> {
    src: &'a Text,
    n: usize,
    out: Tokens,
    stack: Vec<u8>,
    state: u8,
    p: usize,
    next_direction: Finder,
    next_stereotype: Finder,
    next_line_end: Finder,
    next_percent: Finder,
    next_brace: Finder,
    dir_from: isize,
    dir_to: isize,
    dir_at: [isize; 4],
    dir_end: [usize; 4],
    stereo_from: isize,
    stereo_to: isize,
    stereo_at: [isize; 6],
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

    fn emit_text(&mut self, kind: u8, text: String, s: usize, e: usize) {
        self.out.types.push(kind);
        self.out.texts.push(text);
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

    fn word(&self, w: &str, q: usize) -> bool {
        word(self.src, w, q)
    }

    fn ws_end(&self, q: usize) -> usize {
        let mut j = q;
        while j < self.n && is_space(self.c(j)) {
            j += 1;
        }
        j
    }

    fn word_end(&self, q: usize) -> usize {
        let mut j = q;
        while j < self.n && is_word(self.c(j)) {
            j += 1;
        }
        j
    }

    fn eol(&self, q: usize) -> usize {
        self.src.find_char(10, q).unwrap_or(self.n)
    }

    fn line_end(&mut self, q: usize) -> usize {
        let k = self.next_line_end.find(self.src, q);
        if k == -1 {
            self.n
        } else {
            k as usize
        }
    }

    /// Ports `direction`: rules `.*direction\s+TB[^\n]*` and its three
    /// siblings, the first kind, in grammar order, found between q and the
    /// end of its line.
    fn direction(&mut self, q: usize) -> isize {
        let qi = q as isize;
        if qi < self.dir_from || qi > self.dir_to {
            let first = self.next_direction.find(self.src, q);
            let end = if first == -1 {
                self.n
            } else {
                self.line_end(q)
            };
            self.dir_from = qi;
            self.dir_to = end as isize;
            self.dir_at = [-1; 4];
            let mut d = first;
            while d != -1 && (d as usize) < end {
                let du = d as usize;
                let w = self.ws_end(du + 9);
                if w != du + 9 {
                    let kind = if self.word("tb", w) {
                        0
                    } else if self.word("bt", w) {
                        1
                    } else if self.word("rl", w) {
                        2
                    } else if self.word("lr", w) {
                        3
                    } else {
                        -1
                    };
                    if kind != -1 {
                        self.dir_at[kind as usize] = d;
                        self.dir_end[kind as usize] = w + 2;
                    }
                }
                d = self.next_direction.find(self.src, du + 1);
            }
        }
        for kind in 0..4 {
            if self.dir_at[kind] >= qi {
                return kind as isize;
            }
        }
        -1
    }

    /// Ports `stereotype`.
    fn stereotype(&mut self, q: usize) -> isize {
        let qi = q as isize;
        if qi < self.stereo_from || qi > self.stereo_to {
            let first = self.next_stereotype.find(self.src, q);
            let end = if first == -1 {
                self.n
            } else {
                self.line_end(q)
            };
            self.stereo_from = qi;
            self.stereo_to = end as isize;
            self.stereo_at = [-1; 6];
            let mut d = first;
            while d != -1 && (d as usize) < end {
                let du = d as usize;
                let letter = self.c(du + 2) | 32;
                let k = (if self.c(du) == 60 { 0 } else { 3 })
                    + if letter == 102 {
                        0
                    } else if letter == 106 {
                        1
                    } else {
                        2
                    };
                self.stereo_at[k] = d;
                d = self.next_stereotype.find(self.src, du + 1);
            }
        }
        for kind in 0..6 {
            if self.stereo_at[kind] >= qi {
                return kind as isize;
            }
        }
        -1
    }

    /// Ports `emitId`: the grammar's processId(), where an id ends at an
    /// inline `%%`, and a text that starts with one is dropped.
    fn emit_id(&mut self, s: usize, e: usize) -> bool {
        let at = self.next_percent.find(self.src, s);
        if at == -1 || at as usize + 2 > e {
            self.emit(t::ID, s, e);
            return true;
        }
        if at as usize == s {
            self.p = e;
            return false;
        }
        self.emit(t::ID, s, at as usize);
        true
    }

    fn finish(mut self, kind: u8) -> Tokens {
        self.out.types.push(kind);
        self.out.texts.push(String::new());
        self.out.starts.push(self.src.byte(self.p));
        self.out
    }

    /// The comment rule: `#` or `%%` not followed by `{`.
    fn comment(&self, c: u32) -> bool {
        c == 35 || (c == 37 && self.c(self.p + 1) == 37 && self.c(self.p + 2) != 123)
    }
}

/// What one step of the main loop decided.
enum Step {
    Next,
    Finish(u8),
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
        next_direction: Finder::new(Re::Direction),
        next_stereotype: Finder::new(Re::Stereotype),
        next_line_end: Finder::new(Re::LineEnd),
        next_percent: Finder::new(Re::Percent),
        next_brace: Finder::new(Re::Brace),
        dir_from: -1,
        dir_to: -1,
        dir_at: [-1; 4],
        dir_end: [0; 4],
        stereo_from: -1,
        stereo_to: -1,
        stereo_at: [-1; 6],
    };

    loop {
        if lx.p >= n {
            // Jison tries the current state's rules once more on the empty remainder.
            let kind = match lx.state {
                INITIAL => Some(t::NL),
                ACC_TITLE => Some(t::acc_title_value),
                ACC_DESCR => Some(t::acc_descr_value),
                ACC_DESCR_MULTILINE => Some(t::acc_descr_multiline_value),
                CLASSDEFID => Some(t::CLASSDEF_STYLEOPTS),
                CLASS_STYLE => Some(t::STYLECLASS),
                STYLEDEF_STYLES => Some(t::STYLEDEF_STYLEOPTS),
                STATE_STRING => Some(t::STATE_DESCR),
                FLOATING_NOTE => Some(t::NOTE_TEXT),
                STATE_ID | FLOATING_NOTE_ID => Some(t::ID),
                _ => None,
            };
            if let Some(k) = kind {
                lx.emit(k, n, n);
            }
            return lx.finish(t::END);
        }
        let c = lx.c(lx.p);
        match step(&mut lx, c) {
            Step::Next => {}
            Step::Finish(k) => return lx.finish(k),
        }
    }
}

fn step(lx: &mut Lexer, c: u32) -> Step {
    let n = lx.n;
    let p = lx.p;
    match lx.state {
        INITIAL | STRUCT => {
            let root = lx.state == INITIAL;
            let lc = c | 32;
            if root {
                if lc == 99 && lx.word("click", p) && !is_word(lx.c(p + 5)) {
                    lx.emit(t::CLICK, p, p + 5);
                    return Step::Next;
                }
                if lc == 104 && lx.word("href", p) && !is_word(lx.c(p + 4)) {
                    lx.emit(t::HREF, p, p + 4);
                    return Step::Next;
                }
                if c == 34 {
                    if let Some(close) = lx.src.find_char(34, p + 1) {
                        lx.emit(t::STRING, p, close + 1);
                        return Step::Next;
                    }
                }
                if lc == 100 && lx.word("default", p) && !is_word(lx.c(p + 7)) {
                    lx.emit(t::DEFAULT, p, p + 7);
                    return Step::Next;
                }
                let kind = lx.direction(p);
                if kind != -1 {
                    let e = lx.eol(lx.dir_end[kind as usize]);
                    lx.emit(t::direction_tb + kind as u8, p, e);
                    return Step::Next;
                }
                if c == 10 {
                    let mut j = p + 1;
                    while lx.c(j) == 10 {
                        j += 1;
                    }
                    lx.emit(t::NL, p, j);
                    return Step::Next;
                }
                if is_space(c) {
                    lx.p = lx.ws_end(p);
                    return Step::Next;
                }
            } else if c != 10 && is_space(c) {
                let mut j = p + 1;
                while j < n && lx.c(j) != 10 && is_space(lx.c(j)) {
                    j += 1;
                }
                lx.p = j;
                return Step::Next;
            }
            if lx.comment(c) {
                lx.p = lx.eol(p);
                return Step::Next;
            }
            if lc == 115 {
                if root && lx.word("scale", p) {
                    let e = lx.ws_end(p + 5);
                    if e > p + 5 {
                        lx.emit(t::scale, p, e);
                        lx.push(SCALE);
                        return Step::Next;
                    }
                }
                if lx.word("style", p) {
                    let e = lx.ws_end(p + 5);
                    if e > p + 5 {
                        lx.emit(t::style, p, e);
                        lx.push(STYLE);
                        return Step::Next;
                    }
                }
                if lx.word("state", p) {
                    let e = lx.ws_end(p + 5);
                    if e > p + 5 {
                        lx.p = e;
                        lx.push(STATE);
                        return Step::Next;
                    }
                    if root && lx.word("diagram", p + 5) {
                        let from = if lx.word("-v2", p + 12) {
                            p + 15
                        } else {
                            p + 12
                        };
                        let e = lx.ws_end(from);
                        if e > from {
                            lx.emit(t::SD, p, e);
                            return Step::Next;
                        }
                    }
                }
            } else if lc == 99 {
                if lx.word("class", p) {
                    let def = lx.word("def", p + 5);
                    let from = if def { p + 8 } else { p + 5 };
                    let e = lx.ws_end(from);
                    if e > from {
                        lx.emit(if def { t::classDef } else { t::class }, p, e);
                        lx.push(if def { CLASSDEF } else { CLASS });
                        return Step::Next;
                    }
                }
            } else if lc == 97 && root {
                if lx.word("acctitle", p) || lx.word("accdescr", p) {
                    let title = (lx.c(p + 3) | 32) == 116;
                    let q = lx.ws_end(p + 8);
                    let d = lx.c(q);
                    if d == 58 {
                        let e = lx.ws_end(q + 1);
                        lx.emit(if title { t::acc_title } else { t::acc_descr }, p, e);
                        lx.push(if title { ACC_TITLE } else { ACC_DESCR });
                        return Step::Next;
                    }
                    if d == 123 && !title {
                        lx.p = lx.ws_end(q + 1);
                        lx.push(ACC_DESCR_MULTILINE);
                        return Step::Next;
                    }
                }
            }
            if root {
                if c == 123 {
                    lx.emit(t::STRUCT_START, p, p + 1);
                    lx.push(STRUCT);
                    return Step::Next;
                }
            } else {
                let kind = lx.direction(p);
                if kind != -1 {
                    let e = lx.eol(lx.dir_end[kind as usize]);
                    lx.emit(t::direction_tb + kind as u8, p, e);
                    return Step::Next;
                }
                if c == 125 {
                    lx.emit(t::STRUCT_STOP, p, p + 1);
                    lx.pop();
                    return Step::Next;
                }
                if c == 10 {
                    lx.p += 1;
                    return Step::Next;
                }
            }
            if lc == 110 && lx.word("note", p) {
                let e = lx.ws_end(p + 4);
                if e > p + 4 {
                    lx.emit(t::note, p, e);
                    lx.push(NOTE);
                    return Step::Next;
                }
            }
            if root && lc == 104 && lx.word("hide empty description", p) && !is_word(lx.c(p + 22)) {
                lx.emit(t::HIDE_EMPTY, p, p + 22);
                return Step::Next;
            }
            if c == 91 && lx.c(p + 1) == 42 && lx.c(p + 2) == 93 {
                lx.emit(t::EDGE_STATE, p, p + 3);
                return Step::Next;
            }
            let mut j = p;
            while j < n {
                let d = lx.c(j);
                if d == 58 || d == 45 || d == 123 || is_space(d) {
                    break;
                }
                j += 1;
            }
            if j > p {
                lx.emit_id(p, j);
                return Step::Next;
            }
            if c == 58 {
                j = p + 1;
                while j < n {
                    let d = lx.c(j);
                    if d == 10 || d == 59 {
                        break;
                    }
                    if d == 58 {
                        let after = lx.c(j + 1);
                        if j + 1 >= n || after == 58 || after == 10 || after == 59 {
                            break;
                        }
                        j += 1;
                    }
                    j += 1;
                }
                if j > p + 1 {
                    let text = trim(lx.src.slice(p, j)).into();
                    lx.emit_text(t::DESCR, text, p, j);
                    return Step::Next;
                }
                if lx.c(p + 1) == 58 && lx.c(p + 2) == 58 {
                    lx.emit(t::STYLE_SEPARATOR, p, p + 3);
                    return Step::Next;
                }
            } else if c == 45 && lx.c(p + 1) == 45 {
                if lx.c(p + 2) == 62 {
                    lx.emit(t::ARROW, p, p + 3);
                    return Step::Next;
                }
                if !root {
                    lx.emit(t::CONCURRENT, p, p + 2);
                    return Step::Next;
                }
            }
            if !root {
                return Step::Finish(t::ERROR);
            }
            lx.emit(t::INVALID, p, p + 1);
            Step::Next
        }

        STATE => {
            if c != 10 && is_space(c) {
                let mut j = p + 1;
                while j < n && lx.c(j) != 10 && is_space(lx.c(j)) {
                    j += 1;
                }
                lx.p = j;
                return Step::Next;
            }
            if lx.comment(c) {
                lx.p = lx.eol(p);
                return Step::Next;
            }
            let kind = lx.stereotype(p);
            if kind != -1 {
                let at = lx.stereo_at[kind as usize] as usize;
                let choice = kind == 2 || kind == 5;
                let text = trim(lx.src.slice(p, at)).into();
                let k = if choice {
                    t::CHOICE
                } else if kind % 3 == 0 {
                    t::FORK
                } else {
                    t::JOIN
                };
                lx.emit_text(k, text, p, at + if choice { 10 } else { 8 });
                lx.pop();
                return Step::Next;
            }
            if c == 34 {
                lx.p += 1;
                lx.push(STATE_STRING);
                return Step::Next;
            }
            let q = if c == 10 { lx.ws_end(p) } else { p };
            if lx.word("as", q) {
                let e = lx.ws_end(q + 2);
                if e > q + 2 {
                    lx.emit(t::AS, p, e);
                    lx.push(STATE_ID);
                    return Step::Next;
                }
            }
            if c == 10 {
                lx.p += 1;
                lx.pop();
                return Step::Next;
            }
            if c == 123 {
                lx.emit(t::STRUCT_START, p, p + 1);
                lx.pop();
                lx.push(STRUCT);
                return Step::Next;
            }
            if is_word(c) {
                let first = lx.word_end(p);
                let second = lx.ws_end(first);
                if second > first && is_word(lx.c(second)) {
                    let brace = lx.next_brace.find(lx.src, second + 1);
                    if brace != -1 && (brace as usize) < lx.line_end(second) {
                        let found = trim(lx.src.slice(p, brace as usize + 1));
                        lx.out.error = Some(format!(
                            "Error: State name must be a single word. Found: \"{}\"",
                            found
                        ));
                        return Step::Finish(t::ERROR);
                    }
                }
            }
            let mut j = p + 1;
            while j < n {
                let d = lx.c(j);
                if d == 123 || is_space(d) {
                    break;
                }
                j += 1;
            }
            lx.emit(t::COMPOSIT_STATE, p, j);
            Step::Next
        }

        STATE_STRING => {
            if c == 34 {
                lx.p += 1;
                lx.pop();
                return Step::Next;
            }
            let close = lx.src.find_char(34, p).unwrap_or(n);
            lx.emit(t::STATE_DESCR, p, close);
            Step::Next
        }

        STATE_ID => {
            let mut j = p;
            while j < n {
                let d = lx.c(j);
                if d == 10 || d == 123 {
                    break;
                }
                j += 1;
            }
            if lx.emit_id(p, j) {
                lx.pop();
            }
            Step::Next
        }

        SCALE => {
            if (48..=57).contains(&c) {
                let mut j = p + 1;
                while j < n && (48..=57).contains(&lx.c(j)) {
                    j += 1;
                }
                lx.emit(t::WIDTH, p, j);
                return Step::Next;
            }
            let q = lx.ws_end(p);
            if q > p && lx.word("width", q) && !is_word(lx.c(q + 5)) {
                lx.p = q + 5;
                lx.pop();
                return Step::Next;
            }
            Step::Finish(t::ERROR)
        }

        ACC_TITLE => {
            let e = lx.eol(p);
            lx.emit(t::acc_title_value, p, e);
            lx.pop();
            Step::Next
        }

        ACC_DESCR => {
            let e = lx.eol(p);
            lx.emit(t::acc_descr_value, p, e);
            lx.pop();
            Step::Next
        }

        ACC_DESCR_MULTILINE => {
            if c == 125 {
                lx.p += 1;
                lx.pop();
                return Step::Next;
            }
            let close = lx.src.find_char(125, p).unwrap_or(n);
            lx.emit(t::acc_descr_multiline_value, p, close);
            Step::Next
        }

        CLASSDEF => {
            let is_default = lx.word("default", p) && lx.ws_end(p + 7) > p + 7;
            let j = if is_default { p + 7 } else { lx.word_end(p) };
            let e = lx.ws_end(j);
            if j == p || e == j {
                return Step::Finish(t::ERROR);
            }
            lx.emit(
                if is_default {
                    t::DEFAULT_CLASSDEF_ID
                } else {
                    t::CLASSDEF_ID
                },
                p,
                e,
            );
            lx.pop();
            lx.push(CLASSDEFID);
            Step::Next
        }

        CLASSDEFID => {
            let e = lx.eol(p);
            lx.emit(t::CLASSDEF_STYLEOPTS, p, e);
            lx.pop();
            Step::Next
        }

        CLASS => {
            let mut j = lx.word_end(p);
            if j == p {
                return Step::Finish(t::ERROR);
            }
            while lx.c(j) == 44 {
                let k = lx.ws_end(j + 1);
                let e = lx.word_end(k);
                if e == k {
                    break;
                }
                j = e;
            }
            lx.emit(t::CLASSENTITY_IDS, p, j);
            lx.pop();
            lx.push(CLASS_STYLE);
            Step::Next
        }

        CLASS_STYLE => {
            let e = lx.eol(p);
            lx.emit(t::STYLECLASS, p, e);
            lx.pop();
            Step::Next
        }

        STYLE => {
            let mut j = p;
            while j < n && (is_word(lx.c(j)) || lx.c(j) == 44) {
                j += 1;
            }
            let e = lx.ws_end(j);
            if j == p || e == j {
                return Step::Finish(t::ERROR);
            }
            lx.emit(t::STYLE_IDS, p, e);
            lx.pop();
            lx.push(STYLEDEF_STYLES);
            Step::Next
        }

        STYLEDEF_STYLES => {
            let e = lx.eol(p);
            lx.emit(t::STYLEDEF_STYLEOPTS, p, e);
            lx.pop();
            Step::Next
        }

        NOTE => {
            if lx.word("left of", p) && !is_word(lx.c(p + 7)) {
                lx.emit(t::left_of, p, p + 7);
                lx.pop();
                lx.push(NOTE_ID);
                return Step::Next;
            }
            if lx.word("right of", p) && !is_word(lx.c(p + 8)) {
                lx.emit(t::right_of, p, p + 8);
                lx.pop();
                lx.push(NOTE_ID);
                return Step::Next;
            }
            if c == 34 {
                lx.p += 1;
                lx.pop();
                lx.push(FLOATING_NOTE);
                return Step::Next;
            }
            Step::Finish(t::ERROR)
        }

        NOTE_ID => {
            let q = lx.ws_end(p);
            let mut j = q;
            while j < n {
                let d = lx.c(j);
                if d == 58 || d == 45 || is_space(d) {
                    break;
                }
                j += 1;
            }
            if j == q {
                return Step::Finish(t::ERROR);
            }
            if lx.emit_id(p, j) {
                lx.pop();
                lx.push(NOTE_TEXT);
            }
            Step::Next
        }

        NOTE_TEXT => {
            let q = lx.ws_end(p);
            if lx.c(q) == 58 {
                let mut j = q + 1;
                while j < n {
                    let d = lx.c(j);
                    if d == 58 || d == 10 || d == 59 {
                        break;
                    }
                    j += 1;
                }
                if j > q + 1 {
                    // Mermaid drops two characters here, expecting " :"; with no space before the colon
                    // the first character of the note goes too.
                    let text = trim(lx.src.slice(p + 2, j)).into();
                    lx.emit_text(t::NOTE_TEXT, text, p, j);
                    lx.pop();
                    return Step::Next;
                }
            }
            let mut k = lx.src.find_char(10, p);
            let mut e = 0usize;
            while let Some(kk) = k {
                e = lx.ws_end(kk);
                if lx.word("end note", e) && !is_word(lx.c(e + 8)) {
                    break;
                }
                k = lx.src.find_char(10, e);
            }
            if k.is_none() {
                return Step::Finish(t::ERROR);
            }
            let text = trim(lx.src.slice(p, e)).into();
            lx.emit_text(t::NOTE_TEXT, text, p, e + 8);
            lx.pop();
            Step::Next
        }

        FLOATING_NOTE => {
            let q = lx.ws_end(p);
            if lx.word("as", q) {
                let e = lx.ws_end(q + 2);
                lx.emit(t::AS, p, e);
                lx.pop();
                lx.push(FLOATING_NOTE_ID);
                return Step::Next;
            }
            if c == 34 {
                lx.p += 1;
                return Step::Next;
            }
            let close = lx.src.find_char(34, p).unwrap_or(n);
            lx.emit(t::NOTE_TEXT, p, close);
            Step::Next
        }

        _ => {
            // FLOATING_NOTE_ID
            let e = lx.eol(p);
            if lx.emit_id(p, e) {
                lx.pop();
            }
            Step::Next
        }
    }
}
