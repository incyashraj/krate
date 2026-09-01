//! Trace -- a log and trace viewer for a service, drawn entirely on a canvas.
//!
//! The hero layout: a service bar with a time range and p50/p99/error
//! readouts, a request-volume bar chart across the full width with error
//! minutes standing out in red, and under it the real subject -- a log table
//! whose rows carry a level stripe, an aligned timestamp, a service tag, and
//! a message. One error row is expanded to show its context block, because
//! that is what you actually do with an error line.
//!
//! All data is a seeded local demo window of traffic. Nothing is fetched;
//! the app declares no network capability at all.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler. All state is fixed-size; numbers are formatted by
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
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;

const MARGIN: f32 = 28.0;
const CONTENT_W: f32 = WIDTH - MARGIN * 2.0;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;
const MAX_IDLE_ROUNDS: u32 = 300;

// ---- palette (the shared canvas design system) ------------------------------

const BG_TOP: gfx::Color = rgb(0.043, 0.055, 0.082);
const BG_BOT: gfx::Color = rgb(0.063, 0.078, 0.114);
const CARD: gfx::Color = rgb(0.086, 0.106, 0.149);
const CARD_EDGE: gfx::Color = rgb(0.137, 0.165, 0.220);
const INK: gfx::Color = rgb(0.949, 0.961, 0.980);
const INK_DIM: gfx::Color = rgb(0.604, 0.647, 0.710);
const INK_QUIET: gfx::Color = rgb(0.365, 0.408, 0.471);

/// Level colors. These carry the whole scan: a reader finds the red stripe
/// before reading a single word, so nothing else in the app may be red.
const LV_INFO: gfx::Color = rgb(0.373, 0.612, 0.851);
const LV_WARN: gfx::Color = rgb(0.976, 0.694, 0.267);
const LV_ERROR: gfx::Color = rgb(0.965, 0.353, 0.373);
const LV_DEBUG: gfx::Color = rgb(0.510, 0.545, 0.639);

/// Pre-blended opaque tints: translucent rounded shapes double alpha where the
/// rect and corner discs overlap, so chips and washes use flattened colors.
const CHIP: gfx::Color = rgb(0.114, 0.137, 0.192);
const CHIP_EDGE: gfx::Color = rgb(0.169, 0.204, 0.271);
const BAR_OK: gfx::Color = rgb(0.243, 0.404, 0.639);
const BAR_OK_TOP: gfx::Color = rgb(0.318, 0.529, 0.839);
const GRID: gfx::Color = gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.055 };
const HAIRLINE: gfx::Color = gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.045 };

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

// ---- the demo traffic window ------------------------------------------------

const LV_I: u8 = 0;
const LV_W: u8 = 1;
const LV_E: u8 = 2;
const LV_D: u8 = 3;

fn level_color(level: u8) -> gfx::Color {
    match level {
        LV_W => LV_WARN,
        LV_E => LV_ERROR,
        LV_D => LV_DEBUG,
        _ => LV_INFO,
    }
}

fn level_name(level: u8) -> &'static str {
    match level {
        LV_W => "WARN",
        LV_E => "ERROR",
        LV_D => "DEBUG",
        _ => "INFO",
    }
}

/// One log line. `context` is the expanded block under an error row; empty
/// means the row is a single line.
struct Line {
    stamp: &'static str,
    level: u8,
    service: &'static str,
    message: &'static str,
    context: &'static [&'static str],
}

const ORDER_CTX: [&str; 3] = [
    "upstream: payments-api.svc.cluster.local:8443",
    "trace_id: 7f3a91c4e0b28d65  span: checkout.charge",
    "retry 3/3 exhausted, returning 504 to client",
];

const ROWS: [Line; 12] = [
    Line { stamp: "14:32:07.412", level: LV_I, service: "api-gateway", message: "GET /v1/orders 200 18ms", context: &[] },
    Line { stamp: "14:32:07.588", level: LV_I, service: "api-gateway", message: "GET /v1/orders/8821 200 24ms", context: &[] },
    Line { stamp: "14:32:08.031", level: LV_D, service: "auth", message: "token cache hit for tenant acme-prod", context: &[] },
    Line { stamp: "14:32:08.947", level: LV_W, service: "orders", message: "retrying connection to postgres-primary", context: &[] },
    Line { stamp: "14:32:09.203", level: LV_I, service: "api-gateway", message: "POST /v1/checkout 201 142ms", context: &[] },
    Line { stamp: "14:32:11.664", level: LV_E, service: "checkout", message: "upstream timeout after 5000ms", context: &ORDER_CTX },
    Line { stamp: "14:32:12.010", level: LV_I, service: "api-gateway", message: "GET /v1/health 200 3ms", context: &[] },
    Line { stamp: "14:32:12.774", level: LV_W, service: "api-gateway", message: "rate limit exceeded for key sk_live_4Kf9...", context: &[] },
    Line { stamp: "14:32:13.126", level: LV_I, service: "orders", message: "PATCH /v1/orders/8821 200 31ms", context: &[] },
    Line { stamp: "14:32:13.905", level: LV_E, service: "payments", message: "charge declined: card_expired (tok_1P9x)", context: &[] },
    Line { stamp: "14:32:14.338", level: LV_D, service: "orders", message: "flushed 24 events to analytics buffer", context: &[] },
    Line { stamp: "14:32:15.017", level: LV_I, service: "api-gateway", message: "GET /v1/orders 200 21ms", context: &[] },
];

