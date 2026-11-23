//! Ports diagrams/class/parser.ts: makes the calls on the db that
//! Mermaid's class grammar makes, and reports errors where its generated
//! parser would.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::ClassDb;
use super::lexer::{tokenize, Tokens};
use super::tokens as t;
use super::types::{ClassRelation, Relation, RelationEnd};
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js::trim;

/// A token type past the end of the list: no type at all.
const NONE: u8 = u8::MAX;

/// Ports `ALPHANUM`.
fn alphanum(kind: u8) -> bool {
    matches!(kind, t::UNICODE_TEXT | t::ALPHA | t::MINUS)
}

/// Ports `STYLE_PART`.
fn style_part(kind: u8) -> bool {
    matches!(
        kind,
        t::ALPHA | t::COLON | t::BRKT | t::STYLE | t::PCT | t::LABEL
    )
}

/// Ports `END_TYPE`: Mermaid's relationType numbers, plus one so that zero
/// means "not an end marker".
fn end_type(kind: u8) -> u8 {
    match kind {
        t::AGGREGATION => 1,
        t::EXTENSION => 2,
        t::COMPOSITION => 3,
        t::DEPENDENCY => 4,
        t::LOLLIPOP => 5,
        _ => 0,
    }
}

const NAME_TOKENS: &str = "'UNICODE_TEXT', 'NUM', 'ALPHA', 'MINUS', 'BQUOTE_STR'";

/// Ports `interface Frame`.
struct Frame {
    id: String,
    classes: Vec<String>,
    notes: Vec<String>,
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

    fn fail(&self, expected: &str) -> PeleError {
        let kind = self.cur();
        let at = self
            .tokens
            .starts
            .get(self.i)
            .copied()
            .unwrap_or(self.src.len());
        if kind == t::ERROR {
            return syntax_error("class", self.src, at, "", true);
        }
        let name = t::TOKEN_NAMES
            .get(kind as usize)
            .copied()
            .unwrap_or("undefined");
        syntax_error(
            "class",
            self.src,
            at,
            &format!("Expecting {}, got '{}'", expected, name),
            false,
        )
    }

    fn expect(&mut self, kind: u8) -> R<String> {
        if self.cur() != kind {
            return Err(self.fail(&format!("'{}'", t::TOKEN_NAMES[kind as usize])));
        }
        Ok(self.next_text())
    }

    fn alpha_num(&mut self) -> R<String> {
        if !alphanum(self.cur()) {
            return Err(self.fail("'UNICODE_TEXT', 'NUM', 'ALPHA', 'MINUS'"));
        }
        Ok(self.next_text())
    }

    fn name(&mut self, generic: bool) -> R<String> {
        let mut out = String::new();
        loop {
            if self.cur() == t::BQUOTE_STR {
                out.push_str(&self.next_text());
                break;
            }
            if !alphanum(self.cur()) {
                return Err(self.fail(NAME_TOKENS));
            }
            out.push_str(&self.next_text());
            let k = self.cur();
            if k == t::DOT {
                out.push('.');
                self.i += 1;
            } else if !alphanum(k) && k != t::BQUOTE_STR {
                break;
            }
        }
        if generic && self.cur() == t::GENERICTYPE {
            out.push('~');
            out.push_str(&self.next_text());
            out.push('~');
        }
        Ok(out)
    }

    fn class_label(&mut self) -> R<String> {
        self.i += 1;
        let label = self.expect(t::STR)?;
        self.expect(t::SQE)?;
        Ok(label)
    }

    fn members(&mut self) -> R<Vec<String>> {
        if self.cur() != t::MEMBER {
            return Err(self.fail("'STRUCT_STOP', 'MEMBER'"));
        }
        let mut list: Vec<String> = Vec::new();
        while self.cur() == t::MEMBER {
            list.push(self.next_text());
        }
        self.expect(t::STRUCT_STOP)?;
        // The grammar collects members last to first, and addMembers turns them back.
        list.reverse();
        Ok(list)
    }

    fn class_statement(&mut self, db: &mut ClassDb) -> R<String> {
        self.i += 1;
        let id = self.name(true)?;
        db.add_class(&id);
        if self.cur() == t::SQS {
            let label = self.class_label()?;
            db.set_class_label(&id, &label);
        }
        match self.cur() {
            t::STYLE_SEPARATOR => {
                self.i += 1;
                let css = self.alpha_num()?;
                db.set_css_class(&id, &css);
                if self.cur() == t::STRUCT_START {
                    self.i += 1;
                    let m = self.members()?;
                    db.add_members(&id, m);
                }
            }
            t::STRUCT_START => {
                self.i += 1;
                if self.cur() == t::STRUCT_STOP {
                    self.i += 1;
                } else {
                    let m = self.members()?;
                    db.add_members(&id, m);
                }
            }
            t::ANNOTATION_START => {
                self.i += 1;
                let annotation = self.alpha_num()?;
                self.expect(t::ANNOTATION_END)?;
                db.add_annotation(&id, &annotation);
                if self.cur() == t::STRUCT_START {
                    self.i += 1;
                    if self.cur() == t::STRUCT_STOP {
                        self.i += 1;
                    } else {
                        let m = self.members()?;
                        db.add_members(&id, m);
                    }
                }
            }
            _ => {}
        }
        Ok(id)
    }

