//! Laying out and drawing a note, one line at a time.
//!
//! Only the lines on screen are ever laid out. A line's layout -- where it
//! wraps, which syntax is hidden, where each styled run sits -- depends only
//! on its own text and a few facts about it (its block, whether the caret is
//! on it, the width), so it is cached by those: scrolling lays out only the
//! lines coming into view, and typing lays out only the line typed in. A
//! 200,000-line note draws the same few dozen lines a short one does.
//!
//! The view's position is a row and an offset into it (`Doc::top`,
//! `Doc::top_off`), never a pixel offset from the top of the note, so
//! nothing ever needs the height of the whole note.

use super::look::{self, fill, r, round, Rgba, Sizes, Theme};
use crate::buffer::Pos;
use crate::editor::{Doc, Mode};
use crate::md::{self, Block, Target};
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::canvas2d;
use krate::layout::Rect;
#[allow(unused_imports)]
use krate::prelude::*;

/// One styled stretch of a laid-out line, on one row.
#[derive(Clone, Copy)]
pub struct Run {
    pub row: u16,
    pub x: f32,
    pub w: f32,
    pub start: usize,
    pub end: usize,
    pub st: md::Style,
    pub hidden: bool,
    pub target: Option<Target>,
    pub size: f32,
    pub weight: u16,
    pub italic: bool,
    pub mono: bool,
}

impl Run {
    fn style(&self) -> krate::gfx::types::TextStyle {
        look::style(self.weight, self.italic, self.mono)
    }
}

#[derive(Clone, Copy)]
pub struct RowL {
    pub top: f32,
    pub h: f32,
    pub base: f32,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Deco {
    None,
    Bullet,
    Task(bool),
    Quote,
    Code,
    Fence { open: bool, shown: bool },
    Rule,
    Title,
}

pub struct Lay {
    pub h: f32,
    pub rows: Vec<RowL>,
    pub runs: Vec<Run>,
    pub deco: Deco,
    /// Where the line's text starts (after list indentation and markers).
    pub x0: f32,
    /// The centre of a bullet or checkbox.
    pub marker: f32,
    pub level: usize,
    pub size: f32,
}

impl Lay {
    /// The row a column sits on. At a soft wrap the column belongs to the
    /// row it starts.
    pub fn row_of(&self, col: usize) -> usize {
        let n = self.rows.len();
        for (i, row) in self.rows.iter().enumerate() {
            if col < row.end || i + 1 == n {
                return i;
            }
        }
        0
    }

    /// The x of a column (relative to the column's left edge), and its row.
    pub fn x_of(&self, c: u64, text: &str, col: usize) -> (usize, f32) {
        let row = self.row_of(col);
        let mut x = self.x0;
        let mut found = false;
        for run in self.runs.iter().filter(|r| r.row as usize == row) {
            if col < run.start {
                break;
            }
            if col <= run.end {
                if run.hidden {
                    x = run.x;
                } else {
                    let part = text.get(run.start..col).unwrap_or("");
                    x = run.x + look::width_styled(c, part, run.size, run.style());
                }
                found = true;
                if col < run.end || !run.hidden {
                    break;
                }
            } else {
                x = run.x + run.w;
            }
        }
        if !found {
            if let Some(last) = self
                .runs
                .iter()
                .rfind(|r| r.row as usize == row && !r.hidden)
            {
                if col >= last.end {
                    x = last.x + last.w;
                }
            }
        }
        (row, x)
    }

