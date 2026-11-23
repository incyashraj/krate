//! Ports diagrams/requirement/parser.ts: makes the calls on the db that
//! Mermaid's requirement grammar makes, and reports errors where its
//! generated parser would.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::{values as v, RequirementDb};
use super::lexer::{tokenize, Tokens};
use super::tokens as t;
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js::trim;

/// Ports `VALUES`: what the keyword tokens stand for, from REQUIREMENT to
/// TRACES in token order.
const VALUES: [&str; 21] = [
    v::REQUIREMENT,
    v::FUNCTIONAL_REQUIREMENT,
    v::INTERFACE_REQUIREMENT,
    v::PERFORMANCE_REQUIREMENT,
    v::PHYSICAL_REQUIREMENT,
    v::DESIGN_CONSTRAINT,
    v::LOW_RISK,
    v::MED_RISK,
    v::HIGH_RISK,
    v::VERIFY_ANALYSIS,
    v::VERIFY_DEMONSTRATION,
    v::VERIFY_INSPECTION,
    v::VERIFY_TEST,
    "",
    v::CONTAINS,
    v::COPIES,
    v::DERIVES,
    v::SATISFIES,
    v::VERIFIES,
    v::REFINES,
    v::TRACES,
];

/// A token type past the end of the list: no type at all.
const NONE: u8 = u8::MAX;

/// Ports `isStylePart`.
fn is_style_part(kind: u8) -> bool {
    matches!(
        kind,
        t::ALPHA | t::COLON | t::BRKT | t::MINUS | t::SEMICOLON
    )
}

