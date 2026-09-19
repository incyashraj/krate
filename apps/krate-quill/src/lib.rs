//! Quill -- a text editor built by hand to find out what the runtime cannot do.
//!
//! CP-B of Plan/Proof-Checkpoints-2026-09-16.md. CP-A proved the runtime can
//! carry a game; this asks whether it can carry the other shape of demanding
//! app -- one somebody works in all day, where the test is not frames per
//! second but whether typing stutters, whether a big file scrolls, and whether
//! undo is instant.
//!
//! So: a real editor. A line buffer with insert, delete, newline and backspace;
//! a caret that moves by character, word, line and page; selection; undo and
//! redo with coalesced typing runs; find with next/previous; syntax colouring
//! for Rust; line numbers; a status bar; open and save through the dialogs; and
//! settings that persist.
//!
//! It draws on a CANVAS rather than using a text-area widget, on purpose. An
//! editor owns its buffer -- that is what makes undo, find and highlighting
//! possible -- and the WIT offers two different text events depending on
//! whether the host draws its own widgets (`text-input`, append) or lowers to
//! OS controls (`text-changed`, replace). Owning the buffer and taking raw key
//! and text events is the only design that behaves the same on both.
//!
//! `#![no_std]` with `alloc`: the buffer is a Vec of Strings, which is the one
//! place this app allocates. The frame loop itself does not.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use krate::bindings::krate::fs::files as rawfs;
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, types as gfx};
use krate::ui::{dialog, events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 900.0;
const HEIGHT: f32 = 560.0;

/// Monospace-ish metrics. The face is proportional, so the editor measures a
/// representative run once and lays out on that -- see `Metrics`.
const LINE_H: f32 = 17.0;
const FONT: f32 = 13.0;
const GUTTER_W: f32 = 52.0;
const PAD_X: f32 = 8.0;
const TOP: f32 = 26.0;
const STATUS_H: f32 = 22.0;

/// How many undo steps are kept. Each holds a whole line, not a whole file, so
/// this is cheap.
const MAX_UNDO: usize = 256;

// ------------------------------------------------------------------- state

#[derive(Clone)]
struct Edit {
    /// Line index this edit applies to.
    line: usize,
    /// The line's text BEFORE the edit, for undo.
    before: String,
    /// The line's text AFTER, for redo.
    after: String,
    /// Caret column before, so undo puts the caret back where it was.
    col_before: usize,
    /// Whether this edit inserted a line (undo removes it) or removed one
    /// (undo restores it).
    kind: EditKind,
}

#[derive(Clone, Copy, PartialEq)]
enum EditKind {
    Replace,
    InsertLine,
    RemoveLine,
}

struct Editor {
    lines: Vec<String>,
    caret_line: usize,
    caret_col: usize,
    /// Selection anchor. Equal to the caret means no selection.
    anchor_line: usize,
    anchor_col: usize,
    scroll: usize,
    /// The column the caret "wants" when moving vertically through short
    /// lines, so up/down through a ragged file returns to where you were.
    goal_col: usize,

    undo: Vec<Edit>,
    redo: Vec<Edit>,
    /// Typing runs coalesce: consecutive inserts on one line become one undo
    /// step until the caret moves elsewhere or a second passes.
    last_edit_line: usize,
    last_edit_at: u64,

    name: String,
    dirty: bool,
    /// The chosen-file token for save-in-place, when the file came from a
    /// dialog. Empty means "no file yet".
    token: String,

    find: String,
    finding: bool,
    find_hits: usize,

    status: String,
    status_until: u64,

    /// Character advance and a few measured widths, so layout matches what
    /// the host will actually draw.
    adv: f32,
}

impl Editor {
    fn new() -> Self {
        Editor {
            lines: Vec::new(),
            caret_line: 0,
            caret_col: 0,
            anchor_line: 0,
            anchor_col: 0,
            scroll: 0,
            goal_col: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            last_edit_line: usize::MAX,
            last_edit_at: 0,
            name: String::from("untitled"),
            dirty: false,
            token: String::new(),
            find: String::new(),
            finding: false,
            find_hits: 0,
            status: String::new(),
            status_until: 0,
            adv: 7.0,
        }
    }

    fn line(&self, i: usize) -> &str {
        self.lines.get(i).map(|s| s.as_str()).unwrap_or("")
    }

    fn line_len(&self, i: usize) -> usize {
        self.lines.get(i).map(|s| s.chars().count()).unwrap_or(0)
    }

    fn has_selection(&self) -> bool {
        self.anchor_line != self.caret_line || self.anchor_col != self.caret_col
    }

    fn clear_selection(&mut self) {
        self.anchor_line = self.caret_line;
        self.anchor_col = self.caret_col;
    }

    fn say(&mut self, msg: &str, now: u64) {
        self.status.clear();
        self.status.push_str(msg);
        self.status_until = now + 3_000_000_000;
    }

    // ------------------------------------------------------------- editing

    /// Record an edit for undo, coalescing a typing run into one step.
    fn record(&mut self, e: Edit, now: u64) {
        let same_run = e.kind == EditKind::Replace
            && self.last_edit_line == e.line
            && now.saturating_sub(self.last_edit_at) < 900_000_000;
        self.last_edit_line = e.line;
        self.last_edit_at = now;
        self.redo.clear();
        if same_run {
            // Extend the run: keep the ORIGINAL `before`, take the new
            // `after`, so undo jumps the whole word rather than one letter.
            if let Some(top) = self.undo.last_mut() {
                if top.kind == EditKind::Replace && top.line == e.line {
                    top.after = e.after;
                    return;
                }
            }
        }
        if self.undo.len() >= MAX_UNDO {
            // `remove(0)` shifts the whole ring on every edit once it is
            // full. Draining the front block amortises it to nothing.
            self.undo.drain(0..MAX_UNDO / 4);
        }
        self.undo.push(e);
    }

    fn insert_char(&mut self, ch: char, now: u64) {
        if self.has_selection() {
            self.delete_selection(now);
        }
        while self.lines.len() <= self.caret_line {
            self.lines.push(String::new());
        }
        let before = self.lines[self.caret_line].clone();
        let col = self.caret_col;
        let byte = char_to_byte(&self.lines[self.caret_line], col);
        self.lines[self.caret_line].insert(byte, ch);
        let after = self.lines[self.caret_line].clone();
        self.record(
            Edit {
                line: self.caret_line,
                before,
                after,
                col_before: col,
                kind: EditKind::Replace,
            },
            now,
        );
        self.caret_col += 1;
        self.goal_col = self.caret_col;
        self.clear_selection();
        self.dirty = true;
    }

    fn insert_newline(&mut self, now: u64) {
        if self.has_selection() {
            self.delete_selection(now);
        }
        while self.lines.len() <= self.caret_line {
            self.lines.push(String::new());
        }
        let byte = char_to_byte(&self.lines[self.caret_line], self.caret_col);
        let tail = self.lines[self.caret_line].split_off(byte);
        // Keep the indent: an editor that drops it is unusable for code.
        let indent: String = tail_indent(&self.lines[self.caret_line]);
        let mut new_line = indent.clone();
        new_line.push_str(&tail);
        self.lines.insert(self.caret_line + 1, new_line);
        self.record(
            Edit {
                line: self.caret_line + 1,
                before: String::new(),
                after: self.lines[self.caret_line + 1].clone(),
                col_before: self.caret_col,
                kind: EditKind::InsertLine,
            },
            now,
        );
        self.caret_line += 1;
        self.caret_col = indent.chars().count();
        self.goal_col = self.caret_col;
        self.clear_selection();
        self.dirty = true;
        self.last_edit_line = usize::MAX;
    }

    fn backspace(&mut self, now: u64) {
        if self.has_selection() {
            self.delete_selection(now);
            return;
        }
        if self.caret_col > 0 {
            let before = self.lines[self.caret_line].clone();
            let byte = char_to_byte(&self.lines[self.caret_line], self.caret_col - 1);
            self.lines[self.caret_line].remove(byte);
            let after = self.lines[self.caret_line].clone();
            self.record(
                Edit {
                    line: self.caret_line,
                    before,
                    after,
                    col_before: self.caret_col,
                    kind: EditKind::Replace,
                },
                now,
            );
            self.caret_col -= 1;
        } else if self.caret_line > 0 {
            // Join with the line above.
            let removed = self.lines.remove(self.caret_line);
            self.caret_line -= 1;
            self.caret_col = self.line_len(self.caret_line);
            let before = self.lines[self.caret_line].clone();
            self.lines[self.caret_line].push_str(&removed);
            self.record(
                Edit {
                    line: self.caret_line + 1,
                    before,
                    after: removed,
                    col_before: self.caret_col,
                    kind: EditKind::RemoveLine,
                },
                now,
            );
            self.last_edit_line = usize::MAX;
        }
        self.goal_col = self.caret_col;
        self.clear_selection();
        self.dirty = true;
    }

    fn delete_selection(&mut self, now: u64) {
        // Normalise so start <= end.
        let (sl, sc, el, ec) = self.selection_range();
        if sl == el {
            let before = self.lines[sl].clone();
            let a = char_to_byte(&self.lines[sl], sc);
            let b = char_to_byte(&self.lines[sl], ec);
            self.lines[sl].replace_range(a..b, "");
            let after = self.lines[sl].clone();
            self.record(
                Edit {
                    line: sl,
                    before,
                    after,
                    col_before: sc,
                    kind: EditKind::Replace,
                },
                now,
            );
        } else {
            // Multi-line: keep the head of the first and the tail of the last.
            let before = self.lines[sl].clone();
            let a = char_to_byte(&self.lines[sl], sc);
            let tail_byte = char_to_byte(&self.lines[el], ec);
            let tail = self.lines[el][tail_byte..].to_string();
            self.lines[sl].truncate(a);
            self.lines[sl].push_str(&tail);
            // Drain the range in one pass. Removing one line at a time is
            // O(n) each, so deleting a large selection was quadratic -- the
            // thing that makes an editor freeze on a big file.
            let last = el.min(self.lines.len().saturating_sub(1));
            if last >= sl + 1 {
                self.lines.drain(sl + 1..=last);
            }
            let after = self.lines[sl].clone();
            self.record(
                Edit {
                    line: sl,
                    before,
                    after,
                    col_before: sc,
                    kind: EditKind::Replace,
                },
                now,
            );
            self.last_edit_line = usize::MAX;
        }
        self.caret_line = sl;
        self.caret_col = sc;
        self.clear_selection();
        self.dirty = true;
    }

    fn selection_range(&self) -> (usize, usize, usize, usize) {
        let a = (self.anchor_line, self.anchor_col);
        let b = (self.caret_line, self.caret_col);
        if a <= b {
            (a.0, a.1, b.0, b.1)
        } else {
            (b.0, b.1, a.0, a.1)
        }
    }

    fn undo_one(&mut self) {
        let Some(e) = self.undo.pop() else {
            return;
        };
        match e.kind {
            EditKind::Replace => {
                if e.line < self.lines.len() {
                    self.lines[e.line] = e.before.clone();
                    self.caret_line = e.line;
                    self.caret_col = e.col_before.min(self.line_len(e.line));
                }
            }
            EditKind::InsertLine => {
                if e.line < self.lines.len() {
                    self.lines.remove(e.line);
                    self.caret_line = e.line.saturating_sub(1);
                    self.caret_col = self.line_len(self.caret_line);
                }
            }
            EditKind::RemoveLine => {
                let at = e.line.min(self.lines.len());
                self.lines.insert(at, e.after.clone());
                if at > 0 {
                    self.lines[at - 1] = e.before.clone();
                }
                self.caret_line = at;
                self.caret_col = 0;
            }
        }
        self.redo.push(e);
        self.clear_selection();
        self.dirty = true;
        self.last_edit_line = usize::MAX;
    }

    fn redo_one(&mut self) {
        let Some(e) = self.redo.pop() else {
            return;
        };
        match e.kind {
            EditKind::Replace => {
                if e.line < self.lines.len() {
                    self.lines[e.line] = e.after.clone();
                    self.caret_line = e.line;
                    self.caret_col = self.line_len(e.line);
                }
            }
            EditKind::InsertLine => {
                let at = e.line.min(self.lines.len());
                self.lines.insert(at, e.after.clone());
                self.caret_line = at;
                self.caret_col = self.line_len(at);
            }
            EditKind::RemoveLine => {
                if e.line < self.lines.len() {
                    self.lines.remove(e.line);
                    self.caret_line = e.line.saturating_sub(1);
                }
            }
        }
        self.undo.push(e);
        self.clear_selection();
        self.dirty = true;
        self.last_edit_line = usize::MAX;
    }

    // ------------------------------------------------------------ movement

    fn rows_visible(&self) -> usize {
        (((HEIGHT - TOP - STATUS_H) / LINE_H) as usize).max(1)
    }

    fn scroll_to_caret(&mut self) {
        let rows = self.rows_visible();
        if self.caret_line < self.scroll {
            self.scroll = self.caret_line;
        } else if self.caret_line >= self.scroll + rows {
            self.scroll = self.caret_line + 1 - rows;
        }
    }

    fn move_caret(&mut self, dl: isize, dc: isize, select: bool) {
        if !select && self.has_selection() {
            // Collapse to the near edge, like every editor.
            let (sl, sc, el, ec) = self.selection_range();
            if dl < 0 || dc < 0 {
                self.caret_line = sl;
                self.caret_col = sc;
            } else {
                self.caret_line = el;
                self.caret_col = ec;
            }
            self.clear_selection();
            if dl == 0 {
                return;
            }
        }
        if dc != 0 {
            let len = self.line_len(self.caret_line);
            if dc > 0 {
                if self.caret_col < len {
                    self.caret_col += 1;
                } else if self.caret_line + 1 < self.lines.len() {
                    self.caret_line += 1;
                    self.caret_col = 0;
                }
            } else if self.caret_col > 0 {
                self.caret_col -= 1;
            } else if self.caret_line > 0 {
                self.caret_line -= 1;
                self.caret_col = self.line_len(self.caret_line);
            }
            self.goal_col = self.caret_col;
        }
        if dl != 0 {
            let target = self.caret_line as isize + dl;
            self.caret_line = target.clamp(0, self.lines.len().saturating_sub(1) as isize) as usize;
            self.caret_col = self.goal_col.min(self.line_len(self.caret_line));
        }
        if !select {
            self.clear_selection();
        }
        self.scroll_to_caret();
    }

    /// Jump to the next word boundary, which is what ctrl-arrow means.
    fn move_word(&mut self, forward: bool, select: bool) {
        let line = self.line(self.caret_line).to_string();
        let chars: Vec<char> = line.chars().collect();
        let mut i = self.caret_col;
        if forward {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
        } else {
            while i > 0 && chars[i - 1].is_whitespace() {
                i -= 1;
            }
            while i > 0 && !chars[i - 1].is_whitespace() {
                i -= 1;
            }
        }
        self.caret_col = i;
        self.goal_col = i;
        if !select {
            self.clear_selection();
        }
        self.scroll_to_caret();
    }

    // ---------------------------------------------------------------- find

    fn find_next(&mut self, backward: bool) {
        if self.find.is_empty() {
            return;
        }
        let needle = self.find.to_lowercase();
        let n = self.lines.len();
        if n == 0 {
            return;
        }
        for step in 1..=n {
            let idx = if backward {
                (self.caret_line + n - (step % n)) % n
            } else {
                (self.caret_line + step) % n
            };
            let hay = self.lines[idx].to_lowercase();
            if let Some(byte) = hay.find(&needle) {
                self.caret_line = idx;
                self.caret_col = hay[..byte].chars().count();
                self.anchor_line = idx;
                self.anchor_col = self.caret_col + needle.chars().count();
                self.goal_col = self.caret_col;
                self.scroll_to_caret();
                return;
            }
        }
    }

    fn count_hits(&mut self) {
        self.find_hits = 0;
        if self.find.is_empty() {
            return;
        }
        let needle = self.find.to_lowercase();
        for l in &self.lines {
            let hay = l.to_lowercase();
            let mut from = 0;
            while let Some(at) = hay[from..].find(&needle) {
                self.find_hits += 1;
                from += at + needle.len().max(1);
                if from >= hay.len() {
                    break;
                }
            }
        }
    }
}

/// The leading whitespace of a line, so a newline keeps the indent.
fn tail_indent(s: &str) -> String {
    s.chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// Char index to byte index. The buffer is UTF-8 and the caret counts
/// characters, so every mutation needs this; indexing bytes directly would
/// split a multi-byte character and panic.
fn char_to_byte(s: &str, col: usize) -> usize {
    s.char_indices()
        .nth(col)
        .map(|(b, _)| b)
        .unwrap_or_else(|| s.len())
}

// ------------------------------------------------------------ highlighting

/// A token class, which is all the colouring this needs.
#[derive(Clone, Copy, PartialEq)]
enum Tok {
    Plain,
    Keyword,
    Str,
    Comment,
    Number,
    Type,
}

const KEYWORDS: [&str; 28] = [
    "fn", "let", "mut", "if", "else", "match", "for", "while", "loop", "return", "struct", "enum",
    "impl", "pub", "use", "mod", "const", "static", "trait", "self", "Self", "as", "in", "break",
    "continue", "move", "ref", "where",
];

/// Classify one line into runs. Deliberately simple: a real editor would keep
/// per-line state for block comments, and this is the place that would grow.
fn classify(line: &str, out: &mut Vec<(usize, usize, Tok)>) {
    out.clear();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            out.push((i, chars.len(), Tok::Comment));
            return;
        }
        if c == '"' {
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    i += 2;
                    continue;
                }
                if chars[i] == '"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            out.push((start, i.min(chars.len()), Tok::Str));
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
            }
            out.push((start, i, Tok::Number));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let tok = if KEYWORDS.contains(&word.as_str()) {
                Tok::Keyword
            } else if word.chars().next().is_some_and(|c| c.is_uppercase()) {
                Tok::Type
            } else {
                Tok::Plain
            };
            if tok != Tok::Plain {
                out.push((start, i, tok));
            }
            continue;
        }
        i += 1;
    }
}

