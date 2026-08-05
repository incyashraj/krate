//! Krate Focus — a pomodoro timer drawn entirely on a canvas2d.
//!
//! The centerpiece is a large progress ring: a quiet track circle with the
//! elapsed part of the session drawn as an accent arc, the remaining time
//! large inside it, and the session kind ("Focus" / "Break") underneath.
//! Below the ring, four session dots show how many pomodoros this cycle has
//! banked, and Start/Pause + Reset buttons drive it. 25:00 of focus, 5:00 of
//! break, timed with monotonic `now_millis` deltas while running. Completed
//! sessions persist in `store::kv` so the count survives restarts.
//!
//! `#![no_std]` keeps the component `krate:*`-only: the SDK owns the
//! allocator and panic handler, strings are built in fixed byte buffers, and
//! the arc trigonometry is a small polynomial sine — no libm, no `format!`,
//! no panicking index.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::kv;
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The size the window opens at. Nothing is laid out from these -- the window
/// is resizable, so every rectangle comes from `Layout::for_size`, built from
/// `canvas2d::canvas_size` at the top of each frame.
const WIDTH: f32 = 520.0;
const HEIGHT: f32 = 680.0;

const FOCUS_MS: u64 = 25 * 60 * 1000;
const BREAK_MS: u64 = 5 * 60 * 1000;

const DONE_KEY: &str = "focus-completed";

const WAIT_ROUND_MILLIS: u32 = 50;
const MAX_WAIT_ROUNDS: u32 = 6_000_000;
/// Consecutive quiet rounds before a paused, unwatched run stops (~10 s).
const MAX_IDLE_ROUNDS: u32 = 200;

// ---- geometry -----------------------------------------------------
//
// One `Layout`, computed from the canvas's current size once per frame, read
// by both the drawing and the hit-testing. The ring centres itself on the
// canvas and the buttons stay under it at any size. Nothing here is a `const`
// coordinate: a rect drawn from one set of numbers and clicked against another
// drifts apart the moment the window is resized.

const DOT_R: f32 = 5.0;
const DOT_GAP: f32 = 28.0;

/// Below this the layout clamps rather than computing a negative radius.
const MIN_CANVAS_W: f32 = 260.0;
const MIN_CANVAS_H: f32 = 380.0;

/// Every rectangle and centre in the timer, derived from the canvas size.
struct Layout {
    width: f32,
    height: f32,
    /// Horizontal centre -- everything in this app hangs off it.
    cx: f32,
    caption_y: f32,
    ring_cy: f32,
    ring_r: f32,
    ring_stroke: f32,
    /// Type sizes, scaled with the ring so the clock always fits inside it.
    time_size: f32,
    label_size: f32,
    dots_y: f32,
    btn_y: f32,
    btn_h: f32,
    btn_primary_w: f32,
    btn_ghost_w: f32,
    btn_gap: f32,
}

impl Layout {
    fn for_size(width: f32, height: f32) -> Self {
        let width = width.max(MIN_CANVAS_W);
        let height = height.max(MIN_CANVAS_H);
        let cx = width * 0.5;

        let caption_y = (height * 0.094).clamp(38.0, 72.0);

        // Buttons sit a fixed distance off the bottom; the ring takes the
        // space between the caption and the dots above them.
        let btn_h = 44.0;
        let btn_y = height - 76.0;
        let dots_y = btn_y - 44.0;

        let ring_top = caption_y + 24.0;
        let ring_space = (dots_y - 24.0 - ring_top).max(60.0);
        // The ring is bounded by BOTH the width and the height it has, so it
        // never spills out of a narrow window or a short one.
        let ring_r = (ring_space * 0.5).min(width * 0.42).clamp(40.0, 190.0);
        let ring_cy = ring_top + ring_space * 0.5;
        let ring_stroke = (ring_r * 0.082).clamp(6.0, 16.0);

        let time_size = (ring_r * 0.52).clamp(24.0, 88.0);
        let label_size = (ring_r * 0.088).clamp(10.0, 15.0);

        // The buttons keep a sane share of a narrow window rather than
        // overrunning its edges.
        let avail = (width - 48.0).max(120.0);
        let btn_primary_w = (avail * 0.52).clamp(96.0, 140.0);
        let btn_ghost_w = (avail * 0.40).clamp(80.0, 112.0);
        let btn_gap = 16.0;

        Self {
            width,
            height,
            cx,
            caption_y,
            ring_cy,
            ring_r,
            ring_stroke,
            time_size,
            label_size,
            dots_y,
            btn_y,
            btn_h,
            btn_primary_w,
            btn_ghost_w,
            btn_gap,
        }
    }