/// Requests per minute over the last 15 minutes, and how many of those were
/// 5xx. The error count is what turns a bar red, so the minute of the
/// timeout above reads as the tall red one.
const BUCKETS: usize = 60;
const VOLUME: [u16; BUCKETS] = [
    418, 442, 396, 471, 503, 462, 449, 488, 512, 476, 431, 458, 497, 523, 486,
    444, 468, 502, 539, 561, 508, 473, 455, 491, 528, 566, 594, 617, 583, 541,
    498, 462, 447, 479, 516, 552, 588, 634, 671, 702, 655, 601, 559, 522, 487,
    463, 441, 476, 509, 544, 578, 612, 588, 551, 517, 483, 459, 494, 531, 468,
];
/// 5xx count per minute. Nonzero minutes paint red; the height still comes
/// from VOLUME so an error spike is visible as both color and mass.
const ERRORS: [u16; BUCKETS] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0, 41, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 88, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 27, 0, 0, 0, 0, 0, 0, 0, 0,
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

/// Rendered width of a string, measured by the host with the same font layout
/// `draw_text` draws with. The host face is proportional, so a right-aligned
/// number only lines up if the width is measured rather than guessed from the
/// character count.
fn est_width(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

fn text_right(canvas: u64, s: &str, right: f32, y: f32, size: f32, c: gfx::Color) {
    text(canvas, s, right - est_width(canvas, s, size), y, size, c);
}

fn text_center(canvas: u64, s: &str, cx: f32, y: f32, size: f32, c: gfx::Color) {
    text(canvas, s, cx - est_width(canvas, s, size) * 0.5, y, size, c);
}

fn rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_round_rect(
        canvas,
        gfx::Rect { x, y, width: w, height: h },
        gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r },
        c,
    )
}

/// A card: soft shadow, panel fill, hairline top edge highlight.
fn card(canvas: u64, x: f32, y: f32, w: f32, h: f32) -> Result<(), gfx::GfxError> {
    canvas2d::drop_shadow_round_rect(
        canvas,
        gfx::Rect { x, y: y + 4.0, width: w, height: h },
        gfx::CornerRadii { top_left: 14.0, top_right: 14.0, bottom_right: 14.0, bottom_left: 14.0 },
        12.0,
        gfx::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.28 },
    )?;
    rounded(canvas, x, y, w, h, 14.0, CARD)?;
    fill(canvas, x + 14.0, y, w - 28.0, 1.0, CARD_EDGE)?;
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

/// Format an unsigned integer into `buf` with thousands separators, returning
/// the used slice: "1,284".
fn commas(buf: &mut [u8; 16], mut value: u32) -> &str {
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
            *slot = b'0' + (value % 10) as u8;
            n += 1;
        }
        group += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let mut out = 0usize;
    let mut i = n;
    while i > 0 {
        i -= 1;
        if let (Some(&src), Some(slot)) = (tmp.get(i), buf.get_mut(out)) {
            *slot = src;
            out += 1;
        }
    }
    core::str::from_utf8(buf.get(..out).unwrap_or(b"0")).unwrap_or("0")
}

// ---- layout regions ---------------------------------------------------------

const BAR_Y: f32 = 26.0;
const BAR_H: f32 = 62.0;

const CHART_Y: f32 = BAR_Y + BAR_H + 10.0;
const CHART_H: f32 = 132.0;
/// The bar band inside the chart card. The card also has to hold the caption
/// above and the minute ticks below, so this is what is left over, not a free
/// choice.
const PLOT_H: f32 = 84.0;

const TABLE_Y: f32 = CHART_Y + CHART_H + 14.0;
const TABLE_H: f32 = HEIGHT - TABLE_Y - MARGIN;

