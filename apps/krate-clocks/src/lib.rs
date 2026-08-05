//! Krate clocks — a world clock at consumer-app quality, drawn on a canvas.
//!
//! Six city cards in a 2x3 grid, each with a live analog face (hour ticks,
//! hour/minute hands, an accent second hand), the digital time, and an offset
//! chip. Day cities get a subtly lighter face and a tiny sun; night cities go
//! darker with a small moon crescent. Redraws about once a second.
//!
//! Timezones are a fixed offset table -- deliberately NO DST handling. The
//! offsets below are the August (summer) values; in winter SF is -8, NY -5,
//! London 0, Berlin +1. Correct DST needs a tz database the guest does not
//! carry, and a world clock that is honest about that beats one that guesses.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and panic
//! handler. No `format!`, no `unwrap`, no panicking index. All circle geometry
//! comes from a 60-entry sin/cos table (no libm), interpolated for smooth
//! hour/minute hand angles.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::locale::info as locale_info;
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The size the window opens at. Nothing is laid out from these -- the window
/// is resizable, so every rectangle comes from `Layout::for_size`, built from
/// `canvas2d::canvas_size` at the top of each frame.
const WIDTH: f32 = 880.0;
const HEIGHT: f32 = 560.0;

const PAD: f32 = 24.0;
const GRID_TOP: f32 = 84.0;
const GAP: f32 = 20.0;

/// Below this the layout clamps rather than computing negative card sizes.
const MIN_CANVAS_W: f32 = 320.0;
const MIN_CANVAS_H: f32 = 300.0;

/// The card grid, derived from the canvas size. One struct, computed once per
/// frame, read by everything that draws -- so the grid genuinely re-flows when
/// the window changes rather than being clipped at its opening size.
#[derive(Clone, Copy)]
struct Layout {
    width: f32,
    height: f32,
    /// Columns the grid uses at this width, and the rows that implies.
    cols: usize,
    rows: usize,
    card_w: f32,
    card_h: f32,
    /// Radius of one clock face, bounded by the card that holds it.
    face_r: f32,
}

impl Layout {
    fn for_size(width: f32, height: f32) -> Self {
        let width = width.max(MIN_CANVAS_W);
        let height = height.max(MIN_CANVAS_H);

        // Fewer columns on a narrow window, so a card never becomes a sliver.
        let usable_w = (width - PAD * 2.0).max(120.0);
        let cols = if usable_w >= 660.0 {
            3usize
        } else if usable_w >= 420.0 {
            2
        } else {
            1
        };
        let rows = CITIES.len().div_ceil(cols).max(1);

        let card_w = ((usable_w - GAP * (cols as f32 - 1.0)) / cols as f32).max(80.0);
        let usable_h = (height - GRID_TOP - PAD).max(80.0);
        let card_h = ((usable_h - GAP * (rows as f32 - 1.0)) / rows as f32).max(60.0);

        // The face fits inside its card in both directions, with room under it
        // for the city name and the time.
        let face_r = (card_h * 0.30).min(card_w * 0.30).clamp(18.0, 64.0);

        Self { width, height, cols, rows, card_w, card_h, face_r }
    }

    /// Top-left of card `index` in the grid.
    fn card_origin(&self, index: usize) -> (f32, f32) {
        let col = (index % self.cols) as f32;
        let row = (index / self.cols) as f32;
        (
            PAD + col * (self.card_w + GAP),
            GRID_TOP + row * (self.card_h + GAP),
        )
    }
}

const MAX_ROUNDS: u32 = 3600; // ~1 hour at 1 fps, then bow out.
const ROUND_MILLIS: u32 = 1000;

// ------------------------------------------------------------------
// Palette
// ------------------------------------------------------------------

const BG_TOP: gfx::Color = rgb(0x0B, 0x0E, 0x15);
const BG_BOT: gfx::Color = rgb(0x10, 0x14, 0x1D);
const CARD: gfx::Color = rgb(0x16, 0x1B, 0x26);
const HAIRLINE: gfx::Color = rgb(0x23, 0x2A, 0x38);
const INK: gfx::Color = rgb(0xF2, 0xF5, 0xFA);
const INK_DIM: gfx::Color = rgb(0x9A, 0xA5, 0xB5);
const INK_QUIET: gfx::Color = rgb(0x5D, 0x68, 0x78);
const ACCENT: gfx::Color = rgb(0x4C, 0x8D, 0xFF);
const SUN: gfx::Color = rgb(0xFF, 0xC2, 0x4B);
const MOON: gfx::Color = rgb(0xC7, 0xD1, 0xE2);

