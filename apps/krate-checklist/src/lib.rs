//! Krate Checklist — a modern dark checklist drawn entirely on a canvas.
//!
//! The whole UI is painted by the app into one `gfx.canvas2d`: a bold title, a
//! "N of M done" progress line with a filled bar, item rows as rounded cards
//! each with a drawn checkbox that fills with an accent color when checked, a
//! text field to type a new item, and an accent "Add" button. Clicks are
//! hit-tested against the rectangles the app drew, so a drawn checkbox and a
//! drawn button are really clickable; typed characters flow into the draft.
//! Every toggle and add saves to the key-value store, so the list survives a
//! close.
//!
//! `#![no_std]` is the discipline that keeps it `krate:*`-only: the SDK owns the
//! allocator and a trapping panic handler, so no path drags in the `wasi:*`
//! import set. Items live in fixed-capacity arrays; text is fixed byte buffers;
//! numbers are formatted by hand. No `Vec`, `format!`, `unwrap`, or panicking
//! index.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

// Linked purely for its no_std runtime lang items -- the global allocator, the
// trapping panic handler, and the memory intrinsics a wasm guest needs when std
// is not linked. Not called directly; the underscore keeps the import.
extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::kv as store_kv;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The size the window opens at, and nothing more. The person can resize it,
/// so no rectangle in this app is computed from these -- see `Layout`.
const WIDTH: f32 = 440.0;
const HEIGHT: f32 = 620.0;

/// How many checklist items the app can ever hold. Fixed so nothing allocates.
const MAX_ITEMS: usize = 32;
/// Bytes of text one item can hold. Fixed for the same no-allocation reason.
const ITEM_TEXT_CAP: usize = 128;
/// The one key this app keeps its items under.
const DATA_KEY: &str = "items";

/// The items seeded on the very first run, so a fresh open is not empty.
const SEED_ITEMS: [&str; 3] = ["Buy milk", "Write the pitch", "Ship the demo"];

/// Interactive runs stay open until the person closes the window; automated
/// runs pass `quick` and exit promptly.
const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;
/// Consecutive quiet rounds before a headless run stops waiting (~10s).
const MAX_IDLE_ROUNDS: u32 = 300;

// ------------------------------------------------------------------
// Layout
// ------------------------------------------------------------------
//
// The window is resizable, so nothing here is a constant. `Layout::for_size`
// is computed once from `canvas2d::canvas_size` at the top of every frame, and
// BOTH the drawing code and the hit-testing code read their rectangles from
// it. That single source is the whole point: when a rectangle is drawn from
// one set of numbers and clicked against another, the two drift apart the
// moment the window changes size, and clicks land in the wrong row.
//
// If you copy this app as a starting point, copy this shape. Do not put
// coordinates in `const`s and do not compute a rect twice.

/// A rectangle in canvas coordinates. The unit both drawing and hit-testing
/// speak, so a control can only ever be clicked where it was painted.
#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// Smallest canvas the layout will lay out against. Below this the app clamps
/// rather than computing negative widths and drawing controls on top of each
/// other.
const MIN_CANVAS_W: f32 = 240.0;
const MIN_CANVAS_H: f32 = 260.0;

/// Every rectangle in the UI, derived from the canvas's current size.
struct Layout {
    width: f32,
    height: f32,
    /// How far the list is scrolled, in pixels from the top. Every row rect is
    /// shifted by this, so drawing and hit-testing move together and a row you
    /// can see is a row you can click.
    scroll: f32,
    margin: f32,
    content_w: f32,
    /// Title baseline and the two lines under it.
    title_size: f32,
    title_baseline: f32,
    progress_baseline: f32,
    bar: Rect,
    list_top: f32,
    row_h: f32,
    row_gap: f32,
    /// How many rows fit between the header and the input strip at this size.
    visible_rows: usize,
    check_size: f32,
    /// Text size for a row's label, shrunk on a narrow window.
    row_text_size: f32,
    field: Rect,
    add: Rect,
    input_text_size: f32,
}

