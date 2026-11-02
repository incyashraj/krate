//! Trace -- a log and trace viewer for a service, drawn entirely on a canvas.
//!
//! A service bar with a time range and p50/p99/error readouts, a
//! request-volume bar chart with error minutes in red, and under it the real
//! subject -- a log whose rows carry a level stripe, a timestamp, a service
//! and a message. One error row is open to show its context, because that is
//! what you do with an error line; tapping another row with context opens
//! that one instead, and the log scrolls.
//!
//! It lays itself out from the canvas it really has: a desktop or tablet gets
//! a table with fixed columns, a phone one column of two-line records, a
//! phone turned sideways the chart beside the log. Every font follows the
//! person's text size, and each frame tells the screen reader what it shows.
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

use alloc::string::String;
use krate::bindings::krate::io::{args, stdio};
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said, Screen};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The window asked for on a desktop. Only a first guess: every frame is
/// laid out from the canvas the app is actually given.
const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;

const WAIT_ROUND_MILLIS: u32 = 33;

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
const GRID: gfx::Color = gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.055 };
const HAIRLINE: gfx::Color = gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.045 };

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

/// Opaque source-over blend of `c` at `a` onto `base` -- used to pre-flatten
/// tints so translucent rounded shapes never double up at their corners.
fn blend(base: gfx::Color, c: gfx::Color, a: f32) -> gfx::Color {
    rgb(base.r * (1.0 - a) + c.r * a, base.g * (1.0 - a) + c.g * a, base.b * (1.0 - a) + c.b * a)
}

// ---- the demo traffic window ------------------------------------------------

const LV_I: u8 = 0;
const LV_W: u8 = 1;
const LV_E: u8 = 2;
const LV_D: u8 = 3;

/// Each level's color, its tag, and the word a screen reader says for it:
/// "WARN" read aloud is a word nobody uses.
fn level(level: u8) -> (gfx::Color, &'static str, &'static str) {
    match level {
        LV_W => (LV_WARN, "WARN", "warning"),
        LV_E => (LV_ERROR, "ERROR", "error"),
        LV_D => (LV_DEBUG, "DEBUG", "debug"),
        _ => (LV_INFO, "INFO", "info"),
    }
}

/// One log line. `context` is the block that opens under it; empty means the
/// row is a single line and tapping it does nothing.
struct Line {
    stamp: &'static str,
    level: u8,
    service: &'static str,
    message: &'static str,
    context: &'static [&'static str],
}

const ORDER_CTX: [&str; 3] = ["upstream: payments-api.svc.cluster.local:8443", "trace_id: 7f3a91c4e0b28d65  span: checkout.charge", "retry 3/3 exhausted, returning 504 to client"];
const PG_CTX: [&str; 2] = ["host: postgres-primary.db:5432  attempt 2 of 5", "last error: connection reset by peer after 3001ms"];
const LIMIT_CTX: [&str; 2] = ["limit: 600 req/min per key  window: 60s", "client: 203.0.113.42  user-agent: acme-sync/2.4"];
const CARD_CTX: [&str; 2] = ["decline_code: expired_card  amount: 84.00 EUR", "order: 8821  customer: cus_Q2m7  not retried: card error"];

/// The row whose context is open when the app starts: the timeout, because
/// it is the reason anybody opened a log viewer this afternoon.
const FIRST_OPEN: usize = 5;

