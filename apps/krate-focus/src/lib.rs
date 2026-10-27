//! Krate Focus -- a pomodoro timer drawn entirely on a canvas2d.
//!
//! The centerpiece is a large progress ring: a quiet track circle with the
//! elapsed part of the session drawn as an accent arc, the remaining time
//! large inside it, and the session kind ("Focus" / "Break") underneath.
//! Four session dots show how many pomodoros this cycle has banked, and
//! Start/Pause + Reset buttons drive it. 25:00 of focus, 5:00 of break,
//! timed with monotonic `now_millis` deltas while running. Completed
//! sessions persist in `store::kv` so the count survives restarts.
//!
//! It lays itself out from the canvas it really has, every frame: the ring
//! is sized from the smaller side, the controls go below it on a tall
//! screen (a phone held upright, a tablet) and beside it on a wide one (a
//! phone turned sideways, a desktop window). Text follows the person's text
//! size, and each frame tells the screen reader what it shows.
//!
//! `#![no_std]` keeps the component `krate:*`-only: the SDK owns the
//! allocator and panic handler, strings are built in fixed byte buffers, and
//! the ring's head dot is placed with a small polynomial sine -- no libm, no
//! `format!`, no panicking index.

#![no_std]

extern crate alloc;

use alloc::string::String;
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::store::kv;
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The window asked for on a desktop. Only a starting size: every frame is
/// laid out from what the canvas really is.
const WIDTH: f32 = 520.0;
const HEIGHT: f32 = 680.0;

const FOCUS_MS: u64 = 25 * 60 * 1000;
const BREAK_MS: u64 = 5 * 60 * 1000;

const DONE_KEY: &str = "focus-completed";

const WAIT_ROUND_MILLIS: u32 = 50;
const MAX_WAIT_ROUNDS: u32 = 6_000_000;
/// Consecutive quiet rounds before a paused, unwatched run stops (~10 s).
const MAX_IDLE_ROUNDS: u32 = 200;

// ---- palette (the house design system) ----------------------------

const BG_TOP: gfx::Color = rgb(0x0B, 0x0E, 0x15);
const BG_BOT: gfx::Color = rgb(0x10, 0x14, 0x1D);
const TRACK: gfx::Color = rgb(0x23, 0x2A, 0x38);
const DOT_EMPTY: gfx::Color = rgb(0x31, 0x3A, 0x4C);
const ACCENT: gfx::Color = rgb(0x4C, 0x8D, 0xFF);
const HEAD: gfx::Color = rgb(0xC7, 0xDE, 0xFF);
const INK: gfx::Color = rgb(0xF2, 0xF5, 0xFA);
const INK_DIM: gfx::Color = rgb(0x9A, 0xA5, 0xB5);
const INK_QUIET: gfx::Color = rgb(0x5D, 0x68, 0x78);
const WHITE: gfx::Color = rgb(0xFF, 0xFF, 0xFF);

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
// Timer state
// ------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Focus,
    Break,
}

impl Phase {
    fn duration_ms(self) -> u64 {
        match self {
            Phase::Focus => FOCUS_MS,
            Phase::Break => BREAK_MS,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Phase::Focus => "Focus",
            Phase::Break => "Break",
        }
    }
}

struct Timer {
    phase: Phase,
    /// Milliseconds left in the current session.
    remaining_ms: u64,
    running: bool,
    /// `now_millis` reading the last time the countdown was folded in.
    anchor_ms: u64,
    /// Completed focus sessions, persisted. Dots show `completed % 4`.
    completed: u64,
}

impl Timer {
    fn new(completed: u64) -> Self {
        Self {
            phase: Phase::Focus,
            remaining_ms: FOCUS_MS,
            running: false,
            anchor_ms: 0,
            completed,
        }
    }

    fn start(&mut self, now: u64) {
        if !self.running {
            self.running = true;
            self.anchor_ms = now;
        }
    }

    fn pause(&mut self, now: u64) {
        self.tick(now);
        self.running = false;
    }

    fn reset(&mut self) {
        self.phase = Phase::Focus;
        self.remaining_ms = FOCUS_MS;
        self.running = false;
    }

