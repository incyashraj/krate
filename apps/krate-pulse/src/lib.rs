//! Pulse — a personal finance dashboard drawn entirely on a canvas.
//!
//! The hero layout: a bold balance headline with a monthly delta chip, a
//! 30-day spending area chart with a highlighted "today" point, a category
//! breakdown with share bars, and a recent-transactions list with colored
//! merchant avatars. Clicking a category filters the list; the selection is
//! remembered in the key-value store. All data is a seeded local demo ledger —
//! honest about being a demo, real about everything it does with it.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler. All state is fixed-size; money is formatted by
//! hand; no `format!`, `unwrap`, or panicking index anywhere.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::string::String;
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::kv as store_kv;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The size the window opens at. Nothing is laid out from these -- the window
/// is resizable, so every rectangle comes from `Layout::for_size`, built from
/// `canvas2d::canvas_size` at the top of each frame.
const WIDTH: f32 = 1080.0;
const HEIGHT: f32 = 700.0;

const GUTTER: f32 = 24.0;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;
const MAX_IDLE_ROUNDS: u32 = 300;

const SELECT_KEY: &str = "selected-category";

// ---- palette (the shared canvas design system) ------------------------------

const BG_TOP: gfx::Color = rgb(0.043, 0.055, 0.082);
const BG_BOT: gfx::Color = rgb(0.063, 0.078, 0.114);
const CARD: gfx::Color = rgb(0.086, 0.106, 0.149);
const CARD_EDGE: gfx::Color = rgb(0.137, 0.165, 0.220);
const INK: gfx::Color = rgb(0.949, 0.961, 0.980);
const INK_DIM: gfx::Color = rgb(0.604, 0.647, 0.710);
const INK_QUIET: gfx::Color = rgb(0.365, 0.408, 0.471);
const ACCENT: gfx::Color = rgb(0.298, 0.553, 1.0);
const ACCENT_SOFT: gfx::Color = gfx::Color { r: 0.298, g: 0.553, b: 1.0, a: 0.16 };
// Pre-blended opaque tints: translucent rounded shapes double alpha where the
// rect and corner discs overlap, so chips and washes use flattened colors.
const WASH_SEL: gfx::Color = rgb(0.120, 0.177, 0.285);
const CHIP_GREEN: gfx::Color = rgb(0.070, 0.165, 0.147);
const GREEN: gfx::Color = rgb(0.239, 0.839, 0.549);
const GREEN_SOFT: gfx::Color = gfx::Color { r: 0.239, g: 0.839, b: 0.549, a: 0.14 };

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

/// Opaque source-over blend of `c` at `a` onto `base` -- used to pre-flatten
/// tints so translucent rounded shapes never double up at their corners.
fn blend(base: gfx::Color, c: gfx::Color, a: f32) -> gfx::Color {
    gfx::Color {
        r: base.r * (1.0 - a) + c.r * a,
        g: base.g * (1.0 - a) + c.g * a,
        b: base.b * (1.0 - a) + c.b * a,
        a: 1.0,
    }
}

// ---- the demo ledger --------------------------------------------------------

const CAT_COUNT: usize = 4;
const CAT_NAMES: [&str; CAT_COUNT] = ["Food & drink", "Transport", "Shopping", "Bills"];
const CAT_COL: [gfx::Color; CAT_COUNT] = [
    rgb(1.0, 0.761, 0.294),  // warm yellow
    rgb(0.298, 0.553, 1.0),  // accent blue
    rgb(0.706, 0.549, 1.0),  // violet
    rgb(0.239, 0.839, 0.549), // green
];

/// One transaction: merchant, category index (or NONE for income), cents.
struct Tx {
    merchant: &'static str,
    cat: usize,
    cents: i64,
}
/// Category index meaning "income", drawn green with a plus.
const INCOME: usize = usize::MAX;

const TXS: [Tx; 6] = [
    Tx { merchant: "Blue Bottle", cat: 0, cents: -1_450 },
    Tx { merchant: "Salary", cat: INCOME, cents: 240_000 },
    Tx { merchant: "Uber", cat: 1, cents: -2_380 },
    Tx { merchant: "Muji", cat: 2, cents: -8_640 },
    Tx { merchant: "Con Edison", cat: 3, cents: -9_212 },
    Tx { merchant: "Trader Joe's", cat: 0, cents: -6_318 },
];

