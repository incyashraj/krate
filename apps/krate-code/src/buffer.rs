//! The text of one file: a gap buffer of lines, with undo and redo.
//!
//! Lines are the unit because everything an editor does to code is shaped by
//! them -- drawing, line numbers, `j` and `k`, `dd`, highlighting state that
//! carries from one line to the next. The lines live in a gap buffer
//! ([`GapVec`]): two stacks meeting at the gap, so inserting or removing lines
//! near where the person is working costs a push or a pop however long the
//! file is, and only a jump to a far line pays for moving the gap. A line
//! itself is a `String`; code lines are short, so editing inside one is a
//! small move.
//!
//! Every change goes through [`Buffer::replace`], which records it. Changes
//! are grouped (`begin_group` / `end_group`) so one command -- `dd`, a whole
//! stretch of typing in insert mode -- undoes as one step, the way a person
//! thinks of it.

use alloc::string::String;
use alloc::vec::Vec;

/// A place in the text: a line, and a byte offset into that line that always
/// sits on a character boundary. `col == len` is the end of the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

impl Pos {
    pub const fn new(line: usize, col: usize) -> Self {
        Pos { line, col }
    }
}

/// A gap buffer: `front` holds the items before the gap in order, `back`
/// the items after it in reverse, so the gap moves by popping from one and
/// pushing onto the other.
#[derive(Debug, Clone, Default)]
pub struct GapVec<T> {
    front: Vec<T>,
    back: Vec<T>,
}

impl<T> GapVec<T> {
    pub fn new() -> Self {
        GapVec {
            front: Vec::new(),
            back: Vec::new(),
        }
    }

    pub fn from_vec(items: Vec<T>) -> Self {
        GapVec {
            front: items,
            back: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.front.len() + self.back.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Move the gap so it sits just before item `at`.
    fn move_gap(&mut self, at: usize) {
        let at = at.min(self.len());
        while self.front.len() > at {
            if let Some(item) = self.front.pop() {
                self.back.push(item);
            }
        }
        while self.front.len() < at {
            match self.back.pop() {
                Some(item) => self.front.push(item),
                None => break,
            }
        }
    }

    pub fn get(&self, i: usize) -> Option<&T> {
        if i < self.front.len() {
            self.front.get(i)
        } else {
            let k = i - self.front.len();
            if k >= self.back.len() {
                None
            } else {
                self.back.get(self.back.len() - 1 - k)
            }
        }
    }

    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        if i < self.front.len() {
            self.front.get_mut(i)
        } else {
            let k = i - self.front.len();
            if k >= self.back.len() {
                None
            } else {
                let n = self.back.len();
                self.back.get_mut(n - 1 - k)
            }
        }
    }

    pub fn insert(&mut self, at: usize, item: T) {
        self.move_gap(at);
        self.front.push(item);
    }

    pub fn remove(&mut self, at: usize) -> Option<T> {
        if at >= self.len() {
            return None;
        }
        self.move_gap(at);
        self.back.pop()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.front.iter().chain(self.back.iter().rev())
    }
}

/// One recorded change: at `at`, `removed` was taken out and `inserted` put
/// in. Undo is the same record with the two swapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub at: Pos,
    pub removed: String,
    pub inserted: String,
}

/// The changes one command made, undone and redone together.
#[derive(Debug, Clone)]
struct Group {
    edits: Vec<Edit>,
    before: Pos,
    after: Pos,
    id: u64,
}

/// What an edit did to the line count, for things pinned to lines
/// (breakpoints, problems) to follow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineShift {
    /// The first line whose number changed. When lines were removed, the
    /// lines `from .. from - delta` are the ones that went.
    pub from: usize,
    /// How many lines were added (positive) or removed (negative) there.
    pub delta: isize,
}

#[derive(Debug, Clone)]
pub struct Buffer {
    lines: GapVec<String>,
    done: Vec<Group>,
    undone: Vec<Group>,
    open: Option<Group>,
    next_id: u64,
    /// The history state when the file was last read or written.
    saved_id: u64,
    /// The lowest line changed since the highlighter last asked.
    dirty_from: Option<usize>,
    /// Line moves since the workspace last asked.
    shifts: Vec<LineShift>,
    /// The file used CRLF line ends; saving keeps them.
    pub crlf: bool,
    /// Bumped on every change, so a view can tell its cache is stale.
    pub version: u64,
}

