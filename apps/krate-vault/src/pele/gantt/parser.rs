//! Ports diagrams/gantt/parser.ts: the Jison grammar, by hand.

use alloc::format;
use alloc::string::String;

use super::db::GanttDb;
use super::lexer::tokenize;
use super::tokens::*;
use crate::pele::common::chars::Text;
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js::trim;

const WEEKDAYS: [&str; 7] = [
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
    "sunday",
];

const LINE: &str = "'EOF', 'SPACE', 'NL', 'weekday_monday', 'weekday_tuesday', 'weekday_wednesday', 'weekday_thursday', 'weekday_friday', 'weekday_saturday', 'weekday_sunday', 'weekend_friday', 'weekend_saturday', 'dateFormat', 'inclusiveEndDates', 'topAxis', 'axisFormat', 'tickInterval', 'excludes', 'includes', 'todayMarker', 'title', 'acc_title', 'acc_descr', 'acc_descr_multiline_value', 'section', 'taskTxt', 'click'";

/// `STARTS_LINE[t]`: tokens that can start a line, which are also the only
/// ones that can follow a complete statement.
fn starts_line(t: i32) -> bool {
    (WEEKDAY_MONDAY..=WEEKEND_SATURDAY).contains(&t)
        || matches!(
            t,
            EOF | NL
                | DATE_FORMAT
                | INCLUSIVE_END_DATES
                | TOP_AXIS
                | AXIS_FORMAT
                | TICK_INTERVAL
                | EXCLUDES
                | INCLUDES
                | TODAY_MARKER
                | TITLE
                | ACC_TITLE
                | ACC_DESCR
                | ACC_DESCR_MULTILINE_VALUE
                | SECTION
                | TASK_TXT
                | CLICK
        )
}

/// `text.slice(k)` past `k` characters of the Basic Multilingual Plane
/// (a keyword and a whitespace character).
fn after(text: &str, k: usize) -> &str {
    match text.char_indices().nth(k) {
        Some((i, _)) => &text[i..],
        None => "",
    }
}

/// Ports `parseGantt`.
pub fn parse_gantt(source: &str, db: &mut GanttDb) -> Result<(), PeleError> {
    let src = Text::new(source);
    let toks = tokenize(&src);
    let types = &toks.types;
    let texts = &toks.texts;
    let starts = &toks.starts;
    let mut i = 0usize;

    let fail = |i: usize, expected: &str| -> PeleError {
        let t = types[i];
        syntax_error(
            "gantt",
            source,
            src.byte(starts[i]),
            &format!(
                "Expecting {}, got '{}'",
                expected,
                if t == END {
                    "1"
                } else {
                    TOKEN_NAMES[t as usize]
                }
            ),
            false,
        )
    };

    // Mermaid's parser runs a statement's action only after seeing that the next token can follow it.
    let end = |i: usize, also: &str| -> Result<(), PeleError> {
        if !starts_line(types[i]) {
            return Err(fail(i, &format!("{}{}", LINE, also)));
        }
        Ok(())
    };

    if types[i] != GANTT {
        return Err(fail(i, "'gantt'"));
    }
    i += 1;

    loop {
        let t = types[i];
        let text = &texts[i];
        if !starts_line(t) {
            return Err(fail(i, LINE));
        }
        i += 1;
        match t {
            EOF => {
                if types[i] == END {
                    return Ok(());
                }
                end(i, "")?;
            }
            NL => {}
            DATE_FORMAT => {
                end(i, "")?;
                db.set_date_format(after(text, 11));
            }
            INCLUSIVE_END_DATES => {
                end(i, "")?;
                db.inclusive_end_dates = true;
            }
            TOP_AXIS => {
                end(i, "")?;
                db.top_axis = true;
            }
            AXIS_FORMAT => {
                end(i, "")?;
                db.axis_format = after(text, 11).into();
            }
            TICK_INTERVAL => {
                end(i, "")?;
                db.tick_interval = Some(after(text, 13).into());
            }
            EXCLUDES => {
                end(i, "")?;
                db.set_excludes(after(text, 9));
            }
            INCLUDES => {
                end(i, "")?;
                db.set_includes(after(text, 9));
            }
            TODAY_MARKER => {
                end(i, "")?;
                db.today_marker = after(text, 12).into();
            }
            WEEKEND_FRIDAY => {
                end(i, "")?;
                db.weekend = "friday".into();
            }
            WEEKEND_SATURDAY => {
                end(i, "")?;
                db.weekend = "saturday".into();
            }
            TITLE => {
                end(i, "")?;
                db.title = Some(after(text, 6).into());
            }
            ACC_TITLE => {
                if types[i] != ACC_TITLE_VALUE {
                    return Err(fail(i, "'acc_title_value'"));
                }
                let value = texts[i].clone();
                i += 1;
                end(i, "")?;
                db.set_acc_title(trim(&value));
            }
            ACC_DESCR => {
                if types[i] != ACC_DESCR_VALUE {
                    return Err(fail(i, "'acc_descr_value'"));
                }
                let value = texts[i].clone();
                i += 1;
                end(i, "")?;
                db.set_acc_description(trim(&value));
            }
            ACC_DESCR_MULTILINE_VALUE => {
                end(i, "")?;
                db.set_acc_description(trim(text));
            }
            SECTION => {
                end(i, "")?;
                db.add_section(after(text, 8));
            }
            TASK_TXT => {
                if types[i] != TASK_DATA {
                    return Err(fail(i, "'taskData'"));
                }
                let data = texts[i].clone();
                i += 1;
                end(i, "")?;
                db.add_task(text, &data)?;
            }
            CLICK => {
                let mut name: Option<String> = None;
                let mut args: Option<String> = None;
                let mut href: Option<String> = None;
                if types[i] == CALLBACKNAME {
                    name = Some(texts[i].clone());
                    i += 1;
                    if types[i] == CALLBACKARGS {
                        args = Some(texts[i].clone());
                        i += 1;
                    }
                    if types[i] == HREF {
                        href = Some(texts[i].clone());
                        i += 1;
                        end(i, "")?;
                    } else {
                        end(
                            i,
                            if args.is_none() {
                                ", 'callbackargs', 'href'"
                            } else {
                                ", 'href'"
                            },
                        )?;
                    }
                } else if types[i] == HREF {
                    href = Some(texts[i].clone());
                    i += 1;
                    if types[i] == CALLBACKNAME {
                        name = Some(texts[i].clone());
                        i += 1;
                        if types[i] == CALLBACKARGS {
                            args = Some(texts[i].clone());
                            i += 1;
                            end(i, "")?;
                        } else {
                            end(i, ", 'callbackargs'")?;
                        }
                    } else {
                        end(i, ", 'callbackname'")?;
                    }
                } else {
                    return Err(fail(i, "'callbackname', 'href'"));
                }
                let _ = args;
                if name.is_some() {
                    db.set_class(text, "clickable");
                }
                if let Some(href) = href {
                    db.set_link(text, &href);
                }
            }
            _ => {
                end(i, "")?;
                db.weekday = Some(WEEKDAYS[(t - WEEKDAY_MONDAY) as usize].into());
            }
        }
    }
}