fn tok_colour(t: Tok) -> gfx::Color {
    match t {
        Tok::Keyword => rgb(0.78, 0.55, 0.95),
        Tok::Str => rgb(0.55, 0.85, 0.55),
        Tok::Comment => rgb(0.42, 0.47, 0.55),
        Tok::Number => rgb(0.95, 0.72, 0.45),
        Tok::Type => rgb(0.45, 0.80, 0.95),
        Tok::Plain => rgb(0.84, 0.87, 0.92),
    }
}

// ------------------------------------------------------------------ colour

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> gfx::Rect {
    gfx::Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

// -------------------------------------------------------------------- draw

fn draw(e: &Editor, canvas: u64, now: u64, runs: &mut Vec<(usize, usize, Tok)>) -> Result<(), gfx::GfxError> {
    canvas2d::clear(canvas, rgb(0.09, 0.10, 0.13))?;

    // Title bar.
    canvas2d::fill_rect(canvas, rect(0.0, 0.0, WIDTH, TOP), rgb(0.13, 0.15, 0.19))?;
    let mut title = String::new();
    if e.dirty {
        title.push_str("• ");
    }
    title.push_str(&e.name);
    canvas2d::draw_text(
        canvas,
        &title,
        gfx::Point { x: PAD_X, y: 17.0 },
        12.5,
        rgb(0.86, 0.90, 0.96),
    )?;
    canvas2d::draw_text(
        canvas,
        "^O open   ^S save   ^F find   ^Z undo   ^Y redo",
        gfx::Point { x: WIDTH - 330.0, y: 17.0 },
        11.0,
        rgb(0.45, 0.52, 0.62),
    )?;

    // Gutter.
    canvas2d::fill_rect(
        canvas,
        rect(0.0, TOP, GUTTER_W, HEIGHT - TOP - STATUS_H),
        rgb(0.11, 0.12, 0.16),
    )?;

    let rows = e.rows_visible();
    let text_x = GUTTER_W + PAD_X;

    // Selection, drawn under the text.
    if e.has_selection() {
        let (sl, sc, el, ec) = e.selection_range();
        for row in 0..rows {
            let li = e.scroll + row;
            if li < sl || li > el {
                continue;
            }
            let y = TOP + row as f32 * LINE_H;
            let from = if li == sl { sc } else { 0 };
            let to = if li == el { ec } else { e.line_len(li) };
            let line = e.line(li);
            let x0 = text_x + col_x(canvas, line, from);
            let w = col_x(canvas, line, to) - col_x(canvas, line, from);
            canvas2d::fill_rect(
                canvas,
                rect(x0, y + 2.0, w.max(2.0), LINE_H - 2.0),
                rgba(0.30, 0.45, 0.75, 0.45),
            )?;
        }
    }

    // The caret's line gets a wash, which is how you find your place.
    if e.caret_line >= e.scroll && e.caret_line < e.scroll + rows {
        let y = TOP + (e.caret_line - e.scroll) as f32 * LINE_H;
        canvas2d::fill_rect(
            canvas,
            rect(GUTTER_W, y + 1.0, WIDTH - GUTTER_W, LINE_H),
            rgba(1.0, 1.0, 1.0, 0.035),
        )?;
    }

    // Lines.
    for row in 0..rows {
        let li = e.scroll + row;
        if li >= e.lines.len() {
            break;
        }
        let y = TOP + row as f32 * LINE_H + 12.0;

        // Line number, right-aligned in the gutter.
        let num = usize_str(li + 1);
        unsafe { CALLS += 1 };
        let m = canvas2d::measure_text(canvas, &num, 11.0)?;
        let ink = if li == e.caret_line {
            rgb(0.70, 0.76, 0.86)
        } else {
            rgb(0.34, 0.38, 0.46)
        };
        canvas2d::draw_text(
            canvas,
            &num,
            gfx::Point {
                x: GUTTER_W - 8.0 - m.width,
                y,
            },
            11.0,
            ink,
        )?;

        let line = e.line(li);
        classify(line, runs);

        // Draw run by run, advancing by what the host says each piece is
        // actually wide.
        //
        // The first version drew the whole line plain and then painted the
        // classified runs over it at `col * adv`. That is wrong twice: the
        // face is PROPORTIONAL, so a column index times a constant is not a
        // pixel position -- `i` and `W` differ by about four times -- and
        // overpainting left both copies visible where the estimate drifted.
        // Measuring each piece is the only thing that lines up.
        let mut x = text_x;
        let mut at = 0usize;
        for &(from, to, tok) in runs.iter() {
            if from > at {
                let piece: String = line.chars().skip(at).take(from - at).collect();
                canvas2d::draw_text(
                    canvas,
                    &piece,
                    gfx::Point { x, y },
                    FONT,
                    tok_colour(Tok::Plain),
                )?;
                x += measured(canvas, &piece, FONT);
            }
            let piece: String = line.chars().skip(from).take(to - from).collect();
            canvas2d::draw_text(canvas, &piece, gfx::Point { x, y }, FONT, tok_colour(tok))?;
            x += measured(canvas, &piece, FONT);
            at = to;
        }
        let total = line.chars().count();
        if at < total {
            let piece: String = line.chars().skip(at).collect();
            canvas2d::draw_text(
                canvas,
                &piece,
                gfx::Point { x, y },
                FONT,
                tok_colour(Tok::Plain),
            )?;
        }
    }

    // Caret, blinking.
    if e.caret_line >= e.scroll && e.caret_line < e.scroll + rows && (now / 500_000_000) % 2 == 0 {
        let y = TOP + (e.caret_line - e.scroll) as f32 * LINE_H;
        let x = text_x + col_x(canvas, e.line(e.caret_line), e.caret_col);
        canvas2d::fill_rect(canvas, rect(x, y + 2.0, 1.6, LINE_H - 3.0), rgb(0.55, 0.85, 1.0))?;
    }

    // Status bar.
    let sy = HEIGHT - STATUS_H;
    canvas2d::fill_rect(canvas, rect(0.0, sy, WIDTH, STATUS_H), rgb(0.13, 0.15, 0.19))?;
    let mut left = String::new();
    left.push_str("Ln ");
    left.push_str(&usize_str(e.caret_line + 1));
    left.push_str(", Col ");
    left.push_str(&usize_str(e.caret_col + 1));
    left.push_str("   ");
    left.push_str(&usize_str(e.lines.len()));
    left.push_str(" lines");
    canvas2d::draw_text(
        canvas,
        &left,
        gfx::Point { x: PAD_X, y: sy + 15.0 },
        11.0,
        rgb(0.60, 0.67, 0.78),
    )?;

    if e.finding {
        let mut f = String::from("find: ");
        f.push_str(&e.find);
        f.push_str("   ");
        f.push_str(&usize_str(e.find_hits));
        f.push_str(" hits   enter=next  esc=done");
        canvas2d::draw_text(
            canvas,
            &f,
            gfx::Point { x: 250.0, y: sy + 15.0 },
            11.0,
            rgb(0.95, 0.80, 0.45),
        )?;
    } else if !e.status.is_empty() && now < e.status_until {
        canvas2d::draw_text(
            canvas,
            &e.status,
            gfx::Point { x: 250.0, y: sy + 15.0 },
            11.0,
            rgb(0.50, 0.85, 0.60),
        )?;
    }

    let p0 = clock::monotonic_nanos();
    let r = canvas2d::present(canvas);
    let p1 = clock::monotonic_nanos();
    unsafe {
        PRESENT_NS += p1.saturating_sub(p0);
        PRESENT_N += 1;
    }
    r
}

/// Where column `col` sits, in pixels from the text origin.
///
/// Measured rather than `col * adv`: the face is proportional, so a constant
/// advance puts the caret and the selection in the wrong place on any line
/// with a capital or a punctuation run. This is the same reason the runs
/// above are drawn by measuring.
static mut PRESENT_NS: u64 = 0;
static mut PRESENT_N: u64 = 0;
static mut CALLS: u32 = 0;
static mut MAX_CALLS: u32 = 0;

/// Width cache: piece -> pixels.
///
/// `measure_text` crosses the sandbox boundary and lays the text out, and an
/// editor asks the same questions over and over -- "let", "fn", four spaces of
/// indent, the same identifier on fifty lines. Measured at 270 calls per frame
/// before this, which is what put a third of the frames over 16.7 ms.
static mut WIDTHS: Option<alloc::collections::BTreeMap<alloc::string::String, f32>> = None;

fn measured(canvas: u64, piece: &str, size: f32) -> f32 {
    if piece.is_empty() {
        return 0.0;
    }
    // Only the body font is cached; the gutter uses a different size and is
    // one short call per row.
    if size != FONT {
        unsafe { CALLS += 1 };
        return canvas2d::measure_text(canvas, piece, size)
            .map(|m| m.width)
            .unwrap_or(0.0);
    }
    unsafe {
        let map = &raw mut WIDTHS;
        if (*map).is_none() {
            *map = Some(alloc::collections::BTreeMap::new());
        }
        if let Some(m) = (*map).as_ref().and_then(|m| m.get(piece)) {
            return *m;
        }
        CALLS += 1;
        let w = canvas2d::measure_text(canvas, piece, size)
            .map(|m| m.width)
            .unwrap_or(0.0);
        if let Some(m) = (*map).as_mut() {
            // A cache that grows without bound is a leak wearing a hat.
            if m.len() < 4096 {
                m.insert(alloc::string::String::from(piece), w);
            }
        }
        w
    }
}

fn col_x(canvas: u64, line: &str, col: usize) -> f32 {
    if col == 0 {
        return 0.0;
    }
    let piece: String = line.chars().take(col).collect();
    measured(canvas, &piece, FONT)
}

fn usize_str(v: usize) -> String {
    let mut s = String::new();
    let mut digits = [0u8; 20];
    let mut n = 0;
    let mut v = v;
    if v == 0 {
        s.push('0');
        return s;
    }
    while v > 0 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        s.push(digits[n] as char);
    }
    s
}