    /// Fold elapsed wall-clock into the countdown. Returns true when the
    /// display (whole seconds / phase) changed. A finished session flips the
    /// phase and pauses at the boundary; a finished focus bumps `completed`.
    fn tick(&mut self, now: u64) -> bool {
        if !self.running {
            return false;
        }
        let delta = now.saturating_sub(self.anchor_ms);
        if delta == 0 {
            return false;
        }
        self.anchor_ms = now;
        let before_secs = self.remaining_ms / 1000;
        self.remaining_ms = self.remaining_ms.saturating_sub(delta);
        if self.remaining_ms == 0 {
            if self.phase == Phase::Focus {
                self.completed = self.completed.saturating_add(1);
                save_completed(self.completed);
                self.phase = Phase::Break;
            } else {
                self.phase = Phase::Focus;
            }
            self.remaining_ms = self.phase.duration_ms();
            self.running = false;
            return true;
        }
        self.remaining_ms / 1000 != before_secs
    }

    /// Fraction of the current session already elapsed, 0.0..=1.0.
    fn progress(&self) -> f32 {
        let total = self.phase.duration_ms() as f32;
        let done = total - self.remaining_ms as f32;
        (done / total).clamp(0.0, 1.0)
    }

    /// The primary button's verb for the current state.
    fn verb(&self) -> &'static str {
        if self.running {
            "Pause"
        } else if self.remaining_ms < self.phase.duration_ms() {
            "Resume"
        } else {
            "Start"
        }
    }

    /// Pomodoros banked in the current cycle of four.
    fn dots_filled(&self) -> u64 {
        self.completed % 4
    }
}

// ------------------------------------------------------------------
// Component
// ------------------------------------------------------------------

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let completed = match kv::get(DONE_KEY) {
            Ok(Some(bytes)) => parse_u64(&bytes),
            Ok(None) => 0,
            Err(_) => {
                let out = stdio::stdout();
                let _ = out.write(b"store:unavailable\n");
                return 40;
            }
        };

        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Focus", size) else {
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

        if quick {
            // The automated shot: mid-focus, paused at 18:24 with two of the
            // four pomodoros banked, so the frame shows the arc, the dots,
            // and the Resume state all at once. Display-only seed -- nothing
            // is written back to the store.
            let mut timer = Timer::new(2);
            timer.remaining_ms = (18 * 60 + 24) * 1000;
            let _ = draw(canvas, win, &timer);
            let _ = window::close(win);
            let out = stdio::stdout();
            let _ = out.write(b"focus:ok\n");
            return 0;
        }

        let mut timer = Timer::new(completed);
        // The layout of the frame on screen. Clicks are tested against this,
        // never against numbers worked out again, so a click lands on the
        // button the person sees even mid-resize.
        let mut shown = match draw(canvas, win, &timer) {
            Ok(layout) => layout,
            Err(_) => {
                let _ = window::close(win);
                return 34;
            }
        };

        let mut idle_rounds = 0u32;
        for _ in 0..MAX_WAIT_ROUNDS {
            let event = events::wait(Some(WAIT_ROUND_MILLIS));

            // Keep the clock honest every round while running; redraw only
            // when the visible second flips.
            if timer.running && timer.tick(clock::now_millis()) {
                redraw(canvas, win, &timer, &mut shown);
            }

            if event.is_none() {
                // A running timer is never idle; only a paused, unwatched
                // window counts quiet rounds toward giving up.
                if timer.running {
                    idle_rounds = 0;
                } else {
                    idle_rounds += 1;
                    // Only a headless check gives up on silence. A person who
                    // pauses the timer and walks away must not come back to a
                    // window that closed itself.
                    if quick && idle_rounds >= MAX_IDLE_ROUNDS {
                        break;
                    }
                }
                continue;
            }
            idle_rounds = 0;

            match event {
                Some(types::Event::Pointer(p)) if p.pressed => {
                    let now = clock::now_millis();
                    if shown.primary.contains(p.x, p.y) {
                        if timer.running {
                            timer.pause(now);
                        } else {
                            timer.start(now);
                        }
                        redraw(canvas, win, &timer, &mut shown);
                    } else if shown.ghost.contains(p.x, p.y) {
                        timer.reset();
                        redraw(canvas, win, &timer, &mut shown);
                    }
                }
                // A phone turned, a window dragged wider, the person's text
                // size or dark mode changed: lay the frame out again.
                Some(types::Event::Resized(_)) | Some(types::Event::ThemeChanged(_)) => {
                    redraw(canvas, win, &timer, &mut shown);
                }
                Some(types::Event::CloseRequested(id)) if id == win => {
                    break;
                }
                _ => {}
            }
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"focus:ok\n");
        0
    }
}

