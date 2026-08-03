//! Krate paint — the limitation probe for fast animation.
//!
//! The wall it tests: can an app run a smooth real-time loop, drawing a fresh
//! frame many times a second? A game, a visualizer, a live chart -- all of it
//! needs clear, draw, present, repeat, without stutter and without the frame
//! budget spiralling. This app bounces a field of balls off the walls, redraws
//! the whole canvas every frame, and reports the frame count and average rate
//! it sustained, so a screenshot shows a moment of motion and stdout proves it
//! was really animating.
//!
//! Fixed-size arrays, integer-free physics on f32, no allocation in the loop,
//! no panic paths -- only `krate:*` imports.

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 480.0;
const HEIGHT: f32 = 360.0;
const BALL_COUNT: usize = 24;
const BALL_RADIUS: f32 = 12.0;

/// Frames drawn in a quick automated run before reporting. Enough to prove the
/// loop sustains, small enough to finish fast under the headless budget.
const QUICK_FRAMES: u32 = 90;
/// Interactive runs animate until the window closes or this cap.
const MAX_FRAMES: u32 = 5000;

struct Component;

/// One ball: position and velocity in pixels and pixels-per-second.
#[derive(Clone, Copy)]
struct Ball {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    r: f32,
    g: f32,
    b: f32,
}

impl Ball {
    const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        vx: 0.0,
        vy: 0.0,
        r: 0.0,
        g: 0.0,
        b: 0.0,
    };

    fn step(&mut self, dt: f32) {
        self.x += self.vx * dt;
        self.y += self.vy * dt;
        // Bounce off the four walls, keeping the whole ball on screen.
        if self.x < BALL_RADIUS {
            self.x = BALL_RADIUS;
            self.vx = -self.vx;
        } else if self.x > WIDTH - BALL_RADIUS {
            self.x = WIDTH - BALL_RADIUS;
            self.vx = -self.vx;
        }
        if self.y < BALL_RADIUS {
            self.y = BALL_RADIUS;
            self.vy = -self.vy;
        } else if self.y > HEIGHT - BALL_RADIUS {
            self.y = HEIGHT - BALL_RADIUS;
            self.vy = -self.vy;
        }
    }
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Bouncing balls", size) else {
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
            Ok(canvas) => canvas,
            Err(_) => {
                let _ = window::close(win);
                return 33;
            }
        };

        // Seed the balls from a simple hash so they spread across the field
        // with varied directions and colors, deterministic for a stable shot.
        let mut balls = [Ball::ZERO; BALL_COUNT];
        let mut i = 0usize;
        while i < BALL_COUNT {
            let seed = (i as u32).wrapping_mul(2654435761);
            let fx = ((seed & 0xFF) as f32) / 255.0;
            let fy = (((seed >> 8) & 0xFF) as f32) / 255.0;
            let fdx = (((seed >> 16) & 0xFF) as f32) / 255.0 - 0.5;
            let fdy = (((seed >> 24) & 0xFF) as f32) / 255.0 - 0.5;
            balls[i] = Ball {
                x: BALL_RADIUS + fx * (WIDTH - 2.0 * BALL_RADIUS),
                y: BALL_RADIUS + fy * (HEIGHT - 2.0 * BALL_RADIUS),
                vx: fdx * 260.0,
                vy: fdy * 260.0,
                r: 0.35 + fx * 0.6,
                g: 0.4 + fy * 0.5,
                b: 0.7 - fx * 0.3,
            };
            i += 1;
        }

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");
        let frame_cap = if quick { QUICK_FRAMES } else { MAX_FRAMES };

        let started = clock::monotonic_nanos();
        let mut last = started;
        let mut frames = 0u32;
        let mut closed = false;

        while frames < frame_cap {
            let now = clock::monotonic_nanos();
            let dt = (now.saturating_sub(last) as f32 / 1_000_000_000.0).min(0.05);
            last = now;

            // Step physics and draw the whole frame.
            let mut b = 0usize;
            while b < BALL_COUNT {
                balls[b].step(dt);
                b += 1;
            }
            if draw_frame(canvas, &balls).is_err() {
                break;
            }
            frames += 1;

            // Drain events so a close is honored promptly; a game does not
            // block waiting for input.
            if let Some(types::Event::CloseRequested(id)) = events::poll() {
                if id == win {
                    closed = true;
                    break;
                }
            }
        }

        let elapsed = clock::monotonic_nanos().saturating_sub(started);
        report(frames, elapsed);

        let _ = window::close(win);
        if closed {
            0
        } else {
            0
        }
    }
}

/// Clear and redraw the whole field. A square stands in for each ball -- the
/// painter has fast rectangle fills, and the probe measures the loop, not
/// circle quality.
fn draw_frame(canvas: u64, balls: &[Ball; BALL_COUNT]) -> Result<(), gfx::GfxError> {
    canvas2d::clear(canvas, color(0.06, 0.07, 0.11, 1.0))?;
    let mut i = 0usize;
    while i < BALL_COUNT {
        let ball = balls[i];
        canvas2d::fill_rect(
            canvas,
            gfx::Rect {
                x: ball.x - BALL_RADIUS,
                y: ball.y - BALL_RADIUS,
                width: BALL_RADIUS * 2.0,
                height: BALL_RADIUS * 2.0,
            },
            color(ball.r, ball.g, ball.b, 1.0),
        )?;
        i += 1;
    }
    canvas2d::present(canvas)?;
    Ok(())
}

/// Print `frames:N` and `fps-centi:M` (frames per second times 100, so no
/// float printing) for a script to assert the loop really ran.
fn report(frames: u32, elapsed_nanos: u64) {
    let out = stdio::stdout();
    let _ = out.write(b"frames:");
    let _ = out.write(u32_slice(frames, &mut [0u8; 12]));
    let _ = out.write(b"\n");

    if elapsed_nanos > 0 {
        // fps * 100 = frames * 100 * 1e9 / elapsed_nanos, in integer math.
        let fps_centi = (u64::from(frames) * 100 * 1_000_000_000) / elapsed_nanos;
        let _ = out.write(b"fps-centi:");
        let _ = out.write(u64_slice(fps_centi, &mut [0u8; 20]));
        let _ = out.write(b"\n");
    }
}

// ----- widget builders -----

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

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

// ----- number formatting, panic-free -----

fn u32_slice(value: u32, buf: &mut [u8; 12]) -> &[u8] {
    let mut wide = [0u8; 20];
    let slice = u64_slice(u64::from(value), &mut wide);
    let len = slice.len().min(buf.len());
    let mut i = 0usize;
    while i < len {
        if let (Some(src), Some(dst)) = (slice.get(i), buf.get_mut(i)) {
            *dst = *src;
        }
        i += 1;
    }
    buf.get(..len).unwrap_or(b"0")
}

fn u64_slice(value: u64, buf: &mut [u8; 20]) -> &[u8] {
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

bindings::export!(Component with_types_in bindings);
