//! One open note: its text, its caret and selection, and the edits a
//! Markdown editor makes beyond plain typing -- lists that continue on
//! Enter, brackets that close themselves, `[[` that offers note names,
//! checkboxes, bold and italic around a selection.
//!
//! Every change goes through [`Doc::replace`], which keeps the word and
//! character counts and the fence index up to date by looking only at the
//! lines it touched, so a keystroke in a 200,000-line note costs what it
//! costs in a short one.

use crate::blocks::{self, Footnotes, Marks};
use crate::buffer::{Buffer, Pos};
use crate::md::{self, Block, Fences};
use crate::text::{chars_in, words_in};
use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

/// How a note is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Styled as it is written; the line being edited shows its syntax.
    Live,
    /// Every character as typed, lightly coloured.
    Source,
    /// Read only, all syntax hidden.
    Reading,
}

pub struct Doc {
    pub path: String,
    /// The note's name, shown as its title and on its tab.
    pub title: String,
    pub buf: Buffer,
    pub fences: Fences,
    pub cur: Pos,
    /// Where a selection began; it runs from here to `cur`.
    pub anchor: Option<Pos>,
    /// The x the caret keeps when it moves up and down through short lines.
    pub goal_x: Option<f32>,
    /// The first row shown: 0 is the title, `n + 1` is line `n`.
    pub top: usize,
    /// How far into that row the view is scrolled, in pixels.
    pub top_off: f32,
    pub mode: Mode,
    pub words: usize,
    pub chars: usize,
    /// When the last unsaved change was made (ms), for saving on a pause.
    pub dirty_since: Option<u64>,
    pub save_error: Option<String>,
    /// The caret moved since the view last made sure it is on screen.
    pub reveal: bool,
    /// Opened and not touched yet: like Obsidian, a note opens without the
    /// editor focused, so no caret shows and no line shows its syntax
    /// until the person clicks into it or presses a key.
    pub fresh: bool,
    /// The `$$` lines of display-math blocks, kept like the fences.
    pub maths: Marks,
    /// The `%%` lines of comment blocks.
    pub comments: Marks,
    /// The last line of the front matter, when the note has one.
    pub front: Option<usize>,
    /// Headings and list items folded shut, by line.
    pub folds: BTreeSet<usize>,
    /// Callouts whose fold was flipped from how they are written (`-`
    /// opened, `+` closed), by the line of their head.
    pub callouts_flipped: BTreeSet<usize>,
    /// Carets beyond the main one (Option-click, Cmd+D), each with its own
    /// selection anchor.
    pub extra: Vec<(Option<Pos>, Pos)>,
    /// Find and replace in this note, while its bar is open.
    pub find: Option<crate::find::Find>,
    /// A property being edited inside the Properties block.
    pub prop_edit: Option<crate::propedit::PropEdit>,
    /// Spelling choices for the guest to carry out (learn, ignore).
    pub spell_pending: Vec<crate::spell::SpellAct>,
    /// The types the vault keeps for property names (`.obsidian/types.json`).
    pub prop_types: Vec<(String, crate::props::Kind)>,
    /// Edits made while several carets type, so each caret can follow the
    /// edits made before it.
    edit_log: Option<Vec<(Pos, Pos, Pos)>>,
    /// The fold ranges and footnotes as of a buffer version, worked out
    /// when the view first asks after an edit.
    fold_cache: RefCell<(u64, Vec<(usize, usize)>)>,
    notes_cache: RefCell<Option<(u64, alloc::rc::Rc<Footnotes>)>>,
}

/// A run of lines drawn as one piece (when the caret is not in it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    Front,
    Math,
    Table,
    Callout,
    Embed,
    /// A ```mermaid block holding a flowchart.
    Diagram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub kind: SpanKind,
    pub first: usize,
    pub last: usize,
}

/// Where position `q` lands after `a..b` was replaced by text ending at `e`
/// (`q` at or after `b`).
fn follow(q: Pos, a: Pos, b: Pos, e: Pos) -> Pos {
    if q < b {
        return if q < a { q } else { e };
    }
    if q.line == b.line {
        Pos::new(e.line, e.col + (q.col - b.col))
    } else {
        Pos::new(
            (q.line as isize + e.line as isize - b.line as isize) as usize,
            q.col,
        )
    }
}

/// The edit a list line continues with on Enter, if any.
fn continuation(line: &str, block: Block) -> Option<String> {
    let indent = crate::text::indent_of(line);
    let mut s = String::from(indent);
    match block {
        Block::Task { .. } => {
            let bullet = line.trim_start().chars().next().unwrap_or('-');
            s.push(bullet);
            s.push_str(" [ ] ");
        }
        Block::Bullet { .. } => {
            let bullet = line.trim_start().chars().next().unwrap_or('-');
            s.push(bullet);
            s.push(' ');
        }
        Block::Ordered { .. } => {
            let t = line.trim_start();
            let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
            let n: usize = digits.parse().unwrap_or(0);
            let sep = t.chars().nth(digits.len()).unwrap_or('.');
            crate::text::push_num(&mut s, n + 1);
            s.push(sep);
            s.push(' ');
        }
        Block::Quote { .. } => s.push_str("> "),
        _ => return None,
    }
    Some(s)
}

fn closer(c: char) -> Option<char> {
    match c {
        '[' => Some(']'),
        '(' => Some(')'),
        '{' => Some('}'),
        _ => None,
    }
}

impl Doc {
    pub fn new(path: &str, title: &str, text: &str) -> Doc {
        let buf = Buffer::from_text(text);
        let fences = Fences::build((0..buf.line_count()).map(|i| buf.line(i)));
        let mut d = Doc {
            path: String::from(path),
            title: String::from(title),
            buf,
            fences,
            cur: Pos::new(0, 0),
            anchor: None,
            goal_x: None,
            top: 0,
            top_off: 0.0,
            mode: Mode::Live,
            words: 0,
            chars: 0,
            dirty_since: None,
            save_error: None,
            reveal: false,
            fresh: true,
            maths: Marks::default(),
            comments: Marks::default(),
            front: None,
            folds: BTreeSet::new(),
            callouts_flipped: BTreeSet::new(),
            extra: Vec::new(),
            find: None,
            prop_edit: None,
            spell_pending: Vec::new(),
            prop_types: Vec::new(),
            edit_log: None,
            fold_cache: RefCell::new((u64::MAX, Vec::new())),
            notes_cache: RefCell::new(None),
        };
        d.rebuild_blocks();
        d.recount();
        // The caret starts below the Properties block, as it does in
        // Obsidian, so the block shows as properties, not as YAML.
        if let Some(f) = d.front {
            d.cur = d.buf.clamp(Pos::new(f + 1, 0));
        }
        d
    }

    /// Find the math and comment blocks and the front matter afresh.
    pub fn rebuild_blocks(&mut self) {
        let buf = &self.buf;
        let fences = &self.fences;
        let n = buf.line_count();
        let code = |i: usize| fences.in_code(i) || fences.is_fence(i);
        self.maths = Marks::build(n, |i| !code(i) && blocks::is_math_mark(buf.line(i)));
        self.comments = Marks::build(n, |i| !code(i) && blocks::is_comment_mark(buf.line(i)));
        self.front = blocks::front_matter_end(|i| buf.line(i), n);
    }