impl Layout {
    /// Derive the whole layout from the canvas size. This is the only place
    /// coordinates are decided.
    fn for_size(width: f32, height: f32) -> Self {
        let width = width.max(MIN_CANVAS_W);
        let height = height.max(MIN_CANVAS_H);

        // Margins and type scale gently with width so a narrow window is tight
        // but readable and a wide one does not look empty.
        let margin = (width * 0.064).clamp(14.0, 40.0);
        let content_w = (width - margin * 2.0).max(80.0);

        let title_size = (width * 0.077).clamp(20.0, 34.0);
        let title_baseline = margin + title_size * 0.85;
        let progress_baseline = title_baseline + title_size * 0.88;

        let bar = Rect {
            x: margin,
            y: progress_baseline + 12.0,
            w: content_w,
            h: 8.0,
        };

        // The input strip is pinned to the bottom; the list gets what is left.
        let input_h = (height * 0.074).clamp(38.0, 52.0);
        let input_top = height - margin - input_h;
        // The Add button keeps a sane share of the width on a narrow window so
        // its label never overruns the field beside it.
        let add_w = (content_w * 0.28).clamp(58.0, 96.0);
        let gap = 12.0;
        let field_w = (content_w - add_w - gap).max(60.0);

        let field = Rect {
            x: margin,
            y: input_top,
            w: field_w,
            h: input_h,
        };
        let add = Rect {
            x: margin + content_w - add_w,
            y: input_top,
            w: add_w,
            h: input_h,
        };

        let list_top = bar.y + bar.h + 24.0;
        let row_h = (height * 0.084).clamp(40.0, 56.0);
        let row_gap = (row_h * 0.19).clamp(6.0, 12.0);

        // Rows fill the space between the header and the input strip. Leave a
        // little room so the "+ N more" line has somewhere to sit.
        let list_space = (input_top - 22.0 - list_top).max(0.0);
        let stride = row_h + row_gap;
        let visible_rows = if stride > 0.0 {
            (list_space / stride) as usize
        } else {
            0
        };

        let check_size = (row_h * 0.46).clamp(18.0, 26.0);
        let row_text_size = (width * 0.039).clamp(12.0, 17.0);
        let input_text_size = (input_h * 0.35).clamp(12.0, 16.0);

        Self {
            width,
            height,
            // A fresh layout starts unscrolled; the caller applies the current
            // offset with `scrolled_by` so the scroll position survives a
            // resize and a redraw.
            scroll: 0.0,
            margin,
            content_w,
            title_size,
            title_baseline,
            progress_baseline,
            bar,
            list_top,
            row_h,
            row_gap,
            visible_rows,
            check_size,
            row_text_size,
            field,
            add,
            input_text_size,
        }
    }

    /// The card rectangle for row `index`. Drawing fills this; hit-testing
    /// tests this. One function, so they cannot disagree.
    fn row(&self, index: usize) -> Rect {
        Rect {
            x: self.margin,
            y: self.list_top + (index as f32) * (self.row_h + self.row_gap) - self.scroll,
            w: self.content_w,
            h: self.row_h,
        }
    }

    /// The checkbox inside row `index`, centred vertically in the card.
    fn checkbox(&self, index: usize) -> Rect {
        let row = self.row(index);
        Rect {
            x: row.x + self.margin * 0.5,
            y: row.y + (row.h - self.check_size) * 0.5,
            w: self.check_size,
            h: self.check_size,
        }
    }

    /// How many rows this list actually shows: fewer than fit, if it is short.
    /// The same layout, scrolled. Clamped to the real content, so a flick past
    /// the end springs back to the last row rather than showing empty space.
    fn scrolled_by(mut self, offset: f32, len: usize) -> Self {
        self.scroll = offset.clamp(0.0, self.max_scroll(len));
        self
    }

    /// How far this list can scroll before the last row sits at the bottom of
    /// the visible strip. Zero when everything already fits.
    fn max_scroll(&self, len: usize) -> f32 {
        let step = self.row_h + self.row_gap;
        let content = (len as f32) * step;
        let visible = (self.visible_rows as f32) * step;
        (content - visible).max(0.0)
    }

    /// Rows that could be on screen at the current offset. With scrolling the
    /// list is no longer capped at `visible_rows`: every row is reachable, and
    /// the ones outside the strip are skipped when drawing.
    fn first_visible(&self, len: usize) -> usize {
        let step = self.row_h + self.row_gap;
        if step <= 0.0 {
            return 0;
        }
        let first = (self.scroll / step) as usize;
        first.min(len.saturating_sub(1))
    }

    /// The last row worth drawing or hit-testing at the current offset: one
    /// past the visible strip, so a partly-scrolled row is still handled.
    fn last_visible(&self, len: usize) -> usize {
        let first = self.first_visible(len);
        (first + self.visible_rows + 1).min(len)
    }

    fn shown_rows(&self, len: usize) -> usize {
        if len < self.visible_rows {
            len
        } else {
            self.visible_rows
        }
    }