    fn note_statement(&mut self, db: &mut ClassDb) -> R<String> {
        let k = self.cur();
        self.i += 1;
        if k == t::NOTE {
            let text = self.expect(t::STR)?;
            return Ok(db.add_note(&text, None));
        }
        let id = self.name(true)?;
        let text = self.expect(t::STR)?;
        Ok(db.add_note(&text, Some(&id)))
    }

    /// Namespaces nest, so they are parsed with a stack of open ones.
    fn namespace_statement(&mut self, db: &mut ClassDb) -> R<()> {
        let mut open: Vec<Frame> = Vec::new();
        loop {
            self.i += 1;
            let ns = self.name(false)?;
            let id = if self.cur() == t::SQS {
                let label = self.class_label()?;
                db.add_namespace(&ns, Some(&label))
            } else {
                db.add_namespace(&ns, None)
            };
            self.expect(t::STRUCT_START)?;
            if self.cur() == t::NEWLINE {
                self.i += 1;
            }
            open.push(Frame {
                id,
                classes: Vec::new(),
                notes: Vec::new(),
            });
            let mut item = true;
            while item {
                let k = self.cur();
                if k == t::NAMESPACE {
                    break;
                }
                if k == t::CLASS {
                    let c = self.class_statement(db)?;
                    open.last_mut().unwrap().classes.push(c);
                } else if k == t::NOTE || k == t::NOTE_FOR {
                    let n = self.note_statement(db)?;
                    open.last_mut().unwrap().notes.push(n);
                } else {
                    return Err(self.fail("'NAMESPACE', 'CLASS', 'NOTE_FOR', 'NOTE'"));
                }
                loop {
                    if self.cur() == t::NEWLINE {
                        let next = self.ty(self.i + 1);
                        if next == t::NAMESPACE
                            || next == t::CLASS
                            || next == t::NOTE
                            || next == t::NOTE_FOR
                        {
                            self.i += 1;
                            break;
                        }
                        self.i += 1;
                    }
                    self.expect(t::STRUCT_STOP)?;
                    let done = open.pop().unwrap();
                    db.add_classes_to_namespace(&done.id, &done.classes, &done.notes);
                    db.pop_namespace();
                    if open.is_empty() {
                        item = false;
                        break;
                    }
                }
            }
            if open.is_empty() {
                return Ok(());
            }
        }
    }

    fn relation_end(&mut self) -> RelationEnd {
        let kind = end_type(self.cur());
        if kind == 0 {
            return None;
        }
        self.i += 1;
        Some(kind - 1)
    }

    fn class_or_relation(&mut self, db: &mut ClassDb) -> R<()> {
        let id1 = self.name(true)?;
        let mut k = self.cur();
        if k == t::LABEL {
            let text = self.next_text();
            let label = db.cleanup_label(&text);
            db.add_member(&id1, &label);
            return Ok(());
        }
        let mut title1 = String::from("none");
        if k == t::STR {
            title1 = self.next_text();
            k = self.cur();
        } else if end_type(k) == 0 && k != t::LINE && k != t::DOTTED_LINE {
            return Ok(());
        }
        let _ = k;
        let type1 = self.relation_end();
        let line = self.cur();
        if line != t::LINE && line != t::DOTTED_LINE {
            return Err(self.fail(
                "'AGGREGATION', 'EXTENSION', 'COMPOSITION', 'DEPENDENCY', 'LOLLIPOP', 'LINE', 'DOTTED_LINE'",
            ));
        }
        self.i += 1;
        let type2 = self.relation_end();
        let title2 = if self.cur() == t::STR {
            self.next_text()
        } else {
            String::from("none")
        };
        let id2 = self.name(true)?;
        let mut relation = ClassRelation {
            id1,
            id2,
            relation: Relation {
                type1,
                type2,
                line_type: if line == t::LINE { 0 } else { 1 },
            },
            relation_title1: title1,
            relation_title2: title2,
            title: None,
        };
        if self.cur() == t::LABEL {
            let text = self.next_text();
            relation.title = Some(db.cleanup_label(&text));
        }
        db.add_relation(relation);
        Ok(())
    }

