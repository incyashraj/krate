//! Krate Budget Splitter -- a modern allocation view drawn on a canvas.
//!
//! You type a monthly income; the app splits it into Rent, Taxes, Living, and
//! Investments by fixed percentages and shows the result as a single stacked,
//! segmented bar with labeled dollar amounts, plus a legend of swatch, name,
//! percent, and amount for each slice. A big income field sits at the top with
//! a Calculate button beside it; the whole UI is painted by the app, and the
//! field, digit keys, and button are hit-tested against the rectangles it drew.
//! The last income you entered is remembered in the key-value store.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler, so no path drags in the `wasi:*` import set. All
//! state is fixed-size; money is formatted by hand with thousands separators;
//! no `Vec`, `format!`, `unwrap`, or panicking index.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::kv as store_kv;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 460.0;
const HEIGHT: f32 = 560.0;

const DATA_KEY: &str = "income";

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;
const MAX_IDLE_ROUNDS: u32 = 300;

const MARGIN: f32 = 28.0;
const CONTENT_W: f32 = WIDTH - MARGIN * 2.0;

// The four allocation categories, in bar order. Percentages sum to 100. Amounts
// are computed by integer math; the last slice takes the remainder so the parts
// always sum back to the whole.
const CAT_COUNT: usize = 4;
const CAT_NAMES: [&str; CAT_COUNT] = ["Rent", "Taxes", "Living", "Investments"];
const CAT_PCT: [u32; CAT_COUNT] = [35, 20, 25, 20];
// Segment colors: a considered, distinct set that reads on the dark ground.
const CAT_COL: [gfx::Color; CAT_COUNT] = [
    gfx::Color { r: 0.42, g: 0.62, b: 1.0, a: 1.0 },  // Rent -- blue
    gfx::Color { r: 1.0, g: 0.45, b: 0.42, a: 1.0 },  // Taxes -- coral
    gfx::Color { r: 0.36, g: 0.82, b: 0.62, a: 1.0 }, // Living -- green
    gfx::Color { r: 0.85, g: 0.66, b: 0.35, a: 1.0 }, // Investments -- amber
];

// Palette.
const BG_TOP: gfx::Color = gfx::Color { r: 0.075, g: 0.086, b: 0.125, a: 1.0 };
const BG_BOT: gfx::Color = gfx::Color { r: 0.043, g: 0.051, b: 0.078, a: 1.0 };
const CARD: gfx::Color = gfx::Color { r: 0.121, g: 0.137, b: 0.184, a: 1.0 };
const INK: gfx::Color = gfx::Color { r: 0.902, g: 0.925, b: 0.98, a: 1.0 };
const INK_DIM: gfx::Color = gfx::Color { r: 0.478, g: 0.525, b: 0.627, a: 1.0 };

struct Component;

/// The income being typed / calculated, held as digits in a fixed buffer.
struct Money {
    digits: [u8; 12],
    len: usize,
}

impl Money {
    const fn new() -> Self {
        Self { digits: [0; 12], len: 0 }
    }

    fn value(&self) -> u64 {
        let mut v = 0u64;
        let mut i = 0usize;
        while i < self.len {
            if let Some(d) = self.digits.get(i) {
                v = v.saturating_mul(10).saturating_add(u64::from(*d - b'0'));
            }
            i += 1;
        }
        v
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn push_digit(&mut self, byte: u8) {
        if byte.is_ascii_digit() {
            if let Some(slot) = self.digits.get_mut(self.len) {
                *slot = byte;
                self.len += 1;
            }
        }
    }

    fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }

    fn set_value(&mut self, mut v: u64) {
        self.len = 0;
        if v == 0 {
            return;
        }
        let mut scratch = [0u8; 12];
        let mut count = 0usize;
        while v > 0 && count < scratch.len() {
            if let Some(s) = scratch.get_mut(count) {
                *s = b'0' + (v % 10) as u8;
            }
            v /= 10;
            count += 1;
        }
        let mut i = count;
        while i > 0 {
            i -= 1;
            if let Some(d) = scratch.get(i) {
                self.push_digit(*d);
            }
        }
    }
}

