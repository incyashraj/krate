//! Ports diagrams/gantt/lexer.ts: the Jison lexer of Mermaid's gantt
//! grammar, by hand. Positions are character indices of a [`Text`].

use alloc::string::String;
use alloc::vec::Vec;

use super::tokens::*;
use crate::pele::common::caseless::word_at;
use crate::pele::common::chars::{is_digit, is_space, is_word, Text};

/// Ports `interface Tokens`.
#[derive(Debug, Clone, Default)]
pub struct Tokens {
    pub types: Vec<i32>,
    pub texts: Vec<String>,
    pub starts: Vec<usize>,
}

// `const enum S`.
const INITIAL: u8 = 0;
const S_ACC_TITLE: u8 = 1;
const S_ACC_DESCR: u8 = 2;
const S_ACC_DESCR_MULTILINE: u8 = 3;
const S_HREF: u8 = 4;
const S_CALLBACKNAME: u8 = 5;
const S_CALLBACKARGS: u8 = 6;
const S_CLICK: u8 = 7;

const DAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

/// `\s*` from `p`.
fn spaces(src: &Text, mut p: usize) -> usize {
    while is_space(src.c(p)) {
        p += 1;
    }
    p
}

/// `\b` at `p`: between a word character and another.
fn boundary(src: &Text, p: usize) -> bool {
    let before = p > 0 && is_word(src.c(p - 1));
    before != is_word(src.c(p))
}

/// `word\s*<c>\s*` (case-insensitive word) at `p`.
fn word_then(src: &Text, p: usize, word: &str, c: u32) -> Option<usize> {
    if !word_at(src, p, word) {
        return None;
    }
    let q = spaces(src, p + word.len());
    if src.c(q) != c {
        return None;
    }
    Some(spaces(src, q + 1))
}

/// `word\s+` at `p`.
fn word_spaces(src: &Text, p: usize, word: &str) -> Option<usize> {
    if !word_at(src, p, word) {
        return None;
    }
    let q = spaces(src, p + word.len());
    (q > p + word.len()).then_some(q)
}

/// `word\b` at `p`.
fn word_boundary(src: &Text, p: usize, word: &str) -> Option<usize> {
    (word_at(src, p, word) && boundary(src, p + word.len())).then_some(p + word.len())
}

/// `word<one char of class><rest+>`: `word\s[^#\n;]+` and the like. The
/// class of the separator is `sep`; the rest stops at any of `stop`.
fn word_rest(
    src: &Text,
    p: usize,
    word: &str,
    sep: fn(u32) -> bool,
    stop: &[u32],
) -> Option<usize> {
    if !word_at(src, p, word) {
        return None;
    }
    let s = p + word.len();
    if !sep(src.c(s)) {
        return None;
    }
    let mut e = s + 1;
    while e < src.len() && !stop.contains(&src.c(e)) {
        e += 1;
    }
    (e > s + 1).then_some(e)
}

/// `[^\S\r\n]`: whitespace other than a line break.
fn blank(c: u32) -> bool {
    is_space(c) && c != 13 && c != 10
}

