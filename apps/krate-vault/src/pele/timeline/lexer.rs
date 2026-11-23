//! Ports diagrams/timeline/lexer.ts: the Jison lexer of Mermaid's
//! timeline grammar, by hand.

use super::super::common::scan::{accessibility, keyword_at, line_end, skip_trivia, Tokens};
use crate::pele::common::chars::{is_space, is_word, Text};

pub const END: u32 = 0;
pub const EOF: u32 = 1;
pub const INVALID: u32 = 2;
pub const NEWLINE: u32 = 3;
pub const TIMELINE: u32 = 4;
pub const TIMELINE_LR: u32 = 5;
pub const TIMELINE_TD: u32 = 6;
pub const TITLE: u32 = 7;
pub const ACC_TITLE: u32 = 8;
pub const ACC_TITLE_VALUE: u32 = 9;
pub const ACC_DESCR: u32 = 10;
pub const ACC_DESCR_VALUE: u32 = 11;
pub const ACC_DESCR_MULTILINE_VALUE: u32 = 12;
pub const SECTION: u32 = 13;
pub const PERIOD: u32 = 14;
pub const EVENT: u32 = 15;

/// Ports `TOKEN_NAMES`.
pub const TOKEN_NAMES: [&str; 16] = [
    "$end",
    "EOF",
    "INVALID",
    "NEWLINE",
    "timeline",
    "timeline_lr",
    "timeline_td",
    "title",
    "acc_title",
    "acc_title_value",
    "acc_descr",
    "acc_descr_value",
    "acc_descr_multiline_value",
    "section",
    "period",
    "event",
];

/// Ports `tokenize`. `starts` are byte offsets.
pub fn tokenize(src: &Text) -> Tokens {
    let n = src.len();
    let mut out = Tokens::default();
    let mut p = 0usize;

    let emit = |out: &mut Tokens, p: usize, ty: u32, e: usize| -> usize {
        out.types.push(ty);
        out.texts.push(src.slice(p, e).into());
        out.starts.push(src.byte(p));
        e
    };

    loop {
        p = skip_trivia(src, p);
        if p >= n {
            break;
        }
        let c = src.c(p);
        let mut e = p + 1;
        if c == 10 {
            while src.c(e) == 10 {
                e += 1;
            }
            p = emit(&mut out, p, NEWLINE, e);
            continue;
        }
        if keyword_at(src, p, "timeline") {
            e = p + 8;
            while src.c(e) == 32 || src.c(e) == 9 {
                e += 1;
            }
            let lr = keyword_at(src, e, "lr");
            if e > p + 8 && (lr || keyword_at(src, e, "td")) && !is_word(src.c(e + 2)) {
                p = emit(
                    &mut out,
                    p,
                    if lr { TIMELINE_LR } else { TIMELINE_TD },
                    e + 2,
                );
                continue;
            }
            if !is_word(src.c(p + 8)) {
                p = emit(&mut out, p, TIMELINE, p + 8);
                continue;
            }
        }
        if keyword_at(src, p, "title") {
            // Unlike Mermaid, ignore bare titles.
            e = p + 5;
            while src.c(e) != 10 && is_space(src.c(e)) {
                e += 1;
            }
            if e == n || src.c(e) == 10 {
                p = e;
                continue;
            }
            let sep = src.c(p + 5);
            if sep != 13 && is_space(sep) {
                let end = line_end(src, p + 6);
                p = emit(&mut out, p, TITLE, end);
                continue;
            }
        }
        e = accessibility(src, p, &mut out, ACC_TITLE);
        if e != p {
            p = e;
            continue;
        }
        // Like Mermaid, a bare section uses the next line as its name.
        if keyword_at(src, p, "section") && is_space(src.c(p + 7)) {
            e = p + 8;
            while e < n && src.c(e) != 58 && src.c(e) != 10 {
                e += 1;
            }
            if e > p + 8 {
                p = emit(&mut out, p, SECTION, e);
                continue;
            }
        }
        if c != 58 {
            e = p + 1;
            while e < n {
                let k = src.c(e);
                if k == 35 || k == 58 || k == 10 {
                    break;
                }
                e += 1;
            }
            p = emit(&mut out, p, PERIOD, e);
            continue;
        }
        // An event runs to the end of the line or to a colon that is followed by whitespace.
        if is_space(src.c(p + 1)) {
            e = p + 2;
            while e < n {
                let k = src.c(e);
                if k == 10 || (k == 58 && is_space(src.c(e + 1))) {
                    break;
                }
                e += 1;
            }
            if e > p + 2 {
                p = emit(&mut out, p, EVENT, e);
                continue;
            }
        }
        p = emit(&mut out, p, INVALID, p + 1);
    }

    // Jison marks its lexer done when a rule runs on empty input, so an accessibility value that
    // matches nothing at the very end of the text is followed by no EOF token.
    let bytes = src.src.len();
    if out.starts.last().is_none_or(|&s| s < bytes) {
        emit(&mut out, n, EOF, n);
    }
    emit(&mut out, n, END, n);
    out
}
