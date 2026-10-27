//! Krate Checklist -- a modern dark checklist drawn entirely on a canvas.
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
//! It lays itself out from the canvas it really has, every frame: one column
//! that fills a phone, keeps a short header when the phone is turned sideways,
//! and on a desktop sits centred with bigger rows. Text follows the person's
//! text size, and each frame tells the screen reader what it shows.
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

use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::store::kv as store_kv;
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The size the window opens at on a desktop. Only a starting point: every
/// frame is laid out from the canvas it is really given.
const WIDTH: f32 = 480.0;
const HEIGHT: f32 = 680.0;
const TITLE: &str = "Checklist";

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

/// Below this height the title and the progress line share one row, so a
/// phone turned sideways still shows several items rather than a header.
const SHORT_H: f32 = 560.0;
/// The widest the column grows, at the default text size. Past it, extra
/// room on a desktop goes to bigger rows and more of them.
const COLUMN_MAX: f32 = 680.0;

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
    let push = |bytes: &[u8], out: &mut [u8], len: &mut usize| {
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

// ------------------------------------------------------------------
// Layout: where everything goes, worked out from the canvas each frame
// ------------------------------------------------------------------

/// Every rectangle one frame draws and hit-tests. `draw` and the pointer
/// handler both ask `layout` for it, so a click lands on what was drawn at
/// whatever size the window is now -- a phone, a phone turned sideways, a
/// tablet, a desktop window being dragged wider.
struct Layout {
    w: f32,
    h: f32,
    /// Every font size and every row height is multiplied by this: the
    /// person's text size, and a little more on a big window so the list
    /// uses the room instead of floating small in it.
    s: f32,
    /// The column everything sits in: the whole width on a phone, centred
    /// and capped on a desktop, where a row a metre wide is hard to read.
    col: Rect,
    /// A short screen (a phone turned sideways): the title and the progress
    /// line share one row, so the height goes to the list.
    short: bool,
    title_size: f32,
    title_y: f32,
    sub_y: f32,
    bar_y: f32,
    list_top: f32,
    row_h: f32,
    row_gap: f32,
    /// How many whole rows the list has room for.
    rows: usize,
    field: Rect,
    add: Rect,
}

fn layout(canvas: u64, items: usize) -> Layout {
    let size = canvas2d::canvas_size(canvas).unwrap_or(gfx::Size {
        width: WIDTH,
        height: HEIGHT,
    });
    let w = size.width.max(1.0);
    let h = size.height.max(1.0);
    // The person's text size, kept to a range a row can still hold.
    let t = Look::now().text_scale.clamp(0.8, 2.4);
    // A tablet or a desktop window: room for bigger rows, not just wider ones.
    let roomy = w >= 700.0 && h >= 700.0;
    let s = if roomy { t * 1.15 } else { t };
    let short = h < SHORT_H;
    let margin = if w < 600.0 { 16.0 } else { 32.0 };
    let col_w = (w - 2.0 * margin).min(COLUMN_MAX * s.max(1.0)).max(1.0);
    let col = Rect::new((w - col_w) * 0.5, 0.0, col_w, h);
    let top = if short {
        12.0
    } else if roomy {
        56.0
    } else {
        24.0
    };

    // The title shrinks before it overflows: at the largest text size the
    // word is wider than a phone.
    let mut title_size = if short { 24.0 } else { 34.0 } * s;
    let title_room = if short { col_w * 0.55 } else { col_w };
    let tw = text_width(canvas, TITLE, title_size);
    if tw > title_room {
        title_size *= title_room / tw;
    }
    let title_y = top + title_size;
    let sub_y = if short { title_y } else { title_y + 28.0 * s };
    let bar_y = sub_y + 14.0 * s;
    let list_top = bar_y + 8.0 + if short { 12.0 } else { 22.0 * s };

    // Controls are never shorter than a fingertip, whatever the text size.
    let input_h = (46.0 * s).max(44.0);
    let bottom = if short {
        12.0
    } else if roomy {
        48.0
    } else {
        24.0
    };
    let lowest = h - bottom - input_h;
    // A short screen trades a little row height for one more row.
    let row_h = (if short { 46.0 } else { 52.0 } * s).max(44.0);
    let row_gap = if short { 8.0 } else { 10.0 * s.min(1.5) };
    // Whole rows only, and when they do not all fit, a line kept for
    // "+ N more" so the person knows to scroll.
    let region = lowest - 16.0 - list_top;
    let fits = |room: f32| ((room + row_gap) / (row_h + row_gap)).max(0.0) as usize;
    let mut rows = fits(region);
    if items > rows {
        rows = fits(region - 22.0 * s);
    }
    // The input strip follows the last item, like the next row of the list,
    // until the list is long enough to push it to the bottom edge. Pinned
    // there from the start, a short list on a tall screen left a band of
    // nothing between the items and the field.
    let after_last = list_top + items as f32 * (row_h + row_gap) + 8.0;
    let input_y = after_last.min(lowest);
    let add_w = (92.0 * s).min(col_w * 0.4);
    let add = Rect::new(col.right() - add_w, input_y, add_w, input_h);
    let field = Rect::new(col.x, input_y, col_w - add_w - 12.0, input_h);
    Layout {
        w,
        h,
        s,
        col,
        short,
        title_size,
        title_y,
        sub_y,
        bar_y,
        list_top,
        row_h,
        row_gap,
        rows,
        field,
        add,
    }
}

impl Layout {
    /// The card for the row on screen at `slot` (0 is the top one shown).
    fn row(&self, slot: usize) -> Rect {
        let y = self.list_top + slot as f32 * (self.row_h + self.row_gap);
        Rect::new(self.col.x, y, self.col.w, self.row_h)
    }

    /// The first item shown, held so the list never scrolls past its last
    /// row -- it also pulls the list back after the window grows.
    fn first_shown(&self, first: usize, items: usize) -> usize {
        first.min(items.saturating_sub(self.rows))
    }
}

/// Which item, if any, the row under (x, y) shows.
fn hit_row(lay: &Layout, list: &Checklist, first: usize, x: f32, y: f32) -> Option<usize> {
    let first = lay.first_shown(first, list.len);
    (0..lay.rows)
        .find(|slot| first + slot < list.len && lay.row(*slot).contains(x, y))
        .map(|slot| first + slot)
}

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

/// One frame, laid out from the canvas it is drawn on. As it draws, it tells
/// the screen reader what is where (`Said`), so a person using VoiceOver or
/// TalkBack hears the title, the progress, every item with whether it is
/// done, the field and the button -- and a double-tap on one arrives as an
/// ordinary tap at its centre.
fn draw(
    canvas: u64,
    win: u64,
    list: &Checklist,
    first: usize,
    draft: &Draft,
    field_focus: bool,
) -> Result<(), gfx::GfxError> {
    let lay = layout(canvas, list.len);
    let s = lay.s;
    let col = lay.col;
    let mut said = Said::new();

    // Deep, considered ground -- a soft vertical gradient, not flat black.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect {
            x: 0.0,
            y: 0.0,
            width: lay.w,
            height: lay.h,
        },
        BG_TOP,
        BG_BOT,
    )?;

    let accent = color(0.42, 0.62, 1.0, 1.0);
    let accent_soft = color(0.42, 0.62, 1.0, 0.16);

    // ---- header: bold title + progress ----
    let ts = lay.title_size;
    draw_text(canvas, TITLE, col.x, lay.title_y, ts, INK)?;
    let tw = text_width(canvas, TITLE, ts);
    said.heading(Rect::new(col.x, lay.title_y - ts, tw, ts * 1.3), TITLE);

    let total = list.len;
    let done = list.done_count();
    let mut buf = [0u8; 32];
    let sub = progress_label(done as u32, total as u32, &mut buf);
    if let Ok(txt) = core::str::from_utf8(sub) {
        let size = 15.0 * s;
        let sw = text_width(canvas, txt, size);
        // On a short screen it sits at the right end of the title's line.
        let sx = if lay.short { col.right() - sw } else { col.x };
        draw_text(canvas, txt, sx, lay.sub_y, size, INK_DIM)?;
        said.text(Rect::new(sx, lay.sub_y - size, sw, size * 1.3), txt);
    }

    // Progress bar track + accent fill.
    let bar_h = 8.0 * s.max(1.0);
    rounded_rect(
        canvas,
        col.x,
        lay.bar_y,
        col.w,
        bar_h,
        bar_h * 0.5,
        color(0.16, 0.18, 0.24, 1.0),
    )?;
    if total > 0 {
        let frac = (done as f32 / total as f32).clamp(0.0, 1.0);
        let fw = (col.w * frac).max(if done > 0 { 10.0 } else { 0.0 });
        if fw > 0.0 {
            rounded_rect(canvas, col.x, lay.bar_y, fw, bar_h, bar_h * 0.5, accent)?;
        }
    }

    // ---- item rows as cards, from the scrolled-to item down ----
    let first = lay.first_shown(first, list.len);
    for slot in 0..lay.rows {
        let index = first + slot;
        if index >= list.len {
            break;
        }
        if let Some(item) = list.items.get(index) {
            if item.used {
                draw_row(canvas, &lay, slot, item, accent, &mut said)?;
            }
        }
    }
    let below = list.len.saturating_sub(first + lay.rows);
    if below > 0 {
        let mut mbuf = [0u8; 24];
        let more = more_label(below as u32, &mut mbuf);
        if let Ok(txt) = core::str::from_utf8(more) {
            let size = 13.0 * s;
            let y = lay.row(lay.rows).y + size;
            draw_text(canvas, txt, col.x, y, size, INK_DIM)?;
            said.text(Rect::new(col.x, y - size, col.w, size * 1.3), txt);
        }
    }

    // ---- input strip: text field + Add button ----
    let field = lay.field;
    if field_focus {
        rounded_rect(
            canvas,
            field.x - 2.0,
            field.y - 2.0,
            field.w + 4.0,
            field.h + 4.0,
            14.0,
            accent_soft,
        )?;
    }
    rounded_rect(
        canvas,
        field.x,
        field.y,
        field.w,
        field.h,
        12.0,
        color(0.11, 0.125, 0.17, 1.0),
    )?;
    stroke_rounded(
        canvas,
        field.x,
        field.y,
        field.w,
        field.h,
        12.0,
        color(0.24, 0.27, 0.35, 1.0),
    )?;

    let size = 16.0 * s;
    let text_x = field.x + 16.0;
    let text_y = field.y + field.h * 0.5 + size * 0.36;
    let room = field.w - 32.0;
    let typed = draft.as_str();
    // A long draft shows its end, where the typing is happening.
    let n = typed.chars().count();
    let keep = most_that_fit(n, |k| {
        text_width(canvas, skip_chars(typed, n - k), size) <= room
    });
    let shown = skip_chars(typed, n - keep);
    if draft.is_empty() {
        draw_fit(
            canvas,
            "Add an item...",
            text_x,
            text_y,
            size,
            INK_DIM,
            room,
        )?;
    } else {
        draw_text(canvas, shown, text_x, text_y, size, INK)?;
    }
    if field_focus {
        let cx = text_x + text_width(canvas, shown, size) + 2.0;
        fill(canvas, cx, field.y + 12.0, 2.0, field.h - 24.0, accent)?;
    }
    said.field(field, "New item", typed);

    // Add button: filled accent rounded rect with a centered label.
    let add = lay.add;
    let can_add = !draft.is_empty();
    let btn = if can_add {
        accent
    } else {
        color(0.2, 0.24, 0.33, 1.0)
    };
    rounded_rect(canvas, add.x, add.y, add.w, add.h, 12.0, btn)?;
    let label_ink = if can_add {
        color(0.05, 0.08, 0.16, 1.0)
    } else {
        INK_DIM
    };
    let label = 17.0 * s;
    let lw = text_width(canvas, "Add", label);
    let ly = add.y + add.h * 0.5 + label * 0.36;
    draw_text(
        canvas,
        "Add",
        add.x + (add.w - lw) * 0.5,
        ly,
        label,
        label_ink,
    )?;
    said.button(add, "Add");

    canvas2d::present(canvas)?;
    said.send(win);
    Ok(())
}

