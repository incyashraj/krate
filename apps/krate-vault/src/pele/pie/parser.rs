//! Ports diagrams/pie/parser.ts: the Langium pie grammar, by hand.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::pele::common::chars::Text;
use crate::pele::common::tokens::{
    acc_descr_type, acc_descr_value, acc_title_type, acc_title_value, directive_type, keyword,
    newline_type, single_line_comment_type, string_type, string_value, title_type, title_value,
    tokenize, whitespace_type, yaml_type, Reader, TokenType,
};
use crate::pele::errors::PeleError;
use crate::pele::js;

// `const enum T`.
const SHOW_DATA: i32 = 0;
const PIE: i32 = 1;
const COLON: i32 = 2;
const NUMBER: i32 = 3;
const ACC_DESCR_T: i32 = 4;
const ACC_TITLE_T: i32 = 5;
const TITLE_T: i32 = 6;
const STRING_T: i32 = 7;
const NEWLINE_T: i32 = 8;

/// The end of a run of digits from `from` (`digits`).
fn digits(src: &Text, from: usize) -> usize {
    let mut i = from;
    while (48..=57).contains(&src.c(i)) {
        i += 1;
    }
    i
}

/// NUMBER_PIE's pattern, as `numberScanner` answers it:
/// `(?:-?[0-9]+\.[0-9]+(?!\.))|(?:-?(0|[1-9][0-9]*)(?!\.))`.
fn number(src: &Text, p: usize) -> Option<usize> {
    let at = if src.c(p) == 45 { p + 1 } else { p };
    let end = digits(src, at);
    if end == at {
        return None;
    }
    if src.c(end) == 46 {
        let stop = digits(src, end + 1);
        let length = stop - end - 1;
        if length > 0 && src.c(stop) != 46 {
            return Some(stop);
        } else if length > 1 {
            return Some(stop - 1);
        }
    }
    if src.c(at) == 48 {
        return if src.c(at + 1) == 46 {
            None
        } else {
            Some(at + 1)
        };
    }
    if src.c(end) != 46 {
        return Some(end);
    }
    if end - 1 > at {
        Some(end - 1)
    } else {
        None
    }
}

/// Ports `PIE_TOKENS`.
pub fn pie_tokens() -> Vec<TokenType> {
    vec![
        keyword("showData"),
        keyword("pie"),
        TokenType::lit(":", ":"),
        TokenType::re("NUMBER_PIE", number, Some("-0123456789")),
        acc_descr_type(),
        acc_title_type(),
        title_type(),
        string_type(),
        newline_type(),
        whitespace_type(),
        yaml_type(),
        directive_type(),
        single_line_comment_type(),
    ]
}

/// Ports `interface PieAst`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PieAst {
    pub show_data: bool,
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    pub sections: Vec<(String, f64)>,
}

/// Ports `parsePie`.
pub fn parse_pie(src: &str) -> Result<PieAst, PeleError> {
    let types = pie_tokens();
    let mut r = Reader::new(tokenize(src, &types, "pie")?, &types, "pie");
    let mut ast = PieAst::default();

    let end_of_line = |r: &mut Reader| -> Result<(), PeleError> {
        if r.kind() == -1 {
            return Ok(());
        }
        if r.kind() != NEWLINE_T {
            return Err(r.fail("a line break or the end of input"));
        }
        while r.kind() == NEWLINE_T {
            r.i += 1;
        }
        Ok(())
    };

    while r.kind() == NEWLINE_T {
        r.i += 1;
    }
    r.expect(PIE)?;
    ast.show_data = r.accept(SHOW_DATA);
    while r.kind() != -1 {
        match r.kind() {
            NEWLINE_T => r.i += 1,
            ACC_DESCR_T => {
                ast.acc_descr = Some(acc_descr_value(&r.take()));
                end_of_line(&mut r)?;
            }
            ACC_TITLE_T => {
                ast.acc_title = Some(acc_title_value(&r.take()));
                end_of_line(&mut r)?;
            }
            TITLE_T => {
                ast.title = Some(title_value(&r.take()));
                end_of_line(&mut r)?;
            }
            STRING_T => {
                let label = string_value(&r.take());
                r.expect(COLON)?;
                let value = js::number(&r.expect(NUMBER)?);
                ast.sections.push((label, value));
                end_of_line(&mut r)?;
            }
            _ => return Err(r.fail("a section, a title, or a line break")),
        }
    }
    Ok(ast)
}
