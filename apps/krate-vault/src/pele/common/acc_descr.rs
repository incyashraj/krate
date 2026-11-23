//! Ports diagrams/common/accDescr.ts: the accDescr terminal for grammars
//! where lexing goes on after an `accDescr {` that never closes, because
//! `accDescr` is also a name there.

use alloc::boxed::Box;

use super::chars::Text;
use super::tokens::{acc_descr_open, acc_descr_type, Scanner, TokenType};

/// Ports `scanner`: the braced form scans for its closing brace. Once none
/// is found there is none further on, so later braced descriptions fail
/// without scanning the rest of the text again.
fn scanner() -> Scanner {
    let mut unclosed = false;
    Box::new(move |src: &Text, at: usize, _previous: i32| -> isize {
        let Some((end, braced)) = acc_descr_open(src, at) else {
            return -1;
        };
        if !braced {
            return end as isize;
        }
        if unclosed {
            return -1;
        }
        match src.find_char(125, end) {
            Some(close) => (close + 1) as isize,
            None => {
                unclosed = true;
                -1
            }
        }
    })
}

/// Ports `accDescr`: the common accDescr terminal, named `name`.
pub fn acc_descr(name: &str) -> TokenType {
    let mut t = acc_descr_type();
    t.name = name.into();
    t.scanner = Some(scanner);
    t
}
