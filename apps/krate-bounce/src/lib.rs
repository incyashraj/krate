//! Krate bounce — the first app that animates.
//!
//! A ball moving under gravity, redrawn every frame with `gfx.canvas2d`. It
//! exists to answer a question the chart could not: can a Krate app move on
//! its own, or only react to clicks? The loop is the ordinary game loop —
//! measure elapsed time, advance the simulation by that much, draw, ask for
//! the next frame — and `quick` reports the frame rate it actually achieved so
//! a regression in the redraw path shows up as a number rather than a feeling.
//!
//! Physics is time-based, not frame-based. A frame-based ball falls faster on
//! a fast machine, which is the oldest bug in games and the reason the same
//! `.krate` file must not behave differently on a gaming desktop and a laptop.

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 240.0;
const HEIGHT: f32 = 200.0;
const RADIUS: f32 = 10.0;
/// Pixels per second, per second. Tuned to look like a ball, not a meteor.
const GRAVITY: f32 = 520.0;
/// How much speed survives a bounce. Below 1.0 the ball settles, which is
/// what makes the simulation obviously time-based when watched.
const RESTITUTION: f32 = 0.82;
/// Frames the `quick` run measures before reporting.
const QUICK_FRAMES: u32 = 60;

struct Component;

struct Ball {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
}

impl Ball {
    fn step(&mut self, dt: f32) {
        self.dy += GRAVITY * dt;
        self.x += self.dx * dt;
        self.y += self.dy * dt;

        // Walls and floor. Clamping position as well as flipping velocity
        // stops the ball sticking when a frame is long enough to carry it past
        // the edge -- a stall must not leave the ball outside the box.
        if self.x - RADIUS < 0.0 {
            self.x = RADIUS;
            self.dx = -self.dx;
        }
        if self.x + RADIUS > WIDTH {
            self.x = WIDTH - RADIUS;
            self.dx = -self.dx;
        }
        if self.y + RADIUS > HEIGHT {
            self.y = HEIGHT - RADIUS;
            self.dy = -self.dy * RESTITUTION;
        }
        if self.y - RADIUS < 0.0 {
            self.y = RADIUS;
            self.dy = -self.dy * RESTITUTION;
        }
    }
}

fn color(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
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
            grow: 1.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// An 8x8 sprite: a round yellow blob with transparent corners.
///
/// Built once at startup rather than decoded from a file, so the sample stays
/// dependency-free -- the point is that `draw-pixels` blends a sprite over the
/// canvas, not where the bytes came from. A real game decodes a sprite sheet
/// with `zune-png` and slices it the same way.
fn ball_sprite() -> [u8; 8 * 8 * 4] {
    let mut rgba = [0_u8; 8 * 8 * 4];
    for y in 0..8_i32 {
        for x in 0..8_i32 {
            // Distance from centre, in half-pixels, without sqrt.
            let dx = x * 2 - 7;
            let dy = y * 2 - 7;
            let inside = dx * dx + dy * dy <= 7 * 7;
            let at = ((y * 8 + x) * 4) as usize;
            if inside {
                if let Some(px) = rgba.get_mut(at..at + 4) {
                    px[0] = 253;
                    px[1] = 191;
                    px[2] = 45;
                    px[3] = 255;
                }
            }
            // Outside stays fully transparent, which is what the canvas must
            // show through.
        }
    }
    rgba
}

/// Draw one frame: sky, then the ball as a sprite.
fn draw(canvas: u64, ball: &Ball) -> Result<(), gfx::GfxError> {
    canvas2d::clear(canvas, color(0.09, 0.11, 0.16))?;

    // The ball is a sprite now: transparent corners let the sky show through,
    // which a filled rectangle could never do.
    let sprite = ball_sprite();
    canvas2d::draw_pixels(
        canvas,
        gfx::Rect {
            x: ball.x - RADIUS,
            y: ball.y - RADIUS,
            width: RADIUS * 2.0,
            height: RADIUS * 2.0,
        },
        8,
        8,
        &sprite,
    )?;

    // The floor line, so the bounce has something to bounce against.
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x: 0.0,
            y: HEIGHT - 2.0,
            width: WIDTH,
            height: 2.0,
        },
        color(0.35, 0.38, 0.45),
    )?;

    canvas2d::present(canvas)
}

fn out(text: &str) {
    let handle = stdio::stdout();
    let _ = handle.write(text.as_bytes());
    let _ = handle.write(b"\n");
}

/// Write a small unsigned number without `format!`.
///
/// A `no_std` guest that reaches for `.to_string()` or `format!` pulls std's
/// out-of-memory handler in, and with it the whole `wasi:*` import surface.
fn out_number(label: &str, value: u64) {
    let mut digits = [0_u8; 20];
    let mut len = 0;
    let mut value = value;
    if value == 0 {
        if let Some(slot) = digits.get_mut(0) {
            *slot = b'0';
        }
        len = 1;
    }
    while value > 0 && len < digits.len() {
        if let Some(slot) = digits.get_mut(len) {
            *slot = b'0' + (value % 10) as u8;
        }
        value /= 10;
        len += 1;
    }
    let handle = stdio::stdout();
    let _ = handle.write(label.as_bytes());
    for index in (0..len).rev() {
        if let Some(byte) = digits.get(index..index + 1) {
            let _ = handle.write(byte);
        }
    }
    let _ = handle.write(b"\n");
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw.split_whitespace().any(|word| word == "quick");

        let win = match window::create(
            "Bounce",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) {
            Ok(win) => win,
            Err(_) => {
                out("window:no");
                return 1;
            }
        };
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            out("tree:no");
            return 1;
        }
        let _ = tree::upsert_node(win, &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas));

        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => {
                out("bind:no");
                return 1;
            }
        };

        let mut ball = Ball {
            x: WIDTH / 2.0,
            y: RADIUS * 2.0,
            dx: 95.0,
            dy: 0.0,
        };

        let started = clock::monotonic_nanos();
        let mut last = started;
        let mut frames: u32 = 0;

        loop {
            let now = clock::monotonic_nanos();
            // Seconds since the previous frame, capped: if the app was paused
            // or the machine stalled, advancing the simulation by the whole
            // gap would teleport the ball through the floor.
            let dt = ((now - last) as f32 / 1_000_000_000.0).min(0.05);
            last = now;

            ball.step(dt);
            if draw(canvas, &ball).is_err() {
                out("draw:no");
                return 1;
            }
            frames += 1;

            if quick {
                if frames >= QUICK_FRAMES {
                    break;
                }
                continue;
            }

            // Ask for the next frame, then take whatever input arrived. A
            // short timeout keeps the loop running when nothing is happening,
            // which is what makes the ball keep moving.
            let _ = window::request_redraw(win);
            match events::wait(Some(16)) {
                Some(types::Event::CloseRequested(_)) => break,
                Some(_) | None => {}
            }
        }

        let elapsed_nanos = clock::monotonic_nanos() - started;
        out_number("frames:", frames as u64);
        // Frames per second, times 100, so the number carries two decimals
        // without any floating-point formatting.
        let fps_centi = if elapsed_nanos > 0 {
            (frames as u64 * 100 * 1_000_000_000) / elapsed_nanos
        } else {
            0
        };
        out_number("fps-centi:", fps_centi);
        out("animated:yes");

        let _ = window::close(win);
        0
    }
}

bindings::export!(Component with_types_in bindings);