/// Log table columns, as x offsets from the card's left edge. Fixed columns
/// are what makes twelve rows read as a table rather than twelve sentences.
const COL_STRIPE: f32 = 0.0;
const COL_STAMP: f32 = 20.0;
const COL_LEVEL: f32 = 132.0;
const COL_SERVICE: f32 = 200.0;
const COL_MSG: f32 = 316.0;

/// Row rhythm. Twelve rows plus one three-line expansion have to land inside
/// the card without a scrollbar, so these are the numbers that make the whole
/// window fit -- change one and the last row falls off the bottom.
const ROW_H: f32 = 28.0;
const CTX_LINE_H: f32 = 17.0;

// ---- drawing ----------------------------------------------------------------

fn draw(canvas: u64) -> Result<(), gfx::GfxError> {
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    )?;

    draw_service_bar(canvas)?;
    draw_volume(canvas)?;
    draw_table(canvas)?;
    canvas2d::present(canvas)
}

/// A small pill with a border: the shared shape for the time range and the
/// three stat readouts.
fn chip(canvas: u64, x: f32, y: f32, w: f32, h: f32) -> Result<(), gfx::GfxError> {
    rounded(canvas, x, y, w, h, h * 0.5, CHIP_EDGE)?;
    rounded(canvas, x + 1.0, y + 1.0, w - 2.0, h - 2.0, (h - 2.0) * 0.5, CHIP)?;
    Ok(())
}

fn draw_service_bar(canvas: u64) -> Result<(), gfx::GfxError> {
    let base = BAR_Y + 38.0;

    // Live dot + service name: the one thing a reader identifies the screen by.
    disc(canvas, MARGIN + 6.0, base - 7.0, 4.5, rgb(0.239, 0.839, 0.549))?;
    text(canvas, "api-gateway", MARGIN + 20.0, base, 22.0, INK);
    text(canvas, "production", MARGIN + 20.0 + est_width(canvas, "api-gateway", 22.0) + 12.0, base - 1.0, 12.5, INK_QUIET);

    // Time range chip, immediately after the name so the numbers below have
    // a stated window rather than floating free.
    let range = "last 15 min";
    let rw = est_width(canvas, range, 12.5) + 30.0;
    let rx = MARGIN + 20.0 + est_width(canvas, "api-gateway", 22.0) + 12.0
        + est_width(canvas, "production", 12.5) + 16.0;
    chip(canvas, rx, base - 20.0, rw, 26.0)?;
    // A tiny clock glyph: ring plus two hands.
    disc(canvas, rx + 15.0, base - 7.0, 5.0, INK_QUIET)?;
    disc(canvas, rx + 15.0, base - 7.0, 3.6, CHIP)?;
    fill(canvas, rx + 14.4, base - 10.0, 1.2, 3.4, INK_DIM)?;
    fill(canvas, rx + 15.0, base - 7.6, 3.0, 1.2, INK_DIM)?;
    text(canvas, range, rx + 24.0, base - 2.0, 12.5, INK_DIM);

    // Three stat readouts, right-aligned as a block so their values form a
    // column the eye can compare against.
    let stats: [(&str, &str, gfx::Color); 3] = [
        ("p50", "24ms", INK),
        ("p99", "310ms", LV_WARN),
        ("err", "0.4%", LV_ERROR),
    ];
    let mut right = WIDTH - MARGIN;
    let mut i = stats.len();
    while i > 0 {
        i -= 1;
        let (label, value, color) = match stats.get(i) {
            Some(s) => *s,
            None => continue,
        };
        let vw = est_width(canvas, value, 17.0);
        let lw = est_width(canvas, label, 11.5);
        let block = if vw > lw { vw } else { lw };
        text_right(canvas, label, right, base - 18.0, 11.5, INK_QUIET);
        text_right(canvas, value, right, base + 2.0, 17.0, color);
        right -= block + 34.0;
    }
    Ok(())
}

