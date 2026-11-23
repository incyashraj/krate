//! Ports diagrams/common/outline.ts: the indented outline grammar that
//! Mermaid's mindmap and kanban diagrams share. Kanban adds `@{ ... }`
//! metadata after a node and keeps `@` out of node ids.
//!
//! The tokenizer walks a [`Text`] by character; `starts` are byte offsets
//! in the source, as errors locate them.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::chars::{is_line_end, is_space, is_word, Text};
use crate::pele::errors::{syntax_error, PeleError};
use crate::pele::js::len16;

/// Ports `const enum T`.
pub mod t {
    pub const END: u32 = 0;
    pub const ERROR: u32 = 1;
    pub const SPACELINE: u32 = 2;
    pub const NL: u32 = 3;
    pub const EOF: u32 = 4;
    pub const KEYWORD: u32 = 5;
    pub const SPACELIST: u32 = 6;
    pub const ICON: u32 = 7;
    pub const CLASS: u32 = 8;
    pub const NODE_DSTART: u32 = 9;
    pub const NODE_ID: u32 = 10;
    pub const NODE_DESCR: u32 = 11;
    pub const NODE_DEND: u32 = 12;
    pub const SHAPE_DATA: u32 = 13;
}

/// As the generated parser prints them. It has no name for the end of
/// input and prints its number (`NAMES`).
const NAMES: [&str; 14] = [
    "1",
    "INVALID",
    "SPACELINE",
    "NL",
    "EOF",
    "",
    "SPACELIST",
    "ICON",
    "CLASS",
    "NODE_DSTART",
    "NODE_ID",
    "NODE_DESCR",
    "NODE_DEND",
    "SHAPE_DATA",
];

/// Ports `tokenName`.
pub fn token_name(kind: u32, kanban: bool) -> &'static str {
    if kind == t::KEYWORD {
        if kanban {
            "KANBAN"
        } else {
            "MINDMAP"
        }
    } else {
        NAMES[kind as usize]
    }
}

/// Ports `interface Tokens`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tokens {
    pub types: Vec<u32>,
    pub texts: Vec<String>,
    pub starts: Vec<usize>,
}

/// Ports `NODE_TYPE`.
pub mod node_type {
    pub const DEFAULT: u32 = 0;
    pub const NO_BORDER: u32 = 0;
    pub const ROUNDED_RECT: u32 = 1;
    pub const RECT: u32 = 2;
    pub const CIRCLE: u32 = 3;
    pub const CLOUD: u32 = 4;
    pub const BANG: u32 = 5;
    pub const HEXAGON: u32 = 6;
}

/// Ports `getType`: any opening delimiter pairs with any closing one, as
/// in Mermaid.
pub fn get_type(start: &str, end: &str) -> u32 {
    match start {
        "[" => node_type::RECT,
        "(" => {
            if end == ")" {
                node_type::ROUNDED_RECT
            } else {
                node_type::CLOUD
            }
        }
        "((" => node_type::CIRCLE,
        ")" => node_type::CLOUD,
        "))" => node_type::BANG,
        "{{" => node_type::HEXAGON,
        _ => node_type::DEFAULT,
    }
}

/// Ports `interface OutlineDb`.
pub trait OutlineDb {
    fn get_type(&self, start: &str, end: &str) -> u32;
    fn add_node(
        &mut self,
        level: usize,
        id: &str,
        descr: &str,
        kind: u32,
        shape_data: Option<&str>,
    ) -> Result<(), PeleError>;
    /// `decorateNode({ icon })` or `decorateNode({ class })`.
    fn decorate_node(&mut self, icon: Option<&str>, class: Option<&str>) -> Result<(), PeleError>;
}

// `const enum S`.
const INITIAL: u8 = 0;
const CLASS: u8 = 1;
const ICON: u8 = 2;
const NODE: u8 = 3;
const NSTR: u8 = 4;
const NSTR2: u8 = 5;
const DATA: u8 = 6;
const DATA_STR: u8 = 7;

/// Jison's case-insensitive flag folds ASCII letters only (`letters`).
fn letters(src: &Text, p: usize, word: &str) -> bool {
    for (k, w) in word.bytes().enumerate() {
        if (src.c(p + k) | 32) != w as u32 {
            return false;
        }
    }
    true
}

