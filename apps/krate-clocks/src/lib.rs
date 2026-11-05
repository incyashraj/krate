//! Krate clocks -- a world clock at consumer-app quality, drawn on a canvas.
//!
//! Six cities, each with a live analog face (hour ticks, hour and minute
//! hands, an accent second hand), the digital time, and how far it is from
//! here. A day city gets a light face and a small sun; a night city a dark
//! face and a crescent moon. Redraws about once a second.
//!
//! It is laid out from the canvas it really has, every frame. A phone held
//! upright gets one column of rows, the way a phone's own clock app shows
//! them: a small face, the city, and the time large at the right. A phone
//! turned sideways gets the same rows in two columns. A tablet and a desktop
//! window get cards with big faces, two or three across. Colours follow the
//! person's light or dark mode, every font size follows their text size, and
//! each frame tells the screen reader what it shows.
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

extern crate alloc;

use alloc::string::String;
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::locale::info as locale_info;
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said, Screen};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The window asked for on a desktop. Only a first size: every frame is laid
/// out from the canvas the app really has.
const WIDTH: f32 = 880.0;
const HEIGHT: f32 = 560.0;

const ROUND_MILLIS: u32 = 1000;

// ------------------------------------------------------------------
// Colours, for light and dark
// ------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Theme {
    bg_top: gfx::Color,
    bg_bot: gfx::Color,
    card: gfx::Color,
    edge: gfx::Color,
    ink: gfx::Color,
    ink2: gfx::Color,
    ink3: gfx::Color,
    accent: gfx::Color,
    shadow: gfx::Color,
    sun: gfx::Color,
    moon: gfx::Color,
    day: Face,
    night: Face,
}

/// The colours of one clock face: a day face and a night face differ.
#[derive(Clone, Copy)]
struct Face {
    fill: gfx::Color,
    rim: gfx::Color,
    hand: gfx::Color,
    tick: gfx::Color,
    tick_minor: gfx::Color,
    gloss: f32,
}