    /// Count words and characters across the whole note.
    pub fn recount(&mut self) {
        let mut w = 0;
        let mut c = 0;
        for i in 0..self.buf.line_count() {
            let l = self.buf.line(i);
            w += words_in(l);
            c += chars_in(l);
        }
        self.words = w;
        self.chars = c;
    }

    pub fn text(&self) -> String {
        self.buf.text()
    }

    pub fn modified(&self) -> bool {
        self.buf.modified()
    }

    pub fn line(&self, n: usize) -> &str {
        self.buf.line(n)
    }

    pub fn block(&self, n: usize) -> Block {
        md::classify(self.buf.line(n), self.fences.in_code(n))
    }

    /// The selection, start first; `None` when nothing is selected.
    pub fn selection(&self) -> Option<(Pos, Pos)> {
        let a = self.anchor?;
        if a == self.cur {
            return None;
        }
        Some(if a < self.cur {
            (a, self.cur)
        } else {
            (self.cur, a)
        })
    }

    pub fn selected_text(&self) -> String {
        match self.selection() {
            Some((a, b)) => self.buf.slice(a, b),
            None => String::new(),
        }
    }

    /// Replace `a..b` with `text`, keeping the counts and fences right.
    /// Returns where the new text ends.
    pub fn replace(&mut self, a: Pos, b: Pos, text: &str, now: u64) -> Pos {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let a = self.buf.clamp(a);
        let b = self.buf.clamp(b);
        for n in a.line..=b.line {
            let l = self.buf.line(n);
            self.words = self.words.saturating_sub(words_in(l));
            self.chars = self.chars.saturating_sub(chars_in(l));
        }
        let end = self.buf.replace(a, b, text);
        for n in a.line..=end.line {
            let l = self.buf.line(n);
            self.words += words_in(l);
            self.chars += chars_in(l);
        }
        let fences_before = self.fences.count();
        let buf = &self.buf;
        let touched_fence = (a.line..=end.line).any(|n| md::is_fence(buf.line(n)));
        self.fences
            .edit(a.line, b.line - a.line + 1, end.line - a.line + 1, |n| {
                buf.line(n)
            });
        if touched_fence || self.fences.count() != fences_before {
            // Code moved: which `$$` and `%%` lines count changed with it.
            self.rebuild_blocks();
        } else {
            let buf = &self.buf;
            let fences = &self.fences;
            let code = |i: usize| fences.in_code(i) || fences.is_fence(i);
            let (old, new) = (b.line - a.line + 1, end.line - a.line + 1);
            self.maths.edit(a.line, old, new, |i| {
                !code(i) && blocks::is_math_mark(buf.line(i))
            });
            self.comments.edit(a.line, old, new, |i| {
                !code(i) && blocks::is_comment_mark(buf.line(i))
            });
            if a.line <= self.front.map_or(0, |f| f + 1) {
                self.front = blocks::front_matter_end(|i| buf.line(i), buf.line_count());
            }
        }
        // Folds and flipped callouts follow the lines they are on.
        let delta = end.line as isize - b.line as isize;
        if delta != 0 || !self.folds.is_empty() {
            let shift = |set: &BTreeSet<usize>| -> BTreeSet<usize> {
                set.iter()
                    .filter_map(|l| {
                        if *l < a.line || (*l == a.line && a.col > 0) {
                            Some(*l)
                        } else if *l > b.line || (*l == b.line && b.col == 0) {
                            Some((*l as isize + delta).max(0) as usize)
                        } else {
                            None
                        }
                    })
                    .collect()
            };
            self.folds = shift(&self.folds);
            self.callouts_flipped = shift(&self.callouts_flipped);
        }
        if let Some(log) = self.edit_log.as_mut() {
            log.push((a, b, end));
        }
        self.fresh = false;
        self.dirty_since.get_or_insert(now);
        self.reveal = true;
        end
    }

    fn set_cursor(&mut self, p: Pos, extend: bool) {
        self.fresh = false;
        // Placing the caret in the note ends a property edit, kept.
        if self.prop_edit.is_some() {
            let now = self.dirty_since.unwrap_or(0);
            crate::propedit::commit(self, now, false);
        }
        if extend {
            self.anchor.get_or_insert(self.cur);
        } else {
            self.anchor = None;
        }
        self.cur = self.buf.clamp(p);
        self.reveal = true;
    }

    /// Put the caret at `p`; with `extend`, grow the selection to it.
    pub fn place(&mut self, p: Pos, extend: bool) {
        self.goal_x = None;
        self.set_cursor(p, extend);
    }

    /// Move to a line keeping `goal_x` (vertical moves set it themselves).
    pub fn place_vertical(&mut self, p: Pos, extend: bool) {
        self.set_cursor(p, extend);
    }

    /// Type text at the caret, over any selection.
    pub fn insert(&mut self, text: &str, now: u64) {
        self.goal_x = None;
        let mut chars = text.chars();
        let single = match (chars.next(), chars.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        };
        if let Some(c) = single {
            // A selection with a bracket or emphasis typed over it is wrapped.
            if let Some((a, b)) = self.selection() {
                let wrap = match c {
                    '[' | '(' | '{' => closer(c),
                    '*' | '_' | '`' | '=' | '~' => Some(c),
                    _ => None,
                };
                if let Some(close) = wrap {
                    let inner = self.buf.slice(a, b);
                    let mut s = String::new();
                    s.push(c);
                    s.push_str(&inner);
                    s.push(close);
                    self.buf.begin_group(self.cur);
                    let end = self.replace(a, b, &s, now);
                    self.buf.end_group(end);
                    let start = Pos::new(a.line, a.col + c.len_utf8());
                    self.anchor = Some(start);
                    self.cur = Pos::new(end.line, end.col.saturating_sub(close.len_utf8()));
                    return;
                }
            }
            let next = self.buf.char_at(self.cur);
            // Typing the closing bracket that is already there steps over it.
            if self.selection().is_none() && matches!(c, ']' | ')' | '}') && next == Some(c) {
                self.place(Pos::new(self.cur.line, self.cur.col + 1), false);
                return;
            }
            // An opening bracket brings its partner when nothing follows.
            if let Some(close) = closer(c) {
                if self.selection().is_none()
                    && next.is_none_or(|n| {
                        n == '\n' || n.is_whitespace() || matches!(n, ']' | ')' | '}')
                    })
                {
                    let mut s = String::new();
                    s.push(c);
                    s.push(close);
                    let at = self.cur;
                    self.replace(at, at, &s, now);
                    self.cur = Pos::new(at.line, at.col + c.len_utf8());
                    self.anchor = None;
                    return;
                }
            }
        }
        let (a, b) = self.selection().unwrap_or((self.cur, self.cur));
        let end = self.replace(a, b, text, now);
        self.cur = end;
        self.anchor = None;
    }