/// One item row: a rounded card, a drawn checkbox that fills with accent when
/// checked (with a drawn tick), and the item text (dimmed + struck when done),
/// cut with an ellipsis when it is wider than the card.
fn draw_row(
    canvas: u64,
    lay: &Layout,
    slot: usize,
    item: &Item,
    accent: gfx::Color,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    let r = lay.row(slot);
    let card = if item.done { CARD_DONE } else { CARD };
    shadow(canvas, r.x, r.y, r.w, r.h, 14.0)?;
    rounded_rect(canvas, r.x, r.y, r.w, r.h, 14.0, card)?;

    let check = 24.0 * lay.s.max(1.0);
    let bx = r.x + 16.0;
    let by = r.y + (r.h - check) * 0.5;
    if item.done {
        rounded_rect(canvas, bx, by, check, check, 7.0, accent)?;
        draw_tick(canvas, bx, by, check)?;
    } else {
        rounded_rect(
            canvas,
            bx,
            by,
            check,
            check,
            7.0,
            color(0.17, 0.19, 0.26, 1.0),
        )?;
        stroke_rounded(
            canvas,
            bx,
            by,
            check,
            check,
            7.0,
            color(0.35, 0.39, 0.49, 1.0),
        )?;
    }

    let size = 17.0 * lay.s;
    let tx = bx + check + 16.0;
    let ty = r.y + r.h * 0.5 + size * 0.36;
    let ink = if item.done { INK_DONE } else { INK };
    let drawn = draw_fit(
        canvas,
        item.text_str(),
        tx,
        ty,
        size,
        ink,
        r.right() - 16.0 - tx,
    )?;
    if item.done {
        fill(canvas, tx, ty - size * 0.35, drawn, 1.5, INK_DONE)?;
    }
    said.toggle(r, item.text_str(), item.done);
    Ok(())
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

/// A filled rounded rectangle. One host call, antialiased on the curve --
/// the cross-of-rects-plus-corner-discs version this replaced showed seams
/// where the bands met the discs.
fn rounded_rect(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::fill_round_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        radii(r),
        c,
    )
}