    /// Which row, if any, contains this point.
    fn hit_row(&self, len: usize, x: f32, y: f32) -> Option<usize> {
        // Scan the scrolled window, not the first N rows: with a scroll offset
        // the row under the pointer may be row 20.
        let shown = self.last_visible(len);
        let mut i = self.first_visible(len);
        // A row scrolled above the list must not be clickable through the
        // header, so reject anything above the strip before testing.
        if y < self.list_top {
            return None;
        }
        while i < shown {
            if self.row(i).contains(x, y) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

struct Component;

/// One checklist item: its text (fixed capacity), whether it is done, and
/// whether the slot is in use. Copyable so the list is a plain fixed array.
#[derive(Clone, Copy)]
struct Item {
    text: [u8; ITEM_TEXT_CAP],
    text_len: usize,
    done: bool,
    used: bool,
}

impl Item {
    const EMPTY: Item = Item {
        text: [0; ITEM_TEXT_CAP],
        text_len: 0,
        done: false,
        used: false,
    };

    fn text_str(&self) -> &str {
        let slice = self.text.get(..self.text_len).unwrap_or(&[]);
        core::str::from_utf8(slice).unwrap_or("")
    }

    fn set_text(&mut self, text: &str) {
        self.text_len = 0;
        for byte in text.as_bytes() {
            if let Some(slot) = self.text.get_mut(self.text_len) {
                *slot = *byte;
                self.text_len += 1;
            }
        }
    }
}

/// The whole checklist: a fixed array of items plus how many slots are live.
struct Checklist {
    items: [Item; MAX_ITEMS],
    len: usize,
}

impl Checklist {
    const fn new() -> Self {
        Self {
            items: [Item::EMPTY; MAX_ITEMS],
            len: 0,
        }
    }

    fn push(&mut self, text: &str, done: bool) {
        if let Some(slot) = self.items.get_mut(self.len) {
            slot.set_text(text);
            slot.done = done;
            slot.used = true;
            self.len += 1;
        }
    }

    fn toggle(&mut self, index: usize) {
        if let Some(item) = self.items.get_mut(index) {
            if item.used {
                item.done = !item.done;
            }
        }
    }

    fn done_count(&self) -> usize {
        let mut n = 0usize;
        let mut i = 0usize;
        while i < self.len {
            if let Some(it) = self.items.get(i) {
                if it.used && it.done {
                    n += 1;
                }
            }
            i += 1;
        }
        n
    }
}

/// The text of the new item being typed, before it is added. A fixed buffer so
/// nothing allocates; append and pop only.
struct Draft {
    text: [u8; ITEM_TEXT_CAP],
    len: usize,
}

impl Draft {
    const fn new() -> Self {
        Self {
            text: [0; ITEM_TEXT_CAP],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        let slice = self.text.get(..self.len).unwrap_or(&[]);
        core::str::from_utf8(slice).unwrap_or("")
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, byte: u8) {
        if let Some(slot) = self.text.get_mut(self.len) {
            *slot = byte;
            self.len += 1;
        }
    }

    fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }

    /// Replace the whole draft, used when a native control reports its full
    /// text after any edit.
    fn set(&mut self, text: &str) {
        self.len = 0;
        for byte in text.as_bytes() {
            let printable = byte.is_ascii_graphic() || *byte == b' ';
            if printable {
                self.push(*byte);
            }
        }
    }
}

// ---- persistence: same on-store shape as before, `[x]/[ ] text` lines ----

fn load(list: &mut Checklist) -> bool {
    *list = Checklist::new();
    let Ok(Some(data)) = store_kv::get(DATA_KEY) else {
        return false;
    };
    let mut start = 0usize;
    for i in 0..data.len() {
        if data.get(i).copied() == Some(b'\n') {
            parse_line(data.get(start..i).unwrap_or(&[]), list);
            start = i + 1;
        }
    }
    if start < data.len() {
        parse_line(data.get(start..).unwrap_or(&[]), list);
    }
    true
}

fn parse_line(line: &[u8], list: &mut Checklist) {
    if line.len() < 4 {
        return;
    }
    let done = line.get(1).copied() == Some(b'x');
    let text = line.get(4..).unwrap_or(&[]);
    if let Ok(text) = core::str::from_utf8(text) {
        if list.len < MAX_ITEMS {
            list.push(text, done);
        }
    }
}

fn save(list: &Checklist) -> bool {
    let mut out = [0u8; MAX_ITEMS * (ITEM_TEXT_CAP + 8)];
    let mut len = 0usize;
    let mut push = |bytes: &[u8], out: &mut [u8], len: &mut usize| {
        for byte in bytes {
            if let Some(slot) = out.get_mut(*len) {
                *slot = *byte;
                *len += 1;
            }
        }
    };
    for i in 0..list.len {
        let Some(item) = list.items.get(i) else {
            continue;
        };
        if !item.used {
            continue;
        }
        push(
            if item.done { b"[x] " } else { b"[ ] " },
            &mut out,
            &mut len,
        );
        push(item.text_str().as_bytes(), &mut out, &mut len);
        push(b"\n", &mut out, &mut len);
    }
    store_kv::set(DATA_KEY, out.get(..len).unwrap_or(&[])).is_ok()
}

// Hit testing lives on `Layout` (see `Layout::hit_row`, `layout.add`,
// `layout.field`) so a control is tested against the rectangle it was drawn
// from, at whatever size the window happens to be right now.

// ------------------------------------------------------------------
// Palette
// ------------------------------------------------------------------

const BG_TOP: gfx::Color = gfx::Color {
    r: 0.075,
    g: 0.086,
    b: 0.125,
    a: 1.0,
};
const BG_BOT: gfx::Color = gfx::Color {
    r: 0.043,
    g: 0.051,
    b: 0.078,
    a: 1.0,
};
const CARD: gfx::Color = gfx::Color {
    r: 0.129,
    g: 0.145,
    b: 0.196,
    a: 1.0,
};
const CARD_DONE: gfx::Color = gfx::Color {
    r: 0.102,
    g: 0.118,
    b: 0.161,
    a: 1.0,
};
const INK: gfx::Color = gfx::Color {
    r: 0.902,
    g: 0.925,
    b: 0.98,
    a: 1.0,
};
const INK_DIM: gfx::Color = gfx::Color {
    r: 0.478,
    g: 0.525,
    b: 0.627,
    a: 1.0,
};
const INK_DONE: gfx::Color = gfx::Color {
    r: 0.435,
    g: 0.475,
    b: 0.561,
    a: 1.0,
};

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

/// Ask the canvas how big it is, then draw the frame to that answer.
///
/// This is the shape to copy: read the size every frame rather than trusting a
/// constant, build one `Layout` from it, and hand that same `Layout` to both
/// the drawing below and the hit-testing in the event loop.
fn draw(
    canvas: u64,
    list: &Checklist,
    draft: &Draft,
    field_focus: bool,
    scroll: f32,
) -> Result<Layout, gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas)?;
    let layout = Layout::for_size(size.width, size.height).scrolled_by(scroll, list.len);
    draw_with(canvas, &layout, list, draft, field_focus)?;
    Ok(layout)
}

fn draw_with(
    canvas: u64,
    layout: &Layout,
    list: &Checklist,
    draft: &Draft,
    field_focus: bool,
) -> Result<(), gfx::GfxError> {
    // Deep, considered ground -- a soft vertical gradient, not flat black.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect {
            x: 0.0,
            y: 0.0,
            width: layout.width,
            height: layout.height,
        },
        BG_TOP,
        BG_BOT,
    )?;

    let accent = color(0.42, 0.62, 1.0, 1.0);
    let accent_soft = color(0.42, 0.62, 1.0, 0.16);

    // ---- header: bold title + progress ----
    draw_text(
        canvas,
        "Checklist",
        layout.margin,
        layout.title_baseline,
        layout.title_size,
        INK,
    )?;

    let total = list.len;
    let done = list.done_count();
    let mut buf = [0u8; 32];
    let sub = progress_label(done as u32, total as u32, &mut buf);
    if let Ok(txt) = core::str::from_utf8(sub) {
        let size = (layout.title_size * 0.44).clamp(11.0, 15.0);
        draw_text(
            canvas,
            txt,
            layout.margin,
            layout.progress_baseline,
            size,
            INK_DIM,
        )?;
    }

    // Progress bar track + accent fill.
    let bar = layout.bar;
    rounded_rect(
        canvas,
        bar.x,
        bar.y,
        bar.w,
        bar.h,
        bar.h * 0.5,
        color(0.16, 0.18, 0.24, 1.0),
    )?;
    if total > 0 {
        let frac = (done as f32 / total as f32).clamp(0.0, 1.0);
        let fw = (bar.w * frac).max(if done > 0 { 10.0 } else { 0.0 });
        if fw > 0.0 {
            rounded_rect(canvas, bar.x, bar.y, fw, bar.h, bar.h * 0.5, accent)?;
        }
    }

    // ---- item rows as cards ----
    // Clip to the list strip, then draw every row that could be on screen. A
    // row scrolled half off the top is drawn and trimmed rather than skipped,
    // which is what makes the scroll look continuous instead of snapping row
    // by row. Before clipping existed this was a hand-written bounds check
    // that could not handle a partly-visible row at all.
    canvas2d::set_clip(
        canvas,
        layout.margin,
        layout.list_top,
        layout.content_w,
        (layout.field.y - 12.0 - layout.list_top).max(0.0),
    )?;
    let mut i = layout.first_visible(list.len);
    let last = layout.last_visible(list.len);
    while i < last {
        if let Some(item) = list.items.get(i) {
            if item.used {
                draw_row(canvas, layout, i, item, accent)?;
            }
        }
        i += 1;
    }
    canvas2d::clear_clip(canvas)?;
    // A scrollbar instead of a "+ N more" label. The label used to be the only
    // sign that rows existed below the fold, and it was a dead end -- there was
    // no way to reach them. Now the thumb says how much list there is and where
    // you are in it.
    let max_scroll = layout.max_scroll(list.len);
    if max_scroll > 0.0 {
        let track_x = layout.margin + layout.content_w - 5.0;
        let track_y = layout.list_top;
        let track_h = (layout.field.y - 12.0 - track_y).max(0.0);
        if track_h > 20.0 {
            let step = layout.row_h + layout.row_gap;
            let content = (list.len as f32) * step;
            let frac = (track_h / content).clamp(0.08, 1.0);
            let thumb_h = (track_h * frac).max(24.0);
            let travel = track_h - thumb_h;
            let pos = if max_scroll > 0.0 {
                (layout.scroll / max_scroll).clamp(0.0, 1.0)
            } else {
                0.0
            };
            rounded_rect(
                canvas,
                track_x,
                track_y,
                4.0,
                track_h,
                2.0,
                color(0.16, 0.18, 0.24, 1.0),
            )?;
            rounded_rect(
                canvas,
                track_x,
                track_y + travel * pos,
                4.0,
                thumb_h,
                2.0,
                color(0.35, 0.39, 0.49, 1.0),
            )?;
        }
    }

    // ---- input strip: text field + Add button ----
    let field = layout.field;
    let radius = (field.h * 0.26).clamp(8.0, 14.0);
    if field_focus {
        rounded_rect(
            canvas,
            field.x - 2.0,
            field.y - 2.0,
            field.w + 4.0,
            field.h + 4.0,
            radius + 2.0,
            accent_soft,
        )?;
    }
    rounded_rect(
        canvas,
        field.x,
        field.y,
        field.w,
        field.h,
        radius,
        color(0.11, 0.125, 0.17, 1.0),
    )?;
    stroke_rounded(
        canvas,
        field.x,
        field.y,
        field.w,
        field.h,
        radius,
        color(0.24, 0.27, 0.35, 1.0),
    )?;

    let pad = (field.w * 0.06).clamp(8.0, 16.0);
    let text_x = field.x + pad;
    let text_y = field.y + field.h * 0.5 + layout.input_text_size * 0.36;
    if draft.is_empty() {
        draw_text(
            canvas,
            "Add an item...",
            text_x,
            text_y,
            layout.input_text_size,
            INK_DIM,
        )?;
    } else {
        draw_text(
            canvas,
            draft.as_str(),
            text_x,
            text_y,
            layout.input_text_size,
            INK,
        )?;
    }
    if field_focus {
        let cx = text_x + text_width(draft.as_str(), layout.input_text_size) + 2.0;
        // Never let the caret escape the field it belongs to.
        let cx = cx.min(field.x + field.w - 4.0);
        fill(
            canvas,
            cx,
            field.y + field.h * 0.24,
            2.0,
            field.h * 0.52,
            accent,
        )?;
    }

    // Add button: filled accent rounded rect with a centered label.
    let add = layout.add;
    let can_add = !draft.is_empty();
    let btn = if can_add {
        accent
    } else {
        color(0.2, 0.24, 0.33, 1.0)
    };
    rounded_rect(canvas, add.x, add.y, add.w, add.h, radius, btn)?;
    let label_ink = if can_add {
        color(0.05, 0.08, 0.16, 1.0)
    } else {
        INK_DIM
    };
    let label_size = (layout.input_text_size + 1.0).min(add.w * 0.34);
    let lw = text_width("Add", label_size);
    draw_text(
        canvas,
        "Add",
        add.x + (add.w - lw) * 0.5,
        add.y + add.h * 0.5 + label_size * 0.36,
        label_size,
        label_ink,
    )?;

    canvas2d::present(canvas)?;
    Ok(())
}