fn redraw(canvas: u64, win: u64, timer: &Timer, shown: &mut Layout) {
    if let Ok(layout) = draw(canvas, win, timer) {
        *shown = layout;
    }
}

// ------------------------------------------------------------------
// Layout: every position, from the canvas size and the text size
// ------------------------------------------------------------------

/// Where everything sits in one frame. Worked out once per frame and used
/// by both the drawing and the hit-testing.
#[derive(Clone, Copy, Default)]
struct Layout {
    w: f32,
    h: f32,
    /// The ring: centre, radius to the middle of the band, band width.
    cx: f32,
    cy: f32,
    r: f32,
    band: f32,
    /// The centre line the caption, dots and buttons are set on.
    mid: f32,
    cap_y: f32,
    dots_y: f32,
    dot_r: f32,
    dot_gap: f32,
    primary: Rect,
    ghost: Rect,
    /// Font sizes, already at the person's text size.
    cap_size: f32,
    time_size: f32,
    label_size: f32,
    verb_size: f32,
}

/// A screen is "wide" when it is clearly wider than tall: there the
/// controls go beside the ring, so the ring can use the full height.
const WIDE: f32 = 1.2;

fn layout(canvas: u64, look: Look) -> Layout {
    let size = canvas2d::canvas_size(canvas).unwrap_or(gfx::Size {
        width: WIDTH,
        height: HEIGHT,
    });
    let (w, h) = (size.width.max(1.0), size.height.max(1.0));
    let margin = (w.min(h) * 0.06).clamp(16.0, 48.0);
    // The space between the caption, the face and the controls.
    let space = margin * 0.8;

    // Chrome grows a little on a big screen, so the controls do not shrink
    // to a postage stamp beside a ring that fills a tablet or a desktop.
    let k = (w.min(h) / 640.0).clamp(1.0, 1.5);
    let cap_size = look.text(13.0 * k);
    let cap_h = cap_size * 1.6;
    let verb_size = look.text(17.0 * k);
    // 44 points is the smallest a finger hits reliably; larger text needs a
    // taller button, not text spilling out of it.
    let btn_h = (verb_size * 2.6).max(44.0);
    let pad = verb_size * 1.5;
    // As wide as the longest verb, so the button keeps its width when Start
    // turns into Pause.
    let mut verb_w = 0.0f32;
    for verb in ["Start", "Pause", "Resume"] {
        verb_w = verb_w.max(text_width(canvas, verb, verb_size, 600));
    }
    let primary_w = verb_w + pad * 2.0;
    let ghost_w = text_width(canvas, "Reset", verb_size, 600) + pad * 2.0;
    let btn_gap = 16.0 * k;
    let row_w = primary_w + btn_gap + ghost_w;
    let dot_r = look.text(5.0 * k);
    let dot_gap = dot_r * 5.6;

    let wide = w > h * WIDE;
    // How wide the controls may be: the whole width under the ring, or a
    // column of about two fifths beside it.
    let room = if wide {
        (w * 0.42 - margin).max(primary_w.max(ghost_w))
    } else {
        w - margin * 2.0
    };
    // Very large text on a narrow phone: the buttons stack rather than run
    // off the edge.
    let stacked = row_w > room;
    let col_w = if stacked {
        room.min(primary_w.max(ghost_w).max(room * 0.8))
    } else {
        row_w
    };
    let buttons_h = if stacked { btn_h * 2.0 + 12.0 } else { btn_h };
    let controls_h = dot_r * 2.0 + space + buttons_h;

    // The face takes what is left, measured on the shorter side so it stays a
    // circle; a desktop wider than it needs keeps the group centred instead
    // of stretching anything.
    let (face, cx, cy, mid, cap_top, controls_top);
    if wide {
        face = (h - margin * 2.0)
            .min(w - margin * 2.0 - space * 2.0 - col_w)
            .max(80.0);
        let left = ((w - face - space * 2.0 - col_w) * 0.5).max(margin);
        cx = left + face * 0.5;
        cy = h * 0.5;
        mid = left + face + space * 2.0 + col_w * 0.5;
        cap_top = (h - cap_h - space - controls_h) * 0.5;
        controls_top = cap_top + cap_h + space;
    } else {
        face = (w - margin * 2.0)
            .min(h - margin * 2.0 - cap_h - controls_h - space * 2.0)
            .max(80.0);
        // Height the face could not use (a phone is narrower than tall)
        // goes above, below, and between the face and the controls, which
        // lowers the buttons toward the thumb instead of leaving one block
        // floating between two empty bands.
        let extra = ((h - margin * 2.0 - cap_h - face - controls_h - space * 2.0) / 3.0).max(0.0);
        cx = w * 0.5;
        mid = cx;
        cap_top = margin + extra;
        cy = cap_top + cap_h + space + face * 0.5;
        controls_top = cy + face * 0.5 + space + extra;
    }

    let band = (face * 0.04).clamp(8.0, 22.0);
    let r = face * 0.5 - band;
    let btn_y = controls_top + dot_r * 2.0 + space;
    let (primary, ghost) = if stacked {
        let x = mid - col_w * 0.5;
        (
            Rect::new(x, btn_y, col_w, btn_h),
            Rect::new(x, btn_y + btn_h + 12.0, col_w, btn_h),
        )
    } else {
        let x = mid - row_w * 0.5;
        (
            Rect::new(x, btn_y, primary_w, btn_h),
            Rect::new(x + primary_w + btn_gap, btn_y, ghost_w, btn_h),
        )
    };

    Layout {
        w,
        h,
        cx,
        cy,
        r,
        band,
        mid,
        cap_y: cap_top + cap_size,
        dots_y: controls_top + dot_r,
        dot_r,
        dot_gap,
        primary,
        ghost,
        cap_size,
        // The readout scales with the ring, at the person's text size, but
        // never wider than the inside of the ring.
        time_size: look.text(r * 0.38).min(r * 0.56),
        label_size: look.text((r * 0.09).max(14.0)).min(r * 0.2),
        verb_size,
    }
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(canvas: u64, win: u64, timer: &Timer) -> Result<Layout, gfx::GfxError> {
    let look = Look::now();
    let l = layout(canvas, look);
    // More contrast asked for: the quiet caption steps up to the dim ink.
    let quiet = if look.high_contrast {
        INK_DIM
    } else {
        INK_QUIET
    };
    let mut said = Said::new();

    // Ground: the house near-black blue, top to bottom.
    canvas2d::linear_gradient(
        canvas,
        gfx_rect(Rect::new(0.0, 0.0, l.w, l.h)),
        BG_TOP,
        BG_BOT,
    )?;

    // A faint accent pool behind the ring so it sits in its own light.
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: l.cx, y: l.cy },
        l.r * 1.45,
        tint(ACCENT, 0.07),
        tint(ACCENT, 0.0),
    )?;

    // Quiet caption, letter-spaced so it reads as a wordmark.
    let cap = "POMODORO";
    let cap_style = style(500, l.cap_size * 0.35);
    let cw = styled_width(canvas, cap, l.cap_size, cap_style);
    text(
        canvas,
        cap,
        l.mid - cw * 0.5,
        l.cap_y,
        l.cap_size,
        quiet,
        cap_style,
    )?;
    said.heading(
        Rect::new(l.mid - cw * 0.5, l.cap_y - l.cap_size, cw, l.cap_size * 1.3),
        "Pomodoro",
    );

    draw_ring(canvas, &l, timer.progress())?;

    // ---- remaining time, big, centred in the ring ----
    let mut buf = [0u8; 8];
    let mmss = core::str::from_utf8(fmt_mmss(timer.remaining_ms, &mut buf)).unwrap_or("0:00");
    let time_style = style(600, -l.time_size * 0.02);
    let tw = styled_width(canvas, mmss, l.time_size, time_style);
    let time_y = l.cy + l.time_size * 0.2;
    text(
        canvas,
        mmss,
        l.cx - tw * 0.5,
        time_y,
        l.time_size,
        INK,
        time_style,
    )?;

    // ---- session label under the time ----
    let label = timer.phase.label();
    let lw = text_width(canvas, label, l.label_size, 400);
    let label_y = time_y + l.time_size * 0.25 + l.label_size * 1.3;
    text(
        canvas,
        label,
        l.cx - lw * 0.5,
        label_y,
        l.label_size,
        INK_DIM,
        style(400, 0.0),
    )?;

    let mut phrase = String::from(mmss);
    phrase.push_str(" left in ");
    phrase.push_str(label);
    phrase.push_str(if timer.running {
        ", running"
    } else {
        ", paused"
    });
    said.text(
        Rect::new(l.cx - l.r, l.cy - l.r, l.r * 2.0, l.r * 2.0),
        &phrase,
    );

    // ---- session dots: pomodoros banked this cycle ----
    let filled = timer.dots_filled();
    for i in 0..4u64 {
        let x = l.mid + (i as f32 - 1.5) * l.dot_gap;
        // Empty dots sit a step above the ring track so they stay legible at
        // this small size on the dark ground.
        let c = if i < filled { ACCENT } else { DOT_EMPTY };
        disc(canvas, x, l.dots_y, l.dot_r, c)?;
    }
    let mut nb = [0u8; 20];
    let mut dots = String::from(core::str::from_utf8(u64_to_bytes(filled, &mut nb)).unwrap_or("0"));
    dots.push_str(" of 4 focus sessions done this cycle");
    let dots_w = l.dot_gap * 3.0 + l.dot_r * 2.0;
    said.text(
        Rect::new(
            l.mid - dots_w * 0.5,
            l.dots_y - l.dot_r,
            dots_w,
            l.dot_r * 2.0,
        ),
        &dots,
    );

    // ---- buttons ----
    let radius = l.primary.h * 0.28;
    canvas2d::fill_round_rect(canvas, gfx_rect(l.primary), radii(radius), ACCENT)?;
    label_in(canvas, l.primary, timer.verb(), l.verb_size, WHITE)?;
    said.button(l.primary, timer.verb());

    // Ghost: a hairline border on the ground, no fill.
    canvas2d::stroke_round_rect(canvas, gfx_rect(l.ghost), radii(radius), 1.5, TRACK)?;
    label_in(canvas, l.ghost, "Reset", l.verb_size, INK_DIM)?;
    said.button(l.ghost, "Reset");

    canvas2d::present(canvas)?;
    said.send(win);
    Ok(l)
}

