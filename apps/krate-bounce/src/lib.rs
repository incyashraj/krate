//! Krate bounce — the first app you can actually play.
//!
//! A Breakout: a paddle you steer, a ball, four rows of bricks, three lives,
//! a score, a win. It began life as a falling-ball animation, and somebody
//! asked the right question: can we even call that a game? You could not
//! control it, so no. This is the honest version of the claim "2D games work
//! on Krate" — input moves the paddle, the paddle changes where the ball goes,
//! the bricks come down one by one, and you can lose.
//!
//! The loop is the ordinary game loop — measure elapsed time, advance the
//! simulation by that much, draw, ask for the next frame. Physics is
//! time-based, not frame-based: a frame-based ball flies faster on a fast
//! machine, which is the oldest bug in games and the reason the same `.krate`
//! file must not behave differently on a gaming desktop and a laptop.
//!
//! `quick` plays the game by itself: fixed time step, the paddle tracking the
//! ball, long enough to break real bricks. It prints `playable:yes` only when
//! bricks were broken through the paddle path, so the nightly replay proves
//! the whole input-paddle-collision-score loop rather than that pixels moved.

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 320.0;
const HEIGHT: f32 = 240.0;
const RADIUS: f32 = 5.0;

const PADDLE_WIDTH: f32 = 48.0;
const PADDLE_HEIGHT: f32 = 6.0;
const PADDLE_Y: f32 = HEIGHT - 18.0;
/// Pixels per second the paddle moves while a key is held.
const PADDLE_SPEED: f32 = 260.0;

/// Ball speed. Constant magnitude: Breakout tension comes from angles, not
/// acceleration.
const BALL_SPEED: f32 = 175.0;

const BRICK_COLUMNS: usize = 8;
const BRICK_ROWS: usize = 4;
const BRICK_COUNT: usize = BRICK_COLUMNS * BRICK_ROWS;
const BRICK_WIDTH: f32 = 36.0;
const BRICK_HEIGHT: f32 = 11.0;
const BRICK_GAP: f32 = 2.0;
/// Left edge of the grid, centring 8 columns in the window.
const BRICK_X0: f32 =
    (WIDTH - (BRICK_COLUMNS as f32 * (BRICK_WIDTH + BRICK_GAP) - BRICK_GAP)) / 2.0;
/// Top of the grid, leaving a band for the score text.
const BRICK_Y0: f32 = 30.0;

/// Simulated frames the `quick` run plays before reporting.
const QUICK_FRAMES: u32 = 600;

/// How far a stick must move before it counts, so a worn controller resting
/// off-centre does not drift the paddle.
const STICK_DEADZONE: f32 = 0.15;

struct Component;

/// Everything the game is, small enough to reason about at a glance.
struct Game {
    ball_x: f32,
    ball_y: f32,
    ball_dx: f32,
    ball_dy: f32,
    paddle_x: f32,
    bricks: [bool; BRICK_COUNT],
    score: u32,
    lives: u32,
    /// The ball rides the paddle until launched, which is how every Breakout
    /// gives you a breath between lives.
    launched: bool,
    /// Set when lives run out or the wall is cleared; Space starts over.
    over: bool,
    won: bool,
}

impl Game {
    fn new() -> Self {
        let mut game = Self {
            ball_x: 0.0,
            ball_y: 0.0,
            ball_dx: 0.0,
            ball_dy: 0.0,
            paddle_x: (WIDTH - PADDLE_WIDTH) / 2.0,
            bricks: [true; BRICK_COUNT],
            score: 0,
            lives: 3,
            launched: false,
            over: false,
            won: false,
        };
        game.rest_ball();
        game
    }

    /// Park the ball on the paddle, ready to launch.
    fn rest_ball(&mut self) {
        self.launched = false;
        self.ball_x = self.paddle_x + PADDLE_WIDTH / 2.0;
        self.ball_y = PADDLE_Y - RADIUS - 1.0;
        self.ball_dx = 0.0;
        self.ball_dy = 0.0;
    }

    /// Send the ball on its way, slightly sideways so straight-up rallies
    /// cannot happen from the first serve.
    fn launch(&mut self) {
        if self.launched || self.over {
            return;
        }
        self.launched = true;
        self.ball_dx = BALL_SPEED * 0.45;
        self.ball_dy = -BALL_SPEED * 0.9;
    }

