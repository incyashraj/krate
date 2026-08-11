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

// ---- layout constants (the rectangles the app draws and hit-tests) ----
const MARGIN: f32 = 28.0;
const CONTENT_W: f32 = WIDTH - MARGIN * 2.0;
const LIST_TOP: f32 = 148.0;
const ROW_H: f32 = 52.0;
const ROW_GAP: f32 = 10.0;
/// How many rows fit in the region before the input strip.
const VISIBLE_ROWS: usize = 6;
const CHECK_SIZE: f32 = 24.0;

const INPUT_H: f32 = 46.0;
const ADD_W: f32 = 92.0;

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
        push(if item.done { b"[x] " } else { b"[ ] " }, &mut out, &mut len);
        push(item.text_str().as_bytes(), &mut out, &mut len);
        push(b"\n", &mut out, &mut len);
    }
    store_kv::set(DATA_KEY, out.get(..len).unwrap_or(&[])).is_ok()
}

// ------------------------------------------------------------------
// Hit testing: which control, if any, contains (x, y)?
// ------------------------------------------------------------------

fn row_y(index: usize) -> f32 {
    LIST_TOP + (index as f32) * (ROW_H + ROW_GAP)
}

fn hit_row(list: &Checklist, x: f32, y: f32) -> Option<usize> {
    if x < MARGIN || x > WIDTH - MARGIN {
        return None;
    }
    let shown = if list.len < VISIBLE_ROWS { list.len } else { VISIBLE_ROWS };
    let mut i = 0usize;
    while i < shown {
        let ry = row_y(i);
        if y >= ry && y <= ry + ROW_H {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn input_y() -> f32 {
    HEIGHT - 76.0
}

fn hit_add(x: f32, y: f32) -> bool {
    let ay = input_y();
    let ax = WIDTH - MARGIN - ADD_W;
    x >= ax && x <= ax + ADD_W && y >= ay && y <= ay + INPUT_H
}

fn hit_field(x: f32, y: f32) -> bool {
    let ay = input_y();
    let fx = MARGIN;
    let fw = CONTENT_W - ADD_W - 12.0;
    x >= fx && x <= fx + fw && y >= ay && y <= ay + INPUT_H
}

// ------------------------------------------------------------------
// Palette
// ------------------------------------------------------------------

const BG_TOP: gfx::Color = gfx::Color { r: 0.075, g: 0.086, b: 0.125, a: 1.0 };
const BG_BOT: gfx::Color = gfx::Color { r: 0.043, g: 0.051, b: 0.078, a: 1.0 };
const CARD: gfx::Color = gfx::Color { r: 0.129, g: 0.145, b: 0.196, a: 1.0 };
const CARD_DONE: gfx::Color = gfx::Color { r: 0.102, g: 0.118, b: 0.161, a: 1.0 };
const INK: gfx::Color = gfx::Color { r: 0.902, g: 0.925, b: 0.98, a: 1.0 };
const INK_DIM: gfx::Color = gfx::Color { r: 0.478, g: 0.525, b: 0.627, a: 1.0 };
const INK_DONE: gfx::Color = gfx::Color { r: 0.435, g: 0.475, b: 0.561, a: 1.0 };

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(canvas: u64, list: &Checklist, draft: &Draft, field_focus: bool) -> Result<(), gfx::GfxError> {
    // Deep, considered ground -- a soft vertical gradient, not flat black.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    )?;

    let accent = color(0.42, 0.62, 1.0, 1.0);
    let accent_soft = color(0.42, 0.62, 1.0, 0.16);

    // ---- header: bold title + progress ----
    draw_text(canvas, "Checklist", MARGIN, 58.0, 34.0, INK)?;

    let total = list.len;
    let done = list.done_count();
    let mut buf = [0u8; 32];
    let sub = progress_label(done as u32, total as u32, &mut buf);
    if let Ok(txt) = core::str::from_utf8(sub) {
        draw_text(canvas, txt, MARGIN, 88.0, 15.0, INK_DIM)?;
    }

    // Progress bar track + accent fill.
    let bar_y = 108.0;
    let bar_w = CONTENT_W;
    rounded_rect(canvas, MARGIN, bar_y, bar_w, 8.0, 4.0, color(0.16, 0.18, 0.24, 1.0))?;
    if total > 0 {
        let frac = (done as f32 / total as f32).clamp(0.0, 1.0);
        let fw = (bar_w * frac).max(if done > 0 { 10.0 } else { 0.0 });
        if fw > 0.0 {
            rounded_rect(canvas, MARGIN, bar_y, fw, 8.0, 4.0, accent)?;
        }
    }

    // ---- item rows as cards ----
    let shown = if list.len < VISIBLE_ROWS { list.len } else { VISIBLE_ROWS };
    let mut i = 0usize;
    while i < shown {
        if let Some(item) = list.items.get(i) {
            if item.used {
                draw_row(canvas, i, item, accent)?;
            }
        }
        i += 1;
    }
    if list.len > VISIBLE_ROWS {
        let mut mbuf = [0u8; 24];
        let more = more_label((list.len - VISIBLE_ROWS) as u32, &mut mbuf);
        if let Ok(txt) = core::str::from_utf8(more) {
            draw_text(canvas, txt, MARGIN, row_y(VISIBLE_ROWS) + 4.0, 13.0, INK_DIM)?;
        }
    }

    // ---- input strip: text field + Add button ----
    let iy = input_y();
    let fw = CONTENT_W - ADD_W - 12.0;
    if field_focus {
        rounded_rect(canvas, MARGIN - 2.0, iy - 2.0, fw + 4.0, INPUT_H + 4.0, 14.0, accent_soft)?;
    }
    rounded_rect(canvas, MARGIN, iy, fw, INPUT_H, 12.0, color(0.11, 0.125, 0.17, 1.0))?;
    stroke_rounded(canvas, MARGIN, iy, fw, INPUT_H, 12.0, color(0.24, 0.27, 0.35, 1.0))?;

    let text_x = MARGIN + 16.0;
    let text_y = iy + INPUT_H * 0.5 + 6.0;
    if draft.is_empty() {
        draw_text(canvas, "Add an item...", text_x, text_y, 16.0, INK_DIM)?;
    } else {
        draw_text(canvas, draft.as_str(), text_x, text_y, 16.0, INK)?;
    }
    if field_focus {
        let cx = text_x + text_width(canvas, draft.as_str(), 16.0) + 2.0;
        fill(canvas, cx, iy + 12.0, 2.0, INPUT_H - 24.0, accent)?;
    }

    // Add button: filled accent rounded rect with a centered label.
    let ax = WIDTH - MARGIN - ADD_W;
    let can_add = !draft.is_empty();
    let btn = if can_add { accent } else { color(0.2, 0.24, 0.33, 1.0) };
    rounded_rect(canvas, ax, iy, ADD_W, INPUT_H, 12.0, btn)?;
    let label_ink = if can_add { color(0.05, 0.08, 0.16, 1.0) } else { INK_DIM };
    let lw = text_width(canvas, "Add", 17.0);
    draw_text(canvas, "Add", ax + (ADD_W - lw) * 0.5, text_y, 17.0, label_ink)?;

    canvas2d::present(canvas)?;
    Ok(())
}

/// One item row: a rounded card, a drawn checkbox that fills with accent when
/// checked (with a drawn tick), and the item text (dimmed + struck when done).
fn draw_row(canvas: u64, index: usize, item: &Item, accent: gfx::Color) -> Result<(), gfx::GfxError> {
    let y = row_y(index);
    let card = if item.done { CARD_DONE } else { CARD };
    rounded_rect(canvas, MARGIN, y, CONTENT_W, ROW_H, 14.0, card)?;

    let bx = MARGIN + 16.0;
    let by = y + (ROW_H - CHECK_SIZE) * 0.5;
    if item.done {
        rounded_rect(canvas, bx, by, CHECK_SIZE, CHECK_SIZE, 7.0, accent)?;
        draw_tick(canvas, bx, by, CHECK_SIZE)?;
    } else {
        rounded_rect(canvas, bx, by, CHECK_SIZE, CHECK_SIZE, 7.0, color(0.17, 0.19, 0.26, 1.0))?;
        stroke_rounded(canvas, bx, by, CHECK_SIZE, CHECK_SIZE, 7.0, color(0.35, 0.39, 0.49, 1.0))?;
    }

    let tx = bx + CHECK_SIZE + 16.0;
    let ty = y + ROW_H * 0.5 + 6.0;
    let ink = if item.done { INK_DONE } else { INK };
    draw_text(canvas, item.text_str(), tx, ty, 17.0, ink)?;
    if item.done {
        let w = text_width(canvas, item.text_str(), 17.0);
        fill(canvas, tx, ty - 6.0, w, 1.5, INK_DONE)?;
    }
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
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

/// A filled rounded rectangle: a cross of two rects plus four corner discs.
fn rounded_rect(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
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
fn stroke_rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
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
fn thick_line(canvas: u64, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
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

fn draw_text(canvas: u64, text: &str, x: f32, y: f32, size: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text(canvas, text, gfx::Point { x, y }, size, c)
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
        // The app's own coordinate system: keep drawing in these numbers
        // and the host scales them to any window, centred, never stretched
        // out of proportion (K-096).
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: WIDTH,
            },
        );

        let mut list = Checklist::new();
        if !load(&mut list) || list.len == 0 {
            for seed in SEED_ITEMS {
                list.push(seed, false);
            }
        }
        let mut draft = Draft::new();
        let mut field_focus = false;

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
            let _ = draw(canvas, &list, &draft, false);
            report(&list, saved_any);
            let _ = window::close(win);
            return 0;
        }

        if draw(canvas, &list, &draft, field_focus).is_err() {
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
                    if let Some(index) = hit_row(&list, p.x, p.y) {
                        list.toggle(index);
                        if save(&list) {
                            saved_any = true;
                        }
                        field_focus = false;
                        dirty = true;
                    } else if hit_add(p.x, p.y) {
                        if commit_draft(&mut list, &mut draft) {
                            saved_any = true;
                        }
                        dirty = true;
                    } else if hit_field(p.x, p.y) {
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
                let _ = draw(canvas, &list, &draft, field_focus);
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