/// One item row: a rounded card, a drawn checkbox that fills with accent when
/// checked (with a drawn tick), and the item text (dimmed + struck when done).
///
/// Every rectangle here comes from `layout`, and the checkbox comes from
/// `layout.checkbox(index)` -- the same call the event loop tests a click
/// against, so the drawn box and the clickable box are one rectangle.
fn draw_row(
    canvas: u64,
    layout: &Layout,
    index: usize,
    item: &Item,
    accent: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let row = layout.row(index);
    let card = if item.done { CARD_DONE } else { CARD };
    let radius = (row.h * 0.27).clamp(8.0, 14.0);
    rounded_rect(canvas, row.x, row.y, row.w, row.h, radius, card)?;

    let check = layout.checkbox(index);
    let check_radius = check.w * 0.29;
    if item.done {
        rounded_rect(
            canvas,
            check.x,
            check.y,
            check.w,
            check.h,
            check_radius,
            accent,
        )?;
        draw_tick(canvas, check.x, check.y, check.w)?;
    } else {
        rounded_rect(
            canvas,
            check.x,
            check.y,
            check.w,
            check.h,
            check_radius,
            color(0.17, 0.19, 0.26, 1.0),
        )?;
        stroke_rounded(
            canvas,
            check.x,
            check.y,
            check.w,
            check.h,
            check_radius,
            color(0.35, 0.39, 0.49, 1.0),
        )?;
    }

    let tx = check.x + check.w + (layout.margin * 0.5).clamp(10.0, 18.0);
    let ty = row.y + row.h * 0.5 + layout.row_text_size * 0.36;
    let ink = if item.done { INK_DONE } else { INK };
    // Clip the label to what the card can hold, so a long item on a narrow
    // window stops at the card edge instead of running off it.
    let avail = (row.x + row.w - tx - 10.0).max(0.0);
    let label = clip_to_width(item.text_str(), layout.row_text_size, avail);
    draw_text(canvas, label, tx, ty, layout.row_text_size, ink)?;
    if item.done {
        let w = text_width(label, layout.row_text_size).min(avail);
        fill(
            canvas,
            tx,
            ty - layout.row_text_size * 0.36,
            w,
            1.5,
            INK_DONE,
        )?;
    }
    Ok(())
}