fn theme(look: &Look) -> Theme {
    let contrast = look.high_contrast;
    // A night face is dark in a light room as in a dark one, and a day face
    // light: the face says day or night before the time is read.
    let night = Face {
        fill: rgb(0x1B, 0x21, 0x30),
        rim: rgb(0x2E, 0x36, 0x47),
        hand: rgb(0xF2, 0xF5, 0xFA),
        tick: rgb(0xB4, 0xBE, 0xCC),
        tick_minor: rgb(0x6A, 0x75, 0x86),
        gloss: 0.05,
    };
    if look.dark {
        Theme {
            bg_top: rgb(0x0B, 0x0E, 0x15),
            bg_bot: rgb(0x10, 0x14, 0x1D),
            card: rgb(0x16, 0x1B, 0x26),
            edge: if contrast {
                rgb(0x4A, 0x55, 0x68)
            } else {
                rgb(0x23, 0x2A, 0x38)
            },
            ink: rgb(0xF2, 0xF5, 0xFA),
            ink2: if contrast {
                rgb(0xDC, 0xE2, 0xEA)
            } else {
                rgb(0xA6, 0xB0, 0xBF)
            },
            ink3: if contrast {
                rgb(0xC4, 0xCC, 0xD8)
            } else {
                rgb(0x7A, 0x85, 0x96)
            },
            accent: rgb(0x4C, 0x8D, 0xFF),
            shadow: gfx::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.30,
            },
            sun: rgb(0xFF, 0xC2, 0x4B),
            moon: rgb(0xC7, 0xD1, 0xE2),
            // A day face stays light in dark mode, a little dimmed, so day
            // and night still read apart at a glance.
            day: Face {
                fill: rgb(0xD6, 0xDC, 0xE5),
                rim: rgb(0x3C, 0x47, 0x5C),
                hand: rgb(0x14, 0x18, 0x22),
                tick: rgb(0x3A, 0x43, 0x52),
                tick_minor: rgb(0x8E, 0x97, 0xA6),
                gloss: 0.0,
            },
            night,
        }
    } else {
        Theme {
            bg_top: rgb(0xF6, 0xF7, 0xFA),
            bg_bot: rgb(0xEC, 0xEF, 0xF4),
            card: rgb(0xFF, 0xFF, 0xFF),
            edge: if contrast {
                rgb(0x9A, 0xA3, 0xB2)
            } else {
                rgb(0xE1, 0xE5, 0xEC)
            },
            ink: rgb(0x12, 0x15, 0x1C),
            ink2: if contrast {
                rgb(0x2C, 0x33, 0x3F)
            } else {
                rgb(0x55, 0x5E, 0x6D)
            },
            ink3: if contrast {
                rgb(0x3E, 0x46, 0x53)
            } else {
                rgb(0x7E, 0x87, 0x95)
            },
            accent: rgb(0x2F, 0x6F, 0xEB),
            shadow: gfx::Color {
                r: 0.07,
                g: 0.09,
                b: 0.15,
                a: 0.10,
            },
            sun: rgb(0xF2, 0xA1, 0x1C),
            moon: rgb(0x8E, 0x9A, 0xB0),
            day: Face {
                fill: rgb(0xF4, 0xF6, 0xF9),
                rim: rgb(0xDC, 0xE1, 0xE9),
                hand: rgb(0x14, 0x18, 0x22),
                tick: rgb(0x4A, 0x53, 0x62),
                tick_minor: rgb(0xA9, 0xB1, 0xBE),
                gloss: 0.0,
            },
            night,
        }
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> gfx::Color {
    gfx::Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

const fn tint(c: gfx::Color, a: f32) -> gfx::Color {
    gfx::Color {
        r: c.r,
        g: c.g,
        b: c.b,
        a,
    }
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

#[rustfmt::skip]
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
#[rustfmt::skip]
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
    let wrapped = if pos >= 60.0 {
        pos - 60.0
    } else if pos < 0.0 {
        pos + 60.0
    } else {
        pos
    };
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

impl krate::Guest for Component {
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

        let mut scroll = Scroll { at: 0.0, max: 0.0 };
        if draw(canvas, win, local, &mut scroll).is_err() {
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
        'session: loop {
            // Take everything already queued, then draw once: a resize or a
            // finger on the list sends far more events than there are frames.
            let mut next = events::wait(Some(ROUND_MILLIS));
            while let Some(event) = next {
                match event {
                    types::Event::CloseRequested(id) if id == win => break 'session,
                    types::Event::Wheel(w) => scroll.by(w.dy),
                    _ => {}
                }
                next = events::poll();
            }
            // A new second, a new size or a new look: draw again.
            if draw(canvas, win, local, &mut scroll).is_err() {
                break;
            }
        }

        let _ = window::close(win);
        0
    }
}

/// How far the list is scrolled, and how far it can be: only when the rows
/// do not all fit, which at the largest text sizes they may not.
struct Scroll {
    at: f32,
    max: f32,
}

impl Scroll {
    fn by(&mut self, dy: f32) {
        self.at = (self.at + dy).clamp(0.0, self.max);
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

/// What one city shows right now, worked out once per frame.
struct City<'a> {
    name: &'a str,
    /// Seconds into the city's day.
    sod: i64,
    is_day: bool,
    /// Its offset from the header clock, in minutes.
    diff: i64,
    /// -1 yesterday, 0 today, 1 tomorrow, against the header clock.
    day: i64,
}

fn draw(
    canvas: u64,
    win: u64,
    local: Option<i64>,
    scroll: &mut Scroll,
) -> Result<(), gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas).unwrap_or(gfx::Size {
        width: WIDTH,
        height: HEIGHT,
    });
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let screen = Screen::of(win);
    let look = screen.look;
    let t = theme(&look);
    let mut said = Said::new();
    let now_secs = (clock::now_millis() / 1000) as i64;

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
    let (top, right, bottom, left) = screen.safe;
    let narrow = w < 600.0;
    let short = h < 500.0;
    let margin = if narrow || short { 16.0 } else { 28.0 };
    let mut area = Rect::new(0.0, 0.0, w, h)
        .inset(top, right, bottom, left)
        .pad(margin);

    // The offsets read against the header clock: local when we know it,
    // UTC otherwise.
    let base_off = local.unwrap_or(0);
    draw_header(
        canvas, &mut area, now_secs, local, &look, &t, short, narrow, &mut said,
    )?;

    let cities: [City; 6] = core::array::from_fn(|i| {
        let (name, off) = CITIES.get(i).copied().unwrap_or(("", 0));
        let sod = day_seconds(now_secs, off);
        City {
            name,
            sod,
            is_day: (6..18).contains(&(sod / 3600)),
            diff: off - base_off,
            day: (now_secs + off * 60).div_euclid(86_400)
                - (now_secs + base_off * 60).div_euclid(86_400),
        }
    });

    // Cards with big faces where there is room for them -- three across on a
    // wide screen, two on a tablet -- otherwise rows: one column on a phone,
    // two when it is turned sideways.
    let gap = if narrow || short { 10.0 } else { 18.0 };
    let cols = if area.w >= 900.0 || area.w > area.h * 1.6 {
        3
    } else {
        2
    };
    let rows = cities.len().div_ceil(cols);
    let card_w = (area.w - gap * (cols as f32 - 1.0)) / cols as f32;
    let card_h = (area.h - gap * (rows as f32 - 1.0)) / rows as f32;
    let cards = !narrow && card_h >= 190.0 * look.text_scale.max(1.0) && card_w >= 220.0;

    if cards {
        scroll.max = 0.0;
        scroll.at = 0.0;
        for (i, city) in cities.iter().enumerate() {
            let (c, r) = ((i % cols) as f32, (i / cols) as f32);
            let card = Rect::new(
                area.x + c * (card_w + gap),
                area.y + r * (card_h + gap),
                card_w,
                card_h,
            );
            draw_card(canvas, card, city, &look, &t, &mut said)?;
        }
    } else {
        let cols = if narrow { 1 } else { 2 };
        let rows = cities.len().div_ceil(cols);
        let name_size = look.text(17.0);
        let sub_size = look.text(13.0);
        let natural = (name_size + sub_size) * 1.3 + 22.0;
        let natural = natural.max(screen.tap() + 12.0);
        // Rows share the height when they fit, up to half again their size,
        // so a tall phone is filled rather than left half empty.
        let fill_h = (area.h - gap * (rows as f32 - 1.0)) / rows as f32;
        let row_h = fill_h.clamp(natural, natural * 1.5);
        let row_w = (area.w - gap * (cols as f32 - 1.0)) / cols as f32;
        let content = rows as f32 * (row_h + gap) - gap;
        scroll.max = (content - area.h).max(0.0);
        scroll.at = scroll.at.clamp(0.0, scroll.max);
        // Clipped only when the rows scroll, so a row's shadow is not cut
        // off at the bottom of a list that fits.
        if scroll.max > 0.0 {
            canvas2d::set_clip(canvas, area.x, area.y, area.w, area.h)?;
        }
        for (i, city) in cities.iter().enumerate() {
            let (c, r) = ((i % cols) as f32, (i / cols) as f32);
            let y = area.y + r * (row_h + gap) - scroll.at;
            if y + row_h < area.y || y > area.bottom() {
                continue;
            }
            let row = Rect::new(area.x + c * (row_w + gap), y, row_w, row_h);
            draw_row(canvas, row, city, &look, &t, &mut said)?;
        }
        if scroll.max > 0.0 {
            canvas2d::clear_clip(canvas)?;
        }
    }

    canvas2d::present(canvas)?;
    said.send(win);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_header(
    canvas: u64,
    area: &mut Rect,
    now_secs: i64,
    local: Option<i64>,
    look: &Look,
    t: &Theme,
    short: bool,
    narrow: bool,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    let title = "World Clock";
    let tsize = look.text(if short {
        22.0
    } else if narrow {
        28.0
    } else {
        32.0
    });
    let ssize = look.text(15.0);

    let mut buf = TextBuf::new();
    match local {
        Some(off) => {
            buf.push_str("Local time ");
            push_hhmm(&mut buf, day_seconds(now_secs, off));
        }
        None => {
            buf.push_str("UTC ");
            push_hhmm(&mut buf, day_seconds(now_secs, 0));
        }
    }

    // A phone turned sideways has the height for one line: the local time
    // goes beside the title instead of under it.
    let head_h = if short {
        tsize * 1.25 + 10.0
    } else {
        tsize * 1.2 + ssize * 1.6 + 18.0
    };
    let head = area.take_top(head_h);
    let base = head.y + tsize * 0.95;
    let tw = width_of(canvas, title, tsize, 700).min(head.w);
    fit(canvas, title, head.x, base, tsize, 700, t.ink, head.w)?;
    said.heading(Rect::new(head.x, head.y, tw, tsize * 1.2), title);

    let (sx, sy) = if short {
        (head.x + tw + 16.0, base)
    } else {
        (head.x, base + ssize * 1.6)
    };
    let room = head.right() - sx;
    if room > 40.0 {
        fit(canvas, buf.as_str(), sx, sy, ssize, 400, t.ink2, room)?;
        said.text(Rect::new(sx, sy - ssize, room, ssize * 1.3), buf.as_str());
    }
    Ok(())
}

// ------------------------------------------------------------------
// A city as a card (tablet, desktop) or as a row (phone)
// ------------------------------------------------------------------

fn card_shell(canvas: u64, r: Rect, radius: f32, t: &Theme) -> Result<(), gfx::GfxError> {
    canvas2d::drop_shadow_round_rect(
        canvas,
        gfx_rect(Rect { y: r.y + 3.0, ..r }),
        radii(radius),
        10.0,
        t.shadow,
    )?;
    canvas2d::fill_round_rect(canvas, gfx_rect(r), radii(radius), t.card)?;
    canvas2d::stroke_round_rect(canvas, gfx_rect(r), radii(radius), 1.0, t.edge)
}

/// A card: the face centred and large, the city under it, then the time and
/// how far it is from here.
fn draw_card(
    canvas: u64,
    r: Rect,
    city: &City,
    look: &Look,
    t: &Theme,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    card_shell(canvas, r, 18.0, t)?;
    let pad = 18.0;
    let nsize = look.text(18.0);
    // The time grows with the card, so a tablet's big faces get a time
    // that is read from across the room.
    let tsize = look.text(20.0) * (r.w / 300.0).clamp(1.0, 1.3);
    let csize = look.text(13.0);
    let text_h = nsize * 1.35 + tsize * 1.15 + csize * 1.7 + 4.0;
    let radius = ((r.h - text_h - pad * 3.0) * 0.5)
        .min(r.w * 0.32)
        .clamp(24.0, 130.0);
    let cx = r.x + r.w * 0.5;
    let cy = r.y + pad + radius + 4.0;
    draw_face(canvas, cx, cy, radius, city, t)?;

    // Day or night, in the corner.
    let mark = (r.x + r.w - 26.0, r.y + 26.0);
    if city.is_day {
        draw_sun(canvas, mark.0, mark.1, 1.0, t)?;
    } else {
        draw_moon(canvas, mark.0, mark.1, 1.0, t.moon, t.card)?;
    }

    // The block under the face, centred in what is left.
    let below = cy + radius + pad * 0.5;
    let block_top = below + ((r.bottom() - pad - below) - text_h).max(0.0) * 0.5;
    let ny = block_top + nsize;
    let nw = width_of(canvas, city.name, nsize, 600).min(r.w - 2.0 * pad);
    fit(
        canvas,
        city.name,
        cx - nw * 0.5,
        ny,
        nsize,
        600,
        t.ink,
        r.w - 2.0 * pad,
    )?;

    let mut tb = TextBuf::new();
    push_hhmm(&mut tb, city.sod);
    let ty = ny + nsize * 0.35 + tsize * 1.15;
    let tw = width_of(canvas, tb.as_str(), tsize, 500);
    draw_text(canvas, tb.as_str(), cx - tw * 0.5, ty, tsize, 500, t.ink)?;

    let mut cb = TextBuf::new();
    push_offset(&mut cb, city.diff, city.day);
    let cy2 = ty + csize * 1.7;
    let cw = width_of(canvas, cb.as_str(), csize, 500).min(r.w - 2.0 * pad);
    fit(
        canvas,
        cb.as_str(),
        cx - cw * 0.5,
        cy2,
        csize,
        500,
        t.ink2,
        r.w - 2.0 * pad,
    )?;

    say_city(said, r, city);
    Ok(())
}

/// A row: a small face, the city with its day and offset under it, and the
/// time large at the right -- the shape of a phone's own world clock.
fn draw_row(
    canvas: u64,
    r: Rect,
    city: &City,
    look: &Look,
    t: &Theme,
    said: &mut Said,
) -> Result<(), gfx::GfxError> {
    card_shell(canvas, r, 16.0, t)?;
    let pad = 14.0;
    let radius = (r.h * 0.34).clamp(16.0, 34.0);
    let cx = r.x + pad + radius;
    let cy = r.y + r.h * 0.5;
    draw_face(canvas, cx, cy, radius, city, t)?;

    let mut tb = TextBuf::new();
    push_hhmm(&mut tb, city.sod);
    // The time is the thing being read: as large as the row allows, but it
    // never takes more than two fifths of it.
    let nsize = look.text(17.0);
    let ssize = look.text(13.0);
    let x = cx + radius + 14.0;
    let mut sub = TextBuf::new();
    push_offset(&mut sub, city.diff, city.day);
    let mark = ssize * 0.45;
    // What the city's name and its line need, so the time gives way to
    // them before either is cut.
    let wants = width_of(canvas, city.name, nsize, 600)
        .max(width_of(canvas, sub.as_str(), ssize, 400) + mark * 2.0 + 6.0);
    let mut tsize = look.text(30.0).min(r.h * 0.5);
    let mut tw = width_of(canvas, tb.as_str(), tsize, 300);
    let room_for_time = r.right() - pad - (x + wants + 12.0);
    let most = (r.w * 0.4).min(room_for_time.max(tw * 0.7));
    if tw > most && tw > 0.0 {
        tsize *= most / tw;
        tw = most;
    }
    let tx = r.right() - pad - tw;
    draw_text(
        canvas,
        tb.as_str(),
        tx,
        cy + tsize * 0.36,
        tsize,
        300,
        t.ink,
    )?;

    let room = tx - 12.0 - x;
    let block = nsize + ssize * 1.5;
    let ny = cy - block * 0.5 + nsize * 0.85;
    fit(canvas, city.name, x, ny, nsize, 600, t.ink, room)?;

    let sy = ny + ssize * 1.5;
    if city.is_day {
        draw_sun(canvas, x + mark, sy - ssize * 0.35, ssize / 15.0, t)?;
    } else {
        draw_moon(
            canvas,
            x + mark,
            sy - ssize * 0.35,
            ssize / 15.0,
            t.moon,
            t.card,
        )?;
    }
    fit(
        canvas,
        sub.as_str(),
        x + mark * 2.0 + 6.0,
        sy,
        ssize,
        400,
        t.ink2,
        room - mark * 2.0 - 6.0,
    )?;

    say_city(said, r, city);
    Ok(())
}

/// "Tokyo, 23:05, night, tomorrow, 16 hours ahead" for the screen reader.
fn say_city(said: &mut Said, r: Rect, city: &City) {
    let mut s = String::new();
    s.push_str(city.name);
    s.push_str(", ");
    let mut tb = TextBuf::new();
    push_hhmm(&mut tb, city.sod);
    s.push_str(tb.as_str());
    s.push_str(if city.is_day { ", day" } else { ", night" });
    s.push_str(match city.day {
        d if d < 0 => ", yesterday",
        0 => ", today",
        _ => ", tomorrow",
    });
    if city.diff == 0 {
        s.push_str(", local time");
    } else {
        let mut hb = TextBuf::new();
        let abs = city.diff.abs();
        push_num(&mut hb, abs / 60);
        if abs % 60 >= 30 {
            hb.push_str(" and a half");
        }
        s.push_str(", ");
        s.push_str(hb.as_str());
        s.push_str(if abs / 60 == 1 && abs % 60 < 30 {
            " hour"
        } else {
            " hours"
        });
        s.push_str(if city.diff > 0 { " ahead" } else { " behind" });
    }
    said.text(r, &s);
}

fn draw_face(
    canvas: u64,
    cx: f32,
    cy: f32,
    radius: f32,
    city: &City,
    t: &Theme,
) -> Result<(), gfx::GfxError> {
    let f = if city.is_day { t.day } else { t.night };
    let s = radius / 64.0;
    // Soft drop shadow, then a rim ring, then the face itself.
    canvas2d::radial_gradient(
        canvas,
        gfx::Point {
            x: cx,
            y: cy + 4.0 * s,
        },
        radius + 10.0 * s,
        t.shadow,
        tint(t.shadow, 0.0),
    )?;
    disc(canvas, cx, cy, radius + (2.0 * s).max(1.0), f.rim)?;
    disc(canvas, cx, cy, radius, f.fill)?;

    // A whisper of top light so the face reads as a dial, not a flat disc.
    if f.gloss > 0.0 {
        canvas2d::radial_gradient(
            canvas,
            gfx::Point {
                x: cx,
                y: cy - radius * 0.55,
            },
            radius * 1.05,
            gfx::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: f.gloss,
            },
            gfx::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 0.0,
            },
        )?;
    }

    // Twelve hour ticks; the cardinals sit a touch heavier.
    for k in 0..12usize {
        let (dx, dy) = dial_dir((k * 5) as f32);
        let cardinal = k % 3 == 0;
        let (r, c) = if cardinal {
            ((2.6 * s).max(1.4), f.tick)
        } else {
            ((1.7 * s).max(1.0), f.tick_minor)
        };
        disc(
            canvas,
            cx + dx * radius * 0.86,
            cy + dy * radius * 0.86,
            r,
            c,
        )?;
    }

    let sod = city.sod;
    let h = (sod / 3600) % 12;
    let m = (sod / 60) % 60;
    let sec = sod % 60;
    let hour_pos = (h * 5) as f32 + m as f32 / 12.0;
    let min_pos = m as f32 + sec as f32 / 60.0;

    // Hour: thick and short. Minute: thin and long. Second: accent hairline.
    hand(
        canvas,
        cx,
        cy,
        hour_pos,
        -0.09 * radius,
        0.53 * radius,
        (3.0 * s).max(1.3),
        f.hand,
    )?;
    hand(
        canvas,
        cx,
        cy,
        min_pos,
        -0.12 * radius,
        0.80 * radius,
        (1.9 * s).max(1.0),
        f.hand,
    )?;
    hand(
        canvas,
        cx,
        cy,
        sec as f32,
        -0.16 * radius,
        0.86 * radius,
        (1.0 * s).max(0.6),
        t.accent,
    )?;

    // Center cap: accent ring with a face-coloured core, like a real movement.
    disc(canvas, cx, cy, (4.4 * s).max(2.0), t.accent)?;
    disc(canvas, cx, cy, (1.7 * s).max(0.8), f.fill)?;
    Ok(())
}