/// The regular expressions the tokenizer matches, sticky at a position.
#[derive(Clone, Copy)]
enum Re {
    /// `/\s+/y`
    Space,
    /// `/.+/y`
    Line,
    /// `/[^([\n){}]+/y`
    Id,
    /// `/[^([\n){}@]+/y`
    IdKanban,
    /// `/[^)\](}]+/y`
    Descr,
    /// `/[^`"]+/y`
    Md,
    /// `/[^}^"]+/y`
    Data,
}

fn in_class(re: Re, c: u32) -> bool {
    match re {
        Re::Space => is_space(c),
        Re::Line => !is_line_end(c),
        Re::Id => !matches!(c, 40 | 91 | 10 | 41 | 123 | 125),
        Re::IdKanban => !matches!(c, 40 | 91 | 10 | 41 | 123 | 125 | 64),
        Re::Descr => !matches!(c, 41 | 93 | 40 | 125),
        Re::Md => !matches!(c, 96 | 34),
        Re::Data => !matches!(c, 125 | 94 | 34),
    }
}

/// `re.lastIndex = from; re.test(src) ? re.lastIndex : -1`.
fn at(src: &Text, re: Re, from: usize) -> Option<usize> {
    let mut e = from;
    while e < src.len() && in_class(re, src.c(e)) {
        e += 1;
    }
    if e > from {
        Some(e)
    } else {
        None
    }
}

/// `text.replace(/\n\s*/g, '<br/>')`.
fn breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' {
            while chars.peek().is_some_and(|&c| crate::pele::js::is_space(c)) {
                chars.next();
            }
            out.push_str("<br/>");
        } else {
            out.push(c);
        }
    }
    out
}