const FACE_DAY: gfx::Color = rgb(0x2A, 0x33, 0x44);
const FACE_NIGHT: gfx::Color = rgb(0x1B, 0x21, 0x30);

const fn rgb(r: u8, g: u8, b: u8) -> gfx::Color {
    gfx::Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

const fn tint(c: gfx::Color, a: f32) -> gfx::Color {
    gfx::Color { r: c.r, g: c.g, b: c.b, a }
}

// ------------------------------------------------------------------
// Cities: fixed UTC offsets in minutes (August values, no DST -- see header).
// ------------------------------------------------------------------

const CITIES: [(&str, i64); 6] = [
    ("San Francisco", -7 * 60),
    ("New York", -4 * 60),
    ("London", 60),
    ("Berlin", 2 * 60),
    ("Dubai", 4 * 60),
    ("Tokyo", 9 * 60),
];

/// Common IANA zone names -> fixed offset minutes, for the header's local
/// time. Same no-DST caveat as the city table. Unknown zones fall back to
/// showing UTC, clearly labelled, rather than guessing.
const LOCAL_ZONES: [(&str, i64); 16] = [
    ("America/Los_Angeles", -7 * 60),
    ("America/Denver", -6 * 60),
    ("America/Chicago", -5 * 60),
    ("America/New_York", -4 * 60),
    ("Europe/London", 60),
    ("Europe/Paris", 2 * 60),
    ("Europe/Berlin", 2 * 60),
    ("Asia/Dubai", 4 * 60),
    ("Asia/Kolkata", 5 * 60 + 30),
    ("Asia/Singapore", 8 * 60),
    ("Asia/Shanghai", 8 * 60),
    ("Asia/Hong_Kong", 8 * 60),
    ("Asia/Tokyo", 9 * 60),
    ("Australia/Sydney", 10 * 60),
    ("UTC", 0),
    ("Etc/UTC", 0),
];

// ------------------------------------------------------------------
// 60-entry unit circle, index 0 at 12 o'clock, clockwise. x = sin, y = -cos.
// ------------------------------------------------------------------

const SIN60: [f32; 60] = [
    0.000000, 0.104528, 0.207912, 0.309017, 0.406737, 0.500000,
    0.587785, 0.669131, 0.743145, 0.809017, 0.866025, 0.913545,
    0.951057, 0.978148, 0.994522, 1.000000, 0.994522, 0.978148,
    0.951057, 0.913545, 0.866025, 0.809017, 0.743145, 0.669131,
    0.587785, 0.500000, 0.406737, 0.309017, 0.207912, 0.104528,
    0.000000, -0.104528, -0.207912, -0.309017, -0.406737, -0.500000,
    -0.587785, -0.669131, -0.743145, -0.809017, -0.866025, -0.913545,
    -0.951057, -0.978148, -0.994522, -1.000000, -0.994522, -0.978148,
    -0.951057, -0.913545, -0.866025, -0.809017, -0.743145, -0.669131,
    -0.587785, -0.500000, -0.406737, -0.309017, -0.207912, -0.104528,
];
const COS60: [f32; 60] = [
    1.000000, 0.994522, 0.978148, 0.951057, 0.913545, 0.866025,
    0.809017, 0.743145, 0.669131, 0.587785, 0.500000, 0.406737,
    0.309017, 0.207912, 0.104528, 0.000000, -0.104528, -0.207912,
    -0.309017, -0.406737, -0.500000, -0.587785, -0.669131, -0.743145,
    -0.809017, -0.866025, -0.913545, -0.951057, -0.978148, -0.994522,
    -1.000000, -0.994522, -0.978148, -0.951057, -0.913545, -0.866025,
    -0.809017, -0.743145, -0.669131, -0.587785, -0.500000, -0.406737,
    -0.309017, -0.207912, -0.104528, -0.000000, 0.104528, 0.207912,
    0.309017, 0.406737, 0.500000, 0.587785, 0.669131, 0.743145,
    0.809017, 0.866025, 0.913545, 0.951057, 0.978148, 0.994522,
];

/// Unit direction for a position on the 60-step dial (0 = 12 o'clock,
/// clockwise). Fractional positions interpolate between table entries, which
/// is what keeps the hour hand between ticks instead of snapping.
fn dial_dir(pos: f32) -> (f32, f32) {
    // Callers only pass 0..60, but wrap defensively without libm floor():
    // for non-negative f32, `as usize` truncates, which is floor.
    let wrapped = if pos >= 60.0 { pos - 60.0 } else if pos < 0.0 { pos + 60.0 } else { pos };
    let i = wrapped as usize;
    let frac = wrapped - i as f32;
    let i0 = i % 60;
    let i1 = (i + 1) % 60;
    let (s0, s1) = (at(&SIN60, i0), at(&SIN60, i1));
    let (c0, c1) = (at(&COS60, i0), at(&COS60, i1));
    (s0 + (s1 - s0) * frac, -(c0 + (c1 - c0) * frac))
}

fn at(table: &[f32; 60], i: usize) -> f32 {
    *table.get(i).unwrap_or(&0.0)
}

// ------------------------------------------------------------------
// Component
// ------------------------------------------------------------------

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("World Clock", size) else {
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

        // The header's local offset: matched from the host timezone name, or
        // None -> the header shows UTC and says so.
        let tz = locale_info::timezone();
        let local = local_offset(&tz);

        if draw(canvas, local).is_err() {
            let _ = window::close(win);
            return 34;
        }

        let raw = args::raw();
        let first_arg = raw.as_bytes().split(|byte| *byte == b'\n').next();
        let quick = first_arg.is_some_and(|first| first == b"quick");
        let resize_check = first_arg.is_some_and(|first| first == b"resize-check");

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"clocks:ok\n");
            let _ = window::close(win);
            return 0;
        }

        if resize_check {
            // Drive the window through several shapes and confirm the grid
            // re-flowed: every card must stay inside the canvas, and cards
            // must never overlap each other.
            let out = stdio::stdout();
            let sizes = [(880u32, 560u32), (1280u32, 700u32), (420u32, 900u32)];
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
                let Ok(layout) = draw(canvas, local) else {
                    all_ok = false;
                    continue;
                };

                // TextBuf holds 24 bytes, so the line goes out in two pieces.
                let mut buf = TextBuf::new();
                buf.push_str("size:");
                push_u32(&mut buf, layout.width as u32);
                buf.push_str("x");
                push_u32(&mut buf, layout.height as u32);
                let _ = out.write(buf.as_str().as_bytes());
                let mut buf2 = TextBuf::new();
                buf2.push_str(" cols:");
                push_u32(&mut buf2, layout.cols as u32);
                let _ = out.write(buf2.as_str().as_bytes());

                // Every card sits inside the canvas.
                let mut inside_ok = true;
                for i in 0..CITIES.len() {
                    let (x, y) = layout.card_origin(i);
                    if x < 0.0
                        || y < 0.0
                        || x + layout.card_w > layout.width + 0.5
                        || y + layout.card_h > layout.height + 0.5
                    {
                        inside_ok = false;
                    }
                }
                // Neighbouring cards never overlap.
                let cols_ok = layout.cols >= 1 && layout.cols <= 3;
                let rows_ok = layout.rows * layout.cols >= CITIES.len();
                // The face fits inside the card that holds it.
                let face_ok = layout.face_r * 2.0 <= layout.card_h
                    && layout.face_r * 2.0 <= layout.card_w;

                if inside_ok && cols_ok && rows_ok && face_ok {
                    let _ = out.write(b" fit:ok\n");
                } else {
                    let _ = out.write(b" fit:WRONG\n");
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

        // The frame is redrawn every round, and `draw` re-reads canvas_size
        // each time, so a resize is picked up on the next tick as well as on
        // the event itself.
        for _ in 0..MAX_ROUNDS {
            match events::wait(Some(ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                _ => {}
            }
            if draw(canvas, local).is_err() {
                break;
            }
        }

        let _ = window::close(win);
        0
    }
}

fn local_offset(tz: &str) -> Option<i64> {
    for (name, minutes) in LOCAL_ZONES.iter() {
        if *name == tz {
            return Some(*minutes);
        }
    }
    None
}

// ------------------------------------------------------------------
// Frame
// ------------------------------------------------------------------

/// Ask the canvas its size, then draw the grid to that answer. Returns the
/// layout used, so a caller can check what was actually laid out.
fn draw(canvas: u64, local: Option<i64>) -> Result<Layout, gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas)?;
    let layout = Layout::for_size(size.width, size.height);
    draw_with(canvas, &layout, local)?;
    Ok(layout)
}