    /// The primary button. Drawn from this, clicked against this.
    fn primary_rect(&self) -> gfx::Rect {
        let total = self.btn_primary_w + self.btn_gap + self.btn_ghost_w;
        gfx::Rect {
            x: self.cx - total * 0.5,
            y: self.btn_y,
            width: self.btn_primary_w,
            height: self.btn_h,
        }
    }

    /// The ghost (Reset) button beside it.
    fn ghost_rect(&self) -> gfx::Rect {
        let total = self.btn_primary_w + self.btn_gap + self.btn_ghost_w;
        gfx::Rect {
            x: self.cx - total * 0.5 + self.btn_primary_w + self.btn_gap,
            y: self.btn_y,
            width: self.btn_ghost_w,
            height: self.btn_h,
        }
    }
}

// ---- palette (the house design system) ----------------------------

const BG_TOP: gfx::Color = rgb(0x0B, 0x0E, 0x15);
const BG_BOT: gfx::Color = rgb(0x10, 0x14, 0x1D);
const TRACK: gfx::Color = rgb(0x23, 0x2A, 0x38);
const ACCENT: gfx::Color = rgb(0x4C, 0x8D, 0xFF);
const INK: gfx::Color = rgb(0xF2, 0xF5, 0xFA);
const INK_DIM: gfx::Color = rgb(0x9A, 0xA5, 0xB5);
const INK_QUIET: gfx::Color = rgb(0x5D, 0x68, 0x78);

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

impl bindings::Guest for Component {
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
        let first_arg = raw.as_bytes().split(|byte| *byte == b'\n').next();
        let quick = first_arg.is_some_and(|first| first == b"quick");
        let resize_check = first_arg.is_some_and(|first| first == b"resize-check");

