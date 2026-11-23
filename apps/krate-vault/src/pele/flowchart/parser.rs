//! Ports diagrams/flowchart/parser.ts: a recursive-descent parser over the
//! lexer's tokens, with Jison's error messages.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use super::db::FlowDb;
use super::lexer::tokenize;
use super::tokens::{self as t, TOKEN_NAMES};
use super::types::{DocItem, FlowText, LabelType, LinkInfo};
use crate::pele::errors::{input_offset, syntax_error, Edit, PeleError};

fn id_token(k: u8) -> bool {
    matches!(
        k,
        t::NUM
            | t::NODE_STRING
            | t::DOWN
            | t::MINUS
            | t::DEFAULT
            | t::COMMA
            | t::COLON
            | t::AMP
            | t::BRKT
            | t::MULT
            | t::UNICODE_TEXT
    )
}

fn alphanum_token(k: u8) -> bool {
    matches!(
        k,
        t::NUM
            | t::UNICODE_TEXT
            | t::NODE_STRING
            | t::DIR
            | t::DOWN
            | t::MINUS
            | t::COMMA
            | t::COLON
            | t::AMP
            | t::BRKT
            | t::MULT
    )
}

fn no_tags_token(k: u8) -> bool {
    matches!(
        k,
        t::NUM
            | t::NODE_STRING
            | t::SPACE
            | t::MINUS
            | t::AMP
            | t::UNICODE_TEXT
            | t::COLON
            | t::MULT
            | t::BRKT
            | t::STYLE
            | t::LINKSTYLE
            | t::CLASSDEF
            | t::CLASS
            | t::CLICK
            | t::GRAPH
            | t::DIR
            | t::subgraph
            | t::end
            | t::DOWN
            | t::UP
            | t::START_LINK
    )
}

fn style_part(k: u8) -> bool {
    matches!(
        k,
        t::NUM | t::NODE_STRING | t::COLON | t::SPACE | t::BRKT | t::STYLE
    )
}

const TEXT_TOKENS: &str = "'UNICODE_TEXT', 'TEXT', 'TAGSTART'";
const DIRECTIONS: [&str; 5] = ["TB", "BT", "RL", "LR", "TD"];
const STATEMENT_START: &str = "'SEMI', 'NEWLINE', 'SPACE', 'EOF', 'subgraph', 'acc_title', 'acc_descr', 'acc_descr_multiline_value', 'STYLE', 'LINKSTYLE', 'CLASSDEF', 'CLASS', 'CLICK', 'NODE_STRING'";

/// Ports `interface Frame`.
struct Frame {
    id: Option<FlowText>,
    title: Option<FlowText>,
    list: Vec<DocItem>,
    collect: bool,
}

type R<T> = Result<T, PeleError>;

struct Parser<'a> {
    source: &'a str,
    src: String,
    types: Vec<u8>,
    texts: Vec<String>,
    starts: Vec<usize>,
    i: usize,
    pending_data: Option<String>,
    /// Where Mermaid's trim of blanks after a `}` changed the text, to map
    /// an error's position back (`replacements(source, RE_BRACE_TRAIL, 2)`).
    trims: Vec<Edit>,
}

