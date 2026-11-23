//! Ports diagrams/block/parser.ts: builds the statements Mermaid's block
//! grammar builds, and reports errors where its generated parser would.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::{
    edge_str_to_edge_data, edge_str_to_edge_start_data, edge_str_to_pattern, edge_str_to_thickness,
    type_str2_type, BlockDb,
};
use super::lexer::{tokenize, Tokens};
use super::tokens as t;
use super::types::{Block, Statement};
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js::{parse_int, trim};

/// Ports `interface NodeInfo`.
struct NodeInfo {
    id: String,
    label: Option<String>,
    type_str: Option<String>,
    directions: Option<Vec<String>>,
}

/// Ports `interface Frame`.
struct Frame {
    list: Vec<Statement>,
    header: Option<Statement>,
}

const STATEMENT_START: &str =
    "'NODE_ID', 'COLUMNS', 'SPACE_BLOCK', 'BLOCK_DIAGRAM_KEY', 'id-block', 'classDef', 'class', 'style'";

/// A token type past the end of the list: no type at all.
const NONE: u8 = u8::MAX;

/// Ports `closeDocument`: the grammar inlines a document's last statement
/// when it is a list.
fn close_document(mut list: Vec<Statement>) -> Vec<Statement> {
    if let Some(Statement::Chain(_)) = list.last() {
        if let Some(Statement::Chain(last)) = list.pop() {
            list.extend(last.into_iter().map(Statement::One));
        }
    }
    list
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
            return syntax_error("block", self.src, at, "", true);
        }
        let name = t::TOKEN_NAMES
            .get(kind as usize)
            .copied()
            .unwrap_or("undefined");
        syntax_error(
            "block",
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

    /// Ports `node`.
    fn node(&mut self) -> R<NodeInfo> {
        let id = self.expect(t::NODE_ID)?;
        if self.cur() == t::NODE_DSTART {
            let open = self.next_text();
            let label = self.expect(t::STR)?;
            let close = self.expect(t::NODE_DEND)?;
            return Ok(NodeInfo {
                id,
                label: Some(label),
                type_str: Some(open + &close),
                directions: None,
            });
        }
        if self.cur() == t::BLOCK_ARROW_START {
            let open = self.next_text();
            let label = self.expect(t::STR)?;
            let mut directions = alloc::vec![self.expect(t::DIR)?];
            while self.cur() == t::DIR {
                directions.push(self.next_text());
            }
            let close = self.expect(t::BLOCK_ARROW_END)?;
            return Ok(NodeInfo {
                id,
                label: Some(label),
                type_str: Some(open + &close),
                directions: Some(directions),
            });
        }
        Ok(NodeInfo {
            id,
            label: None,
            type_str: None,
            directions: None,
        })
    }

    /// Ports `nodeStatement`.
    fn node_statement(&mut self, db: &mut BlockDb) -> R<Statement> {
        let first = self.node()?;
        let size = if self.cur() == t::SIZE {
            parse_int(&self.next_text(), 10)
        } else {
            1.0
        };
        let mut from = Block {
            id: Some(first.id),
            label: first.label,
            kind: Some(type_str2_type(first.type_str.as_deref()).into()),
            directions: first.directions,
            width_in_columns: Some(size),
            ..Block::default()
        };
        let mut chain: Option<Vec<Block>> = None;
        while self.cur() == t::LINK || self.cur() == t::START_LINK {
            let mut label = String::new();
            let k = self.cur();
            self.i += 1;
            if k == t::START_LINK {
                self.expect(t::LINK_LABEL)?;
                label = self.expect(t::STR)?;
                self.expect(t::LINK)?;
            }
            let link = self.text(self.i - 1);
            let to = self.node()?;
            let arrow_type_end = edge_str_to_edge_data(&link);
            let arrow_type_start = edge_str_to_edge_start_data(&link);
            let thickness = edge_str_to_thickness(&link);
            let pattern = edge_str_to_pattern(&link);
            // Mermaid keeps only the last link of `A --> B --> C` and loses its start. Here every link counts.
            let list = chain.get_or_insert_with(|| {
                alloc::vec![Block {
                    id: from.id.clone(),
                    label: from.label.clone(),
                    kind: from.kind.clone(),
                    directions: from.directions.clone(),
                    ..Block::default()
                }]
            });
            from = Block {
                id: Some(to.id.clone()),
                label: to.label,
                kind: Some(type_str2_type(to.type_str.as_deref()).into()),
                directions: to.directions.clone(),
                ..Block::default()
            };
            let prev = list.last().and_then(|b| b.id.clone()).unwrap_or_default();
            list.push(Block {
                id: Some(format!("{}-{}", prev, to.id)),
                start: Some(prev),
                end: Some(to.id),
                label: Some(label),
                kind: Some("edge".into()),
                thickness: Some(thickness.into()),
                pattern: Some(pattern.into()),
                directions: to.directions,
                arrow_type_end: Some(arrow_type_end.into()),
                arrow_type_start: Some(arrow_type_start.into()),
                ..Block::default()
            });
            list.push(from.clone());
        }
        Ok(match chain {
            Some(list) => Statement::Chain(list.into_iter().map(|b| db.add(b)).collect()),
            None => Statement::One(db.add(from)),
        })
    }
}