/// A thin rounded-rect outline. The hand-built version drew the corners as
/// 1px dots, so every empty checkbox and text field had four visible gaps.
fn stroke_rounded(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::stroke_round_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        radii(r),
        1.5,
        c,
    )
}

/// A soft shadow under a card, drawn before the card itself.
fn shadow(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32) -> Result<(), gfx::GfxError> {
    canvas2d::drop_shadow_round_rect(
        canvas,
        gfx::Rect {
            x,
            y: y + 2.0,
            width: w,
            height: h,
        },
        radii(r),
        10.0,
        color(0.0, 0.0, 0.0, 0.28),
    )
}

fn radii(r: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: r,
        top_right: r,
        bottom_right: r,
        bottom_left: r,
    }
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

/// Draw `s` in at most `room` points: all of it, or the longest leading part
/// that fits and an ellipsis. A row running past its card reads as a broken
/// app; one cut short reads as "there is more". Returns the width drawn, so a
/// strike-through covers exactly the text.
fn draw_fit(
    canvas: u64,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    c: gfx::Color,
    room: f32,
) -> Result<f32, gfx::GfxError> {
    let full = text_width(canvas, s, size);
    if full <= room {
        draw_text(canvas, s, x, y, size, c)?;
        return Ok(full);
    }
    let ell = text_width(canvas, "\u{2026}", size);
    let n = s.chars().count();
    let keep = most_that_fit(n, |k| {
        text_width(canvas, take_chars(s, k), size) + ell <= room
    });
    let part = take_chars(s, keep).trim_end();
    let pw = text_width(canvas, part, size);
    draw_text(canvas, part, x, y, size, c)?;
    draw_text(canvas, "\u{2026}", x + pw, y, size, c)?;
    Ok(pw + ell)
}

