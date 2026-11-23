//! Ports diagrams/c4/parser.ts: accepts what the parser Jison generates
//! from c4Diagram.jison accepts, and calls the model builder in the order
//! its semantic actions do. Boundaries nest through a counter, not
//! recursion.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::C4Db;
use super::lexer::{tokenize, Tokens};
use super::tokens as t;
use super::types::{C4Attr, C4Named};
use crate::pele::common::chars::Text;
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js::trim;

/// Ports `KINDS`: Mermaid's typeC4Shape for each shape keyword, indexed
/// from T.PERSON.
const KINDS: [&str; 20] = [
    "person",
    "external_person",
    "system",
    "system_db",
    "system_queue",
    "external_system",
    "external_system_db",
    "external_system_queue",
    "container",
    "container_db",
    "container_queue",
    "external_container",
    "external_container_db",
    "external_container_queue",
    "component",
    "component_db",
    "component_queue",
    "external_component",
    "external_component_db",
    "external_component_queue",
];
const REL_TYPES: [&str; 8] = [
    "rel", "birel", "rel_u", "rel_d", "rel_l", "rel_r", "rel_b", "rel",
];
const NODE_TYPES: [&str; 3] = ["node", "nodeL", "nodeR"];

const HEADERS: &str = "'direction_tb', 'direction_bt', 'direction_rl', 'direction_lr', 'C4_CONTEXT', 'C4_CONTAINER', 'C4_COMPONENT', 'C4_DYNAMIC', 'C4_DEPLOYMENT'";
const ATTRIBUTES: &str = "'STR', 'STR_KEY', 'ATTRIBUTE', 'ATTRIBUTE_EMPTY'";

// `const enum P`.
/// A statement is required.
const ITEM: u8 = 0;
/// Only a diagram statement may come next.
const DIAGRAM: u8 = 1;
const ITEM_OR_END: u8 = 2;
const AFTER_OTHER: u8 = 3;
const AFTER_DIAGRAM: u8 = 4;

/// A token type past the end of the list: no type at all.
const NONE: u8 = u8::MAX;

/// Ports `isOther`.
fn is_other(kind: u8) -> bool {
    matches!(
        kind,
        t::title | t::accDescription | t::acc_title | t::acc_descr | t::acc_descr_multiline_value
    )
}

/// Ports `isDiagram`.
fn is_diagram(kind: u8) -> bool {
    (t::PERSON..=t::UPDATE_LAYOUT_CONFIG).contains(&kind)
}

struct Parser<'a> {
    src: &'a str,
    text: Text,
    tokens: Tokens,
    i: usize,
}

type R<T> = Result<T, PeleError>;