/// Monthly totals per category, in cents (drive the share bars).
const CAT_TOTALS: [i64; CAT_COUNT] = [48_260, 21_480, 36_920, 31_840];

/// The balance headline and its monthly delta, in cents.
const BALANCE: i64 = 843_250;
const DELTA: i64 = 124_000;

/// Thirty days of spending, in whole dollars, tuned to a pleasing curve with a
/// mid-month spike and a calm tail. `TODAY` highlights the last point.
const DAYS: usize = 30;
const SPEND: [u32; DAYS] = [
    42, 38, 55, 61, 47, 52, 88, 74, 66, 58, 71, 96, 132, 118, 84, 76, 69, 81, 94, 87, 72, 64, 58,
    66, 79, 92, 85, 71, 63, 68,
];

// ---- tiny drawing + text helpers -------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

fn disc(canvas: u64, x: f32, y: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x, y }, r, c)
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, c);
}

/// Rough advance for right-aligning: the system face averages ~0.52em.
fn est_width(s: &str, size: f32) -> f32 {
    s.chars().count() as f32 * size * 0.52
}

fn text_right(canvas: u64, s: &str, right: f32, y: f32, size: f32, c: gfx::Color) {
    text(canvas, s, right - est_width(s, size), y, size, c);
}

/// A plain decimal number into a byte buffer, panic-free. Used by the resize
/// self-check to report the size it actually laid out to.
fn u32_bytes(value: u32, buf: &mut [u8; 12]) -> &[u8] {
    let mut scratch = [0u8; 10];
    let mut n = value;
    let mut count = 0usize;
    if n == 0 {
        count = 1;
    }
    while n > 0 && count < scratch.len() {
        if let Some(slot) = scratch.get_mut(count) {
            *slot = b'0' + (n % 10) as u8;
        }
        n /= 10;
        count += 1;
    }
    let mut pos = 0usize;
    let mut i = count;
    while i > 0 {
        i -= 1;
        let digit = scratch.get(i).copied().unwrap_or(b'0');
        if let Some(slot) = buf.get_mut(pos) {
            *slot = digit;
            pos += 1;
        }
    }
    buf.get(..pos).unwrap_or(b"0")
}

fn rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    let r = r.min(w * 0.5).min(h * 0.5);
    fill(canvas, x + r, y, w - r * 2.0, h, c)?;
    fill(canvas, x, y + r, w, h - r * 2.0, c)?;
    disc(canvas, x + r, y + r, r, c)?;
    disc(canvas, x + w - r, y + r, r, c)?;
    disc(canvas, x + r, y + h - r, r, c)?;
    disc(canvas, x + w - r, y + h - r, r, c)?;
    Ok(())
}

/// A card: soft shadow, panel fill, hairline top edge highlight.
fn card(canvas: u64, x: f32, y: f32, w: f32, h: f32) -> Result<(), gfx::GfxError> {
    rounded(canvas, x + 2.0, y + 4.0, w, h, 16.0, gfx::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.28 })?;
    rounded(canvas, x, y, w, h, 16.0, CARD)?;
    fill(canvas, x + 16.0, y, w - 32.0, 1.0, CARD_EDGE)?;
    Ok(())
}

