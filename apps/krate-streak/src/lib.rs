//! Streak -- a habit tracker drawn entirely on a canvas.
//!
//! The hero layout: a huge current-streak number with a flame dot, a
//! GitHub-style year heat map (26 weeks x 7 days, five intensity levels),
//! and a "Today" column of habit rows with completion rings. Clicking a
//! habit toggles it and the completed count is stored. All data is a seeded
//! local demo year; honest about being a demo, real about what it does.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler. Fixed-size state, no `format!`, no panicking index.

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
const LEFT_W: f32 = 672.0;
const GUTTER: f32 = 24.0;
const RIGHT_X: f32 = MARGIN + LEFT_W + GUTTER;
const RIGHT_W: f32 = WIDTH - RIGHT_X - MARGIN;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;
const MAX_IDLE_ROUNDS: u32 = 300;

const DONE_KEY: &str = "done-today";

// ---- palette (the shared canvas design system) ------------------------------

const BG_TOP: gfx::Color = rgb(0.043, 0.055, 0.082);
const BG_BOT: gfx::Color = rgb(0.063, 0.078, 0.114);
const CARD: gfx::Color = rgb(0.086, 0.106, 0.149);
const CARD_EDGE: gfx::Color = rgb(0.137, 0.165, 0.220);
const INK: gfx::Color = rgb(0.949, 0.961, 0.980);
const INK_DIM: gfx::Color = rgb(0.604, 0.647, 0.710);
const INK_QUIET: gfx::Color = rgb(0.365, 0.408, 0.471);
const ACCENT: gfx::Color = rgb(0.298, 0.553, 1.0);
const FLAME: gfx::Color = rgb(1.0, 0.624, 0.263);
const GREEN: gfx::Color = rgb(0.239, 0.839, 0.549);

/// Heat levels, dim to bright. Level 0 is the empty cell.
const HEAT: [gfx::Color; 5] = [
    rgb(0.102, 0.125, 0.173),
    rgb(0.114, 0.216, 0.373),
    rgb(0.153, 0.333, 0.588),
    rgb(0.216, 0.463, 0.804),
    rgb(0.361, 0.620, 1.0),
];

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

// ---- the demo year -----------------------------------------------------------

const WEEKS: usize = 26;
const STREAK: u32 = 47;
const BEST: u32 = 62;
const YEAR_DAYS: u32 = 214;

/// Deterministic heat level for a cell: a hand-tuned pseudo-random pattern
/// that trends brighter toward recent weeks, with rest days sprinkled in.
fn heat_level(week: usize, day: usize) -> usize {
    let n = (week * 7 + day) as u32;
    let h = n.wrapping_mul(2_654_435_761) >> 27; // 0..31
    let bias = (week * 3) / WEEKS; // 0..2, later weeks brighter
    let raw = match h {
        0..=7 => 0,
        8..=14 => 1,
        15..=21 => 2,
        22..=27 => 3,
        _ => 4,
    };
    // Recent six weeks: the streak, no zero days.
    if week >= WEEKS - 6 && raw == 0 {
        return 1 + bias;
    }
    (raw + bias).min(4)
}

const HABIT_COUNT: usize = 4;
const HABITS: [(&str, &str); HABIT_COUNT] = [
    ("Morning run", "6:30 am"),
    ("Read 20 pages", "any time"),
    ("No sugar", "all day"),
    ("Ship something", "before 6 pm"),
];

// ---- tiny drawing + text helpers --------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

fn disc(canvas: u64, x: f32, y: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x, y }, r, c)
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, c);
}

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
    canvas2d::fill_round_rect(
        canvas,
        gfx::Rect { x, y, width: w, height: h },
        gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r },
        c,
    )
}

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