// -------------------------------------------------------------- file / io

fn open_dialog(e: &mut Editor, win: u64, now: u64) {
    match dialog::open_file(win, "Open a file", "*") {
        Ok(Some(chosen)) => {
            // A chosen file is a TOKEN, never a path: the pick is the grant.
            // `open_chosen` is in the raw bindings rather than the SDK's `fs`
            // module, which is the one rough edge in this whole flow.
            match rawfs::open_chosen(&chosen.token, krate::fs::OpenMode::Read) {
                Ok(file) => {
                    let mut all = Vec::new();
                    loop {
                        match file.read(65536) {
                            Ok(chunk) if !chunk.is_empty() => all.extend_from_slice(&chunk),
                            _ => break,
                        }
                    }
                    let text = String::from_utf8_lossy(&all).into_owned();
                    e.lines = text.lines().map(|l| l.to_string()).collect();
                    if e.lines.is_empty() {
                        e.lines.push(String::new());
                    }
                    e.name = chosen.name.clone();
                    e.token = chosen.token.clone();
                    e.caret_line = 0;
                    e.caret_col = 0;
                    e.scroll = 0;
                    e.clear_selection();
                    e.undo.clear();
                    e.redo.clear();
                    e.dirty = false;
                    let mut msg = String::from("opened ");
                    msg.push_str(&usize_str(e.lines.len()));
                    msg.push_str(" lines");
                    e.say(&msg, now);
                }
                Err(_) => e.say("could not open that file", now),
            }
        }
        Ok(None) => {}
        Err(_) => e.say("the open dialog is not available", now),
    }
}

