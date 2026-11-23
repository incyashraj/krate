//! Ports diagrams/er/parser.ts: makes the calls on the db that Mermaid's
//! ER grammar makes, and reports errors where its generated parser would.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::db::ErDb;
use super::lexer::{tokenize, Tokens};
use super::tokens as t;
use super::types::{DocItem, ParsedAttribute, RelSpec};
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js::trim;

/// Ports `CARDINALITY`.
fn cardinality(kind: u8) -> Option<&'static str> {
    match kind {
        t::ZERO_OR_ONE => Some("ZERO_OR_ONE"),
        t::ZERO_OR_MORE => Some("ZERO_OR_MORE"),
        t::ONE_OR_MORE => Some("ONE_OR_MORE"),
        t::ONLY_ONE => Some("ONLY_ONE"),
        t::MD_PARENT => Some("MD_PARENT"),
        _ => None,
    }
}

const DIRECTIONS: [&str; 4] = ["TB", "BT", "RL", "LR"];

const NAME: &str = "'UNICODE_TEXT', 'NUM', 'ENTITY_NAME', 'DECIMAL_NUM', 'ENTITY_ONE'";
const CARDS: &str = "'ZERO_OR_ONE', 'ZERO_OR_MORE', 'ONE_OR_MORE', 'ONLY_ONE', 'MD_PARENT'";
const LINE: &str = concat!(
    "'EOF', 'NEWLINE', 'title', 'acc_title', 'acc_descr', 'acc_descr_multiline_value', 'SUBGRAPH', 'direction_tb', ",
    "'direction_bt', 'direction_rl', 'direction_lr', 'CLASSDEF', 'CLASS', 'STYLE', ",
    "'UNICODE_TEXT', 'NUM', 'ENTITY_NAME', 'DECIMAL_NUM', 'ENTITY_ONE'"
);

/// A token type past the end of the list: no type at all.
const NONE: u8 = u8::MAX;

/// Ports `interface Frame`: what Mermaid's grammar hands to addSubGraph,
/// the value of every statement inside.
struct Frame {
    id: String,
    title: String,
    list: Vec<DocItem>,
}

struct Parser<'a> {
    src: &'a str,
    tokens: Tokens,
    i: usize,
}

type R<T> = Result<T, PeleError>;

/// `text.slice(1, -1)`.
fn inner(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 2 {
        return String::new();
    }
    chars[1..chars.len() - 1].iter().collect()
}

/// Ports `isName`.
fn is_name(kind: u8) -> bool {
    matches!(
        kind,
        t::UNICODE_TEXT | t::ENTITY_NAME | t::NUM | t::DECIMAL_NUM | t::ENTITY_ONE
    )
}