    /// The column nearest `x` on `row`.
    pub fn col_at(&self, c: u64, text: &str, row: usize, x: f32) -> usize {
        let Some(rl) = self.rows.get(row) else {
            return 0;
        };
        let last_row = row + 1 == self.rows.len();
        let mut best = rl.start;
        let mut first = true;
        for run in self
            .runs
            .iter()
            .filter(|r| r.row as usize == row && !r.hidden)
        {
            if x < run.x {
                return if first { run.start.max(rl.start) } else { best };
            }
            first = false;
            if x <= run.x + run.w {
                let s = text.get(run.start..run.end).unwrap_or("");
                // The last boundary at or before x, then the nearer of it
                // and the next one.
                let mut lo = 0usize;
                let mut hi = s.len();
                while lo < hi {
                    let mut mid = (lo + hi).div_ceil(2);
                    while mid < s.len() && !s.is_char_boundary(mid) {
                        mid += 1;
                    }
                    if mid >= hi && mid > lo {
                        let mut m2 = hi;
                        while m2 > lo && !s.is_char_boundary(m2) {
                            m2 -= 1;
                        }
                        mid = m2;
                        if mid == lo {
                            break;
                        }
                    }
                    let w =
                        look::width_styled(c, s.get(..mid).unwrap_or(""), run.size, run.style());
                    if run.x + w <= x {
                        lo = mid;
                    } else {
                        hi = mid - 1;
                        while hi > lo && !s.is_char_boundary(hi) {
                            hi -= 1;
                        }
                    }
                }
                let a = lo;
                let b = s
                    .get(a..)
                    .and_then(|t| t.chars().next())
                    .map_or(a, |ch| a + ch.len_utf8());
                let wa = look::width_styled(c, s.get(..a).unwrap_or(""), run.size, run.style());
                let wb = look::width_styled(c, s.get(..b).unwrap_or(""), run.size, run.style());
                let pick = if (x - (run.x + wa)).abs() <= (run.x + wb - x).abs() {
                    a
                } else {
                    b
                };
                let col = run.start + pick;
                if !last_row && col >= rl.end {
                    return prev_boundary(text, rl.end);
                }
                return col;
            }
            best = run.end;
        }
        if !last_row {
            // The end of a wrapped row is the start of the next one: stop
            // just before it.
            return prev_boundary(text, rl.end).max(rl.start);
        }
        rl.end.max(best)
    }

    /// The run under a point (relative to the line's top-left).
    pub fn run_at(&self, row: usize, x: f32) -> Option<&Run> {
        self.runs
            .iter()
            .find(|r| r.row as usize == row && !r.hidden && x >= r.x && x <= r.x + r.w)
    }