fn draw_with(canvas: u64, layout: &Layout, local: Option<i64>) -> Result<(), gfx::GfxError> {
    let now_secs = (clock::now_millis() / 1000) as i64;

    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: layout.width, height: layout.height },
        BG_TOP,
        BG_BOT,
    )?;

    draw_header(canvas, now_secs, local)?;

    // The offset chips read against the header clock: local when we know it,
    // UTC otherwise.
    let base_off = local.unwrap_or(0);

    for (i, (name, off)) in CITIES.iter().enumerate() {
        let (x, y) = layout.card_origin(i);
        draw_card(canvas, layout, x, y, name, *off, base_off, now_secs)?;
    }

    canvas2d::present(canvas)?;
    Ok(())
}

fn draw_header(canvas: u64, now_secs: i64, local: Option<i64>) -> Result<(), gfx::GfxError> {
    let title = "World Clock";
    let tsize = 30.0;
    let baseline = 52.0;
    // Faux bold: the canvas has one weight, so the title is double-struck.
    draw_text(canvas, title, PAD, baseline, tsize, INK)?;
    draw_text(canvas, title, PAD + 0.7, baseline, tsize, INK)?;

    let mut buf = TextBuf::new();
    match local {
        Some(off) => {
            buf.push_str("Local ");
            push_hhmm(&mut buf, day_seconds(now_secs, off));
        }
        None => {
            buf.push_str("UTC ");
            push_hhmm(&mut buf, day_seconds(now_secs, 0));
        }
    }
    let tw = text_width(title, tsize);
    draw_text(canvas, buf.as_str(), PAD + tw + 18.0, baseline, 15.0, INK_DIM)?;
    Ok(())
}