const N_ROWS: usize = 20;
const ROWS: [Line; N_ROWS] = [
    Line { stamp: "14:32:07.412", level: LV_I, service: "api-gateway", message: "GET /v1/orders 200 18ms", context: &[] },
    Line { stamp: "14:32:07.588", level: LV_I, service: "api-gateway", message: "GET /v1/orders/8821 200 24ms", context: &[] },
    Line { stamp: "14:32:08.031", level: LV_D, service: "auth", message: "token cache hit for tenant acme-prod", context: &[] },
    Line { stamp: "14:32:08.947", level: LV_W, service: "orders", message: "retrying connection to postgres-primary", context: &PG_CTX },
    Line { stamp: "14:32:09.203", level: LV_I, service: "api-gateway", message: "POST /v1/checkout 201 142ms", context: &[] },
    Line { stamp: "14:32:11.664", level: LV_E, service: "checkout", message: "upstream timeout after 5000ms", context: &ORDER_CTX },
    Line { stamp: "14:32:12.010", level: LV_I, service: "api-gateway", message: "GET /v1/health 200 3ms", context: &[] },
    Line { stamp: "14:32:12.774", level: LV_W, service: "api-gateway", message: "rate limit exceeded for key sk_test_4Kf9...", context: &LIMIT_CTX },
    Line { stamp: "14:32:13.126", level: LV_I, service: "orders", message: "PATCH /v1/orders/8821 200 31ms", context: &[] },
    Line { stamp: "14:32:13.905", level: LV_E, service: "payments", message: "charge declined: card_expired (tok_1P9x)", context: &CARD_CTX },
    Line { stamp: "14:32:14.338", level: LV_D, service: "orders", message: "flushed 24 events to analytics buffer", context: &[] },
    Line { stamp: "14:32:15.017", level: LV_I, service: "api-gateway", message: "GET /v1/orders 200 21ms", context: &[] },
    Line { stamp: "14:32:15.402", level: LV_I, service: "orders", message: "GET /v1/orders/8822 200 19ms", context: &[] },
    Line { stamp: "14:32:15.880", level: LV_D, service: "auth", message: "refreshed signing keys (kid 2026-11)", context: &[] },
    Line { stamp: "14:32:16.231", level: LV_I, service: "api-gateway", message: "POST /v1/checkout 201 133ms", context: &[] },
    Line { stamp: "14:32:16.909", level: LV_W, service: "payments", message: "slow response from card network: 2140ms", context: &[] },
    Line { stamp: "14:32:17.455", level: LV_I, service: "api-gateway", message: "GET /v1/orders 200 17ms", context: &[] },
    Line { stamp: "14:32:18.012", level: LV_I, service: "orders", message: "POST /v1/orders 201 48ms", context: &[] },
    Line { stamp: "14:32:18.637", level: LV_D, service: "orders", message: "flushed 19 events to analytics buffer", context: &[] },
    Line { stamp: "14:32:19.104", level: LV_I, service: "api-gateway", message: "GET /v1/health 200 2ms", context: &[] },
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
/// What the chart shows, for the screen reader: a picture of sixty bars
/// means nothing read aloud, its shape does.
const CHART_SAYS: &str = "Request volume, last 15 minutes: 396 to 702 requests a minute, \
    peak 702. Four minutes had server errors, the worst 88 errors at the peak.";

const RANGE: &str = "last 15 min";
/// The highest minute in VOLUME, for the chart's caption.
const PEAK: &str = "peak 702 req / min";
/// Minute ticks under the chart, by bucket.
const TICKS: [(usize, &str); 4] = [(0, "-15m"), (20, "-10m"), (40, "-5m"), (59, "now")];
const STATS: [(&str, &str, gfx::Color); 3] = [("p50", "24ms", INK), ("p99", "310ms", LV_WARN), ("err", "0.4%", LV_ERROR)];

// ---- tiny drawing + text helpers -------------------------------------------

type Drawn = Result<(), gfx::GfxError>;

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Drawn {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

fn disc(canvas: u64, x: f32, y: f32, r: f32, c: gfx::Color) -> Drawn {
    canvas2d::fill_circle(canvas, gfx::Point { x, y }, r, c)
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, c);
}

/// Rendered width of a string, measured by the host that draws it: the face
/// is proportional, so a guess from the character count misaligns.
fn width_of(canvas: u64, s: &str, size: f32) -> f32 {
    canvas2d::measure_text(canvas, s, size).map(|m| m.width).unwrap_or(0.0)
}

/// A filled rounded rectangle. A card-sized one is three rects and four
/// corner discs: the same look at a fraction of a phone's time, since only
/// the corners need antialiasing. Every color here is opaque, so the
/// overlaps never show.
fn rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Drawn {
    let r = r.min(w * 0.5).min(h * 0.5).max(0.0);
    if w <= 0.0 || h <= 0.0 {
        return Ok(());
    }
    if r < 0.5 || w * h < 6000.0 {
        let radii = gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r };
        return canvas2d::fill_round_rect(canvas, gfx::Rect { x, y, width: w, height: h }, radii, c);
    }
    fill(canvas, x + r, y, w - 2.0 * r, h, c)?;
    fill(canvas, x, y + r, r, h - 2.0 * r, c)?;
    fill(canvas, x + w - r, y + r, r, h - 2.0 * r, c)?;
    for (cx, cy) in [(x + r, y + r), (x + w - r, y + r), (x + r, y + h - r), (x + w - r, y + h - r)] {
        disc(canvas, cx, cy, r, c)?;
    }
    Ok(())
}

/// A card: the panel fill with a hairline highlight along its top edge. (A
/// blurred shadow under it cost a phone more than the rest of the frame
/// together, and on this ground it was barely visible.)
fn card(canvas: u64, r: Rect) -> Drawn {
    rounded(canvas, r.x, r.y, r.w, r.h, 14.0, CARD)?;
    fill(canvas, r.x + 14.0, r.y, r.w - 28.0, 1.0, CARD_EDGE)
}

/// How much of a string fits a room: its first `len` bytes, `w` wide, and
/// whether an ellipsis follows. A line running into the next column reads
/// as a broken app; one cut short reads as "there is more".
#[derive(Clone, Copy, Default)]
struct Cut {
    len: usize,
    w: f32,
    more: bool,
}

/// All of `s` when it fits `room`, else its longest leading part that fits
/// with an ellipsis after it. Every string here is ASCII.
fn cut(canvas: u64, s: &str, size: f32, room: f32) -> Cut {
    let full = width_of(canvas, s, size);
    if full <= room || room <= 0.0 {
        return if full <= room { Cut { len: s.len(), w: full, more: false } } else { Cut::default() };
    }
    let ell = width_of(canvas, "…", size);
    let (mut lo, mut hi) = (0usize, s.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if width_of(canvas, s.get(..mid).unwrap_or(""), size) + ell <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let part = s.get(..lo).unwrap_or("").trim_end();
    Cut { len: part.len(), w: width_of(canvas, part, size), more: true }
}

fn draw_cut(canvas: u64, s: &str, c: Cut, x: f32, y: f32, size: f32, color: gfx::Color) {
    if c.len > 0 {
        text(canvas, s.get(..c.len).unwrap_or(""), x, y, size, color);
    }
    if c.more {
        text(canvas, "…", x + c.w, y, size, color);
    }
}

fn fit(canvas: u64, s: &str, x: f32, y: f32, size: f32, color: gfx::Color, room: f32) {
    draw_cut(canvas, s, cut(canvas, s, size, room), x, y, size, color);
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

/// Join `parts` into `buf` -- a phrase for the screen reader, with no
/// `format!`. Anything past the buffer is dropped.
fn join<'a>(buf: &'a mut [u8; 192], parts: &[&str]) -> &'a str {
    let mut n = 0usize;
    for &byte in parts.iter().flat_map(|part| part.as_bytes()) {
        if let Some(slot) = buf.get_mut(n) {
            *slot = byte;
            n += 1;
        }
    }
    core::str::from_utf8(buf.get(..n).unwrap_or(&[])).unwrap_or("")
}