    pub fn row_at(&self, y: f32) -> usize {
        for (i, row) in self.rows.iter().enumerate() {
            if y < row.top + row.h {
                return i;
            }
        }
        self.rows.len().saturating_sub(1)
    }
}

fn prev_boundary(s: &str, at: usize) -> usize {
    s.get(..at.min(s.len()))
        .and_then(|h| h.char_indices().next_back())
        .map_or(0, |(i, _)| i)
}

/// What a line's layout depends on, besides its text.
#[derive(Clone, Copy, PartialEq)]
pub struct Ctx {
    pub block: Block,
    pub mode: Mode,
    pub active: bool,
    pub width: f32,
    pub body: f32,
    pub code: f32,
}

fn heading_size(level: u8, body: f32) -> f32 {
    body * match level {
        1 => 1.8,
        2 => 1.6,
        3 => 1.4,
        4 => 1.25,
        5 => 1.12,
        _ => 1.0,
    }
}

/// Lay out one line.
pub fn layout(c: u64, text: &str, ctx: Ctx) -> Lay {
    let live = ctx.mode != Mode::Source;
    let show_syntax = !live || (ctx.active && ctx.mode != Mode::Reading);
    let k = ctx.body / 16.0;
    let (mut size, mut weight, mut mono) = (ctx.body, 400u16, false);
    let mut deco = Deco::None;
    let mut x0 = 0.0f32;
    let mut marker = 0.0f32;
    let mut level = 0usize;
    let mut segs: Vec<md::Seg> = Vec::new();
    let mut pad_top = 0.0f32;
    let mut line_h = 1.5f32;
    let plain = |a: usize, b: usize, syntax: bool| md::Seg {
        start: a,
        end: b,
        style: md::Style {
            syntax,
            ..md::Style::default()
        },
        target: None,
    };
    let indent_w = 24.0 * k;
    match ctx.block {
        Block::Fence => {
            mono = true;
            size = ctx.code;
            let open = true;
            deco = Deco::Fence {
                open,
                shown: show_syntax,
            };
            segs.push(plain(0, text.len(), !show_syntax));
            line_h = if show_syntax { 1.6 } else { 0.9 };
        }
        Block::Code => {
            mono = true;
            size = ctx.code;
            deco = Deco::Code;
            segs.push(plain(0, text.len(), false));
            line_h = 1.6;
        }
        Block::Heading { level: l, content } => {
            size = if live {
                heading_size(l, ctx.body)
            } else {
                ctx.body
            };
            weight = 700;
            line_h = if live { 1.3 } else { 1.5 };
            pad_top = if live && l <= 2 {
                8.0 * k
            } else if live {
                4.0 * k
            } else {
                0.0
            };
            segs.push(plain(0, content, true));
            segs.extend(md::inline(text, content));
        }
        Block::Bullet {
            indent,
            marker: m,
            content,
        }
        | Block::Task {
            indent,
            marker: m,
            content,
            ..
        } if live => {
            level = indent;
            let task = matches!(ctx.block, Block::Task { .. });
            let mark_w = if task { 26.0 * k } else { 20.0 * k };
            marker = level as f32 * indent_w + mark_w * 0.45;
            x0 = level as f32 * indent_w + mark_w;
            deco = match ctx.block {
                Block::Task { checked, .. } => Deco::Task(checked),
                _ => Deco::Bullet,
            };
            // Indentation and the marker are drawn, not typed out.
            segs.push(plain(0, m, true));
            segs.push(md::Seg {
                start: m,
                end: content,
                style: md::Style {
                    syntax: true,
                    ..md::Style::default()
                },
                target: None,
            });
            let mut inl = md::inline(text, content);
            if let Block::Task { checked: true, .. } = ctx.block {
                for s in inl.iter_mut() {
                    s.style.strike = true;
                }
            }
            segs.extend(inl);
        }
        Block::Ordered {
            indent,
            marker: m,
            content,
        } if live => {
            level = indent;
            x0 = level as f32 * indent_w;
            segs.push(plain(0, m, true));
            segs.push(plain(m, content, false));
            segs.extend(md::inline(text, content));
        }
        Block::Quote { content } if live => {
            deco = Deco::Quote;
            x0 = 18.0 * k;
            segs.push(plain(0, content, true));
            segs.extend(md::inline(text, content));
        }
        Block::Rule if live && !show_syntax => {
            deco = Deco::Rule;
            segs.push(plain(0, text.len(), true));
        }
        _ => {
            segs.extend(md::inline(text, 0));
        }
    }
    // The hidden-marker trick only hides when the line is not being edited;
    // a list's indentation and marker stay drawn either way.
    let always_drawn = |i: usize| -> bool {
        live && matches!(ctx.block, Block::Bullet { .. } | Block::Task { .. }) && i < 2
    };
    let avail = (ctx.width - x0).max(40.0);
    let lh = (size * line_h).round();
    let mut rows: Vec<RowL> = Vec::new();
    let mut runs: Vec<Run> = Vec::new();
    let mut row: u16 = 0;
    let mut x = x0;
    let mut row_start = 0usize;
    let mut row_has = false;
    let push_run = |runs: &mut Vec<Run>, run: Run| {
        if let Some(last) = runs.last_mut() {
            if !last.hidden
                && !run.hidden
                && last.row == run.row
                && last.end == run.start
                && last.st == run.st
                && last.target == run.target
                && last.size == run.size
                && last.weight == run.weight
                && last.italic == run.italic
                && last.mono == run.mono
            {
                last.end = run.end;
                last.w = run.x + run.w - last.x;
                return;
            }
        }
        runs.push(run);
    };
    for (si, seg) in segs.iter().enumerate() {
        let hidden = if always_drawn(si) {
            true
        } else if seg.style.syntax {
            !show_syntax && (live || ctx.mode == Mode::Reading)
        } else {
            false
        };
        let code = seg.style.code;
        let rsize = if code && !mono { ctx.code } else { size };
        let rweight = if seg.style.bold { 700 } else { weight };
        let rmono = mono || code;
        let ritalic = seg.style.italic;
        let base = Run {
            row,
            x,
            w: 0.0,
            start: seg.start,
            end: seg.end,
            st: seg.style,
            hidden,
            target: seg.target,
            size: rsize,
            weight: rweight,
            italic: ritalic,
            mono: rmono,
        };
        if hidden {
            push_run(&mut runs, Run { row, x, ..base });
            continue;
        }
        let st = look::style(rweight, ritalic, rmono);
        // Words, each ending with the spaces after it.
        let s = text.get(seg.start..seg.end).unwrap_or("");
        let mut a = 0usize;
        let bytes = s.as_bytes();
        while a < s.len() {
            let mut b = a;
            while b < s.len() && bytes.get(b) != Some(&b' ') {
                b += s
                    .get(b..)
                    .and_then(|t| t.chars().next())
                    .map_or(1, char::len_utf8);
            }
            while b < s.len() && bytes.get(b) == Some(&b' ') {
                b += 1;
            }
            let word = s.get(a..b).unwrap_or("");
            let mut w = look::width_styled(c, word, rsize, st);
            if row_has && x + w > x0 + avail {
                let trimmed = word.trim_end_matches(' ');
                let tw = look::width_styled(c, trimmed, rsize, st);
                if x + tw > x0 + avail {
                    rows.push(RowL {
                        top: 0.0,
                        h: lh,
                        base: 0.0,
                        start: row_start,
                        end: seg.start + a,
                    });
                    row += 1;
                    x = x0;
                    row_start = seg.start + a;
                    row_has = false;
                }
            }
            // A word longer than the whole line is cut by characters.
            let mut a2 = a;
            while w > avail && !row_has {
                let piece = s.get(a2..b).unwrap_or("");
                let mut cut = piece.len();
                while cut > 1 {
                    let mut m = cut / 2;
                    while m > 0 && !piece.is_char_boundary(m) {
                        m -= 1;
                    }
                    if m == 0 {
                        break;
                    }
                    if look::width_styled(c, piece.get(..m).unwrap_or(""), rsize, st) <= avail {
                        // Grow back toward the longest prefix that fits.
                        let mut best = m;
                        let mut step = piece.get(m..).map_or(0, |t| t.len()) / 2;
                        while step > 0 {
                            let mut tryp = (best + step).min(piece.len());
                            while tryp < piece.len() && !piece.is_char_boundary(tryp) {
                                tryp += 1;
                            }
                            if look::width_styled(c, piece.get(..tryp).unwrap_or(""), rsize, st)
                                <= avail
                            {
                                best = tryp;
                            }
                            step /= 2;
                        }
                        cut = best;
                        break;
                    }
                    cut = m;
                }
                let cut = cut.max(piece.chars().next().map_or(1, char::len_utf8));
                let pw = look::width_styled(c, piece.get(..cut).unwrap_or(""), rsize, st);
                push_run(
                    &mut runs,
                    Run {
                        row,
                        x,
                        w: pw,
                        start: seg.start + a2,
                        end: seg.start + a2 + cut,
                        ..base
                    },
                );
                a2 += cut;
                rows.push(RowL {
                    top: 0.0,
                    h: lh,
                    base: 0.0,
                    start: row_start,
                    end: seg.start + a2,
                });
                row += 1;
                x = x0;
                row_start = seg.start + a2;
                w = look::width_styled(c, s.get(a2..b).unwrap_or(""), rsize, st);
                if a2 >= b {
                    break;
                }
            }
            if a2 < b {
                push_run(
                    &mut runs,
                    Run {
                        row,
                        x,
                        w,
                        start: seg.start + a2,
                        end: seg.start + b,
                        ..base
                    },
                );
                x += w;
                row_has = true;
            }
            a = b;
        }
    }
    rows.push(RowL {
        top: 0.0,
        h: lh,
        base: 0.0,
        start: row_start,
        end: text.len(),
    });
    // Rows stack; the first carries a heading's space above it.
    let mut y = pad_top;
    for rl in rows.iter_mut() {
        rl.top = y;
        rl.base = y + (lh - size) * 0.5 + size * 0.8;
        y += lh;
    }
    Lay {
        h: y,
        rows,
        runs,
        deco,
        x0,
        marker,
        level,
        size,
    }
}

/// The note's title, drawn above its first line.
pub fn layout_title(c: u64, title: &str, width: f32, sz: &Sizes) -> Lay {
    let size = sz.body * 1.8;
    let st = look::style(700, false, false);
    let mut rows = Vec::new();
    let mut runs = Vec::new();
    let w = look::width_styled(c, title, size, st).min(width);
    let lh = (size * 1.25).round();
    let top = 30.0 * sz.scale;
    rows.push(RowL {
        top,
        h: lh,
        base: top + size * 0.95,
        start: 0,
        end: title.len(),
    });
    runs.push(Run {
        row: 0,
        x: 0.0,
        w,
        start: 0,
        end: title.len(),
        st: md::Style::default(),
        hidden: false,
        target: None,
        size,
        weight: 700,
        italic: false,
        mono: false,
    });
    Lay {
        h: top + lh + 18.0 * sz.scale,
        rows,
        runs,
        deco: Deco::Title,
        x0: 0.0,
        marker: 0.0,
        level: 0,
        size,
    }
}

/// Laid-out lines by what they depend on.
#[derive(Default)]
pub struct LayCache {
    map: BTreeMap<u64, Rc<Lay>>,
    pub misses: u64,
}

const CACHE_MAX: usize = 6_000;

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
}

