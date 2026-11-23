//! Ports diagrams/sequence/lexer.ts: the Jison lexer of Mermaid's
//! sequence grammar, by hand. Positions are character indices of a [`Text`].

use alloc::string::String;
use alloc::vec::Vec;

use super::tokens::*;
use crate::pele::common::chars::{is_digit, is_space, is_word, Text};
use crate::pele::js::trim;

/// Ports `interface Tokens`.
#[derive(Debug, Clone, Default)]
pub struct Tokens {
    pub types: Vec<i32>,
    pub texts: Vec<String>,
    pub starts: Vec<usize>,
}

// `const enum S`.
const INITIAL: u8 = 0;
const ID: u8 = 1;
const ALIAS: u8 = 2;
const LINE: u8 = 3;
const CONFIG: u8 = 4;
const S_ACC_TITLE: u8 = 5;
const S_ACC_DESCR: u8 = 6;
const S_ACC_DESCR_MULTILINE: u8 = 7;

// `const enum C`: character classes of Mermaid's lexer rules.
const ACTOR_START: u8 = 1;
const ACTOR_REST: u8 = 2;
const C_ID: u8 = 4;
const ID_WIDE: u8 = 8;
const C_LINE: u8 = 16;
const MENU: u8 = 32;

/// `CLASS[c]` for an ASCII character.
fn class(c: u32) -> u8 {
    let ch = char::from_u32(c).unwrap_or('\0');
    let ws = c == 32 || (9..=13).contains(&c);
    let mut bits = 0;
    if !"/\\+()<->:\n,;".contains(ch) {
        bits |= ACTOR_START;
    }
    if !"+<->:\n,;".contains(ch) {
        bits |= ACTOR_REST;
    }
    if !"<>:\n,;@".contains(ch) {
        bits |= ID_WIDE | if ws { 0 } else { C_ID };
    }
    if !"#\n;".contains(ch) {
        bits |= C_LINE;
    }
    if !"/\\+()<>:\n,;-".contains(ch) && !ws {
        bits |= MENU;
    }
    bits
}

struct Lexer<'a> {
    src: &'a Text,
    n: usize,
    out: Tokens,
    stack: Vec<u8>,
    state: u8,
    p: usize,
    next_nl: isize,
}

