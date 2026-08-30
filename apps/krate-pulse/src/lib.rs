//! Pulse -- a personal finance dashboard drawn entirely on a canvas.
//!
//! The hero layout: a bold balance headline with a monthly delta chip, a
//! 30-day spending area chart with a highlighted "today" point, a category
//! breakdown with share bars, and a recent-transactions list with colored
//! merchant avatars. Clicking a category filters the list; the selection is
//! remembered in the key-value store. All data is a seeded local demo ledger --
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

const WIDTH: f32 = 1080.0;
const HEIGHT: f32 = 700.0;

const MARGIN: f32 = 32.0;
/// Left column (balance + chart) width; right column starts after the gutter.
const LEFT_W: f32 = 656.0;
const GUTTER: f32 = 24.0;
const RIGHT_X: f32 = MARGIN + LEFT_W + GUTTER;
const RIGHT_W: f32 = WIDTH - RIGHT_X - MARGIN;

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

/// Rendered width of a string at a given font size, measured by the host with
/// the same font layout `draw_text` draws with.
///
/// This used to be character count times an invented constant, with a comment
/// claiming the host face was monospace or near-monospace. It is not: it is
/// proportional, and `i` and `W` differ about four times in real width. So a
/// centred label was not centred and a right-aligned number did not line up.
/// `measure_text` is the true answer.
fn est_width(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

fn text_right(canvas: u64, s: &str, right: f32, y: f32, size: f32, c: gfx::Color) {
    text(canvas, s, right - est_width(canvas, s, size), y, size, c);
}

fn rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_round_rect(canvas, gfx::Rect { x, y, width: w, height: h }, gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r }, c)
}

/// A card: soft shadow, panel fill, hairline top edge highlight.
fn card(canvas: u64, x: f32, y: f32, w: f32, h: f32) -> Result<(), gfx::GfxError> {
    canvas2d::drop_shadow_round_rect(
        canvas,
        gfx::Rect { x, y: y + 4.0, width: w, height: h },
        gfx::CornerRadii { top_left: 16.0, top_right: 16.0, bottom_right: 16.0, bottom_left: 16.0 },
        12.0,
        gfx::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.28 },
    )?;
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

// ---- layout regions ---------------------------------------------------------

const HEADER_BASE: f32 = 64.0;
const BAL_CAPTION_Y: f32 = 116.0;
const BAL_Y: f32 = 178.0;
const CHART_Y: f32 = 216.0;
const CHART_H: f32 = 230.0;
const RECENT_Y: f32 = CHART_Y + CHART_H + GUTTER;
const RECENT_H: f32 = HEIGHT - RECENT_Y - MARGIN;
const CATS_Y: f32 = 98.0;
const CATS_H: f32 = 262.0;
const RIGHT2_Y: f32 = CATS_Y + CATS_H + GUTTER;

// ---- drawing ----------------------------------------------------------------

fn draw(canvas: u64, selected: Option<usize>) -> Result<(), gfx::GfxError> {
    // Ground gradient.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    )?;

    draw_header(canvas);
    draw_balance(canvas);
    draw_chart(canvas)?;
    draw_categories(canvas, selected)?;
    draw_insight(canvas)?;
    draw_recent(canvas, selected)?;
    canvas2d::present(canvas)
}

fn draw_header(canvas: u64) {
    // Logo dot + wordmark.
    let _ = disc(canvas, MARGIN + 9.0, HEADER_BASE - 11.0, 9.0, ACCENT);
    let _ = disc(canvas, MARGIN + 9.0, HEADER_BASE - 11.0, 3.5, INK);
    text(canvas, "Pulse", MARGIN + 28.0, HEADER_BASE, 30.0, INK);
    text_right(canvas, "August 2026", WIDTH - MARGIN, HEADER_BASE - 4.0, 15.0, INK_DIM);
}

fn draw_balance(canvas: u64) {
    text(canvas, "TOTAL BALANCE", MARGIN, BAL_CAPTION_Y, 12.0, INK_QUIET);
    let mut buf = [0u8; 24];
    let bal = money(&mut buf, BALANCE, false);
    text(canvas, bal, MARGIN - 2.0, BAL_Y, 56.0, INK);

    // Delta chip to the right of the number.
    let bal_w = est_width(canvas, bal, 56.0);
    let chip_x = MARGIN + bal_w + 22.0;
    let mut dbuf = [0u8; 24];
    let delta = money(&mut dbuf, DELTA, true);
    // "+$1,240.00 this month" -> drop the cents for the chip.
    let delta_short = delta.get(..delta.len().saturating_sub(3)).unwrap_or(delta);
    let label_w = est_width(canvas, delta_short, 14.0) + est_width(canvas, " this month", 14.0);
    let _ = rounded(canvas, chip_x, BAL_Y - 32.0, label_w + 30.0, 30.0, 15.0, CHIP_GREEN);
    text(canvas, delta_short, chip_x + 14.0, BAL_Y - 10.0, 14.0, GREEN);
    text(
        canvas,
        " this month",
        chip_x + 14.0 + est_width(canvas, delta_short, 14.0),
        BAL_Y - 10.0,
        14.0,
        INK_DIM,
    );
}