    /// Enter: a new line that continues the list or quote the caret is in;
    /// on an empty item, end the list instead.
    pub fn newline(&mut self, now: u64) {
        self.goal_x = None;
        if let Some((a, b)) = self.selection() {
            self.replace(a, b, "", now);
            self.cur = a;
            self.anchor = None;
        }
        let line = String::from(self.buf.line(self.cur.line));
        let block = self.block(self.cur.line);
        let content = block.content();
        if let Some(cont) = continuation(&line, block) {
            if line.get(content..).is_some_and(|r| r.trim().is_empty()) && self.cur.col >= content {
                // An empty item: Enter outdents it, or ends a top-level list.
                let n = self.cur.line;
                let cut = if line.starts_with('\t') {
                    1
                } else if line.starts_with("  ") {
                    2
                } else {
                    0
                };
                if block.level() > 0 && cut > 0 {
                    self.replace(Pos::new(n, 0), Pos::new(n, cut), "", now);
                    self.cur = Pos::new(n, self.buf.line_len(n));
                } else {
                    self.cur = self.replace(Pos::new(n, 0), Pos::new(n, line.len()), "", now);
                }
                return;
            }
            if self.cur.col >= content {
                let mut s = String::from("\n");
                s.push_str(&cont);
                let at = self.cur;
                self.cur = self.replace(at, at, &s, now);
                return;
            }
        }
        let at = self.cur;
        self.cur = self.replace(at, at, "\n", now);
    }

    /// Backspace: the selection, a pair of brackets, a list marker, or one
    /// character.
    pub fn backspace(&mut self, now: u64) {
        self.goal_x = None;
        if let Some((a, b)) = self.selection() {
            self.replace(a, b, "", now);
            self.cur = a;
            self.anchor = None;
            return;
        }
        let p = self.cur;
        let block = self.block(p.line);
        let content = block.content();
        if p.col > 0
            && p.col == content
            && (block.is_list() || matches!(block, Block::Quote { .. }))
        {
            // At the start of an item's text: the marker goes, the text stays.
            let line = String::from(self.buf.line(p.line));
            let indent = String::from(crate::text::indent_of(&line));
            let e = self.replace(Pos::new(p.line, 0), Pos::new(p.line, content), &indent, now);
            self.cur = e;
            return;
        }
        let Some(prev) = self.buf.prev_pos(p) else {
            return;
        };
        let before = self.buf.char_at(prev);
        let after = self.buf.char_at(p);
        let mut end = p;
        if let (Some(b), Some(a)) = (before, after) {
            if closer(b) == Some(a) {
                end = Pos::new(p.line, p.col + a.len_utf8());
            }
        }
        self.replace(prev, end, "", now);
        self.cur = prev;
        self.anchor = None;
    }

    /// Delete: the selection or the character after the caret.
    pub fn delete_forward(&mut self, now: u64) {
        self.goal_x = None;
        if let Some((a, b)) = self.selection() {
            self.replace(a, b, "", now);
            self.cur = a;
            self.anchor = None;
            return;
        }
        if let Some(next) = self.buf.next_pos(self.cur) {
            let at = self.cur;
            self.replace(at, next, "", now);
        }
    }

    /// Delete back to the start of the word (Alt+Backspace) or the line
    /// (Cmd+Backspace).
    pub fn delete_back_to(&mut self, to: Pos, now: u64) {
        if let Some((a, b)) = self.selection() {
            self.replace(a, b, "", now);
            self.cur = a;
            self.anchor = None;
            return;
        }
        let at = self.cur;
        self.replace(to, at, "", now);
        self.cur = self.buf.clamp(to);
        self.anchor = None;
    }

    pub fn left(&mut self, extend: bool) {
        if !extend {
            if let Some((a, _)) = self.selection() {
                self.place(a, false);
                return;
            }
        }
        if let Some(p) = self.buf.prev_pos(self.cur) {
            self.place(p, extend);
        } else {
            self.place(self.cur, extend);
        }
    }

    pub fn right(&mut self, extend: bool) {
        if !extend {
            if let Some((_, b)) = self.selection() {
                self.place(b, false);
                return;
            }
        }
        if let Some(p) = self.buf.next_pos(self.cur) {
            self.place(p, extend);
        } else {
            self.place(self.cur, extend);
        }
    }

    /// The start of the word before `p` (crossing to the previous line).
    pub fn word_left_of(&self, p: Pos) -> Pos {
        if p.col == 0 {
            return self.buf.prev_pos(p).unwrap_or(p);
        }
        let line = self.buf.line(p.line);
        let head = line.get(..p.col).unwrap_or("");
        let mut idx = head.len();
        let mut seen_word = false;
        for (i, c) in head.char_indices().rev() {
            let w = c.is_alphanumeric() || c == '_';
            if w {
                seen_word = true;
                idx = i;
            } else if seen_word {
                break;
            } else {
                idx = i;
            }
        }
        Pos::new(p.line, idx)
    }

    /// The end of the word after `p` (crossing to the next line).
    pub fn word_right_of(&self, p: Pos) -> Pos {
        let line = self.buf.line(p.line);
        if p.col >= line.len() {
            return self.buf.next_pos(p).unwrap_or(p);
        }
        let tail = line.get(p.col..).unwrap_or("");
        let mut end = p.col + tail.len();
        let mut seen_word = false;
        for (i, c) in tail.char_indices() {
            let w = c.is_alphanumeric() || c == '_';
            if w {
                seen_word = true;
            } else if seen_word {
                end = p.col + i;
                break;
            }
        }
        Pos::new(p.line, end)
    }

    /// The word around `p`, for a double click.
    pub fn word_at(&self, p: Pos) -> (Pos, Pos) {
        let line = self.buf.line(p.line);
        let is_w = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
        let mut a = p.col.min(line.len());
        while a > 0 {
            let Some(c) = line.get(..a).and_then(|h| h.chars().next_back()) else {
                break;
            };
            if !is_w(c) {
                break;
            }
            a -= c.len_utf8();
        }
        let mut b = p.col.min(line.len());
        while let Some(c) = line.get(b..).and_then(|t| t.chars().next()) {
            if !is_w(c) {
                break;
            }
            b += c.len_utf8();
        }
        if a == b {
            b = self.buf.next_col(p.line, b);
        }
        (Pos::new(p.line, a), Pos::new(p.line, b))
    }

    pub fn select_all(&mut self) {
        self.fresh = false;
        self.anchor = Some(Pos::new(0, 0));
        self.cur = self.buf.end();
        self.goal_x = None;
    }

    pub fn select_range(&mut self, a: Pos, b: Pos) {
        self.fresh = false;
        self.anchor = Some(self.buf.clamp(a));
        self.cur = self.buf.clamp(b);
        self.goal_x = None;
        self.reveal = true;
    }

    pub fn undo(&mut self) {
        if let Some(p) = self.buf.undo() {
            self.after_history(p);
        }
    }

    pub fn redo(&mut self) {
        if let Some(p) = self.buf.redo() {
            self.after_history(p);
        }
    }

    fn after_history(&mut self, p: Pos) {
        self.fences = Fences::build((0..self.buf.line_count()).map(|i| self.buf.line(i)));
        self.rebuild_blocks();
        self.extra.clear();
        self.recount();
        self.cur = self.buf.clamp(p);
        self.anchor = None;
        self.goal_x = None;
        self.reveal = true;
        if self.buf.modified() {
            self.dirty_since.get_or_insert(0);
        } else {
            self.dirty_since = None;
        }
    }