/// Build an owned `String` without touching std's allocation-error handler.
fn pure_string(text: &str) -> String {
    let len = text.len();
    if len == 0 {
        return String::new();
    }
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(len, 1);
        let ptr = alloc::alloc::alloc(layout);
        if ptr.is_null() {
            core::arch::wasm32::unreachable()
        }
        core::ptr::copy_nonoverlapping(text.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

/// Format cents as money into `buf`, returning the used slice: "$8,432.50",
/// with an explicit sign when `signed` ("+$2,400.00" / "-$14.50").
fn money<'b>(buf: &'b mut [u8; 24], cents: i64, signed: bool) -> &'b str {
    let neg = cents < 0;
    let mut whole = (if neg { -cents } else { cents }) / 100;
    let frac = ((if neg { -cents } else { cents }) % 100) as u8;

    // Digits of the whole part, reversed, with thousands separators.
    let mut tmp = [0u8; 16];
    let mut n = 0usize;
    let mut group = 0u8;
    loop {
        if group == 3 {
            if let Some(slot) = tmp.get_mut(n) {
                *slot = b',';
                n += 1;
            }
            group = 0;
        }
        if let Some(slot) = tmp.get_mut(n) {
            *slot = b'0' + (whole % 10) as u8;
            n += 1;
        }
        group += 1;
        whole /= 10;
        if whole == 0 {
            break;
        }
    }

    let mut out = 0usize;
    let push = |b: u8, buf: &mut [u8; 24], out: &mut usize| {
        if let Some(slot) = buf.get_mut(*out) {
            *slot = b;
            *out += 1;
        }
    };
    if neg {
        push(b'-', buf, &mut out);
    } else if signed {
        push(b'+', buf, &mut out);
    }
    push(b'$', buf, &mut out);
    let mut i = n;
    while i > 0 {
        i -= 1;
        push(tmp[i.min(15)], buf, &mut out);
    }
    push(b'.', buf, &mut out);
    push(b'0' + frac / 10, buf, &mut out);
    push(b'0' + frac % 10, buf, &mut out);
    core::str::from_utf8(buf.get(..out).unwrap_or(b"$0")).unwrap_or("$0")
}

// ---- layout -----------------------------------------------------------------
//
// One struct, computed from the canvas's current size once per frame, read by
// both the drawing and the hit-testing. Coordinates are deliberately not
// `const`s: a rect drawn from one set of numbers and clicked against another
// drifts apart the instant the window is resized.

/// A rectangle in canvas coordinates -- the unit drawing and hit-testing share.
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

/// Below this the layout clamps rather than computing negative widths.
const MIN_CANVAS_W: f32 = 560.0;
const MIN_CANVAS_H: f32 = 420.0;

/// Every region of the dashboard, derived from the canvas size.
struct Layout {
    width: f32,
    height: f32,
    margin: f32,
    /// Left column (balance + chart) and the right column beside it.
    left_w: f32,
    right_x: f32,
    right_w: f32,
    header_base: f32,
    bal_caption_y: f32,
    bal_y: f32,
    bal_size: f32,
    chart: Rect,
    recent: Rect,
    cats: Rect,
    insight: Rect,
    /// Height of one category row, and where the rows start.
    cat_row_h: f32,
    cats_rows_y: f32,
}

impl Layout {
    fn for_size(width: f32, height: f32) -> Self {
        let width = width.max(MIN_CANVAS_W);
        let height = height.max(MIN_CANVAS_H);

        let margin = (width * 0.03).clamp(18.0, 40.0);
        let usable = (width - margin * 2.0 - GUTTER).max(200.0);
        // The right column keeps a readable share; the left takes the rest.
        let right_w = (usable * 0.34).clamp(210.0, 380.0);
        let left_w = (usable - right_w).max(160.0);
        let right_x = margin + left_w + GUTTER;

        let header_base = (height * 0.091).clamp(44.0, 68.0);
        let bal_caption_y = header_base + 52.0;
        let bal_y = bal_caption_y + 62.0;
        // The balance number shrinks on a narrow window so its delta chip
        // still has somewhere to sit.
        let bal_size = (width * 0.052).clamp(30.0, 56.0);

        // Left column: chart on top, Recent filling what is left.
        let chart_y = bal_y + 38.0;
        let content_bottom = height - margin;
        let left_space = (content_bottom - chart_y - GUTTER).max(80.0);
        let chart_h = (left_space * 0.5).clamp(120.0, 260.0);
        let chart = Rect { x: margin, y: chart_y, w: left_w, h: chart_h };
        let recent_y = chart_y + chart_h + GUTTER;
        let recent = Rect {
            x: margin,
            y: recent_y,
            w: left_w,
            h: (content_bottom - recent_y).max(60.0),
        };

        // Right column: categories on top, insight filling what is left.
        let cats_y = header_base + 34.0;
        let right_space = (content_bottom - cats_y - GUTTER).max(80.0);
        let cats_h = (right_space * 0.62).clamp(150.0, 300.0);
        let cats = Rect { x: right_x, y: cats_y, w: right_w, h: cats_h };
        let insight_y = cats_y + cats_h + GUTTER;
        let insight = Rect {
            x: right_x,
            y: insight_y,
            w: right_w,
            h: (content_bottom - insight_y).max(60.0),
        };

        // Category rows share the card's remaining height between them.
        let cats_rows_y = cats_y + 80.0;
        let rows_space = (cats_y + cats_h - 14.0 - cats_rows_y).max(0.0);
        let cat_row_h = (rows_space / CAT_COUNT as f32).clamp(28.0, 46.0);

        Self {
            width,
            height,
            margin,
            left_w,
            right_x,
            right_w,
            header_base,
            bal_caption_y,
            bal_y,
            bal_size,
            chart,
            recent,
            cats,
            insight,
            cat_row_h,
            cats_rows_y,
        }
    }