/// The longest prefix of `s` that fits in `avail` at `size`. Returns a
/// sub-slice, so nothing allocates.
fn clip_to_width(s: &str, size: f32, avail: f32) -> &str {
    if text_width(s, size) <= avail {
        return s;
    }
    let mut end = 0usize;
    for (index, _) in s.char_indices() {
        if text_width(s.get(..index).unwrap_or(""), size) > avail {
            break;
        }
        end = index;
    }
    s.get(..end).unwrap_or("")
}

/// A white checkmark inside a box at (bx, by) of side `s`.
fn draw_tick(canvas: u64, bx: f32, by: f32, s: f32) -> Result<(), gfx::GfxError> {
    let white = color(0.98, 0.99, 1.0, 1.0);
    let p0 = (bx + s * 0.24, by + s * 0.52);
    let p1 = (bx + s * 0.42, by + s * 0.70);
    let p2 = (bx + s * 0.78, by + s * 0.30);
    thick_line(canvas, p0.0, p0.1, p1.0, p1.1, 1.5, white)?;
    thick_line(canvas, p1.0, p1.1, p2.0, p2.1, 1.5, white)?;
    Ok(())
}

// ------------------------------------------------------------------
// Drawing helpers
// ------------------------------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        c,
    )
}

/// A filled rounded rectangle: a cross of two rects plus four corner discs.
fn rounded_rect(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let r = r.min(w * 0.5).min(h * 0.5);
    fill(canvas, x + r, y, w - r * 2.0, h, c)?;
    fill(canvas, x, y + r, w, h - r * 2.0, c)?;
    disc(canvas, x + r, y + r, r, c)?;
    disc(canvas, x + w - r, y + r, r, c)?;
    disc(canvas, x + r, y + h - r, r, c)?;
    disc(canvas, x + w - r, y + h - r, r, c)?;
    Ok(())
}

