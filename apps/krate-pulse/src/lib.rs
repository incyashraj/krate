//! Pulse -- a personal finance dashboard drawn entirely on a canvas.
//!
//! The hero layout: a bold balance headline with a monthly delta chip, a
//! 30-day spending area chart with a highlighted "today" point, a category
//! breakdown with share bars, and a recent-transactions list with colored
//! merchant avatars. Tapping a category filters the list; the selection is
//! remembered in the key-value store. All data is a seeded local demo ledger --
//! honest about being a demo, real about everything it does with it.
//!
//! It is laid out from the canvas it really has, every frame. A desktop
//! window puts the balance, chart and recent list on the left and the
//! categories and insight on the right. A tablet stacks the chart over
//! categories and insight side by side. A phone gets one column that
//! scrolls, with rows a finger can hit. Colours follow light and dark mode,
//! every font size follows the person's text size, and each frame tells the
//! screen reader what it shows.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler. All state is fixed-size; money is formatted by
//! hand; no `format!`, `unwrap`, or panicking index anywhere.

#![no_std]

extern crate alloc;

use alloc::string::String;
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::store::kv as store_kv;
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said, Screen};
use krate::math::F32Ext as _;
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The window asked for on a desktop. Only a first size: every frame is laid
/// out from the canvas the app really has.
const WIDTH: f32 = 1080.0;
const HEIGHT: f32 = 700.0;

const WAIT_ROUND_MILLIS: u32 = 100;

const SELECT_KEY: &str = "selected-category";

// ---- colours, for light and dark ---------------------------------------------

#[derive(Clone, Copy)]
struct Theme {
    dark: bool,
    bg_top: gfx::Color,
    bg_bot: gfx::Color,
    card: gfx::Color,
    edge: gfx::Color,
    ink: gfx::Color,
    ink2: gfx::Color,
    ink3: gfx::Color,
    accent: gfx::Color,
    green: gfx::Color,
    track: gfx::Color,
    grid: gfx::Color,
    tag: gfx::Color,
    tag_ink: gfx::Color,
    shadow: gfx::Color,
}

fn theme(look: &Look) -> Theme {
    let contrast = look.high_contrast;
    if look.dark {
        Theme {
            dark: true,
            bg_top: rgb(0.043, 0.055, 0.082),
            bg_bot: rgb(0.063, 0.078, 0.114),
            card: rgb(0.086, 0.106, 0.149),
            edge: if contrast {
                rgb(0.30, 0.34, 0.42)
            } else {
                rgb(0.137, 0.165, 0.220)
            },
            ink: rgb(0.949, 0.961, 0.980),
            ink2: if contrast {
                rgb(0.86, 0.89, 0.93)
            } else {
                rgb(0.604, 0.647, 0.710)
            },
            ink3: if contrast {
                rgb(0.76, 0.80, 0.86)
            } else {
                rgb(0.45, 0.50, 0.57)
            },
            accent: rgb(0.298, 0.553, 1.0),
            green: rgb(0.239, 0.839, 0.549),
            track: rgb(0.125, 0.153, 0.208),
            grid: gfx::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.06,
            },
            tag: rgb(0.153, 0.184, 0.247),
            tag_ink: rgb(0.949, 0.961, 0.980),
            shadow: gfx::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.28,
            },
        }
    } else {
        Theme {
            dark: false,
            bg_top: rgb(0.965, 0.969, 0.980),
            bg_bot: rgb(0.925, 0.937, 0.957),
            card: rgb(1.0, 1.0, 1.0),
            edge: if contrast {
                rgb(0.60, 0.63, 0.69)
            } else {
                rgb(0.89, 0.905, 0.93)
            },
            ink: rgb(0.07, 0.082, 0.11),
            ink2: if contrast {
                rgb(0.20, 0.23, 0.28)
            } else {
                rgb(0.34, 0.375, 0.44)
            },
            ink3: if contrast {
                rgb(0.26, 0.29, 0.34)
            } else {
                rgb(0.50, 0.535, 0.59)
            },
            accent: rgb(0.184, 0.435, 0.922),
            green: rgb(0.05, 0.56, 0.31),
            track: rgb(0.925, 0.937, 0.957),
            grid: gfx::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.07,
            },
            tag: rgb(0.07, 0.082, 0.11),
            tag_ink: rgb(1.0, 1.0, 1.0),
            shadow: gfx::Color {
                r: 0.07,
                g: 0.09,
                b: 0.15,
                a: 0.08,
            },
        }
    }
}

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

fn alpha(c: gfx::Color, a: f32) -> gfx::Color {
    gfx::Color { a, ..c }
}

// ---- the demo ledger --------------------------------------------------------

const CAT_COUNT: usize = 4;
const CAT_NAMES: [&str; CAT_COUNT] = ["Food & drink", "Transport", "Shopping", "Bills"];
const CAT_COL: [gfx::Color; CAT_COUNT] = [
    rgb(1.0, 0.761, 0.294),   // warm yellow
    rgb(0.298, 0.553, 1.0),   // accent blue
    rgb(0.706, 0.549, 1.0),   // violet
    rgb(0.239, 0.839, 0.549), // green
];

/// One transaction: merchant, category index (or INCOME), cents.
struct Tx {
    merchant: &'static str,
    cat: usize,
    cents: i64,
}
/// Category index meaning "income", drawn green with a plus.
const INCOME: usize = usize::MAX;

const TXS: [Tx; 6] = [
    Tx {
        merchant: "Blue Bottle",
        cat: 0,
        cents: -1_450,
    },
    Tx {
        merchant: "Salary",
        cat: INCOME,
        cents: 240_000,
    },
    Tx {
        merchant: "Uber",
        cat: 1,
        cents: -2_380,
    },
    Tx {
        merchant: "Muji",
        cat: 2,
        cents: -8_640,
    },
    Tx {
        merchant: "Con Edison",
        cat: 3,
        cents: -9_212,
    },
    Tx {
        merchant: "Trader Joe's",
        cat: 0,
        cents: -6_318,
    },
];