// ---- layout -----------------------------------------------------------------

/// Log table columns, as x offsets from the card's left edge at the default
/// text size: fixed columns make twenty rows read as a table. They grow with
/// the text, so a larger size never runs a timestamp into its tag.
const COL_STAMP: f32 = 20.0;
const COL_LEVEL: f32 = 132.0;
const COL_SERVICE: f32 = 200.0;
const COL_MSG: f32 = 316.0;
/// The least a message column may be. Below it a message would be cut to a
/// word or two, so the table turns into two-line rows instead.
const MSG_MIN: f32 = 220.0;
/// Row rhythm at the default text size: a table row, a phone's two-line
/// row, and one line of an opened context block.
const ROW_H: f32 = 28.0;
const ROW_H_NARROW: f32 = 50.0;
const CTX_LINE_H: f32 = 17.0;

/// What the viewer is doing: which row's context is open, how far the log
/// is scrolled (in pixels, so a wheel glides rather than jumping rows), and
/// which row a press started on.
struct View {
    open: Option<usize>,
    scroll: f32,
    down: Option<usize>,
}

/// One row's text fitted to its room: the tag's width, where the service
/// goes, and how much of the service, message and context lines is shown.
#[derive(Clone, Copy, Default)]
struct RowFit {
    tag_w: f32,
    svc_x: f32,
    service: Cut,
    message: Cut,
    context: [Cut; 3],
}

/// Where everything goes, from the canvas the app really has, the covered
/// edges and the person's text size `t`. Drawing and hit-testing read this
/// one value, so a tap opens the row drawn under the finger. It is worked out
/// when a size changes, not every frame: a phone runs this code on an
/// interpreter, and a scroll must not re-measure every string.
#[derive(Clone, Default)]
struct Layout {
    key: (u32, u32, u32),
    w: f32,
    h: f32,
    t: f32,
    /// Inside the covered edges and the margin.
    area: Rect,
    /// Table columns, or one column of two-line rows.
    wide: bool,
    /// Little height (a phone turned sideways): no column heads or footer.
    short: bool,
    /// The header flows onto as many rows as it needs; the stats are a
    /// right-aligned block, or `spread` as three columns when that is too wide.
    name_base: f32,
    name_w: f32,
    prod: bool,
    chip_at: (f32, f32),
    chip_w: f32,
    stats_w: f32,
    stat_x: [(f32, f32); 3],
    stats_base: f32,
    spread: bool,
    /// The chart: buckets per bar, bar pitch and width, and the labels (a NaN
    /// tick has no room).
    chart: Rect,
    plot_h: f32,
    group: usize,
    slot: f32,
    bar_w: f32,
    peak_x: f32,
    caption: Cut,
    ticks: [f32; 4],
    /// The log: its card, header labels, and the band rows scroll in.
    table: Rect,
    count_x: f32,
    unit_x: f32,
    legend_x: Option<[f32; 3]>,
    list: Rect,
    row_h: f32,
    ctx_h: f32,
    msg_x: f32,
    ctx_x: f32,
    rows: [RowFit; N_ROWS],
}

impl Layout {
    fn pt(&self, points: f32) -> f32 {
        points * self.t
    }

    /// The height of row `i`, with its context block when it is the open one.
    fn block_h(&self, i: usize, open: Option<usize>) -> f32 {
        let n = ROWS.get(i).filter(|_| open == Some(i)).map_or(0, |row| row.context.len());
        self.row_h + if n > 0 { n as f32 * self.ctx_h + self.pt(8.0) } else { 0.0 }
    }

    /// Every row's top edge from the start of the log, and the log's end.
    fn tops(&self, open: Option<usize>) -> [f32; N_ROWS + 1] {
        let mut tops = [0.0f32; N_ROWS + 1];
        for i in 0..N_ROWS {
            let next = tops.get(i).copied().unwrap_or(0.0) + self.block_h(i, open);
            if let Some(slot) = tops.get_mut(i + 1) {
                *slot = next;
            }
        }
        tops
    }

    fn max_scroll(&self, open: Option<usize>) -> f32 {
        (self.tops(open).last().copied().unwrap_or(0.0) - self.list.h).max(0.0)
    }

    /// The list band cut at the bottom to the last whole record: a line sliced
    /// through its letters reads as a broken app, not as "more below".
    fn band(&self, view: &View) -> Rect {
        let (top, bottom) = (self.list.y, self.list.bottom());
        let ends = self.tops(view.open).map(|y| top - view.scroll + y);
        let edge = ends.iter().copied().filter(|&y| y > top && y <= bottom + 0.5).fold(f32::MIN, f32::max);
        Rect::new(self.list.x, top, self.list.w, if edge > top { edge - top } else { self.list.h })
    }

    /// The row under a point, from the same numbers the frame was drawn with.
    fn row_at(&self, view: &View, x: f32, y: f32) -> Option<usize> {
        if !self.band(view).contains(x, y) {
            return None;
        }
        let (tops, at) = (self.tops(view.open), y - self.list.y + view.scroll);
        (0..N_ROWS).find(|&i| at >= tops.get(i).copied().unwrap_or(0.0) && at < tops.get(i + 1).copied().unwrap_or(0.0))
    }
}

