//! Ports diagrams/xychart/lexer.ts: the Jison lexer of Mermaid's XY chart
//! grammar, by hand. Positions are character indices of a [`Text`].

use alloc::string::String;
use alloc::vec::Vec;

use super::tokens::*;
use crate::pele::common::caseless::word_at;
use crate::pele::common::chars::{is_digit, is_letter, is_space, is_word, Text};

/// Ports `interface Tokens`.
#[derive(Debug, Clone, Default)]
pub struct Tokens {
    pub types: Vec<i32>,
    pub texts: Vec<String>,
    pub starts: Vec<usize>,
}

// `const enum S`.
const INITIAL: usize = 0;
const AXIS_DATA: usize = 1;
const AXIS_BAND_DATA: usize = 2;
const DATA: usize = 3;
const DATA_INNER: usize = 4;
const STRING: usize = 5;
const S_ACC_TITLE: usize = 6;
const S_ACC_DESCR: usize = 7;
const S_ACC_DESCR_MULTILINE: usize = 8;

/// The token each state gives for the empty rest of the input.
const AT_END: [i32; 9] = [
    EOF,
    EOF,
    EOF,
    EOF,
    EOF,
    STR,
    ACC_TITLE_VALUE,
    ACC_DESCR_VALUE,
    ACC_DESCR_MULTILINE_VALUE,
];

/// Ports `BROKEN_RULE`: Mermaid's grammar means to open a markdown string
/// at `"` followed by a backtick, but its quoting turns the rule into one
/// that matches this text and leaves the current state.
pub fn broken_rule() -> String {
    let mut s = String::from("`)");
    s.push_str(&" ".repeat(36));
    s.push_str("{ this.pushstate(md_string); }\n<md_string>(?:(?!`\").)+");
    s.push_str(&" ".repeat(18));
    s.push_str("{ return md_str; }\n<md_string>(?:`");
    s
}

/// `SINGLE[c]`.
fn single(c: u32) -> i32 {
    match char::from_u32(c).unwrap_or('\0') {
        ':' => COLON,
        '+' => PLUS,
        ',' => COMMA,
        '=' => EQUALS,
        '*' => MULT,
        '#' => BRKT,
        '_' => UNDERSCORE,
        '.' => DOT,
        '&' => AMP,
        '-' => MINUS,
        ';' => SEMI,
        _ => 0,
    }
}