/// Monthly totals per category, in cents (drive the share bars).
const CAT_TOTALS: [i64; CAT_COUNT] = [48_260, 21_480, 36_920, 31_840];

/// The balance headline and its monthly delta, in cents.
const BALANCE: i64 = 843_250;
const DELTA: i64 = 124_000;

/// Thirty days of spending, in whole dollars, tuned to a pleasing curve with a
/// mid-month spike and a calm tail. The last point is today.
const DAYS: usize = 30;
const SPEND: [u32; DAYS] = [
    42, 38, 55, 61, 47, 52, 88, 74, 66, 58, 71, 96, 132, 118, 84, 76, 69, 81, 94, 87, 72, 64, 58,
    66, 79, 92, 85, 71, 63, 68,
];

fn cat_color(cat: usize) -> gfx::Color {
    CAT_COL
        .get(cat)
        .copied()
        .unwrap_or(rgb(0.239, 0.839, 0.549))
}

fn cat_name(cat: usize) -> &'static str {
    CAT_NAMES.get(cat).copied().unwrap_or("Income")
}

fn spend(i: usize) -> f32 {
    SPEND.get(i.min(DAYS - 1)).copied().unwrap_or(0) as f32
}

fn shown(selected: Option<usize>) -> impl Iterator<Item = &'static Tx> {
    TXS.iter()
        .filter(move |tx| selected.is_none_or(|sel| tx.cat == sel))
}

// ---- tiny drawing + text helpers -------------------------------------------

fn gfx_rect(r: Rect) -> gfx::Rect {
    gfx::Rect {
        x: r.x,
        y: r.y,
        width: r.w,
        height: r.h,
    }
}

fn radii(r: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: r,
        top_right: r,
        bottom_right: r,
        bottom_left: r,
    }
}

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

fn disc(canvas: u64, x: f32, y: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x, y }, r, c)
}

fn rounded(
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

fn style(size: f32, weight: u16) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: if size >= 40.0 {
            -1.0
        } else if size >= 22.0 {
            -0.4
        } else {
            0.0
        },
        family: gfx::FontFamily::Sans,
    }
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, weight: u16, c: gfx::Color) {
    let _ =
        canvas2d::draw_text_styled(canvas, s, gfx::Point { x, y }, size, c, style(size, weight));
}

/// Rendered width of a string, measured by the host with the same font
/// layout `text` draws with.
fn width_of(canvas: u64, s: &str, size: f32, weight: u16) -> f32 {
    canvas2d::measure_text_styled(canvas, s, size, style(size, weight)).map_or(0.0, |m| m.width)
}

fn text_right(canvas: u64, s: &str, right: f32, y: f32, size: f32, weight: u16, c: gfx::Color) {
    text(
        canvas,
        s,
        right - width_of(canvas, s, size, weight),
        y,
        size,
        weight,
        c,
    );
}