/// The ring: a full track circle, the elapsed arc from 12 o'clock clockwise,
/// and a brighter head dot at its leading end so progress has a focal point.
fn draw_ring(canvas: u64, l: &Layout, progress: f32) -> Result<(), gfx::GfxError> {
    const TWO_PI: f32 = 6.283_185_5;
    let centre = gfx::Point { x: l.cx, y: l.cy };
    canvas2d::stroke_circle(canvas, centre, l.r, l.band, TRACK)?;
    let progress = progress.clamp(0.0, 1.0);
    if progress <= 0.0 {
        return Ok(());
    }
    canvas2d::stroke_arc(canvas, centre, l.r, -90.0, progress * 360.0, l.band, ACCENT)?;
    // Round the arc's start: the band begins at 12 o'clock.
    disc(canvas, l.cx, l.cy - l.r, l.band * 0.5, ACCENT)?;

    // Head dot with a soft glow at the leading edge.
    let (x, y) = ring_point(l, progress * TWO_PI);
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x, y },
        l.band * 1.3,
        tint(ACCENT, 0.45),
        tint(ACCENT, 0.0),
    )?;
    disc(canvas, x, y, l.band * 0.5 + 1.5, HEAD)
}

/// Point on the ring at `a` radians past 12 o'clock, clockwise.
fn ring_point(l: &Layout, a: f32) -> (f32, f32) {
    const HALF_PI: f32 = 1.570_796_3;
    // Screen angle: 12 o'clock is -90 deg; clockwise means +angle in screen
    // coords (y down).
    let t = a - HALF_PI;
    (l.cx + l.r * cos_approx(t), l.cy + l.r * sin_approx(t))
}