/// Ports `parseBlock`.
pub fn parse_block(src: &str, db: &mut BlockDb) -> Result<(), PeleError> {
    let mut p = Parser {
        src,
        tokens: tokenize(src),
        i: 0,
    };
    p.expect(t::KEY)?;
    // Composites nest without recursion: each open one keeps the statement list of its parent.
    let mut open: Vec<Frame> = Vec::new();
    let mut list: Vec<Statement> = Vec::new();
    loop {
        match p.cur() {
            t::NODE_ID => {
                let s = p.node_statement(db)?;
                list.push(s);
            }
            t::COLUMNS => {
                let columns = parse_int(&p.next_text(), 10);
                let k = db.add(Block {
                    kind: Some("column-setting".into()),
                    columns: Some(columns),
                    ..Block::default()
                });
                list.push(Statement::One(k));
            }
            t::SPACE_BLOCK => {
                let width = parse_int(&p.next_text(), 10);
                let id = db.generate_id();
                let k = db.add(Block {
                    id: Some(id),
                    kind: Some("space".into()),
                    label: Some(String::new()),
                    width: Some(width),
                    stmts: Some(Vec::new()),
                    ..Block::default()
                });
                list.push(Statement::One(k));
            }
            t::KEY => {
                p.i += 1;
                open.push(Frame {
                    list: core::mem::take(&mut list),
                    header: None,
                });
            }
            t::ID_BLOCK => {
                p.i += 1;
                if p.cur() != t::NODE_ID {
                    return Err(p.fail("'NODE_ID'"));
                }
                let header = p.node_statement(db)?;
                open.push(Frame {
                    list: core::mem::take(&mut list),
                    header: Some(header),
                });
            }
            t::end => {
                let Some(frame) = open.pop().filter(|_| !list.is_empty()) else {
                    return Err(p.fail(STATEMENT_START));
                };
                p.i += 1;
                let id = db.generate_id();
                let children = close_document(core::mem::take(&mut list));
                list = frame.list;
                let block = match frame.header {
                    None => Block {
                        id: Some(id),
                        kind: Some("composite".into()),
                        label: Some(String::new()),
                        stmts: Some(children),
                        ..Block::default()
                    },
                    Some(Statement::One(h)) => Block {
                        kind: Some("composite".into()),
                        stmts: Some(children),
                        ..db.arena[h].clone()
                    },
                    // A list spreads into no fields a block has.
                    Some(Statement::Chain(_)) => Block {
                        kind: Some("composite".into()),
                        stmts: Some(children),
                        ..Block::default()
                    },
                };
                let k = db.add(block);
                list.push(Statement::One(k));
            }
            t::CLASSDEF => {
                p.i += 1;
                let id: String = trim(&p.expect(t::CLASSDEF_ID)?).into();
                let css: String = trim(&p.expect(t::CLASSDEF_STYLEOPTS)?).into();
                let k = db.add(Block {
                    kind: Some("classDef".into()),
                    id: Some(id),
                    css: Some(css),
                    ..Block::default()
                });
                list.push(Statement::One(k));
            }
            t::CLASS => {
                p.i += 1;
                let id: String = trim(&p.expect(t::CLASSENTITY_IDS)?).into();
                let class: String = trim(&p.expect(t::STYLECLASS)?).into();
                let k = db.add(Block {
                    kind: Some("applyClass".into()),
                    id: Some(id),
                    style_class: Some(class),
                    ..Block::default()
                });
                list.push(Statement::One(k));
            }
            t::STYLE => {
                p.i += 1;
                let id: String = trim(&p.expect(t::STYLE_ENTITY_IDS)?).into();
                let styles: String = trim(&p.expect(t::STYLE_DEFINITION_DATA)?).into();
                let k = db.add(Block {
                    kind: Some("applyStyles".into()),
                    id: Some(id),
                    styles_str: Some(styles),
                    ..Block::default()
                });
                list.push(Statement::One(k));
            }
            t::EOF => {
                if !open.is_empty() {
                    return Err(p.fail(&format!("{}, 'end'", STATEMENT_START)));
                }
                if list.is_empty() {
                    return Err(p.fail(STATEMENT_START));
                }
                return db.set_hierarchy(close_document(list));
            }
            _ => {
                return Err(p.fail(&if list.is_empty() {
                    String::from(STATEMENT_START)
                } else if !open.is_empty() {
                    format!("{}, 'end'", STATEMENT_START)
                } else {
                    format!("{}, 'EOF'", STATEMENT_START)
                }));
            }
        }
    }
}