/// Ports `tokenize`.
pub fn tokenize(source: &str, kanban: bool) -> Tokens {
    let src = Text::new(source);
    let n = src.len();
    let mut out = Tokens::default();
    let keyword = if kanban { "kanban" } else { "mindmap" };
    let re_id = if kanban { Re::IdKanban } else { Re::Id };
    let mut state = INITIAL;
    let mut p = 0usize;
    let mut next_break: isize = -1;

    let put = |out: &mut Tokens, p: &mut usize, kind: u32, text: String, end: usize| {
        out.types.push(kind);
        out.texts.push(text);
        out.starts.push(src.byte(*p));
        *p = end;
    };
    let up_to = |p: usize, ch: u32| src.find_char(ch, p).unwrap_or(n);

    macro_rules! emit {
        ($kind:expr, $end:expr, $next:expr) => {{
            let end = $end;
            let text = String::from(src.slice(p, end));
            put(&mut out, &mut p, $kind, text, end);
            state = $next;
        }};
    }

    while p < n {
        let c = src.c(p);
        let d = src.c(p + 1);
        if state == INITIAL {
            let w = if c <= 32 || c >= 160 {
                at(&src, Re::Space, p).unwrap_or(p).max(p)
            } else {
                p
            };
            if kanban && c == 64 && d == 123 {
                let e = p + 2;
                put(&mut out, &mut p, t::SHAPE_DATA, String::new(), e);
                state = DATA;
            } else if src.c(w) == 37 && src.c(w + 1) == 37 {
                emit!(t::SPACELINE, at(&src, Re::Line, w).unwrap_or(w), INITIAL);
            } else if w > p {
                if next_break < p as isize {
                    next_break = up_to(p, 10) as isize;
                }
                if next_break >= w as isize {
                    emit!(t::SPACELIST, w, INITIAL);
                } else if let Some(e) = src.last_char(10, w - 1).filter(|&e| e > p) {
                    emit!(t::SPACELINE, e + 1, INITIAL);
                } else {
                    emit!(t::NL, p + 1, INITIAL);
                }
            } else if letters(&src, p, keyword) && !is_word(src.c(p + keyword.len())) {
                emit!(t::KEYWORD, p + keyword.len(), INITIAL);
            } else if c == 58 && d == 58 && src.c(p + 2) == 58 {
                p += 3;
                state = CLASS;
            } else if c == 58 && d == 58 && letters(&src, p + 2, "icon") && src.c(p + 6) == 40 {
                p += 7;
                state = ICON;
            } else if (c == 45 && d == 41)
                || (c == 40 && (d == 45 || d == 40))
                || ((c == 41 || c == 123) && d == c)
            {
                emit!(t::NODE_DSTART, p + 2, NODE);
            } else if c == 41 || c == 40 || c == 91 {
                emit!(t::NODE_DSTART, p + 1, NODE);
            } else if let Some(e) = at(&src, re_id, p) {
                emit!(t::NODE_ID, e, INITIAL);
            } else {
                break;
            }
        } else if state == NODE {
            if c == 34 {
                state = if d == 96 { NSTR2 } else { NSTR };
                p += if d == 96 { 2 } else { 1 };
            } else if c == 41 || c == 40 {
                emit!(
                    t::NODE_DEND,
                    if d == c || (c == 40 && d == 45) {
                        p + 2
                    } else {
                        p + 1
                    },
                    INITIAL
                );
            } else if c == 93 {
                emit!(t::NODE_DEND, p + 1, INITIAL);
            } else if (c == 125 && d == 125) || (c == 45 && d == 41) {
                emit!(t::NODE_DEND, p + 2, INITIAL);
            } else {
                // A lone closing brace takes the rest of its line as text.
                let re = if c == 125 { Re::Line } else { Re::Descr };
                emit!(t::NODE_DESCR, at(&src, re, p).unwrap_or(p + 1), NODE);
            }
        } else if state == NSTR {
            if c == 34 {
                p += 1;
                state = NODE;
            } else {
                emit!(t::NODE_DESCR, up_to(p, 34), NSTR);
            }
        } else if state == NSTR2 {
            if c == 96 && d == 34 {
                p += 2;
                state = NODE;
            } else if let Some(e) = at(&src, Re::Md, p) {
                emit!(t::NODE_DESCR, e, NSTR2);
            } else {
                break;
            }
        } else if state == CLASS {
            if let Some(e) = at(&src, Re::Line, p) {
                emit!(t::CLASS, e, INITIAL);
            } else if c == 10 {
                p += 1;
                state = INITIAL;
            } else {
                break;
            }
        } else if state == ICON {
            if c == 41 {
                p += 1;
                state = INITIAL;
            } else {
                emit!(t::ICON, up_to(p, 41), ICON);
            }
        } else if state == DATA {
            if c == 34 {
                emit!(t::SHAPE_DATA, p + 1, DATA_STR);
            } else if c == 125 {
                p += 1;
                state = INITIAL;
            } else if let Some(e) = at(&src, Re::Data, p) {
                emit!(t::SHAPE_DATA, e, DATA);
            } else {
                break;
            }
        } else if c == 34 {
            emit!(t::SHAPE_DATA, p + 1, DATA);
        } else {
            let e = up_to(p, 34);
            let text = breaks(src.slice(p, e));
            put(&mut out, &mut p, t::SHAPE_DATA, text, e);
        }
    }

    if p < n {
        let e = p;
        put(&mut out, &mut p, t::ERROR, String::new(), e);
    } else {
        if state == INITIAL {
            put(&mut out, &mut p, t::EOF, String::new(), n);
        }
        put(&mut out, &mut p, t::END, String::new(), n);
    }
    out
}

const STOP: &str = "'SPACELINE', 'NL', 'EOF'";

/// `START`: the tokens a statement can start with.
fn start(kind: u32) -> bool {
    matches!(
        kind,
        t::SPACELINE | t::SPACELIST | t::ICON | t::CLASS | t::NODE_DSTART | t::NODE_ID
    )
}

/// Ports `isStop`.
fn is_stop(kind: u32) -> bool {
    kind == t::SPACELINE || kind == t::NL || kind == t::EOF
}