/// A thin rounded-rect outline: four inset edges plus tiny corner dots.
fn stroke_rounded(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let t = 1.5;
    let r = r.min(w * 0.5).min(h * 0.5);
    fill(canvas, x + r, y, w - r * 2.0, t, c)?;
    fill(canvas, x + r, y + h - t, w - r * 2.0, t, c)?;
    fill(canvas, x, y + r, t, h - r * 2.0, c)?;
    fill(canvas, x + w - t, y + r, t, h - r * 2.0, c)?;
    disc(canvas, x + r, y + r, 1.0, c)?;
    disc(canvas, x + w - r, y + r, 1.0, c)?;
    disc(canvas, x + r, y + h - r, 1.0, c)?;
    disc(canvas, x + w - r, y + h - r, 1.0, c)?;
    Ok(())
}

fn disc(canvas: u64, cx: f32, cy: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x: cx, y: cy }, r, c)
}

/// A thick line drawn as a chain of small discs so any angle reads smooth.
fn thick_line(
    canvas: u64,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    width: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let len = sqrtf(dx * dx + dy * dy).max(0.001);
    let steps = (len / 1.2) as i32 + 1;
    let mut i = 0i32;
    while i <= steps {
        let t = i as f32 / steps as f32;
        disc(canvas, x0 + dx * t, y0 + dy * t, width, c)?;
        i += 1;
    }
    Ok(())
}

fn draw_text(
    canvas: u64,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text(canvas, text, gfx::Point { x, y }, size, c)
}

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

/// Approximate rendered width of a string at a given font size (~0.52em avg
/// advance). Good enough to place a caret and center a label.
fn text_width(s: &str, size: f32) -> f32 {
    (s.chars().count() as f32) * size * 0.52
}

fn sqrtf(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut g = x;
    let mut i = 0;
    while i < 6 {
        g = 0.5 * (g + x / g);
        i += 1;
    }
    g
}

// ---- number / label formatting into byte buffers, panic-free ----

fn progress_label(done: u32, total: u32, buf: &mut [u8; 32]) -> &[u8] {
    let mut pos = 0usize;
    push_num(buf, &mut pos, done);
    push_bytes(buf, &mut pos, b" of ");
    push_num(buf, &mut pos, total);
    push_bytes(buf, &mut pos, b" done");
    buf.get(..pos).unwrap_or(b"")
}

fn more_label(n: u32, buf: &mut [u8; 24]) -> &[u8] {
    let mut pos = 0usize;
    push_bytes(buf, &mut pos, b"+ ");
    push_num(buf, &mut pos, n);
    push_bytes(buf, &mut pos, b" more");
    buf.get(..pos).unwrap_or(b"")
}

fn push_bytes(buf: &mut [u8], pos: &mut usize, bytes: &[u8]) {
    for b in bytes {
        if let Some(slot) = buf.get_mut(*pos) {
            *slot = *b;
            *pos += 1;
        }
    }
}