/// Amounts per category. The first three are floor(income*pct/100); the last
/// takes the remainder so the four amounts sum back to income exactly.
fn amounts(income: u64) -> [u64; CAT_COUNT] {
    let mut out = [0u64; CAT_COUNT];
    let mut assigned = 0u64;
    let mut i = 0usize;
    while i < CAT_COUNT - 1 {
        let pct = CAT_PCT.get(i).copied().unwrap_or(0) as u64;
        let a = income.saturating_mul(pct) / 100;
        if let Some(slot) = out.get_mut(i) {
            *slot = a;
        }
        assigned = assigned.saturating_add(a);
        i += 1;
    }
    if let Some(slot) = out.get_mut(CAT_COUNT - 1) {
        *slot = income.saturating_sub(assigned);
    }
    out
}

// ------------------------------------------------------------------
// Hit testing
// ------------------------------------------------------------------

const FIELD_Y: f32 = 96.0;
const FIELD_H: f32 = 58.0;
const CALC_W: f32 = 128.0;

fn field_rect() -> (f32, f32, f32, f32) {
    let fw = CONTENT_W - CALC_W - 12.0;
    (MARGIN, FIELD_Y, fw, FIELD_H)
}

fn calc_rect() -> (f32, f32, f32, f32) {
    (WIDTH - MARGIN - CALC_W, FIELD_Y, CALC_W, FIELD_H)
}