/// Ports `parseOutline`: makes the calls on `db` that the grammar's actions
/// make, in the same order, and reports errors in the state where the
/// generated parser would.
pub fn parse_outline(src: &str, db: &mut dyn OutlineDb, kanban: bool) -> Result<(), PeleError> {
    let diagram: &'static str = if kanban { "kanban" } else { "mindmap" };
    let Tokens {
        types,
        texts,
        starts,
    } = tokenize(src, kanban);
    let data = if kanban { ", 'SHAPE_DATA'" } else { "" };
    let mut i = 0usize;
    let ty = |i: usize| types.get(i).copied().unwrap_or(t::END);

    let fail = |i: usize, expected: &str| -> PeleError {
        let kind = ty(i);
        let at = starts.get(i).copied().unwrap_or(src.len());
        if kind == t::ERROR {
            return syntax_error(diagram, src, at, "", true);
        }
        syntax_error(
            diagram,
            src,
            at,
            &format!("Expecting {}, got '{}'", expected, token_name(kind, kanban)),
            false,
        )
    };

    let keyword = format!("'{}'", token_name(t::KEYWORD, kanban));
    if ty(i) == t::SPACELINE {
        i += 1;
        while ty(i) == t::SPACELINE || ty(i) == t::NL {
            i += 1;
        }
        if ty(i) != t::KEYWORD {
            return Err(fail(i, &format!("'SPACELINE', 'NL', {}", keyword)));
        }
    } else if ty(i) != t::KEYWORD {
        return Err(fail(i, &format!("'SPACELINE', {}", keyword)));
    }
    i += 1;
    let mut first = "'SPACELINE', 'NL', 'SPACELIST', 'ICON', 'CLASS', 'NODE_DSTART', 'NODE_ID'";
    if ty(i) == t::NL {
        i += 1;
        first = "'SPACELINE', 'SPACELIST', 'ICON', 'CLASS', 'NODE_DSTART', 'NODE_ID'";
    }

    loop {
        let mut k = ty(i);
        let mut level = 0usize;
        let indented = k == t::SPACELIST;
        if indented {
            level = len16(&texts[i]);
            i += 1;
            k = ty(i);
        }
        if k == t::NODE_ID || k == t::NODE_DSTART {
            let mut id: Option<&str> = None;
            let descr: &str;
            let mut kind = 0;
            if k == t::NODE_ID {
                id = Some(&texts[i]);
                i += 1;
            }
            if ty(i) == t::NODE_DSTART {
                let start = &texts[i];
                i += 1;
                if ty(i) != t::NODE_DESCR {
                    return Err(fail(i, "'NODE_DESCR'"));
                }
                descr = &texts[i];
                i += 1;
                if ty(i) != t::NODE_DEND {
                    return Err(fail(i, "'NODE_DEND'"));
                }
                let end = &texts[i];
                i += 1;
                if !is_stop(ty(i)) && ty(i) != t::SHAPE_DATA {
                    return Err(fail(i, &format!("{}{}", STOP, data)));
                }
                kind = db.get_type(start, end);
            } else {
                if !is_stop(ty(i)) && ty(i) != t::SHAPE_DATA {
                    return Err(fail(i, &format!("{}, 'NODE_DSTART'{}", STOP, data)));
                }
                descr = id.unwrap_or("");
            }
            if ty(i) == t::SHAPE_DATA {
                let mut shape_data = texts[i].clone();
                i += 1;
                while ty(i) == t::SHAPE_DATA {
                    shape_data.push_str(&texts[i]);
                    i += 1;
                }
                if !is_stop(ty(i)) {
                    return Err(fail(i, &format!("{}{}", STOP, data)));
                }
                db.add_node(level, id.unwrap_or(descr), descr, kind, Some(&shape_data))?;
            } else {
                db.add_node(level, id.unwrap_or(descr), descr, kind, None)?;
            }
        } else if k == t::ICON || k == t::CLASS {
            let text = &texts[i];
            i += 1;
            if !is_stop(ty(i)) {
                return Err(fail(i, STOP));
            }
            if k == t::ICON {
                db.decorate_node(Some(text), None)?;
            } else {
                db.decorate_node(None, Some(text))?;
            }
        } else if indented {
            if !is_stop(k) {
                return Err(fail(
                    i,
                    &format!("{}, 'ICON', 'CLASS', 'NODE_DSTART', 'NODE_ID'", STOP),
                ));
            }
        } else if k == t::SPACELINE {
            i += 1;
            if !is_stop(ty(i)) {
                return Err(fail(i, STOP));
            }
        } else {
            return Err(fail(i, first));
        }

        i += 1;
        while ty(i) == t::NL || ty(i) == t::EOF {
            i += 1;
        }
        if ty(i) == t::END {
            return Ok(());
        }
        if !start(ty(i)) {
            return Err(fail(
                i,
                &format!(
                    "{}, 'SPACELIST', 'ICON', 'CLASS', 'NODE_DSTART', 'NODE_ID'",
                    STOP
                ),
            ));
        }
    }
}