/// Ports `isStyleComponent`.
fn is_style_component(kind: u8) -> bool {
    matches!(kind, t::STYLE_TEXT | t::NUM | t::COLON | t::BRKT)
}

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
            return syntax_error("er", self.src, at, "", true);
        }
        let got = if kind == t::CHAR {
            self.text(self.i)
        } else {
            String::from(
                t::TOKEN_NAMES
                    .get(kind as usize)
                    .copied()
                    .unwrap_or("undefined"),
            )
        };
        syntax_error(
            "er",
            self.src,
            at,
            &format!("Expecting {}, got '{}'", expected, got),
            false,
        )
    }

    fn expect(&mut self, kind: u8) -> R<String> {
        if self.cur() != kind {
            return Err(self.fail(&format!("'{}'", t::TOKEN_NAMES[kind as usize])));
        }
        Ok(self.next_text())
    }

    fn separator(&mut self) -> R<()> {
        let k = self.cur();
        if k != t::NEWLINE && k != t::SEMI && k != t::EOF {
            return Err(self.fail("'EOF', 'NEWLINE', 'SEMI'"));
        }
        self.i += 1;
        Ok(())
    }

    fn entity_name(&mut self) -> R<String> {
        let k = self.cur();
        if !is_name(k) {
            return Err(self.fail(NAME));
        }
        let text = self.next_text();
        Ok(if k == t::ENTITY_NAME {
            inner(&text)
        } else {
            text
        })
    }

    fn id_list(&mut self) -> R<Vec<String>> {
        let mut ids = Vec::new();
        loop {
            let k = self.cur();
            if k != t::UNICODE_TEXT && k != t::STYLE_TEXT {
                return Err(self.fail("'UNICODE_TEXT', 'STYLE_TEXT'"));
            }
            ids.push(self.next_text());
            if self.cur() != t::COMMA {
                return Ok(ids);
            }
            self.i += 1;
        }
    }

    fn styles_opt(&mut self) -> R<Vec<String>> {
        let mut styles = Vec::new();
        loop {
            if !is_style_component(self.cur()) {
                return Err(self.fail("'STYLE_TEXT', 'COLON', 'NUM', 'BRKT'"));
            }
            let mut style = self.next_text();
            while is_style_component(self.cur()) {
                style.push_str(&self.next_text());
            }
            styles.push(style);
            if self.cur() != t::COMMA {
                return Ok(styles);
            }
            self.i += 1;
        }
    }

    fn rel_spec(&mut self) -> R<RelSpec> {
        let card_b = cardinality(self.cur()).unwrap_or("");
        self.i += 1;
        let r = self.cur();
        if r != t::IDENTIFYING && r != t::NON_IDENTIFYING {
            return Err(self.fail("'NON_IDENTIFYING', 'IDENTIFYING'"));
        }
        let rel_type = if r == t::IDENTIFYING {
            "IDENTIFYING"
        } else {
            "NON_IDENTIFYING"
        };
        self.i += 1;
        let Some(card_a) = cardinality(self.cur()) else {
            return Err(self.fail(CARDS));
        };
        self.i += 1;
        Ok(RelSpec {
            card_a,
            rel_type,
            card_b,
        })
    }

    fn role(&mut self) -> R<String> {
        let k = self.cur();
        if k != t::WORD && k != t::ENTITY_NAME && k != t::UNICODE_TEXT {
            return Err(self.fail("'UNICODE_TEXT', 'ENTITY_NAME', 'WORD'"));
        }
        let text = self.next_text();
        Ok(if k == t::UNICODE_TEXT {
            text
        } else {
            inner(&text)
        })
    }

    fn is_char(&self, ch: &str) -> bool {
        self.cur() == t::CHAR && self.text(self.i) == ch
    }

    /// Returns the attributes last to first, as Mermaid's right-recursive
    /// rule builds them.
    fn attributes(&mut self) -> R<Vec<ParsedAttribute>> {
        let mut list = Vec::new();
        loop {
            let mut kind = self.expect(t::ATTRIBUTE_WORD)?;
            if self.is_char("?") {
                kind.push_str(&self.next_text());
            }
            if self.cur() != t::ATTRIBUTE_WORD {
                return Err(self.fail(if kind.ends_with('?') {
                    "'ATTRIBUTE_WORD'"
                } else {
                    "'ATTRIBUTE_WORD', '?'"
                }));
            }
            let name = self.next_text();
            let mut attribute = ParsedAttribute {
                kind,
                name,
                keys: None,
                comment: None,
            };
            if self.cur() == t::ATTRIBUTE_KEY {
                let mut keys = alloc::vec![self.next_text()];
                while self.is_char(",") {
                    self.i += 1;
                    keys.push(self.expect(t::ATTRIBUTE_KEY)?);
                }
                attribute.keys = Some(keys);
            }
            if self.cur() == t::COMMENT {
                attribute.comment = Some(inner(&self.next_text()));
            }
            list.push(attribute);
            if self.cur() == t::BLOCK_STOP {
                break;
            }
        }
        list.reverse();
        Ok(list)
    }

    /// Parses an optional `{ ... }` block; None when there is none or it is empty.
    fn block(&mut self) -> R<Option<Vec<ParsedAttribute>>> {
        if self.cur() != t::BLOCK_START {
            return Ok(None);
        }
        self.i += 1;
        if self.cur() == t::BLOCK_STOP {
            self.i += 1;
            return Ok(None);
        }
        let list = self.attributes()?;
        self.i += 1;
        Ok(Some(list))
    }

    fn entity_statement(&mut self, db: &mut ErDb) -> R<Vec<String>> {
        let a = self.entity_name()?;
        let mut k = self.cur();
        if k == t::SQS {
            self.i += 1;
            let alias = self.entity_name()?;
            self.expect(t::SQE)?;
            let mut classes: Option<Vec<String>> = None;
            if self.cur() == t::STYLE_SEPARATOR {
                self.i += 1;
                classes = Some(self.id_list()?);
            }
            let attrs = self.block()?;
            db.add_entity(&a, &alias);
            if let Some(attrs) = attrs {
                db.add_attributes(&a, attrs);
            }
            if let Some(classes) = classes {
                db.set_class(core::slice::from_ref(&a), &classes);
            }
            return Ok(alloc::vec![a]);
        }
        let mut classes: Option<Vec<String>> = None;
        if k == t::STYLE_SEPARATOR {
            self.i += 1;
            classes = Some(self.id_list()?);
            k = self.cur();
        }
        if cardinality(k).is_some() {
            let spec = self.rel_spec()?;
            let b = self.entity_name()?;
            let mut classes_b: Option<Vec<String>> = None;
            if self.cur() == t::STYLE_SEPARATOR {
                self.i += 1;
                classes_b = Some(self.id_list()?);
            }
            self.expect(t::COLON)?;
            let label = self.role()?;
            db.add_entity(&a, "");
            db.add_entity(&b, "");
            db.add_relationship(&a, &label, &b, spec);
            if let Some(classes) = classes {
                db.set_class(core::slice::from_ref(&a), &classes);
            }
            if let Some(classes) = classes_b {
                db.set_class(core::slice::from_ref(&b), &classes);
            }
            return Ok(alloc::vec![a, b]);
        }
        let attrs = self.block()?;
        db.add_entity(&a, "");
        if let Some(attrs) = attrs {
            db.add_attributes(&a, attrs);
        }
        if let Some(classes) = classes {
            db.set_class(core::slice::from_ref(&a), &classes);
        }
        Ok(alloc::vec![a])
    }
}