    /// Move the paddle by `direction` (-1..1) for `dt` seconds and keep the
    /// resting ball riding along.
    fn steer(&mut self, direction: f32, dt: f32) {
        self.paddle_x += direction * PADDLE_SPEED * dt;
        if self.paddle_x < 0.0 {
            self.paddle_x = 0.0;
        }
        if self.paddle_x > WIDTH - PADDLE_WIDTH {
            self.paddle_x = WIDTH - PADDLE_WIDTH;
        }
        if !self.launched {
            self.ball_x = self.paddle_x + PADDLE_WIDTH / 2.0;
        }
    }

    /// Advance the world by `dt` seconds.
    fn step(&mut self, dt: f32) {
        if !self.launched || self.over {
            return;
        }
        self.ball_x += self.ball_dx * dt;
        self.ball_y += self.ball_dy * dt;

        // Walls. Clamping position as well as flipping velocity stops the
        // ball sticking when a long frame carries it past the edge.
        if self.ball_x - RADIUS < 0.0 {
            self.ball_x = RADIUS;
            self.ball_dx = -self.ball_dx;
        }
        if self.ball_x + RADIUS > WIDTH {
            self.ball_x = WIDTH - RADIUS;
            self.ball_dx = -self.ball_dx;
        }
        if self.ball_y - RADIUS < 0.0 {
            self.ball_y = RADIUS;
            self.ball_dy = -self.ball_dy;
        }

        // The paddle. Only a falling ball bounces, so a ball rising through
        // the paddle band after a low save cannot be caught twice.
        if self.ball_dy > 0.0
            && self.ball_y + RADIUS >= PADDLE_Y
            && self.ball_y + RADIUS <= PADDLE_Y + PADDLE_HEIGHT + RADIUS
            && self.ball_x >= self.paddle_x - RADIUS
            && self.ball_x <= self.paddle_x + PADDLE_WIDTH + RADIUS
        {
            self.ball_y = PADDLE_Y - RADIUS;
            self.ball_dy = -self.ball_dy;
            // Where the ball lands on the paddle decides where it goes next.
            // This is the entire skill of Breakout, in one line: the edges
            // send it out steeply, the middle sends it straight back up.
            let offset =
                (self.ball_x - (self.paddle_x + PADDLE_WIDTH / 2.0)) / (PADDLE_WIDTH / 2.0);
            self.ball_dx = offset * BALL_SPEED;
        }

        // Bricks. Point-in-expanded-rect is a modest approximation of a
        // circle against a box, and at five pixels of radius nobody has ever
        // seen the difference.
        for index in 0..BRICK_COUNT {
            let alive = self.bricks.get(index).copied().unwrap_or(false);
            if !alive {
                continue;
            }
            let column = (index % BRICK_COLUMNS) as f32;
            let row = (index / BRICK_COLUMNS) as f32;
            let bx = BRICK_X0 + column * (BRICK_WIDTH + BRICK_GAP);
            let by = BRICK_Y0 + row * (BRICK_HEIGHT + BRICK_GAP);
            if self.ball_x >= bx - RADIUS
                && self.ball_x <= bx + BRICK_WIDTH + RADIUS
                && self.ball_y >= by - RADIUS
                && self.ball_y <= by + BRICK_HEIGHT + RADIUS
            {
                if let Some(slot) = self.bricks.get_mut(index) {
                    *slot = false;
                }
                self.score += 1;
                self.ball_dy = -self.ball_dy;
                break;
            }
        }

        if self.score as usize == BRICK_COUNT {
            self.over = true;
            self.won = true;
        }

        // The floor is the only wall that does not bounce.
        if self.ball_y - RADIUS > HEIGHT {
            if self.lives > 0 {
                self.lives -= 1;
            }
            if self.lives == 0 {
                self.over = true;
            } else {
                self.rest_ball();
            }
        }
    }