    /// Put `mark` around the selection, or take it away when it is already
    /// there (Cmd+B, Cmd+I).
    pub fn toggle_wrap(&mut self, mark: &str, now: u64) {
        let Some((a, b)) = self.selection() else {
            // Nothing selected: an empty pair with the caret inside.
            let at = self.cur;
            let mut s = String::from(mark);
            s.push_str(mark);
            self.replace(at, at, &s, now);
            self.cur = Pos::new(at.line, at.col + mark.len());
            self.anchor = None;
            return;
        };
        let inner = self.buf.slice(a, b);
        let line = self.buf.line(a.line);
        let m = mark.len();
        let wrapped = a.line == b.line
            && a.col >= m
            && line.get(a.col - m..a.col) == Some(mark)
            && line.get(b.col..b.col + m) == Some(mark);
        self.buf.begin_group(self.cur);
        if wrapped {
            let start = Pos::new(a.line, a.col - m);
            let stop = Pos::new(b.line, b.col + m);
            let end = self.replace(start, stop, &inner, now);
            self.anchor = Some(start);
            self.cur = end;
        } else {
            let mut s = String::from(mark);
            s.push_str(&inner);
            s.push_str(mark);
            let end = self.replace(a, b, &s, now);
            self.anchor = Some(Pos::new(a.line, a.col + m));
            self.cur = Pos::new(end.line, end.col.saturating_sub(m));
        }
        self.buf.end_group(self.cur);
    }

    /// Tick or untick the checkbox on `line`; a line that is not a task
    /// becomes one.
    pub fn toggle_task(&mut self, n: usize, now: u64) {
        let line = String::from(self.buf.line(n));
        match self.block(n) {
            Block::Task {
                box_at, checked, ..
            } => {
                let at = Pos::new(n, box_at + 1);
                self.replace(
                    at,
                    Pos::new(n, box_at + 2),
                    if checked { " " } else { "x" },
                    now,
                );
            }
            Block::Bullet {
                marker, content, ..
            } => {
                let _ = marker;
                self.replace(Pos::new(n, content), Pos::new(n, content), "[ ] ", now);
                if self.cur.line == n && self.cur.col >= content {
                    self.cur.col += 4;
                }
            }
            _ => {
                let indent = crate::text::indent_of(&line).len();
                self.replace(Pos::new(n, indent), Pos::new(n, indent), "- [ ] ", now);
                if self.cur.line == n && self.cur.col >= indent {
                    self.cur.col += 6;
                }
            }
        }
        self.cur = self.buf.clamp(self.cur);
    }

    /// Tab and Shift+Tab: indent or outdent the list items the caret or
    /// selection is on; elsewhere Tab types a tab.
    pub fn indent(&mut self, out: bool, now: u64) {
        let (a, b) = self.selection().unwrap_or((self.cur, self.cur));
        let lists = (a.line..=b.line).any(|n| self.block(n).is_list());
        if !lists && !out {
            self.insert("\t", now);
            return;
        }
        self.buf.begin_group(self.cur);
        let mut shift_cur: isize = 0;
        let mut shift_anchor: isize = 0;
        for n in a.line..=b.line {
            let line = String::from(self.buf.line(n));
            let delta: isize = if out {
                if line.starts_with('\t') {
                    self.replace(Pos::new(n, 0), Pos::new(n, 1), "", now);
                    -1
                } else if line.starts_with("  ") {
                    self.replace(Pos::new(n, 0), Pos::new(n, 2), "", now);
                    -2
                } else {
                    0
                }
            } else if self.block(n).is_list() {
                self.replace(Pos::new(n, 0), Pos::new(n, 0), "\t", now);
                1
            } else {
                0
            };
            if n == self.cur.line {
                shift_cur = delta;
            }
            if self.anchor.is_some_and(|an| an.line == n) {
                shift_anchor = delta;
            }
        }
        let c = self.cur;
        self.cur = self.buf.clamp(Pos::new(
            c.line,
            (c.col as isize + shift_cur).max(0) as usize,
        ));
        if let Some(an) = self.anchor {
            self.anchor = Some(self.buf.clamp(Pos::new(
                an.line,
                (an.col as isize + shift_anchor).max(0) as usize,
            )));
        }
        self.buf.end_group(self.cur);
        self.reveal = true;
    }

    /// When the caret is just after `[[` and what follows it on the line
    /// (the note name being typed): the column after `[[` and the text.
    pub fn link_query(&self) -> Option<(usize, String)> {
        if self.selection().is_some() || self.fences.in_code(self.cur.line) {
            return None;
        }
        let line = self.buf.line(self.cur.line);
        let head = line.get(..self.cur.col)?;
        let open = head.rfind("[[")?;
        let query = head.get(open + 2..)?;
        if query.contains("]]") || query.contains('|') || query.contains('[') || query.len() > 120 {
            return None;
        }
        Some((open + 2, String::from(query)))
    }

    /// Finish the link being typed with `name`: what was typed is replaced,
    /// the closing `]]` is added if it is not already there, and the caret
    /// lands after it.
    pub fn accept_link(&mut self, name: &str, now: u64) {
        let Some((start, _)) = self.link_query() else {
            return;
        };
        let line = String::from(self.buf.line(self.cur.line));
        let tail = line.get(self.cur.col..).unwrap_or("");
        // An auto-closed `]]` (or the `]` of a half-typed one) is reused.
        let closing = if tail.starts_with("]]") {
            2
        } else if tail.starts_with(']') {
            1
        } else {
            0
        };
        let l = self.cur.line;
        let mut s = String::from(name);
        s.push_str("]]");
        let end = self.replace(
            Pos::new(l, start),
            Pos::new(l, self.cur.col + closing),
            &s,
            now,
        );
        self.cur = end;
        self.anchor = None;
    }

    /// Every heading, for the outline pane.
    pub fn headings(&self) -> Vec<md::Heading> {
        let mut out = Vec::new();
        for n in 0..self.buf.line_count() {
            let line = self.buf.line(n);
            if !line.starts_with('#') && !line.starts_with(" #") && !line.starts_with("  #") {
                continue;
            }
            if let Block::Heading { level, content } = self.block(n) {
                out.push(md::Heading {
                    line: n,
                    level,
                    text: md::plain(line, content),
                });
            }
        }
        out
    }

    /// The wiki links in the note, for the outgoing-links pane, in order
    /// and once each.
    pub fn outgoing(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for n in 0..self.buf.line_count() {
            let line = self.buf.line(n);
            if !line.contains("[[") || self.fences.in_code(n) || self.fences.is_fence(n) {
                continue;
            }
            md::refs(
                line,
                |l| {
                    if !out.iter().any(|o| o.eq_ignore_ascii_case(l)) {
                        out.push(String::from(l));
                    }
                },
                |_| {},
            );
        }
        out
    }

    // ---- blocks that span lines ----------------------------------------------------