fn draw_volume(canvas: u64) -> Result<(), gfx::GfxError> {
    card(canvas, MARGIN, CHART_Y, CONTENT_W, CHART_H)?;

    let pad = 20.0;
    let inner_x = MARGIN + pad;
    let inner_w = CONTENT_W - pad * 2.0;

    text(canvas, "REQUEST VOLUME", inner_x, CHART_Y + 25.0, 11.0, INK_QUIET);
    text_right(canvas, "req / min", MARGIN + CONTENT_W - pad, CHART_Y + 25.0, 11.0, INK_QUIET);

    let base_y = CHART_Y + CHART_H - 24.0;

    // Peak is the scale, rounded up so the tallest bar does not touch the top.
    let mut peak = 1u16;
    for i in 0..BUCKETS {
        if let Some(&v) = VOLUME.get(i) {
            if v > peak {
                peak = v;
            }
        }
    }
    let max = peak as f32 * 1.12;

    // Two gridlines plus the baseline, labelled at the right so the labels
    // never collide with the bars.
    for step in [0.0f32, 0.5, 1.0] {
        let gy = base_y - PLOT_H * step;
        fill(canvas, inner_x, gy, inner_w, 1.0, GRID)?;
    }

    let slot = inner_w / BUCKETS as f32;
    let bar_w = slot - 2.2;
    for i in 0..BUCKETS {
        let v = match VOLUME.get(i) {
            Some(&v) => v as f32,
            None => continue,
        };
        let errs = ERRORS.get(i).copied().unwrap_or(0);
        let h = (PLOT_H * (v / max)).max(2.0);
        let x = inner_x + i as f32 * slot;
        let y = base_y - h;
        if errs > 0 {
            // An error minute is one solid red column: partial stacking read
            // as a rendering artifact at this bar width, not as a signal.
            fill(canvas, x, y, bar_w, h, LV_ERROR)?;
            fill(canvas, x, y, bar_w, 2.0, rgb(1.0, 0.545, 0.541))?;
        } else {
            fill(canvas, x, y, bar_w, h, BAR_OK)?;
            fill(canvas, x, y, bar_w, 2.0, BAR_OK_TOP)?;
        }
    }

    // The peak readout owns the right end of the axis line, so the minute
    // ticks get the width left over -- otherwise "now" and the peak number
    // land on the same pixels and both become unreadable.
    let mut buf = [0u8; 16];
    let peak_s = commas(&mut buf, peak as u32);
    let axis_y = base_y + 16.0;
    let peak_right = MARGIN + CONTENT_W - pad;
    let peak_left = peak_right
        - est_width(canvas, peak_s, 11.0)
        - est_width(canvas, "peak ", 10.0);
    text(canvas, "peak ", peak_left, axis_y, 10.0, INK_QUIET);
    text_right(canvas, peak_s, peak_right, axis_y, 11.0, INK_DIM);

    // Minute ticks under the axis, so the window reads as fifteen minutes
    // rather than sixty anonymous columns. "now" is right-aligned to the last
    // bar instead of centred on it, for the same reason the first is left-
    // aligned: a centred label at either end hangs off the plot.
    let ticks: [(usize, &str); 4] = [(0, "-15m"), (20, "-10m"), (40, "-5m"), (59, "now")];
    for &(idx, label) in ticks.iter() {
        let w = est_width(canvas, label, 10.0);
        let cx = inner_x + idx as f32 * slot + bar_w * 0.5;
        let lx = (cx - w * 0.5).max(inner_x).min(inner_x + inner_w - w);
        // Skip a tick that would run into the peak readout rather than draw
        // it on top of one.
        if lx + w > peak_left - 14.0 {
            continue;
        }
        text(canvas, label, lx, axis_y, 10.0, INK_QUIET);
    }
    Ok(())
}