/// Draw `s` at most `room` wide: the longest leading part that fits, then an
/// ellipsis, rather than running off the card.
#[allow(clippy::too_many_arguments)]
fn fit(canvas: u64, s: &str, x: f32, y: f32, size: f32, weight: u16, c: gfx::Color, room: f32) {
    if room <= 0.0 {
        return;
    }
    if width_of(canvas, s, size, weight) <= room {
        return text(canvas, s, x, y, size, weight, c);
    }
    let ell = width_of(canvas, "…", size, weight);
    let (mut lo, mut hi) = (0usize, s.len());
    while lo < hi {
        let mut mid = (lo + hi).div_ceil(2);
        while mid > lo && !s.is_char_boundary(mid) {
            mid -= 1;
        }
        if mid == lo {
            break;
        }
        if width_of(canvas, s.get(..mid).unwrap_or(""), size, weight) + ell <= room {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let part = s.get(..lo).unwrap_or("").trim_end();
    if !part.is_empty() {
        text(canvas, part, x, y, size, weight, c);
        text(
            canvas,
            "…",
            x + width_of(canvas, part, size, weight),
            y,
            size,
            weight,
            c,
        );
    }
}

/// Word-wrap `s` into lines at most `room` wide, calling `line` with each and
/// its index. Returns how many lines it took.
fn wrap(canvas: u64, s: &str, size: f32, room: f32, mut line: impl FnMut(&str, usize)) -> usize {
    let mut start = 0usize;
    let mut end = 0usize;
    let mut n = 0usize;
    for (i, _) in s.match_indices(' ').chain(core::iter::once((s.len(), ""))) {
        let candidate = s.get(start..i).unwrap_or("");
        if end > start && width_of(canvas, candidate, size, 400) > room {
            line(s.get(start..end).unwrap_or(""), n);
            n += 1;
            start = end + 1;
        }
        end = i;
    }
    if start < s.len() {
        line(s.get(start..).unwrap_or(""), n);
        n += 1;
    }
    n
}

/// A card: soft shadow, panel fill, hairline edge.
fn card(canvas: u64, r: Rect, t: &Theme) -> Result<(), gfx::GfxError> {
    canvas2d::drop_shadow_round_rect(
        canvas,
        gfx_rect(Rect { y: r.y + 4.0, ..r }),
        radii(16.0),
        12.0,
        t.shadow,
    )?;
    canvas2d::fill_round_rect(canvas, gfx_rect(r), radii(16.0), t.card)?;
    canvas2d::stroke_round_rect(canvas, gfx_rect(r), radii(16.0), 1.0, t.edge)
}

/// A straight stroke from one point to another: a tiny solid sprite,
/// stretched and turned. One host call however steep the line is.
fn segment(
    canvas: u64,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    thick: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = dx.hypot(dy);
    if len <= 0.0 {
        return Ok(());
    }
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    let px = [byte(c.r), byte(c.g), byte(c.b), byte(c.a)];
    let mut rgba = [0u8; 16];
    for (i, b) in rgba.iter_mut().enumerate() {
        *b = px.get(i % 4).copied().unwrap_or(0);
    }
    canvas2d::draw_sprite(
        canvas,
        gfx::Point {
            x: (x0 + x1) * 0.5,
            y: (y0 + y1) * 0.5,
        },
        gfx::Size {
            width: len + thick * 0.5,
            height: thick,
        },
        dy.atan2(dx),
        2,
        2,
        &rgba,
    )
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
fn money(buf: &mut [u8; 24], cents: i64, signed: bool) -> &str {
    let neg = cents < 0;
    let mut whole = cents.abs() / 100;
    let frac = (cents.abs() % 100) as u8;

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
        push(tmp.get(i).copied().unwrap_or(b'0'), buf, &mut out);
    }
    push(b'.', buf, &mut out);
    push(b'0' + frac / 10, buf, &mut out);
    push(b'0' + frac % 10, buf, &mut out);
    core::str::from_utf8(buf.get(..out).unwrap_or(b"$0")).unwrap_or("$0")
}

// ---- layout -----------------------------------------------------------------

/// Text sizes for this person, worked out once per frame.
struct Sizes {
    title: f32,
    big: f32,
    caption: f32,
    head: f32,
    sub: f32,
    body: f32,
    small: f32,
    /// A row a finger (or a mouse) can hit.
    row: f32,
}

/// Where each part of the dashboard goes, in page coordinates: y = 0 is the
/// top of the safe area, before the page is scrolled.
struct Lay {
    header: Rect,
    balance: Rect,
    chart: Rect,
    cats: Rect,
    insight: Rect,
    recent: Rect,
    /// How tall the whole page is.
    page_h: f32,
    /// The balance's delta chip goes under the number when it does not fit
    /// beside it.
    chip_below: bool,
}

const INSIGHT_HEAD: &str = "12% less than July";
const INSIGHT_BODY: &str = "Nice pace. At this rate you save about $410 more this month.";

fn cats_h(z: &Sizes) -> f32 {
    z.head * 1.3 + z.small * 1.6 + 34.0 + CAT_COUNT as f32 * z.row + 10.0
}

/// How the insight card lays out at width `w`: the lines its sentence wraps
/// to, whether the two stat tiles sit side by side, and its height.
fn insight_parts(canvas: u64, z: &Sizes, w: f32) -> (usize, bool, f32) {
    let room = w - 22.0 * 2.0 - 46.0;
    let lines = wrap(canvas, INSIGHT_BODY, z.small, room, |_, _| {});
    let side = (w - 44.0 - 12.0) * 0.5 >= 120.0 * z.body / 15.0;
    let tiles = if side {
        tile_h(z)
    } else {
        tile_h(z) * 2.0 + 10.0
    };
    let h =
        z.head * 1.3 + 30.0 + z.body * 1.4 + lines as f32 * z.small * 1.45 + 18.0 + tiles + 22.0;
    (lines, side, h)
}

fn insight_h(canvas: u64, z: &Sizes, w: f32) -> f32 {
    insight_parts(canvas, z, w).2
}

fn tile_h(z: &Sizes) -> f32 {
    z.small * 1.3 + z.body * 1.35 + 26.0
}

/// The month's daily average and its biggest day, worked out from the
/// thirty days the chart draws: (average dollars, biggest dollars, day).
fn month_stats() -> (u32, u32, usize) {
    let sum: u32 = SPEND.iter().sum();
    let (mut top, mut day) = (0u32, 0usize);
    for (i, v) in SPEND.iter().enumerate() {
        if *v > top {
            top = *v;
            day = i + 1;
        }
    }
    ((sum + DAYS as u32 / 2) / DAYS as u32, top, day)
}

/// "$74" or "Aug 13" into `buf`.
fn num_text<'b>(buf: &'b mut [u8; 16], lead: &str, n: u32) -> &'b str {
    let mut pos = 0usize;
    for b in lead.bytes() {
        if let Some(slot) = buf.get_mut(pos) {
            *slot = b;
            pos += 1;
        }
    }
    let mut digits = [0u8; 10];
    let mut k = 0usize;
    let mut v = n;
    loop {
        if let Some(slot) = digits.get_mut(k) {
            *slot = b'0' + (v % 10) as u8;
            k += 1;
        }
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while k > 0 {
        k -= 1;
        if let (Some(slot), Some(d)) = (buf.get_mut(pos), digits.get(k)) {
            *slot = *d;
            pos += 1;
        }
    }
    core::str::from_utf8(buf.get(..pos).unwrap_or(b"")).unwrap_or("")
}

fn recent_h(z: &Sizes, selected: Option<usize>) -> f32 {
    let n = shown(selected).count();
    let rows = if n == 0 {
        z.body * 3.0
    } else {
        n as f32 * z.row
    };
    z.head * 1.3 + 34.0 + rows + 10.0
}

fn layout(canvas: u64, area: Rect, z: &Sizes, selected: Option<usize>, short: bool) -> Lay {
    let gap = if area.w < 600.0 { 14.0 } else { 22.0 };
    let header_h = z.title * 1.25 + 8.0;

    // Does the delta chip fit beside the balance on this width?
    let mut buf = [0u8; 24];
    let bal_w = width_of(canvas, money(&mut buf, BALANCE, false), z.big, 700);
    let chip_w = width_of(canvas, "+$1,240 this month", z.body, 500) + 30.0;
    let balance_w = if area.w >= 900.0 {
        (area.w - gap) * 0.61
    } else {
        area.w
    };
    let chip_below = bal_w + 22.0 + chip_w > balance_w;
    let balance_h =
        z.caption * 1.8 + z.big * 1.1 + if chip_below { z.body * 2.2 + 12.0 } else { 0.0 } + gap;

    let mut y = 0.0;
    let header = Rect::new(area.x, y, area.w, header_h);
    y += header_h + gap * 0.5;

    if area.w >= 900.0 {
        // Two columns: the money on the left, the breakdown on the right.
        let left_w = (area.w - gap) * 0.61;
        let right_w = area.w - gap - left_w;
        let right_x = area.x + left_w + gap;
        let balance = Rect::new(area.x, y, left_w, balance_h);
        // The chart takes what the recent list leaves; when the list does
        // not fit beside a readable chart, it shows the newest rows that do
        // (three at least, and the page scrolls for the rest).
        let recent_need = recent_h(z, None);
        let recent_least = z.head * 1.3 + 44.0 + 3.0 * z.row;
        let top = y + balance_h;
        let room = area.h - top - gap;
        let chart_h = (room - recent_need).max(240.0);
        let chart = Rect::new(area.x, top, left_w, chart_h);
        let recent_h = recent_need.min((room - chart_h).max(recent_least));
        let recent = Rect::new(area.x, chart.bottom() + gap, left_w, recent_h);
        // The right column ends where the left does: the categories take
        // up to two fifths more room for their rows, the insight the rest.
        let left_bottom = recent.bottom();
        let insight_need = insight_h(canvas, z, right_w);
        let spare = (left_bottom - y - cats_h(z) - gap - insight_need).max(0.0);
        let cats = Rect::new(
            right_x,
            y,
            right_w,
            cats_h(z) + spare.min(CAT_COUNT as f32 * z.row * 0.4),
        );
        let insight_h = (left_bottom - cats.bottom() - gap).max(insight_need);
        let insight = Rect::new(right_x, cats.bottom() + gap, right_w, insight_h);
        let page_h = left_bottom.max(insight.bottom());
        Lay {
            header,
            balance,
            chart,
            cats,
            insight,
            recent,
            page_h,
            chip_below,
        }
    } else {
        // One column; on a tablet the categories and the insight share a row.
        let balance = Rect::new(area.x, y, area.w, balance_h);
        y += balance_h;
        // Taller as the text grows, so the heading never eats the plot.
        let grow = (z.head - 18.0).max(0.0) * 3.0;
        let chart_h = if short {
            210.0
        } else {
            (area.w * 0.55).clamp(220.0, 300.0)
        } + grow;
        let chart = Rect::new(area.x, y, area.w, chart_h);
        y += chart_h + gap;
        let (cats, insight);
        if area.w >= 600.0 {
            let half = (area.w - gap) * 0.5;
            let h = cats_h(z).max(insight_h(canvas, z, half));
            cats = Rect::new(area.x, y, half, h);
            insight = Rect::new(area.x + half + gap, y, half, h);
            y += h + gap;
        } else {
            cats = Rect::new(area.x, y, area.w, cats_h(z));
            y += cats.h + gap;
            insight = Rect::new(area.x, y, area.w, insight_h(canvas, z, area.w));
            y += insight.h + gap;
        }
        let recent = Rect::new(area.x, y, area.w, recent_h(z, selected));
        let page_h = recent.bottom();
        Lay {
            header,
            balance,
            chart,
            cats,
            insight,
            recent,
            page_h,
            chip_below,
        }
    }
}

// ---- the view ---------------------------------------------------------------

/// What the person is looking at, and where the last frame drew what can be
/// tapped. Taps are tested against these rects, so the drawing and the hit
/// test can never disagree.
struct View {
    selected: Option<usize>,
    scroll: f32,
    max_scroll: f32,
    rows: [Option<Rect>; CAT_COUNT],
    clear: Option<Rect>,
}

impl View {
    /// A tap at `x`, `y`. Returns whether the selection changed.
    fn tap(&mut self, x: f32, y: f32) -> bool {
        if self.clear.is_some_and(|r| r.contains(x, y)) {
            self.selected = None;
            return true;
        }
        let hit = self
            .rows
            .iter()
            .position(|r| r.is_some_and(|r| r.contains(x, y)));
        if let Some(i) = hit {
            // Tap the active row again to clear the filter.
            self.selected = if self.selected == Some(i) {
                None
            } else {
                Some(i)
            };
            return true;
        }
        false
    }

    fn wheel(&mut self, dy: f32) -> bool {
        let next = (self.scroll + dy).clamp(0.0, self.max_scroll);
        let moved = next != self.scroll;
        self.scroll = next;
        moved
    }
}

fn draw(canvas: u64, win: u64, view: &mut View) -> Result<(), gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas).unwrap_or(gfx::Size {
        width: WIDTH,
        height: HEIGHT,
    });
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let screen = Screen::of(win);
    let look = screen.look;
    let t = theme(&look);
    let mut said = Said::new();
    view.rows = [None; CAT_COUNT];
    view.clear = None;

    canvas2d::linear_gradient(
        canvas,
        gfx::Rect {
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
        },
        t.bg_top,
        t.bg_bot,
    )?;

    // Text and controls stay clear of a notch, the home bar and the close
    // button Krate draws on a phone; the ground still fills the canvas.
    let narrow = w < 600.0;
    let short = h < 500.0;
    let margin = if narrow || short { 16.0 } else { 32.0 };
    let (top, right, bottom, left) = screen.safe;
    let area = Rect::new(0.0, 0.0, w, h)
        .inset(top, right, bottom, left)
        .pad(margin);
    let z = Sizes {
        title: look.text(if short { 24.0 } else { 30.0 }),
        big: look.text(if narrow || short { 44.0 } else { 56.0 }),
        caption: look.text(12.0),
        head: look.text(18.0),
        sub: look.text(12.5),
        body: look.text(15.0),
        small: look.text(13.0),
        row: (look.text(15.0) * 1.3 + look.text(11.5) * 1.4 + 12.0).max(screen.tap() + 6.0),
    };
    let lay = layout(canvas, area, &z, view.selected, short);
    view.max_scroll = (lay.page_h - area.h).max(0.0);
    view.scroll = view.scroll.clamp(0.0, view.max_scroll);
    let oy = area.y - view.scroll;
    let at = |r: Rect| Rect { y: r.y + oy, ..r };

    // Everything below scrolls with the page; it is clipped to the safe
    // area so nothing slides under the notch.
    canvas2d::set_clip(canvas, 0.0, top, w, (h - top - bottom).max(0.0))?;
    draw_header(canvas, at(lay.header), &z, &t, &mut said);
    draw_balance(canvas, at(lay.balance), &z, &t, lay.chip_below, &mut said)?;
    draw_chart(canvas, at(lay.chart), &z, &t, &mut said)?;
    draw_categories(canvas, at(lay.cats), &z, &t, view, &mut said)?;
    draw_insight(canvas, at(lay.insight), &z, &t, &mut said)?;
    draw_recent(canvas, at(lay.recent), &z, &t, view, &mut said)?;
    canvas2d::clear_clip(canvas)?;

    canvas2d::present(canvas)?;
    said.send(win);
    Ok(())
}