/// Keep `cache` the layout for the canvas as it is now, working it out again
/// only when its size or the text size changed. True when it did.
fn current(canvas: u64, win: u64, cache: &mut Option<Layout>) -> bool {
    let size = canvas2d::canvas_size(canvas).unwrap_or(gfx::Size { width: WIDTH, height: HEIGHT });
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let t = Look::now().text_scale.clamp(0.8, 2.4);
    let key = (w.to_bits(), h.to_bits(), t.to_bits());
    if cache.as_ref().is_some_and(|l| l.key == key) {
        return false;
    }
    *cache = Some(layout(canvas, win, w, h, t));
    true
}

fn layout(canvas: u64, win: u64, w: f32, h: f32, t: f32) -> Layout {
    // The notch, the home bar and the close button Krate draws on a phone
    // cover the edges; text stays inside them.
    let (st, sr, sb, sl) = Screen::of(win).safe;
    let short = h < 500.0;
    let margin = if w < 600.0 || short { 16.0 } else { 28.0 };
    let area = Rect::new(0.0, 0.0, w, h).inset(st, sr, sb, sl).pad(margin);
    // A phone turned sideways has width to spare and no height: the chart
    // goes beside the log instead of above it, or the log would get two rows.
    let side = short && area.w >= 600.0 * t;
    let chart_w = if side { area.w * 0.36 } else { area.w };
    let table_w = if side { area.w - chart_w - 14.0 * t } else { area.w };
    let wide = table_w >= (COL_MSG + MSG_MIN) * t;
    let mut l = Layout { key: (w.to_bits(), h.to_bits(), t.to_bits()), w, h, t, area, wide, short, ..Layout::default() };

    // Each header item goes on the first row with room for it, so a desktop
    // gets one line and a phone two or three.
    l.name_base = area.y + 34.0 * t;
    l.name_w = width_of(canvas, "api-gateway", 22.0 * t);
    let prod_w = 12.0 * t + width_of(canvas, "production", 12.5 * t);
    let mut x = area.x + 20.0 * t + l.name_w;
    l.prod = x + prod_w <= area.right();
    if l.prod {
        x += prod_w;
    }
    l.chip_w = width_of(canvas, RANGE, 12.5 * t) + 30.0 * t;
    let mut base = l.name_base;
    if x + 16.0 * t + l.chip_w <= area.right() {
        l.chip_at = (x + 16.0 * t, base);
    } else {
        base += 40.0 * t;
        l.chip_at = (area.x, base);
    }
    let mut right = area.right();
    for (&(label, value, _), at) in STATS.iter().zip(l.stat_x.iter_mut()).rev() {
        let (lw, vw) = (width_of(canvas, label, 11.5 * t), width_of(canvas, value, 17.0 * t));
        *at = (right - lw, right - vw);
        right -= lw.max(vw) + 34.0 * t;
    }
    l.stats_w = area.right() - right - 34.0 * t;
    l.spread = l.stats_w > area.w;
    if l.spread || l.chip_at.0 + l.chip_w + 24.0 * t + l.stats_w > area.right() {
        base += 40.0 * t;
    }
    l.stats_base = base;

    // The bars take the height the log does not need, up to a limit, so a
    // tall tablet gets a taller chart, not an empty band.
    let (cap_h, axis_h) = (36.0 * t, 26.0 * t);
    let plot_min = t * if short { 40.0 } else if wide { 84.0 } else { 60.0 };
    let chart_y = base + 26.0 * t;
    let (top, foot) = (t * if wide && !short { 66.0 } else { 46.0 }, t * if short { 8.0 } else { 28.0 });
    l.row_h = t * if wide { ROW_H } else { ROW_H_NARROW };
    l.ctx_h = CTX_LINE_H * t;
    // Sized for the largest context block, so opening a row never moves the
    // chart.
    let most = (0..N_ROWS).map(|i| l.block_h(i, Some(i)) - l.row_h).fold(0.0, f32::max);
    let slack = area.bottom() - chart_y - (cap_h + plot_min + axis_h) - 14.0 * t - (top + foot + most + N_ROWS as f32 * l.row_h);
    l.plot_h = if side { (area.bottom() - chart_y - cap_h - axis_h).max(plot_min) } else { plot_min + slack.clamp(0.0, 140.0 * t) };
    l.chart = Rect::new(area.x, chart_y, chart_w, cap_h + l.plot_h + axis_h);
    let table_y = if side { chart_y } else { l.chart.bottom() + 14.0 * t };
    l.table = Rect::new(area.right() - table_w, table_y, table_w, (area.bottom() - table_y).max(top + foot + l.row_h));
    l.list = Rect::new(l.table.x, table_y + top, table_w, (l.table.h - top - foot).max(l.row_h));

    // Where a bar would be under six pixels (a phone), three buckets share
    // one -- the busiest, red if any had errors -- so it reads as bars.
    let (inner_x, inner_w) = (l.chart.x + 20.0 * t, l.chart.w - 40.0 * t);
    l.group = if inner_w < 6.0 * BUCKETS as f32 { 3 } else { 1 };
    l.slot = inner_w / (BUCKETS / l.group) as f32;
    l.bar_w = l.slot - (l.slot * 0.3).min(2.2);
    // The peak readout sits in the caption line, right-aligned, so the
    // minute ticks below have the whole axis.
    let peak_w = width_of(canvas, PEAK, 11.0 * t);
    l.peak_x = inner_x + inner_w - peak_w;
    l.caption = cut(canvas, "REQUEST VOLUME", 11.0 * t, inner_w - peak_w - 12.0 * t);
    // End ticks align to the plot's edges so neither hangs off; a tick that
    // would touch the one before is left out.
    let mut last = f32::MIN;
    for (at, &(idx, label)) in l.ticks.iter_mut().zip(TICKS.iter()) {
        let w = width_of(canvas, label, 10.0 * t);
        let lx = (inner_x + (idx / l.group) as f32 * l.slot + (l.bar_w - w) * 0.5).max(inner_x).min(inner_x + inner_w - w);
        *at = if lx >= last + 10.0 * t { lx } else { f32::NAN };
        last = if at.is_nan() { last } else { lx + w };
    }

    // The log's header, then every row's text fitted to its room, once.
    let (tx, right) = (l.table.x, l.table.right() - 20.0 * t);
    l.unit_x = right - width_of(canvas, " lines", 12.0 * t);
    l.count_x = l.unit_x - width_of(canvas, "8,412", 12.5 * t);
    let mut lx = tx + COL_STAMP * t + width_of(canvas, "Logs", 17.0 * t) + 24.0 * t;
    let mut xs = [0.0f32; 3];
    for (x, lv) in xs.iter_mut().zip([LV_I, LV_W, LV_E]) {
        *x = lx;
        lx += 27.0 * t + width_of(canvas, level(lv).1, 10.5 * t);
    }
    l.legend_x = Some(xs).filter(|_| lx <= l.count_x);
    l.msg_x = tx + t * if wide { COL_MSG } else { COL_STAMP };
    l.ctx_x = tx + t * if wide { COL_MSG + 10.0 } else { COL_STAMP + 4.0 };
    for (fit, row) in l.rows.iter_mut().zip(ROWS.iter()) {
        // A row that opens keeps room at its end for the plus sign.
        let msg_right = if row.context.is_empty() { right } else { right - 40.0 * t };
        fit.tag_w = width_of(canvas, level(row.level).1, 10.0 * t) + 14.0 * t;
        if wide {
            fit.svc_x = tx + COL_SERVICE * t;
            fit.service = cut(canvas, row.service, 12.0 * t, (COL_MSG - COL_SERVICE - 10.0) * t);
            fit.message = cut(canvas, row.message, 13.0 * t, msg_right - l.msg_x);
        } else {
            // The service ends the first line when it fits there, and is
            // left to the screen reader's phrase when it does not.
            let after = l.msg_x + fit.tag_w + 22.0 * t + width_of(canvas, row.stamp, 12.5 * t);
            let svc = cut(canvas, row.service, 12.0 * t, f32::INFINITY);
            if after + svc.w <= msg_right {
                (fit.svc_x, fit.service) = (msg_right - svc.w, svc);
            }
            fit.message = cut(canvas, row.message, 14.0 * t, right - l.msg_x);
        }
        for (c, line) in fit.context.iter_mut().zip(row.context.iter()) {
            *c = cut(canvas, line, 11.5 * t, right - l.ctx_x - 12.0 * t);
        }
    }
    l
}