fn key(text: &str, ctx: &Ctx) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    fnv(&mut h, text.as_bytes());
    fnv(&mut h, &(text.len() as u64).to_le_bytes());
    let kind: u8 = match ctx.block {
        Block::Blank => 0,
        Block::Para => 1,
        Block::Heading { level, .. } => 10 + level,
        Block::Bullet { .. } => 2,
        Block::Ordered { .. } => 3,
        Block::Task { checked, .. } => 4 + u8::from(checked),
        Block::Quote { .. } => 6,
        Block::Fence => 7,
        Block::Code => 8,
        Block::Rule => 9,
        Block::Table => 20,
    };
    let mode: u8 = match ctx.mode {
        Mode::Live => 0,
        Mode::Source => 1,
        Mode::Reading => 2,
    };
    fnv(&mut h, &[kind, mode, u8::from(ctx.active)]);
    fnv(&mut h, &ctx.width.to_bits().to_le_bytes());
    fnv(&mut h, &ctx.body.to_bits().to_le_bytes());
    h
}

impl LayCache {
    pub fn get(&mut self, c: u64, text: &str, ctx: Ctx) -> Rc<Lay> {
        let k = key(text, &ctx);
        if let Some(l) = self.map.get(&k) {
            return l.clone();
        }
        self.misses += 1;
        if self.map.len() >= CACHE_MAX {
            self.map.clear();
        }
        let l = Rc::new(layout(c, text, ctx));
        self.map.insert(k, l.clone());
        l
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}

/// Where the note is drawn this frame: the column, and the rows shown.
pub struct View {
    pub area: Rect,
    pub x: f32,
    pub width: f32,
    pub sz: Sizes,
}

/// The rows of a note on screen last frame, for clicks.
#[derive(Default)]
pub struct Shown {
    pub rows: Vec<(usize, f32, Rc<Lay>)>,
    pub area: Option<Rect>,
    pub x: f32,
    pub thumb: Option<Rect>,
    pub track: Option<Rect>,
}

fn active_lines(doc: &Doc) -> (usize, usize) {
    match doc.selection() {
        Some((a, b)) => (a.line, b.line),
        None => (doc.cur.line, doc.cur.line),
    }
}

/// The layout of row `vi` of a note (0 is the title).
pub fn lay(cache: &mut LayCache, c: u64, doc: &Doc, vi: usize, v: &View) -> Rc<Lay> {
    if vi == 0 {
        let k = {
            let mut h: u64 = 0x1234_5678;
            fnv(&mut h, doc.title.as_bytes());
            fnv(&mut h, &v.width.to_bits().to_le_bytes());
            fnv(&mut h, &v.sz.body.to_bits().to_le_bytes());
            h
        };
        if let Some(l) = cache.map.get(&k) {
            return l.clone();
        }
        let l = Rc::new(layout_title(c, &doc.title, v.width, &v.sz));
        cache.map.insert(k, l.clone());
        return l;
    }
    let n = vi - 1;
    let (a, b) = active_lines(doc);
    let ctx = Ctx {
        block: doc.block(n),
        mode: doc.mode,
        active: n >= a && n <= b && doc.mode != Mode::Reading,
        width: v.width,
        body: v.sz.body,
        code: v.sz.code,
    };
    cache.get(c, doc.line(n), ctx)
}

pub fn rows_total(doc: &Doc) -> usize {
    doc.buf.line_count() + 1
}

/// Scroll by `dy` pixels, row by row, never past the start, and never so
/// far that less than half a screen of the note is left showing.
pub fn scroll_by(cache: &mut LayCache, c: u64, doc: &mut Doc, v: &View, dy: f32) {
    let total = rows_total(doc);
    doc.top_off += dy;
    while doc.top_off < 0.0 {
        if doc.top == 0 {
            doc.top_off = 0.0;
            break;
        }
        doc.top -= 1;
        doc.top_off += lay(cache, c, doc, doc.top, v).h;
    }
    loop {
        let h = lay(cache, c, doc, doc.top, v).h;
        if doc.top_off >= h && doc.top + 1 < total {
            doc.top_off -= h;
            doc.top += 1;
        } else {
            break;
        }
    }
    // Keep at least half a screen of the note in view at the end.
    let want = v.area.h * 0.5;
    let mut left = -doc.top_off;
    let mut i = doc.top;
    while i < total && left < want {
        left += lay(cache, c, doc, i, v).h;
        i += 1;
    }
    if left < want {
        let short = want - left;
        scroll_up_exact(cache, c, doc, v, short);
    }
}

fn scroll_up_exact(cache: &mut LayCache, c: u64, doc: &mut Doc, v: &View, by: f32) {
    doc.top_off -= by;
    while doc.top_off < 0.0 {
        if doc.top == 0 {
            doc.top_off = 0.0;
            break;
        }
        doc.top -= 1;
        doc.top_off += lay(cache, c, doc, doc.top, v).h;
    }
}

/// Put the view where the scroll bar says: a fraction of the way down.
pub fn scroll_to_fraction(cache: &mut LayCache, c: u64, doc: &mut Doc, v: &View, f: f32) {
    let total = rows_total(doc);
    doc.top = ((total as f32 * f.clamp(0.0, 1.0)) as usize).min(total.saturating_sub(1));
    doc.top_off = 0.0;
    scroll_by(cache, c, doc, v, 0.0);
}

/// Make sure the caret is on screen, moving the view as little as it can.
pub fn reveal(cache: &mut LayCache, c: u64, doc: &mut Doc, v: &View) {
    doc.reveal = false;
    let vi = doc.cur.line + 1;
    let l = lay(cache, c, doc, vi, v);
    let text = String::from(doc.line(doc.cur.line));
    let (row, _) = l.x_of(c, &text, doc.cur.col);
    let row_top = l.rows.get(row).map_or(0.0, |rl| rl.top);
    let row_bot = l.rows.get(row).map_or(l.h, |rl| rl.top + rl.h);
    let margin = 8.0 * v.sz.scale;
    if vi < doc.top || (vi == doc.top && row_top < doc.top_off) {
        doc.top = vi;
        doc.top_off = (row_top - margin).max(0.0);
        if doc.top == 1 && doc.top_off <= 0.0 {
            // At the first line: show the title above it too.
            doc.top = 0;
            doc.top_off = 0.0;
        }
        return;
    }
    // Is it above the bottom edge?
    let mut y = -doc.top_off;
    let mut i = doc.top;
    while i < vi && y < v.area.h {
        y += lay(cache, c, doc, i, v).h;
        i += 1;
    }
    if i == vi && y + row_bot + margin <= v.area.h {
        return;
    }
    // Put the caret's row at the bottom.
    let mut acc = row_bot + margin;
    let mut top = vi;
    while acc < v.area.h && top > 0 {
        top -= 1;
        acc += lay(cache, c, doc, top, v).h;
    }
    doc.top = top;
    doc.top_off = (acc - v.area.h).max(0.0);
    let h = lay(cache, c, doc, doc.top, v).h;
    if doc.top_off >= h {
        doc.top_off = 0.0;
    }
}

/// Move the caret up or down by `n` rows as drawn (wrapped rows count).
pub fn move_vertical(
    cache: &mut LayCache,
    c: u64,
    doc: &mut Doc,
    v: &View,
    down: bool,
    n: usize,
    extend: bool,
) {
    let mut line = doc.cur.line;
    let text = String::from(doc.line(line));
    let l = lay(cache, c, doc, line + 1, v);
    let (mut row, x) = l.x_of(c, &text, doc.cur.col);
    let goal = doc.goal_x.unwrap_or(x);
    let last = doc.buf.last_line();
    for _ in 0..n {
        let rows_here = lay(cache, c, doc, line + 1, v).rows.len();
        if down {
            if row + 1 < rows_here {
                row += 1;
            } else if line < last {
                line += 1;
                row = 0;
            } else {
                let e = doc.buf.end();
                doc.place_vertical(e, extend);
                doc.goal_x = Some(goal);
                return;
            }
        } else if row > 0 {
            row -= 1;
        } else if line > 0 {
            line -= 1;
            row = lay(cache, c, doc, line + 1, v).rows.len().saturating_sub(1);
        } else {
            doc.place_vertical(Pos::new(0, 0), extend);
            doc.goal_x = Some(goal);
            return;
        }
    }
    // The target line is laid out as it will look once the caret is on it.
    let target_text = String::from(doc.line(line));
    let save = doc.cur;
    doc.cur = Pos::new(line, 0);
    let anchor = doc.anchor;
    if !extend {
        doc.anchor = None;
    }
    let tl = lay(cache, c, doc, line + 1, v);
    doc.cur = save;
    doc.anchor = anchor;
    let row = row.min(tl.rows.len().saturating_sub(1));
    let col = tl.col_at(c, &target_text, row, goal);
    doc.place_vertical(Pos::new(line, col), extend);
    doc.goal_x = Some(goal);
}

/// Draw the note in `v`, and remember where each row went.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    cache: &mut LayCache,
    c: u64,
    doc: &Doc,
    v: &View,
    t: &Theme,
    shown: &mut Shown,
    resolved: &dyn Fn(&str) -> bool,
    caret_on: bool,
    said: &mut krate::layout::Said,
) {
    shown.rows.clear();
    shown.area = Some(v.area);
    shown.x = v.x;
    let total = rows_total(doc);
    let _ = canvas2d::set_clip(c, v.area.x, v.area.y, v.area.w, v.area.h);
    let mut y = v.area.y - doc.top_off;
    let mut vi = doc.top;
    let sel = doc.selection();
    let (act_a, act_b) = active_lines(doc);
    while vi < total && y < v.area.bottom() {
        let l = lay(cache, c, doc, vi, v);
        let text: &str = if vi == 0 {
            &doc.title
        } else {
            doc.line(vi - 1)
        };
        let n = vi.saturating_sub(1);
        draw_line(c, &l, text, v.x, y, v.width, t, resolved, doc, vi);
        // The selection, over the backgrounds and under nothing that hides it.
        if vi > 0 {
            if let Some((a, b)) = sel {
                if n >= a.line && n <= b.line {
                    let from = if n == a.line { a.col } else { 0 };
                    let to = if n == b.line { b.col } else { text.len() };
                    draw_selection(c, &l, text, v.x, y, from, to, n < b.line, t);
                }
            }
            if caret_on && n == doc.cur.line && doc.mode != Mode::Reading {
                let (row, x) = l.x_of(c, text, doc.cur.col);
                if let Some(rl) = l.rows.get(row) {
                    let size = if l.deco == Deco::Code || matches!(l.deco, Deco::Fence { .. }) {
                        v.sz.code
                    } else {
                        l.size
                    };
                    let hgt = size * 1.25;
                    fill(
                        c,
                        r(
                            v.x + x - 0.5,
                            y + rl.base - size * 0.95,
                            1.6 * v.sz.scale,
                            hgt,
                        ),
                        t.text,
                    );
                }
            }
            let shown_text = text.get(..text.len().min(240)).unwrap_or("");
            if !shown_text.is_empty() && (n < act_a || n > act_b || doc.mode == Mode::Reading) {
                said.text(r(v.x, y, v.width, l.h), &md::plain(shown_text, 0));
            } else if !shown_text.is_empty() {
                said.text(r(v.x, y, v.width, l.h), shown_text);
            }
        } else {
            said.heading(r(v.x, y, v.width, l.h), &doc.title);
        }
        shown.rows.push((vi, y, l.clone()));
        y += l.h;
        vi += 1;
    }
    let _ = canvas2d::clear_clip(c);
    // The scroll bar, when the note is longer than the view.
    shown.thumb = None;
    shown.track = None;
    let visible = shown.rows.len().max(1) as f32;
    if (total as f32) > visible || doc.top > 0 {
        let track = r(
            v.area.right() - 10.0 * v.sz.scale,
            v.area.y + 2.0,
            8.0 * v.sz.scale,
            v.area.h - 4.0,
        );
        let frac = (doc.top as f32) / (total as f32);
        let th = (track.h * (visible / total as f32)).clamp(28.0, track.h);
        let ty = track.y
            + (track.h - th)
                * (frac / (1.0 - (visible / total as f32)).max(0.0001)).clamp(0.0, 1.0);
        let thumb = r(track.x + 2.0, ty, track.w - 4.0, th);
        round(c, thumb, 3.0, look::alpha(t.muted, 0.35));
        shown.thumb = Some(thumb);
        shown.track = Some(track);
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_selection(
    c: u64,
    l: &Lay,
    text: &str,
    ox: f32,
    oy: f32,
    from: usize,
    to: usize,
    past_end: bool,
    t: &Theme,
) {
    for (ri, rl) in l.rows.iter().enumerate() {
        let a = from.max(rl.start);
        let b = to.min(rl.end);
        if a > b || (a == b && !(past_end && ri + 1 == l.rows.len())) {
            continue;
        }
        let (_, xa) = l.x_of(c, text, a);
        let (_, xb) = if b >= rl.end && ri + 1 < l.rows.len() {
            (
                ri,
                l.runs
                    .iter()
                    .filter(|r| r.row as usize == ri)
                    .map(|r| r.x + r.w)
                    .fold(xa, f32::max),
            )
        } else {
            l.x_of(c, text, b)
        };
        let extra = if past_end && ri + 1 == l.rows.len() {
            6.0
        } else {
            0.0
        };
        fill(
            c,
            r(ox + xa, oy + rl.top, (xb - xa).max(0.0) + extra, rl.h),
            t.sel,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_line(
    c: u64,
    l: &Lay,
    text: &str,
    ox: f32,
    oy: f32,
    width: f32,
    t: &Theme,
    resolved: &dyn Fn(&str) -> bool,
    doc: &Doc,
    vi: usize,
) {
    let k = l.size / 16.0;
    match l.deco {
        Deco::Code | Deco::Fence { .. } => {
            let is_fence = matches!(l.deco, Deco::Fence { .. });
            let n = vi.saturating_sub(1);
            let opens = is_fence && doc.fences.opens(n);
            let top_pad = if opens { 2.0 } else { 0.0 };
            fill(
                c,
                r(ox - 6.0, oy + top_pad, width + 12.0, l.h - top_pad),
                t.code_bg,
            );
            if let Deco::Fence { shown: false, .. } = l.deco {
                if opens {
                    let lang = md::fence_lang(text);
                    if !lang.is_empty() {
                        let lw = look::width(c, lang, 11.0, 400);
                        look::text(
                            c,
                            lang,
                            ox + width - lw - 4.0,
                            oy + l.h * 0.5 + 6.0,
                            11.0,
                            400,
                            t.faint,
                        );
                    }
                }
            }
        }
        Deco::Quote => {
            fill(c, r(ox + 2.0, oy, 2.5, l.h), t.quote);
        }
        Deco::Rule => {
            fill(c, r(ox, oy + l.h * 0.5, width, 1.0), t.border);
        }
        Deco::Bullet => {
            if let Some(rl) = l.rows.first() {
                let _ = canvas2d::fill_circle(
                    c,
                    krate::gfx::types::Point {
                        x: ox + l.marker,
                        y: oy + rl.base - l.size * 0.3,
                    },
                    2.6 * k,
                    t.muted,
                );
            }
            // Indent guides for nested items.
            for lv in 0..l.level {
                fill(
                    c,
                    r(ox + lv as f32 * 24.0 * k + 8.0 * k, oy, 1.0, l.h),
                    t.border,
                );
            }
        }
        Deco::Task(done) => {
            if let Some(rl) = l.rows.first() {
                let s = 14.0 * k;
                let bx = ox + l.marker - s * 0.5;
                let by = oy + rl.base - l.size * 0.3 - s * 0.5;
                if done {
                    round(c, r(bx, by, s, s), 3.0, t.accent);
                    look::line(
                        c,
                        bx + s * 0.25,
                        by + s * 0.52,
                        bx + s * 0.43,
                        by + s * 0.7,
                        1.8 * k,
                        t.on_accent,
                    );
                    look::line(
                        c,
                        bx + s * 0.43,
                        by + s * 0.7,
                        bx + s * 0.76,
                        by + s * 0.3,
                        1.8 * k,
                        t.on_accent,
                    );
                } else {
                    look::outline(c, r(bx, by, s, s), 3.0, 1.3, t.muted);
                }
            }
            for lv in 0..l.level {
                fill(
                    c,
                    r(ox + lv as f32 * 24.0 * k + 8.0 * k, oy, 1.0, l.h),
                    t.border,
                );
            }
        }
        _ => {}
    }
    for run in &l.runs {
        if run.hidden || run.end <= run.start {
            continue;
        }
        let Some(rl) = l.rows.get(run.row as usize) else {
            continue;
        };
        let s = text.get(run.start..run.end).unwrap_or("");
        let x = ox + run.x;
        let base = oy + rl.base;
        let st = run.st;
        if st.mark {
            fill(c, r(x, oy + rl.top + 2.0, run.w, rl.h - 4.0), t.mark_bg);
        }
        if st.code && l.deco != Deco::Code {
            round(
                c,
                r(x - 2.0, base - run.size * 0.95, run.w + 4.0, run.size * 1.3),
                3.0,
                t.code_bg,
            );
        }
        if st.tag {
            round(
                c,
                r(x - 3.0, base - run.size * 0.95, run.w + 6.0, run.size * 1.3),
                run.size * 0.65,
                t.tag_bg,
            );
        }
        let mut col: Rgba = t.text;
        if st.syntax {
            col = t.faint;
        } else if st.link {
            col = match run.target {
                Some(Target::Wiki(a, b)) => {
                    if resolved(text.get(a..b).unwrap_or("")) {
                        t.link
                    } else {
                        t.link_missing
                    }
                }
                _ => t.link,
            };
        } else if st.tag {
            col = t.link;
        } else if l.deco == Deco::Task(true) {
            col = t.muted;
        } else if l.deco == Deco::Title {
            col = t.text;
        }
        look::text_styled(c, s, x, base, run.size, run.style(), col);
        if st.link && !st.syntax {
            fill(c, r(x, base + 2.0, run.w, 1.0), look::alpha(col, 0.7));
        }
        if st.strike {
            fill(c, r(x, base - run.size * 0.3, run.w, 1.2), col);
        }
    }
}