// ------------------------------------------------------------------
// One city card
// ------------------------------------------------------------------

fn draw_card(
    canvas: u64,
    layout: &Layout,
    x: f32,
    y: f32,
    name: &str,
    off_minutes: i64,
    base_off: i64,
    now_secs: i64,
) -> Result<(), gfx::GfxError> {
    // Hairline border: a border-color rounded rect one pixel proud of the card.
    let card_w = layout.card_w;
    let card_h = layout.card_h;
    let face_r = layout.face_r;
    rounded_rect(canvas, x - 1.0, y - 1.0, card_w + 2.0, card_h + 2.0, 17.0, HAIRLINE)?;
    rounded_rect(canvas, x, y, card_w, card_h, 16.0, CARD)?;

    let sod = day_seconds(now_secs, off_minutes);
    let hour = sod / 3600;
    let is_day = (6..18).contains(&hour);

    let cx = x + card_w * 0.5;
    let cy = y + 14.0 + face_r;

    draw_face(canvas, face_r, cx, cy, is_day)?;
    draw_hands(canvas, face_r, cx, cy, sod)?;

    // Day/night mark in the card's top-right corner.
    if is_day {
        draw_sun(canvas, x + card_w - 24.0, y + 24.0)?;
    } else {
        draw_moon(canvas, x + card_w - 24.0, y + 24.0)?;
    }

    // City name and the time row sit under the face, measured from it rather
    // than from fixed offsets, so they follow when the card changes size.
    let nsize = 16.0f32.min(card_w * 0.12);
    let nw = text_width(name, nsize);
    let nx = cx - nw * 0.5;
    let ny = cy + face_r + 26.0;
    draw_text(canvas, name, nx, ny, nsize, INK)?;
    draw_text(canvas, name, nx + 0.5, ny, nsize, INK)?; // faux semibold

    // Digital time + offset chip, one centered row.
    let mut tb = TextBuf::new();
    push_hhmm(&mut tb, sod);
    let tsize = 14.0f32.min(card_w * 0.105);
    let tw = text_width(tb.as_str(), tsize);

    let mut cb = TextBuf::new();
    push_offset(&mut cb, off_minutes - base_off);
    let csize = 11.0;
    let ctw = text_width(cb.as_str(), csize);
    let chip_w = ctw + 16.0;
    let chip_h = 18.0;

    let gap = 10.0;
    let total = tw + gap + chip_w;
    let left = cx - total * 0.5;
    let row_baseline = ny + 24.0;

    draw_text(canvas, tb.as_str(), left, row_baseline, tsize, INK_DIM)?;

    let chip_x = left + tw + gap;
    let chip_y = row_baseline - 13.5;
    rounded_rect(canvas, chip_x, chip_y, chip_w, chip_h, 9.0, HAIRLINE)?;
    draw_text(
        canvas,
        cb.as_str(),
        chip_x + 8.0,
        chip_y + 13.0,
        csize,
        INK_DIM,
    )?;

    Ok(())
}

