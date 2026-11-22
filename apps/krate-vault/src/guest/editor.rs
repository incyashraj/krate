//! Laying out and drawing a note, one line at a time.
//!
//! Only the lines on screen are ever laid out. A line's layout -- where it
//! wraps, which syntax is hidden, where each styled run sits -- depends only
//! on its own text and a few facts about it (its block, whether the caret is
//! on it, the width), so it is cached by those: scrolling lays out only the
//! lines coming into view, and typing lays out only the line typed in. A
//! 200,000-line note draws the same few dozen lines a short one does.
//!
//! Blocks that span lines -- tables, callouts, `$$` math, front matter,
//! embeds -- are drawn whole by their first line (see `widgets`) while the
//! caret is outside them; their other lines take no room. With the caret
//! inside, each line shows its source, as Obsidian's live preview does.
//! Reading view lays lines out by its own rules: blank lines collapse into
//! paragraph gaps, comments and footnote definitions go, footnotes are
//! numbered and listed at the end, and every block is drawn whole.
//!
//! The view's position is a row and an offset into it (`Doc::top`,
//! `Doc::top_off`), never a pixel offset from the top of the note, so
//! nothing ever needs the height of the whole note.

use super::look::{self, fill, r, round, Rgba, Sizes, Theme};
use super::widgets::{self, Env, Widget};
use crate::buffer::Pos;
use crate::editor::{Doc, Mode};
use crate::highlight::{self, Tok};
pub use crate::links::{Act, FindHit};
use crate::math::{self, MathBox};
use crate::md::{self, Block, Target};
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::canvas2d;
use krate::layout::Rect;
#[allow(unused_imports)]
use krate::prelude::*;