// ---- drawing ----------------------------------------------------------------

/// Draw a frame, then tell the screen reader what it shows. A `fresh` layout
/// draws every row, the hidden ones clipped away: the host prepares a string
/// the first time it draws it, and doing that now rather than as each row
/// scrolls in keeps a phone's scroll smooth.
fn draw(canvas: u64, win: u64, l: &Layout, view: &mut View, fresh: bool) -> Drawn {
    // What this frame can show decides how far scrolling may go; a window
    // made taller pulls the last rows back into view.
    view.scroll = view.scroll.clamp(0.0, l.max_scroll(view.open));
    canvas2d::linear_gradient(canvas, gfx::Rect { x: 0.0, y: 0.0, width: l.w, height: l.h }, BG_TOP, BG_BOT)?;
    draw_service_bar(canvas, l)?;
    draw_volume(canvas, l)?;
    draw_table(canvas, l, view, fresh)?;
    canvas2d::present(canvas)?;
    // Told after the frame is up, so the person sees it a little sooner.
    say(l, view).send(win);
    Ok(())
}

fn draw_service_bar(canvas: u64, l: &Layout) -> Drawn {
    let (a, t, base) = (l.area, l.t, l.name_base);
    let small = l.pt(12.5);

    // Live dot + service name: the one thing a reader identifies the screen by.
    disc(canvas, a.x + 6.0 * t, base - 7.0 * t, 4.5 * t, rgb(0.239, 0.839, 0.549))?;
    text(canvas, "api-gateway", a.x + 20.0 * t, base, l.pt(22.0), INK);
    if l.prod {
        text(canvas, "production", a.x + 32.0 * t + l.name_w, base - 1.0, small, INK_QUIET);
    }

    // Time range chip, right after the name when it fits so the numbers
    // below have a stated window rather than floating free.
    let (rx, rb) = l.chip_at;
    rounded(canvas, rx, rb - 20.0 * t, l.chip_w, 26.0 * t, 13.0 * t, CHIP_EDGE)?;
    rounded(canvas, rx + 1.0, rb - 19.0 * t, l.chip_w - 2.0, 26.0 * t - 2.0, 13.0 * t - 1.0, CHIP)?;
    // A tiny clock glyph: ring plus two hands.
    disc(canvas, rx + 15.0 * t, rb - 7.0 * t, 5.0 * t, INK_QUIET)?;
    disc(canvas, rx + 15.0 * t, rb - 7.0 * t, 3.6 * t, CHIP)?;
    fill(canvas, rx + 14.4 * t, rb - 10.0 * t, 1.2 * t, 3.4 * t, INK_DIM)?;
    fill(canvas, rx + 15.0 * t, rb - 7.6 * t, 3.0 * t, 1.2 * t, INK_DIM)?;
    text(canvas, RANGE, rx + 24.0 * t, rb - 2.0 * t, small, INK_DIM);

    // Three stat readouts, right-aligned as a block so their values form a
    // column the eye can compare against.
    let (label_s, value_s, sb) = (l.pt(11.5), l.pt(17.0), l.stats_base);
    let col = a.w / STATS.len() as f32;
    for (i, (&(label, value, color), &(lx, vx))) in STATS.iter().zip(l.stat_x.iter()).enumerate() {
        if l.spread {
            let x = a.x + i as f32 * col;
            fit(canvas, label, x, sb - 18.0 * t, label_s, INK_QUIET, col - 8.0);
            fit(canvas, value, x, sb + 2.0 * t, value_s, color, col - 8.0);
        } else {
            text(canvas, label, lx, sb - 18.0 * t, label_s, INK_QUIET);
            text(canvas, value, vx, sb + 2.0 * t, value_s, color);
        }
    }
    Ok(())
}