fn draw_header(canvas: u64, r: Rect, z: &Sizes, t: &Theme, said: &mut Said) {
    // Logo dot + wordmark.
    let base = r.y + z.title * 0.95;
    let dot = z.title * 0.3;
    let _ = disc(canvas, r.x + dot, base - z.title * 0.36, dot, t.accent);
    let _ = disc(canvas, r.x + dot, base - z.title * 0.36, dot * 0.38, t.card);
    let x = r.x + dot * 2.0 + 10.0;
    text(canvas, "Pulse", x, base, z.title, 700, t.ink);
    let tw = width_of(canvas, "Pulse", z.title, 700);
    said.heading(Rect::new(r.x, r.y, x + tw - r.x, z.title * 1.2), "Pulse");
    let month = "August 2026";
    let mw = width_of(canvas, month, z.body, 400);
    if r.right() - mw > x + tw + 16.0 {
        text_right(canvas, month, r.right(), base - 3.0, z.body, 400, t.ink2);
        said.text(Rect::new(r.right() - mw, r.y, mw, z.title * 1.2), month);
    }
}

fn draw_balance(
    canvas: u64,
    r: Rect,
    z: &Sizes,
    t: &Theme,
    chip_below: bool,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    let cap_y = r.y + z.caption * 1.2;
    let _ = canvas2d::draw_text_styled(
        canvas,
        "TOTAL BALANCE",
        gfx::Point { x: r.x, y: cap_y },
        z.caption,
        t.ink3,
        gfx::TextStyle {
            weight: 600,
            italic: false,
            letter_spacing: 1.0,
            family: gfx::FontFamily::Sans,
        },
    );
    let mut buf = [0u8; 24];
    let bal = money(&mut buf, BALANCE, false);
    let bal_y = cap_y + z.caption * 0.6 + z.big;
    fit(canvas, bal, r.x - 2.0, bal_y, z.big, 700, t.ink, r.w);

    // The delta chip: beside the number, or under it on a narrow screen.
    let mut dbuf = [0u8; 24];
    let delta = money(&mut dbuf, DELTA, true);
    // "+$1,240.00" -> drop the cents for the chip.
    let delta_short = delta.get(..delta.len().saturating_sub(3)).unwrap_or(delta);
    let dw = width_of(canvas, delta_short, z.body, 600);
    let label_w = dw + width_of(canvas, " this month", z.body, 400);
    let chip_h = z.body * 2.0;
    let (chip_x, chip_y) = if chip_below {
        (r.x, bal_y + 14.0)
    } else {
        (
            r.x + width_of(canvas, bal, z.big, 700) + 22.0,
            bal_y - z.big * 0.36 - chip_h * 0.5,
        )
    };
    let chip_bg = blend(t.card, t.green, if t.dark { 0.14 } else { 0.12 });
    rounded(
        canvas,
        chip_x,
        chip_y,
        label_w + 28.0,
        chip_h,
        chip_h * 0.5,
        chip_bg,
    )?;
    let ty = chip_y + chip_h * 0.5 + z.body * 0.36;
    text(canvas, delta_short, chip_x + 14.0, ty, z.body, 600, t.green);
    text(
        canvas,
        " this month",
        chip_x + 14.0 + dw,
        ty,
        z.body,
        400,
        t.ink2,
    );

    let mut s = String::new();
    s.push_str("Total balance ");
    s.push_str(bal);
    s.push_str(", up ");
    s.push_str(delta_short.trim_start_matches('+'));
    s.push_str(" this month");
    said.text(Rect::new(r.x, r.y, r.w, r.h), &s);
    Ok(())
}