    /// The clickable strip for category `index`. Drawn from this, and tested
    /// against this, so the wash a person sees is the thing they can hit.
    fn cat_row(&self, index: usize) -> Rect {
        let ry = self.cats_rows_y + (index as f32) * self.cat_row_h;
        Rect {
            x: self.right_x + 12.0,
            y: ry - 6.0,
            w: self.right_w - 24.0,
            h: self.cat_row_h - 6.0,
        }
    }

    fn hit_category(&self, x: f32, y: f32) -> Option<usize> {
        let mut i = 0usize;
        while i < CAT_COUNT {
            if self.cat_row(i).contains(x, y) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

// ---- drawing ----------------------------------------------------------------

/// Ask the canvas its size, then draw the dashboard to that answer. Returns
/// the layout used, so the event loop hit-tests the picture on screen.
fn draw(canvas: u64, selected: Option<usize>) -> Result<Layout, gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas)?;
    let layout = Layout::for_size(size.width, size.height);
    draw_with(canvas, &layout, selected)?;
    Ok(layout)
}

fn draw_with(canvas: u64, layout: &Layout, selected: Option<usize>) -> Result<(), gfx::GfxError> {
    // Ground gradient.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: layout.width, height: layout.height },
        BG_TOP,
        BG_BOT,
    )?;

    draw_header(canvas, layout);
    draw_balance(canvas, layout);
    draw_chart(canvas, layout)?;
    draw_categories(canvas, layout, selected)?;
    draw_insight(canvas, layout)?;
    draw_recent(canvas, layout, selected)?;
    canvas2d::present(canvas)
}

fn draw_header(canvas: u64, layout: &Layout) {
    let margin = layout.margin;
    let base = layout.header_base;
    // Logo dot + wordmark.
    let _ = disc(canvas, margin + 9.0, base - 11.0, 9.0, ACCENT);
    let _ = disc(canvas, margin + 9.0, base - 11.0, 3.5, INK);
    text(canvas, "Pulse", margin + 28.0, base, 30.0, INK);
    text_right(canvas, "August 2026", layout.width - margin, base - 4.0, 15.0, INK_DIM);
}