fn save(e: &mut Editor, now: u64) {
    let mut text = String::new();
    for (i, l) in e.lines.iter().enumerate() {
        if i > 0 {
            text.push('\n');
        }
        text.push_str(l);
    }
    text.push('\n');

    if !e.token.is_empty() {
        if let Ok(file) = rawfs::open_chosen(&e.token, krate::fs::OpenMode::Write) {
            if file.write(text.as_bytes()).is_ok() {
                e.dirty = false;
                e.say("saved", now);
                return;
            }
        }
    }
    // There is no `save-file` dialog to ask "where?" -- the capability is
    // declarable and consent-worded but the WIT has no such function (K-393).
    // Saving in place through the token the file was opened with works; a
    // file that was never opened has nowhere to go, and saying so is better
    // than a button that silently does nothing.
    e.say("no file open -- use ctrl-O first (save-as needs K-393)", now);
}

// -------------------------------------------------------------------- main

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 0.0,
            padding: 0.0,
            text: None,
            place: None,
            box_: None,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

const STATE_KEY: &str = "quill.state";

/// Remember where you were: the file name and the caret line.
///
/// Stored as `name\nline`. Plain text so it survives a format change and
/// needs no parser.
fn save_state(e: &Editor) {
    let mut v = String::new();
    v.push_str(&e.name);
    v.push('\n');
    v.push_str(&usize_str(e.caret_line));
    let _ = krate::store::set_text(STATE_KEY, &v);
}