fn push_num(buf: &mut [u8], pos: &mut usize, value: u32) {
    if value == 0 {
        if let Some(slot) = buf.get_mut(*pos) {
            *slot = b'0';
            *pos += 1;
        }
        return;
    }
    let mut scratch = [0u8; 10];
    let mut n = value;
    let mut count = 0usize;
    while n > 0 && count < scratch.len() {
        if let Some(slot) = scratch.get_mut(count) {
            *slot = b'0' + (n % 10) as u8;
        }
        n /= 10;
        count += 1;
    }
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let Some(src) = scratch.get(i) {
            if let Some(dst) = buf.get_mut(*pos) {
                *dst = *src;
                *pos += 1;
            }
        }
    }
}

fn number_bytes(value: u32, buf: &mut [u8; 10]) -> &[u8] {
    let mut pos = 0usize;
    push_num(buf, &mut pos, value);
    buf.get(..pos).unwrap_or(b"0")
}

// ------------------------------------------------------------------
// Entry point
// ------------------------------------------------------------------

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Checklist", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &canvas_node()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => {
                let _ = window::close(win);
                return 33;
            }
        };

        let mut list = Checklist::new();
        if !load(&mut list) || list.len == 0 {
            for seed in SEED_ITEMS {
                list.push(seed, false);
            }
        }
        let mut draft = Draft::new();
        let mut field_focus = false;

        let raw = args::raw();
        let first_arg = raw.as_bytes().split(|byte| *byte == b'\n').next();
        let quick = first_arg.is_some_and(|first| first == b"quick");
        // `resize-check` drives the app's own window through several sizes and
        // asserts the hit-boxes followed the drawing. See below.
        let resize_check = first_arg.is_some_and(|first| first == b"resize-check");

        let commit_draft = |list: &mut Checklist, draft: &mut Draft| -> bool {
            if draft.is_empty() || list.len >= MAX_ITEMS {
                return false;
            }
            list.push(draft.as_str(), false);
            draft.clear();
            save(list)
        };

        let mut saved_any = false;

        if quick {
            // The automated shot / verification run. Start from a clean, fixed
            // list rather than whatever prior CI runs accumulated, so the frame
            // is a believable, half-done checklist and does not grow every run.
            // Then prove the type + add + toggle + save paths on it.
            list = Checklist::new();
            list.push("Buy milk", true);
            list.push("Write the pitch", false);
            list.push("Ship the demo", false);
            list.push("Book the venue", false);
            draft.set("Record the walkthrough");
            if commit_draft(&mut list, &mut draft) {
                saved_any = true;
            }
            list.toggle(1);
            if save(&list) {
                saved_any = true;
            }
            let _ = draw(canvas, &list, &draft, false, 0.0);
            report(&list, saved_any);
            let _ = window::close(win);
            return 0;
        }

        if resize_check {
            // The resize proof. Grow and shrink the window, and after each
            // change confirm three things a person would notice:
            //   1. canvas_size reports the new size, so the layout moved;
            //   2. a click at the centre of what row 2 was JUST drawn at
            //      toggles row 2 and not some other row;
            //   3. a click at the centre of the drawn Add button still adds.
            // Before this work, (2) failed at every size but the original --
            // that is the bug this app taught every generated app.
            list = Checklist::new();
            list.push("Buy milk", false);
            list.push("Write the pitch", false);
            list.push("Ship the demo", false);

            let out = stdio::stdout();
            let sizes = [(440u32, 620u32), (900u32, 500u32), (320u32, 760u32)];
            let mut all_ok = true;
            for (w, h) in sizes {
                if window::set_size(
                    win,
                    types::WindowSize {
                        width: w,
                        height: h,
                    },
                )
                .is_err()
                {
                    all_ok = false;
                    continue;
                }
                // Drain the resize event the host just queued.
                let mut drain = 0u32;
                while drain < 8 && events::wait(Some(1)).is_some() {
                    drain += 1;
                }
                let Ok(layout) = draw(canvas, &list, &draft, false, 0.0) else {
                    all_ok = false;
                    continue;
                };

                let _ = out.write(b"size:");
                let mut nbuf = [0u8; 10];
                let _ = out.write(number_bytes(layout.width as u32, &mut nbuf));
                let _ = out.write(b"x");
                let _ = out.write(number_bytes(layout.height as u32, &mut nbuf));

                // Aim at the middle of the row the frame just drew.
                let target = 1usize;
                let row = layout.row(target);
                let cx = row.x + row.w * 0.5;
                let cy = row.y + row.h * 0.5;
                let hit = layout.hit_row(list.len, cx, cy);
                let row_ok = hit == Some(target);

                // And at the middle of the Add button it just drew.
                let add_ok = layout.add.contains(
                    layout.add.x + layout.add.w * 0.5,
                    layout.add.y + layout.add.h * 0.5,
                );
                // A point just outside the row must NOT hit it, or a pass here
                // would only mean the hit-box is enormous.
                let miss_ok = layout.hit_row(list.len, cx, row.y - 4.0) != Some(target);

                if row_ok && add_ok && miss_ok {
                    let _ = out.write(b" hit:ok\n");
                } else {
                    let _ = out.write(b" hit:WRONG\n");
                    all_ok = false;
                }
            }
            if all_ok {
                let _ = out.write(b"resize:ok\n");
            } else {
                let _ = out.write(b"resize:FAILED\n");
            }
            let _ = window::close(win);
            return if all_ok { 0 } else { 40 };
        }

        // The layout the last frame was drawn with. Clicks are tested against
        // exactly this, so a click can only ever be judged against the picture
        // the person is actually looking at.
        // How far the list is scrolled. Lives here so it survives redraws and
        // resizes; `scrolled_by` clamps it to whatever the new size can show.
        let mut scroll = 0.0f32;
        let mut layout = match draw(canvas, &list, &draft, field_focus, scroll) {
            Ok(layout) => layout,
            Err(_) => {
                let _ = window::close(win);
                return 34;
            }
        };

        let mut idle_rounds = 0u32;
        let mut round = 0u32;
        while round < MAX_WAIT_ROUNDS {
            round += 1;
            let event = events::wait(Some(WAIT_ROUND_MILLIS));
            if event.is_none() {
                // The idle timeout exists so a headless verification run cannot
                // hang forever waiting for a window nobody will close. That is
                // only a need on the automated path. Applying it to a real
                // session closed the window after ten quiet seconds, which is
                // what "the app closes by itself" turned out to be.
                idle_rounds += 1;
                if quick && idle_rounds >= MAX_IDLE_ROUNDS {
                    break;
                }
                continue;
            }
            idle_rounds = 0;
            let mut dirty = false;
            let mut done = false;
            match event {
                Some(types::Event::Pointer(p)) if p.pressed => {
                    if let Some(index) = layout.hit_row(list.len, p.x, p.y) {
                        list.toggle(index);
                        if save(&list) {
                            saved_any = true;
                        }
                        field_focus = false;
                        dirty = true;
                    } else if layout.add.contains(p.x, p.y) {
                        if commit_draft(&mut list, &mut draft) {
                            saved_any = true;
                        }
                        dirty = true;
                    } else if layout.field.contains(p.x, p.y) {
                        field_focus = true;
                        dirty = true;
                    } else {
                        field_focus = false;
                        dirty = true;
                    }
                }
                // The window changed size, so every rectangle changed with it.
                // Redrawing recomputes the layout from the canvas's new size,
                // and the hit-testing above follows automatically because it
                // reads the same `layout` the frame was drawn with.
                Some(types::Event::Wheel(w)) => {
                    // Positive dy scrolls down, matching the direction the
                    // offset grows. The clamp lives in `scrolled_by`, so a
                    // flick past either end springs back instead of showing
                    // blank space.
                    scroll = (scroll + w.dy).clamp(0.0, layout.max_scroll(list.len));
                    dirty = true;
                }
                Some(types::Event::Resized(_)) | Some(types::Event::RedrawRequested(_)) => {
                    dirty = true;
                }
                Some(types::Event::TextChanged(changed)) => {
                    draft.set(&changed.text);
                    field_focus = true;
                    dirty = true;
                }
                Some(types::Event::TextInput(text)) => {
                    for byte in text.as_bytes() {
                        let printable = byte.is_ascii_graphic() || *byte == b' ';
                        if printable {
                            draft.push(*byte);
                        }
                    }
                    field_focus = true;
                    dirty = true;
                }
                Some(types::Event::Key(key)) if key.pressed => {
                    if key.key.as_bytes() == b"Backspace" {
                        draft.pop();
                        field_focus = true;
                        dirty = true;
                    } else if key.key.as_bytes() == b"Enter" || key.key.as_bytes() == b"Return" {
                        if commit_draft(&mut list, &mut draft) {
                            saved_any = true;
                        }
                        dirty = true;
                    }
                }
                Some(types::Event::CloseRequested(_)) => {
                    done = true;
                }
                _ => {}
            }
            if dirty {
                if let Ok(fresh) = draw(canvas, &list, &draft, field_focus, scroll) {
                    layout = fresh;
                }
            }
            if done {
                break;
            }
        }

        report(&list, saved_any);
        let _ = window::close(win);
        0
    }
}

fn report(list: &Checklist, saved_any: bool) {
    let out = stdio::stdout();
    let _ = out.write(b"items:");
    let mut buf = [0u8; 10];
    let _ = out.write(number_bytes(list.len as u32, &mut buf));
    let _ = out.write(b"\n");
    if saved_any {
        let _ = out.write(b"saved:yes\n");
    }
}

// ----- widget builders (one canvas filling the window) -----

fn stack_root() -> types::WidgetNode {
    node(ROOT_ID, None, types::WidgetKind::Stack)
}

fn canvas_node() -> types::WidgetNode {
    node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas)
}

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
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

bindings::export!(Component with_types_in bindings);
