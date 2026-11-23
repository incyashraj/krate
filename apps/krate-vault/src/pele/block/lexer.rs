//! Ports diagrams/block/lexer.ts: the block diagram tokenizer, a hand port
//! of Mermaid's Jison lexer. Positions are character indices of a
//! [`Text`]; `starts` are byte offsets, as errors locate them.

#![allow(clippy::collapsible_match, clippy::collapsible_if)]

use alloc::string::String;
use alloc::vec::Vec;

use super::tokens as t;
use crate::pele::common::chars::{is_space, is_word, Text};

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
const MD_STRING: u8 = 2;
const NODE: u8 = 3;
const BLOCK_ARROW: u8 = 4;
const ARROW_DIR: u8 = 5;
const LLABEL: u8 = 6;
const CLASSDEF: u8 = 7;
const CLASSDEFID: u8 = 8;
const CLASS: u8 = 9;
const CLASS_STYLE: u8 = 10;
const STYLE_STMNT: u8 = 11;
const STYLE_DEFINITION: u8 = 12;
const ACC_TITLE: u8 = 13;
const ACC_DESCR: u8 = 14;
const ACC_DESCR_MULTILINE: u8 = 15;

const DIRECTIONS: [&str; 6] = ["right", "left", "x", "y", "up", "down"];

/// Ports `ID_STOP`: characters that end a node id, `( ) [ { } < > - : =`
/// and whitespace.
fn id_stop(c: u32) -> bool {
    matches!(
        c,
        9 | 10 | 11 | 12 | 13 | 32 | 40 | 41 | 45 | 58 | 60 | 61 | 62 | 91 | 123 | 125
    )
}

struct Lexer<'a> {
    src: &'a Text,
    n: usize,
    out: Tokens,
    stack: Vec<u8>,
    state: u8,
    p: usize,
}