/// Ports `tokenize`.
pub fn tokenize(src: &Text) -> Tokens {
    let n = src.len();
    let mut out = Tokens::default();
    let mut state = INITIAL;
    let mut p = 0usize;

    let emit = |out: &mut Tokens, ty: i32, s: usize, e: usize| -> usize {
        out.types.push(ty);
        out.texts.push(String::from(src.slice(s, e)));
        out.starts.push(s);
        e
    };
    let until = |ch: u32, from: usize| -> usize { src.find_char(ch, from).unwrap_or(n) };

    // Ports `keyword`: the token at `p`, its end, and the state it enters.
    let keyword = |p: usize, c: u32| -> Option<(Option<i32>, usize, u8)> {
        match c | 32 {
            97 => {
                if let Some(e) = word_then(src, p, "acctitle", 58) {
                    return Some((Some(ACC_TITLE), e, S_ACC_TITLE));
                }
                if let Some(e) = word_then(src, p, "accdescr", 58) {
                    return Some((Some(ACC_DESCR), e, S_ACC_DESCR));
                }
                if let Some(e) = word_then(src, p, "accdescr", 123) {
                    return Some((None, e, S_ACC_DESCR_MULTILINE));
                }
            }
            104 => {
                if let Some(q) = word_spaces(src, p, "href") {
                    if src.c(q) == 34 {
                        return Some((None, q + 1, S_HREF));
                    }
                }
            }
            99 => {
                if let Some(e) = word_spaces(src, p, "call") {
                    return Some((None, e, S_CALLBACKNAME));
                }
                if let Some(e) = word_spaces(src, p, "click") {
                    return Some((None, e, S_CLICK));
                }
            }
            // /title[^\S\r\n]*(?=[\r\n]|$)/iy
            116 if word_at(src, p, "title") => {
                let mut e = p + 5;
                while blank(src.c(e)) {
                    e += 1;
                }
                if e >= n || src.c(e) == 10 || src.c(e) == 13 {
                    return Some((None, e, INITIAL));
                }
            }
            119 => {
                if let Some(q) = word_spaces(src, p, "weekday") {
                    for (k, day) in DAYS.iter().enumerate() {
                        if word_at(src, q, day) && boundary(src, q + day.len()) {
                            return Some((Some(WEEKDAY_MONDAY + k as i32), q + day.len(), INITIAL));
                        }
                    }
                }
                if let Some(q) = word_spaces(src, p, "weekend") {
                    if word_at(src, q, "friday") && boundary(src, q + 6) {
                        return Some((Some(WEEKEND_FRIDAY), q + 6, INITIAL));
                    }
                    if word_at(src, q, "saturday") && boundary(src, q + 8) {
                        return Some((Some(WEEKEND_SATURDAY), q + 8, INITIAL));
                    }
                }
            }
            _ => {}
        }
        let rest = |word: &str, stop: &[u32]| word_rest(src, p, word, is_space, stop);
        let found: Option<(i32, usize)> = match c | 32 {
            97 => rest("axisformat", &[35, 10, 59])
                .map(|e| (AXIS_FORMAT, e))
                .or_else(|| rest("accdescription", &[35, 10, 59]).map(|e| (ACC_DESCRIPTION, e))),
            103 => word_boundary(src, p, "gantt").map(|e| (GANTT, e)),
            100 => rest("dateformat", &[35, 10, 59]).map(|e| (DATE_FORMAT, e)),
            105 => word_boundary(src, p, "inclusiveenddates")
                .map(|e| (INCLUSIVE_END_DATES, e))
                .or_else(|| rest("includes", &[35, 10, 59]).map(|e| (INCLUDES, e))),
            116 => word_boundary(src, p, "topaxis")
                .map(|e| (TOP_AXIS, e))
                .or_else(|| rest("tickinterval", &[35, 10, 59]).map(|e| (TICK_INTERVAL, e)))
                .or_else(|| rest("todaymarker", &[10, 59]).map(|e| (TODAY_MARKER, e)))
                .or_else(|| word_rest(src, p, "title", blank, &[10]).map(|e| (TITLE, e))),
            101 => rest("excludes", &[35, 10, 59]).map(|e| (EXCLUDES, e)),
            115 => rest("section", &[10]).map(|e| (SECTION, e)),
            _ => None,
        };
        found.map(|(ty, e)| (Some(ty), e, INITIAL))
    };

    loop {
        if p >= n {
            let ty = match state {
                S_ACC_TITLE => ACC_TITLE_VALUE,
                S_ACC_DESCR => ACC_DESCR_VALUE,
                S_ACC_DESCR_MULTILINE => ACC_DESCR_MULTILINE_VALUE,
                S_HREF => HREF,
                S_CALLBACKNAME => CALLBACKNAME,
                S_CALLBACKARGS => CALLBACKARGS,
                S_CLICK => CLICK,
                _ => EOF,
            };
            emit(&mut out, ty, n, n);
            break;
        }

        let c = src.c(p);

        match state {
            INITIAL => {
                let d = src.c(p + 1);
                if c == 37 {
                    if d != 37 {
                        let mut e = until(10, p + 1);
                        while src.c(e) == 10 {
                            e += 1;
                        }
                        p = e;
                    } else if src.c(p + 2) == 123 {
                        // The grammar enters a lexer state it never defines, so nothing can follow.
                        emit(&mut out, OPEN_DIRECTIVE, p, p + 3);
                        emit(&mut out, ERROR, p, p);
                        return out;
                    } else {
                        p = until(10, p + 2);
                    }
                } else if c == 10 {
                    let mut e = p + 1;
                    while src.c(e) == 10 {
                        e += 1;
                    }
                    p = emit(&mut out, NL, p, e);
                } else if is_space(c) {
                    p += 1;
                    while p < n && is_space(src.c(p)) {
                        p += 1;
                    }
                } else if let Some((ty, e, next)) = keyword(p, c) {
                    p = match ty {
                        Some(ty) => emit(&mut out, ty, p, e),
                        None => e,
                    };
                    state = next;
                } else if is_digit(c) && date_at(src, p) {
                    p = emit(&mut out, DATE, p, p + 10);
                } else if c != 58 {
                    let mut e = p + 1;
                    while e < n {
                        let ch = src.c(e);
                        if ch == 58 || ch == 10 {
                            break;
                        }
                        e += 1;
                    }
                    p = emit(&mut out, TASK_TXT, p, e);
                } else {
                    let mut e = p + 1;
                    while e < n {
                        let ch = src.c(e);
                        if ch == 35 || ch == 10 || ch == 59 {
                            break;
                        }
                        e += 1;
                    }
                    p = emit(&mut out, if e > p + 1 { TASK_DATA } else { COLON }, p, e);
                }
            }
            S_ACC_TITLE | S_ACC_DESCR => {
                let ty = if state == S_ACC_TITLE {
                    ACC_TITLE_VALUE
                } else {
                    ACC_DESCR_VALUE
                };
                state = INITIAL;
                let e = until(10, p);
                p = emit(&mut out, ty, p, e);
            }
            S_ACC_DESCR_MULTILINE => {
                if c == 125 {
                    state = INITIAL;
                    p += 1;
                } else {
                    let e = until(125, p);
                    p = emit(&mut out, ACC_DESCR_MULTILINE_VALUE, p, e);
                }
            }
            S_HREF => {
                if c == 34 {
                    state = INITIAL;
                    p += 1;
                } else {
                    let e = until(34, p);
                    p = emit(&mut out, HREF, p, e);
                }
            }
            S_CALLBACKNAME => {
                if c == 40 {
                    // /\(\s*\)/y
                    let q = spaces(src, p + 1);
                    if src.c(q) == 41 {
                        state = INITIAL;
                        p = q + 1;
                    } else {
                        state = S_CALLBACKARGS;
                        p += 1;
                    }
                } else {
                    let e = until(40, p);
                    p = emit(&mut out, CALLBACKNAME, p, e);
                }
            }
            S_CALLBACKARGS => {
                if c == 41 {
                    state = INITIAL;
                    p += 1;
                } else {
                    let e = until(41, p);
                    p = emit(&mut out, CALLBACKARGS, p, e);
                }
            }
            _ => {
                if is_space(c) {
                    state = INITIAL;
                    p += 1;
                } else {
                    let mut e = p + 1;
                    while e < n && !is_space(src.c(e)) {
                        e += 1;
                    }
                    p = emit(&mut out, CLICK, p, e);
                }
            }
        }
    }

    out.types.push(END);
    out.texts.push(String::new());
    out.starts.push(n);
    out
}

/// `/\d\d\d\d-\d\d-\d\d\b/y` at `p`.
fn date_at(src: &Text, p: usize) -> bool {
    let d = |i: usize| is_digit(src.c(p + i));
    d(0) && d(1)
        && d(2)
        && d(3)
        && src.c(p + 4) == 45
        && d(5)
        && d(6)
        && src.c(p + 7) == 45
        && d(8)
        && d(9)
        && boundary(src, p + 10)
}