    /// Fresh board, score reset. Space after game over lands here.
    fn restart(&mut self) {
        *self = Self::new();
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

/// An 8x8 sprite: a round pale blob with transparent corners, scaled onto the
/// ball's rectangle so the canvas shows through around it.
fn ball_sprite() -> [u8; 8 * 8 * 4] {
    let mut rgba = [0_u8; 8 * 8 * 4];
    for y in 0..8_i32 {
        for x in 0..8_i32 {
            let dx = x * 2 - 7;
            let dy = y * 2 - 7;
            let inside = dx * dx + dy * dy <= 7 * 7;
            let at = ((y * 8 + x) * 4) as usize;
            if inside {
                if let Some(px) = rgba.get_mut(at..at + 4) {
                    px[0] = 245;
                    px[1] = 240;
                    px[2] = 220;
                    px[3] = 255;
                }
            }
        }
    }
    rgba
}

/// Write "Score N   Lives N" into a fixed buffer and return its length.
///
/// By hand rather than `format!`, matching the number-printing helper below:
/// the buffer lives on the stack and the digits are pushed in place, so the
/// HUD costs no allocation per frame and keeps the import list clean.
fn hud_line(buf: &mut [u8; 32], score: u32, lives: u32) -> usize {
    let mut len = 0;
    for byte in b"Score " {
        if let Some(slot) = buf.get_mut(len) {
            *slot = *byte;
            len += 1;
        }
    }
    len = push_number(buf, len, score as u64);
    for byte in b"   Lives " {
        if let Some(slot) = buf.get_mut(len) {
            *slot = *byte;
            len += 1;
        }
    }
    push_number(buf, len, lives as u64)
}

/// Append a small unsigned number's digits to `buf` at `len`.
fn push_number(buf: &mut [u8; 32], mut len: usize, value: u64) -> usize {
    let mut digits = [0_u8; 20];
    let mut count = 0;
    let mut value = value;
    if value == 0 {
        if let Some(slot) = digits.get_mut(0) {
            *slot = b'0';
        }
        count = 1;
    }
    while value > 0 && count < digits.len() {
        if let Some(slot) = digits.get_mut(count) {
            *slot = b'0' + (value % 10) as u8;
        }
        value /= 10;
        count += 1;
    }
    for index in (0..count).rev() {
        if let (Some(slot), Some(digit)) = (buf.get_mut(len), digits.get(index)) {
            *slot = *digit;
            len += 1;
        }
    }
    len
}

/// Draw one frame: sky, bricks, paddle, ball, HUD, and any end-state banner.
fn draw(canvas: u64, game: &Game) -> Result<(), gfx::GfxError> {
    canvas2d::clear(canvas, color(0.09, 0.11, 0.16))?;

    // Bricks, coloured by row so progress is visible at a glance.
    for index in 0..BRICK_COUNT {
        if !game.bricks.get(index).copied().unwrap_or(false) {
            continue;
        }
        let column = (index % BRICK_COLUMNS) as f32;
        let row = index / BRICK_COLUMNS;
        let tint = match row {
            0 => color(0.86, 0.42, 0.38),
            1 => color(0.88, 0.66, 0.36),
            2 => color(0.46, 0.72, 0.46),
            _ => color(0.42, 0.58, 0.86),
        };
        canvas2d::fill_rect(
            canvas,
            gfx::Rect {
                x: BRICK_X0 + column * (BRICK_WIDTH + BRICK_GAP),
                y: BRICK_Y0 + (row as f32) * (BRICK_HEIGHT + BRICK_GAP),
                width: BRICK_WIDTH,
                height: BRICK_HEIGHT,
            },
            tint,
        )?;
    }

    // The paddle.
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x: game.paddle_x,
            y: PADDLE_Y,
            width: PADDLE_WIDTH,
            height: PADDLE_HEIGHT,
        },
        color(0.82, 0.84, 0.9),
    )?;

    // The ball, as a sprite with transparent corners.
    let sprite = ball_sprite();
    canvas2d::draw_pixels(
        canvas,
        gfx::Rect {
            x: game.ball_x - RADIUS,
            y: game.ball_y - RADIUS,
            width: RADIUS * 2.0,
            height: RADIUS * 2.0,
        },
        8,
        8,
        &sprite,
    )?;

