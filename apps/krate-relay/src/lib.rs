//! Relay -- a platformer built by hand to find out what the runtime cannot do.
//!
//! CP-A of Plan/Proof-Checkpoints-2026-09-16.md. The question is not whether
//! Krate Studio can generate this; it is whether a complete, playable game
//! works properly on the runtime, and what is missing when it does not.
//!
//! So this is a real game rather than a demo loop: four hand-authored levels
//! of solid tiles and moving platforms, a runner with acceleration, friction,
//! coyote time and a jump buffer, collectible cells, spikes that kill, an
//! exit that ends the level, a HUD, a title screen, a pause screen, synthesised
//! sound effects, and a save that remembers your furthest level and best time
//! across runs.
//!
//! The four things being measured, per the checkpoint: does it WORK when
//! played like a person plays it; is MEMORY flat over a long session; is the
//! FRAME TIME stable rather than merely fast on average; and is the bundle a
//! defensible SIZE.
//!
//! `#![no_std]`, following krate-nova: the SDK owns the allocator and a
//! trapping panic handler, so nothing drags in the wasi:* import set. Entities
//! live in fixed-capacity arrays with an active count, numbers are formatted
//! by hand into byte buffers, and the hot loop allocates nothing.

#![no_std]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]

extern crate alloc;

use krate::audio::playback;
use krate::audio::playback::StreamConfig;
// `SampleFormat` is shared across the audio package, so it lives in
// `krate::audio::types` rather than in `playback` -- the SDK path exists, it
// is just one module over from where you first look.
use krate::audio::types::SampleFormat;
use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, types as gfx};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// Design space. Everything below is in these units; the host scales.
const WIDTH: f32 = 640.0;
const HEIGHT: f32 = 400.0;

const TILE: f32 = 20.0;
const COLS: usize = 32;
const ROWS: usize = 20;

// ---------------------------------------------------------------- levels

/// Four levels, drawn as text so they can be read and edited as pictures.
///
/// `#` solid, `=` one-way platform, `^` spike, `o` cell, `@` start, `E` exit,
/// `~` a moving platform's home cell.
const LEVELS: [[&str; ROWS]; 4] = [
    [
        "################################",
        "#                              #",
        "#                              #",
        "#   @                          #",
        "#  ###                         #",
        "#                    o         #",
        "#                  =====       #",
        "#         o                    #",
        "#      ======            o     #",
        "#                      =====   #",
        "#   o                          #",
        "#  ====        ^^        ===   #",
        "#           ########           #",
        "#                        o     #",
        "#      ===            ======   #",
        "#                            E #",
        "#                        ##### #",
        "#          ^^^^                #",
        "##############       ###########",
        "################################",
    ],
    [
        "################################",
        "#                              #",
        "#  @                           #",
        "# ###                    o     #",
        "#                      =====   #",
        "#        o                     #",
        "#     ~~~~~                    #",
        "#                   o          #",
        "#                 ====         #",
        "#   o                          #",
        "#  ===       ^^^^^^        o   #",
        "#         #########      ===== #",
        "#                              #",
        "#     ~~~~~                  E #",
        "#                        ##### #",
        "#                o             #",
        "#            =======           #",
        "#   ^^^^^^^^            ^^^^   #",
        "################################",
        "################################",
    ],
    [
        "################################",
        "#                              #",
        "# @        o           o       #",
        "####     =====       =====     #",
        "#                              #",
        "#    o                    o    #",
        "#  =====                =====  #",
        "#            ~~~~              #",
        "#                              #",
        "#   ^^^^^^^^^^^^^^^^^^^^^^^^   #",
        "#  ##########################  #",
        "#                              #",
        "#        o          o          #",
        "#      =====      =====        #",
        "#                              #",
        "#  ~~~~~                       #",
        "#                  o         E #",
        "#                =====   ##### #",
        "#   ^^^^^^^^^^^^^^^^^^^^^^^^^  #",
        "################################",
    ],
    [
        "################################",
        "#                              #",
        "#@   o    ^^^^    o        o   #",
        "###=====########=====    ===== #",
        "#                              #",
        "#     ~~~~                     #",
        "#                  ^^^^        #",
        "#           o    #######       #",
        "#        ======                #",
        "#                        o     #",
        "#   ^^^^^              =====   #",
        "#  #######                     #",
        "#              ~~~~~           #",
        "#   o                          #",
        "#  ====      ^^^^^^^^        E #",
        "#          ###########   ##### #",
        "#                              #",
        "#     ^^^^^^^^^^^^^^^^^^^^^^   #",
        "################################",
        "################################",
    ],
];

const LEVEL_COUNT: usize = 4;

// ------------------------------------------------------------- entities

const MAX_CELLS: usize = 32;
const MAX_MOVERS: usize = 8;
const MAX_SPARKS: usize = 96;

#[derive(Clone, Copy, PartialEq)]
enum Tile {
    Empty,
    Solid,
    Platform,
    Spike,
    Exit,
}

struct Cell {
    x: f32,
    y: f32,
    taken: bool,
    /// Bob phase, so the pickups are not a static grid of squares.
    phase: f32,
}

/// A platform that slides left and right between two walls.
struct Mover {
    x: f32,
    y: f32,
    w: f32,
    home: f32,
    span: f32,
    phase: f32,
    /// How far it moved last frame, so a rider moves with it.
    dx: f32,
}

struct Spark {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    life: f32,
    warm: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Title,
    Playing,
    Paused,
    Dead,
    LevelDone,
    Won,
}

struct Game {
    screen: Screen,
    level: usize,
    tiles: [[Tile; COLS]; ROWS],

    // The runner.
    px: f32,
    py: f32,
    vx: f32,
    vy: f32,
    on_ground: bool,
    face_right: bool,
    /// Seconds since leaving the ground, for coyote time.
    coyote: f32,
    /// Seconds since jump was pressed, for the jump buffer.
    jump_buffer: f32,
    /// Walk cycle phase, so the runner animates rather than slides.
    anim: f32,
    /// Squash and stretch on landing, which is most of what "juice" means.
    squash: f32,