fn draw_chart(
    canvas: u64,
    r: Rect,
    z: &Sizes,
    t: &Theme,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    card(canvas, r, t)?;
    let pad = if r.w < 400.0 { 18.0 } else { 24.0 };
    let head_y = r.y + pad + z.head * 0.9;
    text(canvas, "Spending", r.x + pad, head_y, z.head, 600, t.ink);
    let sub_y = head_y + z.sub * 1.6;
    text(canvas, "Last 30 days", r.x + pad, sub_y, z.sub, 400, t.ink3);
    said.heading(
        Rect::new(r.x + pad, r.y + pad, r.w - 2.0 * pad, z.head * 1.2),
        "Spending, last 30 days",
    );

    // The dollar labels sit in their own column at the left, so today's tag
    // at the right never lands on one.
    let label = look_small(z);
    let label_w = width_of(canvas, "$140", label, 400) + 10.0;
    let inner_x = r.x + pad + label_w;
    let inner_w = r.w - pad * 2.0 - label_w - 6.0;
    let base_y = r.bottom() - pad - 6.0;
    // The plot takes the height under the heading, with room above the
    // highest point for today's tag.
    let plot_top = sub_y + 34.0;
    let plot_h = (base_y - plot_top).max(40.0);

    // Gridlines + axis labels at $0 / $70 / $140.
    let max = 140.0f32;
    for (value, name) in [(0.0f32, "$0"), (70.0, "$70"), (140.0, "$140")] {
        let gy = base_y - plot_h * (value / max);
        fill(canvas, inner_x, gy, inner_w, 1.0, t.grid)?;
        text_right(
            canvas,
            name,
            inner_x - 10.0,
            gy + label * 0.36,
            label,
            400,
            t.ink3,
        );
    }

    // Y for a data value.
    let y_of = |v: f32| base_y - plot_h * (v / max).min(1.0);
    let step = inner_w / (DAYS - 1) as f32;
    let value_at = |fx: f32| {
        let seg = (fx / step).clamp(0.0, (DAYS - 2) as f32);
        let i = seg as usize;
        let k = seg - i as f32;
        spend(i) * (1.0 - k) + spend(i + 1) * k
    };

    // Area fill: translucent columns three points wide, under the line.
    let soft = alpha(t.accent, if t.dark { 0.16 } else { 0.12 });
    let mut fx = 0.0f32;
    while fx < inner_w {
        let wide = 3.0f32.min(inner_w - fx);
        let top = y_of(value_at(fx + wide * 0.5));
        fill(canvas, inner_x + fx, top, wide, base_y - top, soft)?;
        fx += 3.0;
    }
    // The line: one straight stroke per day, with a round joint at each.
    for i in 0..DAYS - 1 {
        let (x0, x1) = (inner_x + i as f32 * step, inner_x + (i + 1) as f32 * step);
        let (y0, y1) = (y_of(spend(i)), y_of(spend(i + 1)));
        segment(canvas, x0, y0, x1, y1, 2.4, t.accent)?;
        disc(canvas, x1, y1, 1.2, t.accent)?;
    }

    // Today: a glow, a solid dot, and a value tag above it.
    let today_x = inner_x + inner_w;
    let today_y = y_of(spend(DAYS - 1));
    canvas2d::radial_gradient(
        canvas,
        gfx::Point {
            x: today_x,
            y: today_y,
        },
        26.0,
        alpha(t.accent, 0.35),
        alpha(t.accent, 0.0),
    )?;
    disc(canvas, today_x, today_y, 5.0, t.accent)?;
    disc(canvas, today_x, today_y, 2.2, t.card)?;
    let tag = "$68 today";
    let tag_h = label * 2.1;
    let tag_w = width_of(canvas, tag, label, 600) + 20.0;
    let tag_y = (today_y - 16.0 - tag_h).max(plot_top - tag_h * 0.5);
    rounded(
        canvas,
        today_x - tag_w - 10.0,
        tag_y,
        tag_w,
        tag_h,
        tag_h * 0.5,
        t.tag,
    )?;
    text(
        canvas,
        tag,
        today_x - tag_w,
        tag_y + tag_h * 0.5 + label * 0.36,
        label,
        600,
        t.tag_ink,
    );
    said.image(
        Rect::new(inner_x, plot_top, inner_w, base_y - plot_top),
        "Daily spending over the last 30 days: highest $132 mid-month, $68 today",
    );
    Ok(())
}

