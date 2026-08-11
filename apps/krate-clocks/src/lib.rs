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

const WIDTH: f32 = 880.0;
const HEIGHT: f32 = 560.0;

const PAD: f32 = 24.0;
const GRID_TOP: f32 = 84.0;
const GAP: f32 = 20.0;
const CARD_W: f32 = (WIDTH - PAD * 2.0 - GAP * 2.0) / 3.0; // 264
const CARD_H: f32 = (HEIGHT - GRID_TOP - PAD - GAP) / 2.0; // 216

const FACE_R: f32 = 64.0;

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
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"clocks:ok\n");
            let _ = window::close(win);
            return 0;
        }

        // The quick path already returned above, so this loop is only ever
        // a real session: it ends when the person closes the window, not
        // on a count. The old one-hour bound closed a clock somebody was
        // still watching (K-092).
        loop {
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

fn draw(canvas: u64, local: Option<i64>) -> Result<(), gfx::GfxError> {
    let now_secs = (clock::now_millis() / 1000) as i64;

    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    )?;

    draw_header(canvas, now_secs, local)?;

    // The offset chips read against the header clock: local when we know it,
    // UTC otherwise.
    let base_off = local.unwrap_or(0);

    for (i, (name, off)) in CITIES.iter().enumerate() {
        let col = (i % 3) as f32;
        let row = (i / 3) as f32;
        let x = PAD + col * (CARD_W + GAP);
        let y = GRID_TOP + row * (CARD_H + GAP);
        draw_card(canvas, x, y, name, *off, base_off, now_secs)?;
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
    let tw = text_width(canvas, title, tsize);
    draw_text(canvas, buf.as_str(), PAD + tw + 18.0, baseline, 15.0, INK_DIM)?;
    Ok(())
}

// ------------------------------------------------------------------
// One city card
// ------------------------------------------------------------------

fn draw_card(
    canvas: u64,
    x: f32,
    y: f32,
    name: &str,
    off_minutes: i64,
    base_off: i64,
    now_secs: i64,
) -> Result<(), gfx::GfxError> {
    // Hairline border: a border-color rounded rect one pixel proud of the card.
    rounded_rect(canvas, x - 1.0, y - 1.0, CARD_W + 2.0, CARD_H + 2.0, 17.0, HAIRLINE)?;
    rounded_rect(canvas, x, y, CARD_W, CARD_H, 16.0, CARD)?;

    let sod = day_seconds(now_secs, off_minutes);
    let hour = sod / 3600;
    let is_day = (6..18).contains(&hour);

    let cx = x + CARD_W * 0.5;
    let cy = y + 14.0 + FACE_R;

    draw_face(canvas, cx, cy, is_day)?;
    draw_hands(canvas, cx, cy, sod)?;

    // Day/night mark in the card's top-right corner.
    if is_day {
        draw_sun(canvas, x + CARD_W - 24.0, y + 24.0)?;
    } else {
        draw_moon(canvas, x + CARD_W - 24.0, y + 24.0)?;
    }

    // City name.
    let nsize = 16.0;
    let nw = text_width(canvas, name, nsize);
    let nx = cx - nw * 0.5;
    let ny = y + 168.0;
    draw_text(canvas, name, nx, ny, nsize, INK)?;
    draw_text(canvas, name, nx + 0.5, ny, nsize, INK)?; // faux semibold

    // Digital time + offset chip, one centered row.
    let mut tb = TextBuf::new();
    push_hhmm(&mut tb, sod);
    let tsize = 14.0;
    let tw = text_width(canvas, tb.as_str(), tsize);

    let mut cb = TextBuf::new();
    push_offset(&mut cb, off_minutes - base_off);
    let csize = 11.0;
    let ctw = text_width(canvas, cb.as_str(), csize);
    let chip_w = ctw + 16.0;
    let chip_h = 18.0;

    let gap = 10.0;
    let total = tw + gap + chip_w;
    let left = cx - total * 0.5;
    let row_baseline = y + 192.0;

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

fn draw_face(canvas: u64, cx: f32, cy: f32, is_day: bool) -> Result<(), gfx::GfxError> {
    // Soft drop shadow, then a 2px rim ring, then the face itself.
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: cx, y: cy + 4.0 },
        FACE_R + 10.0,
        gfx::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.30 },
        gfx::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.0 },
    )?;
    disc(canvas, cx, cy, FACE_R + 2.0, HAIRLINE)?;
    disc(canvas, cx, cy, FACE_R, if is_day { FACE_DAY } else { FACE_NIGHT })?;

    // A whisper of top light so the face reads as a dial, not a flat disc.
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: cx, y: cy - FACE_R * 0.55 },
        FACE_R * 1.05,
        gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: if is_day { 0.06 } else { 0.04 } },
        gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 0.0 },
    )?;

    // Twelve hour ticks; the cardinals sit a touch heavier.
    for k in 0..12usize {
        let (dx, dy) = dial_dir((k * 5) as f32);
        let cardinal = k % 3 == 0;
        let (r, c) = if cardinal {
            (2.4, INK_DIM)
        } else {
            (1.6, INK_QUIET)
        };
        disc(canvas, cx + dx * 55.0, cy + dy * 55.0, r, c)?;
    }
    Ok(())
}

fn draw_hands(canvas: u64, cx: f32, cy: f32, sod: i64) -> Result<(), gfx::GfxError> {
    let h = (sod / 3600) % 12;
    let m = (sod / 60) % 60;
    let s = sod % 60;

    let hour_pos = (h * 5) as f32 + m as f32 / 12.0;
    let min_pos = m as f32 + s as f32 / 60.0;
    let sec_pos = s as f32;

    // Hour: thick and short. Minute: thin and long. Second: accent hairline.
    hand(canvas, cx, cy, hour_pos, -6.0, 34.0, 2.9, INK)?;
    hand(canvas, cx, cy, min_pos, -8.0, 52.0, 1.7, INK)?;
    hand(canvas, cx, cy, sec_pos, -10.0, 56.0, 0.9, ACCENT)?;

    // Center cap: accent ring with a dark core, like a real movement.
    disc(canvas, cx, cy, 4.2, ACCENT)?;
    disc(canvas, cx, cy, 1.6, BG_TOP)?;
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
