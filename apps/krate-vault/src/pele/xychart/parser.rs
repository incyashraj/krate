//! Ports diagrams/xychart/parser.ts: the Jison grammar, by hand.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::{DataPoint, XyChartDb, XyText};
use super::lexer::tokenize;
use super::tokens::*;
use crate::pele::common::chars::Text;
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js::{number, trim};

fn word(t: i32) -> bool {
    matches!(
        t,
        AMP | NUM | ALPHA | PLUS | EQUALS | MULT | DOT | BRKT | MINUS | UNDERSCORE
    )
}

fn eol(t: i32) -> bool {
    matches!(t, NEWLINE | SEMI | EOF)
}

/// Ports `parseXyChart`.
pub fn parse_xy_chart(source: &str, db: &mut XyChartDb) -> Result<(), PeleError> {
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
            return syntax_error("xychart", source, at, "", true);
        }
        syntax_error(
            "xychart",
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

    let is_text = |t: i32| -> bool { t == STR || word(t) };

    let text = |i: &mut usize| -> Result<XyText, PeleError> {
        if !is_text(types[*i]) {
            return Err(fail(*i, "'STR', 'ALPHA', 'NUM'"));
        }
        let mut s = texts[*i].clone();
        *i += 1;
        if types[*i - 1] != STR {
            while word(types[*i]) {
                s.push_str(&texts[*i]);
                *i += 1;
            }
        }
        Ok(XyText { text: s })
    };

    let range = |i: &mut usize| -> Result<(f64, f64), PeleError> {
        let min = number(&expect(i, NUMBER_WITH_DECIMAL)?);
        expect(i, ARROW_DELIMITER)?;
        Ok((min, number(&expect(i, NUMBER_WITH_DECIMAL)?)))
    };

    let points = |i: &mut usize| -> Result<Vec<DataPoint>, PeleError> {
        let mut out = Vec::new();
        expect(i, SQUARE_BRACES_START)?;
        loop {
            let value = number(&expect(i, NUMBER_WITH_DECIMAL)?);
            let label = if types[*i] == STR {
                *i += 1;
                texts[*i - 1].clone()
            } else {
                String::new()
            };
            out.push(DataPoint { value, label });
            if types[*i] != COMMA {
                break;
            }
            *i += 1;
        }
        expect(i, SQUARE_BRACES_END)?;
        Ok(out)
    };

    loop {
        if eol(types[i]) {
            i += 1;
        } else if types[i] != XYCHART {
            break;
        } else {
            i += 1;
            if types[i] == CHART_ORIENTATION {
                db.set_orientation(&texts[i]);
                i += 1;
            }
        }
    }

    while types[i] != END {
        let t = types[i];
        i += 1;
        match t {
            TITLE => {
                let v = text(&mut i)?;
                db.set_diagram_title(trim(&v.text));
            }
            X_AXIS | Y_AXIS => {
                let title = if is_text(types[i]) {
                    Some(text(&mut i)?)
                } else {
                    None
                };
                if t == X_AXIS && types[i] == SQUARE_BRACES_START {
                    let mut categories: Vec<XyText> = Vec::new();
                    loop {
                        i += 1;
                        categories.push(text(&mut i)?);
                        if types[i] != COMMA {
                            break;
                        }
                    }
                    expect(&mut i, SQUARE_BRACES_END)?;
                    db.set_x_axis_band(&categories);
                } else if types[i] == NUMBER_WITH_DECIMAL || title.is_none() {
                    let (min, max) = range(&mut i)?;
                    if t == X_AXIS {
                        db.set_x_axis_range_data(min, max);
                    } else {
                        db.set_y_axis_range_data(min, max);
                    }
                }
                let title = title.unwrap_or_default();
                if t == X_AXIS {
                    db.set_x_axis_title(&title);
                } else {
                    db.set_y_axis_title(&title);
                }
            }
            LINE | BAR => {
                let title = if types[i] == SQUARE_BRACES_START {
                    XyText::default()
                } else {
                    text(&mut i)?
                };
                let data = points(&mut i)?;
                if t == LINE {
                    db.set_line_data(&title, &data);
                } else {
                    db.set_bar_data(&title, &data);
                }
            }
            ACC_TITLE => {
                let v = expect(&mut i, ACC_TITLE_VALUE)?;
                db.set_acc_title(trim(&v));
            }
            ACC_DESCR => {
                let v = expect(&mut i, ACC_DESCR_VALUE)?;
                db.set_acc_description(trim(&v));
            }
            ACC_DESCR_MULTILINE_VALUE => {
                db.set_acc_description(trim(&texts[i - 1]));
            }
            _ => {
                i -= 1;
                return Err(fail(i, "'title', 'X_AXIS', 'Y_AXIS', 'LINE', 'BAR', 'acc_title', 'acc_descr', 'acc_descr_multiline_value'"));
            }
        }
        while eol(types[i]) {
            i += 1;
        }
    }
    Ok(())
}