// ------------------------------------------------------------------
// Tiny trig: a wrapped parabolic sine, refined. Max error ~0.001 --
// invisible at ring scale. No libm anywhere.
// ------------------------------------------------------------------

fn absf(x: f32) -> f32 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

fn sin_approx(x: f32) -> f32 {
    const PI: f32 = 3.141_592_7;
    const TWO_PI: f32 = 6.283_185_5;
    let mut x = x;
    while x > PI {
        x -= TWO_PI;
    }
    while x < -PI {
        x += TWO_PI;
    }
    // Bhaskara-style parabola, then one refinement pass.
    const B: f32 = 4.0 / PI;
    const C: f32 = -4.0 / (PI * PI);
    let y = B * x + C * x * absf(x);
    0.225 * (y * absf(y) - y) + y
}

fn cos_approx(x: f32) -> f32 {
    const HALF_PI: f32 = 1.570_796_3;
    sin_approx(x + HALF_PI)
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

fn radii(all: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: all,
        top_right: all,
        bottom_right: all,
        bottom_left: all,
    }
}

fn style(weight: u16, letter_spacing: f32) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing,
        family: gfx::FontFamily::Sans,
    }
}

fn disc(canvas: u64, cx: f32, cy: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x: cx, y: cy }, r, c)
}