        if resize_check {
            // Drive the window through several shapes and confirm a click at
            // the centre of the button just drawn still lands on that button.
            let out = stdio::stdout();
            let mut timer = Timer::new(2);
            timer.remaining_ms = (18 * 60 + 24) * 1000;
            let sizes = [(520u32, 680u32), (900u32, 460u32), (300u32, 820u32)];
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
                let Ok(layout) = draw(canvas, &timer) else {
                    all_ok = false;
                    continue;
                };

                let _ = out.write(b"size:");
                let mut nbuf = [0u8; 12];
                let _ = out.write(u32_bytes(layout.width as u32, &mut nbuf));
                let _ = out.write(b"x");
                let _ = out.write(u32_bytes(layout.height as u32, &mut nbuf));

                let pr = layout.primary_rect();
                let gr = layout.ghost_rect();
                let primary_ok = hit(pr.x + pr.width * 0.5, pr.y + pr.height * 0.5, pr);
                let ghost_ok = hit(gr.x + gr.width * 0.5, gr.y + gr.height * 0.5, gr);
                // The two buttons must never overlap, or one would swallow the
                // other's clicks.
                let apart_ok = pr.x + pr.width <= gr.x;
                // Both must stay inside the canvas at every size.
                let inside_ok = pr.x >= 0.0
                    && gr.x + gr.width <= layout.width
                    && gr.y + gr.height <= layout.height;
                // The ring must not run into the buttons below it.
                let ring_ok = layout.ring_cy + layout.ring_r <= layout.btn_y;

                if primary_ok && ghost_ok && apart_ok && inside_ok && ring_ok {
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
            let _ = window::close(win);
            return if all_ok { 0 } else { 40 };
        }

        if quick {
            // The automated shot: mid-focus, paused at 18:24 with two of the
            // four pomodoros banked, so the frame shows the arc, the dots,
            // and the Resume state all at once. Display-only seed — nothing
            // is written back to the store.
            let mut timer = Timer::new(2);
            timer.remaining_ms = (18 * 60 + 24) * 1000;
            let _ = draw(canvas, &timer);
            let _ = window::close(win);
            let out = stdio::stdout();
            let _ = out.write(b"focus:ok\n");
            return 0;
        }

        let mut timer = Timer::new(completed);
        // The layout the visible frame was drawn with, so clicks follow the
        // window when it is resized.
        let mut layout = match draw(canvas, &timer) {
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
                if let Ok(fresh) = draw(canvas, &timer) {
                    layout = fresh;
                }
            }

            if event.is_none() {
                // A running timer is never idle; only a paused, unwatched
                // window counts quiet rounds toward giving up.
                if timer.running {
                    idle_rounds = 0;
                } else {
                    idle_rounds += 1;
                    if idle_rounds >= MAX_IDLE_ROUNDS {
                        break;
                    }
                }
                continue;
            }
            idle_rounds = 0;

            match event {
                Some(types::Event::Pointer(p)) if p.pressed => {
                    let now = clock::now_millis();
                    if hit(p.x, p.y, layout.primary_rect()) {
                        if timer.running {
                            timer.pause(now);
                        } else {
                            timer.start(now);
                        }
                        if let Ok(fresh) = draw(canvas, &timer) {
                            layout = fresh;
                        }
                    } else if hit(p.x, p.y, layout.ghost_rect()) {
                        timer.reset();
                        if let Ok(fresh) = draw(canvas, &timer) {
                            layout = fresh;
                        }
                    }
                }
                Some(types::Event::CloseRequested(id)) if id == win => {
                    break;
                }
                // Resized: recompute the layout from the canvas's new size.
                // Hit-testing follows, because it reads this same layout.
                Some(types::Event::Resized(_)) | Some(types::Event::RedrawRequested(_)) => {
                    if let Ok(fresh) = draw(canvas, &timer) {
                        layout = fresh;
                    }
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

// Layout rectangles and hit-testing both live on `Layout` -- see
// `Layout::primary_rect` and `Layout::ghost_rect`.

fn hit(x: f32, y: f32, r: gfx::Rect) -> bool {
    x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

/// Ask the canvas its size, then draw the timer to that answer. Returns the
/// layout used, so the event loop hit-tests the picture on screen.
fn draw(canvas: u64, timer: &Timer) -> Result<Layout, gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas)?;
    let layout = Layout::for_size(size.width, size.height);
    draw_with(canvas, &layout, timer)?;
    Ok(layout)
}

fn draw_with(canvas: u64, layout: &Layout, timer: &Timer) -> Result<(), gfx::GfxError> {
    let cx = layout.cx;
    let ring_cy = layout.ring_cy;
    // Ground: the house near-black blue, top to bottom.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: layout.width, height: layout.height },
        BG_TOP,
        BG_BOT,
    )?;

    // A faint accent pool behind the ring so it sits in its own light.
    canvas2d::radial_gradient(
        canvas,
        gfx::Point { x: cx, y: ring_cy },
        layout.ring_r + 80.0,
        tint(ACCENT, 0.07),
        tint(ACCENT, 0.0),
    )?;

    // Quiet caption up top, hand-tracked with spaces so it reads as a wordmark.
    let cap = "P O M O D O R O";
    let csize = 13.0;
    let cw = text_width(cap, csize);
    draw_text(canvas, cap, cx - cw * 0.5, layout.caption_y, csize, INK_QUIET)?;

    draw_ring(canvas, layout, timer.progress())?;

    // ---- remaining time, big, centered in the ring ----
    let mut buf = [0u8; 8];
    let text = fmt_mmss(timer.remaining_ms, &mut buf);
    if let Ok(txt) = core::str::from_utf8(text) {
        let size = layout.time_size;
        let w = time_width(txt, size);
        let x = cx - w * 0.5;
        let y = ring_cy + size * 0.125;
        // Double-strike for weight: real type, faux bold.
        draw_text(canvas, txt, x + 0.7, y, size, INK)?;
        draw_text(canvas, txt, x, y, size, INK)?;
    }

    // ---- session label under the time ----
    let label = timer.phase.label();
    let lsize = layout.label_size;
    let lw = text_width(label, lsize);
    draw_text(canvas, label, cx - lw * 0.5, ring_cy + layout.ring_r * 0.27, lsize, INK_DIM)?;

    // ---- session dots: pomodoros banked this cycle ----
    let filled = timer.dots_filled();
    for i in 0..4u64 {
        let x = cx + (i as f32 - 1.5) * DOT_GAP;
        // Empty dots sit a step above the ring track so they stay legible at
        // this small size on the dark ground.
        let c = if i < filled { ACCENT } else { rgb(0x31, 0x3A, 0x4C) };
        disc(canvas, x, layout.dots_y, DOT_R, c)?;
    }

    // ---- buttons ----
    let pr = layout.primary_rect();
    rounded_rect(canvas, pr.x, pr.y, pr.width, pr.height, 12.0, ACCENT)?;
    let verb = timer.verb();
    let vsize = 17.0;
    let vw = text_width(verb, vsize);
    draw_text(
        canvas,
        verb,
        pr.x + (pr.width - vw) * 0.5,
        pr.y + pr.height * 0.5 + 6.0,
        vsize,
        gfx::Color { r: 1.0, g: 1.0, b: 1.0, a: 1.0 },
    )?;

    let gr = layout.ghost_rect();
    // Ghost: hairline border, hollow body. The body is refilled with the
    // ground color sampled at the button's band so the border reads 1px.
    rounded_rect(canvas, gr.x, gr.y, gr.width, gr.height, 12.0, TRACK)?;
    let inner = bg_at((gr.y + gr.height * 0.5) / layout.height);
    rounded_rect(canvas, gr.x + 1.0, gr.y + 1.0, gr.width - 2.0, gr.height - 2.0, 11.0, inner)?;
    let reset = "Reset";
    let rw = text_width(reset, vsize);
    draw_text(
        canvas,
        reset,
        gr.x + (gr.width - rw) * 0.5,
        gr.y + gr.height * 0.5 + 6.0,
        vsize,
        INK_DIM,
    )?;

    canvas2d::present(canvas)?;
    Ok(())
}

/// The ring: a full track circle plus the elapsed arc, both drawn as runs of
/// overlapping discs along the circle so the band has round, soft edges. The
/// arc starts at 12 o'clock and sweeps clockwise; its leading end gets a
/// slightly larger head dot so progress has a focal point.
fn draw_ring(canvas: u64, layout: &Layout, progress: f32) -> Result<(), gfx::GfxError> {
    const TWO_PI: f32 = 6.283_185_5;
    let dot_r = layout.ring_stroke * 0.5;
    // Disc spacing along the circumference: ~1.5px keeps the band smooth.
    let steps = 720u32;
    let step = TWO_PI / steps as f32;

    // Track, full turn.
    for i in 0..steps {
        let a = i as f32 * step;
        let (x, y) = ring_point(layout, a);
        disc(canvas, x, y, dot_r, TRACK)?;
    }

    // Elapsed arc on top.
    let sweep = (progress.clamp(0.0, 1.0) * steps as f32) as u32;
    for i in 0..=sweep.min(steps) {
        let a = i as f32 * step;
        let (x, y) = ring_point(layout, a);
        disc(canvas, x, y, dot_r, ACCENT)?;
    }

    // Head dot with a soft glow at the leading edge.
    if progress > 0.0 {
        let a = progress.clamp(0.0, 1.0) * TWO_PI;
        let (x, y) = ring_point(layout, a);
        canvas2d::radial_gradient(
            canvas,
            gfx::Point { x, y },
            dot_r * 2.6,
            tint(ACCENT, 0.45),
            tint(ACCENT, 0.0),
        )?;
        disc(canvas, x, y, dot_r + 1.5, gfx::Color { r: 0.78, g: 0.87, b: 1.0, a: 1.0 })?;
    }

    Ok(())
}

/// Point on the ring at `a` radians past 12 o'clock, clockwise.
fn ring_point(layout: &Layout, a: f32) -> (f32, f32) {
    const HALF_PI: f32 = 1.570_796_3;
    // Screen angle: 12 o'clock is -90 deg; clockwise means +angle in screen
    // coords (y down).
    let t = a - HALF_PI;
    (
        layout.cx + layout.ring_r * cos_approx(t),
        layout.ring_cy + layout.ring_r * sin_approx(t),
    )
}

// ------------------------------------------------------------------
// Tiny trig: a wrapped parabolic sine, refined. Max error ~0.001 —
// invisible at ring scale. No libm anywhere.
// ------------------------------------------------------------------

fn absf(x: f32) -> f32 {
    if x < 0.0 { -x } else { x }
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

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

fn rounded_rect(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
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

fn draw_text(canvas: u64, text: &str, x: f32, y: f32, size: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text(canvas, text, gfx::Point { x, y }, size, c)
}

/// The ground gradient sampled at `t` (0 top, 1 bottom), for hollow fills.
fn bg_at(t: f32) -> gfx::Color {
    let t = t.clamp(0.0, 1.0);
    gfx::Color {
        r: BG_TOP.r + (BG_BOT.r - BG_TOP.r) * t,
        g: BG_TOP.g + (BG_BOT.g - BG_TOP.g) * t,
        b: BG_TOP.b + (BG_BOT.b - BG_TOP.b) * t,
        a: 1.0,
    }
}

/// Rough advance for UI labels in the system face: ~0.52em per char.
fn text_width(s: &str, size: f32) -> f32 {
    (s.chars().count() as f32) * size * 0.52
}

/// A plain decimal number into a byte buffer, panic-free. Used by the resize
/// self-check to report the size it actually laid out to.
fn u32_bytes(value: u32, buf: &mut [u8; 12]) -> &[u8] {
    let mut scratch = [0u8; 10];
    let mut n = value;
    let mut count = if value == 0 { 1usize } else { 0usize };
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

/// Advance for the mm:ss readout: digits are wider than the colon.
fn time_width(s: &str, size: f32) -> f32 {
    let mut w = 0.0f32;
    for ch in s.chars() {
        w += if ch == ':' { size * 0.30 } else { size * 0.56 };
    }
    w
}

// ------------------------------------------------------------------
// Numbers <-> bytes, panic-free
// ------------------------------------------------------------------

fn parse_u64(bytes: &[u8]) -> u64 {
    let mut value = 0u64;
    for byte in bytes {
        if byte.is_ascii_digit() {
            value = value.saturating_mul(10).saturating_add(u64::from(byte - b'0'));
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
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

bindings::export!(Component with_types_in bindings);