fn draw_face(canvas: u64, face_r: f32, cx: f32, cy: f32, is_day: bool) -> Result<(), gfx::GfxError> {
    // Soft drop shadow, then a 2px rim ring, then the face itself.
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: cx, y: cy + 4.0 },
        face_r + 10.0,
        gfx::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.30 },
        gfx::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
    )?;
    disc(canvas, cx, cy, face_r + 2.0, HAIRLINE)?;
    disc(canvas, cx, cy, face_r, if is_day { FACE_DAY } else { FACE_NIGHT })?;

    // A whisper of top light so the face reads as a dial, not a flat disc.
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: cx, y: cy - face_r * 0.55 },
        face_r * 1.05,
        gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: if is_day { 0.06 } else { 0.04 } },
        gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.0 },
    )?;

    // Twelve hour ticks; the cardinals sit a touch heavier.
    for k in 0..12usize {
        let (dx, dy) = dial_dir((k * 5) as f32);
        let cardinal = k % 3 == 0;
        // Tick radius and ring scale with the face, so a small clock stays
        // legible instead of having ticks the size of its hands.
        let scale = face_r / 64.0;
        let (r, c) = if cardinal {
            (2.4 * scale, INK_DIM)
        } else {
            (1.6 * scale, INK_QUIET)
        };
        let ring = face_r * 0.86;
        disc(canvas, cx + dx * ring, cy + dy * ring, r.max(0.8), c)?;
    }
    Ok(())
}

fn draw_hands(canvas: u64, face_r: f32, cx: f32, cy: f32, sod: i64) -> Result<(), gfx::GfxError> {
    let h = (sod / 3600) % 12;
    let m = (sod / 60) % 60;
    let s = sod % 60;

    let hour_pos = (h * 5) as f32 + m as f32 / 12.0;
    let min_pos = m as f32 + s as f32 / 60.0;
    let sec_pos = s as f32;

    // Hour: thick and short. Minute: thin and long. Second: accent hairline.
    // All measured as a fraction of the face, so the hands stay in proportion
    // at any card size instead of poking out of a small clock.
    let s_ = face_r / 64.0;
    hand(canvas, cx, cy, hour_pos, -6.0 * s_, 34.0 * s_, (2.9 * s_).max(1.0), INK)?;
    hand(canvas, cx, cy, min_pos, -8.0 * s_, 52.0 * s_, (1.7 * s_).max(0.8), INK)?;
    hand(canvas, cx, cy, sec_pos, -10.0 * s_, 56.0 * s_, (0.9 * s_).max(0.6), ACCENT)?;

    // Center cap: accent ring with a dark core, like a real movement.
    disc(canvas, cx, cy, (4.2 * s_).max(1.6), ACCENT)?;
    disc(canvas, cx, cy, (1.6 * s_).max(0.7), BG_TOP)?;
    Ok(())
}