impl Default for Buffer {
    fn default() -> Self {
        Buffer::new()
    }
}

/// A char's byte length, for stepping without a panicking index.
fn char_len_at(s: &str, col: usize) -> usize {
    s.get(col..)
        .and_then(|rest| rest.chars().next())
        .map(char::len_utf8)
        .unwrap_or(0)
}

impl Buffer {
    pub fn new() -> Self {
        let mut lines = GapVec::new();
        lines.insert(0, String::new());
        Buffer {
            lines,
            done: Vec::new(),
            undone: Vec::new(),
            open: None,
            next_id: 1,
            saved_id: 0,
            dirty_from: None,
            shifts: Vec::new(),
            crlf: false,
            version: 0,
        }
    }

    /// A buffer holding `text`, with no history and nothing unsaved.
    pub fn from_text(text: &str) -> Self {
        let crlf = text.contains("\r\n");
        let mut items: Vec<String> = Vec::new();
        for line in text.split('\n') {
            let line = if crlf {
                line.strip_suffix('\r').unwrap_or(line)
            } else {
                line
            };
            items.push(String::from(line));
        }
        if items.is_empty() {
            items.push(String::new());
        }
        let mut b = Buffer::new();
        b.lines = GapVec::from_vec(items);
        b.crlf = crlf;
        b
    }

    /// The whole text, with the line ends the file came with.
    pub fn text(&self) -> String {
        let sep = if self.crlf { "\r\n" } else { "\n" };
        let mut out = String::new();
        for (i, line) in self.lines.iter().enumerate() {
            if i > 0 {
                out.push_str(sep);
            }
            out.push_str(line);
        }
        out
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, i: usize) -> &str {
        self.lines.get(i).map(String::as_str).unwrap_or("")
    }

    pub fn line_len(&self, i: usize) -> usize {
        self.line(i).len()
    }

    pub fn last_line(&self) -> usize {
        self.line_count().saturating_sub(1)
    }

    pub fn end(&self) -> Pos {
        let l = self.last_line();
        Pos::new(l, self.line_len(l))
    }

    /// `p` moved onto the text: an existing line, a character boundary.
    pub fn clamp(&self, p: Pos) -> Pos {
        let line = p.line.min(self.last_line());
        let s = self.line(line);
        let mut col = p.col.min(s.len());
        while col > 0 && !s.is_char_boundary(col) {
            col -= 1;
        }
        Pos::new(line, col)
    }

    /// The character at `p`; the end of a line reads as `'\n'` except at the
    /// end of the last line, which is the end of the text.
    pub fn char_at(&self, p: Pos) -> Option<char> {
        let s = self.line(p.line);
        if p.col < s.len() {
            s.get(p.col..).and_then(|r| r.chars().next())
        } else if p.line < self.last_line() {
            Some('\n')
        } else {
            None
        }
    }

    /// The next character position, crossing line ends.
    pub fn next_pos(&self, p: Pos) -> Option<Pos> {
        let s = self.line(p.line);
        if p.col < s.len() {
            Some(Pos::new(p.line, p.col + char_len_at(s, p.col).max(1)))
        } else if p.line < self.last_line() {
            Some(Pos::new(p.line + 1, 0))
        } else {
            None
        }
    }

    /// The previous character position, crossing line ends.
    pub fn prev_pos(&self, p: Pos) -> Option<Pos> {
        if p.col > 0 {
            let s = self.line(p.line);
            let col = s
                .get(..p.col)
                .and_then(|h| h.char_indices().next_back())
                .map(|(i, _)| i)
                .unwrap_or(0);
            Some(Pos::new(p.line, col))
        } else if p.line > 0 {
            Some(Pos::new(p.line - 1, self.line_len(p.line - 1)))
        } else {
            None
        }
    }