    /// The table, callout, math block, embed or front matter line `n` is
    /// part of.
    pub fn span(&self, n: usize) -> Option<Span> {
        if let Some(f) = self.front {
            if n <= f {
                return Some(Span {
                    kind: SpanKind::Front,
                    first: 0,
                    last: f,
                });
            }
        }
        let code = |i: usize| self.fences.in_code(i) || self.fences.is_fence(i);
        if code(n) {
            return self.diagram_span(n);
        }
        if let Some((first, last)) = self.maths.span(n) {
            return Some(Span {
                kind: SpanKind::Math,
                first,
                last,
            });
        }
        let line = self.buf.line(n);
        let t = line.trim();
        if t.len() >= 4 && t.starts_with("$$") && t.ends_with("$$") && t.matches("$$").count() == 2
        {
            return Some(Span {
                kind: SpanKind::Math,
                first: n,
                last: n,
            });
        }
        if blocks::row_like(line) {
            let mut s = n;
            while s > 0
                && n - s < blocks::RUN_LIMIT
                && blocks::row_like(self.buf.line(s - 1))
                && !code(s - 1)
            {
                s -= 1;
            }
            if s + 1 < self.buf.line_count() {
                let header = blocks::split_row(self.buf.line(s)).len();
                let delim = blocks::delimiter_row(self.buf.line(s + 1));
                if delim.is_some_and(|d| d.len() == header) {
                    let mut e = s + 1;
                    let last = self.buf.last_line();
                    while e < last
                        && e - s < blocks::RUN_LIMIT
                        && blocks::row_like(self.buf.line(e + 1))
                        && !code(e + 1)
                    {
                        e += 1;
                    }
                    if n <= e {
                        return Some(Span {
                            kind: SpanKind::Table,
                            first: s,
                            last: e,
                        });
                    }
                }
            }
        }
        if blocks::strip_quote(line).is_some() {
            let mut s = n;
            while s > 0
                && n - s < blocks::RUN_LIMIT
                && blocks::strip_quote(self.buf.line(s - 1)).is_some()
            {
                s -= 1;
            }
            if blocks::callout_head(self.buf.line(s)).is_some() {
                let mut e = n;
                let last = self.buf.last_line();
                while e < last
                    && e - s < blocks::RUN_LIMIT
                    && blocks::strip_quote(self.buf.line(e + 1)).is_some()
                {
                    e += 1;
                }
                return Some(Span {
                    kind: SpanKind::Callout,
                    first: s,
                    last: e,
                });
            }
        }
        if blocks::embed(line).is_some() {
            return Some(Span {
                kind: SpanKind::Embed,
                first: n,
                last: n,
            });
        }
        None
    }

    /// The ```mermaid flowchart block line `n` (inside code) belongs to.
    fn diagram_span(&self, n: usize) -> Option<Span> {
        let open = if self.fences.is_fence(n) {
            if self.fences.opens(n) {
                n
            } else {
                self.fences.prev_before(n)?
            }
        } else {
            self.fences.opener(n)?
        };
        if !md::fence_lang(self.buf.line(open)).eq_ignore_ascii_case("mermaid") {
            return None;
        }
        let close = self.fences.next_after(open)?;
        let first = (open + 1..close)
            .map(|i| self.buf.line(i).trim())
            .find(|l| !l.is_empty() && !l.starts_with("%%"))?;
        // Every kind Vault draws (charts.rs); others stay code.
        crate::charts::kind(first)?;
        Some(Span {
            kind: SpanKind::Diagram,
            first: open,
            last: close,
        })
    }

    /// Whether the caret or a selection touches lines `a..=b`.
    pub fn caret_in(&self, a: usize, b: usize) -> bool {
        if self.fresh {
            return false;
        }
        let (s, e) = match self.selection() {
            Some((x, y)) => (x.line, y.line),
            None => (self.cur.line, self.cur.line),
        };
        let main = s <= b && e >= a;
        main || self.extra.iter().any(|(_, p)| p.line >= a && p.line <= b)
    }

    /// Whether line `n` is inside a `%%` comment block.
    pub fn in_comment(&self, n: usize) -> bool {
        !self.fences.in_code(n) && self.comments.span(n).is_some()
    }

    /// The opening fence of the code block line `n` is in.
    pub fn fence_lang(&self, n: usize) -> &str {
        match self.fences.opener(n) {
            Some(o) => md::fence_lang(self.buf.line(o)),
            None => "",
        }
    }

    /// Whether a callout at `head` is shown folded.
    pub fn callout_folded(&self, head: usize) -> bool {
        let written = blocks::callout_head(self.buf.line(head))
            .and_then(|h| h.fold)
            .is_some_and(|open| !open);
        written != self.callouts_flipped.contains(&head)
    }

    pub fn toggle_callout(&mut self, head: usize) {
        if !self.callouts_flipped.remove(&head) {
            self.callouts_flipped.insert(head);
        }
    }

    // ---- folding --------------------------------------------------------------------

    /// The lines a fold at `n` would hide, when `n` can fold: the section
    /// under a heading, or the children of a list item.
    pub fn fold_range(&self, n: usize) -> Option<(usize, usize)> {
        let last = self.buf.last_line();
        if n >= last {
            return None;
        }
        match self.block(n) {
            Block::Heading { level, .. } => {
                let mut e = n;
                while e < last {
                    let next = e + 1;
                    if let Block::Heading { level: l2, .. } = self.block(next) {
                        if l2 <= level {
                            break;
                        }
                    }
                    e = next;
                }
                // Trailing blank lines stay visible, as they do in the editor.
                while e > n && self.buf.line(e).trim().is_empty() {
                    e -= 1;
                }
                (e > n).then_some((n + 1, e))
            }
            b if b.is_list() => {
                let ind = crate::text::indent_of(self.buf.line(n)).len();
                let mut e = n;
                while e < last {
                    let l = self.buf.line(e + 1);
                    if l.trim().is_empty() || crate::text::indent_of(l).len() <= ind {
                        break;
                    }
                    e += 1;
                }
                (e > n).then_some((n + 1, e))
            }
            _ => None,
        }
    }

    /// Fold or unfold at line `n` (or the line the caret is on).
    pub fn toggle_fold(&mut self, n: usize) -> bool {
        if self.folds.remove(&n) {
            return true;
        }
        if self.fold_range(n).is_some() {
            self.folds.insert(n);
            // The caret does not stay inside what was hidden.
            if let Some((a, b)) = self.fold_range(n) {
                if self.cur.line >= a && self.cur.line <= b {
                    self.place(Pos::new(n, self.buf.line_len(n)), false);
                }
            }
            return true;
        }
        false
    }

    /// Fold every heading and list item that can fold, or unfold them all.
    pub fn fold_all(&mut self, fold: bool) {
        self.folds.clear();
        if fold {
            for n in 0..self.buf.line_count() {
                if self.fold_range(n).is_some() && !self.fences.in_code(n) {
                    self.folds.insert(n);
                }
            }
        }
    }