fn draw_volume(canvas: u64, l: &Layout) -> Drawn {
    let (c, t) = (l.chart, l.t);
    card(canvas, c)?;
    let (inner_x, inner_w) = (c.x + 20.0 * t, c.w - 40.0 * t);
    text(canvas, PEAK, l.peak_x, c.y + 25.0 * t, l.pt(11.0), INK_DIM);
    draw_cut(canvas, "REQUEST VOLUME", l.caption, inner_x, c.y + 25.0 * t, l.pt(11.0), INK_QUIET);

    // Two gridlines plus the baseline, and the bars scaled to the peak,
    // rounded up so the tallest bar does not touch the top.
    let base_y = c.bottom() - 26.0 * t;
    for step in [0.0f32, 0.5, 1.0] {
        fill(canvas, inner_x, base_y - l.plot_h * step, inner_w, 1.0, GRID)?;
    }
    let max = VOLUME.iter().copied().max().unwrap_or(1).max(1) as f32 * 1.12;
    for b in 0..BUCKETS / l.group {
        let (mut v, mut errs) = (0u16, 0u16);
        for i in b * l.group..(b + 1) * l.group {
            v = v.max(VOLUME.get(i).copied().unwrap_or(0));
            errs = errs.saturating_add(ERRORS.get(i).copied().unwrap_or(0));
        }
        let bh = (l.plot_h * (v as f32 / max)).max(2.0);
        let x = inner_x + b as f32 * l.slot;
        // An error minute is one solid red column with a lit top: partial
        // stacking read as a rendering artifact at this bar width.
        fill(canvas, x, base_y - bh, l.bar_w, bh, if errs > 0 { LV_ERROR } else { BAR_OK })?;
        if errs > 0 {
            fill(canvas, x, base_y - bh, l.bar_w, 2.0, rgb(1.0, 0.545, 0.541))?;
        }
    }
    // Minute ticks under the axis, so the window reads as fifteen minutes
    // rather than sixty anonymous columns.
    for (&x, &(_, label)) in l.ticks.iter().zip(TICKS.iter()).filter(|(x, _)| !x.is_nan()) {
        text(canvas, label, x, base_y + 16.0 * t, l.pt(10.0), INK_QUIET);
    }
    Ok(())
}

fn draw_table(canvas: u64, l: &Layout, view: &View, fresh: bool) -> Drawn {
    let (tb, t) = (l.table, l.t);
    card(canvas, tb)?;
    let head = tb.y + 30.0 * t;

    // Title, live count, and between them the legend stating the stripe
    // colors -- all three or none, since a missing level reads as absent.
    text(canvas, "Logs", tb.x + COL_STAMP * t, head, l.pt(17.0), INK);
    text(canvas, "8,412", l.count_x, head - 1.0, l.pt(12.5), INK_DIM);
    text(canvas, " lines", l.unit_x, head - 1.0, l.pt(12.0), INK_QUIET);
    for (&x, lv) in l.legend_x.iter().flatten().zip([LV_I, LV_W, LV_E]) {
        let (c, name, _) = level(lv);
        fill(canvas, x, head - 10.0 * t, 3.0 * t, 11.0 * t, c)?;
        text(canvas, name, x + 9.0 * t, head - 1.0, l.pt(10.5), INK_QUIET);
    }

    // Column heads sit on their own rule, and every row below uses the same
    // x offsets -- that repetition is what makes it look like a table.
    if l.wide && !l.short {
        let hy = tb.y + 50.0 * t;
        for (label, col) in [("TIME", COL_STAMP), ("LEVEL", COL_LEVEL), ("SERVICE", COL_SERVICE), ("MESSAGE", COL_MSG)] {
            text(canvas, label, tb.x + col * t, hy, l.pt(10.0), INK_QUIET);
        }
        fill(canvas, tb.x + 14.0, hy + 10.0 * t, tb.w - 28.0, 1.0, CARD_EDGE)?;
    }

    // Rows scroll inside the band and are cut at its edges, so a row half
    // scrolled away never draws over the heads or the footer.
    let band = l.band(view);
    let tops = l.tops(view.open);
    let _ = canvas2d::set_clip(canvas, band.x, band.y, band.w, band.h);
    for (i, (row, fit)) in ROWS.iter().zip(l.rows.iter()).enumerate() {
        let top = band.y - view.scroll + tops.get(i).copied().unwrap_or(0.0);
        let bh = l.block_h(i, view.open);
        if fresh || (top + bh > band.y && top < band.bottom()) {
            draw_row(canvas, l, row, fit, top, bh, view.open == Some(i))?;
        }
    }
    let _ = canvas2d::clear_clip(canvas);

    // A thin scroll thumb when the log is longer than its band, so the
    // person can see there is more below.
    let (list, content) = (l.list, tops.last().copied().unwrap_or(0.0));
    if content > list.h + 0.5 {
        let th = (list.h * list.h / content).max(24.0);
        let ty = list.y + (list.h - th) * (view.scroll / (content - list.h));
        rounded(canvas, tb.right() - 8.0, ty, 3.0, th, 1.5, CHIP_EDGE)?;
    }

    // Footer: a tail indicator, centred, so the table has an end rather than
    // stopping mid-air.
    if !l.short {
        let (fs, room) = (l.pt(10.5), tb.w - 40.0 * t);
        let foot = cut(canvas, FOOTER, fs, room);
        draw_cut(canvas, FOOTER, foot, tb.x + (tb.w - foot.w.min(room)) * 0.5, tb.bottom() - 12.0 * t, fs, INK_QUIET);
    }
    Ok(())
}