/// The largest `k` in `0..=n` for which `fits(k)` holds, when it holds for
/// every smaller `k` too. A binary search, so a long item costs a handful of
/// measurements rather than one per character.
fn most_that_fit(n: usize, fits: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (0usize, n);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if fits(mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// The first `k` characters of `s`. Counted in characters, not bytes, so a
/// cut never lands inside one.
fn take_chars(s: &str, k: usize) -> &str {
    let end = s.char_indices().nth(k).map_or(s.len(), |(i, _)| i);
    s.get(..end).unwrap_or(s)
}

/// `s` without its first `k` characters.
fn skip_chars(s: &str, k: usize) -> &str {
    let start = s.char_indices().nth(k).map_or(s.len(), |(i, _)| i);
    s.get(start..).unwrap_or("")
}

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

/// Rendered width of a string at a given font size, measured by the host with
/// the same font layout `draw_text` draws with.
///
/// This used to be `chars * size * 0.52`, an invented constant on a
/// proportional face where `i` and `W` differ about four times in real width,
/// so a centred label was not centred and a caret sat beside its text rather
/// than after it. `measure_text` is the true answer; the fallback is only
/// reached if the canvas handle is bad, in which case nothing else draws
/// either.
fn text_width(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
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

impl krate::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create(TITLE, size) else {
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
        // No design size: a list should use the room it is given, so every
        // frame is laid out from `canvas_size` (see `layout`) rather than
        // drawn at one size and scaled into the window with bands around it.

        let mut list = Checklist::new();
        if !load(&mut list) || list.len == 0 {
            for seed in SEED_ITEMS {
                list.push(seed, false);
            }
        }
        let mut draft = Draft::new();
        let mut field_focus = false;
        // The first item shown, and wheel travel not yet turned into a row.
        // The list scrolls a whole row at a time, so it never stops on half
        // a card.
        let mut first = 0usize;
        let mut wheel = 0.0f32;

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

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
            let _ = draw(canvas, win, &list, first, &draft, false);
            report(&list, saved_any);
            let _ = window::close(win);
            return 0;
        }

        if draw(canvas, win, &list, first, &draft, field_focus).is_err() {
            let _ = window::close(win);
            return 34;
        }

        let mut idle_rounds = 0u32;
        let mut round = 0u32;
        while round < MAX_WAIT_ROUNDS {
            round += 1;
            let event = events::wait(Some(WAIT_ROUND_MILLIS));
            if event.is_none() {
                idle_rounds += 1;
                // Only a headless check gives up on silence. A person who
                // opens this and thinks for a moment must not watch the
                // window close itself.
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
                    // The same layout the last frame was drawn from, so the
                    // tap lands on what the person saw.
                    let lay = layout(canvas, list.len);
                    if let Some(index) = hit_row(&lay, &list, first, p.x, p.y) {
                        list.toggle(index);
                        if save(&list) {
                            saved_any = true;
                        }
                        field_focus = false;
                        dirty = true;
                    } else if lay.add.contains(p.x, p.y) {
                        if commit_draft(&mut list, &mut draft) {
                            saved_any = true;
                            first = list.len;
                        }
                        dirty = true;
                    } else if lay.field.contains(p.x, p.y) {
                        field_focus = true;
                        dirty = true;
                    } else {
                        field_focus = false;
                        dirty = true;
                    }
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
                            // Scroll to the end, so the new item is in view.
                            first = list.len;
                        }
                        dirty = true;
                    }
                }
                // Ctrl-scroll means zoom on every desktop; leave it alone.
                Some(types::Event::Wheel(w)) if !w.modifiers.control => {
                    let lay = layout(canvas, list.len);
                    let step = (lay.row_h + lay.row_gap) * 0.5;
                    wheel += w.dy;
                    while wheel >= step {
                        first += 1;
                        wheel -= step;
                    }
                    while wheel <= -step {
                        first = first.saturating_sub(1);
                        wheel += step;
                    }
                    first = lay.first_shown(first, list.len);
                    dirty = true;
                }
                // A new size or a new look: lay the frame out again.
                Some(types::Event::Resized(_)) | Some(types::Event::ThemeChanged(_)) => {
                    dirty = true;
                }
                Some(types::Event::CloseRequested(_)) => {
                    done = true;
                }
                _ => {}
            }
            if dirty {
                first = layout(canvas, list.len).first_shown(first, list.len);
                let _ = draw(canvas, win, &list, first, &draft, field_focus);
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

krate::export!(Component);