/// Axis labels and tags: the smallest text, still at the person's size.
fn look_small(z: &Sizes) -> f32 {
    z.small * 0.88
}

fn draw_categories(
    canvas: u64,
    r: Rect,
    z: &Sizes,
    t: &Theme,
    view: &mut View,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    card(canvas, r, t)?;
    let pad = 22.0;
    let head_y = r.y + pad + z.head * 0.9;
    text(canvas, "Categories", r.x + pad, head_y, z.head, 600, t.ink);
    let sub_y = head_y + z.sub * 1.6;
    text(canvas, "This month", r.x + pad, sub_y, z.sub, 400, t.ink3);
    said.heading(
        Rect::new(r.x + pad, r.y + pad, r.w - 2.0 * pad, z.head * 1.2),
        "Categories, this month",
    );

    let total: i64 = CAT_TOTALS.iter().sum();
    let rows_y = sub_y + 18.0;
    // Rows share a card that was given more height than they need.
    let row_h = ((r.bottom() - 10.0 - rows_y) / CAT_COUNT as f32).clamp(z.row, z.row * 1.4);
    for i in 0..CAT_COUNT {
        let row = Rect::new(
            r.x + 10.0,
            rows_y + i as f32 * row_h,
            r.w - 20.0,
            row_h - 4.0,
        );
        let picked = view.selected == Some(i);
        // Selection wash behind the active row.
        if picked {
            rounded(
                canvas,
                row.x,
                row.y,
                row.w,
                row.h,
                10.0,
                blend(t.card, t.accent, if t.dark { 0.18 } else { 0.10 }),
            )?;
        }
        let total_i = CAT_TOTALS.get(i).copied().unwrap_or(0);
        let line_y = row.y + row.h * 0.5 - 2.0;
        let c = cat_color(i);
        disc(canvas, row.x + 18.0, line_y - z.body * 0.35, 5.0, c)?;
        let mut buf = [0u8; 24];
        let amt = money(&mut buf, -total_i, false);
        let amt_w = width_of(canvas, amt, z.body, 500);
        text_right(canvas, amt, row.right() - 12.0, line_y, z.body, 500, t.ink2);
        let name_x = row.x + 34.0;
        fit(
            canvas,
            cat_name(i),
            name_x,
            line_y,
            z.body,
            500,
            t.ink,
            row.right() - 24.0 - amt_w - name_x,
        );
        // Share bar under the row text.
        let bar_w = row.right() - 12.0 - name_x;
        let share = total_i as f32 / total.max(1) as f32;
        let bar_y = line_y + z.body * 0.45;
        rounded(canvas, name_x, bar_y, bar_w, 5.0, 2.5, t.track)?;
        rounded(
            canvas,
            name_x,
            bar_y,
            (bar_w * share).max(6.0),
            5.0,
            2.5,
            alpha(c, 0.9),
        )?;

        if let Some(slot) = view.rows.get_mut(i) {
            *slot = Some(row);
        }
        // Pressed by its name; the amount is read after it.
        said.toggle(row, cat_name(i), picked);
        let mut s = String::new();
        s.push_str(amt.trim_start_matches('-'));
        s.push_str(" this month");
        said.text(
            Rect::new(row.right() - 12.0 - amt_w, row.y, amt_w, row.h),
            &s,
        );
    }
    Ok(())
}