/// Draw a hand as discs marched along its axis -- there is no rotated-rect
/// primitive, and at these sizes the march reads as a clean line.
#[allow(clippy::too_many_arguments)]
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
    let step = (thick * 0.7).max(0.8);
    let mut t = tail;
    while t <= len {
        disc(canvas, cx + dx * t, cy + dy * t, thick, c)?;
        t += step;
    }
    disc(canvas, cx + dx * len, cy + dy * len, thick, c)?;
    Ok(())
}

fn draw_sun(canvas: u64, x: f32, y: f32, s: f32, t: &Theme) -> Result<(), gfx::GfxError> {
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x, y },
        10.0 * s,
        tint(t.sun, 0.35),
        tint(t.sun, 0.0),
    )?;
    disc(canvas, x, y, 3.8 * s, t.sun)
}

fn draw_moon(
    canvas: u64,
    x: f32,
    y: f32,
    s: f32,
    moon: gfx::Color,
    ground: gfx::Color,
) -> Result<(), gfx::GfxError> {
    // Crescent: a light disc with a ground-coloured disc biting into it.
    disc(canvas, x, y, 4.6 * s, moon)?;
    disc(canvas, x + 2.5 * s, y - 1.7 * s, 4.0 * s, ground)
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

fn push_num(buf: &mut TextBuf, v: i64) {
    let v = v.clamp(0, 99);
    if v >= 10 {
        buf.push(b'0' + (v / 10) as u8);
    }
    buf.push(b'0' + (v % 10) as u8);
}

/// "+9 HRS" / "-4 HRS" / "+5.5 HRS" / "LOCAL", led by "Tomorrow" or
/// "Yesterday" when the city's date is not the header's.
fn push_offset(buf: &mut TextBuf, diff_minutes: i64, day: i64) {
    match day {
        d if d < 0 => buf.push_str("Yesterday, "),
        0 => buf.push_str("Today, "),
        _ => buf.push_str("Tomorrow, "),
    }
    if diff_minutes == 0 {
        buf.push_str("local");
        return;
    }
    let sign = if diff_minutes < 0 { b'-' } else { b'+' };
    let abs = diff_minutes.abs();
    buf.push(sign);
    push_num(buf, abs / 60);
    if abs % 60 >= 30 {
        buf.push_str(".5");
    }
    buf.push_str(" hrs");
}

// ------------------------------------------------------------------
// Tiny fixed text buffer (no format!, no heap)
// ------------------------------------------------------------------

struct TextBuf {
    bytes: [u8; 32],
    len: usize,
}

impl TextBuf {
    fn new() -> Self {
        TextBuf {
            bytes: [0; 32],
            len: 0,
        }
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

fn disc(canvas: u64, cx: f32, cy: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x: cx, y: cy }, r, c)
}

fn style(size: f32, weight: u16) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: if size >= 24.0 { -0.5 } else { 0.0 },
        family: gfx::FontFamily::Sans,
    }
}

