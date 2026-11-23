//! Ports diagrams/quadrant/lexer.ts: the Jison lexer of Mermaid's
//! quadrant grammar, by hand. Positions are character indices of a
//! [`Text`].

use alloc::string::String;
use alloc::vec::Vec;

use super::tokens::*;
use crate::pele::common::caseless::word_at;
use crate::pele::common::chars::{is_digit, is_letter, is_line_end, is_space, is_word, Text};

/// Ports `interface Tokens`.
#[derive(Debug, Clone, Default)]
pub struct Tokens {
    pub types: Vec<i32>,
    pub texts: Vec<String>,
    pub starts: Vec<usize>,
}

// `const enum S`.
const INITIAL: usize = 0;
const S_TITLE: usize = 1;
const S_ACC_TITLE: usize = 2;
const S_ACC_DESCR: usize = 3;
const S_ACC_DESCR_MULTILINE: usize = 4;
const S_STRING: usize = 5;
const S_MD_STRING: usize = 6;
const S_CLASS_NAME: usize = 7;
const S_POINT_START: usize = 8;
const S_POINT_X: usize = 9;
const S_POINT_Y: usize = 10;

/// The token each state gives for the empty rest of the input, where it gives one.
const AT_END: [i32; 6] = [
    EOF,
    TITLE_VALUE,
    ACC_TITLE_VALUE,
    ACC_DESCR_VALUE,
    ACC_DESCR_MULTILINE_VALUE,
    STR,
];