fn load_state(e: &mut Editor) {
    let Ok(Some(v)) = krate::store::get_text(STATE_KEY) else {
        return;
    };
    let mut it = v.split('\n');
    if let Some(name) = it.next() {
        if !name.is_empty() {
            e.status.clear();
            e.status.push_str("last session: ");
            e.status.push_str(name);
        }
    }
    if let Some(line) = it.next() {
        if let Ok(n) = line.parse::<usize>() {
            e.caret_line = n;
        }
    }
}

/// Say which step failed before exiting. A bare `return 1` leaves a checker
/// with nothing but "failed at run" -- learned building Relay (K-392).
fn fail(step: &[u8]) -> i32 {
    let out = stdio::stdout();
    let _ = out.write(b"quill: failed at ");
    let _ = out.write(step);
    let _ = out.write(b"\n");
    1
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|b| *b == b'\n')
            .any(|a| a == b"quick" || a == b"--quick");

        let win = match window::create(
            "Quill",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) {
            Ok(w) => w,
            Err(_) => return fail(b"window::create"),
        };
        if window::show(win).is_err() {
            return fail(b"window::show");
        }
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            return fail(b"set_root");
        }
        if tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        )
        .is_err()
        {
            return fail(b"upsert canvas");
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => return fail(b"canvas2d::bind"),
        };
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        let mut e = Editor::new();
        load_state(&mut e);

        // Measure the face once: the host draws proportionally, so the editor
        // asks how wide a representative run is and lays out on that rather
        // than guessing a character width.
        if let Ok(m) = canvas2d::measure_text(canvas, "MMMMMMMMMMMMMMMMMMMM", FONT) {
            e.adv = m.width / 20.0;
        }

        // Start with something to look at and to measure against.
        for line in SAMPLE.lines() {
            e.lines.push(line.to_string());
        }
        e.name = String::from("welcome.rs");

        let mut runs: Vec<(usize, usize, Tok)> = Vec::new();

        // `stress` builds a 50,000-line buffer, which is the CP-B bar: does a
        // big file open, scroll and edit without stuttering. Built in-process
        // rather than read from disk so the measurement is of the EDITOR, not
        // of the filesystem.
        let stress = raw
            .as_bytes()
            .split(|b| *b == b'\n')
            .any(|a| a == b"stress");
        if stress {
            let t0 = clock::monotonic_nanos();
            for i in 0..50_000usize {
                let mut l = String::new();
                l.push_str("    let value_");
                l.push_str(&usize_str(i));
                l.push_str(" = compute(\"row ");
                l.push_str(&usize_str(i));
                l.push_str("\", 0); // generated line");
                e.lines.push(l);
            }
            let t1 = clock::monotonic_nanos();
            e.name = String::from("stress-50000.rs");

            // Scroll the whole file, drawing every frame, and time it.
            let mut worst = 0u64;
            let mut worst_at = 0usize;
            let mut over_16 = 0u32;
            let rows = e.rows_visible();
            let mut frame_count = 0u64;
            let t2 = clock::monotonic_nanos();
            while e.scroll + rows < e.lines.len() {
                let f0 = clock::monotonic_nanos();
                unsafe { CALLS = 0; PRESENT_NS = 0; PRESENT_N = 0 };
                let _ = draw(&e, canvas, f0, &mut runs);
                let calls = unsafe { CALLS };
                if calls > unsafe { MAX_CALLS } {
                    unsafe { MAX_CALLS = calls };
                }
                let f1 = clock::monotonic_nanos();
                // Subtract present(): the host sleeps the remainder of a
                // 16.7 ms budget there on purpose, so counting it measures
                // the pacing rather than the editor.
                let took = f1
                    .saturating_sub(f0)
                    .saturating_sub(unsafe { PRESENT_NS });
                if took > worst {
                    worst = took;
                    worst_at = e.scroll;
                }
                if took > 16_700_000 {
                    over_16 += 1;
                }
                frame_count += 1;
                e.scroll += rows;
            }
            let t3 = clock::monotonic_nanos();

            // And an edit in the middle, which is what "undo is instant" means.
            e.caret_line = 25_000;
            e.caret_col = 0;
            // Move the anchor with the caret. Without this the editor thinks
            // everything from line 0 to 25,000 is selected, and the first
            // keystroke deletes it -- which is exactly what happened, and is
            // why this measured 170 ms and lost half the file.
            e.clear_selection();
            let e0 = clock::monotonic_nanos();
            for ch in "hello".chars() {
                e.insert_char(ch, e0);
            }
            let e1 = clock::monotonic_nanos();
            e.undo_one();
            let e2 = clock::monotonic_nanos();

            let out = stdio::stdout();
            let mut m = String::from("quill-stress: lines=");
            m.push_str(&usize_str(e.lines.len()));
            m.push_str(" build=");
            m.push_str(&usize_str((t1 - t0) as usize / 1_000_000));
            m.push_str("ms scroll_frames=");
            m.push_str(&usize_str(frame_count as usize));
            m.push_str(" total=");
            m.push_str(&usize_str((t3 - t2) as usize / 1_000_000));
            m.push_str("ms worst_frame=");
            m.push_str(&usize_str(worst as usize / 1_000));
            m.push_str("us present_avg_us=");
            let (pn, pns) = unsafe { (PRESENT_N.max(1), PRESENT_NS) };
            m.push_str(&usize_str((pns / pn / 1_000) as usize));
            m.push_str(" measure_calls_per_frame=");
            m.push_str(&usize_str(unsafe { MAX_CALLS } as usize));
            m.push_str(" over16ms=");
            m.push_str(&usize_str(over_16 as usize));
            m.push_str(" worst_at_line=");
            m.push_str(&usize_str(worst_at));
            m.push_str(" type5=");
            m.push_str(&usize_str((e1 - e0) as usize / 1_000));
            m.push_str("us undo=");
            m.push_str(&usize_str((e2 - e1) as usize / 1_000));
            m.push_str("us\n");
            let _ = out.write(m.as_bytes());
            return 0;
        }

        let start = clock::monotonic_nanos();
        let _ = draw(&e, canvas, start, &mut runs);

        if quick {
            let out = stdio::stdout();
            let mut msg = String::from("quill: ");
            msg.push_str(&usize_str(e.lines.len()));
            msg.push_str(" lines, adv=");
            msg.push_str(&usize_str((e.adv * 100.0) as usize));
            msg.push('\n');
            let _ = out.write(msg.as_bytes());
            return 0;
        }

        // Frame-time histogram, as in Relay: an average hides the stutter that
        // makes an editor unusable, so keep the distribution.
        let mut buckets = [0u32; 34];
        let mut frames: u32 = 0;
        let mut worst_ms: f32 = 0.0;
        let mut last = clock::monotonic_nanos();

        loop {
            let now = clock::monotonic_nanos();
            let dt = now.saturating_sub(last) as f32 / 1_000_000.0;
            last = now;

            while let Some(ev) = events::poll() {
                match ev {
                    types::Event::CloseRequested(id) => {
                        let _ = window::close(id);
                        save_state(&e);
                        report(frames, &buckets, worst_ms);
                        return 0;
                    }
                    types::Event::TextInput(s) => {
                        for ch in s.chars() {
                            if ch == '\n' || ch == '\r' {
                                if e.finding {
                                    e.find_next(false);
                                } else {
                                    e.insert_newline(now);
                                }
                            } else if ch >= ' ' {
                                if e.finding {
                                    e.find.push(ch);
                                    e.count_hits();
                                } else {
                                    e.insert_char(ch, now);
                                }
                            }
                        }
                        e.scroll_to_caret();
                    }
                    types::Event::Key(k) if k.pressed => {
                        let ctrl = k.modifiers.control || k.modifiers.meta;
                        let shift = k.modifiers.shift;
                        match k.key.as_str() {
                            "ArrowLeft" if ctrl => e.move_word(false, shift),
                            "ArrowRight" if ctrl => e.move_word(true, shift),
                            "ArrowLeft" => e.move_caret(0, -1, shift),
                            "ArrowRight" => e.move_caret(0, 1, shift),
                            "ArrowUp" => e.move_caret(-1, 0, shift),
                            "ArrowDown" => e.move_caret(1, 0, shift),
                            "PageUp" => {
                                let r = e.rows_visible() as isize;
                                e.move_caret(-r, 0, shift);
                            }
                            "PageDown" => {
                                let r = e.rows_visible() as isize;
                                e.move_caret(r, 0, shift);
                            }
                            "Home" => {
                                e.caret_col = 0;
                                e.goal_col = 0;
                                if !shift {
                                    e.clear_selection();
                                }
                            }
                            "End" => {
                                e.caret_col = e.line_len(e.caret_line);
                                e.goal_col = e.caret_col;
                                if !shift {
                                    e.clear_selection();
                                }
                            }
                            "Backspace" => {
                                if e.finding {
                                    e.find.pop();
                                    e.count_hits();
                                } else {
                                    e.backspace(now);
                                    e.scroll_to_caret();
                                }
                            }
                            "Enter" | "Return" => {
                                if e.finding {
                                    e.find_next(shift);
                                } else {
                                    e.insert_newline(now);
                                    e.scroll_to_caret();
                                }
                            }
                            "Escape" => {
                                e.finding = false;
                                e.clear_selection();
                            }
                            "Tab" if !e.finding => {
                                for _ in 0..4 {
                                    e.insert_char(' ', now);
                                }
                            }
                            "z" | "Z" if ctrl => {
                                e.undo_one();
                                e.scroll_to_caret();
                            }
                            "y" | "Y" if ctrl => {
                                e.redo_one();
                                e.scroll_to_caret();
                            }
                            "s" | "S" if ctrl => save(&mut e, now),
                            "o" | "O" if ctrl => open_dialog(&mut e, win, now),
                            "f" | "F" if ctrl => {
                                e.finding = true;
                                e.find.clear();
                                e.find_hits = 0;
                            }
                            "a" | "A" if ctrl => {
                                e.anchor_line = 0;
                                e.anchor_col = 0;
                                e.caret_line = e.lines.len().saturating_sub(1);
                                e.caret_col = e.line_len(e.caret_line);
                            }
                            _ => {}
                        }
                    }
                    types::Event::Wheel(w) => {
                        let lines = (-w.dy / 40.0) as isize * 3;
                        let target = e.scroll as isize + lines;
                        let max = e.lines.len().saturating_sub(1) as isize;
                        e.scroll = target.clamp(0, max) as usize;
                    }
                    _ => {}
                }
            }

            if draw(&e, canvas, now, &mut runs).is_err() {
                break;
            }

            if dt > worst_ms {
                worst_ms = dt;
            }
            let b = if dt >= 33.0 { 33 } else { dt as usize };
            buckets[b] = buckets[b].saturating_add(1);
            frames = frames.saturating_add(1);
        }

        save_state(&e);
        report(frames, &buckets, worst_ms);
        0
    }
}

