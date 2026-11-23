//! Ports diagrams/journey/lexer.ts: the Jison lexer of Mermaid's journey
//! grammar, by hand.

use crate::pele::common::chars::{is_space, is_word, Text};
use crate::pele::common::scan::{accessibility, keyword_at, skip_trivia, Tokens};

pub const END: u32 = 0;
pub const EOF: u32 = 1;
pub const INVALID: u32 = 2;
pub const NEWLINE: u32 = 3;
pub const JOURNEY: u32 = 4;
pub const TITLE: u32 = 5;
pub const ACC_TITLE: u32 = 6;
pub const ACC_TITLE_VALUE: u32 = 7;
pub const ACC_DESCR: u32 = 8;
pub const ACC_DESCR_VALUE: u32 = 9;
pub const ACC_DESCR_MULTILINE_VALUE: u32 = 10;
pub const SECTION: u32 = 11;
pub const TASK_NAME: u32 = 12;
pub const TASK_DATA: u32 = 13;
pub const COLON: u32 = 14;

/// Ports `TOKEN_NAMES`.
pub const TOKEN_NAMES: [&str; 15] = [
    "$end",
    "EOF",
    "INVALID",
    "NEWLINE",
    "journey",
    "title",
    "acc_title",
    "acc_title_value",
    "acc_descr",
    "acc_descr_value",
    "acc_descr_multiline_value",
    "section",
    "taskName",
    "taskData",
    ":",
];

/// Ports `textEnd`: `#`, a line break, and `;` end every kind of text; a
/// colon also ends a task name or a section.
fn text_end(src: &Text, from: usize, colon: bool) -> usize {
    let n = src.len();
    let mut e = from;
    while e < n {
        let c = src.c(e);
        if c == 35 || c == 10 || c == 59 || (colon && c == 58) {
            break;
        }
        e += 1;
    }
    e
}

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
        if keyword_at(src, p, "journey") && !is_word(src.c(p + 7)) {
            p = emit(&mut out, p, JOURNEY, p + 7);
            continue;
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
                e = text_end(src, p + 6, false);
                if e > p + 6 {
                    p = emit(&mut out, p, TITLE, e);
                    continue;
                }
            }
        }
        e = accessibility(src, p, &mut out, ACC_TITLE);
        if e != p {
            p = e;
            continue;
        }
        // Like Mermaid, a bare section uses the next line as its name.
        if keyword_at(src, p, "section") && is_space(src.c(p + 7)) {
            e = text_end(src, p + 8, true);
            if e > p + 8 {
                p = emit(&mut out, p, SECTION, e);
                continue;
            }
        }
        if c == 58 {
            e = text_end(src, p + 1, false);
            p = if e > p + 1 {
                emit(&mut out, p, TASK_DATA, e)
            } else {
                emit(&mut out, p, COLON, p + 1)
            };
        } else if c == 59 {
            p = emit(&mut out, p, INVALID, p + 1);
        } else {
            let e = text_end(src, p + 1, true);
            p = emit(&mut out, p, TASK_NAME, e);
        }
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