    fn styles(&mut self) -> R<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        loop {
            if !style_part(self.cur()) {
                return Err(self.fail(
                    "'ALPHA', 'NUM', 'COLON', 'UNIT', 'SPACE', 'BRKT', 'STYLE', 'PCT', 'LABEL'",
                ));
            }
            let mut s = self.next_text();
            while style_part(self.cur()) {
                s.push_str(&self.next_text());
            }
            out.push(s);
            if self.cur() != t::COMMA {
                return Ok(out);
            }
            self.i += 1;
        }
    }

    fn tooltip(&mut self, db: &mut ClassDb, id: &str) {
        if self.cur() == t::STR {
            let tip = self.next_text();
            db.set_tooltip(id, Some(&tip));
        }
    }

    fn link_statement(&mut self, db: &mut ClassDb, id: &str) -> R<()> {
        let href = self.expect(t::STR)?;
        let tip = if self.cur() == t::STR {
            Some(self.next_text())
        } else {
            None
        };
        if self.cur() == t::LINK_TARGET {
            let target = self.next_text();
            db.set_link(id, &href, Some(&target));
        } else {
            db.set_link(id, &href, None);
        }
        if let Some(tip) = tip {
            db.set_tooltip(id, Some(&tip));
        }
        Ok(())
    }

    fn statement(&mut self, db: &mut ClassDb) -> R<()> {
        let k = self.cur();
        match k {
            t::UNICODE_TEXT | t::ALPHA | t::MINUS | t::BQUOTE_STR => {
                return self.class_or_relation(db)
            }
            t::NAMESPACE => return self.namespace_statement(db),
            t::CLASS => {
                self.class_statement(db)?;
                return Ok(());
            }
            t::MEMBER => {
                self.i += 1;
                return Ok(());
            }
            t::ANNOTATION_START => {
                self.i += 1;
                let annotation = self.alpha_num()?;
                self.expect(t::ANNOTATION_END)?;
                let name = self.name(true)?;
                db.add_annotation(&name, &annotation);
                return Ok(());
            }
            t::CALLBACK => {
                self.i += 1;
                let id = self.name(true)?;
                self.expect(t::STR)?;
                db.set_click_event(&id);
                self.tooltip(db, &id);
                return Ok(());
            }
            t::LINK => {
                self.i += 1;
                let id = self.name(true)?;
                return self.link_statement(db, &id);
            }
            t::CLICK => {
                self.i += 1;
                let id = self.name(true)?;
                if self.cur() == t::HREF {
                    self.i += 1;
                    return self.link_statement(db, &id);
                }
                self.expect(t::CALLBACK_NAME)?;
                if self.cur() == t::CALLBACK_ARGS {
                    self.i += 1;
                }
                db.set_click_event(&id);
                self.tooltip(db, &id);
                return Ok(());
            }
            t::STYLE => {
                self.i += 1;
                let id = self.expect(t::ALPHA)?;
                let styles = self.styles()?;
                db.set_css_style(&id, &styles);
                return Ok(());
            }
            t::CSSCLASS => {
                self.i += 1;
                let ids = self.expect(t::STR)?;
                let class = self.expect(t::ALPHA)?;
                db.set_css_class(&ids, &class);
                return Ok(());
            }
            t::NOTE | t::NOTE_FOR => {
                self.note_statement(db)?;
                return Ok(());
            }
            t::CLASSDEF => {
                self.i += 1;
                let mut ids = alloc::vec![self.expect(t::ALPHA)?];
                while self.cur() == t::COMMA {
                    self.i += 1;
                    ids.push(self.expect(t::ALPHA)?);
                }
                let styles = self.styles()?;
                db.define_class(&ids, &styles);
                return Ok(());
            }
            t::direction_tb | t::direction_bt | t::direction_rl | t::direction_lr => {
                self.i += 1;
                db.set_direction(match k {
                    t::direction_tb => "TB",
                    t::direction_bt => "BT",
                    t::direction_rl => "RL",
                    _ => "LR",
                });
                return Ok(());
            }
            t::acc_title => {
                self.i += 1;
                let v = self.expect(t::acc_title_value)?;
                db.set_acc_title(trim(&v));
                return Ok(());
            }
            t::acc_descr => {
                self.i += 1;
                let v = self.expect(t::acc_descr_value)?;
                db.set_acc_description(trim(&v));
                return Ok(());
            }
            t::acc_descr_multiline_value => {
                let v = self.next_text();
                db.set_acc_description(trim(&v));
                return Ok(());
            }
            _ => {}
        }
        Err(self.fail(
            "'acc_title', 'acc_descr', 'acc_descr_multiline_value', 'NAMESPACE', 'ANNOTATION_START', 'CLASS', 'MEMBER', 'NOTE_FOR', 'NOTE', 'CLASSDEF', 'direction_tb', 'CALLBACK', 'LINK', 'CLICK', 'STYLE', 'CSSCLASS', 'ALPHA', 'MINUS', 'BQUOTE_STR'",
        ))
    }
}

/// Ports `parseClassDiagram`.
pub fn parse_class_diagram(src: &str, db: &mut ClassDb) -> Result<(), PeleError> {
    let mut p = Parser {
        src,
        tokens: tokenize(src),
        i: 0,
    };
    // Mermaid's grammar also accepts statements with no `classDiagram` line before them.
    let header = p.cur() == t::CLASS_DIAGRAM;
    if header {
        p.i += 1;
        p.expect(t::NEWLINE)?;
    }
    loop {
        p.statement(db)?;
        if p.cur() != t::NEWLINE {
            break;
        }
        p.i += 1;
        if p.cur() == t::EOF || p.cur() == t::END {
            break;
        }
    }
    if header {
        p.expect(t::EOF)?;
    }
    p.expect(t::END)?;
    Ok(())
}