fn hit(x: f32, y: f32, r: (f32, f32, f32, f32)) -> bool {
    x >= r.0 && x <= r.0 + r.2 && y >= r.1 && y <= r.1 + r.3
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(canvas: u64, income: &Money, computed: u64, field_focus: bool) -> Result<(), gfx::GfxError> {
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    )?;

    let accent = color(0.42, 0.62, 1.0, 1.0);
    let accent_soft = color(0.42, 0.62, 1.0, 0.16);

    // ---- header ----
    draw_text(canvas, "Budget Splitter", MARGIN, 56.0, 30.0, INK)?;
    draw_text(canvas, "Where your monthly income goes", MARGIN, 80.0, 14.0, INK_DIM)?;

    // ---- income field ----
    let (fx, fy, fw, fh) = field_rect();
    if field_focus {
        rounded_rect(canvas, fx - 2.0, fy - 2.0, fw + 4.0, fh + 4.0, 16.0, accent_soft)?;
    }
    rounded_rect(canvas, fx, fy, fw, fh, 14.0, color(0.11, 0.125, 0.17, 1.0))?;
    stroke_rounded(canvas, fx, fy, fw, fh, 14.0, color(0.24, 0.27, 0.35, 1.0))?;

    let tx = fx + 20.0;
    let ty = fy + fh * 0.5 + 9.0;
    draw_text(canvas, "$", tx, ty, 26.0, INK_DIM)?;
    let num_x = tx + 20.0;
    if income.is_empty() {
        draw_text(canvas, "0", num_x, ty, 26.0, INK_DIM)?;
        if field_focus {
            fill(canvas, num_x + 16.0, fy + 14.0, 2.0, fh - 28.0, accent)?;
        }
    } else {
        let mut buf = [0u8; 20];
        let s = grouped(income.value(), &mut buf);
        if let Ok(txt) = core::str::from_utf8(s) {
            draw_text(canvas, txt, num_x, ty, 26.0, INK)?;
            if field_focus {
                let cx = num_x + text_width(canvas, txt, 26.0) + 3.0;
                fill(canvas, cx, fy + 14.0, 2.0, fh - 28.0, accent)?;
            }
        }
    }

    // ---- Calculate button ----
    let (cx0, cy0, cw, ch) = calc_rect();
    rounded_rect(canvas, cx0, cy0, cw, ch, 14.0, accent)?;
    let lw = text_width(canvas, "Calculate", 16.0);
    draw_text(canvas, "Calculate", cx0 + (cw - lw) * 0.5, cy0 + ch * 0.5 + 6.0, 16.0, color(0.05, 0.08, 0.16, 1.0))?;

    // ---- the allocation card ----
    let card_y = 188.0;
    let card_h = HEIGHT - card_y - MARGIN;
    rounded_rect(canvas, MARGIN, card_y, CONTENT_W, card_h, 18.0, CARD)?;

    let inner = MARGIN + 24.0;
    let inner_w = CONTENT_W - 48.0;

    draw_text(canvas, "Total to allocate", inner, card_y + 34.0, 14.0, INK_DIM)?;
    let mut tbuf = [0u8; 24];
    let total_s = dollars(computed, &mut tbuf);
    if let Ok(txt) = core::str::from_utf8(total_s) {
        draw_text(canvas, txt, inner, card_y + 66.0, 28.0, INK)?;
    }

    // ---- the stacked, segmented bar ----
    let bar_y = card_y + 92.0;
    let bar_h = 30.0;
    let amts = amounts(computed);
    if computed == 0 {
        rounded_rect(canvas, inner, bar_y, inner_w, bar_h, 10.0, color(0.16, 0.18, 0.24, 1.0))?;
        draw_text(canvas, "Enter an income to see the split.", inner, bar_y + bar_h + 34.0, 14.0, INK_DIM)?;
    } else {
        let gap = 3.0;
        let usable = inner_w - gap * (CAT_COUNT as f32 - 1.0);
        let mut x = inner;
        let mut i = 0usize;
        while i < CAT_COUNT {
            let pct = CAT_PCT.get(i).copied().unwrap_or(0) as f32;
            let is_last = i == CAT_COUNT - 1;
            let w = if is_last { (inner + inner_w) - x } else { usable * pct / 100.0 };
            let c = CAT_COL.get(i).copied().unwrap_or(INK);
            if i == 0 {
                left_rounded(canvas, x, bar_y, w, bar_h, 10.0, c)?;
            } else if is_last {
                right_rounded(canvas, x, bar_y, w, bar_h, 10.0, c)?;
            } else {
                fill(canvas, x, bar_y, w, bar_h, c)?;
            }
            x += w + gap;
            i += 1;
        }

        // ---- legend rows: swatch, name, percent, amount ----
        let mut ly = bar_y + bar_h + 40.0;
        let row_h = 44.0;
        let mut j = 0usize;
        while j < CAT_COUNT {
            let c = CAT_COL.get(j).copied().unwrap_or(INK);
            let name = CAT_NAMES.get(j).copied().unwrap_or("");
            let pct = CAT_PCT.get(j).copied().unwrap_or(0);
            let amt = amts.get(j).copied().unwrap_or(0);

            rounded_rect(canvas, inner, ly - 12.0, 16.0, 16.0, 5.0, c)?;
            draw_text(canvas, name, inner + 30.0, ly + 4.0, 17.0, INK)?;
            // Percent sits in its own fixed column so it never collides with the
            // longest name (Investments).
            let mut pbuf = [0u8; 8];
            let ps = pct_label(pct, &mut pbuf);
            if let Ok(txt) = core::str::from_utf8(ps) {
                draw_text(canvas, txt, inner + 200.0, ly + 4.0, 14.0, INK_DIM)?;
            }
            let mut abuf = [0u8; 24];
            let asr = dollars(amt, &mut abuf);
            if let Ok(txt) = core::str::from_utf8(asr) {
                let aw = text_width(canvas, txt, 17.0);
                draw_text(canvas, txt, inner + inner_w - aw, ly + 4.0, 17.0, c)?;
            }
            ly += row_h;
            j += 1;
        }
    }

    canvas2d::present(canvas)?;
    Ok(())
}

// ------------------------------------------------------------------
// Drawing helpers
// ------------------------------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

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

/// A rectangle rounded on its left two corners only.
fn left_rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    let r = r.min(w).min(h * 0.5);
    fill(canvas, x + r, y, w - r, h, c)?;
    fill(canvas, x, y + r, r, h - r * 2.0, c)?;
    disc(canvas, x + r, y + r, r, c)?;
    disc(canvas, x + r, y + h - r, r, c)?;
    Ok(())
}