    fn fold_ranges(&self) -> core::cell::Ref<'_, (u64, Vec<(usize, usize)>)> {
        let v = self.buf.version ^ (self.folds.len() as u64).rotate_left(48);
        if self.fold_cache.borrow().0 != v {
            let mut ranges: Vec<(usize, usize)> = self
                .folds
                .iter()
                .filter_map(|f| self.fold_range(*f))
                .collect();
            ranges.sort_unstable();
            *self.fold_cache.borrow_mut() = (v, ranges);
        }
        self.fold_cache.borrow()
    }

    /// Whether line `n` is hidden inside a fold.
    pub fn folded_away(&self, n: usize) -> bool {
        if self.folds.is_empty() {
            return false;
        }
        let r = self.fold_ranges();
        r.1.iter().any(|(a, b)| n >= *a && n <= *b)
    }

    // ---- footnotes -------------------------------------------------------------------

    /// The note's footnotes, numbered by first reference.
    pub fn footnotes(&self) -> alloc::rc::Rc<Footnotes> {
        if let Some((v, f)) = self.notes_cache.borrow().as_ref() {
            if *v == self.buf.version {
                return f.clone();
            }
        }
        let f = alloc::rc::Rc::new(Footnotes::collect(
            self.buf.line_count(),
            |n| self.buf.line(n),
            |n| self.fences.in_code(n) || self.fences.is_fence(n),
        ));
        *self.notes_cache.borrow_mut() = Some((self.buf.version, f.clone()));
        f
    }

    // ---- links into the note ---------------------------------------------------------

    /// The line a link's `#Heading` or `^block` names in this note.
    pub fn anchor_line(&self, anchor: &str) -> Option<usize> {
        let lines: Vec<&str> = (0..self.buf.line_count())
            .map(|n| self.buf.line(n))
            .collect();
        if let Some(id) = anchor.strip_prefix('^') {
            return blocks::block_lines(&lines, id).map(|(_, e)| e);
        }
        // `#A#B` names B under A; the last part decides.
        let last = anchor.rsplit('#').next().unwrap_or(anchor);
        blocks::heading_section(&lines, last).map(|(s, _)| s)
    }

    /// Put the caret at line `n` and show it near the top of the view, as
    /// following a link to a heading does.
    pub fn jump_to(&mut self, n: usize) {
        let n = n.min(self.buf.last_line());
        // Unfold whatever hides it.
        let hiding: Vec<usize> = self
            .folds
            .iter()
            .copied()
            .filter(|f| self.fold_range(*f).is_some_and(|(a, b)| n >= a && n <= b))
            .collect();
        for f in hiding {
            self.folds.remove(&f);
        }
        self.place(Pos::new(n, 0), false);
        self.top = n + 1;
        self.top_off = 0.0;
        self.reveal = false;
    }

    /// Move the selected text to `to` (dragging it), as one undo. False
    /// when there is nothing selected or `to` is inside the selection.
    pub fn move_selection(&mut self, to: Pos, now: u64) -> bool {
        let Some((a, b)) = self.selection() else {
            return false;
        };
        let to = self.buf.clamp(to);
        if to >= a && to <= b {
            return false;
        }
        let text = self.buf.slice(a, b);
        self.buf.begin_group(self.cur);
        // Insert first when the drop is after the text, so `a..b` stays put.
        let (start, end) = if to > b {
            let end = self.replace(to, to, &text, now);
            self.replace(a, b, "", now);
            let start = follow(to, a, b, a);
            let end = follow(end, a, b, a);
            (start, end)
        } else {
            self.replace(a, b, "", now);
            let end = self.replace(to, to, &text, now);
            (to, end)
        };
        self.buf.end_group(end);
        self.extra.clear();
        self.select_range(start, end);
        true
    }

    // ---- several carets ---------------------------------------------------------------

    /// Add a caret at `p` (Option-click), or take away the one there.
    pub fn add_caret(&mut self, p: Pos) {
        let p = self.buf.clamp(p);
        if let Some(i) = self.extra.iter().position(|(_, q)| *q == p) {
            self.extra.remove(i);
            return;
        }
        if p == self.cur {
            return;
        }
        self.extra.push((self.anchor, self.cur));
        self.anchor = None;
        self.cur = p;
    }

    /// Add a caret on the line above or below every caret, at the same
    /// column where the line is long enough.
    pub fn add_caret_vertical(&mut self, down: bool) {
        let mut all: Vec<Pos> = self.extra.iter().map(|(_, p)| *p).collect();
        all.push(self.cur);
        let edge = if down {
            all.iter().max().copied()
        } else {
            all.iter().min().copied()
        };
        let Some(edge) = edge else {
            return;
        };
        let line = if down {
            if edge.line >= self.buf.last_line() {
                return;
            }
            edge.line + 1
        } else {
            if edge.line == 0 {
                return;
            }
            edge.line - 1
        };
        let col = self.cur.col.min(self.buf.line_len(line));
        self.add_caret(Pos::new(line, col));
    }

    /// Cmd+D: select the word at the caret, or add the next place the
    /// selected text occurs as another selection.
    pub fn select_next_occurrence(&mut self) {
        let Some((a, b)) = self.selection() else {
            let (a, b) = self.word_at(self.cur);
            if a != b {
                self.select_range(a, b);
            }
            return;
        };
        if a.line != b.line {
            return;
        }
        let needle = self.buf.slice(a, b);
        let mut taken: Vec<Pos> = self.extra.iter().filter_map(|(an, _)| *an).collect();
        taken.push(a);
        let last = taken.iter().max().copied().unwrap_or(a);
        let lines = self.buf.line_count();
        for k in 0..=lines {
            let n = (last.line + k) % lines;
            let from = if k == 0 { last.col + 1 } else { 0 };
            let line = self.buf.line(n);
            if let Some(off) = line
                .get(from.min(line.len())..)
                .and_then(|r| r.find(needle.as_str()))
            {
                let at = Pos::new(n, from + off);
                if taken.contains(&at) {
                    return;
                }
                let end = Pos::new(n, at.col + needle.len());
                self.extra.push((self.anchor, self.cur));
                self.anchor = Some(at);
                self.cur = end;
                self.reveal = true;
                return;
            }
        }
    }

    pub fn has_extra(&self) -> bool {
        !self.extra.is_empty()
    }

    /// Run `f` at every caret, last in the note first, so an edit never
    /// moves a caret it has not reached; each earlier edit then moves the
    /// carets after it. One undo takes back all of it.
    pub fn each_caret(&mut self, now: u64, f: impl Fn(&mut Doc, u64)) {
        if self.extra.is_empty() {
            f(self, now);
            return;
        }
        let mut carets: Vec<(Option<Pos>, Pos)> = core::mem::take(&mut self.extra);
        carets.push((self.anchor, self.cur));
        carets.sort_by_key(|x| core::cmp::Reverse(x.1));
        carets.dedup_by(|x, y| x.1 == y.1);
        self.buf.begin_group(self.cur);
        let mut done: Vec<(Option<Pos>, Pos)> = Vec::new();
        for (an, p) in carets {
            self.anchor = an;
            self.cur = p;
            self.edit_log = Some(Vec::new());
            f(self, now);
            let log = self.edit_log.take().unwrap_or_default();
            for (a, b, e) in log {
                for (dan, dp) in done.iter_mut() {
                    *dp = follow(*dp, a, b, e);
                    if let Some(x) = dan.as_mut() {
                        *x = follow(*x, a, b, e);
                    }
                }
            }
            done.push((self.anchor, self.cur));
        }
        // The first caret in the note leads; the rest stay with it.
        done.sort_by_key(|x| x.1);
        done.dedup_by(|x, y| x.1 == y.1);
        let lead = done.remove(0);
        self.anchor = lead.0;
        self.cur = lead.1;
        self.extra = done;
        self.buf.end_group(self.cur);
    }

    /// Every caret and its selection, the main one first.
    pub fn carets(&self) -> Vec<(Option<Pos>, Pos)> {
        let mut v = alloc::vec![(self.anchor, self.cur)];
        v.extend(self.extra.iter().copied());
        v
    }

    pub fn mark_saved(&mut self) {
        self.buf.mark_saved();
        self.dirty_since = None;
        self.save_error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Doc {
        Doc::new("n.md", "n", text)
    }

    fn typed(d: &mut Doc, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                d.newline(0);
            } else {
                let mut b = [0u8; 4];
                d.insert(c.encode_utf8(&mut b), 0);
            }
        }
    }

    #[test]
    fn counts_follow_every_edit() {
        let mut d = doc("one two\nthree");
        assert_eq!((d.words, d.chars), (3, 12));
        d.place(Pos::new(1, 5), false);
        typed(&mut d, " four\nfive six");
        assert_eq!(d.text(), "one two\nthree four\nfive six");
        assert_eq!(d.words, 6);
        let mut fresh = doc(&d.text());
        fresh.recount();
        assert_eq!((d.words, d.chars), (fresh.words, fresh.chars));
        d.select_all();
        d.backspace(0);
        assert_eq!((d.words, d.chars), (0, 0));
        d.undo();
        assert_eq!(d.words, 6);
    }

    #[test]
    fn lists_continue_and_end_on_enter() {
        let mut d = doc("- first");
        d.place(Pos::new(0, 7), false);
        typed(&mut d, "\nsecond\n");
        assert_eq!(d.text(), "- first\n- second\n- ");
        d.newline(0);
        assert_eq!(d.text(), "- first\n- second\n");
        let mut o = doc("9. nine");
        o.place(Pos::new(0, 7), false);
        typed(&mut o, "\nten");
        assert_eq!(o.text(), "9. nine\n10. ten");
        let mut t = doc("\t- [x] done");
        t.place(Pos::new(0, 11), false);
        typed(&mut t, "\nnext");
        assert_eq!(t.text(), "\t- [x] done\n\t- [ ] next");
        let mut q = doc("> quote");
        q.place(Pos::new(0, 7), false);
        typed(&mut q, "\nmore");
        assert_eq!(q.text(), "> quote\n> more");
    }

    #[test]
    fn brackets_pair_and_links_complete() {
        let mut d = doc("");
        typed(&mut d, "see [[Pla");
        assert_eq!(d.text(), "see [[Pla]]");
        let (start, q) = d.link_query().unwrap();
        assert_eq!((start, q.as_str()), (6, "Pla"));
        d.accept_link("Plan", 0);
        assert_eq!(d.text(), "see [[Plan]]");
        assert_eq!(d.cur, Pos::new(0, 12));
        assert!(d.link_query().is_none());
        // Typing the closer steps over it; backspace takes a pair.
        let mut e = doc("");
        typed(&mut e, "(x)");
        assert_eq!(e.text(), "(x)");
        let mut f = doc("");
        typed(&mut f, "(");
        f.backspace(0);
        assert_eq!(f.text(), "");
    }

    #[test]
    fn backspace_at_an_item_start_takes_the_marker() {
        let mut d = doc("- item");
        d.place(Pos::new(0, 2), false);
        d.backspace(0);
        assert_eq!(d.text(), "item");
        let mut t = doc("  - [ ] task");
        t.place(Pos::new(0, 8), false);
        t.backspace(0);
        assert_eq!(t.text(), "  task");
    }

    #[test]
    fn tasks_toggle_and_bold_wraps() {
        let mut d = doc("- [ ] buy milk\n- plain\nwords");
        d.toggle_task(0, 0);
        assert_eq!(d.line(0), "- [x] buy milk");
        d.toggle_task(0, 0);
        assert_eq!(d.line(0), "- [ ] buy milk");
        d.toggle_task(1, 0);
        assert_eq!(d.line(1), "- [ ] plain");
        d.toggle_task(2, 0);
        assert_eq!(d.line(2), "- [ ] words");
        let mut b = doc("make this bold");
        b.select_range(Pos::new(0, 10), Pos::new(0, 14));
        b.toggle_wrap("**", 0);
        assert_eq!(b.text(), "make this **bold**");
        assert_eq!(b.selected_text(), "bold");
        b.toggle_wrap("**", 0);
        assert_eq!(b.text(), "make this bold");
        b.undo();
        assert_eq!(b.text(), "make this **bold**");
    }

    #[test]
    fn tab_indents_list_items_only() {
        let mut d = doc("- a\n- b");
        d.select_range(Pos::new(0, 1), Pos::new(1, 1));
        d.indent(false, 0);
        assert_eq!(d.text(), "\t- a\n\t- b");
        d.indent(true, 0);
        assert_eq!(d.text(), "- a\n- b");
        let mut p = doc("x");
        p.place(Pos::new(0, 1), false);
        p.indent(false, 0);
        assert_eq!(p.text(), "x\t");
    }

    #[test]
    fn fences_follow_typing() {
        let mut d = doc("a\nb");
        d.place(Pos::new(0, 1), false);
        typed(&mut d, "\n```");
        assert!(d.fences.is_fence(1));
        assert!(d.fences.in_code(2));
        typed(&mut d, "\nx\n```");
        assert!(!d.fences.in_code(4));
        assert_eq!(d.block(2), Block::Code);
    }

    #[test]
    fn words_and_headings() {
        let d = doc("# Title\nsome text\n## Part *two*\n```\n# not\n```");
        let h = d.headings();
        assert_eq!(h.len(), 2);
        assert_eq!(h[1].text, "Part two");
        assert_eq!(h[1].level, 2);
        let w = doc("hello wide_world again");
        assert_eq!(w.word_at(Pos::new(0, 8)), (Pos::new(0, 6), Pos::new(0, 16)));
        assert_eq!(w.word_left_of(Pos::new(0, 16)), Pos::new(0, 6));
        assert_eq!(w.word_right_of(Pos::new(0, 0)), Pos::new(0, 5));
        let links = doc("[[A]] and [[b|x]] and [[a]]").outgoing();
        assert_eq!(links, ["A", "b"]);
    }

    #[test]
    fn a_long_note_edits_without_rescanning() {
        let mut text = String::new();
        for i in 0..200_000 {
            text.push_str("Line ");
            crate::text::push_num(&mut text, i);
            text.push_str(" with some words in it\n");
        }
        let mut d = doc(&text);
        assert_eq!(d.buf.line_count(), 200_001);
        let words0 = d.words;
        d.place(Pos::new(100_000, 0), false);
        for _ in 0..2_000 {
            d.insert("a", 0);
        }
        d.insert(" ", 0);
        assert_eq!(d.words, words0 + 1);
    }
}