impl Lexer<'_> {
    fn c(&self, i: usize) -> u32 {
        self.src.c(i)
    }

    fn put(&mut self, kind: u8, text: String, end: usize) -> bool {
        self.out.types.push(kind);
        self.out.texts.push(text);
        self.out.starts.push(self.src.byte(self.p));
        self.p = end;
        true
    }

    fn emit(&mut self, kind: u8, end: usize) -> bool {
        let text = String::from(self.src.slice(self.p, end));
        self.put(kind, text, end)
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

    fn ws(&self, mut q: usize) -> usize {
        while q < self.n && is_space(self.c(q)) {
            q += 1;
        }
        q
    }

    fn digits(&self, mut q: usize) -> usize {
        while q < self.n {
            let c = self.c(q);
            if !(48..=57).contains(&c) {
                break;
            }
            q += 1;
        }
        q
    }

    fn word(&self, mut q: usize) -> usize {
        while q < self.n && is_word(self.c(q)) {
            q += 1;
        }
        q
    }

    fn starts(&self, text: &str, at: usize) -> bool {
        self.src.starts_with(text, at)
    }

    fn keyword(&self, text: &str) -> bool {
        self.starts(text, self.p) && !is_word(self.c(self.p + text.chars().count()))
    }

    /// Ports `quote`.
    fn quote(&mut self) -> bool {
        if self.c(self.p + 1) == 96 {
            self.push(MD_STRING);
            self.p += 2;
        } else {
            self.push(STRING);
            self.p += 1;
        }
        true
    }

    /// Ports `dotted`: end of `.+-[xo>]?` at q, or -1.
    fn dotted(&self, mut q: usize) -> isize {
        if self.c(q) != 46 {
            return -1;
        }
        loop {
            q += 1;
            if self.c(q) != 46 {
                break;
            }
        }
        if self.c(q) != 45 {
            return -1;
        }
        let c = self.c(q + 1);
        (if c == 120 || c == 111 || c == 62 {
            q + 2
        } else {
            q + 1
        }) as isize
    }

    /// Ports `linkEnd`: end of a complete link at q, before its trailing
    /// whitespace, or -1.
    fn link_end(&self, mut q: usize) -> isize {
        let c = self.c(q);
        if c == 120 || c == 111 || c == 60 {
            q += 1;
        }
        let line = self.c(q);
        if line == 46 {
            return self.dotted(q);
        }
        if line != 45 && line != 61 {
            return -1;
        }
        let mut k = q + 1;
        while self.c(k) == line {
            k += 1;
        }
        if k - q == 1 {
            return if line == 45 { self.dotted(k) } else { -1 };
        }
        let head = self.c(k);
        if head == 120 || head == 111 || head == 62 {
            return (k + 1) as isize;
        }
        if k - q >= 3 {
            k as isize
        } else {
            -1
        }
    }

    /// Ports `link`.
    fn link(&mut self) -> bool {
        let p = self.p;
        let e = self.link_end(p);
        if e >= 0 {
            let end = self.ws(e as usize);
            return self.emit(t::LINK, end);
        }
        let q = if self.c(p) == 60 { p + 1 } else { p };
        let a = self.c(q);
        let b = self.c(q + 1);
        if (a == 45 && (b == 45 || b == 46)) || (a == 61 && b == 61) {
            self.push(LLABEL);
            let end = self.ws(q + 2);
            return self.emit(t::START_LINK, end);
        }
        false
    }

    /// Ports `lineState`.
    fn line_state(&mut self, kind: u8) -> bool {
        let e = self.src.find_char(10, self.p).unwrap_or(self.n);
        self.pop();
        self.emit(kind, e)
    }

    /// Ports `idList`: `\w+(,\s*\w+)*`.
    fn id_list(&mut self, kind: u8, next: u8) -> bool {
        let p = self.p;
        let mut k = self.word(p);
        if k == p {
            return false;
        }
        while self.c(k) == 44 {
            let from = self.ws(k + 1);
            let to = self.word(from);
            if to == from {
                break;
            }
            k = to;
        }
        self.state = next;
        self.emit(kind, k)
    }

    /// Ports `initial`.
    fn initial(&mut self, c: u32) -> bool {
        let p = self.p;
        if is_space(c) {
            self.p = self.ws(p + 1);
            return true;
        }
        match c {
            98 => {
                if self.starts("block", p) {
                    if self.keyword("block-beta") {
                        return self.emit(t::KEY, p + 10);
                    }
                    if self.c(p + 5) == 58 {
                        return self.emit(t::ID_BLOCK, p + 6);
                    }
                    if !is_word(self.c(p + 5)) {
                        return self.emit(t::KEY, p + 5);
                    }
                }
            }
            99 => {
                if self.starts("columns", p) {
                    let q = self.ws(p + 7);
                    if q > p + 7 {
                        if self.starts("auto", q) && !is_word(self.c(q + 4)) {
                            return self.put(t::COLUMNS, "-1".into(), q + 4);
                        }
                        let e = self.digits(q);
                        if e > q {
                            let text = String::from(self.src.slice(q, e));
                            return self.put(t::COLUMNS, text, e);
                        }
                    }
                } else if self.starts("class", p) {
                    let def = self.starts("Def", p + 5);
                    let from = if def { p + 8 } else { p + 5 };
                    let q = self.ws(from);
                    if q > from {
                        self.push(if def { CLASSDEF } else { CLASS });
                        return self.emit(if def { t::CLASSDEF } else { t::CLASS }, q);
                    }
                }
            }
            34 => return self.quote(),
            115 => {
                if self.starts("space", p) {
                    if self.c(p + 5) == 58 {
                        let e = self.digits(p + 6);
                        if e > p + 6 {
                            let text = String::from(self.src.slice(p + 6, e));
                            return self.put(t::SPACE_BLOCK, text, e);
                        }
                    }
                    if !is_word(self.c(p + 5)) {
                        return self.put(t::SPACE_BLOCK, "1".into(), p + 5);
                    }
                } else if self.starts("style", p) {
                    let q = self.ws(p + 5);
                    if q > p + 5 {
                        self.push(STYLE_STMNT);
                        return self.emit(t::STYLE, q);
                    }
                }
            }
            100 => {
                if self.keyword("default") {
                    return self.emit(t::DEFAULT, p + 7);
                }
            }
            108 => {
                if self.keyword("linkStyle") {
                    return self.emit(t::LINKSTYLE, p + 9);
                }
            }
            105 => {
                if self.keyword("interpolate") {
                    return self.emit(t::INTERPOLATE, p + 11);
                }
            }
            97 => {
                if self.starts("accTitle", p) {
                    let q = self.ws(p + 8);
                    if self.c(q) == 58 {
                        self.push(ACC_TITLE);
                        let e = self.ws(q + 1);
                        return self.emit(t::acc_title, e);
                    }
                } else if self.starts("accDescr", p) {
                    let q = self.ws(p + 8);
                    let d = self.c(q);
                    if d == 58 {
                        self.push(ACC_DESCR);
                        let e = self.ws(q + 1);
                        return self.emit(t::acc_descr, e);
                    }
                    if d == 123 {
                        self.push(ACC_DESCR_MULTILINE);
                        self.p = self.ws(q + 1);
                        return true;
                    }
                }
            }
            101 => {
                if self.keyword("end") {
                    let e = self.ws(p + 3);
                    return self.emit(t::end, e);
                }
            }
            45 => {
                if self.c(p + 1) == 41 {
                    self.push(NODE);
                    return self.emit(t::NODE_DSTART, p + 2);
                }
                return self.link();
            }
            61 => return self.link(),
            60 => {
                if self.c(p + 1) == 91 {
                    self.push(BLOCK_ARROW);
                    return self.emit(t::BLOCK_ARROW_START, p + 2);
                }
                return self.link();
            }
            40 => {
                let d = self.c(p + 1);
                let len = if d == 45 || d == 91 {
                    2
                } else if d == 40 {
                    if self.c(p + 2) == 40 {
                        3
                    } else {
                        2
                    }
                } else {
                    1
                };
                self.push(NODE);
                return self.emit(t::NODE_DSTART, p + len);
            }
            41 | 123 => {
                self.push(NODE);
                let e = if self.c(p + 1) == c { p + 2 } else { p + 1 };
                return self.emit(t::NODE_DSTART, e);
            }
            62 => {
                self.push(NODE);
                return self.emit(t::NODE_DSTART, p + 1);
            }
            91 => {
                let d = self.c(p + 1);
                self.push(NODE);
                let e = if matches!(d, 91 | 124 | 40 | 92 | 47) {
                    p + 2
                } else {
                    p + 1
                };
                return self.emit(t::NODE_DSTART, e);
            }
            58 => {
                let e = self.digits(p + 1);
                if e > p + 1 {
                    let text = String::from(self.src.slice(p + 1, e));
                    return self.put(t::SIZE, text, e);
                }
                return false;
            }
            _ => {}
        }

        let mut q = p;
        while q < self.n {
            let ch = self.c(q);
            if if ch < 128 { id_stop(ch) } else { is_space(ch) } {
                break;
            }
            q += 1;
        }
        q > p && self.emit(t::NODE_ID, q)
    }

    /// Ports `nodeEnd`.
    fn node_end(&mut self, c: u32) -> bool {
        let p = self.p;
        let d = self.c(p + 1);
        let len = match c {
            34 => return self.quote(),
            40 => {
                if d == 45 {
                    2
                } else if d == 40 {
                    if self.c(p + 2) == 40 {
                        3
                    } else {
                        2
                    }
                } else {
                    1
                }
            }
            41 => {
                if d == 41 {
                    if self.c(p + 2) == 41 {
                        3
                    } else {
                        2
                    }
                } else if d == 93 {
                    2
                } else {
                    1
                }
            }
            125 => {
                if d == 125 {
                    2
                } else {
                    1
                }
            }
            45 => {
                if d == 41 {
                    2
                } else {
                    0
                }
            }
            93 => {
                if d == 93 || d == 41 || d == 62 {
                    2
                } else {
                    1
                }
            }
            92 | 47 => {
                if d == 93 {
                    2
                } else {
                    0
                }
            }
            _ => 0,
        };
        if len == 0 {
            return false;
        }
        self.pop();
        self.emit(t::NODE_DEND, p + len)
    }

    /// Ports `arrowDir`.
    fn arrow_dir(&mut self, c: u32) -> bool {
        let p = self.p;
        if c == 41 {
            self.pop();
            self.pop();
            let e = self.ws(p + 1);
            return self.put(t::BLOCK_ARROW_END, "]>".into(), e);
        }
        let from = self.ws(if c == 44 { p + 1 } else { p });
        for dir in DIRECTIONS {
            if self.starts(dir, from) {
                let e = self.ws(from + dir.len());
                let text = String::from(self.src.slice(if c == 44 { from } else { p }, e));
                return self.put(t::DIR, text, e);
            }
        }
        false
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
    };

    loop {
        if lx.p >= n {
            let kind = match lx.state {
                INITIAL => Some(t::EOF),
                STRING => Some(t::STR),
                CLASSDEFID => Some(t::CLASSDEF_STYLEOPTS),
                CLASS_STYLE => Some(t::STYLECLASS),
                STYLE_DEFINITION => Some(t::STYLE_DEFINITION_DATA),
                ACC_TITLE => Some(t::acc_title_value),
                ACC_DESCR => Some(t::acc_descr_value),
                ACC_DESCR_MULTILINE => Some(t::acc_descr_multiline_value),
                _ => None,
            };
            lx.p = n;
            if let Some(k) = kind {
                lx.put(k, String::new(), n);
            }
            break;
        }

        let p = lx.p;
        let c = lx.c(p);
        let mut ok = true;
        match lx.state {
            INITIAL => ok = lx.initial(c),
            STRING => {
                if c == 34 {
                    lx.pop();
                    lx.p += 1;
                } else {
                    let e = src.find_char(34, p).unwrap_or(n);
                    lx.emit(t::STR, e);
                }
            }
            MD_STRING => {
                if c == 96 {
                    ok = lx.c(p + 1) == 34;
                    if ok {
                        lx.pop();
                        lx.p += 2;
                    }
                } else if c == 34 {
                    ok = false;
                } else {
                    let mut q = p + 1;
                    while q < n {
                        let ch = lx.c(q);
                        if ch == 96 || ch == 34 {
                            break;
                        }
                        q += 1;
                    }
                    lx.emit(t::MD_STR, q);
                }
            }
            NODE => ok = lx.node_end(c),
            BLOCK_ARROW => {
                if c == 34 {
                    lx.quote();
                } else if c == 93 && lx.c(p + 1) == 62 {
                    let q = lx.ws(p + 2);
                    ok = lx.c(q) == 40;
                    if ok {
                        lx.push(ARROW_DIR);
                        lx.p = q + 1;
                    }
                } else {
                    ok = false;
                }
            }
            ARROW_DIR => ok = lx.arrow_dir(c),
            LLABEL => {
                if c == 34 {
                    if lx.c(p + 1) == 96 {
                        lx.quote();
                    } else {
                        lx.emit(t::LINK_LABEL, p + 1);
                        lx.push(STRING);
                    }
                } else {
                    let e = lx.link_end(lx.ws(p));
                    ok = e >= 0;
                    if ok {
                        lx.pop();
                        let end = lx.ws(e as usize);
                        lx.emit(t::LINK, end);
                    }
                }
            }
            CLASSDEF => {
                let mut kind = t::DEFAULT_CLASSDEF_ID;
                let mut k = if lx.starts("DEFAULT", p) { p + 7 } else { p };
                let mut q = lx.ws(k);
                if k == p || q == k {
                    kind = t::CLASSDEF_ID;
                    k = lx.word(p);
                    q = lx.ws(k);
                }
                ok = k > p && q > k;
                if ok {
                    lx.state = CLASSDEFID;
                    lx.emit(kind, q);
                }
            }
            CLASSDEFID => {
                lx.line_state(t::CLASSDEF_STYLEOPTS);
            }
            CLASS => ok = lx.id_list(t::CLASSENTITY_IDS, CLASS_STYLE),
            CLASS_STYLE => {
                lx.line_state(t::STYLECLASS);
            }
            STYLE_STMNT => ok = lx.id_list(t::STYLE_ENTITY_IDS, STYLE_DEFINITION),
            STYLE_DEFINITION => {
                lx.line_state(t::STYLE_DEFINITION_DATA);
            }
            ACC_TITLE => {
                lx.line_state(t::acc_title_value);
            }
            ACC_DESCR => {
                lx.line_state(t::acc_descr_value);
            }
            _ => {
                // ACC_DESCR_MULTILINE
                if c == 125 {
                    lx.pop();
                    lx.p += 1;
                } else {
                    let e = src.find_char(125, p).unwrap_or(n);
                    lx.emit(t::acc_descr_multiline_value, e);
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