/// A rectangle rounded on its right two corners only.
fn right_rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    let r = r.min(w).min(h * 0.5);
    fill(canvas, x, y, w - r, h, c)?;
    fill(canvas, x + w - r, y + r, r, h - r * 2.0, c)?;
    disc(canvas, x + w - r, y + r, r, c)?;
    disc(canvas, x + w - r, y + h - r, r, c)?;
    Ok(())
}

fn stroke_rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    let t = 1.5;
    let r = r.min(w * 0.5).min(h * 0.5);
    fill(canvas, x + r, y, w - r * 2.0, t, c)?;
    fill(canvas, x + r, y + h - t, w - r * 2.0, t, c)?;
    fill(canvas, x, y + r, t, h - r * 2.0, c)?;
    fill(canvas, x + w - t, y + r, t, h - r * 2.0, c)?;
    Ok(())
}

fn disc(canvas: u64, cx: f32, cy: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x: cx, y: cy }, r, c)
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
/// This used to be character count times an invented constant. On a
/// proportional face `i` and `W` differ about four times in real width, so a
/// centred label was not centred and a caret sat beside its text rather than
/// after it. `measure_text` is the true answer.
fn text_width(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

// ------------------------------------------------------------------
// Number formatting, panic-free
// ------------------------------------------------------------------

/// Grouped digits (thousands separators), no currency sign.
fn grouped(value: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut pos = 0usize;
    push_grouped(buf, &mut pos, value);
    buf.get(..pos).unwrap_or(b"0")
}

/// `$` + grouped digits.
fn dollars(value: u64, buf: &mut [u8; 24]) -> &[u8] {
    let mut pos = 0usize;
    if let Some(slot) = buf.get_mut(pos) {
        *slot = b'$';
        pos += 1;
    }
    push_grouped(buf, &mut pos, value);
    buf.get(..pos).unwrap_or(b"$0")
}

fn pct_label(pct: u32, buf: &mut [u8; 8]) -> &[u8] {
    let mut pos = 0usize;
    push_u64(buf, &mut pos, pct as u64);
    if let Some(slot) = buf.get_mut(pos) {
        *slot = b'%';
        pos += 1;
    }
    buf.get(..pos).unwrap_or(b"0%")
}

/// Append `value` with a comma before every trailing group of three digits.
fn push_grouped(buf: &mut [u8], pos: &mut usize, value: u64) {
    if value == 0 {
        if let Some(slot) = buf.get_mut(*pos) {
            *slot = b'0';
            *pos += 1;
        }
        return;
    }
    // Reversed digits.
    let mut rev = [0u8; 20];
    let mut n = value;
    let mut count = 0usize;
    while n > 0 && count < rev.len() {
        if let Some(s) = rev.get_mut(count) {
            *s = b'0' + (n % 10) as u8;
        }
        n /= 10;
        count += 1;
    }
    // Emit forward; after emitting the digit at index `i`, if `i` (the number of
    // digits still to come) is a positive multiple of 3, insert a comma.
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let Some(d) = rev.get(i) {
            if let Some(slot) = buf.get_mut(*pos) {
                *slot = *d;
                *pos += 1;
            }
        }
        if i > 0 && i % 3 == 0 {
            if let Some(slot) = buf.get_mut(*pos) {
                *slot = b',';
                *pos += 1;
            }
        }
    }
}

fn push_u64(buf: &mut [u8], pos: &mut usize, value: u64) {
    if value == 0 {
        if let Some(slot) = buf.get_mut(*pos) {
            *slot = b'0';
            *pos += 1;
        }
        return;
    }
    let mut scratch = [0u8; 20];
    let mut n = value;
    let mut count = 0usize;
    while n > 0 && count < scratch.len() {
        if let Some(s) = scratch.get_mut(count) {
            *s = b'0' + (n % 10) as u8;
        }
        n /= 10;
        count += 1;
    }
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let Some(d) = scratch.get(i) {
            if let Some(slot) = buf.get_mut(*pos) {
                *slot = *d;
                *pos += 1;
            }
        }
    }
}