impl Lexer<'_> {
    fn c(&self, q: usize) -> u32 {
        self.src.c(q)
    }

    fn emit(&mut self, ty: i32, s: usize, e: usize) {
        self.out.types.push(ty);
        self.out.texts.push(String::from(self.src.slice(s, e)));
        self.out.starts.push(s);
        self.p = e;
    }

    fn emit_text(&mut self, ty: i32, s: usize, e: usize, text: String) {
        self.out.types.push(ty);
        self.out.texts.push(text);
        self.out.starts.push(s);
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

    fn eol(&mut self, q: usize) -> usize {
        if self.next_nl < q as isize {
            self.next_nl = self.src.find_char(10, q).unwrap_or(self.n) as isize;
        }
        self.next_nl as usize
    }

    fn has(&self, q: usize, bit: u8) -> bool {
        let c = self.c(q);
        if c < 128 {
            return class(c) & bit != 0;
        }
        if bit == C_ID || bit == MENU {
            !is_space(c)
        } else {
            true
        }
    }

    /// Matches a lowercase literal without regard to the case of the source.
    fn ci(&self, q: usize, word: &str) -> bool {
        for (k, w) in word.bytes().enumerate() {
            let w = w as u32;
            let c = self.c(q + k);
            let ok = if (97..=122).contains(&w) {
                c != u32::MAX && (c | 32) == w
            } else {
                c == w
            };
            if !ok {
                return false;
            }
        }
        true
    }

    fn word(&self, lit: &str) -> bool {
        self.ci(self.p, lit) && !is_word(self.c(self.p + lit.len()))
    }

    fn skip_ws(&self, mut q: usize) -> usize {
        while q < self.n && is_space(self.c(q)) {
            q += 1;
        }
        q
    }

    fn skip_same_line(&mut self) -> bool {
        let mut q = self.p;
        while q < self.n {
            let c = self.c(q);
            if c == 10 || !is_space(c) {
                break;
            }
            q += 1;
        }
        if q == self.p {
            return false;
        }
        self.p = q;
        true
    }

    fn line_run(&self, mut q: usize) -> usize {
        while q < self.n && self.has(q, C_LINE) {
            q += 1;
        }
        q
    }

    fn begin(&mut self, ty: i32, len: usize, next: u8) {
        self.emit(ty, self.p, self.p + len);
        self.push(next);
    }

    fn keyword(&mut self, lit: &str, ty: i32, next: Option<u8>) -> bool {
        if !self.word(lit) {
            return false;
        }
        self.emit(ty, self.p, self.p + lit.len());
        if let Some(s) = next {
            self.push(s);
        }
        true
    }

    /// The actor menu keywords count only when spaces and the start of a name follow.
    fn menu(&mut self, lit: &str, ty: i32) -> bool {
        if !self.ci(self.p, lit) {
            return false;
        }
        let mut q = self.p + lit.len();
        let mut c = self.c(q);
        if c != 32 && c != 9 {
            return false;
        }
        while c == 32 || c == 9 {
            q += 1;
            c = self.c(q);
        }
        if q >= self.n || !self.has(q, MENU) {
            return false;
        }
        self.emit(ty, self.p, self.p + lit.len());
        true
    }

    fn arrow_ahead(&self, q: usize) -> bool {
        let c = self.c(q);
        let d = self.c(q + 1);
        if c == 45 {
            if d == 120 || d == 88 || d == 41 || d == 92 || d == 47 || d == 45 {
                return true;
            }
            let e = self.c(q + 2);
            return d == 124 && (e == 92 || e == 47);
        }
        if c == 47 || c == 92 {
            return (d == 124 || d == c) && self.c(q + 2) == 45;
        }
        c == 40 && d == 41
    }

    fn actor(&mut self) {
        let n = self.n;
        let mut q = self.p + 1;
        while q < n && self.has(q, ACTOR_START) {
            q += 1;
        }
        while q < n && !self.arrow_ahead(q) {
            let mut r = q;
            while self.c(r) == 45 {
                r += 1;
            }
            if r >= n || !self.has(r, ACTOR_REST) {
                break;
            }
            r += 1;
            while r < n && self.has(r, ACTOR_REST) {
                r += 1;
            }
            q = r;
        }
        let text = String::from(trim(self.src.slice(self.p, q)));
        self.emit_text(ACTOR, self.p, q, text);
    }

    fn number(&mut self) -> bool {
        let p = self.p;
        let mut q = p;
        while is_digit(self.c(q)) {
            q += 1;
        }
        let whole = q > p;
        if self.c(q) == 46 && is_digit(self.c(q + 1)) {
            let r = if is_digit(self.c(q + 2)) {
                q + 3
            } else {
                q + 2
            };
            let after = self.c(r);
            if after == 32 || after == 10 {
                self.emit(NUM, p, r);
                return true;
            }
        }
        let after = self.c(q);
        if whole && (after == 32 || after == 10) {
            self.emit(NUM, p, q);
            return true;
        }
        false
    }

    fn dash(&mut self) {
        let p = self.p;
        let d = self.c(p + 1);
        let e = self.c(p + 2);
        if d == 62 {
            if e == 62 {
                self.emit(SOLID_ARROW, p, p + 3);
            } else {
                self.emit(SOLID_OPEN_ARROW, p, p + 2);
            }
        } else if d == 120 || d == 88 {
            self.emit(SOLID_CROSS, p, p + 2);
        } else if d == 41 {
            self.emit(SOLID_POINT, p, p + 2);
        } else if d == 124 && e == 92 {
            self.emit(SOLID_ARROW_TOP, p, p + 3);
        } else if d == 124 && e == 47 {
            self.emit(SOLID_ARROW_BOTTOM, p, p + 3);
        } else if d == 92 && e == 92 {
            self.emit(STICK_ARROW_TOP, p, p + 3);
        } else if d == 47 && e == 47 {
            self.emit(STICK_ARROW_BOTTOM, p, p + 3);
        } else if d == 45 {
            let f = self.c(p + 3);
            if e == 62 {
                if f == 62 {
                    self.emit(DOTTED_ARROW, p, p + 4);
                } else {
                    self.emit(DOTTED_OPEN_ARROW, p, p + 3);
                }
            } else if e == 120 || e == 88 {
                self.emit(DOTTED_CROSS, p, p + 3);
            } else if e == 41 {
                self.emit(DOTTED_POINT, p, p + 3);
            } else if e == 124 && f == 92 {
                self.emit(SOLID_ARROW_TOP_DOTTED, p, p + 4);
            } else if e == 124 && f == 47 {
                self.emit(SOLID_ARROW_BOTTOM_DOTTED, p, p + 4);
            } else if e == 92 && f == 92 {
                self.emit(STICK_ARROW_TOP_DOTTED, p, p + 4);
            } else if e == 47 && f == 47 {
                self.emit(STICK_ARROW_BOTTOM_DOTTED, p, p + 4);
            } else {
                self.emit(MINUS, p, p + 1);
            }
        } else {
            self.emit(MINUS, p, p + 1);
        }
    }

    fn reverse(&mut self, c: u32) -> bool {
        let p = self.p;
        let d = self.c(p + 1);
        if (d != 124 && d != c) || self.c(p + 2) != 45 {
            return false;
        }
        let dotted = self.c(p + 3) == 45;
        let top = c == 47;
        let ty = if d == 124 {
            match (dotted, top) {
                (true, true) => SOLID_ARROW_TOP_REVERSE_DOTTED,
                (true, false) => SOLID_ARROW_BOTTOM_REVERSE_DOTTED,
                (false, true) => SOLID_ARROW_TOP_REVERSE,
                (false, false) => SOLID_ARROW_BOTTOM_REVERSE,
            }
        } else {
            match (dotted, top) {
                (true, true) => STICK_ARROW_TOP_REVERSE_DOTTED,
                (true, false) => STICK_ARROW_BOTTOM_REVERSE_DOTTED,
                (false, true) => STICK_ARROW_TOP_REVERSE,
                (false, false) => STICK_ARROW_BOTTOM_REVERSE,
            }
        };
        self.emit(ty, p, p + if dotted { 4 } else { 3 });
        true
    }

    fn title(&mut self) -> bool {
        if !self.ci(self.p, "title") {
            return false;
        }
        let n = self.n;
        let mut q = self.p + 5;
        let legacy = self.c(q) == 58;
        if legacy {
            q += 1;
        }
        let mut e = q;
        while e < n && self.c(e) != 10 && is_space(self.c(e)) {
            e += 1;
        }
        if e == n || self.c(e) == 10 {
            self.p = e;
            return true;
        }
        let sep = self.c(q);
        if !is_space(sep) || sep == 13 || !self.has(q + 1, C_LINE) {
            return false;
        }
        let end = self.line_run(q + 1);
        self.emit(if legacy { T_LEGACY_TITLE } else { T_TITLE }, self.p, end);
        true
    }

    fn acc(&mut self) -> bool {
        let descr = self.ci(self.p, "accdescr");
        if !descr && !self.ci(self.p, "acctitle") {
            return false;
        }
        let q = self.skip_ws(self.p + 8);
        let c = self.c(q);
        if c == 58 {
            let e = self.skip_ws(q + 1);
            self.emit(if descr { T_ACC_DESCR } else { T_ACC_TITLE }, self.p, e);
            self.push(if descr { S_ACC_DESCR } else { S_ACC_TITLE });
            return true;
        }
        if descr && c == 123 {
            self.p = self.skip_ws(q + 1);
            self.push(S_ACC_DESCR_MULTILINE);
            return true;
        }
        false
    }

    fn keywords(&mut self, c: u32) -> bool {
        match c | 32 {
            97 => {
                self.keyword("actor", T_PARTICIPANT_ACTOR, Some(ID))
                    || self.keyword("alt", T_ALT, Some(LINE))
                    || self.keyword("and", T_AND, Some(LINE))
                    || self.keyword("activate", T_ACTIVATE, Some(ID))
                    || self.acc()
                    || self.keyword("autonumber", T_AUTONUMBER, None)
            }
            98 => {
                self.keyword("box", T_BOX, Some(LINE)) || self.keyword("break", T_BREAK, Some(LINE))
            }
            99 => {
                self.keyword("create", T_CREATE, None)
                    || self.keyword("critical", T_CRITICAL, Some(LINE))
            }
            100 => {
                self.keyword("destroy", T_DESTROY, Some(ID))
                    || self.menu("details", T_DETAILS)
                    || self.keyword("deactivate", T_DEACTIVATE, Some(ID))
            }
            101 => self.keyword("else", T_ELSE, Some(LINE)) || self.keyword("end", T_END, None),
            108 => {
                self.keyword("loop", T_LOOP, Some(LINE))
                    || self.keyword("left of", T_LEFT_OF, None)
                    || self.menu("links", T_LINKS)
                    || self.menu("link", T_LINK)
            }
            110 => self.keyword("note", T_NOTE, None),
            111 => {
                self.keyword("opt", T_OPT, Some(LINE))
                    || self.keyword("option", T_OPTION, Some(LINE))
                    || self.keyword("over", T_OVER, None)
                    || self.keyword("off", T_OFF, None)
            }
            112 => {
                self.keyword("participant", T_PARTICIPANT, Some(ID))
                    || self.keyword("par", T_PAR, Some(LINE))
                    || self.keyword("par_over", T_PAR_OVER, Some(LINE))
                    || self.menu("properties", T_PROPERTIES)
            }
            114 => {
                self.keyword("rect", T_RECT, Some(LINE))
                    || self.keyword("right of", T_RIGHT_OF, None)
            }
            115 => self.keyword("sequencediagram", SD, None),
            116 => self.title(),
            _ => false,
        }
    }

    fn initial(&mut self) {
        let p = self.p;
        let c = self.c(p);
        if c == 10 {
            let mut q = p + 1;
            while self.c(q) == 10 {
                q += 1;
            }
            self.emit(NEWLINE, p, q);
            return;
        }
        if is_space(c) {
            self.p = self.skip_ws(p + 1);
            return;
        }
        if c == 35 || (c == 37 && self.c(p + 1) != 123) {
            self.p = self.eol(p);
            return;
        }
        if (is_digit(c) || c == 46) && self.number() {
            return;
        }
        if c < 128 && (97..=122).contains(&(c | 32)) && self.keywords(c) {
            return;
        }
        if c == 44 {
            self.emit(COMMA, p, p + 1);
        } else if c == 59 {
            self.emit(NEWLINE, p, p + 1);
        } else if self.has(p, ACTOR_START) {
            self.actor();
        } else if c == 45 {
            self.dash();
        } else if c == 60 && self.src.starts_with("<<->>", p) {
            self.emit(BIDIRECTIONAL_SOLID_ARROW, p, p + 5);
        } else if c == 60 && self.src.starts_with("<<-->>", p) {
            self.emit(BIDIRECTIONAL_DOTTED_ARROW, p, p + 6);
        } else if (c == 47 || c == 92) && self.reverse(c) {
        } else if c == 58 {
            let e = self.line_run(p + 1);
            self.emit(TXT, p, e);
        } else if c == 43 {
            self.emit(PLUS, p, p + 1);
        } else if c == 40 && self.c(p + 1) == 41 {
            self.emit(CENTRAL, p, p + 2);
        } else {
            self.emit(INVALID, p, p + 1);
        }
    }

    /// A participant name, up to an alias, a config object, or the end of the statement.
    fn id(&mut self) -> bool {
        if self.skip_same_line() {
            return true;
        }
        let n = self.n;
        let p = self.p;
        let c = self.c(p);
        if c == 35 {
            self.p = self.eol(p);
            return true;
        }
        if c == 64 && self.c(p + 1) == 123 {
            self.begin(CONFIG_START, 2, CONFIG);
            return true;
        }
        let mut a = p;
        while a < n && self.has(a, C_ID) {
            a += 1;
        }
        if a > p {
            let w = self.skip_ws(a);
            if self.c(w) == 64 && self.c(w + 1) == 123 {
                self.emit(ACTOR, p, a);
                return true;
            }
            if w > a && self.ci(w, "as") && is_space(self.c(w + 2)) {
                self.emit(ACTOR, p, a);
                self.push(ALIAS);
                return true;
            }
        }
        let mut b = a;
        while b < n && self.has(b, ID_WIDE) {
            b += 1;
        }
        if b > p {
            let end = self.c(b);
            // Before any other delimiter the name must be followed by a comment, so it ends at the last one.
            let mut cut = b;
            if b < n && end != 10 && end != 59 {
                cut -= 1;
                while cut > p && self.c(cut) != 35 {
                    cut -= 1;
                }
            }
            if cut > p {
                let text = String::from(trim(self.src.slice(p, cut)));
                self.emit_text(ACTOR, p, cut, text);
                self.pop();
                return true;
            }
        }
        if self.c(b) == 60 {
            let e = self.eol(p);
            self.emit(INVALID, p, e);
            self.pop();
            return true;
        }
        if c != 10 {
            let e = self.eol(p);
            let text = String::from(trim(self.src.slice(p, e)));
            self.emit_text(INVALID, p, e, text);
            self.pop();
            return true;
        }
        false
    }

    fn step(&mut self) -> bool {
        match self.state {
            INITIAL => {
                self.initial();
                true
            }
            ID => self.id(),
            ALIAS => {
                if self.skip_same_line() {
                    return true;
                }
                if self.c(self.p) == 35 {
                    self.p = self.eol(self.p);
                    return true;
                }
                self.pop();
                self.pop();
                if self.word("as") {
                    self.begin(AS, 2, LINE);
                } else {
                    self.emit(NEWLINE, self.p, self.p);
                }
                true
            }
            LINE => {
                if self.skip_same_line() {
                    return true;
                }
                if self.c(self.p) == 35 {
                    self.p = self.eol(self.p);
                    return true;
                }
                let e = self.line_run(self.p);
                self.emit(T_REST_OF_LINE, self.p, e);
                self.pop();
                true
            }
            CONFIG => {
                let p = self.p;
                if self.c(p) != 125 {
                    let q = self.src.find_char(125, p).unwrap_or(self.n);
                    self.emit(CONFIG_CONTENT, p, q);
                } else {
                    let w = self.skip_ws(p + 1);
                    let alias = w > p + 1 && self.ci(w, "as") && is_space(self.c(w + 2));
                    self.emit(CONFIG_END, p, p + 1);
                    self.pop();
                    if alias {
                        self.push(ALIAS);
                    } else {
                        self.pop();
                    }
                }
                true
            }
            S_ACC_TITLE | S_ACC_DESCR => {
                let ty = if self.state == S_ACC_TITLE {
                    T_ACC_TITLE_VALUE
                } else {
                    T_ACC_DESCR_VALUE
                };
                let e = self.eol(self.p);
                self.emit(ty, self.p, e);
                self.pop();
                true
            }
            _ => {
                let p = self.p;
                if self.c(p) == 125 {
                    self.p += 1;
                    self.pop();
                } else {
                    let q = self.src.find_char(125, p).unwrap_or(self.n);
                    self.emit(T_ACC_DESCR_MULTILINE_VALUE, p, q);
                }
                true
            }
        }
    }
}

/// Ports `tokenize`.
pub fn tokenize(src: &Text) -> Tokens {
    let mut lx = Lexer {
        src,
        n: src.len(),
        out: Tokens::default(),
        stack: Vec::new(),
        state: INITIAL,
        p: 0,
        next_nl: -1,
    };
    while lx.p < lx.n {
        if !lx.step() {
            lx.out.types.push(ERROR);
            lx.out.texts.push(String::new());
            lx.out.starts.push(lx.p);
            return lx.out;
        }
    }
    let n = lx.n;
    // Mermaid's lexer tries its rules once more on the empty remainder before reporting the end.
    match lx.state {
        INITIAL | ALIAS => lx.emit(NEWLINE, n, n),
        LINE => lx.emit(T_REST_OF_LINE, n, n),
        S_ACC_TITLE => lx.emit(T_ACC_TITLE_VALUE, n, n),
        S_ACC_DESCR => lx.emit(T_ACC_DESCR_VALUE, n, n),
        S_ACC_DESCR_MULTILINE => lx.emit(T_ACC_DESCR_MULTILINE_VALUE, n, n),
        _ => {}
    }
    lx.out.types.push(END);
    lx.out.texts.push(String::new());
    lx.out.starts.push(n);
    lx.out
}