/// `source.replace(RE_BRACE_TRAIL, '}\n')`: RE_BRACE_TRAIL = /}\s*\n/g.
fn trim_brace_trails(source: &str) -> (String, Vec<Edit>) {
    let b = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut trims = Vec::new();
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'}' {
            // \s* is greedy and backtracks to the last \n in the run.
            let mut j = i + 1;
            let mut last_nl = None;
            while let Some(c) = source[j..].chars().next() {
                if !crate::pele::js::is_space(c) {
                    break;
                }
                if c == '\n' {
                    last_nl = Some(j);
                }
                j += c.len_utf8();
            }
            if let Some(nl) = last_nl {
                out.push_str(&source[last..i]);
                out.push_str("}\n");
                trims.push(crate::pele::js::edit16(source, i, nl + 1, 2));
                last = nl + 1;
                i = nl + 1;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&source[last..]);
    (out, trims)
}

impl<'a> Parser<'a> {
    fn ty(&self, k: usize) -> u8 {
        self.types.get(k).copied().unwrap_or(t::END)
    }

    fn cur(&self) -> u8 {
        self.ty(self.i)
    }

    fn text_at(&self, k: usize) -> String {
        self.texts.get(k).cloned().unwrap_or_default()
    }

    /// Ports `fail`.
    fn fail<T>(&self, expected: &str) -> R<T> {
        let tk = self.cur();
        let at = self.starts.get(self.i).copied().unwrap_or(self.src.len());
        // Map a position in the trimmed text back to the source.
        let at = if self.trims.is_empty() {
            at
        } else {
            let units = input_offset(
                core::slice::from_ref(&self.trims),
                crate::pele::js::units_at(&self.src, at),
            );
            crate::pele::js::byte_at(self.source, units)
        };
        if tk == t::ERROR {
            return Err(syntax_error("flowchart", self.source, at, "", true));
        }
        Err(syntax_error(
            "flowchart",
            self.source,
            at,
            &format!("Expecting {}, got '{}'", expected, TOKEN_NAMES[tk as usize]),
            false,
        ))
    }

    /// Ports `expect`.
    fn expect(&mut self, kind: u8, also: &str) -> R<String> {
        if self.cur() != kind {
            let extra = if also.is_empty() {
                String::new()
            } else {
                format!(", {}", also)
            };
            return self.fail(&format!("'{}'{}", TOKEN_NAMES[kind as usize], extra));
        }
        let s = self.text_at(self.i);
        self.i += 1;
        Ok(s)
    }

    /// Ports `separator`.
    fn separator(&mut self) -> R<()> {
        let tk = self.cur();
        if tk != t::NEWLINE && tk != t::SEMI && tk != t::EOF {
            return self.fail("'SEMI', 'NEWLINE', 'EOF'");
        }
        self.i += 1;
        Ok(())
    }

    fn take(&mut self) -> String {
        let s = self.text_at(self.i);
        self.i += 1;
        s
    }

    /// Ports `idString`.
    fn id_string(&mut self) -> R<String> {
        if !id_token(self.cur()) {
            return self.fail("'NODE_STRING'");
        }
        let mut s = self.take();
        while id_token(self.cur()) {
            s.push_str(&self.take());
        }
        Ok(s)
    }

    /// Ports `alphaNum`.
    fn alpha_num(&mut self) -> R<String> {
        if !alphanum_token(self.cur()) {
            return self.fail("'NODE_STRING'");
        }
        let mut s = self.take();
        while alphanum_token(self.cur()) {
            s.push_str(&self.take());
        }
        Ok(s)
    }

    /// Ports `text`.
    fn text(&mut self) -> R<FlowText> {
        let tk = self.cur();
        let kind = if tk == t::STR {
            LabelType::String
        } else if tk == t::MD_STR {
            LabelType::Markdown
        } else if matches!(tk, t::TEXT | t::TAGSTART | t::TAGEND | t::UNICODE_TEXT) {
            LabelType::Text
        } else {
            return self.fail(&format!("{}, 'TAGEND', 'STR', 'MD_STR'", TEXT_TOKENS));
        };
        let mut s = self.take();
        while matches!(
            self.cur(),
            t::TEXT | t::TAGSTART | t::TAGEND | t::UNICODE_TEXT
        ) {
            s.push_str(&self.take());
        }
        Ok(FlowText::new(s, kind))
    }

    /// Ports `closed`.
    fn closed(&mut self, kind: u8) -> R<()> {
        self.expect(kind, TEXT_TOKENS).map(|_| ())
    }

    /// Ports `edgeText`.
    fn edge_text(&mut self) -> R<FlowText> {
        let tk = self.cur();
        let kind = if tk == t::STR {
            LabelType::String
        } else if tk == t::MD_STR {
            LabelType::Markdown
        } else if tk == t::EDGE_TEXT || tk == t::UNICODE_TEXT {
            LabelType::Text
        } else {
            return self.fail("'EDGE_TEXT', 'STR', 'MD_STR'");
        };
        let mut s = self.take();
        while matches!(self.cur(), t::EDGE_TEXT | t::UNICODE_TEXT) {
            s.push_str(&self.take());
        }
        Ok(FlowText::new(s, kind))
    }

    /// Ports `textNoTags`.
    fn text_no_tags(&mut self) -> R<FlowText> {
        let tk = self.cur();
        let kind = if tk == t::STR {
            LabelType::Text
        } else if tk == t::MD_STR {
            LabelType::Markdown
        } else if no_tags_token(tk) {
            LabelType::Text
        } else {
            return self.fail("'NODE_STRING', 'STR', 'MD_STR'");
        };
        let mut s = self.take();
        while no_tags_token(self.cur()) {
            s.push_str(&self.take());
        }
        Ok(FlowText::new(s, kind))
    }

    /// Ports `shapeData`.
    fn shape_data(&mut self) -> String {
        let mut s = self.take();
        while self.cur() == t::SHAPE_DATA {
            s.push_str(&self.take());
        }
        s
    }

    /// Ports `vertex`.
    fn vertex(&mut self, db: &mut FlowDb) -> R<String> {
        let id = self.id_string()?;
        let add = |db: &mut FlowDb, label: FlowText, kind: &str| {
            db.add_vertex(&id, Some(label), Some(kind), None, None, None, None, None)
        };
        match self.cur() {
            t::SQS => {
                self.i += 1;
                let label = self.text()?;
                self.closed(t::SQE)?;
                add(db, label, "square")?;
            }
            t::DOUBLECIRCLESTART => {
                self.i += 1;
                let label = self.text()?;
                self.closed(t::DOUBLECIRCLEEND)?;
                add(db, label, "doublecircle")?;
            }
            t::PS => {
                self.i += 1;
                if self.cur() == t::PS {
                    self.i += 1;
                    let label = self.text()?;
                    self.closed(t::PE)?;
                    self.expect(t::PE, "")?;
                    add(db, label, "circle")?;
                } else {
                    let label = self.text()?;
                    self.closed(t::PE)?;
                    add(db, label, "round")?;
                }
            }
            t::ELLIPSE_START => {
                self.i += 1;
                let label = self.text()?;
                self.closed(t::ELLIPSE_END)?;
                add(db, label, "ellipse")?;
            }
            t::STADIUMSTART => {
                self.i += 1;
                let label = self.text()?;
                self.closed(t::STADIUMEND)?;
                add(db, label, "stadium")?;
            }
            t::SUBROUTINESTART => {
                self.i += 1;
                let label = self.text()?;
                self.closed(t::SUBROUTINEEND)?;
                add(db, label, "subroutine")?;
            }
            t::VERTEX_WITH_PROPS_START => {
                self.i += 1;
                let field = self.expect(t::NODE_STRING, "")?;
                self.expect(t::COLON, "")?;
                let value = self.expect(t::NODE_STRING, "")?;
                self.expect(t::PIPE, "")?;
                let label = self.text()?;
                self.closed(t::SQE)?;
                db.add_vertex(
                    &id,
                    Some(label),
                    Some("rect"),
                    None,
                    None,
                    None,
                    Some(vec![(field, value)]),
                    None,
                )?;
            }
            t::CYLINDERSTART => {
                self.i += 1;
                let label = self.text()?;
                self.closed(t::CYLINDEREND)?;
                add(db, label, "cylinder")?;
            }
            t::DIAMOND_START => {
                self.i += 1;
                if self.cur() == t::DIAMOND_START {
                    self.i += 1;
                    let label = self.text()?;
                    self.closed(t::DIAMOND_STOP)?;
                    self.expect(t::DIAMOND_STOP, "")?;
                    add(db, label, "hexagon")?;
                } else {
                    let label = self.text()?;
                    self.closed(t::DIAMOND_STOP)?;
                    add(db, label, "diamond")?;
                }
            }
            t::TAGEND => {
                self.i += 1;
                let label = self.text()?;
                self.closed(t::SQE)?;
                add(db, label, "odd")?;
            }
            t::TRAPSTART => {
                self.i += 1;
                let label = self.text()?;
                if self.cur() == t::INVTRAPEND {
                    self.i += 1;
                    add(db, label, "lean_right")?;
                } else {
                    self.closed(t::TRAPEND)?;
                    add(db, label, "trapezoid")?;
                }
            }
            t::INVTRAPSTART => {
                self.i += 1;
                let label = self.text()?;
                if self.cur() == t::TRAPEND {
                    self.i += 1;
                    add(db, label, "lean_left")?;
                } else {
                    self.closed(t::INVTRAPEND)?;
                    add(db, label, "inv_trapezoid")?;
                }
            }
            _ => db.add_vertex(&id, None, None, None, None, None, None, None)?,
        }
        Ok(id)
    }

    /// Ports `styledVertex`.
    fn styled_vertex(&mut self, db: &mut FlowDb) -> R<String> {
        let id = self.vertex(db)?;
        if self.cur() == t::STYLE_SEPARATOR {
            self.i += 1;
            let class = self.id_string()?;
            db.set_class(&id, &class);
        }
        Ok(id)
    }

    /// Ports `node`.
    fn node(&mut self, db: &mut FlowDb) -> R<Vec<String>> {
        let mut ids = vec![self.styled_vertex(db)?];
        self.pending_data = None;
        loop {
            let data = if self.cur() == t::SHAPE_DATA {
                Some(self.shape_data())
            } else {
                None
            };
            if self.cur() != t::SPACE {
                self.pending_data = data;
                return Ok(ids);
            }
            loop {
                self.i += 1;
                if self.cur() != t::SPACE {
                    break;
                }
            }
            if self.cur() != t::AMP {
                if data.is_some() {
                    return self.fail("'AMP'");
                }
                return Ok(ids);
            }
            self.i += 1;
            if self.cur() != t::SPACE {
                return self.fail("'SPACE'");
            }
            loop {
                self.i += 1;
                if self.cur() != t::SPACE {
                    break;
                }
            }
            let next = self.styled_vertex(db)?;
            if let Some(d) = data {
                let last = ids[ids.len() - 1].clone();
                db.add_vertex(&last, None, None, None, None, None, None, Some(d))?;
            }
            ids.push(next);
        }
    }

    /// Ports `link`.
    fn link(&mut self, db: &mut FlowDb) -> R<LinkInfo> {
        let mut id = None;
        if self.cur() == t::LINK_ID {
            id = Some(self.take());
        }
        if self.cur() == t::LINK {
            let raw = self.take();
            let mut info = db.destruct_link(&raw, None);
            if id.is_some() {
                info.id = id;
            }
            if self.cur() == t::PIPE {
                self.i += 1;
                info.text = Some(self.text()?);
                self.closed(t::PIPE)?;
                if self.cur() == t::SPACE {
                    self.i += 1;
                }
            }
            return Ok(info);
        }
        if self.cur() == t::START_LINK {
            let start = self.take();
            let label = self.edge_text()?;
            let end = self.expect(t::LINK, "'EDGE_TEXT'")?;
            let mut info = db.destruct_link(&end, Some(&start));
            info.text = Some(label);
            if id.is_some() {
                info.id = id;
            }
            return Ok(info);
        }
        self.fail("'LINK', 'START_LINK'")
    }

    /// Ports `vertexStatement`.
    fn vertex_statement(&mut self, db: &mut FlowDb, collect: bool) -> R<Vec<String>> {
        let mut stmt = self.node(db)?;
        if let Some(d) = self.pending_data.take() {
            let last = stmt[stmt.len() - 1].clone();
            db.add_vertex(&last, None, None, None, None, None, None, Some(d))?;
        }
        let mut nodes = stmt.clone();
        loop {
            let tk = self.cur();
            if tk != t::LINK && tk != t::START_LINK && tk != t::LINK_ID {
                return Ok(nodes);
            }
            let info = self.link(db)?;
            let next = self.node(db)?;
            if let Some(d) = self.pending_data.take() {
                let last = next[next.len() - 1].clone();
                db.add_vertex(&last, None, None, None, None, None, None, Some(d))?;
            }
            db.add_link(&stmt, &next, Some(&info))?;
            if collect {
                let mut joined = next.clone();
                joined.extend(nodes);
                nodes = joined;
            }
            stmt = next;
        }
    }

    /// Ports `style`.
    fn style(&mut self) -> R<String> {
        if !style_part(self.cur()) {
            return self.fail("'NODE_STRING', 'NUM', 'COLON', 'BRKT'");
        }
        let mut s = self.take();
        while style_part(self.cur()) {
            s.push_str(&self.take());
        }
        Ok(s)
    }

    /// Ports `stylesOpt`.
    fn styles_opt(&mut self) -> R<Vec<String>> {
        let mut out = vec![self.style()?];
        while self.cur() == t::COMMA {
            self.i += 1;
            out.push(self.style()?);
        }
        Ok(out)
    }

    /// Ports `linkStyleStatement`.
    fn link_style_statement(&mut self, db: &mut FlowDb) -> R<()> {
        self.i += 1;
        self.expect(t::SPACE, "")?;
        let mut positions: Vec<String>;
        if self.cur() == t::DEFAULT {
            positions = vec![self.take()];
        } else {
            positions = vec![self.expect(t::NUM, "'DEFAULT'")?];
            while self.cur() == t::COMMA {
                self.i += 1;
                positions.push(self.expect(t::NUM, "")?);
            }
        }
        self.expect(t::SPACE, "")?;
        if self.cur() != t::INTERPOLATE {
            let styles = self.styles_opt()?;
            return db.update_link(&positions, styles);
        }
        self.i += 1;
        self.expect(t::SPACE, "")?;
        let interpolate = self.alpha_num()?;
        if self.cur() == t::SPACE {
            self.i += 1;
            let styles = self.styles_opt()?;
            db.update_link_interpolate(&positions, &interpolate)?;
            db.update_link(&positions, styles)?;
        } else {
            db.update_link_interpolate(&positions, &interpolate)?;
        }
        Ok(())
    }

    /// Ports `linkTail`.
    fn link_tail(&mut self, db: &mut FlowDb, id: &str, url: &str) -> R<()> {
        if self.cur() != t::SPACE {
            db.set_link(id, url, None);
            return Ok(());
        }
        self.i += 1;
        if self.cur() == t::LINK_TARGET {
            let target = self.take();
            db.set_link(id, url, Some(&target));
            return Ok(());
        }
        let tooltip = self.expect(t::STR, "'LINK_TARGET'")?;
        if self.cur() == t::SPACE {
            self.i += 1;
            let target = self.expect(t::LINK_TARGET, "")?;
            db.set_link(id, url, Some(&target));
        } else {
            db.set_link(id, url, None);
        }
        db.set_tooltip(id, Some(&tooltip));
        Ok(())
    }

    /// Ports `clickStatement`.
    fn click_statement(&mut self, db: &mut FlowDb) -> R<()> {
        let id = self.take();
        match self.cur() {
            t::CALLBACKNAME => {
                let _name = self.take();
                if self.cur() == t::CALLBACKARGS {
                    self.i += 1;
                }
                let mut tooltip = None;
                if self.cur() == t::SPACE {
                    self.i += 1;
                    tooltip = Some(self.expect(t::STR, "")?);
                }
                db.set_click_event(&id);
                if let Some(tt) = tooltip {
                    db.set_tooltip(&id, Some(&tt));
                }
                Ok(())
            }
            t::HREF => {
                self.i += 1;
                let url = self.expect(t::STR, "")?;
                self.link_tail(db, &id, &url)
            }
            t::STR => {
                let url = self.take();
                self.link_tail(db, &id, &url)
            }
            _ => {
                let _name = self.alpha_num()?;
                if self.cur() == t::SPACE {
                    self.i += 1;
                    let tooltip = self.expect(t::STR, "")?;
                    db.set_click_event(&id);
                    db.set_tooltip(&id, Some(&tooltip));
                } else {
                    db.set_click_event(&id);
                }
                Ok(())
            }
        }
    }

    /// Ports `subgraphHeader`.
    fn subgraph_header(&mut self) -> R<Frame> {
        self.i += 1;
        let tk = self.cur();
        if tk == t::NEWLINE || tk == t::SEMI || tk == t::EOF {
            self.i += 1;
            return Ok(Frame {
                id: None,
                title: None,
                list: Vec::new(),
                collect: false,
            });
        }
        self.expect(t::SPACE, "'SEMI', 'NEWLINE', 'EOF'")?;
        let id = self.text_no_tags()?;
        let mut title = id.clone();
        let mut same = true;
        if self.cur() == t::SQS {
            self.i += 1;
            title = self.text()?;
            self.closed(t::SQE)?;
            same = false;
        }
        let tk = self.cur();
        if tk != t::NEWLINE && tk != t::SEMI && tk != t::EOF {
            return self.fail("'SEMI', 'NEWLINE', 'EOF', 'SQS'");
        }
        self.i += 1;
        let mut id = id;
        id.same_as_title = same;
        Ok(Frame {
            id: Some(id),
            title: Some(title),
            list: Vec::new(),
            collect: false,
        })
    }

    /// Ports `statement`.
    fn statement(&mut self, db: &mut FlowDb, collect: bool) -> R<DocItem> {
        let tk = self.cur();
        match tk {
            t::STYLE => {
                self.i += 1;
                self.expect(t::SPACE, "")?;
                let id = self.id_string()?;
                self.expect(t::SPACE, "")?;
                let styles = self.styles_opt()?;
                db.add_vertex(&id, None, None, Some(styles), None, None, None, None)?;
                self.separator()?;
                Ok(DocItem::List(Vec::new()))
            }
            t::LINKSTYLE => {
                self.link_style_statement(db)?;
                self.separator()?;
                Ok(DocItem::List(Vec::new()))
            }
            t::CLASSDEF => {
                self.i += 1;
                self.expect(t::SPACE, "")?;
                let id = self.id_string()?;
                self.expect(t::SPACE, "")?;
                let styles = self.styles_opt()?;
                db.add_class(&id, &styles);
                self.separator()?;
                Ok(DocItem::List(Vec::new()))
            }
            t::CLASS => {
                self.i += 1;
                self.expect(t::SPACE, "")?;
                let ids = self.id_string()?;
                self.expect(t::SPACE, "")?;
                let class = self.id_string()?;
                db.set_class(&ids, &class);
                self.separator()?;
                Ok(DocItem::List(Vec::new()))
            }
            t::CLICK => {
                self.click_statement(db)?;
                self.separator()?;
                Ok(DocItem::List(Vec::new()))
            }
            t::direction_tb
            | t::direction_bt
            | t::direction_rl
            | t::direction_lr
            | t::direction_td => {
                self.i += 1;
                Ok(DocItem::Dir(
                    DIRECTIONS[(tk - t::direction_tb) as usize].to_string(),
                ))
            }
            t::acc_title => {
                self.i += 1;
                let value =
                    crate::pele::js::trim(&self.expect(t::acc_title_value, "")?).to_string();
                db.set_acc_title(&value);
                Ok(DocItem::Text(value))
            }
            t::acc_descr => {
                self.i += 1;
                let value =
                    crate::pele::js::trim(&self.expect(t::acc_descr_value, "")?).to_string();
                db.set_acc_description(&value);
                Ok(DocItem::Text(value))
            }
            t::acc_descr_multiline_value => {
                let value = crate::pele::js::trim(&self.take()).to_string();
                db.set_acc_description(&value);
                Ok(DocItem::Text(value))
            }
            _ => {
                if !id_token(tk) {
                    return self.fail(STATEMENT_START);
                }
                let nodes = self.vertex_statement(db, collect)?;
                self.separator()?;
                Ok(DocItem::List(nodes))
            }
        }
    }

    /// Ports `document`: subgraphs nest without recursion; each open one
    /// keeps the list and collect flag of its parent.
    fn document(&mut self, db: &mut FlowDb) -> R<()> {
        let mut open: Vec<Frame> = Vec::new();
        let mut list: Vec<DocItem> = Vec::new();
        let mut collect = false;
        loop {
            let tk = self.cur();
            if tk == t::END {
                break;
            }
            if tk == t::end {
                let Some(frame) = open.pop() else { break };
                self.i += 1;
                let id = db.add_sub_graph(frame.id.as_ref(), &list, frame.title.as_ref());
                list = frame.list;
                collect = frame.collect;
                if collect {
                    list.push(DocItem::Text(id));
                }
            } else if tk == t::SEMI || tk == t::NEWLINE || tk == t::SPACE || tk == t::EOF {
                if collect {
                    list.push(DocItem::Text(self.text_at(self.i)));
                }
                self.i += 1;
            } else if tk == t::subgraph {
                let mut frame = self.subgraph_header()?;
                frame.list = core::mem::take(&mut list);
                frame.collect = collect;
                open.push(frame);
                collect = true;
            } else {
                let item = self.statement(db, collect)?;
                if collect && !matches!(&item, DocItem::List(l) if l.is_empty()) {
                    list.push(item);
                }
            }
        }
        if !open.is_empty() {
            return self.fail("'end'");
        }
        Ok(())
    }
}

/// Ports `parseFlowchart`.
pub fn parse_flowchart(source: &str, db: &mut FlowDb) -> Result<(), PeleError> {
    let (src, trims) = if source.contains('}') {
        trim_brace_trails(source)
    } else {
        (source.to_string(), Vec::new())
    };
    let tokens = tokenize(&src);
    let mut p = Parser {
        source,
        src,
        types: tokens.types,
        texts: tokens.texts,
        starts: tokens.starts,
        i: 0,
        pending_data: None,
        trims,
    };
    while p.cur() == t::SPACE || p.cur() == t::NEWLINE {
        p.i += 1;
    }
    p.expect(t::GRAPH, "'SPACE', 'NEWLINE'")?;
    if p.cur() == t::NODIR {
        p.i += 1;
        db.set_direction("TB");
    } else {
        let dir = p.expect(t::DIR, "'NODIR'")?;
        let mut tk = p.cur();
        if tk == t::SPACE {
            loop {
                p.i += 1;
                if p.cur() != t::SPACE {
                    break;
                }
            }
            tk = p.cur();
            if tk != t::NEWLINE {
                return p.fail("'NEWLINE', 'SPACE'");
            }
        } else if tk != t::SEMI && tk != t::NEWLINE {
            return p.fail("'SEMI', 'NEWLINE', 'SPACE'");
        }
        p.i += 1;
        db.set_direction(&dir);
    }
    p.document(db)?;
    if p.cur() != t::END {
        return p.fail(STATEMENT_START);
    }
    Ok(())
}