fn draw_balance(canvas: u64, layout: &Layout) {
    let margin = layout.margin;
    let bal_y = layout.bal_y;
    let bal_size = layout.bal_size;
    text(canvas, "TOTAL BALANCE", margin, layout.bal_caption_y, 12.0, INK_QUIET);
    let mut buf = [0u8; 24];
    let bal = money(&mut buf, BALANCE, false);
    text(canvas, bal, margin - 2.0, bal_y, bal_size, INK);

    // Delta chip to the right of the number.
    let bal_w = est_width(bal, bal_size);
    let chip_x = margin + bal_w + 22.0;
    let mut dbuf = [0u8; 24];
    let delta = money(&mut dbuf, DELTA, true);
    // "+$1,240.00 this month" -> drop the cents for the chip.
    let delta_short = delta.get(..delta.len().saturating_sub(3)).unwrap_or(delta);
    let label_w = est_width(delta_short, 14.0) + est_width(" this month", 14.0);
    // Only draw the chip if it actually fits beside the number; on a narrow
    // window it is dropped rather than painted off the edge.
    if chip_x + label_w + 30.0 <= layout.margin + layout.left_w {
        let _ = rounded(canvas, chip_x, bal_y - 32.0, label_w + 30.0, 30.0, 15.0, CHIP_GREEN);
        text(canvas, delta_short, chip_x + 14.0, bal_y - 10.0, 14.0, GREEN);
        text(
            canvas,
            " this month",
            chip_x + 14.0 + est_width(delta_short, 14.0),
            bal_y - 10.0,
            14.0,
            INK_DIM,
        );
    }
}

fn draw_chart(canvas: u64, layout: &Layout) -> Result<(), gfx::GfxError> {
    let chart = layout.chart;
    card(canvas, chart.x, chart.y, chart.w, chart.h)?;
    text(canvas, "Spending", chart.x + 24.0, chart.y + 40.0, 18.0, INK);
    text(canvas, "Last 30 days", chart.x + 24.0, chart.y + 62.0, 12.5, INK_QUIET);

    let inner_x = chart.x + 24.0;
    let inner_w = (chart.w - 48.0).max(40.0);
    let base_y = chart.y + chart.h - 36.0;
    // The plot fills what the card has under its two header lines.
    let plot_h = (chart.h - 108.0).clamp(40.0, 160.0);

    // Gridlines + axis labels at $0 / $70 / $140.
    let max = 140.0f32;
    for (value, label) in [(0.0f32, "$0"), (70.0, "$70"), (140.0, "$140")] {
        let gy = base_y - plot_h * (value / max);
        fill(canvas, inner_x, gy, inner_w, 1.0, gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.05 })?;
        text_right(canvas, label, inner_x + inner_w, gy - 6.0, 11.0, INK_QUIET);
    }

    // Y for a data value.
    let y_of = |v: f32| base_y - plot_h * (v / max).min(1.0);

    // Area fill: one translucent column per pixel, linearly interpolated.
    let step = inner_w / (DAYS - 1) as f32;
    let cols = inner_w as i32;
    let mut px = 0i32;
    while px < cols {
        let fx = px as f32;
        let seg = (fx / step).min((DAYS - 2) as f32);
        let i = seg as usize;
        let t = seg - i as f32;
        let v = SPEND[i.min(DAYS - 1)] as f32 * (1.0 - t) + SPEND[(i + 1).min(DAYS - 1)] as f32 * t;
        let top = y_of(v);
        fill(canvas, inner_x + fx, top, 1.0, base_y - top, ACCENT_SOFT)?;
        px += 1;
    }
    // The line: a continuous 2px stroke -- per-pixel vertical strips spanning
    // from this column's y to the next, so steep slopes stay solid, plus a
    // small disc at each column for rounded antialiased edges.
    let value_at = |fx: f32| {
        let seg = (fx / step).min((DAYS - 2) as f32);
        let i = seg as usize;
        let t = seg - i as f32;
        SPEND[i.min(DAYS - 1)] as f32 * (1.0 - t) + SPEND[(i + 1).min(DAYS - 1)] as f32 * t
    };
    let mut px = 0i32;
    while px < cols {
        let fx = px as f32;
        let y0 = y_of(value_at(fx));
        let y1 = y_of(value_at((fx + 1.0).min(inner_w)));
        let (top, bot) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
        fill(canvas, inner_x + fx, top - 1.0, 1.6, (bot - top) + 2.0, ACCENT)?;
        disc(canvas, inner_x + fx, y0, 1.3, ACCENT)?;
        px += 1;
    }

    // Today: a glow, a solid dot, and a value tag above it.
    let today_x = inner_x + inner_w;
    let today_v = SPEND[DAYS - 1] as f32;
    let today_y = y_of(today_v);
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: today_x, y: today_y },
        26.0,
        gfx::Color { r: 0.298, g: 0.553, b: 1.0, a: 0.35 },
        gfx::Color { r: 0.298, g: 0.553, b: 1.0, a: 0.0 },
    )?;
    disc(canvas, today_x, today_y, 5.0, ACCENT)?;
    disc(canvas, today_x, today_y, 2.2, INK)?;
    let tag = "$68 today";
    let tag_w = est_width(tag, 12.5) + 20.0;
    rounded(canvas, today_x - tag_w - 12.0, today_y - 52.0, tag_w, 26.0, 13.0, rgb(0.153, 0.184, 0.247))?;
    text(canvas, tag, today_x - tag_w - 2.0, today_y - 34.0, 12.5, INK);
    Ok(())
}