    /// The start of the character before `col` on `line`, or 0.
    pub fn prev_col(&self, line: usize, col: usize) -> usize {
        self.line(line)
            .get(..col)
            .and_then(|h| h.char_indices().next_back())
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    /// The position after the character at `col` on `line`.
    pub fn next_col(&self, line: usize, col: usize) -> usize {
        let s = self.line(line);
        (col + char_len_at(s, col)).min(s.len())
    }

    /// The start of the last character of a line (where a normal-mode cursor
    /// sits at `$`), 0 for an empty line.
    pub fn last_char_col(&self, line: usize) -> usize {
        let len = self.line_len(line);
        self.prev_col(line, len)
    }

    /// The column of the first non-blank character of a line.
    pub fn first_non_blank(&self, line: usize) -> usize {
        let s = self.line(line);
        s.char_indices()
            .find(|(_, c)| !c.is_whitespace())
            .map(|(i, _)| i)
            .unwrap_or(s.len())
    }

    /// The text between two positions (in either order).
    pub fn slice(&self, a: Pos, b: Pos) -> String {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let a = self.clamp(a);
        let b = self.clamp(b);
        let mut out = String::new();
        if a.line == b.line {
            out.push_str(self.line(a.line).get(a.col..b.col).unwrap_or(""));
            return out;
        }
        out.push_str(self.line(a.line).get(a.col..).unwrap_or(""));
        for l in a.line + 1..b.line {
            out.push('\n');
            out.push_str(self.line(l));
        }
        out.push('\n');
        out.push_str(self.line(b.line).get(..b.col).unwrap_or(""));
        out
    }

    /// Where `text` would end if inserted at `at`.
    pub fn end_after(at: Pos, text: &str) -> Pos {
        match text.rfind('\n') {
            None => Pos::new(at.line, at.col + text.len()),
            Some(i) => {
                let newlines = text.matches('\n').count();
                Pos::new(at.line + newlines, text.len() - i - 1)
            }
        }
    }

    fn raw_delete(&mut self, a: Pos, b: Pos) -> String {
        let removed = self.slice(a, b);
        if removed.is_empty() {
            return removed;
        }
        if a.line == b.line {
            if let Some(line) = self.lines.get_mut(a.line) {
                line.replace_range(a.col..b.col, "");
            }
        } else {
            let tail = String::from(self.line(b.line).get(b.col..).unwrap_or(""));
            // Remove whole lines from the far end inward so the gap walks once.
            let mut l = b.line;
            while l > a.line {
                self.lines.remove(l);
                l -= 1;
            }
            if let Some(line) = self.lines.get_mut(a.line) {
                line.truncate(a.col);
                line.push_str(&tail);
            }
        }
        removed
    }

    fn raw_insert(&mut self, at: Pos, text: &str) -> Pos {
        if text.is_empty() {
            return at;
        }
        let mut pieces = text.split('\n');
        let first = pieces.next().unwrap_or("");
        let rest: Vec<&str> = pieces.collect();
        if rest.is_empty() {
            if let Some(line) = self.lines.get_mut(at.line) {
                line.insert_str(at.col, first);
            }
            return Pos::new(at.line, at.col + first.len());
        }
        let tail = match self.lines.get_mut(at.line) {
            Some(line) => {
                let tail = String::from(line.get(at.col..).unwrap_or(""));
                line.truncate(at.col);
                line.push_str(first);
                tail
            }
            None => String::new(),
        };
        let mut l = at.line;
        let n = rest.len();
        let mut end = at;
        for (k, piece) in rest.iter().enumerate() {
            l += 1;
            let mut s = String::from(*piece);
            if k + 1 == n {
                end = Pos::new(l, s.len());
                s.push_str(&tail);
            }
            self.lines.insert(l, s);
        }
        end
    }

    fn touched(&mut self, line: usize) {
        self.dirty_from = Some(self.dirty_from.map_or(line, |d| d.min(line)));
        self.version = self.version.wrapping_add(1);
    }

    fn apply(&mut self, at: Pos, remove_to: Pos, text: &str) -> (String, Pos) {
        let removed = self.raw_delete(at, remove_to);
        let end = self.raw_insert(at, text);
        let lines_removed = removed.matches('\n').count() as isize;
        let lines_added = text.matches('\n').count() as isize;
        if lines_added != lines_removed {
            // An edit at a line's very start moves that line too.
            self.shifts.push(LineShift {
                from: if at.col == 0 { at.line } else { at.line + 1 },
                delta: lines_added - lines_removed,
            });
        }
        self.touched(at.line);
        (removed, end)
    }

    /// Replace the text from `a` to `b` with `text`, recording it. Returns
    /// where the inserted text ends.
    pub fn replace(&mut self, a: Pos, b: Pos, text: &str) -> Pos {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let a = self.clamp(a);
        let b = self.clamp(b);
        let (removed, end) = self.apply(a, b, text);
        if removed.is_empty() && text.is_empty() {
            return end;
        }
        let edit = Edit {
            at: a,
            removed,
            inserted: String::from(text),
        };
        self.undone.clear();
        match self.open.as_mut() {
            Some(group) => {
                // Typing a run of characters is one edit, not one per key.
                if let Some(last) = group.edits.last_mut() {
                    if last.removed.is_empty()
                        && edit.removed.is_empty()
                        && Buffer::end_after(last.at, &last.inserted) == edit.at
                    {
                        last.inserted.push_str(&edit.inserted);
                        group.after = end;
                        return end;
                    }
                }
                group.edits.push(edit);
                group.after = end;
            }
            None => {
                let id = self.take_id();
                self.done.push(Group {
                    edits: alloc::vec![edit],
                    before: a,
                    after: end,
                    id,
                });
            }
        }
        end
    }

    pub fn insert(&mut self, at: Pos, text: &str) -> Pos {
        self.replace(at, at, text)
    }

    pub fn delete(&mut self, a: Pos, b: Pos) -> String {
        let (a2, b2) = if a <= b { (a, b) } else { (b, a) };
        let removed = self.slice(a2, b2);
        self.replace(a2, b2, "");
        removed
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Start a group: every change until `end_group` undoes as one step.
    /// Nested calls join the open group.
    pub fn begin_group(&mut self, cursor: Pos) {
        if self.open.is_none() {
            let id = self.take_id();
            self.open = Some(Group {
                edits: Vec::new(),
                before: cursor,
                after: cursor,
                id,
            });
        }
    }

    pub fn end_group(&mut self, cursor: Pos) {
        if let Some(mut group) = self.open.take() {
            if !group.edits.is_empty() {
                group.after = cursor;
                self.done.push(group);
            }
        }
    }

    pub fn group_open(&self) -> bool {
        self.open.is_some()
    }

    /// Undo the last group. Returns where the cursor goes, or `None` when
    /// there is nothing to undo.
    pub fn undo(&mut self) -> Option<Pos> {
        let open_cursor = self.open.as_ref().map(|g| g.after);
        if let Some(c) = open_cursor {
            self.end_group(c);
        }
        let group = self.done.pop()?;
        for edit in group.edits.iter().rev() {
            let end = Buffer::end_after(edit.at, &edit.inserted);
            self.apply(edit.at, end, &edit.removed);
        }
        let to = group.before;
        self.undone.push(group);
        Some(self.clamp(to))
    }

    pub fn redo(&mut self) -> Option<Pos> {
        let group = self.undone.pop()?;
        let mut cursor = group.after;
        for edit in group.edits.iter() {
            let end = Buffer::end_after(edit.at, &edit.removed);
            let (_, e) = self.apply(edit.at, end, &edit.inserted);
            cursor = e;
        }
        let _ = cursor;
        let to = group.after;
        self.done.push(group);
        Some(self.clamp(to))
    }

    pub fn undo_depth(&self) -> usize {
        self.done.len() + usize::from(self.open.as_ref().is_some_and(|g| !g.edits.is_empty()))
    }

    pub fn redo_depth(&self) -> usize {
        self.undone.len()
    }

    fn state_id(&self) -> u64 {
        match &self.open {
            Some(g) if !g.edits.is_empty() => g.id,
            _ => self.done.last().map(|g| g.id).unwrap_or(0),
        }
    }

    /// Whether the text differs from what was last read or written.
    pub fn modified(&self) -> bool {
        self.state_id() != self.saved_id
    }

    pub fn mark_saved(&mut self) {
        let open_cursor = self.open.as_ref().map(|g| g.after);
        if let Some(c) = open_cursor {
            self.end_group(c);
        }
        self.saved_id = self.state_id();
    }

    /// The lowest line changed since the last call, for the highlighter.
    pub fn take_dirty_from(&mut self) -> Option<usize> {
        self.dirty_from.take()
    }

    /// Line moves since the last call, for things pinned to lines.
    pub fn take_shifts(&mut self) -> Vec<LineShift> {
        core::mem::take(&mut self.shifts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(text: &str) -> Buffer {
        Buffer::from_text(text)
    }

    #[test]
    fn gap_vec_inserts_and_removes_anywhere() {
        let mut g: GapVec<u32> = GapVec::new();
        for i in 0..10 {
            g.insert(g.len(), i);
        }
        g.insert(3, 100);
        g.insert(0, 200);
        assert_eq!(
            g.iter().copied().collect::<Vec<_>>(),
            [200, 0, 1, 2, 100, 3, 4, 5, 6, 7, 8, 9]
        );
        assert_eq!(g.remove(5), Some(3));
        assert_eq!(g.get(5), Some(&4));
        assert_eq!(g.get(11), None);
        assert_eq!(g.remove(0), Some(200));
        assert_eq!(g.len(), 10);
        *g.get_mut(9).unwrap() = 99;
        assert_eq!(g.get(9), Some(&99));
    }

    #[test]
    fn text_round_trips_including_trailing_newline_and_crlf() {
        for text in ["", "a", "a\nb", "a\nb\n", "\n\n", "fn main() {\n    x\n}\n"] {
            assert_eq!(b(text).text(), text);
        }
        let crlf = b("a\r\nb\r\n");
        assert!(crlf.crlf);
        assert_eq!(crlf.line(0), "a");
        assert_eq!(crlf.text(), "a\r\nb\r\n");
    }

    #[test]
    fn insert_within_and_across_lines() {
        let mut x = b("hello world");
        let end = x.insert(Pos::new(0, 5), ",");
        assert_eq!(end, Pos::new(0, 6));
        assert_eq!(x.text(), "hello, world");
        let end = x.insert(Pos::new(0, 6), "\nthere\n");
        assert_eq!(end, Pos::new(2, 0));
        assert_eq!(x.text(), "hello,\nthere\n world");
        assert_eq!(x.line_count(), 3);
    }

    #[test]
    fn delete_within_and_across_lines() {
        let mut x = b("one\ntwo\nthree");
        assert_eq!(x.delete(Pos::new(0, 1), Pos::new(2, 2)), "ne\ntwo\nth");
        assert_eq!(x.text(), "oree");
        let mut y = b("ab\ncd");
        assert_eq!(y.delete(Pos::new(0, 2), Pos::new(1, 0)), "\n");
        assert_eq!(y.text(), "abcd");
    }

    #[test]
    fn undo_and_redo_restore_text_and_cursor() {
        let mut x = b("fn a() {}\n");
        x.begin_group(Pos::new(0, 0));
        x.insert(Pos::new(0, 3), "b");
        x.insert(Pos::new(0, 4), "c");
        x.end_group(Pos::new(0, 5));
        x.delete(Pos::new(0, 0), Pos::new(0, 3));
        assert_eq!(x.text(), "bca() {}\n");
        assert_eq!(x.undo(), Some(Pos::new(0, 0)));
        assert_eq!(x.text(), "fn bca() {}\n");
        assert_eq!(x.undo(), Some(Pos::new(0, 0)));
        assert_eq!(x.text(), "fn a() {}\n");
        assert_eq!(x.undo(), None);
        assert_eq!(x.redo(), Some(Pos::new(0, 5)));
        assert_eq!(x.text(), "fn bca() {}\n");
        assert_eq!(x.redo(), Some(Pos::new(0, 0)));
        assert_eq!(x.text(), "bca() {}\n");
        assert_eq!(x.redo(), None);
    }

    #[test]
    fn a_new_change_drops_the_redo_list() {
        let mut x = b("abc");
        x.delete(Pos::new(0, 0), Pos::new(0, 1));
        x.undo();
        assert_eq!(x.redo_depth(), 1);
        x.insert(Pos::new(0, 0), "z");
        assert_eq!(x.redo_depth(), 0);
        assert_eq!(x.text(), "zabc");
    }

    #[test]
    fn undo_across_line_joins_and_splits() {
        let mut x = b("a\nb\nc");
        x.begin_group(Pos::new(0, 0));
        x.delete(Pos::new(0, 1), Pos::new(2, 0));
        x.insert(Pos::new(0, 1), "\nX\nY\n");
        x.end_group(Pos::new(0, 0));
        assert_eq!(x.text(), "a\nX\nY\nc");
        x.undo();
        assert_eq!(x.text(), "a\nb\nc");
        x.redo();
        assert_eq!(x.text(), "a\nX\nY\nc");
    }

    #[test]
    fn modified_follows_save_undo_and_redo() {
        let mut x = b("abc");
        assert!(!x.modified());
        x.insert(Pos::new(0, 3), "d");
        assert!(x.modified());
        x.mark_saved();
        assert!(!x.modified());
        x.undo();
        assert!(x.modified());
        x.redo();
        assert!(!x.modified());
    }

    #[test]
    fn typing_in_one_group_coalesces() {
        let mut x = b("");
        x.begin_group(Pos::new(0, 0));
        let mut at = Pos::new(0, 0);
        for ch in ["h", "e", "l", "l", "o"] {
            at = x.insert(at, ch);
        }
        x.end_group(at);
        assert_eq!(x.text(), "hello");
        assert_eq!(x.undo_depth(), 1);
        x.undo();
        assert_eq!(x.text(), "");
    }

    #[test]
    fn positions_step_over_multibyte_characters_and_line_ends() {
        let x = b("aé\nb");
        assert_eq!(x.next_pos(Pos::new(0, 1)), Some(Pos::new(0, 3)));
        assert_eq!(x.char_at(Pos::new(0, 3)), Some('\n'));
        assert_eq!(x.next_pos(Pos::new(0, 3)), Some(Pos::new(1, 0)));
        assert_eq!(x.prev_pos(Pos::new(1, 0)), Some(Pos::new(0, 3)));
        assert_eq!(x.prev_pos(Pos::new(0, 3)), Some(Pos::new(0, 1)));
        assert_eq!(x.char_at(Pos::new(1, 1)), None);
        assert_eq!(x.clamp(Pos::new(0, 2)), Pos::new(0, 1));
        assert_eq!(x.last_char_col(0), 1);
    }

    #[test]
    fn line_shifts_report_added_and_removed_lines() {
        let mut x = b("a\nb\nc\nd");
        x.take_shifts();
        x.insert(Pos::new(1, 0), "new\n");
        assert_eq!(x.take_shifts(), [LineShift { from: 1, delta: 1 }]);
        x.delete(Pos::new(0, 1), Pos::new(2, 1));
        assert_eq!(x.take_shifts(), [LineShift { from: 1, delta: -2 }]);
        assert_eq!(x.take_dirty_from(), Some(0));
        assert_eq!(x.take_dirty_from(), None);
    }

    #[test]
    fn ten_thousand_lines_edit_fast() {
        let mut text = String::new();
        for i in 0..10_000 {
            text.push_str("    let value_");
            text.push_str(&alloc::format!("{i}"));
            text.push_str(" = compute(i) + 1;\n");
        }
        let mut x = b(&text);
        assert_eq!(x.line_count(), 10_001);
        // Typing near the end, then near the start, then in the middle.
        for i in 0..2_000 {
            let line = [9_000, 10, 5_000][i % 3];
            x.insert(Pos::new(line, 0), "x");
            x.insert(Pos::new(line, 1), "\n");
        }
        assert_eq!(x.line_count(), 12_001);
        for _ in 0..4_000 {
            x.undo();
        }
        assert_eq!(x.text(), text);
    }
}
