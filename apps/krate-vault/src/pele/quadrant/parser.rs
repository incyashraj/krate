//! Ports diagrams/quadrant/parser.ts: the Jison grammar, by hand.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::{QuadrantDb, QuadrantText};
use super::lexer::tokenize;
use super::tokens::*;
use crate::pele::common::chars::Text;
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js::trim;

fn word(t: i32) -> bool {
    matches!(
        t,
        PUNCTUATION
            | AMP
            | NUM
            | ALPHA
            | COMMA
            | PLUS
            | EQUALS
            | MULT
            | DOT
            | BRKT
            | UNDERSCORE
            | UNICODE_TEXT
    )
}

fn id(t: i32) -> bool {
    matches!(
        t,
        ALPHA | NUM | MINUS | COMMA | COLON | AMP | BRKT | MULT | UNICODE_TEXT
    )
}

fn style(t: i32) -> bool {
    matches!(t, ALPHA | NUM | COLON | SPACE | BRKT | MINUS)
}

fn eol(t: i32) -> bool {
    matches!(t, NEWLINE | SEMI | EOF)
}

const TEXT_TOKENS: &str = "'STR', 'MD_STR', 'ALPHA', 'NUM', 'UNICODE_TEXT', 'PUNCTUATION'";

/// Ports `parseQuadrant`.
pub fn parse_quadrant(source: &str, db: &mut QuadrantDb) -> Result<(), PeleError> {
    let src = Text::new(source);
    let toks = tokenize(&src);
    let types = &toks.types;
    let texts = &toks.texts;
    let starts = &toks.starts;
    let mut i = 0usize;

    let fail = |i: usize, expected: &str| -> PeleError {
        let t = types[i];
        let at = src.byte(starts[i]);
        if t == ERROR {
            return syntax_error("quadrantChart", source, at, "", true);
        }
        syntax_error(
            "quadrantChart",
            source,
            at,
            &format!("Expecting {}, got '{}'", expected, TOKEN_NAMES[t as usize]),
            false,
        )
    };

    let expect = |i: &mut usize, ty: i32| -> Result<String, PeleError> {
        if types[*i] != ty {
            return Err(fail(*i, &format!("'{}'", TOKEN_NAMES[ty as usize])));
        }
        *i += 1;
        Ok(texts[*i - 1].clone())
    };

    let is_text = |t: i32| -> bool { t == STR || t == MD_STR || word(t) };

    let text = |i: &mut usize| -> Result<QuadrantText, PeleError> {
        let first = types[*i];
        if !is_text(first) {
            return Err(fail(*i, TEXT_TOKENS));
        }
        let mut s = texts[*i].clone();
        *i += 1;
        while word(types[*i]) || types[*i] == SPACE || types[*i] == MINUS {
            s.push_str(&texts[*i]);
            *i += 1;
        }
        Ok(QuadrantText {
            text: s,
            markdown: first == MD_STR,
        })
    };

    let styles = |i: &mut usize| -> Result<Vec<String>, PeleError> {
        let mut out = Vec::new();
        loop {
            if !style(types[*i]) {
                return Err(fail(
                    *i,
                    "'ALPHA', 'NUM', 'COLON', 'SPACE', 'BRKT', 'MINUS'",
                ));
            }
            let mut s = texts[*i].clone();
            *i += 1;
            while style(types[*i]) {
                s.push_str(&texts[*i]);
                *i += 1;
            }
            out.push(String::from(trim(&s)));
            if types[*i] != COMMA {
                return Ok(out);
            }
            *i += 1;
        }
    };

    while eol(types[i]) || types[i] == SPACE {
        i += 1;
    }
    expect(&mut i, QUADRANT)?;

    while types[i] != END {
        while types[i] == SPACE {
            i += 1;
        }
        let t = types[i];
        match t {
            CLASSDEF => {
                i += 1;
                expect(&mut i, SPACE)?;
                if !id(types[i]) {
                    return Err(fail(i, "'ALPHA', 'NUM', 'UNICODE_TEXT'"));
                }
                let mut name = texts[i].clone();
                i += 1;
                while id(types[i]) {
                    name.push_str(&texts[i]);
                    i += 1;
                }
                expect(&mut i, SPACE)?;
                let list = styles(&mut i)?;
                db.add_class(&name, &list)?;
            }
            X_AXIS | Y_AXIS => {
                i += 1;
                let mut from = text(&mut i)?;
                let mut to: Option<QuadrantText> = None;
                if types[i] == DELIMITER {
                    i += 1;
                    if is_text(types[i]) {
                        to = Some(text(&mut i)?);
                    } else {
                        from.text.push_str(" \u{27f6} ");
                    }
                }
                if t == X_AXIS {
                    db.set_x_axis_left_text(&from);
                    if let Some(to) = &to {
                        db.set_x_axis_right_text(to);
                    }
                } else {
                    db.set_y_axis_bottom_text(&from);
                    if let Some(to) = &to {
                        db.set_y_axis_top_text(to);
                    }
                }
            }
            QUADRANT_1 | QUADRANT_2 | QUADRANT_3 | QUADRANT_4 => {
                i += 1;
                let label = text(&mut i)?;
                db.set_quadrant_text((t - QUADRANT_1) as usize, &label);
            }
            TITLE => {
                i += 1;
                let v = expect(&mut i, TITLE_VALUE)?;
                db.set_diagram_title(trim(&v));
            }
            ACC_TITLE => {
                i += 1;
                let v = expect(&mut i, ACC_TITLE_VALUE)?;
                db.set_acc_title(trim(&v));
            }
            ACC_DESCR => {
                i += 1;
                let v = expect(&mut i, ACC_DESCR_VALUE)?;
                db.set_acc_description(trim(&v));
            }
            ACC_DESCR_MULTILINE_VALUE => {
                let v = texts[i].clone();
                i += 1;
                db.set_acc_description(trim(&v));
            }
            _ => {
                if is_text(t) {
                    let label = text(&mut i)?;
                    let class_name = if types[i] == CLASS_NAME {
                        i += 1;
                        texts[i - 1].clone()
                    } else {
                        String::new()
                    };
                    expect(&mut i, POINT_START)?;
                    let x = expect(&mut i, POINT_X)?;
                    let y = expect(&mut i, POINT_Y)?;
                    let list = if style(types[i]) {
                        styles(&mut i)?
                    } else {
                        Vec::new()
                    };
                    db.add_point(&label, &class_name, &x, &y, &list)?;
                }
            }
        }
        if !eol(types[i]) {
            return Err(fail(i, "'NEWLINE', 'SEMI', 'EOF'"));
        }
        i += 1;
    }
    Ok(())
}