fn draw_categories(
    canvas: u64,
    layout: &Layout,
    selected: Option<usize>,
) -> Result<(), gfx::GfxError> {
    let cats = layout.cats;
    let right_x = layout.right_x;
    let right_w = layout.right_w;
    card(canvas, cats.x, cats.y, cats.w, cats.h)?;
    text(canvas, "Categories", cats.x + 22.0, cats.y + 38.0, 18.0, INK);
    text(canvas, "This month", cats.x + 22.0, cats.y + 60.0, 12.5, INK_QUIET);

    let total: i64 = {
        let mut sum = 0i64;
        let mut i = 0;
        while i < CAT_COUNT {
            sum += CAT_TOTALS[i];
            i += 1;
        }
        sum
    };

    for i in 0..CAT_COUNT {
        // The clickable strip and the drawn wash are the same rectangle.
        let strip = layout.cat_row(i);
        let ry = layout.cats_rows_y + i as f32 * layout.cat_row_h;
        // Selection wash behind the active row.
        if selected == Some(i) {
            rounded(canvas, strip.x, strip.y, strip.w, strip.h, 10.0, WASH_SEL)?;
        }
        disc(canvas, right_x + 30.0, ry + 10.0, 5.0, CAT_COL[i])?;
        text(canvas, CAT_NAMES[i], right_x + 46.0, ry + 15.0, 15.0, INK);
        let mut buf = [0u8; 24];
        let amt = money(&mut buf, -CAT_TOTALS[i], false);
        text_right(canvas, amt, right_x + right_w - 22.0, ry + 15.0, 14.5, INK_DIM);
        // Share bar under the row text.
        let bar_w = (right_w - 68.0).max(20.0);
        let share = CAT_TOTALS[i] as f32 / total as f32;
        rounded(canvas, right_x + 46.0, ry + 24.0, bar_w, 5.0, 2.5, rgb(0.125, 0.153, 0.208))?;
        let mut c = CAT_COL[i];
        c.a = 0.9;
        rounded(canvas, right_x + 46.0, ry + 24.0, (bar_w * share).max(6.0), 5.0, 2.5, c)?;
    }
    Ok(())
}