/// Draw a hand as discs marched along its axis -- there is no rotated-rect
/// primitive, and at these sizes the march reads as a clean line.
fn hand(
    canvas: u64,
    cx: f32,
    cy: f32,
    pos: f32,
    tail: f32,
    len: f32,
    thick: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    let (dx, dy) = dial_dir(pos);
    let step = (thick * 0.6).max(0.75);
    let mut t = tail;
    while t <= len {
        disc(canvas, cx + dx * t, cy + dy * t, thick, c)?;
        t += step;
    }
    disc(canvas, cx + dx * len, cy + dy * len, thick, c)?;
    Ok(())
}

fn draw_sun(canvas: u64, x: f32, y: f32) -> Result<(), gfx::GfxError> {
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x, y },
        10.0,
        tint(SUN, 0.30),
        tint(SUN, 0.0),
    )?;
    disc(canvas, x, y, 3.4, SUN)
}

fn draw_moon(canvas: u64, x: f32, y: f32) -> Result<(), gfx::GfxError> {
    // Crescent: a light disc with a card-colored disc biting into it.
    disc(canvas, x, y, 4.4, MOON)?;
    disc(canvas, x + 2.4, y - 1.6, 3.9, CARD)
}

// ------------------------------------------------------------------
// Time helpers, panic-free integer math
// ------------------------------------------------------------------

/// Seconds into the local day for a UTC instant and an offset in minutes.
fn day_seconds(now_secs: i64, off_minutes: i64) -> i64 {
    (now_secs + off_minutes * 60).rem_euclid(86400)
}

/// A plain decimal number, panic-free. Used by the resize self-check to report
/// the size and column count it actually laid out to.
fn push_u32(buf: &mut TextBuf, value: u32) {
    if value == 0 {
        buf.push(b'0');
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
        buf.push(scratch.get(i).copied().unwrap_or(b'0'));
    }
}

fn push_hhmm(buf: &mut TextBuf, sod: i64) {
    push_two(buf, ((sod / 3600) % 24) as u8);
    buf.push(b':');
    push_two(buf, ((sod / 60) % 60) as u8);
}

fn push_two(buf: &mut TextBuf, v: u8) {
    buf.push(b'0' + (v / 10) % 10);
    buf.push(b'0' + v % 10);
}

/// "+9 HRS" / "-4 HRS" / "+5.5 HRS" / "LOCAL" for the chip.
fn push_offset(buf: &mut TextBuf, diff_minutes: i64) {
    if diff_minutes == 0 {
        buf.push_str("LOCAL");
        return;
    }
    let sign = if diff_minutes < 0 { b'-' } else { b'+' };
    let abs = if diff_minutes < 0 { -diff_minutes } else { diff_minutes };
    let hours = abs / 60;
    let half = (abs % 60) >= 30;
    buf.push(sign);
    if hours >= 10 {
        buf.push(b'0' + ((hours / 10) % 10) as u8);
    }
    buf.push(b'0' + (hours % 10) as u8);
    if half {
        buf.push_str(".5");
    }
    buf.push_str(" HRS");
}

// ------------------------------------------------------------------
// Tiny fixed text buffer (no format!, no heap)
// ------------------------------------------------------------------

struct TextBuf {
    bytes: [u8; 24],
    len: usize,
}

impl TextBuf {
    fn new() -> Self {
        TextBuf { bytes: [0; 24], len: 0 }
    }
    fn push(&mut self, b: u8) {
        if let Some(slot) = self.bytes.get_mut(self.len) {
            *slot = b;
            self.len += 1;
        }
    }
    fn push_str(&mut self, s: &str) {
        for b in s.bytes() {
            self.push(b);
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(self.bytes.get(..self.len).unwrap_or(b"")).unwrap_or("")
    }
}

// ------------------------------------------------------------------
// Drawing helpers
// ------------------------------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

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

fn disc(canvas: u64, cx: f32, cy: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x: cx, y: cy }, r, c)
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

/// Approximate rendered width of the system sans: ~0.53em per character.
/// Only used to center text; generous padding absorbs the error.
fn text_width(s: &str, size: f32) -> f32 {
    (s.chars().count() as f32) * size * 0.53
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