/// Ports `tokenize`.
pub fn tokenize(src: &Text) -> Tokens {
    let n = src.len();
    let mut out = Tokens::default();
    let mut stack: Vec<usize> = Vec::new();
    let mut state = INITIAL;
    let mut p = 0usize;
    let broken = broken_rule();
    let broken_len = broken.chars().count();

    let emit = |out: &mut Tokens, ty: i32, s: usize, e: usize| -> usize {
        out.types.push(ty);
        out.texts.push(String::from(src.slice(s, e)));
        out.starts.push(s);
        e
    };
    let run = |mut q: usize| -> usize {
        while q < n && is_space(src.c(q)) {
            q += 1;
        }
        q
    };
    let digits = |mut q: usize| -> usize {
        while is_digit(src.c(q)) {
            q += 1;
        }
        q
    };
    let keyword =
        |p: usize, word: &str| -> bool { word_at(src, p, word) && !is_word(src.c(p + word.len())) };
    macro_rules! push {
        ($s:expr) => {{
            stack.push(state);
            state = $s;
        }};
    }
    macro_rules! pop {
        () => {{
            if let Some(s) = stack.pop() {
                state = s;
            }
        }};
    }

    while p < n {
        let c = src.c(p);

        if state == STRING {
            if c == 34 {
                p += 1;
                pop!();
            } else {
                let e = src.find_char(34, p).unwrap_or(n);
                p = emit(&mut out, STR, p, e);
            }
            continue;
        }
        if state == S_ACC_TITLE || state == S_ACC_DESCR {
            p = emit(&mut out, AT_END[state], p, src.line_end(p));
            pop!();
            continue;
        }
        if state == S_ACC_DESCR_MULTILINE {
            if c == 125 {
                p += 1;
                pop!();
            } else {
                let e = src.find_char(125, p).unwrap_or(n);
                p = emit(&mut out, ACC_DESCR_MULTILINE_VALUE, p, e);
            }
            continue;
        }

        let next = src.c(p + 1);
        if c == 37 && next == 37 {
            p = src.line_end(p + 2);
            continue;
        }
        if c == 10 || c == 13 {
            // Only a line break that directly follows an axis or a series ends it; one after spaces is skipped
            // with them, and the state carries on to the next line.
            if (state == AXIS_DATA || state == DATA) && (c == 10 || next == 10) {
                p = emit(&mut out, NEWLINE, p, if c == 10 { p + 1 } else { p + 2 });
                pop!();
                continue;
            }
            let mut e = p + 1;
            while src.c(e) == 10 || src.c(e) == 13 {
                e += 1;
            }
            p = emit(&mut out, NEWLINE, p, e);
            continue;
        }

        match c | 32 {
            116 if keyword(p, "title") => {
                p = emit(&mut out, TITLE, p, p + 5);
                continue;
            }
            97 if word_at(src, p, "acc") => {
                let descr = word_at(src, p + 3, "descr");
                if descr || word_at(src, p + 3, "title") {
                    let k = run(p + 8);
                    let d = src.c(k);
                    if d == 58 {
                        let e = run(k + 1);
                        p = emit(&mut out, if descr { ACC_DESCR } else { ACC_TITLE }, p, e);
                        push!(if descr { S_ACC_DESCR } else { S_ACC_TITLE });
                        continue;
                    }
                    if d == 123 && descr {
                        p = run(k + 1);
                        push!(S_ACC_DESCR_MULTILINE);
                        continue;
                    }
                }
            }
            120 => {
                if keyword(p, "xychart-beta") {
                    p = emit(&mut out, XYCHART, p, p + 12);
                    continue;
                }
                if keyword(p, "xychart") {
                    p = emit(&mut out, XYCHART, p, p + 7);
                    continue;
                }
                if keyword(p, "x-axis") {
                    p = emit(&mut out, X_AXIS, p, p + 6);
                    push!(AXIS_DATA);
                    continue;
                }
            }
            121 if keyword(p, "y-axis") => {
                p = emit(&mut out, Y_AXIS, p, p + 6);
                push!(AXIS_DATA);
                continue;
            }
            118 if word_at(src, p, "vertical") => {
                p = emit(&mut out, CHART_ORIENTATION, p, p + 8);
                continue;
            }
            104 if word_at(src, p, "horizontal") => {
                p = emit(&mut out, CHART_ORIENTATION, p, p + 10);
                continue;
            }
            108 if keyword(p, "line") => {
                p = emit(&mut out, LINE, p, p + 4);
                push!(DATA);
                continue;
            }
            98 if keyword(p, "bar") => {
                p = emit(&mut out, BAR, p, p + 3);
                push!(DATA);
                continue;
            }
            _ => {}
        }

        if c == 91 {
            if state == AXIS_DATA {
                push!(AXIS_BAND_DATA);
            } else if state == DATA {
                push!(DATA_INNER);
            }
            p = emit(&mut out, SQUARE_BRACES_START, p, p + 1);
            continue;
        }
        if c == 93 {
            if state == DATA_INNER || state == AXIS_BAND_DATA {
                pop!();
            }
            p = emit(&mut out, SQUARE_BRACES_END, p, p + 1);
            continue;
        }
        if state == AXIS_DATA && c == 45 && next == 45 && src.c(p + 2) == 62 {
            p = emit(&mut out, ARROW_DELIMITER, p, p + 3);
            continue;
        }
        if state == AXIS_DATA || state == DATA_INNER {
            let q = if c == 43 || c == 45 { p + 1 } else { p };
            let d = src.c(q);
            let mut e = None;
            if is_digit(d) {
                let mut f = digits(q + 1);
                if src.c(f) == 46 && is_digit(src.c(f + 1)) {
                    f = digits(f + 2);
                }
                e = Some(f);
            } else if d == 46 && is_digit(src.c(q + 1)) {
                e = Some(digits(q + 2));
            }
            if let Some(e) = e {
                p = emit(&mut out, NUMBER_WITH_DECIMAL, p, e);
                continue;
            }
        }
        if c == 96 && word_at(src, p, &broken) {
            p += broken_len;
            pop!();
            continue;
        }
        if c == 34 {
            p += 1;
            push!(STRING);
            continue;
        }
        if is_letter(c) {
            let mut e = p + 1;
            while is_letter(src.c(e)) {
                e += 1;
            }
            p = emit(&mut out, ALPHA, p, e);
            continue;
        }
        if is_digit(c) {
            let e = digits(p + 1);
            p = emit(&mut out, NUM, p, e);
            continue;
        }
        if is_space(c) {
            p = run(p + 1);
            continue;
        }
        let s = if c < 128 { single(c) } else { 0 };
        if s != 0 {
            p = emit(&mut out, s, p, p + 1);
            continue;
        }
        break;
    }

    if p < n {
        out.types.push(ERROR);
        out.texts.push(String::new());
        out.starts.push(p);
        return out;
    }
    // Jison tries the rules of the current state once more against the empty rest of the input.
    out.types.push(AT_END[state]);
    out.texts.push(String::new());
    out.starts.push(n);
    out.types.push(END);
    out.texts.push(String::new());
    out.starts.push(n);
    out
}