#[cfg(test)]
mod obsidian_blocks {
    use super::*;

    fn doc(text: &str) -> Doc {
        Doc::new("n.md", "n", text)
    }

    const NOTE: &str = "---\ntitle: x\n---\n# Head\n\n| a | b |\n| --- | :-: |\n| 1 | 2 |\n\n> [!tip]- Fold\n> body\n\n$$\nx^2\n$$\n![[pic.png]]\n```\n| not | table |\n| --- | --- |\n$$\n```\n%%\nhidden\n%%\n";

    #[test]
    fn spans_are_found_from_any_of_their_lines() {
        let d = doc(NOTE);
        let k = |n| d.span(n).map(|s| (s.kind, s.first, s.last));
        assert_eq!(k(1), Some((SpanKind::Front, 0, 2)));
        assert_eq!(k(3), None);
        assert_eq!(k(5), Some((SpanKind::Table, 5, 7)));
        assert_eq!(k(7), Some((SpanKind::Table, 5, 7)));
        assert_eq!(k(10), Some((SpanKind::Callout, 9, 10)));
        assert!(d.callout_folded(9));
        assert_eq!(k(13), Some((SpanKind::Math, 12, 14)));
        assert_eq!(k(15), Some((SpanKind::Embed, 15, 15)));
        assert_eq!(k(17), None, "a table in code is code");
        assert_eq!(k(19), None, "and so is $$");
        assert!(d.in_comment(22) && !d.in_comment(20));
        assert_eq!(d.fence_lang(17), "");
    }