fn draw_table(canvas: u64) -> Result<(), gfx::GfxError> {
    card(canvas, MARGIN, TABLE_Y, CONTENT_W, TABLE_H)?;

    let left = MARGIN;
    let right = MARGIN + CONTENT_W;

    // Header: title on the left, live count on the right.
    text(canvas, "Logs", left + COL_STAMP, TABLE_Y + 30.0, 17.0, INK);
    let mut cbuf = [0u8; 16];
    let count = commas(&mut cbuf, 8_412);
    text_right(canvas, count, right - 20.0 - est_width(canvas, " lines", 12.0), TABLE_Y + 29.0, 12.5, INK_DIM);
    text_right(canvas, " lines", right - 20.0, TABLE_Y + 29.0, 12.0, INK_QUIET);

    // Level legend, so the stripe colors are stated once rather than guessed.
    let mut lx = left + COL_STAMP + est_width(canvas, "Logs", 17.0) + 24.0;
    for &lv in [LV_I, LV_W, LV_E].iter() {
        let c = level_color(lv);
        let name = level_name(lv);
        rounded(canvas, lx, TABLE_Y + 20.0, 3.0, 11.0, 1.5, c)?;
        text(canvas, name, lx + 9.0, TABLE_Y + 29.0, 10.5, INK_QUIET);
        lx += 9.0 + est_width(canvas, name, 10.5) + 18.0;
    }

    // Column heads sit on their own rule, and every row below uses the same
    // x offsets -- that repetition is what makes it look like a table.
    let head_y = TABLE_Y + 50.0;
    text(canvas, "TIME", left + COL_STAMP, head_y, 10.0, INK_QUIET);
    text(canvas, "LEVEL", left + COL_LEVEL, head_y, 10.0, INK_QUIET);
    text(canvas, "SERVICE", left + COL_SERVICE, head_y, 10.0, INK_QUIET);
    text(canvas, "MESSAGE", left + COL_MSG, head_y, 10.0, INK_QUIET);
    fill(canvas, left + 14.0, head_y + 10.0, CONTENT_W - 28.0, 1.0, CARD_EDGE)?;

    // The footer owns the last band of the card; rows stop above it.
    let bottom = TABLE_Y + TABLE_H - 24.0;
    let mut ry = head_y + 16.0;
    for row in ROWS.iter() {
        if ry + ROW_H > bottom {
            break;
        }
        let c = level_color(row.level);
        let ctx_h = row.context.len() as f32 * CTX_LINE_H;
        let block_h = ROW_H + if ctx_h > 0.0 { ctx_h + 8.0 } else { 0.0 };

        // An error block gets a faint wash so the expansion reads as one unit
        // rather than a stray indented paragraph.
        if row.level == LV_E {
            let wash = blend(CARD, LV_ERROR, if ctx_h > 0.0 { 0.09 } else { 0.05 });
            rounded(canvas, left + 12.0, ry - 3.0, CONTENT_W - 24.0, block_h - 2.0, 6.0, wash)?;
        }

        // The stripe: the whole point of the left edge. It spans the expansion
        // too, so an error and its context are visibly one record.
        rounded(canvas, left + 12.0 + COL_STRIPE, ry - 3.0, 3.0, block_h - 2.0, 1.5, c)?;

        let text_y = ry + 14.0;
        text(canvas, row.stamp, left + COL_STAMP, text_y, 12.5, INK_DIM);

        // Level tag: a bordered pill in the level color, fixed left edge so
        // the four-letter and five-letter names still start together.
        let name = level_name(row.level);
        let tw = est_width(canvas, name, 10.0);
        let tag_bg = blend(CARD, c, 0.16);
        rounded(canvas, left + COL_LEVEL, ry + 3.0, tw + 14.0, 15.0, 4.0, tag_bg)?;
        text(canvas, name, left + COL_LEVEL + 7.0, text_y - 1.0, 10.0, c);

        text(canvas, row.service, left + COL_SERVICE, text_y, 12.0, INK_DIM);

        let msg_color = if row.level == LV_E { INK } else if row.level == LV_D { INK_DIM } else { INK };
        text(canvas, row.message, left + COL_MSG, text_y, 13.0, msg_color);

        // Context block: indented under the message column, dimmer, with a
        // vertical rule so it reads as attached detail rather than more rows.
        if ctx_h > 0.0 {
            let cx = left + COL_MSG + 10.0;
            fill(canvas, cx, ry + ROW_H - 4.0, 1.0, ctx_h + 2.0, blend(CARD, LV_ERROR, 0.35))?;
            let mut cy = ry + ROW_H + 8.0;
            for line in row.context.iter() {
                text(canvas, line, cx + 12.0, cy, 11.5, INK_QUIET);
                cy += CTX_LINE_H;
            }
        }

        // Hairline between records, skipped after the last one.
        let next = ry + block_h;
        if next + ROW_H <= bottom {
            fill(canvas, left + COL_STAMP, next - 4.0, CONTENT_W - COL_STAMP - 20.0, 1.0, HAIRLINE)?;
        }
        ry = next;
    }

    // Footer: a tail indicator, centred, so the table has an end rather than
    // stopping mid-air.
    let foot = TABLE_Y + TABLE_H - 14.0;
    text_center(canvas, "streaming -- 8,412 lines in the last 15 min", left + CONTENT_W * 0.5, foot, 10.5, INK_QUIET);
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
        // these to WIDTH/HEIGHT froze the canvas at its design size no matter
        // how the window was resized (K-003).
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
        style: types::Style { width: None, height: None, grow: 1.0, padding: 0.0 },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

// ---- app --------------------------------------------------------------------

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Trace", size) else {
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
        let _ = canvas2d::set_design_size(canvas, gfx::Size { width: WIDTH, height: HEIGHT });

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        let _ = draw(canvas);

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"trace:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        let mut idle = 0u32;
        for _ in 0..MAX_WAIT_ROUNDS {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                Some(types::Event::Resized(_)) => {
                    idle = 0;
                    let _ = draw(canvas);
                }
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
        let _ = out.write(b"trace:ok\n");
        let _ = out.flush();
        0
    }
}

bindings::export!(Component with_types_in bindings);