/// Ports `parseEr`.
pub fn parse_er(src: &str, db: &mut ErDb) -> Result<(), PeleError> {
    let mut p = Parser {
        src,
        tokens: tokenize(src),
        i: 0,
    };
    p.expect(t::ER_DIAGRAM)?;
    // Subgraphs nest on this stack rather than on the call stack; the first frame is the diagram itself.
    let mut frames: Vec<Frame> = alloc::vec![Frame {
        id: String::new(),
        title: String::new(),
        list: Vec::new(),
    }];
    let mut nested = false;

    loop {
        let k = p.cur();
        match k {
            t::NEWLINE => p.i += 1,
            t::EOF => {
                p.i += 1;
                if nested {
                    return Err(p.fail(&format!("'END', {}", LINE)));
                }
                return Ok(());
            }
            t::END_KW => {
                if !nested {
                    return Err(p.fail(LINE));
                }
                p.i += 1;
                let frame = frames.pop().unwrap();
                let id = db.add_sub_graph(&frame.id, &frame.list, &frame.title);
                nested = frames.len() > 1;
                if nested {
                    frames.last_mut().unwrap().list.push(DocItem::Name(id));
                }
            }
            t::SUBGRAPH => {
                p.i += 1;
                let id = p.entity_name()?;
                let mut title = id.clone();
                if p.cur() == t::SQS {
                    p.i += 1;
                    title = p.entity_name()?;
                    while is_name(p.cur()) {
                        title.push(' ');
                        title.push_str(&p.entity_name()?);
                    }
                    p.expect(t::SQE)?;
                }
                p.separator()?;
                frames.push(Frame {
                    id,
                    title,
                    list: Vec::new(),
                });
                nested = true;
            }
            t::acc_title => {
                p.i += 1;
                let value: String = trim(&p.expect(t::acc_title_value)?).into();
                db.set_acc_title(&value);
                if nested {
                    frames.last_mut().unwrap().list.push(DocItem::Name(value));
                }
            }
            t::acc_descr => {
                p.i += 1;
                let value: String = trim(&p.expect(t::acc_descr_value)?).into();
                db.set_acc_description(&value);
                if nested {
                    frames.last_mut().unwrap().list.push(DocItem::Name(value));
                }
            }
            t::acc_descr_multiline_value => {
                let value: String = trim(&p.next_text()).into();
                db.set_acc_description(&value);
                if nested {
                    frames.last_mut().unwrap().list.push(DocItem::Name(value));
                }
            }
            t::direction_tb | t::direction_bt | t::direction_rl | t::direction_lr => {
                p.i += 1;
                let value = DIRECTIONS[(k - t::direction_tb) as usize];
                if nested {
                    frames
                        .last_mut()
                        .unwrap()
                        .list
                        .push(DocItem::Dir(value.into()));
                } else {
                    db.set_direction(value);
                }
            }
            t::CLASSDEF => {
                let keyword = p.next_text();
                let ids = p.id_list()?;
                let styles = p.styles_opt()?;
                p.separator()?;
                db.add_class(&ids, &styles);
                if nested {
                    frames.last_mut().unwrap().list.push(DocItem::Name(keyword));
                }
            }
            t::CLASS => {
                let keyword = p.next_text();
                let ids = p.id_list()?;
                let names = p.id_list()?;
                db.set_class(&ids, &names);
                if nested {
                    frames.last_mut().unwrap().list.push(DocItem::Name(keyword));
                }
            }
            t::STYLE => {
                let keyword = p.next_text();
                let ids = p.id_list()?;
                let styles = p.styles_opt()?;
                p.separator()?;
                db.add_css_styles(&ids, &styles);
                if nested {
                    frames.last_mut().unwrap().list.push(DocItem::Name(keyword));
                }
            }
            _ => {
                if !is_name(k) {
                    return Err(p.fail(&if nested {
                        format!("'END', {}", LINE)
                    } else {
                        String::from(LINE)
                    }));
                }
                let names = p.entity_statement(db)?;
                if nested {
                    let frame = frames.last_mut().unwrap();
                    for name in names {
                        frame.list.push(DocItem::Name(name));
                    }
                }
            }
        }
    }
}