/// Format a u32 into `buf`, returning the used slice.
fn num<'b>(buf: &'b mut [u8; 12], mut n: u32) -> &'b str {
    let mut tmp = [0u8; 12];
    let mut i = 0usize;
    loop {
        if let Some(slot) = tmp.get_mut(i) {
            *slot = b'0' + (n % 10) as u8;
            i += 1;
        }
        n /= 10;
        if n == 0 {
            break;
        }
    }
    let mut out = 0usize;
    while i > 0 {
        i -= 1;
        if let Some(slot) = buf.get_mut(out) {
            *slot = tmp[i.min(11)];
            out += 1;
        }
    }
    core::str::from_utf8(buf.get(..out).unwrap_or(b"0")).unwrap_or("0")
}

// ---- layout ------------------------------------------------------------------

const HEADER_BASE: f32 = 64.0;
const HERO_Y: f32 = 120.0;
const MAP_Y: f32 = 320.0;
const MAP_H: f32 = 300.0;
const TODAY_Y: f32 = 98.0;
const TODAY_H: f32 = 380.0;
const STATS_Y: f32 = TODAY_Y + TODAY_H + GUTTER;
const STATS_H: f32 = HEIGHT - STATS_Y - MARGIN;

// ---- drawing -----------------------------------------------------------------

/// 0..1 progress of `t` within [a, a+dur], eased with a cubic out.
fn ease(t: u32, a: u32, dur: u32) -> f32 {
    if t <= a {
        return 0.0;
    }
    let x = ((t - a) as f32 / dur as f32).min(1.0);
    let inv = 1.0 - x;
    1.0 - inv * inv * inv
}

/// Triangle wave in 0..1 with period `p`, for the breathing flame.
fn tri(t: u32, p: u32) -> f32 {
    let m = (t % p) as f32 / p as f32;
    let v = (2.0 * m - 1.0).abs();
    1.0 - v
}

fn draw(canvas: u64, done: [bool; HABIT_COUNT], anim: u32) -> Result<(), gfx::GfxError> {
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    )?;

    // Header: breathing flame dot + wordmark.
    let pulse = tri(anim, 1600);
    let _ = disc(canvas, MARGIN + 9.0, HEADER_BASE - 11.0, 8.0 + 2.5 * pulse, FLAME);
    let _ = disc(canvas, MARGIN + 9.0, HEADER_BASE - 11.0, 3.5, INK);
    text(canvas, "Streak", MARGIN + 28.0, HEADER_BASE, 30.0, INK);
    text_right(canvas, "August 2026", WIDTH - MARGIN, HEADER_BASE - 4.0, 15.0, INK_DIM);

    draw_hero(canvas, anim);
    draw_map(canvas, anim)?;
    draw_today(canvas, done)?;
    draw_stats(canvas)?;
    canvas2d::present(canvas)
}

fn draw_hero(canvas: u64, anim: u32) {
    text(canvas, "CURRENT STREAK", MARGIN, HERO_Y, 12.0, INK_QUIET);
    let mut buf = [0u8; 12];
    // The streak number counts up as the app opens.
    let shown = (STREAK as f32 * ease(anim, 150, 1100)) as u32;
    let s = num(&mut buf, shown.min(STREAK));
    text(canvas, s, MARGIN - 4.0, HERO_Y + 118.0, 132.0, INK);
    let w = est_width(canvas, s, 132.0);
    text(canvas, "days", MARGIN + w + 18.0, HERO_Y + 116.0, 34.0, INK_DIM);
    text(
        canvas,
        "Longest yet. Tonight makes 48.",
        MARGIN + w + 18.0,
        HERO_Y + 150.0,
        16.0,
        INK_QUIET,
    );
}

