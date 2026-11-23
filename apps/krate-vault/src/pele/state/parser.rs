//! Ports diagrams/state/parser.ts: builds the documents Mermaid's state
//! grammar builds, and reports errors where its generated parser would.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::StateDb;
use super::lexer::{tokenize, Tokens};
use super::tokens as t;
use super::types::{Description, StateNote, StateStmt, Stmt};
use crate::pele::errors::{locate, syntax_error, PeleError, PeleErrorCode};
use crate::pele::js::trim;

const DIRECTIONS: [&str; 4] = ["TB", "BT", "RL", "LR"];
const STATEMENT: &str = concat!(
    "'NL', 'ID', 'HIDE_EMPTY', 'scale', 'COMPOSIT_STATE', 'STATE_DESCR', 'FORK', 'JOIN', 'CHOICE', 'CONCURRENT', 'note', ",
    "'acc_title', 'acc_descr', 'acc_descr_multiline_value', 'CLICK', 'classDef', 'style', 'class', 'direction_tb', ",
    "'direction_bt', 'direction_rl', 'direction_lr', 'EDGE_STATE'"
);

/// A token type past the end of the list: no type at all.
const NONE: u8 = u8::MAX;

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
            if let Some(error) = &self.tokens.error {
                let (line, column, snippet) = locate(self.src, at, None);
                let mut e = PeleError::of(error.clone(), PeleErrorCode::Syntax, "state");
                e.line = line as u32;
                e.column = column as u32;
                e.snippet = snippet;
                return e;
            }
            return syntax_error("state", self.src, at, "", true);
        }
        let name = t::TOKEN_NAMES
            .get(kind as usize)
            .copied()
            .unwrap_or("undefined");
        syntax_error(
            "state",
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

    fn id_statement(&mut self) -> R<StateStmt> {
        let k = self.cur();
        if k != t::ID && k != t::EDGE_STATE {
            return Err(self.fail("'ID', 'EDGE_STATE'"));
        }
        let id: String = trim(&self.next_text()).into();
        let mut stmt = StateStmt {
            id,
            kind: Some("default".into()),
            description: Some(Description::One(String::new())),
            ..StateStmt::default()
        };
        if self.cur() != t::STYLE_SEPARATOR {
            return Ok(stmt);
        }
        self.i += 1;
        let class = self.expect(t::ID)?;
        stmt.classes = Some(alloc::vec![trim(&class).into()]);
        Ok(stmt)
    }
}

/// The grammar drops a statement whose value is the string it uses for a
/// blank line (`add`).
fn add(doc: &mut Vec<Stmt>, value: Stmt) {
    if value != Stmt::Text("nl".into()) {
        doc.push(value);
    }
}

