//! Ports diagrams/sankey/lexer.ts: the Jison lexer of Mermaid's sankey
//! grammar (CSV rows), on UTF-16 code units.

use alloc::string::String;
use alloc::vec::Vec;

pub const END: i32 = 0;
/// Not a Jison token: the reference lexer would return the same empty token forever from here.
pub const STUCK: i32 = 1;
pub const SANKEY: i32 = 2;
pub const NEWLINE: i32 = 3;
pub const COMMA: i32 = 4;
pub const DQUOTE: i32 = 5;
pub const NON_ESCAPED_TEXT: i32 = 6;
pub const ESCAPED_TEXT: i32 = 7;
pub const EOF: i32 = 8;

/// Ports `TOKEN_NAMES`.
pub const TOKEN_NAMES: [&str; 9] = [
    "$end",
    "STUCK",
    "SANKEY",
    "NEWLINE",
    "COMMA",
    "DQUOTE",
    "NON_ESCAPED_TEXT",
    "ESCAPED_TEXT",
    "EOF",
];

/// Ports `interface Tokens`; starts are in UTF-16 units.
#[derive(Debug, Clone, Default)]
pub struct Tokens {
    pub types: Vec<i32>,
    pub texts: Vec<String>,
    pub starts: Vec<usize>,
}

// `const enum S`.
const INITIAL: u8 = 0;
const CSV: u8 = 1;
const ESCAPED: u8 = 2;

/// Ports `isTextData`: RFC 4180 TEXTDATA extended to Unicode, excluding
/// controls and invalid XML characters.
fn is_text_data(c: u32) -> bool {
    ((32..=126).contains(&c) && c != 34 && c != 44)
        || (0xa0..=0xd7ff).contains(&c)
        || (0xe000..=0xfffd).contains(&c)
        || (0x10000..=0x10ffff).contains(&c)
}

/// `charCodeAt`: u32::MAX past the end (NaN).
pub fn cu(s: &[u16], i: usize) -> u32 {
    s.get(i).map(|&c| c as u32).unwrap_or(u32::MAX)
}

/// `codePointAt`.
fn code_point_at(s: &[u16], i: usize) -> u32 {
    let c = cu(s, i);
    if (0xd800..=0xdbff).contains(&c) {
        let d = cu(s, i + 1);
        if (0xdc00..=0xdfff).contains(&d) {
            return 0x10000 + ((c - 0xd800) << 10) + (d - 0xdc00);
        }
    }
    c
}

fn is_word(c: u32) -> bool {
    (48..=57).contains(&c) || (65..=90).contains(&c) || (97..=122).contains(&c) || c == 95
}

/// Ports `tokenize`.
pub fn tokenize(src: &[u16]) -> Tokens {
    let n = src.len();
    let mut out = Tokens::default();
    let mut stack: Vec<u8> = Vec::new();
    let mut state = INITIAL;
    let mut p = 0usize;

    let emit = |out: &mut Tokens, ty: i32, s: usize, e: usize| -> usize {
        out.types.push(ty);
        out.texts
            .push(String::from_utf16_lossy(&src[s.min(n)..e.min(n)]));
        out.starts.push(s);
        e
    };
    let keyword = |p: usize, word: &str| -> bool {
        let end = (p + word.len()).min(n);
        String::from_utf16_lossy(&src[p..end]).to_lowercase() == word
            && !is_word(cu(src, p + word.len()))
    };

    while p < n {
        let c = cu(src, p);
        let start = p;
        let mut q = p;
        if state == ESCAPED {
            if c == 34 && cu(src, p + 1) != 34 {
                if let Some(s) = stack.pop() {
                    state = s;
                }
                p = emit(&mut out, DQUOTE, p, p + 1);
                continue;
            }
            while q < n {
                let d = code_point_at(src, q);
                if is_text_data(d) || d == 44 || d == 13 || d == 10 {
                    q += if d > 0xffff { 2 } else { 1 };
                } else if d == 34 && cu(src, q + 1) == 34 {
                    q += 2;
                } else {
                    break;
                }
            }
            p = emit(&mut out, ESCAPED_TEXT, p, q);
        } else {
            if state == INITIAL && (c | 32) == 115 {
                let length = if keyword(p, "sankey-beta") {
                    11
                } else if keyword(p, "sankey") {
                    6
                } else {
                    0
                };
                if length > 0 {
                    stack.push(state);
                    state = CSV;
                    p = emit(&mut out, SANKEY, p, p + length);
                    continue;
                }
            }
            if c == 10 || (c == 13 && cu(src, p + 1) == 10) {
                p = emit(&mut out, NEWLINE, p, if c == 10 { p + 1 } else { p + 2 });
                continue;
            }
            if c == 44 {
                p = emit(&mut out, COMMA, p, p + 1);
                continue;
            }
            if c == 34 {
                stack.push(state);
                state = ESCAPED;
                p = emit(&mut out, DQUOTE, p, p + 1);
                continue;
            }
            while q < n {
                let d = code_point_at(src, q);
                if !is_text_data(d) {
                    break;
                }
                q += if d > 0xffff { 2 } else { 1 };
            }
            p = emit(&mut out, NON_ESCAPED_TEXT, p, q);
        }
        // A character outside the CSV alphabet matches as empty text and is never consumed.
        if q == start {
            emit(&mut out, STUCK, p, p);
            return out;
        }
    }

    emit(
        &mut out,
        if state == ESCAPED { ESCAPED_TEXT } else { EOF },
        n,
        n,
    );
    emit(&mut out, END, n, n);
    out
}