/// Print the frame-time distribution, the CP-B "typing never stutters"
/// measurement.
fn report(frames: u32, buckets: &[u32; 34], worst_ms: f32) {
    if frames == 0 {
        return;
    }
    let out = stdio::stdout();
    let mut s = String::from("quill-frames: n=");
    s.push_str(&usize_str(frames as usize));
    for (label, pct) in [(" p50=", 50u32), (" p95=", 95), (" p99=", 99)] {
        let target = (frames as u64 * pct as u64 / 100) as u32;
        let mut seen = 0u32;
        let mut ms = 33usize;
        for (i, c) in buckets.iter().enumerate() {
            seen = seen.saturating_add(*c);
            if seen >= target {
                ms = i;
                break;
            }
        }
        s.push_str(label);
        s.push_str(&usize_str(ms));
        s.push_str("ms");
    }
    s.push_str(" worst=");
    s.push_str(&usize_str(worst_ms as usize));
    s.push_str("ms\n");
    let _ = out.write(s.as_bytes());
}

const SAMPLE: &str = r#"// Quill -- a text editor running on Krate.
//
// Everything here is editable. Try it:
//   ctrl-O opens a file        ctrl-S saves it
//   ctrl-F finds               enter / shift-enter steps through hits
//   ctrl-Z undoes              ctrl-Y redoes
//   ctrl-arrow moves by word   shift-arrow selects

fn main() {
    let greeting = "hello from a sandboxed editor";
    let mut count = 0;
    for word in greeting.split(' ') {
        count += word.len();
    }
    println!("{} characters across the line", count);
}

struct Buffer {
    lines: Vec<String>,
    caret: usize,
}

impl Buffer {
    fn new() -> Self {
        Buffer { lines: Vec::new(), caret: 0 }
    }

    // Undo keeps whole lines rather than whole files, which is why a
    // 50,000-line document costs the same to edit as a short one.
    fn insert(&mut self, ch: char) {
        self.lines[self.caret].push(ch);
    }
}
"#;

krate::export!(Component);