    cells: [Cell; MAX_CELLS],
    cell_count: usize,
    collected: u32,
    total_cells: u32,

    movers: [Mover; MAX_MOVERS],
    mover_count: usize,

    sparks: [Spark; MAX_SPARKS],
    spark_count: usize,

    spawn_x: f32,
    spawn_y: f32,

    time: f32,
    best: f32,
    furthest: usize,
    deaths: u32,
    shake: f32,

    // Audio handles. Zero means "not available", which is a normal outcome:
    // a machine with no sound device still plays the game.
    stream: u64,
    snd_jump: u64,
    snd_pick: u64,
    snd_die: u64,
    snd_done: u64,
}

// ---------------------------------------------------------------- sound

const SAMPLE_RATE: u32 = 22_050;

/// Build a short sound into `out` as interleaved little-endian i16 mono.
///
/// Synthesised rather than loaded: a game that ships its own audio files is a
/// bigger bundle and a decoder in the app, and neither is needed for the four
/// noises this game makes. `shape` picks the timbre.
fn synth(out: &mut [u8], millis: u32, from_hz: f32, to_hz: f32, shape: u8) -> usize {
    let frames = (SAMPLE_RATE as f32 * millis as f32 / 1000.0) as usize;
    let frames = if frames * 2 > out.len() {
        out.len() / 2
    } else {
        frames
    };
    let mut phase = 0.0f32;
    for i in 0..frames {
        let t = i as f32 / frames as f32;
        let hz = from_hz + (to_hz - from_hz) * t;
        phase += hz / SAMPLE_RATE as f32;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        // A short attack and a long decay: an instant-on square is a click.
        let attack = if t < 0.02 { t / 0.02 } else { 1.0 };
        let decay = (1.0 - t) * (1.0 - t);
        let wave = match shape {
            // Square: bright, arcade.
            0 => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            // Triangle: softer, for pickups.
            1 => {
                if phase < 0.5 {
                    phase * 4.0 - 1.0
                } else {
                    3.0 - phase * 4.0
                }
            }
            // Noise-ish: a cheap deterministic hash, for the death sound.
            _ => {
                let n = (i as u32).wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((n >> 16) as f32 / 32_768.0) - 1.0
            }
        };
        let s = wave * attack * decay * 0.28;
        let v = (s * 32_000.0) as i16;
        let b = v.to_le_bytes();
        out[i * 2] = b[0];
        out[i * 2 + 1] = b[1];
    }
    frames * 2
}

fn play(g: &Game, sound: u64, gain: f32) {
    if g.stream != 0 && sound != 0 {
        let _ = playback::play_sound(g.stream, sound, gain);
    }
}

// ------------------------------------------------------------ formatting

/// Write an integer into `buf` at `at`, returning the new end.
///
/// By hand because `format!` allocates and this runs inside the frame.
fn put_u32(buf: &mut [u8], at: usize, mut v: u32) -> usize {
    let mut digits = [0u8; 10];
    let mut n = 0;
    if v == 0 {
        digits[0] = b'0';
        n = 1;
    }
    while v > 0 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let mut at = at;
    while n > 0 && at < buf.len() {
        n -= 1;
        buf[at] = digits[n];
        at += 1;
    }
    at
}

fn put_str(buf: &mut [u8], at: usize, s: &str) -> usize {
    let mut at = at;
    for &b in s.as_bytes() {
        if at >= buf.len() {
            break;
        }
        buf[at] = b;
        at += 1;
    }
    at
}

/// Seconds as `M:SS.d`, which is how a speedrun clock reads.
fn put_time(buf: &mut [u8], at: usize, secs: f32) -> usize {
    let total = if secs < 0.0 { 0.0 } else { secs };
    let m = (total / 60.0) as u32;
    let s = (total as u32) % 60;
    let d = ((total * 10.0) as u32) % 10;
    let mut at = put_u32(buf, at, m);
    at = put_str(buf, at, ":");
    if s < 10 {
        at = put_str(buf, at, "0");
    }
    at = put_u32(buf, at, s);
    at = put_str(buf, at, ".");
    put_u32(buf, at, d)
}

fn text<'a>(buf: &'a [u8], end: usize) -> &'a str {
    core::str::from_utf8(&buf[..end]).unwrap_or("")
}