    #[test]
    fn mermaid_flowcharts_are_blocks_and_other_code_is_not() {
        let d = doc("```mermaid\ngraph TD; A-->B;\n```\n```mermaid\nsequenceDiagram\nA->>B: hi\n```\n```js\nx\n```");
        let s = d.span(1).unwrap();
        assert_eq!((s.kind, s.first, s.last), (SpanKind::Diagram, 0, 2));
        assert_eq!(d.span(2).map(|s| s.first), Some(0));
        assert_eq!(
            d.span(4).map(|s| s.kind),
            Some(SpanKind::Diagram),
            "a sequence diagram is drawn"
        );
        assert_eq!(d.span(8), None);
    }

    #[test]
    fn blocks_follow_edits() {
        let mut d = doc(NOTE);
        // Typing a fence above everything turns the rest to code.
        d.place(Pos::new(3, 0), false);
        d.insert("```\n", 0);
        assert_eq!(d.span(14), None, "math inside code is code");
        d.undo();
        assert!(d.span(13).is_some());
        // Breaking the front matter's closing line ends the front matter.
        d.replace(Pos::new(2, 0), Pos::new(2, 3), "--", 0);
        assert_eq!(d.front, None);
        d.toggle_callout(9);
        assert!(!d.callout_folded(9));
    }

    #[test]
    fn headings_and_lists_fold() {
        let mut d = doc("# A\none\n## B\ntwo\n\n# C\n- item\n  - child\n  - child\n- next");
        assert_eq!(d.fold_range(0), Some((1, 3)));
        assert_eq!(d.fold_range(2), Some((3, 3)));
        assert_eq!(d.fold_range(6), Some((7, 8)));
        assert_eq!(d.fold_range(9), None);
        d.place(Pos::new(1, 1), false);
        assert!(d.toggle_fold(0));
        assert!(d.folded_away(1) && d.folded_away(3) && !d.folded_away(4));
        assert_eq!(d.cur, Pos::new(0, 3), "the caret leaves the fold");
        // A line typed above moves the fold with its heading.
        d.place(Pos::new(0, 0), false);
        d.insert("intro\n", 0);
        assert!(d.folds.contains(&1) && d.folded_away(2));
        d.jump_to(3);
        assert!(d.folds.is_empty(), "jumping into a fold opens it");
        d.fold_all(true);
        assert_eq!(d.folds.len(), 4);
    }

    #[test]
    fn several_carets_type_together() {
        let mut d = doc("one\ntwo\nthree");
        d.place(Pos::new(0, 3), false);
        d.add_caret(Pos::new(1, 3));
        d.add_caret(Pos::new(2, 5));
        d.each_caret(0, |d, now| d.insert("!", now));
        assert_eq!(d.text(), "one!\ntwo!\nthree!");
        d.each_caret(0, |d, now| d.newline(now));
        assert_eq!(d.text(), "one!\n\ntwo!\n\nthree!\n");
        assert_eq!(d.carets().len(), 3);
        d.undo();
        assert_eq!(d.text(), "one!\ntwo!\nthree!", "one undo for every caret");
        // Two carets on one line.
        let mut e = doc("ab");
        e.place(Pos::new(0, 0), false);
        e.add_caret(Pos::new(0, 1));
        e.each_caret(0, |d, now| d.insert("x", now));
        assert_eq!(e.text(), "xaxb");
        e.each_caret(0, |d, now| d.backspace(now));
        assert_eq!(e.text(), "ab");
        // Cmd+D selects the word, then each next one.
        let mut f = doc("cat dog cat\ncat");
        f.place(Pos::new(0, 1), false);
        f.select_next_occurrence();
        assert_eq!(f.selected_text(), "cat");
        f.select_next_occurrence();
        f.select_next_occurrence();
        assert_eq!(f.carets().len(), 3);
        f.each_caret(0, |d, now| d.insert("emu", now));
        assert_eq!(f.text(), "emu dog emu\nemu");
        let mut g = doc("aaa\nbbb\nccc");
        g.place(Pos::new(0, 2), false);
        g.add_caret_vertical(true);
        g.add_caret_vertical(true);
        assert_eq!(g.carets().len(), 3);
    }

    #[test]
    fn selected_text_moves_by_dragging() {
        let mut d = doc("one two three\nfour");
        d.select_range(Pos::new(0, 4), Pos::new(0, 8));
        assert!(d.move_selection(Pos::new(1, 4), 0));
        assert_eq!(d.text(), "one three\nfourtwo ");
        assert_eq!(d.selected_text(), "two ");
        assert!(d.move_selection(Pos::new(0, 0), 0));
        assert_eq!(d.text(), "two one three\nfour");
        assert!(!d.move_selection(Pos::new(0, 1), 0), "not into itself");
        d.undo();
        assert_eq!(d.text(), "one three\nfourtwo ");
    }

    #[test]
    fn links_find_headings_and_blocks() {
        let mut d = doc("# Top\ntext\n## Part *two*\nmore\nA line ^ref1\n");
        assert_eq!(d.anchor_line("part two"), Some(2));
        assert_eq!(d.anchor_line("Top#Part two"), Some(2));
        assert_eq!(d.anchor_line("^ref1"), Some(4));
        assert_eq!(d.anchor_line("nope"), None);
        d.jump_to(2);
        assert_eq!((d.cur, d.top), (Pos::new(2, 0), 3));
        let f = doc("See[^1] and ^[inline].\n\n[^1]: Note.").footnotes();
        assert_eq!(f.notes.len(), 2);
    }
}