struct Parser<'a> {
    src: &'a str,
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

    fn text(&self, i: usize) -> String {
        self.tokens.texts.get(i).cloned().unwrap_or_default()
    }

    fn next_text(&mut self) -> String {
        let s = self.text(self.i);
        self.i += 1;
        s
    }

    /// Ports `fail`: Jison's wording, naming the token types `from` to `to`
    /// and any others as expected.
    fn fail(&self, from: u8, to: u8, others: &[u8]) -> PeleError {
        let kind = self.cur();
        let at = self
            .tokens
            .starts
            .get(self.i)
            .copied()
            .unwrap_or(self.src.len());
        if kind == t::ERROR {
            return syntax_error("requirement", self.src, at, "", true);
        }
        let mut list: Vec<u8> = (from..=to).collect();
        list.extend_from_slice(others);
        let expected: Vec<String> = list
            .iter()
            .map(|&k| format!("'{}'", t::TOKEN_NAMES[k as usize]))
            .collect();
        let name = t::TOKEN_NAMES
            .get(kind as usize)
            .copied()
            .unwrap_or("undefined");
        syntax_error(
            "requirement",
            self.src,
            at,
            &format!("Expecting {}, got '{}'", expected.join(", "), name),
            false,
        )
    }

    fn expect(&mut self, kind: u8) -> R<String> {
        if self.cur() != kind {
            return Err(self.fail(kind, kind, &[]));
        }
        Ok(self.next_text())
    }

    /// Ports `keyword`: the value of the next token if it is a keyword in
    /// the given range of token types.
    fn keyword(&mut self, from: u8, to: u8) -> R<&'static str> {
        let k = self.cur();
        if k < from || k > to {
            return Err(self.fail(from, to, &[]));
        }
        self.i += 1;
        Ok(VALUES[(k - t::REQUIREMENT) as usize])
    }

    /// Ports `string`.
    fn string(&mut self) -> R<String> {
        let k = self.cur();
        if k != t::unqString && k != t::qString {
            return Err(self.fail(t::qString, t::unqString, &[]));
        }
        Ok(self.next_text())
    }

    /// Ports `directive`.
    fn directive(&mut self, db: &mut RequirementDb) -> R<bool> {
        let k = self.cur();
        if k == t::acc_title {
            self.i += 1;
            let value = self.expect(t::acc_title_value)?;
            db.set_acc_title(trim(&value));
        } else if k == t::acc_descr {
            self.i += 1;
            let value = self.expect(t::acc_descr_value)?;
            db.set_acc_description(trim(&value));
        } else if k == t::acc_descr_multiline_value {
            let value = self.next_text();
            db.set_acc_description(trim(&value));
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// Ports `idList`.
    fn id_list(&mut self) -> R<Vec<String>> {
        let mut list = Vec::new();
        loop {
            let k = self.cur();
            if k != t::ALPHA && k != t::unqString && k != t::qString {
                return Err(self.fail(t::qString, t::unqString, &[t::ALPHA]));
            }
            list.push(self.next_text());
            if self.cur() != t::COMMA {
                return Ok(list);
            }
            self.i += 1;
        }
    }

    /// Ports `stylesOpt`.
    fn styles_opt(&mut self) -> R<Vec<String>> {
        let mut styles = Vec::new();
        loop {
            if !is_style_part(self.cur()) {
                return Err(self.fail(t::ALPHA, t::SEMICOLON, &[t::MINUS, t::BRKT]));
            }
            let mut style = self.next_text();
            while is_style_part(self.cur()) {
                style.push_str(&self.next_text());
            }
            styles.push(style);
            if self.cur() != t::COMMA {
                return Ok(styles);
            }
            self.i += 1;
        }
    }

    /// Ports `classList`.
    fn class_list(&mut self) -> R<Option<Vec<String>>> {
        let mut classes = None;
        if self.cur() == t::STYLE_SEPARATOR {
            self.i += 1;
            classes = Some(self.id_list()?);
            self.expect(t::STRUCT_START)?;
        } else if self.cur() == t::STRUCT_START {
            self.i += 1;
        } else {
            return Err(self.fail(t::STRUCT_START, t::STRUCT_START, &[t::STYLE_SEPARATOR]));
        }
        self.expect(t::NEWLINE)?;
        Ok(classes)
    }

    /// Ports `body`: reads a body up to its closing brace. The grammar is
    /// right recursive, so Mermaid applies the fields last to first and the
    /// first of two repeated fields wins.
    fn body(&mut self, db: &mut RequirementDb, requirement: bool) -> R<()> {
        let mut fields: Vec<u8> = Vec::new();
        let mut values: Vec<String> = Vec::new();
        loop {
            let k = self.cur();
            if k == t::STRUCT_STOP {
                self.i += 1;
                break;
            }
            if k == t::NEWLINE {
                self.i += 1;
                continue;
            }
            let bad = if requirement {
                !(t::ID..=t::VERIFYMTHD).contains(&k)
            } else {
                k != t::TYPE && k != t::DOCREF
            };
            if bad {
                if requirement {
                    return Err(self.fail(t::ID, t::VERIFYMTHD, &[t::NEWLINE, t::STRUCT_STOP]));
                }
                return Err(self.fail(t::TYPE, t::DOCREF, &[t::NEWLINE, t::STRUCT_STOP]));
            }
            self.i += 1;
            self.expect(t::COLONSEP)?;
            let value = if k == t::RISK {
                String::from(self.keyword(t::LOW_RISK, t::HIGH_RISK)?)
            } else if k == t::VERIFYMTHD {
                String::from(self.keyword(t::VERIFY_ANALYSIS, t::VERIFY_TEST)?)
            } else {
                self.string()?
            };
            values.push(value);
            fields.push(k);
            self.expect(t::NEWLINE)?;
        }
        for k in (0..fields.len()).rev() {
            let value = &values[k];
            match fields[k] {
                t::ID => db.set_new_req_id(value),
                t::TEXT => db.set_new_req_text(value),
                t::RISK => db.set_new_req_risk(value),
                t::VERIFYMTHD => db.set_new_req_verify_method(value),
                t::TYPE => db.set_new_element_type(value),
                _ => db.set_new_element_doc_ref(value),
            }
        }
        Ok(())
    }
}

/// Ports `parseRequirement`.
pub fn parse_requirement(src: &str, db: &mut RequirementDb) -> Result<(), PeleError> {
    let mut p = Parser {
        src,
        tokens: tokenize(src),
        i: 0,
    };

    while p.directive(db)? {
        if p.cur() == t::NEWLINE {
            p.i += 1;
        }
    }
    if p.cur() != t::RD {
        return Err(p.fail(
            t::RD,
            t::RD,
            &[t::acc_title, t::acc_descr, t::acc_descr_multiline_value],
        ));
    }
    p.i += 1;
    p.expect(t::NEWLINE)?;

    loop {
        let k = p.cur();
        if k == t::EOF {
            return Ok(());
        }
        if k == t::NEWLINE {
            p.i += 1;
        } else if p.directive(db)? {
            continue;
        } else if (t::direction_tb..=t::direction_lr).contains(&k) {
            p.i += 1;
            db.set_direction(match k {
                t::direction_tb => "TB",
                t::direction_bt => "BT",
                t::direction_rl => "RL",
                _ => "LR",
            });
        } else if (t::REQUIREMENT..=t::DESIGN_CONSTRAINT).contains(&k) {
            let kind = p.keyword(t::REQUIREMENT, t::DESIGN_CONSTRAINT)?;
            let name = p.string()?;
            let classes = p.class_list()?;
            p.body(db, true)?;
            db.add_requirement(&name, kind);
            if let Some(classes) = classes {
                db.set_class(core::slice::from_ref(&name), &classes)?;
            }
        } else if k == t::ELEMENT {
            p.i += 1;
            let name = p.string()?;
            let classes = p.class_list()?;
            p.body(db, false)?;
            db.add_element(&name);
            if let Some(classes) = classes {
                db.set_class(core::slice::from_ref(&name), &classes)?;
            }
        } else if k == t::STYLE {
            p.i += 1;
            let ids = p.id_list()?;
            let styles = p.styles_opt()?;
            db.set_css_style(&ids, &styles)?;
        } else if k == t::CLASSDEF {
            p.i += 1;
            let ids = p.id_list()?;
            let styles = p.styles_opt()?;
            db.define_class(&ids, &styles)?;
        } else if k == t::CLASS {
            p.i += 1;
            let ids = p.id_list()?;
            let names = p.id_list()?;
            db.set_class(&ids, &names)?;
        } else if k == t::unqString || k == t::qString {
            let id = p.next_text();
            let next = p.cur();
            if next == t::STYLE_SEPARATOR {
                p.i += 1;
                let names = p.id_list()?;
                db.set_class(core::slice::from_ref(&id), &names)?;
            } else if next == t::END_ARROW_L || next == t::LINE {
                p.i += 1;
                let relationship = p.keyword(t::CONTAINS, t::TRACES)?;
                p.expect(if next == t::LINE {
                    t::END_ARROW_R
                } else {
                    t::LINE
                })?;
                let other = p.string()?;
                if next == t::LINE {
                    db.add_relationship(relationship, &id, &other);
                } else {
                    db.add_relationship(relationship, &other, &id);
                }
            } else {
                return Err(p.fail(
                    t::STYLE_SEPARATOR,
                    t::STYLE_SEPARATOR,
                    &[t::END_ARROW_L, t::LINE],
                ));
            }
        } else {
            return Err(p.fail(
                t::NEWLINE,
                t::EOF,
                &[
                    t::acc_title,
                    t::acc_descr,
                    t::acc_descr_multiline_value,
                    t::direction_tb,
                    t::direction_bt,
                    t::direction_rl,
                    t::direction_lr,
                    t::REQUIREMENT,
                    t::FUNCTIONAL_REQUIREMENT,
                    t::INTERFACE_REQUIREMENT,
                    t::PERFORMANCE_REQUIREMENT,
                    t::PHYSICAL_REQUIREMENT,
                    t::DESIGN_CONSTRAINT,
                    t::ELEMENT,
                    t::CLASSDEF,
                    t::CLASS,
                    t::STYLE,
                    t::unqString,
                    t::qString,
                ],
            ));
        }
    }
}