const NONE: u16 = u16::MAX;

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
    /// How far the baseline is moved (negative is up): superscripts,
    /// footnote marks.
    pub rise: f32,
    /// A math box drawn instead of the text (`Lay::boxes`).
    pub bx: u16,
    /// Text drawn instead of the source (`Lay::alts`): a footnote's number.
    pub alt: u16,
    /// The code colour of a run in a code block.
    pub tok: Tok,
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
    Fence {
        open: bool,
        shown: bool,
    },
    Rule,
    Title,
    /// Takes no room: a line folded away, or inside a block drawn whole.
    Hidden,
    /// A block drawn whole (`Lay::widget`).
    Widget,
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
    pub boxes: Vec<MathBox>,
    pub alts: Vec<String>,
    pub widget: Option<Rc<Widget>>,
    /// 0: cannot fold; 1: can fold; 2: folded.
    pub fold: u8,
    /// A task's mark (`x`, `/`, `-`) for the states a checkbox draws.
    pub task_mark: u8,
    /// Struck through and dimmed: a done task, or (reading view) an item
    /// under one.
    pub struck: bool,
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
                } else if run.bx != NONE || run.alt != NONE {
                    x = if col == run.start {
                        run.x
                    } else {
                        run.x + run.w
                    };
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
                if run.bx != NONE || run.alt != NONE {
                    return if x - run.x < run.w * 0.5 {
                        run.start
                    } else {
                        run.end
                    };
                }
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

    /// A layout that takes no room.
    pub fn hidden(size: f32) -> Lay {
        Lay {
            h: 0.0,
            rows: alloc::vec![RowL {
                top: 0.0,
                h: 0.0,
                base: 0.0,
                start: 0,
                end: 0,
            }],
            runs: Vec::new(),
            deco: Deco::Hidden,
            x0: 0.0,
            marker: 0.0,
            level: 0,
            size,
            boxes: Vec::new(),
            alts: Vec::new(),
            widget: None,
            fold: 0,
            task_mark: 0,
            struck: false,
        }
    }

    /// The right edge of what is drawn on the last row.
    pub fn end_x(&self) -> f32 {
        let last = self.rows.len().saturating_sub(1) as u16;
        self.runs
            .iter()
            .filter(|r| r.row == last && !r.hidden)
            .map(|r| r.x + r.w)
            .fold(self.x0, f32::max)
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
    /// Reading view: a blank line that ends a block (it becomes the gap
    /// between blocks; the blank lines after it take no room).
    pub gap: bool,
    /// Inside a `%%` comment block.
    pub comment: bool,
    /// 0: cannot fold; 1: can fold; 2: folded.
    pub fold: u8,
    /// Reading view: a list item under a done task, struck through with it.
    pub struck: bool,
}

/// Whether list item `n` sits (at any depth) under a ticked task, which
/// reading view strikes through with its children.
fn under_done_task(doc: &Doc, n: usize) -> bool {
    let mut ind = crate::text::indent_of(doc.line(n)).len();
    let mut p = n;
    while ind > 0 && p > 0 && n - p < 200 {
        p -= 1;
        let l = doc.line(p);
        if l.trim().is_empty() {
            return false;
        }
        let i = crate::text::indent_of(l).len();
        if i < ind {
            match doc.block(p) {
                Block::Task { checked: true, .. }
                    if matches!(l.trim_start().as_bytes().get(3), Some(b'x' | b'X')) =>
                {
                    return true
                }
                b if b.is_list() => ind = i,
                _ => return false,
            }
        }
    }
    false
}

pub fn heading_size(level: u8, body: f32) -> f32 {
    // Obsidian's default heading sizes: 1.618, 1.462, 1.318, 1.188, 1.076,
    // 1 times the body.
    body * match level {
        1 => 1.618,
        2 => 1.462,
        3 => 1.318,
        4 => 1.188,
        5 => 1.076,
        _ => 1.0,
    }
}

/// Measuring maths with the canvas, in a serif face as TeX is set.
pub struct Cm(pub u64);

pub fn math_style(font: math::Font) -> krate::gfx::types::TextStyle {
    krate::gfx::types::TextStyle {
        weight: if font == math::Font::Bold { 700 } else { 400 },
        italic: font == math::Font::Italic,
        letter_spacing: 0.0,
        family: krate::gfx::types::FontFamily::Serif,
    }
}

impl math::Measure for Cm {
    fn width(&self, s: &str, size: f32, font: math::Font) -> f32 {
        look::width_styled(self.0, s, size, math_style(font))
    }
}

/// Lay out one line. `lang` is the language of the code block it is in;
/// `nums` are the numbers its footnote references show in reading view.
pub fn layout(c: u64, text: &str, ctx: Ctx, lang: &str, nums: &[usize]) -> Lay {
    let live = ctx.mode != Mode::Source;
    let reading = ctx.mode == Mode::Reading;
    let show_syntax = !live || (ctx.active && !reading);
    let k = ctx.body / 16.0;
    let (mut size, mut weight, mut mono) = (ctx.body, 400u16, false);
    let mut deco = Deco::None;
    let mut x0 = 0.0f32;
    let mut marker = 0.0f32;
    let mut level = 0usize;
    let mut segs: Vec<md::Seg> = Vec::new();
    let mut toks: Vec<Tok> = Vec::new();
    let mut pad_top = 0.0f32;
    let mut pad_bottom = 0.0f32;
    let mut line_h = 1.5f32;
    let mut task_mark = 0u8;
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
    if reading && ctx.block == Block::Blank {
        let mut l = Lay::hidden(size);
        if ctx.gap {
            l.h = (ctx.body * 1.0).round();
            if let Some(rl) = l.rows.first_mut() {
                rl.h = l.h;
            }
            l.deco = Deco::None;
        }
        return l;
    }
    match ctx.block {
        Block::Fence => {
            mono = true;
            size = ctx.code;
            deco = Deco::Fence {
                open: true,
                shown: show_syntax,
            };
            segs.push(plain(0, text.len(), !show_syntax));
            line_h = if show_syntax { 1.6 } else { 0.9 };
        }
        Block::Code => {
            mono = true;
            size = ctx.code;
            deco = Deco::Code;
            for (a, z, tok) in highlight::line(text, if live { lang } else { "" }) {
                segs.push(plain(a, z, false));
                toks.push(tok);
            }
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
            pad_top = if reading {
                // Reading view keeps a heading's space above it.
                (if l <= 2 { 6.0 } else { 4.0 }) * k
            } else if live && l <= 2 {
                8.0 * k
            } else if live {
                4.0 * k
            } else {
                0.0
            };
            if reading {
                pad_bottom = 4.0 * k;
            }
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
                Block::Task {
                    checked, box_at, ..
                } => {
                    task_mark = text.as_bytes().get(box_at + 1).copied().unwrap_or(b' ');
                    Deco::Task(checked)
                }
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
            if matches!(task_mark, b'x' | b'X') {
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
            // The number is drawn muted, as Obsidian's list markers are.
            toks.push(Tok::Plain);
            toks.push(Tok::Punct);
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
            if reading {
                pad_top = 8.0 * k;
                pad_bottom = 8.0 * k;
            }
        }
        _ => {
            segs.extend(md::inline(text, 0));
        }
    }
    if ctx.comment {
        if reading {
            return Lay::hidden(size);
        }
        for s in segs.iter_mut() {
            s.style = md::Style {
                comment: true,
                ..md::Style::default()
            };
        }
    }
    if ctx.struck {
        for s in segs.iter_mut().skip(2) {
            s.style.strike = true;
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
    let mut boxes: Vec<MathBox> = Vec::new();
    let mut alts: Vec<String> = Vec::new();
    let mut row: u16 = 0;
    let mut x = x0;
    let mut row_start = 0usize;
    let mut row_has = false;
    let mut foot_i = 0usize;
    let push_run = |runs: &mut Vec<Run>, run: Run| {
        if let Some(last) = runs.last_mut() {
            if !last.hidden
                && !run.hidden
                && last.bx == NONE
                && run.bx == NONE
                && last.alt == NONE
                && run.alt == NONE
                && last.row == run.row
                && last.end == run.start
                && last.st == run.st
                && last.target == run.target
                && last.size == run.size
                && last.weight == run.weight
                && last.italic == run.italic
                && last.mono == run.mono
                && last.rise == run.rise
                && last.tok == run.tok
            {
                last.end = run.end;
                last.w = run.x + run.w - last.x;
                return;
            }
        }
        runs.push(run);
    };
    for (si, seg) in segs.iter().enumerate() {
        let st = seg.style;
        let hidden = if always_drawn(si) {
            true
        } else if st.comment || st.blockid {
            reading
        } else if st.footnote {
            reading && st.syntax
        } else if st.syntax {
            !show_syntax && (live || reading)
        } else {
            false
        };
        let code = st.code;
        let small = st.sup || st.sub || (st.footnote && live) || (st.blockid && live);
        let mut rsize = if code && !mono {
            ctx.code
        } else if small {
            (size * 0.75).round()
        } else {
            size
        };
        if st.kbd {
            rsize = ctx.code * 0.92;
        }
        let rise = if st.sup || (st.footnote && live) {
            -size * 0.38
        } else if st.sub {
            size * 0.2
        } else {
            0.0
        };
        let rweight = if st.bold { 700 } else { weight };
        let rmono = mono || code || st.kbd;
        let ritalic = st.italic;
        let tok = toks.get(si).copied().unwrap_or(Tok::Plain);
        let base = Run {
            row,
            x,
            w: 0.0,
            start: seg.start,
            end: seg.end,
            st,
            hidden,
            target: seg.target,
            size: rsize,
            weight: rweight,
            italic: ritalic,
            mono: rmono,
            rise,
            bx: NONE,
            alt: NONE,
            tok,
        };
        if hidden {
            push_run(&mut runs, Run { row, x, ..base });
            continue;
        }
        let rst = look::style(rweight, ritalic, rmono);
        // An atom: a math box or a footnote number, never broken.
        let atom: Option<(f32, u16, u16, f32)> = if st.math && !st.syntax && live && !show_syntax {
            let src = text.get(seg.start..seg.end).unwrap_or("");
            let display = text.get(..seg.start).is_some_and(|h| h.ends_with("$$"));
            let b = math::layout(src, size * 1.08, display, &Cm(c));
            let w = b.w;
            boxes.push(b);
            Some((w, (boxes.len() - 1) as u16, NONE, rsize))
        } else if st.link
            && !st.syntax
            && live
            && !show_syntax
            && matches!(seg.target, Some(Target::Wiki(..)))
            && text
                .get(seg.start..seg.end)
                .is_some_and(|t| t.contains('#'))
            && text.get(..seg.start).is_some_and(|h| h.ends_with("[["))
        {
            // `[[Note#Heading]]` reads "Note > Heading", as Obsidian shows it.
            let raw = text.get(seg.start..seg.end).unwrap_or("");
            let shown = raw.trim_start_matches('#').replace('#', " > ");
            let w = look::width_styled(c, &shown, rsize, rst);
            alts.push(shown);
            Some((w, NONE, (alts.len() - 1) as u16, rsize))
        } else if st.footnote && reading && !st.syntax {
            let n = nums.get(foot_i).copied().unwrap_or(foot_i + 1);
            foot_i += 1;
            let mut s = String::from("[");
            crate::text::push_num(&mut s, n);
            s.push(']');
            let fs = (size * 0.75).round();
            let w = look::width_styled(c, &s, fs, look::style(400, false, false));
            alts.push(s);
            Some((w, NONE, (alts.len() - 1) as u16, fs))
        } else {
            None
        };
        if let Some((w, bx, alt, asize)) = atom {
            if row_has && x + w > x0 + avail {
                rows.push(RowL {
                    top: 0.0,
                    h: lh,
                    base: 0.0,
                    start: row_start,
                    end: seg.start,
                });
                row += 1;
                x = x0;
                row_start = seg.start;
            }
            push_run(
                &mut runs,
                Run {
                    row,
                    x,
                    w,
                    bx,
                    alt,
                    size: asize,
                    rise: if alt != NONE && st.footnote {
                        -size * 0.38
                    } else {
                        0.0
                    },
                    ..base
                },
            );
            x += w;
            row_has = true;
            continue;
        }
        // Words, each ending with the spaces after it.
        let s = text.get(seg.start..seg.end).unwrap_or("");
        let pad = if st.kbd { 6.0 * k } else { 0.0 };
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
            let mut w = look::width_styled(c, word, rsize, rst) + pad;
            if row_has && x + w > x0 + avail {
                let trimmed = word.trim_end_matches(' ');
                let tw = look::width_styled(c, trimmed, rsize, rst);
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
                    if look::width_styled(c, piece.get(..m).unwrap_or(""), rsize, rst) <= avail {
                        // Grow back toward the longest prefix that fits.
                        let mut best = m;
                        let mut step = piece.get(m..).map_or(0, |t| t.len()) / 2;
                        while step > 0 {
                            let mut tryp = (best + step).min(piece.len());
                            while tryp < piece.len() && !piece.is_char_boundary(tryp) {
                                tryp += 1;
                            }
                            if look::width_styled(c, piece.get(..tryp).unwrap_or(""), rsize, rst)
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
                let pw = look::width_styled(c, piece.get(..cut).unwrap_or(""), rsize, rst);
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
                w = look::width_styled(c, s.get(a2..b).unwrap_or(""), rsize, rst);
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
    // Rows stack; the first carries a heading's space above it. A row
    // holding a tall math box grows to fit it.
    let mut y = pad_top;
    let asc0 = (lh - size) * 0.5 + size * 0.8;
    for (ri, rl) in rows.iter_mut().enumerate() {
        let mut asc = asc0;
        let mut desc = lh - asc0;
        for run in runs.iter().filter(|r| r.row as usize == ri && r.bx != NONE) {
            if let Some(b) = boxes.get(run.bx as usize) {
                asc = asc.max(b.asc + 3.0 * k);
                desc = desc.max(b.desc + 3.0 * k);
            }
        }
        rl.top = y;
        rl.h = asc + desc;
        rl.base = y + asc;
        y += rl.h;
    }
    Lay {
        h: y + pad_bottom,
        rows,
        runs,
        deco,
        x0,
        marker,
        level,
        size,
        boxes,
        alts,
        widget: None,
        fold: ctx.fold,
        task_mark,
        struck: ctx.struck || matches!(task_mark, b'x' | b'X'),
    }
}

/// The note's title, drawn above its first line.
pub fn layout_title(c: u64, title: &str, width: f32, sz: &Sizes) -> Lay {
    let size = sz.body * 1.618;
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
        rise: 0.0,
        bx: NONE,
        alt: NONE,
        tok: Tok::Plain,
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
        boxes: Vec::new(),
        alts: Vec::new(),
        widget: None,
        fold: 0,
        task_mark: 0,
        struck: false,
    }
}

/// Laid-out lines by what they depend on, and what the blocks drawn whole
/// need from outside the note (embedded notes, pictures).
#[derive(Default)]
pub struct LayCache {
    map: BTreeMap<u64, Rc<Lay>>,
    pub misses: u64,
    pub env: Env,
    /// Where the pointer is, for the fold chevrons it shows.
    pub pointer: Option<(f32, f32)>,
    /// The link under the pointer while Cmd is held: the page preview.
    pub preview: Option<widgets::Preview>,
    /// Selected text is being dragged to a new place.
    pub text_drag: bool,
    /// Settings > Appearance > Show inline title, turned off.
    pub hide_title: bool,
    /// Settings > Editor > Show line numbers.
    pub line_numbers: bool,
}

impl LayCache {
    /// The settings the note view follows, set each frame by the guest.
    pub fn follow_settings(&mut self, show_inline_title: bool, show_line_numbers: bool) {
        self.hide_title = !show_inline_title;
        self.line_numbers = show_line_numbers;
    }
}

const CACHE_MAX: usize = 6_000;

pub fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
}

fn key(text: &str, ctx: &Ctx, lang: &str, nums: &[usize]) -> u64 {
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
    fnv(
        &mut h,
        &[
            kind,
            mode,
            u8::from(ctx.active),
            u8::from(ctx.gap),
            u8::from(ctx.comment),
            ctx.fold,
            u8::from(ctx.struck),
        ],
    );
    fnv(&mut h, &ctx.width.to_bits().to_le_bytes());
    fnv(&mut h, &ctx.body.to_bits().to_le_bytes());
    fnv(&mut h, lang.as_bytes());
    for n in nums {
        fnv(&mut h, &(*n as u32).to_le_bytes());
    }
    h
}

impl LayCache {
    pub fn get(&mut self, c: u64, text: &str, ctx: Ctx, lang: &str, nums: &[usize]) -> Rc<Lay> {
        let k = key(text, &ctx, lang, nums);
        if let Some(l) = self.map.get(&k) {
            return l.clone();
        }
        self.misses += 1;
        self.make_room();
        let l = Rc::new(layout(c, text, ctx, lang, nums));
        self.map.insert(k, l.clone());
        l
    }

    fn make_room(&mut self) {
        if self.map.len() >= CACHE_MAX {
            self.map.clear();
        }
    }

    pub fn cached(&self, k: u64) -> Option<Rc<Lay>> {
        self.map.get(&k).cloned()
    }

    pub fn put(&mut self, k: u64, l: Rc<Lay>) {
        self.misses += 1;
        self.make_room();
        self.map.insert(k, l);
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
    /// The find bar's parts, for clicks: (what, where).
    pub find: Vec<(FindHit, Rect)>,
    pub width: f32,
}

fn active_lines(doc: &Doc) -> (usize, usize) {
    match doc.selection() {
        Some((a, b)) => (a.line, b.line),
        None => (doc.cur.line, doc.cur.line),
    }
}

/// Whether reading view lists footnotes after the last line.
fn has_notes_row(doc: &Doc) -> bool {
    doc.mode == Mode::Reading && !doc.footnotes().notes.is_empty()
}

/// The layout of row `vi` of a note (0 is the title; one past the last
/// line is reading view's footnotes).
pub fn lay(cache: &mut LayCache, c: u64, doc: &Doc, vi: usize, v: &View) -> Rc<Lay> {
    if vi == 0 {
        if cache.hide_title {
            // Settings > Appearance > Show inline title, off: the note
            // starts at the top with the editor's padding.
            let mut l = Lay::hidden(v.sz.body);
            l.h = 24.0 * v.sz.scale;
            return Rc::new(l);
        }
        let k = {
            let mut h: u64 = 0x1234_5678;
            fnv(&mut h, doc.title.as_bytes());
            fnv(&mut h, &v.width.to_bits().to_le_bytes());
            fnv(&mut h, &v.sz.body.to_bits().to_le_bytes());
            h
        };
        if let Some(l) = cache.cached(k) {
            return l;
        }
        let l = Rc::new(layout_title(c, &doc.title, v.width, &v.sz));
        cache.put(k, l.clone());
        return l;
    }
    let n = vi - 1;
    if n >= doc.buf.line_count() {
        return widgets::footnotes_row(cache, c, doc, v);
    }
    if doc.folded_away(n) {
        return Rc::new(Lay::hidden(v.sz.body));
    }
    let reading = doc.mode == Mode::Reading;
    if doc.mode != Mode::Source {
        if let Some(span) = doc.span(n) {
            // A table is edited inside the drawn table, never as its source
            // (outside source mode), as Obsidian 1.15 does.
            let active = !reading
                && span.kind != crate::editor::SpanKind::Table
                && doc.caret_in(span.first, span.last);
            if !active {
                if n != span.first {
                    return Rc::new(Lay::hidden(v.sz.body));
                }
                return widgets::block(cache, c, doc, span, v);
            }
        }
    }
    let (a, b) = active_lines(doc);
    let block = doc.block(n);
    let text = doc.line(n);
    if reading && hidden_in_reading(doc, n) {
        return Rc::new(Lay::hidden(v.sz.body));
    }
    let extra_active = doc.extra.iter().any(|(_, p)| p.line == n);
    let gap = reading && block == Block::Blank && n < doc.buf.last_line() && {
        // The blank line after the last line reading view shows.
        let mut p = n;
        let mut seen = false;
        while p > 0 && n - p < 64 {
            p -= 1;
            if doc.line(p).trim().is_empty() {
                break;
            }
            if !hidden_in_reading(doc, p) {
                seen = true;
                break;
            }
        }
        seen
    };
    let fold = if doc.mode == Mode::Source
        || reading
        || !matches!(block, Block::Heading { .. }) && !block.is_list()
    {
        0
    } else if doc.folds.contains(&n) {
        2
    } else if doc.fold_range(n).is_some() {
        1
    } else {
        0
    };
    let ctx = Ctx {
        block,
        mode: doc.mode,
        active: ((n >= a && n <= b) || extra_active) && !reading && !doc.fresh,
        width: v.width,
        body: v.sz.body,
        code: v.sz.code,
        gap,
        comment: doc.mode != Mode::Source && doc.in_comment(n),
        fold,
        struck: reading && block.is_list() && under_done_task(doc, n),
    };
    let lang = if block == Block::Code {
        doc.fence_lang(n)
    } else {
        ""
    };
    let nums: Vec<usize> = if reading && (text.contains("[^") || text.contains("^[")) {
        let f = doc.footnotes();
        let i = f.refs.partition_point(|r| r.0 < n);
        f.refs
            .get(i..)
            .unwrap_or(&[])
            .iter()
            .take_while(|r| r.0 == n)
            .map(|r| r.2)
            .collect()
    } else {
        Vec::new()
    };
    cache.get(c, text, ctx, lang, &nums)
}

/// Lines reading view leaves out: comments, footnote definitions (they
/// are listed at the end), lines of nothing but syntax.
fn hidden_in_reading(doc: &Doc, n: usize) -> bool {
    let text = doc.line(n);
    if doc.in_comment(n) {
        return true;
    }
    if doc.fences.in_code(n) || doc.fences.is_fence(n) {
        return false;
    }
    crate::blocks::footnote_def(text).is_some() || md::reading_empty(text)
}

pub fn rows_total(doc: &Doc) -> usize {
    doc.buf.line_count() + 1 + usize::from(has_notes_row(doc))
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

/// Move every caret up or down by `n` rows as drawn (wrapped rows count;
/// folded lines are stepped over).
pub fn move_vertical(
    cache: &mut LayCache,
    c: u64,
    doc: &mut Doc,
    v: &View,
    down: bool,
    n: usize,
    extend: bool,
) {
    if doc.extra.is_empty() {
        move_one(cache, c, doc, v, down, n, extend);
        return;
    }
    let carets = doc.carets();
    doc.extra.clear();
    let mut out: Vec<(Option<Pos>, Pos)> = Vec::new();
    for (an, p) in carets {
        doc.anchor = an;
        doc.cur = p;
        doc.goal_x = None;
        move_one(cache, c, doc, v, down, n, extend);
        out.push((doc.anchor, doc.cur));
    }
    out.dedup_by(|x, y| x.1 == y.1);
    let lead = out.remove(0);
    doc.anchor = lead.0;
    doc.cur = lead.1;
    out.retain(|x| x.1 != lead.1);
    doc.extra = out;
    doc.goal_x = None;
}

#[allow(clippy::too_many_arguments)]
fn move_one(
    cache: &mut LayCache,
    c: u64,
    doc: &mut Doc,
    v: &View,
    down: bool,
    n: usize,
    extend: bool,
) {
    // In a table Up and Down keep the column, row to row.
    if !extend && doc.mode == Mode::Live && n == 1 {
        let now = doc.dirty_since.unwrap_or(0);
        if crate::tables::key(doc, if down { "ArrowDown" } else { "ArrowUp" }, false, now).is_some()
        {
            return;
        }
    }
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
                while line < last && doc.folded_away(line) {
                    line += 1;
                }
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
            while line > 0 && doc.folded_away(line) {
                line -= 1;
            }
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
    // Arriving on a table row puts the caret in a cell.
    if !extend && doc.mode == Mode::Live {
        crate::tables::snap(doc);
    }
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
    shown.find.clear();
    shown.area = Some(v.area);
    shown.x = v.x;
    shown.width = v.width;
    let total = rows_total(doc);
    let _ = canvas2d::set_clip(c, v.area.x, v.area.y, v.area.w, v.area.h);
    let mut y = v.area.y - doc.top_off;
    let mut vi = doc.top;
    let sel = doc.selection();
    let (act_a, act_b) = active_lines(doc);
    let pointer_y = cache.pointer.map(|p| p.1);
    while vi < total && y < v.area.bottom() {
        let l = lay(cache, c, doc, vi, v);
        let n = vi.saturating_sub(1);
        let text: &str = if vi == 0 {
            &doc.title
        } else if n < doc.buf.line_count() {
            doc.line(n)
        } else {
            ""
        };
        if l.deco == Deco::Hidden {
            shown.rows.push((vi, y, l.clone()));
            vi += 1;
            continue;
        }
        let opens = vi > 0 && doc.fences.opens(n);
        let lang = if opens { md::fence_lang(text) } else { "" };
        let hovered = pointer_y.is_some_and(|py| py >= y && py < y + l.h);
        draw_line(
            c,
            &l,
            text,
            v.x,
            y,
            v.width,
            t,
            resolved,
            &LineDraw {
                opens_fence: opens,
                lang,
                hovered,
                env: &cache.env,
            },
        );
        if vi > 0 && n < doc.buf.line_count() && l.deco != Deco::Widget {
            // Find matches, under the selection.
            if let Some(f) = doc.find.as_ref() {
                let i = f.matches.partition_point(|m| m.0 < n);
                for (mi, (ml, mc, mlen)) in f.matches.iter().enumerate().skip(i) {
                    if *ml != n {
                        break;
                    }
                    let cur = f.current == Some(mi);
                    draw_range(
                        c,
                        &l,
                        text,
                        v.x,
                        y,
                        *mc,
                        mc + mlen,
                        if cur { t.sel } else { t.find },
                    );
                }
            }
            // The selection, over the backgrounds and under nothing that hides it.
            if let Some((a, b)) = sel {
                if n >= a.line && n <= b.line {
                    let from = if n == a.line { a.col } else { 0 };
                    let to = if n == b.line { b.col } else { text.len() };
                    draw_selection(c, &l, text, v.x, y, from, to, n < b.line, t);
                }
            }
            for (an, p) in &doc.extra {
                if let Some(an) = an {
                    let (a, b) = if *an < *p { (*an, *p) } else { (*p, *an) };
                    if n >= a.line && n <= b.line {
                        let from = if n == a.line { a.col } else { 0 };
                        let to = if n == b.line { b.col } else { text.len() };
                        draw_selection(c, &l, text, v.x, y, from, to, n < b.line, t);
                    }
                }
            }
            if cache.line_numbers && doc.mode != Mode::Reading {
                // The gutter: each line's number at its first row.
                if let Some(rl) = l.rows.first() {
                    let mut s = String::new();
                    crate::text::push_num(&mut s, n + 1);
                    let size = (v.sz.body * 0.8).round();
                    let w = look::width(c, &s, size, 400);
                    look::text(c, &s, v.x - 14.0 - w, y + rl.base, size, 400, t.faint);
                }
            }
            if caret_on && doc.mode != Mode::Reading && !doc.fresh {
                if n == doc.cur.line {
                    draw_caret(c, &l, text, v, y, doc.cur.col, t);
                }
                for (_, p) in &doc.extra {
                    if p.line == n {
                        draw_caret(c, &l, text, v, y, p.col, t);
                    }
                }
            }
            let shown_text = text.get(..text.len().min(240)).unwrap_or("");
            if !shown_text.is_empty() && (n < act_a || n > act_b || doc.mode == Mode::Reading) {
                said.text(r(v.x, y, v.width, l.h), &md::plain(shown_text, 0));
            } else if !shown_text.is_empty() {
                said.text(r(v.x, y, v.width, l.h), shown_text);
            }
        } else if let Some(w) = l.widget.as_ref() {
            if caret_on && !doc.fresh && doc.mode == Mode::Live {
                if let Some((cx, cy, ch)) = widgets::table_caret(c, w, doc.cur) {
                    fill(c, r(v.x + cx - 0.5, y + cy, 1.6 * v.sz.scale, ch), t.text);
                }
            }
            let s = w.said();
            if !s.is_empty() {
                said.text(r(v.x, y, v.width, l.h), &s);
            }
        } else if vi == 0 {
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
    if doc.find.is_some() {
        widgets::draw_find_bar(c, doc, v, t, shown, caret_on, said);
    }
}

fn draw_caret(c: u64, l: &Lay, text: &str, v: &View, y: f32, col: usize, t: &Theme) {
    let (row, x) = l.x_of(c, text, col);
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

#[allow(clippy::too_many_arguments)]
fn draw_range(c: u64, l: &Lay, text: &str, ox: f32, oy: f32, from: usize, to: usize, col: Rgba) {
    for rl in l.rows.iter() {
        let a = from.max(rl.start);
        let b = to.min(rl.end);
        if a >= b {
            continue;
        }
        let (_, xa) = l.x_of(c, text, a);
        let (_, xb) = l.x_of(c, text, b);
        round(
            c,
            r(ox + xa, oy + rl.top + 1.0, (xb - xa).max(2.0), rl.h - 2.0),
            2.0,
            col,
        );
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

/// What drawing one line needs beyond the line itself.
pub struct LineDraw<'a> {
    pub opens_fence: bool,
    pub lang: &'a str,
    pub hovered: bool,
    pub env: &'a Env,
}

/// Obsidian's code colours on a dark or light page.
pub fn tok_colour(tok: Tok, t: &Theme) -> Rgba {
    let dark = t.bg.r < 0.5;
    let hx = |v: u32| look::hex(v);
    match (tok, dark) {
        (Tok::Keyword, true) => hx(0xFA99CD),
        (Tok::Keyword, false) => hx(0xD53984),
        (Tok::String, true) => hx(0x44CF6E),
        (Tok::String, false) => hx(0x0C9E4A),
        (Tok::Comment, _) => hx(0x999999),
        (Tok::Number, true) => hx(0xA882FF),
        (Tok::Number, false) => hx(0x8A5CF5),
        (Tok::Function, true) => hx(0xE0DE71),
        (Tok::Function, false) => hx(0xB38C00),
        (Tok::Type, true) => hx(0x53DFDD),
        (Tok::Type, false) => hx(0x00A3A0),
        (Tok::Property, true) => hx(0x53DFDD),
        (Tok::Property, false) => hx(0x00A3A0),
        (Tok::Operator, true) => hx(0xFB464C),
        (Tok::Operator, false) => hx(0xE93147),
        (Tok::Punct, _) => t.muted,
        (Tok::Plain, _) => t.text,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw_line(
    c: u64,
    l: &Lay,
    text: &str,
    ox: f32,
    oy: f32,
    width: f32,
    t: &Theme,
    resolved: &dyn Fn(&str) -> bool,
    d: &LineDraw<'_>,
) {
    let k = l.size / 16.0;
    match l.deco {
        Deco::Widget => {
            if let Some(w) = l.widget.as_ref() {
                widgets::draw(c, w, ox, oy, width, t, resolved, d.env);
            }
            return;
        }
        Deco::Hidden => return,
        Deco::Code | Deco::Fence { .. } => {
            let is_fence = matches!(l.deco, Deco::Fence { .. });
            let top_pad = if d.opens_fence { 2.0 } else { 0.0 };
            fill(
                c,
                r(ox - 6.0, oy + top_pad, width + 12.0, l.h - top_pad),
                t.code_bg,
            );
            if let Deco::Fence { shown: false, .. } = l.deco {
                if d.opens_fence && is_fence && !d.lang.is_empty() {
                    let name = highlight::display_name(d.lang);
                    let lw = look::width(c, name, 11.0, 400);
                    look::text(
                        c,
                        name,
                        ox + width - lw - 4.0,
                        oy + 13.0,
                        11.0,
                        400,
                        t.faint,
                    );
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
                    match l.task_mark {
                        b'-' => look::line(
                            c,
                            bx + s * 0.25,
                            by + s * 0.5,
                            bx + s * 0.75,
                            by + s * 0.5,
                            1.8 * k,
                            t.on_accent,
                        ),
                        b'/' => look::line(
                            c,
                            bx + s * 0.3,
                            by + s * 0.75,
                            bx + s * 0.7,
                            by + s * 0.25,
                            1.8 * k,
                            t.on_accent,
                        ),
                        b'x' | b'X' => {
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
                        }
                        _ => {
                            let mut b = [0u8; 4];
                            let ch = char::from(l.task_mark).encode_utf8(&mut b);
                            let cw = look::width(c, ch, s * 0.8, 700);
                            look::text(
                                c,
                                ch,
                                bx + (s - cw) * 0.5,
                                by + s * 0.8,
                                s * 0.8,
                                700,
                                t.on_accent,
                            );
                        }
                    }
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
    // The fold chevron in the margin: always when folded, on hover when it
    // could fold.
    if l.fold == 2 || (l.fold == 1 && d.hovered) {
        if let Some(rl) = l.rows.first() {
            let cy = oy + rl.base - l.size * 0.32;
            look::chevron(c, ox - 14.0, cy, l.fold == 1, 1.0, t.muted);
        }
    }
    for run in &l.runs {
        if run.hidden || run.end < run.start || (run.end == run.start && run.bx == NONE) {
            continue;
        }
        let Some(rl) = l.rows.get(run.row as usize) else {
            continue;
        };
        let s = text.get(run.start..run.end).unwrap_or("");
        let x = ox + run.x;
        let base = oy + rl.base + run.rise;
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
        if st.kbd {
            round(
                c,
                r(x, base - run.size * 0.95, run.w, run.size * 1.3),
                3.0,
                t.code_bg,
            );
            look::outline(
                c,
                r(x, base - run.size * 0.95, run.w, run.size * 1.3),
                3.0,
                1.0,
                t.border,
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
        if st.comment || st.blockid || st.syntax {
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
        } else if st.tag || (st.footnote && run.alt != NONE) {
            col = t.link;
        } else if st.footnote {
            col = t.muted;
        } else if l.deco == Deco::Code {
            col = tok_colour(run.tok, t);
        } else if run.tok == Tok::Punct || l.struck {
            col = t.muted;
        } else if l.deco == Deco::Title {
            col = t.text;
        }
        if run.bx != NONE {
            if let Some(b) = l.boxes.get(run.bx as usize) {
                draw_math(c, b, x, oy + rl.base, col, t);
            }
            continue;
        }
        if run.alt != NONE {
            if let Some(a) = l.alts.get(run.alt as usize) {
                look::text_styled(c, a, x, base, run.size, run.style(), col);
                if st.link {
                    fill(c, r(x, base + 2.0, run.w, 1.0), look::alpha(col, 0.7));
                }
            }
            continue;
        }
        let tx = if st.kbd { x + 3.0 * k } else { x };
        look::text_styled(c, s, tx, base, run.size, run.style(), col);
        if (st.link && !st.syntax) || st.underline {
            fill(c, r(x, base + 2.0, run.w, 1.0), look::alpha(col, 0.7));
        }
        if st.strike {
            fill(c, r(x, base - run.size * 0.3, run.w, 1.2), col);
        }
    }
    // A folded heading says there is more.
    if l.fold == 2 {
        if let Some(rl) = l.rows.last() {
            let x = ox + l.end_x() + 8.0;
            round(
                c,
                r(x, oy + rl.base - l.size * 0.55, 22.0, l.size * 0.6),
                4.0,
                t.code_bg,
            );
            look::text(
                c,
                "…",
                x + 5.0,
                oy + rl.base - l.size * 0.1,
                l.size * 0.7,
                400,
                t.muted,
            );
        }
    }
}

/// Draw a math box with its baseline at `base`.
pub fn draw_math(c: u64, b: &MathBox, x: f32, base: f32, col: Rgba, t: &Theme) {
    for op in &b.ops {
        match op {
            math::Op::Text {
                s,
                x: ox,
                y,
                size,
                font,
                error,
            } => {
                let ink = if *error { t.danger } else { col };
                look::text_styled(c, s, x + ox, base + y, *size, math_style(*font), ink);
            }
            math::Op::Rule { x: ox, y, w, h } => fill(c, r(x + ox, base + y, *w, h.max(1.0)), col),
            math::Op::Line { x0, y0, x1, y1, w } => {
                look::line(c, x + x0, base + y0, x + x1, base + y1, *w, col)
            }
        }
    }
}

/// A wiki link's full target as written: the name and any `#heading` or
/// `#^block` after it (but not the `|alias`).
fn wiki_full(text: &str, a: usize, b: usize) -> String {
    let rest = text.get(b..).unwrap_or("");
    let end = rest.find(['|', ']']).unwrap_or(rest.len());
    let mut s = String::from(text.get(a..b).unwrap_or(""));
    s.push_str(rest.get(..end).unwrap_or(""));
    s
}

fn run_act(run: &Run, text: &str) -> Option<Act> {
    match run.target? {
        Target::Wiki(a, b) => Some(Act::Follow(wiki_full(text, a, b))),
        Target::Tag(a, b) => Some(Act::Tag(String::from(text.get(a + 1..b).unwrap_or("")))),
        Target::Url(a, b) => Some(Act::Url(String::from(text.get(a..b).unwrap_or("")))),
        Target::Footnote(..) => None,
    }
}

/// What a press at (px, py) lands on, from what was drawn last frame.
/// `follow` is whether a link under it should be followed (it is not on
/// the line being edited unless Cmd is held).
pub fn press(c: u64, doc: &Doc, shown: &Shown, px: f32, py: f32, follow: bool) -> Option<Act> {
    // The bar's parts before the bar itself.
    for (h, rc) in shown.find.iter().rev() {
        if rc.contains(px, py) {
            return Some(Act::Find(*h));
        }
    }
    let (vi, y, l) = shown
        .rows
        .iter()
        .rev()
        .find(|(_, y, l)| py >= *y && l.h > 0.0)?;
    if *vi == 0 {
        return None;
    }
    let n = vi - 1;
    let lx = px - shown.x;
    let ly = py - y;
    if n >= doc.buf.line_count() {
        return l
            .widget
            .as_ref()
            .and_then(|w| widgets::press(c, w, lx, ly, shown.width, n));
    }
    if l.deco == Deco::Widget {
        return l
            .widget
            .as_ref()
            .and_then(|w| widgets::press(c, w, lx, ly, shown.width, n));
    }
    let text = doc.line(n);
    let row = l.row_at(ly);
    if l.fold > 0 && lx < 0.0 && lx > -24.0 && row == 0 {
        return Some(Act::Fold(n));
    }
    if let Deco::Task(_) = l.deco {
        if lx < l.x0 && lx >= l.marker - 12.0 && row == 0 && doc.mode != Mode::Reading {
            return Some(Act::Task(n));
        }
    }
    if doc.mode != Mode::Source && follow {
        if let Some(run) = l.run_at(row, lx) {
            return run_act(run, text);
        }
    }
    None
}

/// The link under the pointer, for the page preview: its full target.
pub fn link_at(c: u64, doc: &Doc, shown: &Shown, px: f32, py: f32) -> Option<(String, f32, f32)> {
    let (vi, y, l) = shown
        .rows
        .iter()
        .rev()
        .find(|(_, y, l)| py >= *y && l.h > 0.0)?;
    if *vi == 0 || py > y + l.h {
        return None;
    }
    let n = vi - 1;
    let lx = px - shown.x;
    let ly = py - y;
    if l.deco == Deco::Widget || n >= doc.buf.line_count() {
        let w = l.widget.as_ref()?;
        return match widgets::press(c, w, lx, ly, shown.width, n) {
            Some(Act::Follow(s)) => Some((s, px, y + ly + 12.0)),
            _ => None,
        };
    }
    let text = doc.line(n);
    let row = l.row_at(ly);
    let run = l.run_at(row, lx)?;
    match run_act(run, text)? {
        Act::Follow(s) => {
            let rl = l.rows.get(row)?;
            Some((s, shown.x + run.x, y + rl.top + rl.h))
        }
        _ => None,
    }
}