const FOOTER: &str = "streaming -- 8,412 lines in the last 15 min";

/// One log record at `top`, `bh` tall: on a wide screen a table row, on a
/// phone the level and time on one line with the service at its end, and
/// the message under them.
fn draw_row(canvas: u64, l: &Layout, row: &Line, fit: &RowFit, top: f32, bh: f32, open: bool) -> Drawn {
    let (t, left, right) = (l.t, l.table.x, l.table.right());
    let (c, name, _) = level(row.level);

    // An error block gets a faint wash so its expansion reads as one unit.
    // Square: at this tint a rounded corner is invisible and costs a phone.
    if row.level == LV_E {
        fill(canvas, left + 12.0, top, l.table.w - 24.0, bh - 2.0 * t, blend(CARD, LV_ERROR, if open { 0.09 } else { 0.05 }))?;
    }
    // The stripe: the whole point of the left edge. It spans the expansion
    // too, so a line and its context are visibly one record.
    fill(canvas, left + 12.0, top, 3.0, bh - 2.0 * t, c)?;
    // A row that opens shows a plus, and a minus while it is open: the only
    // hint that there is more to it than one line.
    if !row.context.is_empty() {
        let (mx, my) = (right - 30.0 * t, top + 12.0 * t);
        fill(canvas, mx, my, 9.0 * t, 1.5 * t, INK_DIM)?;
        if !open {
            fill(canvas, mx + 3.75 * t, my - 3.75 * t, 1.5 * t, 9.0 * t, INK_DIM)?;
        }
    }

    // Level tag: a tinted pill in the level color, fixed left edge so the
    // four-letter and five-letter names still start together.
    let line1 = top + 17.0 * t;
    let tag_x = if l.wide { left + COL_LEVEL * t } else { l.msg_x };
    rounded(canvas, tag_x, top + 6.0 * t, fit.tag_w, 15.0 * t, 4.0, blend(CARD, c, 0.16))?;
    text(canvas, name, tag_x + 7.0 * t, line1, l.pt(10.0), c);
    let stamp_x = if l.wide { left + COL_STAMP * t } else { l.msg_x + fit.tag_w + 10.0 * t };
    text(canvas, row.stamp, stamp_x, line1, l.pt(12.5), INK_DIM);
    draw_cut(canvas, row.service, fit.service, fit.svc_x, line1, l.pt(12.0), if l.wide { INK_DIM } else { INK_QUIET });
    let (msg_y, msg_s) = if l.wide { (line1, l.pt(13.0)) } else { (top + 39.0 * t, l.pt(14.0)) };
    draw_cut(canvas, row.message, fit.message, l.msg_x, msg_y, msg_s, if row.level == LV_D { INK_DIM } else { INK });

    // Context block: indented under the message, dimmer, with a vertical
    // rule so it reads as attached detail rather than more rows.
    if open && !row.context.is_empty() {
        fill(canvas, l.ctx_x, top + l.row_h - 4.0 * t, 1.0, row.context.len() as f32 * l.ctx_h + 2.0 * t, blend(CARD, LV_ERROR, 0.35))?;
        for (k, (line, c)) in row.context.iter().zip(fit.context.iter()).enumerate() {
            draw_cut(canvas, line, *c, l.ctx_x + 12.0 * t, top + l.row_h + 11.0 * t + k as f32 * l.ctx_h, l.pt(11.5), INK_QUIET);
        }
    }
    // A hairline under each table record; on a phone the stripes already
    // part the records, and each line drawn there is scroll time.
    if l.wide {
        fill(canvas, left + COL_STAMP * t, top + bh - 1.0 * t, l.table.w - COL_STAMP * t - 20.0, 1.0, HAIRLINE)?;
    }
    Ok(())
}