fn draw_map(canvas: u64, anim: u32) -> Result<(), gfx::GfxError> {
    card(canvas, MARGIN, MAP_Y, LEFT_W, MAP_H)?;
    text(canvas, "The last six months", MARGIN + 24.0, MAP_Y + 40.0, 18.0, INK);
    text(canvas, "Every square is a day", MARGIN + 24.0, MAP_Y + 62.0, 12.5, INK_QUIET);

    let cell = 18.0;
    let gap = 4.0;
    let gx = MARGIN + 24.0;
    let gy = MAP_Y + 84.0;
    for w in 0..WEEKS {
        // Columns sweep in left to right as the app opens.
        let a = ease(anim, 200 + w as u32 * 26, 240);
        if a <= 0.0 {
            continue;
        }
        let sz = cell * a;
        let off = (cell - sz) / 2.0;
        for d in 0..7 {
            let lvl = heat_level(w, d);
            let x = gx + w as f32 * (cell + gap) + off;
            let y = gy + d as f32 * (cell + gap) + off;
            rounded(canvas, x, y, sz, sz, 4.0 * a, HEAT[lvl.min(4)])?;
        }
    }
    // Legend: less -> more.
    let ly = gy + 7.0 * (cell + gap) + 14.0;
    text(canvas, "less", gx, ly + 13.0, 12.0, INK_QUIET);
    for i in 0..5 {
        rounded(canvas, gx + 34.0 + i as f32 * 22.0, ly, 16.0, 16.0, 4.0, HEAT[i])?;
    }
    text(canvas, "more", gx + 34.0 + 5.0 * 22.0 + 6.0, ly + 13.0, 12.0, INK_QUIET);
    Ok(())
}

fn draw_today(canvas: u64, done: [bool; HABIT_COUNT]) -> Result<(), gfx::GfxError> {
    card(canvas, RIGHT_X, TODAY_Y, RIGHT_W, TODAY_H)?;
    text(canvas, "Today", RIGHT_X + 24.0, TODAY_Y + 40.0, 18.0, INK);
    let done_count = done.iter().filter(|d| **d).count() as u32;
    let mut buf = [0u8; 12];
    let mut label = [0u8; 12];
    let d = num(&mut buf, done_count);
    let total = num(&mut label, HABIT_COUNT as u32);
    let mut joined = [0u8; 8];
    let dj = {
        let mut o = 0usize;
        for b in d.as_bytes() {
            if let Some(s) = joined.get_mut(o) {
                *s = *b;
                o += 1;
            }
        }
        if let Some(s) = joined.get_mut(o) {
            *s = b'/';
            o += 1;
        }
        for b in total.as_bytes() {
            if let Some(s) = joined.get_mut(o) {
                *s = *b;
                o += 1;
            }
        }
        core::str::from_utf8(joined.get(..o).unwrap_or(b"0/0")).unwrap_or("0/0")
    };
    text_right(canvas, dj, RIGHT_X + RIGHT_W - 24.0, TODAY_Y + 40.0, 16.0, INK_DIM);

    let rows_y = TODAY_Y + 72.0;
    let row_h = 74.0;
    for i in 0..HABIT_COUNT {
        let y = rows_y + i as f32 * row_h;
        let (name, when) = HABITS[i.min(HABIT_COUNT - 1)];
        // Completion ring: outer disc, inner punch, check disc when done.
        let cx = RIGHT_X + 46.0;
        let cy = y + 26.0;
        if done[i.min(HABIT_COUNT - 1)] {
            disc(canvas, cx, cy, 17.0, GREEN)?;
            disc(canvas, cx, cy, 12.0, CARD)?;
            disc(canvas, cx, cy, 7.0, GREEN)?;
        } else {
            disc(canvas, cx, cy, 17.0, CARD_EDGE)?;
            disc(canvas, cx, cy, 14.0, CARD)?;
        }
        text(canvas, name, RIGHT_X + 76.0, y + 24.0, 17.0, INK);
        text(canvas, when, RIGHT_X + 76.0, y + 45.0, 13.0, INK_QUIET);
        if i + 1 < HABIT_COUNT {
            fill(canvas, RIGHT_X + 24.0, y + row_h - 10.0, RIGHT_W - 48.0, 1.0, CARD_EDGE)?;
        }
    }
    Ok(())
}