fn draw_recent(
    canvas: u64,
    r: Rect,
    z: &Sizes,
    t: &Theme,
    view: &mut View,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    card(canvas, r, t)?;
    let pad = 22.0;
    let head_y = r.y + pad + z.head * 0.9;
    text(canvas, "Recent", r.x + pad, head_y, z.head, 600, t.ink);
    said.heading(
        Rect::new(r.x + pad, r.y + pad, z.head * 4.0, z.head * 1.2),
        "Recent",
    );
    let after = r.x + pad + width_of(canvas, "Recent", z.head, 600) + 14.0;
    match view.selected {
        Some(i) => {
            // The filter as a chip that clears it, so a phone that scrolled
            // past the categories can still show everything again.
            let label = cat_name(i);
            let lw = width_of(canvas, label, z.sub, 600);
            let chip_h = (z.sub * 2.1).max(28.0);
            let chip = Rect::new(
                after,
                head_y - z.head * 0.36 - chip_h * 0.5,
                lw + chip_h + 14.0,
                chip_h,
            );
            rounded(
                canvas,
                chip.x,
                chip.y,
                chip.w,
                chip.h,
                chip_h * 0.5,
                blend(t.card, t.accent, if t.dark { 0.18 } else { 0.10 }),
            )?;
            let ty = chip.y + chip_h * 0.5 + z.sub * 0.36;
            text(canvas, label, chip.x + 12.0, ty, z.sub, 600, t.accent);
            // An x drawn from two short strokes.
            let (cx, cy, k) = (
                chip.right() - chip_h * 0.5 - 2.0,
                chip.y + chip_h * 0.5,
                z.sub * 0.28,
            );
            segment(canvas, cx - k, cy - k, cx + k, cy + k, 1.6, t.accent)?;
            segment(canvas, cx - k, cy + k, cx + k, cy - k, 1.6, t.accent)?;
            // A finger-sized target around the chip.
            let hit = Rect::new(
                chip.x - 4.0,
                chip.y.min(cy - 22.0),
                chip.w + 8.0,
                chip_h.max(44.0),
            );
            view.clear = Some(hit);
            let mut s = String::new();
            s.push_str("Showing ");
            s.push_str(label);
            s.push_str(". Show all activity");
            said.button(hit, &s);
        }
        None => text(
            canvas,
            "All activity",
            after,
            head_y - 1.0,
            z.sub,
            400,
            t.ink3,
        ),
    }

    let mut ry = head_y + 22.0;
    let mut count = 0u32;
    let fits = ((r.bottom() + 1.0 - ry) / z.row).max(0.0) as u32;
    let total = (shown(view.selected).count() as u32).min(fits);
    for tx in shown(view.selected) {
        let row = Rect::new(r.x + pad, ry, r.w - 2.0 * pad, z.row);
        if row.bottom() > r.bottom() + 1.0 {
            break;
        }
        // Avatar: rounded square in the category color (green for income).
        let av = if tx.cat == INCOME {
            t.green
        } else {
            cat_color(tx.cat)
        };
        let side = (z.row * 0.62).min(40.0);
        let av_y = row.y + (row.h - side) * 0.5;
        rounded(
            canvas,
            row.x,
            av_y,
            side,
            side,
            side * 0.3,
            blend(t.card, av, 0.22),
        )?;
        let initial = tx.merchant.get(..1).unwrap_or("?");
        let av_ink = if t.dark { av } else { blend(av, t.ink, 0.45) };
        let iw = width_of(canvas, initial, z.body, 700);
        text(
            canvas,
            initial,
            row.x + (side - iw) * 0.5,
            av_y + side * 0.5 + z.body * 0.36,
            z.body,
            700,
            av_ink,
        );

        let mut buf = [0u8; 24];
        let amt = money(&mut buf, tx.cents, tx.cents > 0);
        let color = if tx.cents > 0 { t.green } else { t.ink };
        let amt_w = width_of(canvas, amt, z.body, 600);
        let mid = row.y + row.h * 0.5;
        text_right(
            canvas,
            amt,
            row.right(),
            mid + z.body * 0.36,
            z.body,
            600,
            color,
        );

        let x = row.x + side + 14.0;
        let room = row.right() - amt_w - 14.0 - x;
        let block = z.body + z.small * 1.3;
        let name_y = mid - block * 0.5 + z.body * 0.85;
        fit(canvas, tx.merchant, x, name_y, z.body, 500, t.ink, room);
        let cat_label = if tx.cat == INCOME {
            "Income"
        } else {
            cat_name(tx.cat)
        };
        fit(
            canvas,
            cat_label,
            x,
            name_y + z.small * 1.35,
            z.small * 0.92,
            400,
            t.ink3,
            room,
        );

        // Hairline between rows, not under the last.
        if count + 1 < total {
            fill(canvas, x, row.bottom() - 1.0, row.right() - x, 1.0, t.grid)?;
        }
        let mut s = String::new();
        s.push_str(tx.merchant);
        s.push_str(", ");
        s.push_str(cat_label);
        s.push_str(", ");
        s.push_str(amt);
        said.text(row, &s);
        ry += z.row;
        count += 1;
    }
    if count == 0 {
        let msg = "No activity in this category yet.";
        text(
            canvas,
            msg,
            r.x + pad,
            ry + z.body * 1.5,
            z.body * 0.9,
            400,
            t.ink3,
        );
        said.text(Rect::new(r.x + pad, ry, r.w - 2.0 * pad, z.body * 2.0), msg);
    }
    Ok(())
}