/// What the frame shows, for the screen reader, with the rects it was drawn
/// in: the service and its numbers, the chart as a sentence, and each log
/// line in view as one phrase -- a button when it opens.
fn say(l: &Layout, view: &View) -> Said {
    let (a, t, tb) = (l.area, l.t, l.table);
    let mut said = Said::new();
    let (name, (rx, rb), sb) = (l.pt(22.0), l.chip_at, l.stats_base);
    said.heading(Rect::new(a.x, l.name_base - name, l.name_w + 40.0 * t, name * 1.3), "api-gateway, production");
    said.text(Rect::new(rx, rb - 20.0 * t, l.chip_w, 26.0 * t), "last 15 minutes");
    let sx = if l.spread { a.x } else { a.right() - l.stats_w };
    said.text(Rect::new(sx, sb - 30.0 * t, a.right() - sx, 36.0 * t), "p50 24 milliseconds, p99 310 milliseconds, error rate 0.4 percent");
    said.image(l.chart, CHART_SAYS);
    said.heading(Rect::new(tb.x, tb.y + 8.0 * t, 80.0 * t, 30.0 * t), "Logs");
    said.text(Rect::new(l.count_x, tb.y + 8.0 * t, tb.right() - l.count_x, 30.0 * t), "8,412 lines");

    let (band, tops) = (l.band(view), l.tops(view.open));
    for (i, row) in ROWS.iter().enumerate() {
        let top = band.y - view.scroll + tops.get(i).copied().unwrap_or(0.0);
        let bottom = top + l.block_h(i, view.open);
        if bottom <= band.y || top >= band.bottom() {
            continue;
        }
        let shown = Rect::new(band.x, top.max(band.y), band.w, bottom.min(band.bottom()) - top.max(band.y));
        let mut buf = [0u8; 192];
        let phrase = join(&mut buf, &[level(row.level).2, ", ", row.service, ", ", row.stamp, ": ", row.message]);
        if row.context.is_empty() {
            said.text(shown, phrase);
        } else {
            said.button(shown, phrase);
        }
        for line in row.context.iter().filter(|_| view.open == Some(i)) {
            said.text(shown, line);
        }
    }
    said
}

// ---- input ------------------------------------------------------------------

/// What one event asks of the loop.
enum Flow {
    Close,
    Redraw,
    Same,
}

fn handle(ev: types::Event, l: &Layout, win: u64, view: &mut View) -> Flow {
    match ev {
        types::Event::CloseRequested(id) if id == win => Flow::Close,
        // A phone turned, a window narrowed, the text size or the theme
        // changed: lay everything out again.
        types::Event::Resized(_) | types::Event::ThemeChanged(_) => Flow::Redraw,
        // A press and a release on the same row with context opens it (or
        // closes it), so a drag that starts on one row does not.
        types::Event::Pointer(p) => {
            let at = l.row_at(view, p.x, p.y);
            if p.pressed {
                view.down = at;
                return Flow::Same;
            }
            let pressed = view.down.take();
            let Some(i) = at.filter(|&i| Some(i) == pressed && ROWS.get(i).is_some_and(|row| !row.context.is_empty())) else {
                return Flow::Same;
            };
            view.open = if view.open == Some(i) { None } else { Some(i) };
            // Bring an opened block fully into view: context that opens below
            // the fold reads as a tap that did nothing.
            let top = l.tops(view.open).get(i).copied().unwrap_or(0.0);
            let below = top + l.block_h(i, view.open) - (view.scroll + l.list.h);
            if below > 0.0 {
                view.scroll += below.min(top - view.scroll);
            }
            Flow::Redraw
        }
        // dy is in logical pixels and positive scrolls down. Ctrl means zoom
        // on every desktop, so a ctrl-wheel is left alone.
        types::Event::Wheel(w) if !w.modifiers.control => {
            view.scroll += w.dy;
            Flow::Redraw
        }
        types::Event::Key(k) if k.pressed => {
            let (page, end) = ((l.list.h - l.row_h).max(l.row_h), l.max_scroll(view.open) + 1.0);
            view.scroll += match k.key.as_bytes() {
                b"ArrowDown" => l.row_h,
                b"ArrowUp" => -l.row_h,
                b"PageDown" | b" " => page,
                b"PageUp" => -page,
                b"Home" => -end,
                b"End" => end,
                _ => return Flow::Same,
            };
            Flow::Redraw
        }
        _ => Flow::Same,
    }
}

// ---- app --------------------------------------------------------------------

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind, role: Option<String>) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role,
        // No fixed size, and grow so the canvas fills the window. Pinning
        // these to WIDTH/HEIGHT froze the canvas at its design size no matter
        // how the window was resized (K-003).
        style: types::Style { width: None, height: None, grow: 1.0, padding: 0.0, text: None, place: None, box_: None },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Trace", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        let root = node(ROOT_ID, None, types::WidgetKind::Stack, None);
        let surface = node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas, Some(pure_string("canvas")));
        if tree::set_root(win, &root).is_err() || tree::upsert_node(win, &surface).is_err() {
            let _ = window::close(win);
            return 32;
        }
        let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
            let _ = window::close(win);
            return 33;
        };
        let raw = args::raw();
        let quick = raw.as_bytes().split(|byte| *byte == b'\n').next().is_some_and(|first| first == b"quick");

        let mut view = View { open: Some(FIRST_OPEN), scroll: 0.0, down: None };
        let mut cache: Option<Layout> = None;
        let _ = current(canvas, win, &mut cache);
        if let Some(l) = cache.as_ref() {
            let _ = draw(canvas, win, l, &mut view, true);
        }

        // A real session ends when the person closes the window (K-092).
        // Everything queued is taken before drawing, so a fast scroll on a
        // phone never falls behind the finger.
        if !quick {
            'session: loop {
                let (mut redraw, mut fresh) = (false, false);
                let mut next = events::wait(Some(WAIT_ROUND_MILLIS));
                while let Some(ev) = next {
                    fresh |= current(canvas, win, &mut cache);
                    let Some(l) = cache.as_ref() else { break 'session };
                    match handle(ev, l, win, &mut view) {
                        Flow::Close => break 'session,
                        Flow::Redraw => redraw = true,
                        Flow::Same => {}
                    }
                    next = events::poll();
                }
                if let Some(l) = cache.as_ref().filter(|_| redraw || fresh) {
                    let _ = draw(canvas, win, l, &mut view, fresh);
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

krate::export!(Component);