// ---------------------------------------------------------------- colour

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> gfx::Rect {
    gfx::Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

// ----------------------------------------------------------------- level

impl Game {
    fn load(&mut self, level: usize) {
        self.level = level;
        self.tiles = [[Tile::Empty; COLS]; ROWS];
        self.cell_count = 0;
        self.mover_count = 0;
        self.spark_count = 0;
        self.collected = 0;
        self.total_cells = 0;

        let rows = &LEVELS[level];
        for r in 0..ROWS {
            let bytes = rows[r].as_bytes();
            for c in 0..COLS {
                let ch = if c < bytes.len() { bytes[c] } else { b' ' };
                let x = c as f32 * TILE;
                let y = r as f32 * TILE;
                match ch {
                    b'#' => self.tiles[r][c] = Tile::Solid,
                    b'=' => self.tiles[r][c] = Tile::Platform,
                    b'^' => self.tiles[r][c] = Tile::Spike,
                    b'E' => self.tiles[r][c] = Tile::Exit,
                    b'o' => {
                        if self.cell_count < MAX_CELLS {
                            self.cells[self.cell_count] = Cell {
                                x: x + TILE * 0.5,
                                y: y + TILE * 0.5,
                                taken: false,
                                phase: (r * COLS + c) as f32 * 0.7,
                            };
                            self.cell_count += 1;
                            self.total_cells += 1;
                        }
                    }
                    b'@' => {
                        self.spawn_x = x + 2.0;
                        self.spawn_y = y;
                    }
                    b'~' => {
                        // Consecutive `~` on one row are ONE platform: find the
                        // run, record it once, and skip the rest by leaving the
                        // tiles empty.
                        let left_is_mine = c > 0 && bytes.get(c - 1) == Some(&b'~');
                        if !left_is_mine && self.mover_count < MAX_MOVERS {
                            let mut w = 1;
                            while c + w < COLS && bytes.get(c + w) == Some(&b'~') {
                                w += 1;
                            }
                            self.movers[self.mover_count] = Mover {
                                x,
                                y,
                                w: w as f32 * TILE,
                                home: x,
                                span: TILE * 3.0,
                                phase: self.mover_count as f32 * 1.3,
                                dx: 0.0,
                            };
                            self.mover_count += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        self.respawn();
        self.time = 0.0;
    }

    fn respawn(&mut self) {
        self.px = self.spawn_x;
        self.py = self.spawn_y;
        self.vx = 0.0;
        self.vy = 0.0;
        self.on_ground = false;
        self.coyote = 0.0;
        self.jump_buffer = 0.0;
        self.squash = 0.0;
    }

    fn tile_at(&self, x: f32, y: f32) -> Tile {
        if x < 0.0 || y < 0.0 {
            return Tile::Solid;
        }
        let c = (x / TILE) as usize;
        let r = (y / TILE) as usize;
        if c >= COLS || r >= ROWS {
            return Tile::Solid;
        }
        self.tiles[r][c]
    }

    /// Is this point inside something that stops a falling body?
    ///
    /// A one-way platform only stops a body moving DOWN, and only when the
    /// body's feet were above it -- otherwise you cannot jump up through it,
    /// which is the whole point of a one-way platform.
    fn blocks(&self, x: f32, y: f32, falling: bool, feet_was: f32) -> bool {
        match self.tile_at(x, y) {
            Tile::Solid => true,
            Tile::Platform => {
                let top = floor(y / TILE) * TILE;
                falling && feet_was <= top + 1.0
            }
            _ => false,
        }
    }

    fn spark_burst(&mut self, x: f32, y: f32, n: usize, warm: bool) {
        for i in 0..n {
            if self.spark_count >= MAX_SPARKS {
                return;
            }
            // Deterministic scatter: no rng needed, and it looks the same on
            // every machine, which matters when a screenshot is the evidence.
            let a = (i as f32) * 2.39996;
            let speed = 40.0 + ((i * 37) % 60) as f32;
            self.sparks[self.spark_count] = Spark {
                x,
                y,
                vx: cos_approx(a) * speed,
                vy: sin_approx(a) * speed - 30.0,
                life: 0.45,
                warm,
            };
            self.spark_count += 1;
        }
    }
}

/// Cheap sine, good to about 0.001 over the range used here.
///
/// `libm` is not linked in a no_std guest and `f32::sin` is not available, so
/// the game brings its own. Bhaskara's approximation, folded to [0, 2pi).
fn sin_approx(x: f32) -> f32 {
    const PI: f32 = 3.141_592_7;
    const TWO_PI: f32 = 6.283_185_3;
    let mut a = x % TWO_PI;
    if a < 0.0 {
        a += TWO_PI;
    }
    let (a, sign) = if a > PI { (a - PI, -1.0) } else { (a, 1.0) };
    let num = 16.0 * a * (PI - a);
    let den = 5.0 * PI * PI - 4.0 * a * (PI - a);
    sign * num / den
}

fn cos_approx(x: f32) -> f32 {
    sin_approx(x + 1.570_796_4)
}

/// `f32::floor` lives in std, which a no_std guest does not link, so the game
/// brings its own. Truncation toward zero differs from floor for negatives;
/// every call here is on a non-negative coordinate, and the branch keeps it
/// correct anyway rather than relying on that.
fn floor(v: f32) -> f32 {
    let t = v as i32 as f32;
    if v < 0.0 && t != v {
        t - 1.0
    } else {
        t
    }
}

fn abs(v: f32) -> f32 {
    if v < 0.0 {
        -v
    } else {
        v
    }
}

fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

// ------------------------------------------------------------ simulation

const GRAVITY: f32 = 1400.0;
const RUN_ACCEL: f32 = 2200.0;
const RUN_MAX: f32 = 190.0;
const FRICTION: f32 = 1800.0;
const JUMP_V: f32 = -430.0;
const COYOTE_TIME: f32 = 0.10;
const JUMP_BUFFER: f32 = 0.12;
const BODY_W: f32 = 12.0;
const BODY_H: f32 = 16.0;

fn step(g: &mut Game, dt: f32, left: bool, right: bool, jump: bool) {
    g.time += dt;

    // Movers first, so a rider sees this frame's platform position.
    for i in 0..g.mover_count {
        let m = &mut g.movers[i];
        m.phase += dt * 0.9;
        let nx = m.home + sin_approx(m.phase) * m.span;
        m.dx = nx - m.x;
        m.x = nx;
    }

    // Horizontal: accelerate toward the held direction, or brake.
    let want = (right as i32 - left as i32) as f32;
    if want != 0.0 {
        g.vx += want * RUN_ACCEL * dt;
        g.vx = clamp(g.vx, -RUN_MAX, RUN_MAX);
        g.face_right = want > 0.0;
    } else if g.vx > 0.0 {
        g.vx = (g.vx - FRICTION * dt).max(0.0);
    } else {
        g.vx = (g.vx + FRICTION * dt).min(0.0);
    }

    // Jump, with coyote time and a buffer -- the two things that make a
    // platformer feel fair rather than strict.
    g.jump_buffer -= dt;
    if jump {
        g.jump_buffer = JUMP_BUFFER;
    }
    g.coyote -= dt;
    if g.jump_buffer > 0.0 && g.coyote > 0.0 {
        g.vy = JUMP_V;
        g.jump_buffer = 0.0;
        g.coyote = 0.0;
        g.on_ground = false;
        g.squash = -0.35;
        play(g, g.snd_jump, 0.5);
    }

    // Variable jump height: releasing early cuts the rise.
    if !jump && g.vy < 0.0 {
        g.vy += GRAVITY * 1.7 * dt;
    }
    g.vy += GRAVITY * dt;
    g.vy = clamp(g.vy, -900.0, 900.0);

    // Move X, then resolve. Axis-at-a-time is what stops corner snagging.
    let feet_before = g.py + BODY_H;
    g.px += (g.vx + g.movers_ride(dt)) * dt;
    if g.vx != 0.0 {
        let probe = if g.vx > 0.0 { g.px + BODY_W } else { g.px };
        for &oy in &[2.0, BODY_H * 0.5, BODY_H - 2.0] {
            if g.tile_at(probe, g.py + oy) == Tile::Solid {
                if g.vx > 0.0 {
                    g.px = floor(probe / TILE) * TILE - BODY_W - 0.01;
                } else {
                    g.px = (floor(probe / TILE) + 1.0) * TILE + 0.01;
                }
                g.vx = 0.0;
                break;
            }
        }
    }

    // Move Y, then resolve.
    g.py += g.vy * dt;
    let was_air = !g.on_ground;
    g.on_ground = false;
    if g.vy >= 0.0 {
        let feet = g.py + BODY_H;
        for &ox in &[2.0, BODY_W * 0.5, BODY_W - 2.0] {
            if g.blocks(g.px + ox, feet, true, feet_before) {
                g.py = floor(feet / TILE) * TILE - BODY_H;
                g.vy = 0.0;
                g.on_ground = true;
                g.coyote = COYOTE_TIME;
                if was_air {
                    g.squash = 0.4;
                }
                break;
            }
        }
    } else {
        let head = g.py;
        for &ox in &[2.0, BODY_W * 0.5, BODY_W - 2.0] {
            if g.tile_at(g.px + ox, head) == Tile::Solid {
                g.py = (floor(head / TILE) + 1.0) * TILE + 0.01;
                g.vy = 0.0;
                break;
            }
        }
    }

    // Riding a moving platform: stand on one and you travel with it.
    if g.on_ground {
        let feet = g.py + BODY_H + 1.0;
        for i in 0..g.mover_count {
            let m = &g.movers[i];
            if feet >= m.y && feet <= m.y + TILE * 0.6 && g.px + BODY_W > m.x && g.px < m.x + m.w {
                g.px += m.dx;
                g.py = m.y - BODY_H;
                break;
            }
        }
    } else {
        // Landing on a mover from above.
        let feet = g.py + BODY_H;
        if g.vy >= 0.0 {
            for i in 0..g.mover_count {
                let m = &g.movers[i];
                if feet >= m.y
                    && feet <= m.y + 10.0
                    && feet_before <= m.y + 1.0
                    && g.px + BODY_W > m.x
                    && g.px < m.x + m.w
                {
                    g.py = m.y - BODY_H;
                    g.vy = 0.0;
                    g.on_ground = true;
                    g.coyote = COYOTE_TIME;
                    if was_air {
                        g.squash = 0.4;
                    }
                    break;
                }
            }
        }
    }

    g.px = clamp(g.px, TILE, WIDTH - TILE - BODY_W);

    // Animation and squash relax toward rest.
    if g.on_ground && abs(g.vx) > 10.0 {
        g.anim += dt * abs(g.vx) * 0.06;
    } else if g.on_ground {
        g.anim = 0.0;
    }
    g.squash *= 1.0 - clamp(dt * 9.0, 0.0, 1.0);

    // Cells.
    for i in 0..g.cell_count {
        if g.cells[i].taken {
            continue;
        }
        let dx = (g.px + BODY_W * 0.5) - g.cells[i].x;
        let dy = (g.py + BODY_H * 0.5) - g.cells[i].y;
        if dx * dx + dy * dy < 15.0 * 15.0 {
            g.cells[i].taken = true;
            g.collected += 1;
            let (cx, cy) = (g.cells[i].x, g.cells[i].y);
            g.spark_burst(cx, cy, 8, false);
            play(g, g.snd_pick, 0.6);
        }
    }

    // Spikes and the exit, sampled at the body's middle and feet.
    let probes = [
        (g.px + BODY_W * 0.5, g.py + BODY_H - 2.0),
        (g.px + 2.0, g.py + BODY_H * 0.5),
        (g.px + BODY_W - 2.0, g.py + BODY_H * 0.5),
    ];
    for (x, y) in probes {
        match g.tile_at(x, y) {
            Tile::Spike => {
                die(g);
                return;
            }
            Tile::Exit => {
                g.screen = Screen::LevelDone;
                play(g, g.snd_done, 0.7);
                let (bx, by) = (g.px, g.py);
                g.spark_burst(bx + BODY_W * 0.5, by + BODY_H * 0.5, 24, false);
                return;
            }
            _ => {}
        }
    }

    // Falling out of the world.
    if g.py > HEIGHT + 40.0 {
        die(g);
        return;
    }

    // Sparks.
    let mut i = 0;
    while i < g.spark_count {
        let s = &mut g.sparks[i];
        s.life -= dt;
        if s.life <= 0.0 {
            let last = g.spark_count - 1;
            g.sparks.swap(i, last);
            g.spark_count -= 1;
            continue;
        }
        s.vy += GRAVITY * 0.5 * dt;
        s.x += s.vx * dt;
        s.y += s.vy * dt;
        i += 1;
    }

    g.shake *= 1.0 - clamp(dt * 6.0, 0.0, 1.0);
}

fn die(g: &mut Game) {
    g.screen = Screen::Dead;
    g.deaths += 1;
    g.shake = 8.0;
    let (x, y) = (g.px + BODY_W * 0.5, g.py + BODY_H * 0.5);
    g.spark_burst(x, y, 20, true);
    play(g, g.snd_die, 0.7);
}

impl Game {
    /// Movers push a rider horizontally; that is folded into the X step.
    fn movers_ride(&self, _dt: f32) -> f32 {
        0.0
    }
}

// ------------------------------------------------------------------ draw

fn draw(g: &Game, canvas: u64) -> Result<(), gfx::GfxError> {
    let sx = if g.shake > 0.1 {
        sin_approx(g.time * 60.0) * g.shake
    } else {
        0.0
    };
    let sy = if g.shake > 0.1 {
        cos_approx(g.time * 71.0) * g.shake
    } else {
        0.0
    };

    canvas2d::clear(canvas, rgb(0.05, 0.06, 0.10))?;

    // A soft vertical wash so the level does not sit on flat black.
    for i in 0..8 {
        let t = i as f32 / 8.0;
        canvas2d::fill_rect(
            canvas,
            rect(0.0, t * HEIGHT, WIDTH, HEIGHT / 8.0),
            rgba(0.10, 0.12, 0.22, 0.25 * (1.0 - t)),
        )?;
    }

    // Tiles.
    for r in 0..ROWS {
        for c in 0..COLS {
            let x = c as f32 * TILE + sx;
            let y = r as f32 * TILE + sy;
            match g.tiles[r][c] {
                Tile::Solid => {
                    canvas2d::fill_rect(canvas, rect(x, y, TILE, TILE), rgb(0.16, 0.19, 0.30))?;
                    canvas2d::fill_rect(canvas, rect(x, y, TILE, 3.0), rgb(0.24, 0.29, 0.44))?;
                }
                Tile::Platform => {
                    canvas2d::fill_rect(canvas, rect(x, y, TILE, 5.0), rgb(0.30, 0.45, 0.55))?;
                }
                Tile::Spike => {
                    // Three teeth, drawn as narrowing bars: cheap, and reads
                    // as danger at this size.
                    for k in 0..3 {
                        let tx = x + k as f32 * (TILE / 3.0);
                        canvas2d::fill_rect(
                            canvas,
                            rect(tx + 1.0, y + TILE * 0.45, TILE / 3.0 - 2.0, TILE * 0.55),
                            rgb(0.85, 0.25, 0.35),
                        )?;
                        canvas2d::fill_rect(
                            canvas,
                            rect(tx + 2.0, y + TILE * 0.2, TILE / 3.0 - 4.0, TILE * 0.3),
                            rgb(0.95, 0.40, 0.45),
                        )?;
                    }
                }
                Tile::Exit => {
                    let pulse = 0.5 + 0.5 * sin_approx(g.time * 3.0);
                    canvas2d::fill_rect(
                        canvas,
                        rect(x, y - TILE, TILE, TILE * 2.0),
                        rgba(0.30, 0.95, 0.55, 0.25 + 0.25 * pulse),
                    )?;
                    canvas2d::fill_rect(
                        canvas,
                        rect(x + 4.0, y - TILE + 4.0, TILE - 8.0, TILE * 2.0 - 8.0),
                        rgb(0.35, 1.0, 0.60),
                    )?;
                }
                Tile::Empty => {}
            }
        }
    }

    // Movers.
    for i in 0..g.mover_count {
        let m = &g.movers[i];
        canvas2d::fill_rect(
            canvas,
            rect(m.x + sx, m.y + sy, m.w, 6.0),
            rgb(0.45, 0.60, 0.80),
        )?;
        canvas2d::fill_rect(
            canvas,
            rect(m.x + sx, m.y + sy + 6.0, m.w, 3.0),
            rgba(0.20, 0.30, 0.45, 0.8),
        )?;
    }

    // Cells: a bobbing diamond, drawn as two stacked bars.
    for i in 0..g.cell_count {
        let c = &g.cells[i];
        if c.taken {
            continue;
        }
        let bob = sin_approx(g.time * 2.5 + c.phase) * 2.5;
        let x = c.x + sx;
        let y = c.y + sy + bob;
        canvas2d::fill_rect(canvas, rect(x - 5.0, y - 2.0, 10.0, 4.0), rgb(1.0, 0.85, 0.35))?;
        canvas2d::fill_rect(canvas, rect(x - 2.0, y - 5.0, 4.0, 10.0), rgb(1.0, 0.92, 0.55))?;
    }

    // Sparks.
    for i in 0..g.spark_count {
        let s = &g.sparks[i];
        let a = clamp(s.life / 0.45, 0.0, 1.0);
        let col = if s.warm {
            rgba(1.0, 0.45, 0.30, a)
        } else {
            rgba(1.0, 0.88, 0.45, a)
        };
        let sz = 2.0 + 2.0 * a;
        canvas2d::fill_rect(canvas, rect(s.x + sx, s.y + sy, sz, sz), col)?;
    }

    // The runner: a body, a visor, and legs that swap while running.
    if g.screen != Screen::Dead {
        let squash = g.squash;
        let w = BODY_W * (1.0 + squash * 0.5);
        let h = BODY_H * (1.0 - squash * 0.5);
        let x = g.px + sx - (w - BODY_W) * 0.5;
        let y = g.py + sy + (BODY_H - h);

        canvas2d::fill_rect(canvas, rect(x, y, w, h), rgb(0.35, 0.90, 0.95))?;
        // Visor, on the side being faced.
        let vx = if g.face_right { x + w - 5.0 } else { x + 1.0 };
        canvas2d::fill_rect(canvas, rect(vx, y + 3.0, 4.0, 4.0), rgb(0.08, 0.15, 0.22))?;
        // Legs.
        if g.on_ground && abs(g.vx) > 10.0 {
            let swing = sin_approx(g.anim);
            canvas2d::fill_rect(
                canvas,
                rect(x + 1.0 + swing * 2.0, y + h, 4.0, 3.0),
                rgb(0.20, 0.65, 0.75),
            )?;
            canvas2d::fill_rect(
                canvas,
                rect(x + w - 5.0 - swing * 2.0, y + h, 4.0, 3.0),
                rgb(0.20, 0.65, 0.75),
            )?;
        } else if !g.on_ground {
            canvas2d::fill_rect(canvas, rect(x + 2.0, y + h, 8.0, 2.0), rgb(0.20, 0.65, 0.75))?;
        }
    }

    // HUD.
    canvas2d::fill_rect(canvas, rect(0.0, 0.0, WIDTH, 22.0), rgba(0.03, 0.04, 0.08, 0.85))?;
    let mut buf = [0u8; 64];
    let mut n = put_str(&mut buf, 0, "LEVEL ");
    n = put_u32(&mut buf, n, (g.level + 1) as u32);
    n = put_str(&mut buf, n, "/");
    n = put_u32(&mut buf, n, LEVEL_COUNT as u32);
    canvas2d::draw_text(
        canvas,
        text(&buf, n),
        gfx::Point { x: 10.0, y: 15.0 },
        11.0,
        rgb(0.75, 0.82, 0.95),
    )?;

    let mut buf2 = [0u8; 64];
    let mut n2 = put_str(&mut buf2, 0, "CELLS ");
    n2 = put_u32(&mut buf2, n2, g.collected);
    n2 = put_str(&mut buf2, n2, "/");
    n2 = put_u32(&mut buf2, n2, g.total_cells);
    canvas2d::draw_text(
        canvas,
        text(&buf2, n2),
        gfx::Point { x: 120.0, y: 15.0 },
        11.0,
        rgb(1.0, 0.88, 0.45),
    )?;

    let mut buf3 = [0u8; 64];
    let mut n3 = put_str(&mut buf3, 0, "TIME ");
    n3 = put_time(&mut buf3, n3, g.time);
    canvas2d::draw_text(
        canvas,
        text(&buf3, n3),
        gfx::Point { x: 250.0, y: 15.0 },
        11.0,
        rgb(0.75, 0.82, 0.95),
    )?;

    let mut buf4 = [0u8; 64];
    let mut n4 = put_str(&mut buf4, 0, "DEATHS ");
    n4 = put_u32(&mut buf4, n4, g.deaths);
    canvas2d::draw_text(
        canvas,
        text(&buf4, n4),
        gfx::Point { x: 370.0, y: 15.0 },
        11.0,
        rgb(0.85, 0.45, 0.50),
    )?;

    if g.best > 0.0 {
        let mut buf5 = [0u8; 64];
        let mut n5 = put_str(&mut buf5, 0, "BEST ");
        n5 = put_time(&mut buf5, n5, g.best);
        canvas2d::draw_text(
            canvas,
            text(&buf5, n5),
            gfx::Point { x: 490.0, y: 15.0 },
            11.0,
            rgb(0.45, 0.90, 0.60),
        )?;
    }

    // Overlays.
    match g.screen {
        Screen::Title => {
            overlay(canvas, 0.82)?;
            centred(canvas, "RELAY", 210.0, 46.0, rgb(0.35, 0.90, 0.95))?;
            centred(
                canvas,
                "arrows or A/D to run, space to jump",
                246.0,
                13.0,
                rgb(0.70, 0.78, 0.92),
            )?;
            centred(
                canvas,
                "collect the cells, reach the green gate",
                266.0,
                13.0,
                rgb(0.55, 0.62, 0.78),
            )?;
            if g.furthest > 0 {
                let mut b = [0u8; 48];
                let mut k = put_str(&mut b, 0, "press SPACE to continue level ");
                k = put_u32(&mut b, k, (g.furthest + 1) as u32);
                centred(canvas, text(&b, k), 300.0, 13.0, rgb(1.0, 0.88, 0.45))?;
            } else {
                centred(canvas, "press SPACE to start", 300.0, 13.0, rgb(1.0, 0.88, 0.45))?;
            }
        }
        Screen::Paused => {
            overlay(canvas, 0.65)?;
            centred(canvas, "PAUSED", 200.0, 34.0, rgb(0.80, 0.86, 0.98))?;
            centred(canvas, "P to resume, R to restart", 236.0, 13.0, rgb(0.60, 0.68, 0.84))?;
        }
        Screen::Dead => {
            overlay(canvas, 0.55)?;
            centred(canvas, "OUCH", 200.0, 34.0, rgb(0.95, 0.40, 0.45))?;
            centred(canvas, "press SPACE to try again", 236.0, 13.0, rgb(0.80, 0.70, 0.75))?;
        }
        Screen::LevelDone => {
            overlay(canvas, 0.70)?;
            centred(canvas, "LEVEL CLEAR", 190.0, 32.0, rgb(0.45, 0.95, 0.60))?;
            let mut b = [0u8; 64];
            let mut k = put_str(&mut b, 0, "time ");
            k = put_time(&mut b, k, g.time);
            k = put_str(&mut b, k, "   cells ");
            k = put_u32(&mut b, k, g.collected);
            k = put_str(&mut b, k, "/");
            k = put_u32(&mut b, k, g.total_cells);
            centred(canvas, text(&b, k), 224.0, 14.0, rgb(0.80, 0.88, 0.98))?;
            centred(canvas, "press SPACE for the next one", 254.0, 13.0, rgb(1.0, 0.88, 0.45))?;
        }
        Screen::Won => {
            overlay(canvas, 0.82)?;
            centred(canvas, "ALL CLEAR", 180.0, 42.0, rgb(1.0, 0.88, 0.45))?;
            let mut b = [0u8; 64];
            let mut k = put_str(&mut b, 0, "total ");
            k = put_time(&mut b, k, g.time);
            k = put_str(&mut b, k, "   deaths ");
            k = put_u32(&mut b, k, g.deaths);
            centred(canvas, text(&b, k), 220.0, 15.0, rgb(0.85, 0.92, 1.0))?;
            centred(canvas, "press SPACE to run it again", 254.0, 13.0, rgb(0.70, 0.78, 0.92))?;
        }
        Screen::Playing => {}
    }

    canvas2d::present(canvas)
}

fn overlay(canvas: u64, a: f32) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, rect(0.0, 0.0, WIDTH, HEIGHT), rgba(0.03, 0.04, 0.09, a))
}

/// Centre a line horizontally by asking the host how wide it will be.
///
/// Character-count times a constant is wrong on a proportional face -- `i` and
/// `W` differ by about four times -- so the text would sit visibly off-centre.
fn centred(canvas: u64, s: &str, y: f32, size: f32, ink: gfx::Color) -> Result<(), gfx::GfxError> {
    let m = canvas2d::measure_text(canvas, s, size)?;
    canvas2d::draw_text(
        canvas,
        s,
        gfx::Point {
            x: (WIDTH - m.width) * 0.5,
            y,
        },
        size,
        ink,
    )
}

// ------------------------------------------------------------------ save

const SAVE_KEY: &str = "relay.progress";

/// Save is `furthest,best_tenths,deaths` -- three integers, comma separated.
///
/// Plain text on purpose: it survives a format change, it is readable when
/// something goes wrong, and parsing it needs no allocation.
fn save(g: &Game) {
    let mut b = [0u8; 48];
    let mut n = put_u32(&mut b, 0, g.furthest as u32);
    n = put_str(&mut b, n, ",");
    n = put_u32(&mut b, n, (g.best * 10.0) as u32);
    n = put_str(&mut b, n, ",");
    n = put_u32(&mut b, n, g.deaths);
    let _ = krate::store::set(SAVE_KEY, &b[..n]);
}

fn load_save(g: &mut Game) {
    let Ok(Some(bytes)) = krate::store::get(SAVE_KEY) else {
        return;
    };
    let mut parts = [0u32; 3];
    let mut idx = 0;
    let mut acc: u32 = 0;
    let mut any = false;
    for &c in bytes.iter() {
        if c == b',' {
            if idx < 3 {
                parts[idx] = acc;
            }
            idx += 1;
            acc = 0;
            any = false;
        } else if c.is_ascii_digit() {
            acc = acc.saturating_mul(10).saturating_add((c - b'0') as u32);
            any = true;
        }
    }
    if any && idx < 3 {
        parts[idx] = acc;
    }
    g.furthest = (parts[0] as usize).min(LEVEL_COUNT - 1);
    g.best = parts[1] as f32 / 10.0;
    g.deaths = parts[2];
}

/// Say which step failed before exiting.
///
/// A bare `return 1` tells a checker only that something went wrong; the log
/// then ends with no clue which call it was. Naming the step is the difference
/// between a two-minute diagnosis and an afternoon.
fn fail(step: &[u8]) -> i32 {
    let out = stdio::stdout();
    let _ = out.write(b"relay: failed at ");
    let _ = out.write(step);
    let _ = out.write(b"\n");
    1
}

/// A widget node with the fields this game does not use left at rest.
///
/// Written out once because `WidgetNode` has eight fields and a game that
/// repeats them per node is mostly punctuation.
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

// ------------------------------------------------------------------ main

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        // `raw()` is the whole argv as one newline-separated string: the
        // no_std guest never builds a Vec<String> for it.
        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|b| *b == b'\n')
            .any(|a| a == b"quick" || a == b"--quick");

        let win = match window::create(
            "Relay",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) {
            Ok(w) => w,
            Err(_) => return fail(b"window::create"),
        };

        // Order matters, and it is not obvious: `show` first, then the ROOT
        // through `set_root` -- not `upsert_node`, which has no parent to
        // attach to and fails. Learned by naming the failure: the log said
        // "failed at upsert root" and nothing else would have.
        if window::show(win).is_err() {
            return fail(b"window::show");
        }
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            return fail(b"set_root");
        }
        if tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        )
        .is_err()
        {
            return fail(b"upsert canvas");
        }

        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => return fail(b"canvas2d::bind"),
        };
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        let mut g = Game {
            screen: Screen::Title,
            level: 0,
            tiles: [[Tile::Empty; COLS]; ROWS],
            px: 0.0,
            py: 0.0,
            vx: 0.0,
            vy: 0.0,
            on_ground: false,
            face_right: true,
            coyote: 0.0,
            jump_buffer: 0.0,
            anim: 0.0,
            squash: 0.0,
            cells: core::array::from_fn(|_| Cell {
                x: 0.0,
                y: 0.0,
                taken: true,
                phase: 0.0,
            }),
            cell_count: 0,
            collected: 0,
            total_cells: 0,
            movers: core::array::from_fn(|_| Mover {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                home: 0.0,
                span: 0.0,
                phase: 0.0,
                dx: 0.0,
            }),
            mover_count: 0,
            sparks: core::array::from_fn(|_| Spark {
                x: 0.0,
                y: 0.0,
                vx: 0.0,
                vy: 0.0,
                life: 0.0,
                warm: false,
            }),
            spark_count: 0,
            spawn_x: 40.0,
            spawn_y: 40.0,
            time: 0.0,
            best: 0.0,
            furthest: 0,
            deaths: 0,
            shake: 0.0,
            stream: 0,
            snd_jump: 0,
            snd_pick: 0,
            snd_die: 0,
            snd_done: 0,
        };

        load_save(&mut g);

        // Sound. A machine with no device is a normal outcome: every handle
        // stays zero and `play` becomes a no-op, so the game is silent rather
        // than broken.
        if let Ok(stream) = playback::open(StreamConfig {
            sample_rate: SAMPLE_RATE,
            channels: 1,
            format: SampleFormat::PcmS16,
            buffer_frames: 1024,
        }) {
            if playback::start(stream).is_ok() {
                g.stream = stream;
                let mut pcm = [0u8; SAMPLE_RATE as usize]; // ~0.5 s of headroom
                let n = synth(&mut pcm, 90, 380.0, 700.0, 0);
                g.snd_jump = playback::load_sound(stream, &pcm[..n]).unwrap_or(0);
                let n = synth(&mut pcm, 120, 900.0, 1500.0, 1);
                g.snd_pick = playback::load_sound(stream, &pcm[..n]).unwrap_or(0);
                let n = synth(&mut pcm, 260, 300.0, 70.0, 2);
                g.snd_die = playback::load_sound(stream, &pcm[..n]).unwrap_or(0);
                let n = synth(&mut pcm, 320, 500.0, 1100.0, 1);
                g.snd_done = playback::load_sound(stream, &pcm[..n]).unwrap_or(0);
            }
        }

        g.load(g.furthest);
        g.time = 0.0;

        // The host captures the FIRST presented frame, so the automated run
        // gets the title screen and then exits.
        let _ = draw(&g, canvas);
        if quick {
            let mut b = [0u8; 96];
            let mut n = put_str(&mut b, 0, "relay: ");
            n = put_u32(&mut b, n, LEVEL_COUNT as u32);
            n = put_str(&mut b, n, " levels, sound=");
            n = put_u32(&mut b, n, if g.stream != 0 { 1 } else { 0 });
            n = put_str(&mut b, n, "\n");
            let out = stdio::stdout();
            let _ = out.write(text(&b, n).as_bytes());
            return 0;
        }

        // Frame-time histogram, in 1 ms buckets up to 32 ms then an overflow
        // bucket. An average frame time hides exactly the thing that matters --
        // one 90 ms hitch in an otherwise perfect second reads as 17 ms.
        let mut buckets = [0u32; 34];
        let mut frames: u32 = 0;
        let mut worst_ms: f32 = 0.0;

        let mut last = clock::monotonic_nanos();
        let mut prev_jump = false;
        let mut prev_pause = false;
        let mut prev_restart = false;

        loop {
            let now = clock::monotonic_nanos();
            let mut dt = (now.saturating_sub(last)) as f32 / 1_000_000_000.0;
            last = now;
            // A frame that took longer than 50 ms is a stall -- a window drag,
            // a breakpoint. Simulating it in one step would teleport the runner
            // through a wall, so clamp instead.
            dt = clamp(dt, 0.0, 0.05);

            let left = events::key_held("ArrowLeft") || events::key_held("a");
            let right = events::key_held("ArrowRight") || events::key_held("d");
            let jump_now = events::key_held("Space") || events::key_held(" ")
                || events::key_held("ArrowUp") || events::key_held("w");
            let pause_now = events::key_held("p");
            let restart_now = events::key_held("r");

            let jump_pressed = jump_now && !prev_jump;
            let pause_pressed = pause_now && !prev_pause;
            let restart_pressed = restart_now && !prev_restart;
            prev_jump = jump_now;
            prev_pause = pause_now;
            prev_restart = restart_now;

            match g.screen {
                Screen::Title => {
                    if jump_pressed {
                        g.load(g.furthest);
                        g.time = 0.0;
                        g.screen = Screen::Playing;
                    }
                }
                Screen::Playing => {
                    if pause_pressed {
                        g.screen = Screen::Paused;
                    } else if restart_pressed {
                        g.respawn();
                    } else {
                        step(&mut g, dt, left, right, jump_pressed);
                    }
                }
                Screen::Paused => {
                    if pause_pressed {
                        g.screen = Screen::Playing;
                    } else if restart_pressed {
                        g.load(g.level);
                        g.screen = Screen::Playing;
                    }
                }
                Screen::Dead => {
                    // Sparks keep moving on the death screen; it reads as an
                    // aftermath rather than a freeze.
                    step_sparks(&mut g, dt);
                    if jump_pressed {
                        g.respawn();
                        g.screen = Screen::Playing;
                    }
                }
                Screen::LevelDone => {
                    step_sparks(&mut g, dt);
                    if jump_pressed {
                        if g.level + 1 >= LEVEL_COUNT {
                            if g.best == 0.0 || g.time < g.best {
                                g.best = g.time;
                            }
                            g.furthest = 0;
                            save(&g);
                            g.screen = Screen::Won;
                        } else {
                            g.furthest = (g.level + 1).max(g.furthest);
                            save(&g);
                            let carried = g.time;
                            g.load(g.level + 1);
                            g.time = carried;
                            g.screen = Screen::Playing;
                        }
                    }
                }
                Screen::Won => {
                    step_sparks(&mut g, dt);
                    if jump_pressed {
                        g.deaths = 0;
                        g.load(0);
                        g.time = 0.0;
                        g.screen = Screen::Playing;
                    }
                }
            }

            if draw(&g, canvas).is_err() {
                break;
            }

            let ms = dt * 1000.0;
            if ms > worst_ms {
                worst_ms = ms;
            }
            let b = if ms >= 33.0 { 33 } else { ms as usize };
            buckets[b] = buckets[b].saturating_add(1);
            frames = frames.saturating_add(1);

            if let Some(types::Event::CloseRequested(id)) = events::poll() {
                let _ = window::close(id);
                break;
            }
        }

        save(&g);

        // The frame-time report: median, 95th, 99th and the worst single
        // frame. This is the CP-A "frame time stable, not merely fast on
        // average" measurement, taken by the app itself so it costs no
        // external profiler.
        if frames > 0 {
            let out = stdio::stdout();
            let mut b = [0u8; 160];
            let mut n = put_str(&mut b, 0, "relay-frames: n=");
            n = put_u32(&mut b, n, frames);
            for (label, pct) in [(" p50=", 50u32), (" p95=", 95), (" p99=", 99)] {
                let target = (frames as u64 * pct as u64 / 100) as u32;
                let mut seen = 0u32;
                let mut ms = 33u32;
                for (i, c) in buckets.iter().enumerate() {
                    seen = seen.saturating_add(*c);
                    if seen >= target {
                        ms = i as u32;
                        break;
                    }
                }
                n = put_str(&mut b, n, label);
                n = put_u32(&mut b, n, ms);
                n = put_str(&mut b, n, "ms");
            }
            n = put_str(&mut b, n, " worst=");
            n = put_u32(&mut b, n, worst_ms as u32);
            n = put_str(&mut b, n, "ms\n");
            let _ = out.write(text(&b, n).as_bytes());
        }
        0
    }
}

/// Sparks and shake keep running while a screen is up.
fn step_sparks(g: &mut Game, dt: f32) {
    g.time += 0.0; // the clock is paused on overlays; sparks are not
    let mut i = 0;
    while i < g.spark_count {
        let s = &mut g.sparks[i];
        s.life -= dt;
        if s.life <= 0.0 {
            let last = g.spark_count - 1;
            g.sparks.swap(i, last);
            g.spark_count -= 1;
            continue;
        }
        s.vy += GRAVITY * 0.5 * dt;
        s.x += s.vx * dt;
        s.y += s.vy * dt;
        i += 1;
    }
    g.shake *= 1.0 - clamp(dt * 6.0, 0.0, 1.0);
}

krate::export!(Component);