fn draw_stats(canvas: u64) -> Result<(), gfx::GfxError> {
    card(canvas, RIGHT_X, STATS_Y, RIGHT_W, STATS_H)?;
    let half = RIGHT_W / 2.0;
    let mut buf = [0u8; 12];
    text(canvas, "BEST", RIGHT_X + 24.0, STATS_Y + 34.0, 11.0, INK_QUIET);
    text(canvas, num(&mut buf, BEST), RIGHT_X + 24.0, STATS_Y + 74.0, 40.0, INK);
    let mut buf2 = [0u8; 12];
    text(canvas, "THIS YEAR", RIGHT_X + half + 8.0, STATS_Y + 34.0, 11.0, INK_QUIET);
    let days = num(&mut buf2, YEAR_DAYS);
    text(canvas, days, RIGHT_X + half + 8.0, STATS_Y + 74.0, 40.0, ACCENT);
    let dw = est_width(canvas, days, 40.0);
    text(canvas, "days", RIGHT_X + half + 8.0 + dw + 8.0, STATS_Y + 74.0, 16.0, INK_DIM);
    Ok(())
}

// ---- ui tree -----------------------------------------------------------------

fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
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

// ---- persistence -------------------------------------------------------------

fn load_done() -> [bool; HABIT_COUNT] {
    let mut done = [true, true, false, true];
    if let Ok(Some(bytes)) = store_kv::get(DONE_KEY) {
        for i in 0..HABIT_COUNT {
            if let Some(b) = bytes.get(i) {
                done[i] = *b != 0;
            }
        }
    }
    done
}

fn save_done(done: [bool; HABIT_COUNT]) {
    let bytes: [u8; HABIT_COUNT] = [
        done[0] as u8,
        done[1] as u8,
        done[2] as u8,
        done[3] as u8,
    ];
    let _ = store_kv::set(DONE_KEY, &bytes);
}

fn hit_habit(x: f32, y: f32) -> Option<usize> {
    if x < RIGHT_X + 12.0 || x > RIGHT_X + RIGHT_W - 12.0 {
        return None;
    }
    let rows_y = TODAY_Y + 72.0;
    let row_h = 74.0;
    for i in 0..HABIT_COUNT {
        let ry = rows_y + i as f32 * row_h - 6.0;
        if y >= ry && y < ry + row_h - 12.0 {
            return Some(i);
        }
    }
    None
}

// ---- app ---------------------------------------------------------------------

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Streak", size) else {
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
        let _ = canvas2d::set_design_size(canvas, gfx::Size { width: WIDTH, height: HEIGHT });

        let mut done = load_done();

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if quick {
            // The automated shot: fully opened, mid flame breath.
            let _ = draw(canvas, [true, true, false, true], 2600);
            let out = stdio::stdout();
            let _ = out.write(b"streak:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        let _ = draw(canvas, done, 0);

        // The wall-clock for the open animation and the breathing flame:
        // each wait round is one 33 ms tick, so the app stays alive on
        // screen instead of freezing into a screenshot of itself.
        let mut anim: u32 = 0;
        let mut idle = 0u32;
        for _ in 0..MAX_WAIT_ROUNDS {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::Pointer(p)) if p.pressed => {
                    idle = 0;
                    if let Some(i) = hit_habit(p.x, p.y) {
                        if let Some(slot) = done.get_mut(i) {
                            *slot = !*slot;
                        }
                        save_done(done);
                    }
                }
                Some(types::Event::CloseRequested(id)) if id == win => break,
                Some(_) => idle = 0,
                None => {
                    idle += 1;
                    if quick && idle > MAX_IDLE_ROUNDS * 20 {
                        break;
                    }
                }
            }
            anim = anim.saturating_add(WAIT_ROUND_MILLIS);
            let _ = draw(canvas, done, anim);
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"streak:ok\n");
        let _ = out.flush();
        0
    }
}

bindings::export!(Component with_types_in bindings);