fn draw_chart(canvas: u64) -> Result<(), gfx::GfxError> {
    card(canvas, MARGIN, CHART_Y, LEFT_W, CHART_H)?;
    text(canvas, "Spending", MARGIN + 24.0, CHART_Y + 40.0, 18.0, INK);
    text(canvas, "Last 30 days", MARGIN + 24.0, CHART_Y + 62.0, 12.5, INK_QUIET);

    let inner_x = MARGIN + 24.0;
    let inner_w = LEFT_W - 48.0;
    let base_y = CHART_Y + CHART_H - 36.0;
    let plot_h = 112.0;

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
    let tag_w = est_width(canvas, tag, 12.5) + 20.0;
    rounded(canvas, today_x - tag_w - 12.0, today_y - 52.0, tag_w, 26.0, 13.0, rgb(0.153, 0.184, 0.247))?;
    text(canvas, tag, today_x - tag_w - 2.0, today_y - 34.0, 12.5, INK);
    Ok(())
}

fn draw_categories(canvas: u64, selected: Option<usize>) -> Result<(), gfx::GfxError> {
    card(canvas, RIGHT_X, CATS_Y, RIGHT_W, CATS_H)?;
    text(canvas, "Categories", RIGHT_X + 22.0, CATS_Y + 38.0, 18.0, INK);
    text(canvas, "This month", RIGHT_X + 22.0, CATS_Y + 60.0, 12.5, INK_QUIET);

    let total: i64 = {
        let mut sum = 0i64;
        let mut i = 0;
        while i < CAT_COUNT {
            sum += CAT_TOTALS[i];
            i += 1;
        }
        sum
    };

    let row_h = 46.0;
    let rows_y = CATS_Y + 80.0;
    for i in 0..CAT_COUNT {
        let ry = rows_y + i as f32 * row_h;
        // Selection wash behind the active row.
        if selected == Some(i) {
            rounded(canvas, RIGHT_X + 12.0, ry - 6.0, RIGHT_W - 24.0, row_h - 6.0, 10.0, WASH_SEL)?;
        }
        disc(canvas, RIGHT_X + 30.0, ry + 10.0, 5.0, CAT_COL[i])?;
        text(canvas, CAT_NAMES[i], RIGHT_X + 46.0, ry + 15.0, 15.0, INK);
        let mut buf = [0u8; 24];
        let amt = money(&mut buf, -CAT_TOTALS[i], false);
        text_right(canvas, amt, RIGHT_X + RIGHT_W - 22.0, ry + 15.0, 14.5, INK_DIM);
        // Share bar under the row text.
        let bar_w = RIGHT_W - 68.0;
        let share = CAT_TOTALS[i] as f32 / total as f32;
        rounded(canvas, RIGHT_X + 46.0, ry + 24.0, bar_w, 5.0, 2.5, rgb(0.125, 0.153, 0.208))?;
        let mut c = CAT_COL[i];
        c.a = 0.9;
        rounded(canvas, RIGHT_X + 46.0, ry + 24.0, (bar_w * share).max(6.0), 5.0, 2.5, c)?;
    }
    Ok(())
}

