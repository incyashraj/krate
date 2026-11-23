//! Ports diagrams/common/scan.ts: lexer rules that Mermaid's journey and
//! timeline grammars have in common, ported from the compiled Jison rules
//! (comments, whitespace, and the accTitle and accDescr statements).
//!
//! Positions are character indices in a [`Text`]; `starts` in [`Tokens`]
//! are byte offsets, as errors locate them.

use alloc::string::String;
use alloc::vec::Vec;

use super::chars::{is_space, Text};

/// Ports `interface Tokens`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tokens {
    pub types: Vec<u32>,
    pub texts: Vec<String>,
    pub starts: Vec<usize>,
}

/// Ports `lineEnd`.
pub fn line_end(src: &Text, from: usize) -> usize {
    src.line_end(from)
}

/// Ports `keywordAt`: `word` is lowercase ASCII letters; the grammars are
/// case-insensitive.
pub fn keyword_at(src: &Text, p: usize, word: &str) -> bool {
    for (i, w) in word.bytes().enumerate() {
        let c = src.c(p + i);
        if c == super::chars::NONE || (c | 32) != w as u32 {
            return false;
        }
    }
    true
}

/// Ports `skipTrivia`: skips comments and whitespace, stopping at a line
/// break that starts a token, which is a NEWLINE. Jison compiles `\%%` to
/// `%`, so a single `%` starts a comment.
pub fn skip_trivia(src: &Text, from: usize) -> usize {
    let n = src.len();
    let mut p = from;
    while p < n {
        let c = src.c(p);
        if c == 37 && src.c(p + 1) != 123 {
            p = line_end(src, p);
        } else if c == 10 {
            return p;
        } else if is_space(c) {
            loop {
                p += 1;
                if !(p < n && is_space(src.c(p))) {
                    break;
                }
            }
        } else if c == 35 {
            p = line_end(src, p);
        } else {
            return p;
        }
    }
    p
}

/// Ports `skipSpace`.
fn skip_space(src: &Text, from: usize) -> usize {
    let mut p = from;
    while is_space(src.c(p)) {
        p += 1;
    }
    p
}

/// Ports `accessibility`: lexes an accTitle or accDescr statement at `p`
/// and returns where it ends, or `p` if there is none. The token types
/// acc_title, acc_title_value, acc_descr, acc_descr_value and
/// acc_descr_multiline_value are numbered consecutively from `first`.
pub fn accessibility(src: &Text, p: usize, out: &mut Tokens, first: u32) -> usize {
    let descr = keyword_at(src, p, "accdescr");
    if !descr && !keyword_at(src, p, "acctitle") {
        return p;
    }
    let n = src.len();
    let mut q = skip_space(src, p + 8);
    let c = src.c(q);
    let mut emit = |kind: u32, s: usize, e: usize| {
        out.types.push(kind);
        out.texts.push(src.slice(s, e).into());
        out.starts.push(src.byte(s));
    };
    if c == 58 {
        q = skip_space(src, q + 1);
        let end = line_end(src, q);
        emit(if descr { first + 2 } else { first }, p, q);
        emit(if descr { first + 3 } else { first + 1 }, q, end);
        return end;
    }
    if !descr || c != 123 {
        return p;
    }
    q = skip_space(src, q + 1);
    while src.c(q) != 125 {
        let end = src.find_char(125, q).unwrap_or(n);
        emit(first + 4, q, end);
        if q == n {
            return n;
        }
        q = end;
    }
    q + 1
}