/// Ports `parseState`: the root document, also handed to `db.setRootDoc`.
pub fn parse_state(src: &str, db: &mut StateDb) -> Result<(), PeleError> {
    let mut p = Parser {
        src,
        tokens: tokenize(src),
        i: 0,
    };

    while p.cur() == t::NL {
        p.i += 1;
    }
    if p.cur() != t::SD {
        return Err(p.fail("'NL', 'SD'"));
    }
    p.i += 1;

    let mut doc: Vec<Stmt> = Vec::new();
    // The documents around the one being read, each with the composite that holds it.
    let mut outer: Vec<(Vec<Stmt>, StateStmt)> = Vec::new();
    let open = |p: &mut Parser,
                doc: &mut Vec<Stmt>,
                outer: &mut Vec<(Vec<Stmt>, StateStmt)>,
                id: String,
                description: String| {
        p.i += 1;
        let stmt = StateStmt {
            id,
            kind: Some("default".into()),
            description: Some(Description::One(description)),
            ..StateStmt::default()
        };
        outer.push((core::mem::take(doc), stmt));
    };

    loop {
        let k = p.cur();
        match k {
            t::NL => p.i += 1,
            t::ID | t::EDGE_STATE => {
                let mut first = p.id_statement()?;
                if p.cur() == t::DESCR {
                    let text = p.next_text();
                    first.description = Some(Description::One(db.trim_colon(&text)));
                    add(&mut doc, Stmt::State(first));
                } else if p.cur() == t::ARROW {
                    p.i += 1;
                    let second = p.id_statement()?;
                    let description = if p.cur() == t::DESCR {
                        let text = p.next_text();
                        Some(db.trim_colon(&text))
                    } else {
                        None
                    };
                    add(
                        &mut doc,
                        Stmt::Relation {
                            state1: first,
                            state2: second,
                            description,
                        },
                    );
                } else {
                    add(&mut doc, Stmt::State(first));
                }
            }
            t::HIDE_EMPTY => {
                let text = p.next_text();
                add(&mut doc, Stmt::Text(text));
            }
            t::scale => {
                p.i += 1;
                p.expect(t::WIDTH)?;
                let text = p.text(p.i - 2);
                add(&mut doc, Stmt::Text(text));
            }
            t::COMPOSIT_STATE => {
                if p.ty(p.i + 1) == t::STRUCT_START {
                    let id = p.next_text();
                    open(&mut p, &mut doc, &mut outer, id, String::new());
                } else {
                    let text = p.next_text();
                    add(&mut doc, Stmt::Text(text));
                }
            }
            t::STATE_DESCR => {
                let description = p.next_text();
                p.expect(t::AS)?;
                let id = p.expect(t::ID)?;
                if p.cur() == t::STRUCT_START {
                    open(&mut p, &mut doc, &mut outer, id, description);
                } else if id.contains(':') {
                    let parts: Vec<&str> = id.split(':').collect();
                    add(
                        &mut doc,
                        Stmt::State(StateStmt {
                            id: parts[0].into(),
                            kind: Some("default".into()),
                            description: Some(Description::Many(alloc::vec![
                                trim(&description).into(),
                                parts[1].into()
                            ])),
                            ..StateStmt::default()
                        }),
                    );
                } else {
                    add(
                        &mut doc,
                        Stmt::State(StateStmt {
                            id,
                            kind: Some("default".into()),
                            description: Some(Description::One(trim(&description).into())),
                            ..StateStmt::default()
                        }),
                    );
                }
            }
            t::FORK | t::JOIN | t::CHOICE => {
                let id = p.next_text();
                let kind = match k {
                    t::FORK => "fork",
                    t::JOIN => "join",
                    _ => "choice",
                };
                add(
                    &mut doc,
                    Stmt::State(StateStmt {
                        id,
                        kind: Some(kind.into()),
                        ..StateStmt::default()
                    }),
                );
            }
            t::CONCURRENT => {
                p.i += 1;
                let id = db.get_divider_id();
                add(
                    &mut doc,
                    Stmt::State(StateStmt {
                        id,
                        kind: Some("divider".into()),
                        ..StateStmt::default()
                    }),
                );
            }
            t::note => {
                let keyword = p.next_text();
                if p.cur() == t::left_of || p.cur() == t::right_of {
                    let position: String = trim(&p.next_text()).into();
                    let id: String = trim(&p.expect(t::ID)?).into();
                    let text: String = trim(&p.expect(t::NOTE_TEXT)?).into();
                    add(
                        &mut doc,
                        Stmt::State(StateStmt {
                            id,
                            note: Some(StateNote {
                                position: Some(position),
                                text,
                            }),
                            ..StateStmt::default()
                        }),
                    );
                } else {
                    if p.cur() != t::NOTE_TEXT {
                        return Err(p.fail("'NOTE_TEXT', 'left_of', 'right_of'"));
                    }
                    p.i += 1;
                    p.expect(t::AS)?;
                    p.expect(t::ID)?;
                    add(&mut doc, Stmt::Text(keyword));
                }
            }
            t::direction_tb | t::direction_bt | t::direction_rl | t::direction_lr => {
                let value = DIRECTIONS[(k - t::direction_tb) as usize];
                p.i += 1;
                db.set_direction(value);
                add(
                    &mut doc,
                    Stmt::Dir {
                        value: value.into(),
                    },
                );
            }
            t::acc_title => {
                p.i += 1;
                let value: String = trim(&p.expect(t::acc_title_value)?).into();
                db.set_acc_title(&value);
                add(&mut doc, Stmt::Text(value));
            }
            t::acc_descr => {
                p.i += 1;
                let value: String = trim(&p.expect(t::acc_descr_value)?).into();
                db.set_acc_description(&value);
                add(&mut doc, Stmt::Text(value));
            }
            t::acc_descr_multiline_value => {
                let value: String = trim(&p.next_text()).into();
                db.set_acc_description(&value);
                add(&mut doc, Stmt::Text(value));
            }
            t::CLICK => {
                p.i += 1;
                let id = p.id_statement()?;
                let url: String;
                let mut tooltip = String::new();
                if p.cur() == t::HREF {
                    p.i += 1;
                    url = p.expect(t::STRING)?;
                } else {
                    if p.cur() != t::STRING {
                        return Err(p.fail("'STRING', 'HREF'"));
                    }
                    url = p.next_text();
                    tooltip = p.expect(t::STRING)?;
                }
                p.expect(t::NL)?;
                add(&mut doc, Stmt::Click { id, url, tooltip });
            }
            t::classDef => {
                p.i += 1;
                let id: String = trim(&p.expect(t::CLASSDEF_ID)?).into();
                let classes: String = trim(&p.expect(t::CLASSDEF_STYLEOPTS)?).into();
                add(&mut doc, Stmt::ClassDef { id, classes });
            }
            t::style => {
                p.i += 1;
                let id: String = trim(&p.expect(t::STYLE_IDS)?).into();
                let style_class: String = trim(&p.expect(t::STYLEDEF_STYLEOPTS)?).into();
                add(&mut doc, Stmt::Style { id, style_class });
            }
            t::class => {
                p.i += 1;
                let id: String = trim(&p.expect(t::CLASSENTITY_IDS)?).into();
                let style_class: String = trim(&p.expect(t::STYLECLASS)?).into();
                add(&mut doc, Stmt::ApplyClass { id, style_class });
            }
            t::STRUCT_STOP => {
                if outer.is_empty() {
                    return Err(p.fail(&format!("'$end', {}", STATEMENT)));
                }
                p.i += 1;
                let (mut parent, mut stmt) = outer.pop().unwrap();
                stmt.doc = Some(core::mem::take(&mut doc));
                parent.push(Stmt::State(stmt));
                doc = parent;
            }
            t::END => {
                if !outer.is_empty() {
                    return Err(p.fail(&format!("'STRUCT_STOP', {}", STATEMENT)));
                }
                return db.set_root_doc(doc);
            }
            _ => {
                return Err(p.fail(&if !outer.is_empty() {
                    format!("'STRUCT_STOP', {}", STATEMENT)
                } else {
                    format!("'$end', {}", STATEMENT)
                }));
            }
        }
    }
}