fn draw_text(
    canvas: u64,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    weight: u16,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text_styled(
        canvas,
        text,
        gfx::Point { x, y },
        size,
        c,
        style(size, weight),
    )
}

/// Rendered width of a string, measured by the host with the same font
/// layout `draw_text` draws with.
fn width_of(canvas: u64, s: &str, size: f32, weight: u16) -> f32 {
    canvas2d::measure_text_styled(canvas, s, size, style(size, weight)).map_or(0.0, |m| m.width)
}

/// Draw `s` at most `room` wide: the longest leading part that fits, then an
/// ellipsis. A name cut short reads as "there is more"; one running off the
/// card reads as a broken app.
#[allow(clippy::too_many_arguments)]
fn fit(
    canvas: u64,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    weight: u16,
    c: gfx::Color,
    room: f32,
) -> Result<(), gfx::GfxError> {
    if room <= 0.0 {
        return Ok(());
    }
    if width_of(canvas, s, size, weight) <= room {
        return draw_text(canvas, s, x, y, size, weight, c);
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
    if part.is_empty() {
        return Ok(());
    }
    draw_text(canvas, part, x, y, size, weight, c)?;
    draw_text(
        canvas,
        "…",
        x + width_of(canvas, part, size, weight),
        y,
        size,
        weight,
        c,
    )
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

krate::export!(Component);