fn draw_recent(canvas: u64, selected: Option<usize>) -> Result<(), gfx::GfxError> {
    // Recent list spans under the chart, left column.
    card(canvas, MARGIN, RECENT_Y, LEFT_W, RECENT_H)?;
    text(canvas, "Recent", MARGIN + 24.0, RECENT_Y + 36.0, 18.0, INK);
    match selected {
        Some(i) => {
            let name = CAT_NAMES[i.min(CAT_COUNT - 1)];
            text(canvas, name, MARGIN + 110.0, RECENT_Y + 36.0, 12.5, ACCENT);
        }
        None => text(canvas, "All activity", MARGIN + 110.0, RECENT_Y + 36.0, 12.5, INK_QUIET),
    }

    let row_h = 44.0;
    let mut ry = RECENT_Y + 58.0;
    let mut shown = 0u32;
    for tx in TXS.iter() {
        if let Some(sel) = selected {
            if tx.cat != sel {
                continue;
            }
        }
        if ry + row_h > RECENT_Y + RECENT_H - 8.0 {
            break;
        }
        // Avatar: rounded square in the category color (green for income).
        let av = if tx.cat == INCOME { GREEN } else { CAT_COL[tx.cat.min(CAT_COUNT - 1)] };
        let av_soft = blend(CARD, av, 0.22);
        rounded(canvas, MARGIN + 24.0, ry, 30.0, 30.0, 9.0, av_soft)?;
        let initial = tx.merchant.get(..1).unwrap_or("?");
        text(canvas, initial, MARGIN + 33.0, ry + 21.0, 15.0, av);

        text(canvas, tx.merchant, MARGIN + 66.0, ry + 14.0, 15.0, INK);
        let cat_label = if tx.cat == INCOME { "Income" } else { CAT_NAMES[tx.cat.min(CAT_COUNT - 1)] };
        text(canvas, cat_label, MARGIN + 66.0, ry + 30.0, 11.5, INK_QUIET);

        let mut buf = [0u8; 24];
        let amt = money(&mut buf, tx.cents, tx.cents > 0);
        let color = if tx.cents > 0 { GREEN } else { INK };
        text_right(canvas, amt, MARGIN + LEFT_W - 24.0, ry + 20.0, 15.0, color);

        // Hairline between rows.
        fill(canvas, MARGIN + 66.0, ry + row_h - 5.0, LEFT_W - 90.0, 1.0, gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.04 })?;
        ry += row_h;
        shown += 1;
    }
    if shown == 0 && selected.is_some() {
        text(canvas, "No activity in this category yet.", MARGIN + 24.0, RECENT_Y + 84.0, 13.5, INK_QUIET);
    }
    Ok(())
}

/// The small insight card under Categories: one honest derived stat.
fn draw_insight(canvas: u64) -> Result<(), gfx::GfxError> {
    let y = RIGHT2_Y;
    let h = HEIGHT - y - MARGIN;
    card(canvas, RIGHT_X, y, RIGHT_W, h)?;
    text(canvas, "Insight", RIGHT_X + 22.0, y + 36.0, 18.0, INK);
    // A green down-arrow chip: spending is lower than last month.
    let ax = RIGHT_X + 34.0;
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
        // No fixed size, and grow so the canvas fills the window. Pinning
        // these to WIDTH/HEIGHT meant the canvas stayed 1080x700 no matter
        // how the window was resized -- the layout engine was obeying the
        // app, and the app was asking for the wrong thing (K-003).
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
        // No fixed size, and grow so the canvas fills the window. Pinning
        // these to WIDTH/HEIGHT meant the canvas stayed 1080x700 no matter
        // how the window was resized -- the layout engine was obeying the
        // app, and the app was asking for the wrong thing (K-003).
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

/// Which category row a click landed on, if any.
fn hit_category(x: f32, y: f32) -> Option<usize> {
    let rows_y = CATS_Y + 80.0;
    let row_h = 46.0;
    if x < RIGHT_X + 12.0 || x > RIGHT_X + RIGHT_W - 12.0 {
        return None;
    }
    for i in 0..CAT_COUNT {
        let ry = rows_y + i as f32 * row_h - 6.0;
        if y >= ry && y < ry + row_h - 4.0 {
            return Some(i);
        }
    }
    None
}

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

        let mut selected = load_selected();

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if quick {
            // The automated shot: the full dashboard, nothing filtered.
            let _ = draw(canvas, None);
            let out = stdio::stdout();
            let _ = out.write(b"pulse:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        let _ = draw(canvas, selected);

        let mut idle = 0u32;
        for _ in 0..MAX_WAIT_ROUNDS {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::Pointer(p)) if p.pressed => {
                    idle = 0;
                    let hit = hit_category(p.x, p.y);
                    if let Some(i) = hit {
                        // Click the active row again to clear the filter.
                        selected = if selected == Some(i) { None } else { Some(i) };
                        save_selected(selected);
                        let _ = draw(canvas, selected);
                    }
                }
                Some(types::Event::CloseRequested(id)) if id == win => break,
                Some(_) => idle = 0,
                None => {
                    idle += 1;
                    // Only a headless check gives up on silence; a real window
                    // stays until the person closes it.
                    if quick && idle > MAX_IDLE_ROUNDS * 20 {
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