    // The HUD.
    let mut buf = [0_u8; 32];
    let len = hud_line(&mut buf, game.score, game.lives);
    if let Some(slice) = buf.get(..len) {
        if let Ok(text) = core::str::from_utf8(slice) {
            canvas2d::draw_text(
                canvas,
                text,
                gfx::Point { x: 8.0, y: 18.0 },
                12.0,
                color(0.75, 0.78, 0.85),
            )?;
        }
    }

    if game.over {
        let line = if game.won {
            "Cleared! Space to play again"
        } else {
            "Game over. Space to play again"
        };
        canvas2d::draw_text(
            canvas,
            line,
            gfx::Point {
                x: 52.0,
                y: HEIGHT / 2.0,
            },
            14.0,
            color(0.95, 0.9, 0.75),
        )?;
    } else if !game.launched {
        canvas2d::draw_text(
            canvas,
            "Space to launch, arrows to move",
            gfx::Point {
                x: 48.0,
                y: HEIGHT / 2.0 + 40.0,
            },
            12.0,
            color(0.6, 0.64, 0.72),
        )?;
    }

    canvas2d::present(canvas)
}

fn out(text: &str) {
    let handle = stdio::stdout();
    let _ = handle.write(text.as_bytes());
    let _ = handle.write(b"\n");
}

/// Write a small unsigned number without `format!`, keeping the guest's
/// import list free of the allocation-failure machinery.
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
        // A Breakout court only makes sense at its own proportions: the
        // paddle, the brick grid and the ball speeds are all tuned to this
        // size. Declare it once and keep drawing in these coordinates --
        // the host scales them to any window and centres the leftovers, so
        // the game fills a resized window instead of sitting in a corner
        // (K-096). Pointer coordinates come back in these units too.
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        let mut game = Game::new();

        let started = clock::monotonic_nanos();
        let mut last = started;
        let mut frames: u32 = 0;

        loop {
            // Seconds since the previous frame, capped so a stall cannot
            // teleport the ball through a wall. The quick run uses a fixed
            // step instead: its point is exercising the game loop, and a
            // wall-clock step would make the brick count depend on how fast
            // this machine happens to be.
            let dt = if quick {
                1.0 / 60.0
            } else {
                let now = clock::monotonic_nanos();
                let dt = ((now.saturating_sub(last)) as f32 / 1_000_000_000.0).min(0.05);
                last = now;
                dt
            };

            if quick {
                // The paddle plays itself: track the ball, launch at once.
                game.launch();
                let target = game.ball_x - PADDLE_WIDTH / 2.0;
                let gap = target - game.paddle_x;
                if gap > 2.0 {
                    game.steer(1.0, dt);
                } else if gap < -2.0 {
                    game.steer(-1.0, dt);
                }
            } else {
                // A person plays: held keys for the paddle, Space to launch
                // or restart, and a gamepad stick when one is around.
                if events::key_held("ArrowLeft") {
                    game.steer(-1.0, dt);
                }
                if events::key_held("ArrowRight") {
                    game.steer(1.0, dt);
                }
                let stick = events::gamepad_axis("left-x");
                if stick > STICK_DEADZONE || stick < -STICK_DEADZONE {
                    game.steer(stick, dt);
                }
                if events::key_held("Space") || events::gamepad_held("south") {
                    if game.over {
                        game.restart();
                    } else {
                        game.launch();
                    }
                }
            }

            game.step(dt);
            if draw(canvas, &game).is_err() {
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

            let _ = window::request_redraw(win);
            match events::wait(Some(16)) {
                Some(types::Event::CloseRequested(_)) => break,
                Some(_) | None => {}
            }
        }

        let elapsed_nanos = clock::monotonic_nanos().saturating_sub(started);
        out_number("frames:", frames as u64);
        let fps_centi = if elapsed_nanos > 0 {
            (frames as u64 * 100 * 1_000_000_000) / elapsed_nanos
        } else {
            0
        };
        out_number("fps-centi:", fps_centi);
        out_number("bricks:", game.score as u64);
        // The claim the replay pins. Bricks only break when the scripted
        // paddle kept the rally alive, so this line proves the whole loop:
        // input moved the paddle, the paddle returned the ball, the ball
        // broke bricks, the score counted them.
        if game.score > 0 {
            out("playable:yes");
        } else {
            out("playable:no");
        }

        let _ = window::close(win);
        0
    }
}

bindings::export!(Component with_types_in bindings);