/// The small insight card under Categories: one honest derived stat.
fn draw_insight(
    canvas: u64,
    r: Rect,
    z: &Sizes,
    t: &Theme,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    card(canvas, r, t)?;
    let pad = 22.0;
    let head_y = r.y + pad + z.head * 0.9;
    text(canvas, "Insight", r.x + pad, head_y, z.head, 600, t.ink);
    said.heading(
        Rect::new(r.x + pad, r.y + pad, r.w - 2.0 * pad, z.head * 1.2),
        "Insight",
    );
    // A green down-arrow chip: spending is lower than last month.
    let green_chip = blend(t.card, t.green, if t.dark { 0.14 } else { 0.12 });
    let ax = r.x + pad + 14.0;
    let line_y = head_y + 22.0 + z.body;
    let ay = line_y - z.body * 0.35;
    disc(canvas, ax, ay, 16.0, green_chip)?;
    segment(canvas, ax, ay - 7.0, ax, ay + 6.0, 2.6, t.green)?;
    segment(canvas, ax - 5.5, ay + 1.0, ax, ay + 6.5, 2.6, t.green)?;
    segment(canvas, ax + 5.5, ay + 1.0, ax, ay + 6.5, 2.6, t.green)?;
    let x = ax + 32.0;
    let room = r.right() - pad - x;
    fit(canvas, INSIGHT_HEAD, x, line_y, z.body, 600, t.ink, room);
    let body_y = line_y + z.small * 1.55;
    let lines = wrap(canvas, INSIGHT_BODY, z.small, room, |line, i| {
        text(
            canvas,
            line,
            x,
            body_y + i as f32 * z.small * 1.45,
            z.small,
            400,
            t.ink2,
        );
    });
    let mut s = String::new();
    s.push_str(INSIGHT_HEAD);
    s.push_str(". ");
    s.push_str(INSIGHT_BODY);
    said.text(
        Rect::new(
            x,
            line_y - z.body,
            room,
            z.body * 1.4 + lines as f32 * z.small * 1.45,
        ),
        &s,
    );

    // Two numbers from the same thirty days the chart draws.
    let (_, side, _) = insight_parts(canvas, z, r.w);
    let (avg, top, day) = month_stats();
    let (mut b1, mut b2, mut b3) = ([0u8; 16], [0u8; 16], [0u8; 16]);
    let tiles = [
        ("Daily average", num_text(&mut b1, "$", avg), "a day"),
        (
            "Biggest day",
            num_text(&mut b2, "$", top),
            num_text(&mut b3, "on Aug ", day as u32),
        ),
    ];
    let th = tile_h(z);
    // Under the sentence, or at the foot of a card given more room.
    let tiles_h = if side { th } else { th * 2.0 + 10.0 };
    let mut ty = (body_y + (lines as f32 - 1.0).max(0.0) * z.small * 1.45 + z.small + 18.0)
        .max(r.bottom() - pad - tiles_h);
    let tw = if side {
        (r.w - 2.0 * pad - 12.0) * 0.5
    } else {
        r.w - 2.0 * pad
    };
    for (i, (label, value, note)) in tiles.iter().enumerate() {
        let tx = if side {
            r.x + pad + i as f32 * (tw + 12.0)
        } else {
            r.x + pad
        };
        let tile = Rect::new(tx, ty, tw, th);
        rounded(canvas, tile.x, tile.y, tile.w, tile.h, 12.0, t.track)?;
        let ly = tile.y + 13.0 + z.small * 0.9;
        fit(
            canvas,
            label,
            tile.x + 14.0,
            ly,
            z.small,
            400,
            t.ink3,
            tile.w - 28.0,
        );
        let vy = ly + z.body * 1.45;
        let value_size = z.body * 1.25;
        text(canvas, value, tile.x + 14.0, vy, value_size, 700, t.ink);
        let vw = width_of(canvas, value, value_size, 700);
        fit(
            canvas,
            note,
            tile.x + 14.0 + vw + 8.0,
            vy,
            z.small,
            400,
            t.ink2,
            tile.w - 36.0 - vw,
        );
        let mut said_tile = String::new();
        said_tile.push_str(label);
        said_tile.push_str(" ");
        said_tile.push_str(value);
        said_tile.push_str(" ");
        said_tile.push_str(note);
        said.text(tile, &said_tile);
        if !side {
            ty += th + 10.0;
        }
    }
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
        style: types::Style {
            width: None,
            height: None,
            grow: 1.0,
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

fn canvas_node() -> types::WidgetNode {
    types::WidgetNode {
        id: CANVAS_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Canvas,
        label: None,
        role: Some(pure_string("canvas")),
        style: types::Style {
            width: None,
            height: None,
            grow: 1.0,
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

// ---- app --------------------------------------------------------------------

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Pulse", size) else {
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

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        let mut view = View {
            selected: if quick { None } else { load_selected() },
            scroll: 0.0,
            max_scroll: 0.0,
            rows: [None; CAT_COUNT],
            clear: None,
        };

        if quick {
            // The automated shot: the full dashboard, nothing filtered.
            let _ = draw(canvas, win, &mut view);
            let out = stdio::stdout();
            let _ = out.write(b"pulse:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        let _ = draw(canvas, win, &mut view);

        // A real session ends when the person closes the window, never on a
        // round count (K-092); the quick shot returned above.
        'session: loop {
            // Take everything already queued, then draw once: a finger
            // dragging the page sends far more events than there are frames.
            let mut dirty = false;
            let mut next = events::wait(Some(WAIT_ROUND_MILLIS));
            while let Some(event) = next {
                match event {
                    types::Event::CloseRequested(id) if id == win => break 'session,
                    // A new size is a new layout; a new theme, new text size.
                    types::Event::Resized(_)
                    | types::Event::ThemeChanged(_)
                    | types::Event::RedrawRequested(_) => dirty = true,
                    // On the release, so a finger that drags to scroll does
                    // not pick the row it started on.
                    types::Event::Pointer(p) if p.button.is_some() && !p.pressed => {
                        if view.tap(p.x, p.y) {
                            save_selected(view.selected);
                            dirty = true;
                        }
                    }
                    types::Event::Wheel(w) => dirty |= view.wheel(w.dy),
                    _ => {}
                }
                next = events::poll();
            }
            if dirty {
                let _ = draw(canvas, win, &mut view);
            }
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"pulse:ok\n");
        let _ = out.flush();
        0
    }
}

krate::export!(Component);
