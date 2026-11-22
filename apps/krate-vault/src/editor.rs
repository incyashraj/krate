//! One open note: its text, its caret and selection, and the edits a
//! Markdown editor makes beyond plain typing -- lists that continue on
//! Enter, brackets that close themselves, `[[` that offers note names,
//! checkboxes, bold and italic around a selection.
//!
//! Every change goes through [`Doc::replace`], which keeps the word and
//! character counts and the fence index up to date by looking only at the
//! lines it touched, so a keystroke in a 200,000-line note costs what it
//! costs in a short one.

use crate::buffer::{Buffer, Pos};
use crate::md::{self, Block, Fences};
use crate::text::{chars_in, words_in};
use alloc::string::String;
use alloc::vec::Vec;

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
        };
        d.recount();
        d
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
        let buf = &self.buf;
        self.fences
            .edit(a.line, b.line - a.line + 1, end.line - a.line + 1, |n| {
                buf.line(n)
            });
        self.dirty_since.get_or_insert(now);
        self.reveal = true;
        end
    }

    fn set_cursor(&mut self, p: Pos, extend: bool) {
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
        self.anchor = Some(Pos::new(0, 0));
        self.cur = self.buf.end();
        self.goal_x = None;
    }

    pub fn select_range(&mut self, a: Pos, b: Pos) {
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