fn u64_bytes(value: u64, buf: &mut [u8; 20]) -> &[u8] {
    let mut pos = 0usize;
    push_u64(buf, &mut pos, value);
    buf.get(..pos).unwrap_or(b"0")
}

// ------------------------------------------------------------------
// Persistence
// ------------------------------------------------------------------

fn load_income() -> u64 {
    match store_kv::get(DATA_KEY) {
        Ok(Some(bytes)) => {
            let mut v = 0u64;
            for b in bytes.iter() {
                if b.is_ascii_digit() {
                    v = v.saturating_mul(10).saturating_add(u64::from(*b - b'0'));
                }
            }
            v
        }
        _ => 0,
    }
}

fn save_income(value: u64) {
    let mut buf = [0u8; 20];
    let s = u64_bytes(value, &mut buf);
    let _ = store_kv::set(DATA_KEY, s);
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
        let Ok(win) = window::create("Budget Splitter", size) else {
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
                height: HEIGHT,
            },
        );

        let mut income = Money::new();
        let saved = load_income();
        if saved > 0 {
            income.set_value(saved);
        }
        let mut computed = saved;
        let mut field_focus = true;

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if quick {
            // The automated shot: a believable income, calculated, so the frame
            // shows the segmented bar and legend fully populated.
            income = Money::new();
            income.set_value(6500);
            computed = income.value();
            save_income(computed);
            let _ = draw(canvas, &income, computed, false);
            report(computed);
            let _ = window::close(win);
            return 0;
        }

        if draw(canvas, &income, computed, field_focus).is_err() {
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
                // window close itself -- which is exactly what happened here,
                // after eleven seconds, with nobody asking it to.
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
                    if hit(p.x, p.y, calc_rect()) {
                        computed = income.value();
                        save_income(computed);
                        field_focus = false;
                        dirty = true;
                    } else if hit(p.x, p.y, field_rect()) {
                        field_focus = true;
                        dirty = true;
                    } else {
                        field_focus = false;
                        dirty = true;
                    }
                }
                Some(types::Event::TextChanged(changed)) => {
                    income = Money::new();
                    for b in changed.text.as_bytes() {
                        income.push_digit(*b);
                    }
                    field_focus = true;
                    dirty = true;
                }
                Some(types::Event::TextInput(text)) => {
                    for b in text.as_bytes() {
                        income.push_digit(*b);
                    }
                    field_focus = true;
                    dirty = true;
                }
                Some(types::Event::Key(key)) if key.pressed => {
                    let k = key.key.as_bytes();
                    if k == b"Backspace" {
                        income.pop();
                        field_focus = true;
                        dirty = true;
                    } else if k == b"Enter" || k == b"Return" {
                        computed = income.value();
                        save_income(computed);
                        dirty = true;
                    } else if k.len() == 1 {
                        if let Some(b) = k.first() {
                            if b.is_ascii_digit() {
                                income.push_digit(*b);
                                field_focus = true;
                                dirty = true;
                            }
                        }
                    }
                }
                Some(types::Event::CloseRequested(_)) => {
                    done = true;
                }
                _ => {}
            }
            if dirty {
                let _ = draw(canvas, &income, computed, field_focus);
            }
            if done {
                break;
            }
        }

        report(computed);
        let _ = window::close(win);
        0
    }
}

fn report(computed: u64) {
    let out = stdio::stdout();
    let _ = out.write(b"income:");
    let mut buf = [0u8; 20];
    let _ = out.write(u64_bytes(computed, &mut buf));
    let _ = out.write(b"\n");
    let amts = amounts(computed);
    let mut i = 0usize;
    while i < CAT_COUNT {
        let a = amts.get(i).copied().unwrap_or(0);
        let _ = out.write(b"slice:");
        let mut b2 = [0u8; 20];
        let _ = out.write(u64_bytes(a, &mut b2));
        let _ = out.write(b"\n");
        i += 1;
    }
}

// ----- widget builders -----

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