/// `SINGLE[c]`: the token of a one-character rule, or 0.
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
        ' ' | '\t' | '\u{b}' | '\u{c}' => SPACE,
        ';' => SEMI,
        '!' | '$' | '%' | '\'' | '`' | '?' | '\\' | '/' => PUNCTUATION,
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

    let emit = |out: &mut Tokens, ty: i32, s: usize, e: usize| -> usize {
        out.types.push(ty);
        out.texts.push(String::from(src.slice(s, e)));
        out.starts.push(s);
        e
    };
    let line_end = |q: usize| -> usize { src.line_end(q) };
    let spaces = |mut q: usize| -> usize {
        while src.c(q) == 32 {
            q += 1;
        }
        q
    };
    let run = |mut q: usize| -> usize {
        while q < n && is_space(src.c(q)) {
            q += 1;
        }
        q
    };
    // Mermaid's `(1)|(0(.\d+)?)`, where the dot is any character within a line.
    let coordinate = |q: usize| -> Option<usize> {
        let c = src.c(q);
        if c == 49 {
            return Some(q + 1);
        }
        if c != 48 {
            return None;
        }
        let mut e = q + 1;
        if e < n && !is_line_end(src.c(e)) && is_digit(src.c(e + 1)) {
            e += 2;
            while is_digit(src.c(e)) {
                e += 1;
            }
        }
        Some(e)
    };

    'lexing: while p < n {
        let c = src.c(p);
        let next = src.c(p + 1);

        match state {
            S_TITLE | S_ACC_TITLE | S_ACC_DESCR => {
                p = emit(&mut out, AT_END[state], p, line_end(p));
                if let Some(s) = stack.pop() {
                    state = s;
                }
                continue;
            }
            S_ACC_DESCR_MULTILINE => {
                if c == 125 {
                    p += 1;
                    if let Some(s) = stack.pop() {
                        state = s;
                    }
                } else {
                    let e = src.find_char(125, p).unwrap_or(n);
                    p = emit(&mut out, ACC_DESCR_MULTILINE_VALUE, p, e);
                }
                continue;
            }
            S_STRING => {
                if c == 34 {
                    p += 1;
                    if let Some(s) = stack.pop() {
                        state = s;
                    }
                } else {
                    let e = src.find_char(34, p).unwrap_or(n);
                    p = emit(&mut out, STR, p, e);
                }
                continue;
            }
            S_MD_STRING => {
                if c == 96 && next == 34 {
                    p += 2;
                    if let Some(s) = stack.pop() {
                        state = s;
                    }
                    continue;
                }
                if c != 96 && c != 34 {
                    let mut e = p + 1;
                    while e < n && src.c(e) != 96 && src.c(e) != 34 {
                        e += 1;
                    }
                    p = emit(&mut out, MD_STR, p, e);
                    continue;
                }
                break 'lexing;
            }
            S_CLASS_NAME => {
                if is_word(c) {
                    let mut e = p + 1;
                    while is_word(src.c(e)) {
                        e += 1;
                    }
                    p = emit(&mut out, CLASS_NAME, p, e);
                    if let Some(s) = stack.pop() {
                        state = s;
                    }
                    continue;
                }
                break 'lexing;
            }
            S_POINT_START => {
                if let Some(e) = coordinate(p) {
                    p = emit(&mut out, POINT_X, p, e);
                    stack.push(state);
                    state = S_POINT_X;
                    continue;
                }
                let k = run(p);
                if src.c(k) == 93 {
                    p = spaces(k + 1);
                    if let Some(s) = stack.pop() {
                        state = s;
                    }
                    continue;
                }
                break 'lexing;
            }
            S_POINT_X => {
                let k = run(p);
                if src.c(k) == 44 {
                    p = run(k + 1);
                    state = S_POINT_Y;
                    continue;
                }
                break 'lexing;
            }
            S_POINT_Y => {
                if let Some(e) = coordinate(p) {
                    p = emit(&mut out, POINT_Y, p, e);
                    if let Some(s) = stack.pop() {
                        state = s;
                    }
                    continue;
                }
                break 'lexing;
            }
            _ => {}
        }

        if c == 37 && next == 37 {
            p = line_end(p + 2);
            continue;
        }
        if c == 10 || c == 13 {
            let mut e = p + 1;
            while src.c(e) == 10 || src.c(e) == 13 {
                e += 1;
            }
            p = emit(&mut out, NEWLINE, p, e);
            continue;
        }

        let lower = c | 32;
        if lower == 116 && word_at(src, p, "title") && !is_word(src.c(p + 5)) {
            p = emit(&mut out, TITLE, p, p + 5);
            stack.push(state);
            state = S_TITLE;
            continue;
        }
        if lower == 97 && word_at(src, p, "acc") {
            let descr = word_at(src, p + 3, "descr");
            if descr || word_at(src, p + 3, "title") {
                let k = run(p + 8);
                let d = src.c(k);
                if d == 58 {
                    let e = run(k + 1);
                    p = emit(&mut out, if descr { ACC_DESCR } else { ACC_TITLE }, p, e);
                    stack.push(state);
                    state = if descr { S_ACC_DESCR } else { S_ACC_TITLE };
                    continue;
                }
                if d == 123 && descr {
                    p = run(k + 1);
                    stack.push(state);
                    state = S_ACC_DESCR_MULTILINE;
                    continue;
                }
            }
        }

        // Axis, quadrant and chart keywords and the axis arrow take the spaces around them.
        let j = if c == 32 { spaces(p) } else { p };
        let d = src.c(j);
        let dl = d | 32;
        let mut ty = 0;
        let mut end = 0;
        if dl == 120 && word_at(src, j, "x-axis") {
            ty = X_AXIS;
            end = j + 6;
        } else if dl == 121 && word_at(src, j, "y-axis") {
            ty = Y_AXIS;
            end = j + 6;
        } else if d == 45 {
            let mut dash_to = j;
            while src.c(dash_to) == 45 {
                dash_to += 1;
            }
            if dash_to - j >= 2 && src.c(dash_to) == 62 {
                ty = DELIMITER;
                end = dash_to + 1;
            }
        } else if dl == 113 && word_at(src, j, "quadrant") {
            let q = src.c(j + 9);
            if src.c(j + 8) == 45 && (49..=52).contains(&q) {
                ty = QUADRANT_1 + q as i32 - 49;
                end = j + 10;
            } else if word_at(src, j + 8, "chart") {
                ty = QUADRANT;
                end = j + 13;
            }
        }
        if ty != 0 {
            let e = spaces(end);
            p = emit(&mut out, ty, p, e);
            continue;
        }

        if lower == 99 && word_at(src, p, "classdef") && !is_word(src.c(p + 8)) {
            p = emit(&mut out, CLASSDEF, p, p + 8);
            continue;
        }
        if c == 34 {
            if next == 96 {
                p += 2;
                stack.push(state);
                state = S_MD_STRING;
            } else {
                p += 1;
                stack.push(state);
                state = S_STRING;
            }
            continue;
        }
        if c == 58 && next == 58 && src.c(p + 2) == 58 {
            p += 3;
            stack.push(state);
            state = S_CLASS_NAME;
            continue;
        }
        // A point's `: [`, with any whitespace before, between and after, line breaks included.
        if c == 58 || is_space(c) {
            let k = if c != 58 { run(p) } else { p };
            if src.c(k) == 58 {
                let gap_to = run(k + 1);
                if src.c(gap_to) == 91 {
                    let e = run(gap_to + 1);
                    p = emit(&mut out, POINT_START, p, e);
                    stack.push(state);
                    state = S_POINT_START;
                    continue;
                }
            }
        }

        if is_letter(c) {
            let mut e = p + 1;
            while is_letter(src.c(e)) {
                e += 1;
            }
            p = emit(&mut out, ALPHA, p, e);
            continue;
        }
        if c >= 128 {
            let mut e = p + 1;
            while e < n && src.c(e) >= 128 {
                e += 1;
            }
            p = emit(&mut out, UNICODE_TEXT, p, e);
            continue;
        }
        if is_digit(c) {
            let mut e = p + 1;
            while is_digit(src.c(e)) {
                e += 1;
            }
            p = emit(&mut out, NUM, p, e);
            continue;
        }
        let s = single(c);
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
    if state < AT_END.len() {
        out.types.push(AT_END[state]);
        out.texts.push(String::new());
        out.starts.push(n);
    }
    out.types.push(END);
    out.texts.push(String::new());
    out.starts.push(n);
    out
}
