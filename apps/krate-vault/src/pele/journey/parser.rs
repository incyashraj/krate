//! Ports diagrams/journey/parser.ts.

use alloc::format;

use super::db::JourneyDb;
use super::lexer::*;
use crate::pele::common::chars::Text;
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js::trim;

const STATEMENT: &str = "'EOF', 'SPACE', 'NEWLINE', 'title', 'acc_title', 'acc_descr', 'acc_descr_multiline_value', 'section', 'taskName'";

/// `text.substring(k)` past `k` characters that are all in the Basic
/// Multilingual Plane (keywords and a whitespace character).
fn after(text: &str, k: usize) -> &str {
    match text.char_indices().nth(k) {
        Some((i, _)) => &text[i..],
        None => "",
    }
}

/// Ports `parseJourney`.
pub fn parse_journey(source: &str, db: &mut JourneyDb) -> Result<(), PeleError> {
    let src = Text::new(source);
    let toks = tokenize(&src);
    let types = &toks.types;
    let texts = &toks.texts;
    let starts = &toks.starts;

    let fail = |i: usize, expected: &str| -> PeleError {
        let got = if types[i] == END {
            "1"
        } else {
            TOKEN_NAMES[types[i] as usize]
        };
        syntax_error(
            "journey",
            source,
            starts[i],
            &format!("Expecting {}, got '{}'", expected, got),
            false,
        )
    };

    if types[0] != JOURNEY {
        return Err(fail(0, "'journey'"));
    }

    let mut i = 1;
    loop {
        let text = &texts[i];
        match types[i] {
            NEWLINE => {}
            TITLE => db.set_diagram_title(after(text, 6)),
            ACC_TITLE => {
                i += 1;
                db.set_acc_title(trim(&texts[i]));
            }
            ACC_DESCR => {
                i += 1;
                db.set_acc_description(trim(&texts[i]));
            }
            ACC_DESCR_MULTILINE_VALUE => db.set_acc_description(trim(text)),
            SECTION => db.add_section(after(text, 8)),
            TASK_NAME => {
                i += 1;
                if types[i] != TASK_DATA {
                    return Err(fail(i, "'taskData'"));
                }
                db.add_task(text, &texts[i]);
            }
            EOF => return Ok(()),
            _ => return Err(fail(i, STATEMENT)),
        }
        i += 1;
    }
}