impl Parser<'_> {
    fn ty(&self, i: usize) -> u8 {
        self.tokens.types.get(i).copied().unwrap_or(NONE)
    }

    fn cur(&self) -> u8 {
        self.ty(self.i)
    }

    /// `text(at)`.
    fn text(&self, at: usize) -> String {
        match (self.tokens.starts.get(at), self.tokens.ends.get(at)) {
            (Some(&s), Some(&e)) => self.text.slice(s, e).into(),
            _ => String::new(),
        }
    }

    fn fail(&self, expected: &str) -> PeleError {
        let kind = self.cur();
        let at = self
            .tokens
            .starts
            .get(self.i)
            .map_or(self.src.len(), |&s| self.text.byte(s));
        if kind == t::ERROR {
            return syntax_error("c4", self.src, at, "", true);
        }
        let name = t::TOKEN_NAMES
            .get(kind as usize)
            .copied()
            .unwrap_or("undefined");
        syntax_error(
            "c4",
            self.src,
            at,
            &format!("Expecting {}, got '{}'", expected, name),
            false,
        )
    }

    fn expect(&mut self, kind: u8) -> R<()> {
        if self.cur() != kind {
            return Err(self.fail(&format!("'{}'", t::TOKEN_NAMES[kind as usize])));
        }
        self.i += 1;
        Ok(())
    }

    fn attributes(&mut self) -> R<Vec<C4Attr>> {
        let mut attrs = Vec::new();
        loop {
            let k = self.cur();
            if k == t::STR {
                attrs.push(C4Attr::Str(trim(&self.text(self.i)).into()));
                self.i += 1;
            } else if k == t::ATTRIBUTE_EMPTY {
                attrs.push(C4Attr::Str(String::new()));
                self.i += 1;
            } else if k == t::STR_KEY {
                self.i += 1;
                if self.cur() != t::STR_VALUE {
                    return Err(self.fail("'STR_VALUE'"));
                }
                attrs.push(C4Attr::Named(C4Named {
                    key: trim(&self.text(self.i - 1)).into(),
                    value: trim(&self.text(self.i)).into(),
                }));
                self.i += 1;
            } else {
                break;
            }
        }
        if attrs.is_empty() {
            return Err(self.fail(ATTRIBUTES));
        }
        Ok(attrs)
    }

    fn other(&mut self, db: &mut C4Db, k: u8) -> R<()> {
        if k == t::title {
            let text = self.text(self.i);
            self.i += 1;
            db.set_title(&text.chars().skip(6).collect::<String>());
        } else if k == t::accDescription {
            let text = self.text(self.i);
            self.i += 1;
            db.set_acc_description(&text.chars().skip(15).collect::<String>());
        } else if k == t::acc_descr_multiline_value {
            let text = self.text(self.i);
            self.i += 1;
            db.set_acc_description(trim(&text));
        } else {
            self.i += 1;
            if k == t::acc_title {
                if self.cur() != t::acc_title_value {
                    return Err(self.fail("'acc_title_value'"));
                }
                // The grammar hands `accTitle:` to setTitle, so it becomes the visible title.
                let text = self.text(self.i);
                self.i += 1;
                db.set_title(trim(&text));
            } else {
                if self.cur() != t::acc_descr_value {
                    return Err(self.fail("'acc_descr_value'"));
                }
                let text = self.text(self.i);
                self.i += 1;
                db.set_acc_description(trim(&text));
            }
        }
        Ok(())
    }

    /// Ports `diagram`: true when the statement opens a boundary.
    fn diagram(&mut self, db: &mut C4Db, k: u8) -> R<bool> {
        self.i += 1;
        let mut attrs = self.attributes()?;
        if k <= t::SYSTEM_EXT_QUEUE {
            db.add_person_or_system(KINDS[(k - t::PERSON) as usize], &attrs);
        } else if k <= t::CONTAINER_EXT_QUEUE {
            db.add_container(KINDS[(k - t::PERSON) as usize], &attrs);
        } else if k <= t::COMPONENT_EXT_QUEUE {
            db.add_component(KINDS[(k - t::PERSON) as usize], &attrs);
        } else if k <= t::NODE_R {
            let at = attrs.len().min(2);
            if k == t::ENTERPRISE_BOUNDARY {
                attrs.insert(at, C4Attr::Str("ENTERPRISE".into()));
            } else if k == t::SYSTEM_BOUNDARY {
                attrs.insert(at, C4Attr::Str("SYSTEM".into()));
            } else if k == t::CONTAINER_BOUNDARY {
                attrs.insert(at, C4Attr::Str("CONTAINER".into()));
            }
            if k >= t::NODE {
                db.add_deployment_node(NODE_TYPES[(k - t::NODE) as usize], &attrs);
            } else if k == t::CONTAINER_BOUNDARY {
                db.add_container_boundary(&attrs);
            } else {
                db.add_person_or_system_boundary(&attrs, "system");
            }
            if self.cur() == t::LBRACE {
                self.i += 1;
                self.expect(t::NEWLINE)?;
            } else if self.cur() == t::NEWLINE {
                self.i += 1;
                self.expect(t::LBRACE)?;
                if self.cur() == t::NEWLINE {
                    self.i += 1;
                }
            } else {
                return Err(self.fail("'NEWLINE', 'LBRACE'"));
            }
            return Ok(true);
        } else if k <= t::REL_INDEX {
            // RelIndex takes a sequence number first, which Mermaid ignores.
            if k == t::REL_INDEX {
                attrs.remove(0);
            }
            db.add_rel(REL_TYPES[(k - t::REL) as usize], &attrs);
        } else if k == t::UPDATE_EL_STYLE {
            db.update_el_style(&attrs);
        } else if k == t::UPDATE_REL_STYLE {
            db.update_rel_style(&attrs);
        } else {
            db.update_layout_config(&attrs);
        }
        Ok(false)
    }
}

/// Ports `parseC4`.
pub fn parse_c4(src: &str, db: &mut C4Db) -> Result<(), PeleError> {
    let mut p = Parser {
        src,
        text: Text::new(src),
        tokens: tokenize(src),
        i: 0,
    };

    let header = p.cur();
    if !(t::C4_CONTEXT..=t::C4_DEPLOYMENT).contains(&header) {
        return Err(p.fail(HEADERS));
    }
    p.i += 1;
    p.expect(t::NEWLINE)?;

    let mut depth = 0usize;
    let mut state = ITEM;
    loop {
        let k = p.cur();
        if state == AFTER_OTHER || state == AFTER_DIAGRAM {
            if k == t::NEWLINE {
                p.i += 1;
                state = ITEM_OR_END;
                continue;
            }
            if state == AFTER_OTHER && is_diagram(k) {
                state = DIAGRAM;
                continue;
            }
        } else if is_diagram(k) {
            if p.diagram(db, k)? {
                depth += 1;
                state = DIAGRAM;
            } else {
                state = AFTER_DIAGRAM;
            }
            continue;
        } else if is_other(k) && state != DIAGRAM {
            p.other(db, k)?;
            state = AFTER_OTHER;
            continue;
        } else if state != ITEM_OR_END {
            return Err(p.fail(
                "'title', 'accDescription', 'acc_title', 'acc_descr', 'acc_descr_multiline_value', 'PERSON', 'REL'",
            ));
        }

        if depth > 0 {
            if k != t::RBRACE {
                return Err(p.fail("'NEWLINE', 'RBRACE'"));
            }
            p.i += 1;
            db.pop_boundary_parse_stack();
            depth -= 1;
            state = AFTER_DIAGRAM;
        } else {
            if k != t::EOF {
                return Err(p.fail("'NEWLINE', 'EOF'"));
            }
            p.i += 1;
            p.expect(t::END)?;
            let header = p.text(0);
            db.set_c4_type(&header);
            return Ok(());
        }
    }
}