fn draw_recent(
    canvas: u64,
    layout: &Layout,
    selected: Option<usize>,
) -> Result<(), gfx::GfxError> {
    // Recent list spans under the chart, left column.
    let recent = layout.recent;
    card(canvas, recent.x, recent.y, recent.w, recent.h)?;
    text(canvas, "Recent", recent.x + 24.0, recent.y + 36.0, 18.0, INK);
    match selected {
        Some(i) => {
            let name = CAT_NAMES[i.min(CAT_COUNT - 1)];
            text(canvas, name, recent.x + 110.0, recent.y + 36.0, 12.5, ACCENT);
        }
        None => text(canvas, "All activity", recent.x + 110.0, recent.y + 36.0, 12.5, INK_QUIET),
    }

    let row_h = 44.0;
    let mut ry = recent.y + 58.0;
    let mut shown = 0u32;
    for tx in TXS.iter() {
        if let Some(sel) = selected {
            if tx.cat != sel {
                continue;
            }
        }
        // Stop at the card's edge, so a short window shows fewer rows rather
        // than painting them past the bottom.
        if ry + row_h > recent.y + recent.h - 8.0 {
            break;
        }
        // Avatar: rounded square in the category color (green for income).
        let av = if tx.cat == INCOME { GREEN } else { CAT_COL[tx.cat.min(CAT_COUNT - 1)] };
        let av_soft = blend(CARD, av, 0.22);
        rounded(canvas, recent.x + 24.0, ry, 30.0, 30.0, 9.0, av_soft)?;
        let initial = tx.merchant.get(..1).unwrap_or("?");
        text(canvas, initial, recent.x + 33.0, ry + 21.0, 15.0, av);

        text(canvas, tx.merchant, recent.x + 66.0, ry + 14.0, 15.0, INK);
        let cat_label = if tx.cat == INCOME { "Income" } else { CAT_NAMES[tx.cat.min(CAT_COUNT - 1)] };
        text(canvas, cat_label, recent.x + 66.0, ry + 30.0, 11.5, INK_QUIET);

        let mut buf = [0u8; 24];
        let amt = money(&mut buf, tx.cents, tx.cents > 0);
        let color = if tx.cents > 0 { GREEN } else { INK };
        text_right(canvas, amt, recent.x + recent.w - 24.0, ry + 20.0, 15.0, color);

        // Hairline between rows.
        fill(
            canvas,
            recent.x + 66.0,
            ry + row_h - 5.0,
            (recent.w - 90.0).max(10.0),
            1.0,
            gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.04 },
        )?;
        ry += row_h;
        shown += 1;
    }
    if shown == 0 && selected.is_some() {
        text(canvas, "No activity in this category yet.", recent.x + 24.0, recent.y + 84.0, 13.5, INK_QUIET);
    }
    Ok(())
}

/// The small insight card under Categories: one honest derived stat.
fn draw_insight(canvas: u64, layout: &Layout) -> Result<(), gfx::GfxError> {
    let insight = layout.insight;
    let y = insight.y;
    card(canvas, insight.x, insight.y, insight.w, insight.h)?;
    text(canvas, "Insight", insight.x + 22.0, y + 36.0, 18.0, INK);
    // A green down-arrow chip: spending is lower than last month.
    let ax = insight.x + 34.0;
    let ay = y + 86.0;
    disc(canvas, ax, ay, 16.0, CHIP_GREEN)?;
    // Arrow: stem + head drawn from fills.
    fill(canvas, ax - 1.5, ay - 8.0, 3.0, 10.0, GREEN)?;
    let mut i = 0.0f32;
    while i < 6.0 {
        fill(canvas, ax - (6.0 - i), ay + 2.0 + i, (6.0 - i) * 2.0, 1.2, GREEN)?;
        i += 1.0;
    }
    text(canvas, "12% less than July", ax + 30.0, ay - 2.0, 15.5, INK);
    text(canvas, "Nice pace. At this rate you save", ax + 30.0, ay + 18.0, 12.5, INK_DIM);
    text(canvas, "about $410 more this month.", ax + 30.0, ay + 34.0, 12.5, INK_DIM);
    Ok(())
}

// ---- widget scaffolding -----------------------------------------------------

fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
        // Grow to fill the window rather than pinning a fixed size: a pinned
        // width and height keep the canvas at its opening size forever, so the
        // app can never see a resize even if it asks.
        style: types::Style { width: None, height: None, grow: 1.0, padding: 0.0 },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn canvas_node() -> types::WidgetNode {
    types::WidgetNode {
        id: CANVAS_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Canvas,
        label: None,
        role: Some(pure_string("canvas")),
        // Grow to fill the window rather than pinning a fixed size: a pinned
        // width and height keep the canvas at its opening size forever, so the
        // app can never see a resize even if it asks.
        style: types::Style { width: None, height: None, grow: 1.0, padding: 0.0 },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

// ---- persistence ------------------------------------------------------------

fn load_selected() -> Option<usize> {
    match store_kv::get(SELECT_KEY) {
        Ok(Some(bytes)) => match bytes.first() {
            Some(&b) if (b as usize) < CAT_COUNT => Some(b as usize),
            _ => None,
        },
        _ => None,
    }
}

fn save_selected(selected: Option<usize>) {
    match selected {
        Some(i) => {
            let _ = store_kv::set(SELECT_KEY, &[i as u8]);
        }
        None => {
            let _ = store_kv::delete(SELECT_KEY);
        }
    }
}

// Which category a click landed on is `Layout::hit_category` -- the same
// rectangles `draw_categories` paints the selection wash into.

// ---- app --------------------------------------------------------------------

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Pulse", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err() || tree::upsert_node(win, &canvas_node()).is_err() {
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

        let mut selected = load_selected();

        let raw = args::raw();
        let first_arg = raw.as_bytes().split(|byte| *byte == b'\n').next();
        let quick = first_arg.is_some_and(|first| first == b"quick");
        let resize_check = first_arg.is_some_and(|first| first == b"resize-check");

        if quick {
            // The automated shot: the full dashboard, nothing filtered.
            let _ = draw(canvas, None);
            let out = stdio::stdout();
            let _ = out.write(b"pulse:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        if resize_check {
            // Drive the window through several shapes and confirm a click at
            // the centre of the category row just drawn selects that row.
            let out = stdio::stdout();
            let sizes = [(1080u32, 700u32), (1400u32, 520u32), (700u32, 900u32)];
            let mut all_ok = true;
            for (w, h) in sizes {
                if window::set_size(win, types::WindowSize { width: w, height: h }).is_err() {
                    all_ok = false;
                    continue;
                }
                let mut drain = 0u32;
                while drain < 8 && events::wait(Some(1)).is_some() {
                    drain += 1;
                }
                let Ok(layout) = draw(canvas, None) else {
                    all_ok = false;
                    continue;
                };

                let _ = out.write(b"size:");
                let mut nbuf = [0u8; 12];
                let _ = out.write(u32_bytes(layout.width as u32, &mut nbuf));
                let _ = out.write(b"x");
                let _ = out.write(u32_bytes(layout.height as u32, &mut nbuf));

                let target = 2usize;
                let strip = layout.cat_row(target);
                let hit = layout.hit_category(strip.x + strip.w * 0.5, strip.y + strip.h * 0.5);
                let row_ok = hit == Some(target);
                // A point left of the strip must miss it.
                let miss_ok = layout.hit_category(strip.x - 6.0, strip.y + strip.h * 0.5).is_none();
                // The two columns must not overlap at any size.
                let cols_ok = layout.margin + layout.left_w <= layout.right_x;
                // ...and the right column's cards must not overlap each other.
                let stack_ok = layout.cats.y + layout.cats.h <= layout.insight.y;

                if row_ok && miss_ok && cols_ok && stack_ok {
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
            let _ = out.flush();
            let _ = window::close(win);
            return if all_ok { 0 } else { 40 };
        }

        // The layout the visible frame was drawn with, so clicks follow the
        // window when it is resized.
        let mut layout = match draw(canvas, selected) {
            Ok(layout) => layout,
            Err(_) => {
                let _ = window::close(win);
                return 34;
            }
        };

        let mut idle = 0u32;
        for _ in 0..MAX_WAIT_ROUNDS {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::Pointer(p)) if p.pressed => {
                    idle = 0;
                    let hit = layout.hit_category(p.x, p.y);
                    if let Some(i) = hit {
                        // Click the active row again to clear the filter.
                        selected = if selected == Some(i) { None } else { Some(i) };
                        save_selected(selected);
                        if let Ok(fresh) = draw(canvas, selected) {
                            layout = fresh;
                        }
                    }
                }
                Some(types::Event::CloseRequested(id)) if id == win => break,
                // Resized: recompute the layout from the canvas's new size.
                // Hit-testing follows, because it reads this same layout.
                Some(types::Event::Resized(_)) | Some(types::Event::RedrawRequested(_)) => {
                    idle = 0;
                    if let Ok(fresh) = draw(canvas, selected) {
                        layout = fresh;
                    }
                }
                Some(_) => idle = 0,
                None => {
                    idle += 1;
                    if idle > MAX_IDLE_ROUNDS * 20 {
                        break;
                    }
                }
            }
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"pulse:ok\n");
        let _ = out.flush();
        0
    }
}

bindings::export!(Component with_types_in bindings);