fn text(
    canvas: u64,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    c: gfx::Color,
    st: gfx::TextStyle,
) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text_styled(canvas, s, gfx::Point { x, y }, size, c, st)
}

/// A button's label, centred in its rectangle.
fn label_in(canvas: u64, r: Rect, s: &str, size: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    let w = text_width(canvas, s, size, 600);
    let baseline = r.y + r.h * 0.5 + size * 0.35;
    text(
        canvas,
        s,
        r.x + (r.w - w) * 0.5,
        baseline,
        size,
        c,
        style(600, 0.0),
    )
}

/// Rendered width of a string, measured by the host with the same font
/// layout `draw_text_styled` draws with. Character count times a constant is
/// wrong on a proportional face: `i` and `W` differ about four times.
fn styled_width(canvas: u64, s: &str, size: f32, st: gfx::TextStyle) -> f32 {
    match canvas2d::measure_text_styled(canvas, s, size, st) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

fn text_width(canvas: u64, s: &str, size: f32, weight: u16) -> f32 {
    styled_width(canvas, s, size, style(weight, 0.0))
}

// ------------------------------------------------------------------
// Numbers <-> bytes, panic-free
// ------------------------------------------------------------------

fn parse_u64(bytes: &[u8]) -> u64 {
    let mut value = 0u64;
    for byte in bytes {
        if byte.is_ascii_digit() {
            value = value
                .saturating_mul(10)
                .saturating_add(u64::from(byte - b'0'));
        }
    }
    value
}

fn save_completed(count: u64) {
    let mut buf = [0u8; 20];
    let text = u64_to_bytes(count, &mut buf);
    let _ = kv::set(DONE_KEY, text);
}

/// Render remaining milliseconds as `m:ss` / `mm:ss` (ceil to the visible
/// second, so 24:59.2 reads 25:00 until a full second has really passed).
fn fmt_mmss(ms: u64, buf: &mut [u8; 8]) -> &[u8] {
    let total_secs = ms.div_ceil(1000);
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    let mut pos = 0usize;
    if minutes >= 10 {
        push_byte(buf, &mut pos, b'0' + ((minutes / 10) % 10) as u8);
    }
    push_byte(buf, &mut pos, b'0' + (minutes % 10) as u8);
    push_byte(buf, &mut pos, b':');
    push_byte(buf, &mut pos, b'0' + (seconds / 10) as u8);
    push_byte(buf, &mut pos, b'0' + (seconds % 10) as u8);
    buf.get(..pos).unwrap_or(b"0:00")
}

fn push_byte(buf: &mut [u8; 8], pos: &mut usize, byte: u8) {
    if let Some(slot) = buf.get_mut(*pos) {
        *slot = byte;
        *pos += 1;
    }
}

fn u64_to_bytes(value: u64, buf: &mut [u8; 20]) -> &[u8] {
    if value == 0 {
        if let Some(slot) = buf.get_mut(0) {
            *slot = b'0';
        }
        return buf.get(..1).unwrap_or(b"0");
    }
    let mut scratch = [0u8; 20];
    let mut n = value;
    let mut count = 0usize;
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
        if let (Some(src), Some(dst)) = (scratch.get(i), buf.get_mut(pos)) {
            *dst = *src;
            pos += 1;
        }
    }
    buf.get(..pos).unwrap_or(b"0")
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
