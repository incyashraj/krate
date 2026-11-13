//! Krate Nova -- a neon top-down arcade space shooter.
//!
//! The hero app: a fast, juicy twin-stick-lite shooter that renders every frame
//! into one canvas. A cyan ship at the bottom moves on arrows/WASD and fires
//! with Space; waves of magenta and orange enemies descend; bullets streak up;
//! dying enemies burst into fading warm particles; the score climbs with little
//! popups; lives sit top-right; big hits kick the whole frame with screen shake;
//! a scrolling multi-layer starfield gives the void depth. All of it draws with
//! gfx.canvas2d rectangle fills and text, driven by clock deltas and held-key
//! polling.
//!
//! This app is `#![no_std]`, which is the discipline that keeps it krate:*-only:
//! the SDK owns the allocator and a trapping panic handler, so no path drags in
//! the wasi:* import set. Entities live in fixed-capacity arrays with active
//! counts -- no growing Vec in the hot loop, no array indexing that can panic,
//! no format!, no unwrap. Numbers are formatted by hand into byte buffers.
//!
//! The best score is kept in `store.kv` (`best-score`), written when a game
//! ends with a new best and read back at start. Nothing else is needed for it
//! to follow the person to every device they sign in on: Krate syncs the
//! store. A best kept only in memory would be gone the moment the window
//! closed.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

// Linked purely for its no_std runtime lang items -- the global allocator, the
// trapping panic handler, and the memory intrinsics a wasm guest needs when std
// is not linked. Not called directly; the underscore keeps the import.

use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, types as gfx};
use krate::ui::{events, pointer, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 480.0;
/// The shortest the world is: a desktop window's 3:4.
const MIN_HEIGHT: f32 = 640.0;
/// The tallest: a 9:20 phone held upright.
const MAX_HEIGHT: f32 = 1100.0;

/// The world's height, in the same units as WIDTH. The width is the design;
/// the height follows the shape of the screen's free space, so a tall phone
/// shows a taller sky instead of a third of it empty (a fixed 480x640 left
/// that band on a Galaxy S22 Ultra). Set each frame by `fit_world`.
static WORLD_H: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0x4420_0000); // 640.0

#[allow(non_snake_case)]
fn HEIGHT() -> f32 {
    f32::from_bits(WORLD_H.load(core::sync::atomic::Ordering::Relaxed))
}

/// The person's text size against the default (`Look::text_scale`), as
/// f32 bits. `fit_world` reads it from the same `Screen::of` it already asks
/// every frame, so following it costs the frame nothing.
static TEXT_K: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0x3F80_0000); // 1.0

/// A font size for this person: `points` at their text size, which is what
/// `krate::layout::Look::text` computes. The HUD reads its sizes through
/// this so a person who asked for larger text gets a larger score.
fn text(points: f32) -> f32 {
    points * f32::from_bits(TEXT_K.load(core::sync::atomic::Ordering::Relaxed))
}

/// How wide `text` draws at `size`, for centring and right-aligning.
fn text_w(canvas: u64, text: &str, size: f32) -> f32 {
    canvas2d::measure_text(canvas, text, size)
        .map(|m| m.width)
        .unwrap_or(text.len() as f32 * size * 0.55)
}

/// Make the world as tall as the screen's free space is, relative to its
/// width -- between a 3:4 window and a 9:20 phone -- and tell the host the
/// design size it now draws in. Returns how far the world grew this frame,
/// so what sits on the bottom (the ship) can move with it.
fn fit_world(win: u64, canvas: u64) -> f32 {
    let screen = krate::layout::Screen::of(win);
    // Larger text, up to where the HUD still fits the 480-wide world.
    let k = screen.look.text(1.0).clamp(0.8, 1.6);
    TEXT_K.store(k.to_bits(), core::sync::atomic::Ordering::Relaxed);
    let (top, right, bottom, left) = screen.safe;
    let (w, h) = (screen.width - left - right, screen.height - top - bottom);
    if w <= 1.0 || h <= 1.0 {
        return 0.0;
    }
    let want = (WIDTH * h / w).clamp(MIN_HEIGHT, MAX_HEIGHT);
    let was = HEIGHT();
    if (want - was).abs() < 0.5 {
        return 0.0;
    }
    WORLD_H.store(want.to_bits(), core::sync::atomic::Ordering::Relaxed);
    let _ = canvas2d::set_design_size(
        canvas,
        gfx::Size {
            width: WIDTH,
            height: want,
        },
    );
    want - was
}

// Fixed-capacity entity pools. An active count marks the live prefix; freeing
// swaps the last live entry down, so the loop is always over a dense range.
const MAX_ENEMIES: usize = 64;
const MAX_BULLETS: usize = 128;
const MAX_EBULLETS: usize = 64;
const MAX_PARTICLES: usize = 512;
const MAX_STARS: usize = 110;
const MAX_POPUPS: usize = 16;

const PLAYER_W: f32 = 30.0;
const PLAYER_H: f32 = 30.0;
const PLAYER_SPEED: f32 = 340.0;
/// How far above a finger the ship steers to, so the finger never hides it.
const FINGER_LEAD: f32 = 70.0;
const BULLET_SPEED: f32 = 620.0;
const FIRE_COOLDOWN: f32 = 0.14;
const EBULLET_SPEED: f32 = 230.0;

const QUICK_FRAMES: u32 = 240;
/// Where the best score is kept, as decimal text so a sync conflict card
/// shows a number a person can read.
const BEST_KEY: &str = "best-score";
const MAX_FRAMES: u32 = 100_000;

struct Component;

// A tiny fast deterministic PRNG (xorshift32). No std, no panic paths.
struct Rng {
    s: u32,
}
impl Rng {
    fn new(seed: u32) -> Self {
        Self {
            s: if seed == 0 { 0x9E3779B9 } else { seed },
        }
    }
    fn next_u32(&mut self) -> u32 {
        let mut x = self.s;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.s = x;
        x
    }
    /// Uniform in [0, 1).
    fn f(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }
    /// Uniform in [lo, hi).
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.f() * (hi - lo)
    }
}

#[derive(Clone, Copy)]
struct Bullet {
    x: f32,
    y: f32,
    vy: f32,
    vx: f32,
}

#[derive(Clone, Copy)]
struct Enemy {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    /// Horizontal weave phase.
    phase: f32,
    /// 0 = magenta grunt, 1 = orange diver (tougher/faster).
    kind: u8,
    hp: i32,
    /// Seconds since spawn, for the spawn pop-in scale.
    age: f32,
    /// Flash timer set on hit, drives a white flash.
    hit: f32,
}

#[derive(Clone, Copy)]
struct Particle {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    life: f32,
    max_life: f32,
    r: f32,
    g: f32,
    b: f32,
    size: f32,
}

#[derive(Clone, Copy)]
struct Star {
    x: f32,
    y: f32,
    speed: f32,
    bright: f32,
    size: f32,
}

#[derive(Clone, Copy)]
struct Popup {
    x: f32,
    y: f32,
    life: f32,
    value: u32,
}

struct World {
    rng: Rng,
    px: f32,
    py: f32,
    lives: i32,
    score: u32,
    /// The best score on any of this person's devices, from `store.kv`.
    best: u32,
    fire_timer: f32,
    spawn_timer: f32,
    /// Difficulty ramps with time: spawn interval shrinks, speeds grow.
    elapsed: f32,
    shake: f32,
    /// Player invulnerability after a hit (blink + no damage).
    invuln: f32,
    muzzle: f32,
    game_over: bool,

    enemies: [Enemy; MAX_ENEMIES],
    n_enemies: usize,
    bullets: [Bullet; MAX_BULLETS],
    n_bullets: usize,
    ebullets: [Bullet; MAX_EBULLETS],
    n_ebullets: usize,
    particles: [Particle; MAX_PARTICLES],
    n_particles: usize,
    stars: [Star; MAX_STARS],
    popups: [Popup; MAX_POPUPS],
    n_popups: usize,
}

const ZERO_ENEMY: Enemy = Enemy {
    x: 0.0,
    y: 0.0,
    vx: 0.0,
    vy: 0.0,
    phase: 0.0,
    kind: 0,
    hp: 0,
    age: 0.0,
    hit: 0.0,
};
const ZERO_BULLET: Bullet = Bullet {
    x: 0.0,
    y: 0.0,
    vy: 0.0,
    vx: 0.0,
};
const ZERO_PARTICLE: Particle = Particle {
    x: 0.0,
    y: 0.0,
    vx: 0.0,
    vy: 0.0,
    life: 0.0,
    max_life: 0.0,
    r: 0.0,
    g: 0.0,
    b: 0.0,
    size: 0.0,
};
const ZERO_STAR: Star = Star {
    x: 0.0,
    y: 0.0,
    speed: 0.0,
    bright: 0.0,
    size: 1.0,
};
const ZERO_POPUP: Popup = Popup {
    x: 0.0,
    y: 0.0,
    life: 0.0,
    value: 0,
};

impl World {
    /// The world grew (or shrank) by `by` at the bottom: the ship keeps its
    /// place above the floor, and the sky's stars spread over the new height.
    fn grown(&mut self, by: f32) {
        self.py = clampf(
            self.py + by,
            HEIGHT() * 0.45,
            HEIGHT() - PLAYER_H * 0.5 - 6.0,
        );
        let h = HEIGHT();
        for star in self.stars.iter_mut() {
            star.y = self.rng.range(0.0, h);
        }
    }

    fn new(seed: u32, quick: bool) -> Self {
        let mut rng = Rng::new(seed);
        let mut stars = [ZERO_STAR; MAX_STARS];
        let mut i = 0usize;
        while i < MAX_STARS {
            // Three depth layers: far/dim/slow to near/bright/fast.
            let layer = i % 3;
            let (speed, bright, size) = match layer {
                0 => (rng.range(18.0, 34.0), rng.range(0.18, 0.34), 1.0),
                1 => (rng.range(38.0, 66.0), rng.range(0.34, 0.55), 1.0),
                _ => (rng.range(80.0, 140.0), rng.range(0.55, 0.9), 2.0),
            };
            stars[i] = Star {
                x: rng.range(0.0, WIDTH),
                y: rng.range(0.0, HEIGHT()),
                speed,
                bright,
                size,
            };
            i += 1;
        }
        World {
            rng,
            px: WIDTH / 2.0,
            py: HEIGHT() - 70.0,
            lives: 3,
            // The automated shot depicts a run already in progress, so it opens
            // on a believable mid-game score rather than zero.
            score: if quick { 12_800 } else { 0 },
            best: 0,
            fire_timer: 0.0,
            // In the automated shot, start with a short fuse and a pre-ramped
            // clock so the frame we capture is already dense with enemies,
            // divers, and their bullets -- not an empty opening.
            spawn_timer: if quick { 0.05 } else { 0.6 },
            elapsed: if quick { 6.0 } else { 0.0 },
            shake: 0.0,
            invuln: 0.0,
            muzzle: 0.0,
            game_over: false,
            enemies: [ZERO_ENEMY; MAX_ENEMIES],
            n_enemies: 0,
            bullets: [ZERO_BULLET; MAX_BULLETS],
            n_bullets: 0,
            ebullets: [ZERO_BULLET; MAX_EBULLETS],
            n_ebullets: 0,
            particles: [ZERO_PARTICLE; MAX_PARTICLES],
            n_particles: 0,
            stars,
            popups: [ZERO_POPUP; MAX_POPUPS],
            n_popups: 0,
        }
    }

    /// Force a crowded, cinematic scene for the automated screenshot: a
    /// staggered formation of grunts and divers spread across the field, a
    /// volley of enemy fire raining down, and a couple of live explosions -- so
    /// the captured frame reads as peak action, not a lull between waves.
    fn stage_showtime(&mut self) {
        // A spread of enemies at varied heights and columns.
        let cols: [f32; 6] = [70.0, 150.0, 235.0, 315.0, 395.0, 445.0];
        // All clear of the HUD band up top; spread down through the field.
        let heights: [f32; 6] = [130.0, 230.0, 175.0, 300.0, 200.0, 360.0];
        let mut i = 0usize;
        while i < cols.len() {
            let x = cols.get(i).copied().unwrap_or(WIDTH * 0.5);
            let y = heights.get(i).copied().unwrap_or(160.0);
            let kind: u8 = if i % 3 == 0 { 1 } else { 0 };
            if self.n_enemies < MAX_ENEMIES {
                if let Some(slot) = self.enemies.get_mut(self.n_enemies) {
                    *slot = Enemy {
                        x,
                        y,
                        vx: self.rng.range(-30.0, 30.0),
                        vy: if kind == 1 { 130.0 } else { 75.0 },
                        phase: self.rng.range(0.0, 6.28),
                        kind,
                        hp: if kind == 1 { 2 } else { 1 },
                        age: 0.5, // already popped in, full size
                        hit: 0.0,
                    };
                    self.n_enemies += 1;
                }
            }
            i += 1;
        }
        // A curtain of enemy bullets falling toward the ship.
        let mut j = 0usize;
        while j < 5 {
            let bx = self.rng.range(60.0, WIDTH - 60.0);
            let by = self.rng.range(200.0, 420.0);
            let bvx = self.rng.range(-40.0, 40.0);
            self.spawn_ebullet(bx, by, bvx, EBULLET_SPEED);
            j += 1;
        }
        // Two explosions mid-burst.
        let ax = self.rng.range(120.0, WIDTH - 120.0);
        let ay = self.rng.range(150.0, 300.0);
        self.explode(ax, ay, 26, true);
        let bx = self.rng.range(120.0, WIDTH - 120.0);
        let by = self.rng.range(300.0, 450.0);
        self.explode(bx, by, 20, true);
        self.add_score(250, ax, ay);
        self.shake = 6.0;
    }

    /// X of the enemy the ship should line up under: the lowest one on screen.
    /// Used only to drive the automated shot's ship so it actually kills things.
    fn target_x(&self) -> Option<f32> {
        let mut best_y = -1.0e9;
        let mut best_x = None;
        let mut i = 0usize;
        while i < self.n_enemies {
            if let Some(e) = self.enemies.get(i) {
                if e.y > best_y {
                    best_y = e.y;
                    best_x = Some(e.x);
                }
            }
            i += 1;
        }
        best_x
    }

    fn spawn_bullet(&mut self, x: f32, y: f32, vx: f32) {
        if self.n_bullets >= MAX_BULLETS {
            return;
        }
        if let Some(slot) = self.bullets.get_mut(self.n_bullets) {
            *slot = Bullet {
                x,
                y,
                vy: -BULLET_SPEED,
                vx,
            };
            self.n_bullets += 1;
        }
    }

    fn spawn_ebullet(&mut self, x: f32, y: f32, vx: f32, vy: f32) {
        if self.n_ebullets >= MAX_EBULLETS {
            return;
        }
        if let Some(slot) = self.ebullets.get_mut(self.n_ebullets) {
            *slot = Bullet { x, y, vy, vx };
            self.n_ebullets += 1;
        }
    }

    fn spawn_enemy(&mut self) {
        if self.n_enemies >= MAX_ENEMIES {
            return;
        }
        // Orange divers grow more common as the game heats up.
        let diver_chance = (0.12 + self.elapsed * 0.012).min(0.5);
        let kind: u8 = if self.rng.f() < diver_chance { 1 } else { 0 };
        let speed_ramp = 1.0 + self.elapsed * 0.03;
        let x = self.rng.range(28.0, WIDTH - 28.0);
        let (vy, hp) = if kind == 1 {
            (self.rng.range(120.0, 165.0) * speed_ramp, 2)
        } else {
            (self.rng.range(58.0, 95.0) * speed_ramp, 1)
        };
        if let Some(slot) = self.enemies.get_mut(self.n_enemies) {
            *slot = Enemy {
                x,
                y: -24.0,
                vx: self.rng.range(-40.0, 40.0),
                vy,
                phase: self.rng.range(0.0, 6.28),
                kind,
                hp,
                age: 0.0,
                hit: 0.0,
            };
            self.n_enemies += 1;
        }
    }

    fn spawn_particle(&mut self, p: Particle) {
        if self.n_particles >= MAX_PARTICLES {
            return;
        }
        if let Some(slot) = self.particles.get_mut(self.n_particles) {
            *slot = p;
            self.n_particles += 1;
        }
    }

    fn spawn_popup(&mut self, x: f32, y: f32, value: u32) {
        if self.n_popups >= MAX_POPUPS {
            return;
        }
        if let Some(slot) = self.popups.get_mut(self.n_popups) {
            *slot = Popup {
                x,
                y,
                life: 0.9,
                value,
            };
            self.n_popups += 1;
        }
    }

    /// Burst of warm particles at (x, y) for an explosion.
    fn explode(&mut self, x: f32, y: f32, count: usize, hot: bool) {
        let mut i = 0usize;
        while i < count {
            let ang = self.rng.range(0.0, 6.2831);
            let spd = self.rng.range(40.0, 260.0);
            let (sn, cs) = sin_cos(ang);
            // Warm palette: yellow-white core to orange-red edges.
            let t = self.rng.f();
            let (r, g, b) = if hot {
                (1.0, 0.55 + t * 0.4, 0.15 + t * 0.2)
            } else {
                (1.0, 0.35 + t * 0.35, 0.1 + t * 0.15)
            };
            let life = self.rng.range(0.35, 0.85);
            let size = self.rng.range(2.0, 5.0);
            self.spawn_particle(Particle {
                x,
                y,
                vx: cs * spd,
                vy: sn * spd,
                life,
                max_life: life,
                r,
                g,
                b,
                size,
            });
            i += 1;
        }
    }

    /// A short trail of thruster particles behind the ship.
    fn thruster(&mut self) {
        let jx = self.rng.range(-3.0, 3.0);
        let vy = self.rng.range(120.0, 210.0);
        let size = self.rng.range(2.0, 4.0);
        let life = 0.28;
        self.spawn_particle(Particle {
            x: self.px + jx,
            y: self.py + PLAYER_H * 0.5,
            vx: jx * 4.0,
            vy,
            life,
            max_life: life,
            r: 0.4,
            g: 0.8,
            b: 1.0,
            size,
        });
    }

    fn fire(&mut self) {
        // Twin cannons for a fuller look; slight outward spread.
        self.spawn_bullet(self.px - 7.0, self.py - PLAYER_H * 0.5, -30.0);
        self.spawn_bullet(self.px + 7.0, self.py - PLAYER_H * 0.5, 30.0);
        self.muzzle = 0.06;
        self.fire_timer = FIRE_COOLDOWN;
    }

    fn hurt_player(&mut self) {
        if self.invuln > 0.0 || self.game_over {
            return;
        }
        self.lives -= 1;
        self.invuln = 1.6;
        self.shake = (self.shake + 16.0).min(22.0);
        let (px, py) = (self.px, self.py);
        self.explode(px, py, 40, true);
        if self.lives <= 0 {
            self.lives = 0;
            self.game_over = true;
        }
    }

    fn add_score(&mut self, pts: u32, x: f32, y: f32) {
        self.score += pts;
        self.spawn_popup(x, y, pts);
    }

    fn remove_enemy(&mut self, i: usize) {
        if self.n_enemies == 0 {
            return;
        }
        let last = self.n_enemies - 1;
        if let (Some(a), Some(b)) = swap_get(&mut self.enemies, i, last) {
            *a = *b;
        }
        self.n_enemies -= 1;
    }
    fn remove_bullet(&mut self, i: usize) {
        if self.n_bullets == 0 {
            return;
        }
        let last = self.n_bullets - 1;
        if let (Some(a), Some(b)) = swap_get(&mut self.bullets, i, last) {
            *a = *b;
        }
        self.n_bullets -= 1;
    }
    fn remove_ebullet(&mut self, i: usize) {
        if self.n_ebullets == 0 {
            return;
        }
        let last = self.n_ebullets - 1;
        if let (Some(a), Some(b)) = swap_get(&mut self.ebullets, i, last) {
            *a = *b;
        }
        self.n_ebullets -= 1;
    }
    fn remove_particle(&mut self, i: usize) {
        if self.n_particles == 0 {
            return;
        }
        let last = self.n_particles - 1;
        if let (Some(a), Some(b)) = swap_get(&mut self.particles, i, last) {
            *a = *b;
        }
        self.n_particles -= 1;
    }
    fn remove_popup(&mut self, i: usize) {
        if self.n_popups == 0 {
            return;
        }
        let last = self.n_popups - 1;
        if let (Some(a), Some(b)) = swap_get(&mut self.popups, i, last) {
            *a = *b;
        }
        self.n_popups -= 1;
    }

    fn update(&mut self, dt: f32, left: bool, right: bool, up: bool, down: bool, firing: bool) {
        if self.game_over {
            // Keep particles and stars alive so the over-screen still breathes.
            self.update_stars(dt);
            self.update_particles(dt);
            self.shake = (self.shake - dt * 30.0).max(0.0);
            return;
        }
        self.elapsed += dt;
        self.shake = (self.shake - dt * 40.0).max(0.0);
        if self.invuln > 0.0 {
            self.invuln = (self.invuln - dt).max(0.0);
        }
        if self.muzzle > 0.0 {
            self.muzzle = (self.muzzle - dt).max(0.0);
        }

        // ---- player movement ----
        let mut moving = false;
        if left {
            self.px -= PLAYER_SPEED * dt;
            moving = true;
        }
        if right {
            self.px += PLAYER_SPEED * dt;
            moving = true;
        }
        if up {
            self.py -= PLAYER_SPEED * dt;
            moving = true;
        }
        if down {
            self.py += PLAYER_SPEED * dt;
            moving = true;
        }
        let _ = moving;
        self.px = clampf(self.px, PLAYER_W * 0.5 + 4.0, WIDTH - PLAYER_W * 0.5 - 4.0);
        self.py = clampf(self.py, HEIGHT() * 0.45, HEIGHT() - PLAYER_H * 0.5 - 6.0);
        // Steady thruster plume.
        self.thruster();

        // ---- firing ----
        self.fire_timer = (self.fire_timer - dt).max(0.0);
        if firing && self.fire_timer <= 0.0 {
            self.fire();
        }

        self.update_stars(dt);
        self.update_bullets(dt);
        self.update_ebullets(dt);
        self.update_enemies(dt);
        self.update_particles(dt);
        self.update_popups(dt);
        self.resolve_collisions();

        // ---- spawning (difficulty ramp) ----
        self.spawn_timer -= dt;
        if self.spawn_timer <= 0.0 {
            self.spawn_enemy();
            // Occasionally a pair for pressure.
            if self.rng.f() < (0.15 + self.elapsed * 0.01).min(0.5) {
                self.spawn_enemy();
            }
            let interval = (0.95 - self.elapsed * 0.02).max(0.28);
            self.spawn_timer = interval;
        }
    }

    fn update_stars(&mut self, dt: f32) {
        let mut i = 0usize;
        while i < MAX_STARS {
            if let Some(s) = self.stars.get_mut(i) {
                s.y += s.speed * dt;
                if s.y > HEIGHT() {
                    s.y = 0.0;
                    s.x = self.rng.range(0.0, WIDTH);
                }
            }
            i += 1;
        }
    }

    fn update_bullets(&mut self, dt: f32) {
        let mut i = 0usize;
        while i < self.n_bullets {
            let mut kill = false;
            if let Some(b) = self.bullets.get_mut(i) {
                b.y += b.vy * dt;
                b.x += b.vx * dt;
                if b.y < -12.0 {
                    kill = true;
                }
            }
            if kill {
                self.remove_bullet(i);
            } else {
                i += 1;
            }
        }
    }

    fn update_ebullets(&mut self, dt: f32) {
        let mut i = 0usize;
        while i < self.n_ebullets {
            let mut kill = false;
            if let Some(b) = self.ebullets.get_mut(i) {
                b.y += b.vy * dt;
                b.x += b.vx * dt;
                if b.y > HEIGHT() + 12.0 || b.x < -12.0 || b.x > WIDTH + 12.0 {
                    kill = true;
                }
            }
            if kill {
                self.remove_ebullet(i);
            } else {
                i += 1;
            }
        }
    }

    fn update_enemies(&mut self, dt: f32) {
        let mut i = 0usize;
        while i < self.n_enemies {
            let mut off = false;
            let mut want_fire: Option<(f32, f32)> = None;
            if let Some(e) = self.enemies.get_mut(i) {
                e.age += dt;
                if e.hit > 0.0 {
                    e.hit = (e.hit - dt).max(0.0);
                }
                e.phase += dt * 2.4;
                let weave = sinf(e.phase) * if e.kind == 1 { 90.0 } else { 45.0 };
                e.x += (e.vx + weave) * dt;
                e.y += e.vy * dt;
                if e.x < 18.0 {
                    e.x = 18.0;
                    e.vx = e.vx.abs();
                }
                if e.x > WIDTH - 18.0 {
                    e.x = WIDTH - 18.0;
                    e.vx = -e.vx.abs();
                }
                if e.y > HEIGHT() + 24.0 {
                    off = true;
                }
                // Divers shoot at the player sometimes.
                if e.kind == 1 && e.age > 0.4 {
                    // fire request handled outside the borrow
                    if (e.phase % 6.2831) < dt * 2.4 {
                        want_fire = Some((e.x, e.y));
                    }
                }
            }
            if let Some((ex, ey)) = want_fire {
                let dx = self.px - ex;
                let dy = self.py - ey;
                let len = sqrtf(dx * dx + dy * dy).max(1.0);
                self.spawn_ebullet(ex, ey, dx / len * EBULLET_SPEED, dy / len * EBULLET_SPEED);
            }
            if off {
                self.remove_enemy(i);
            } else {
                i += 1;
            }
        }
    }

    fn update_particles(&mut self, dt: f32) {
        let mut i = 0usize;
        while i < self.n_particles {
            let mut dead = false;
            if let Some(p) = self.particles.get_mut(i) {
                p.life -= dt;
                p.x += p.vx * dt;
                p.y += p.vy * dt;
                // Gentle drag so bursts decelerate.
                p.vx *= 1.0 - 2.2 * dt;
                p.vy *= 1.0 - 2.2 * dt;
                if p.life <= 0.0 {
                    dead = true;
                }
            }
            if dead {
                self.remove_particle(i);
            } else {
                i += 1;
            }
        }
    }

    fn update_popups(&mut self, dt: f32) {
        let mut i = 0usize;
        while i < self.n_popups {
            let mut dead = false;
            if let Some(p) = self.popups.get_mut(i) {
                p.life -= dt;
                p.y -= 40.0 * dt;
                if p.life <= 0.0 {
                    dead = true;
                }
            }
            if dead {
                self.remove_popup(i);
            } else {
                i += 1;
            }
        }
    }

    fn resolve_collisions(&mut self) {
        // Player bullets vs enemies.
        let mut bi = 0usize;
        while bi < self.n_bullets {
            let (bx, by) = match self.bullets.get(bi) {
                Some(b) => (b.x, b.y),
                None => break,
            };
            let mut hit_enemy: Option<usize> = None;
            let mut ei = 0usize;
            while ei < self.n_enemies {
                if let Some(e) = self.enemies.get(ei) {
                    let r = if e.kind == 1 { 18.0 } else { 15.0 };
                    let dx = bx - e.x;
                    let dy = by - e.y;
                    if dx * dx + dy * dy < r * r {
                        hit_enemy = Some(ei);
                        break;
                    }
                }
                ei += 1;
            }
            if let Some(ei) = hit_enemy {
                self.remove_bullet(bi);
                let mut killed = false;
                let mut ex = 0.0;
                let mut ey = 0.0;
                let mut kind = 0u8;
                if let Some(e) = self.enemies.get_mut(ei) {
                    e.hp -= 1;
                    e.hit = 0.12;
                    ex = e.x;
                    ey = e.y;
                    kind = e.kind;
                    if e.hp <= 0 {
                        killed = true;
                    }
                }
                // A few sparks on any hit.
                self.explode(bx, by, 5, false);
                if killed {
                    let big = kind == 1;
                    self.explode(ex, ey, if big { 34 } else { 22 }, true);
                    self.shake = (self.shake + if big { 8.0 } else { 4.0 }).min(18.0);
                    let pts = if big { 250 } else { 100 };
                    self.add_score(pts, ex, ey);
                    self.remove_enemy(ei);
                }
            } else {
                bi += 1;
            }
        }

        // Enemies vs player (ram), and enemy bullets vs player.
        let mut ei = 0usize;
        while ei < self.n_enemies {
            let mut ram = false;
            if let Some(e) = self.enemies.get(ei) {
                let dx = self.px - e.x;
                let dy = self.py - e.y;
                let rr = 22.0;
                if dx * dx + dy * dy < rr * rr {
                    ram = true;
                }
            }
            if ram {
                let (ex, ey) = self
                    .enemies
                    .get(ei)
                    .map(|e| (e.x, e.y))
                    .unwrap_or((self.px, self.py));
                self.explode(ex, ey, 24, true);
                self.remove_enemy(ei);
                self.hurt_player();
            } else {
                ei += 1;
            }
        }

        let mut i = 0usize;
        while i < self.n_ebullets {
            let mut hit = false;
            if let Some(b) = self.ebullets.get(i) {
                let dx = self.px - b.x;
                let dy = self.py - b.y;
                if dx * dx + dy * dy < 18.0 * 18.0 {
                    hit = true;
                }
            }
            if hit {
                self.remove_ebullet(i);
                self.hurt_player();
            } else {
                i += 1;
            }
        }
    }
}

// ------------------------------------------------------------------
// Rendering
// ------------------------------------------------------------------

fn draw(canvas: u64, w: &World, blink_on: bool) -> Result<(), gfx::GfxError> {
    // Screen shake: offset the whole scene a few pixels while shake decays.
    let (sx, sy) = if w.shake > 0.1 {
        let a = w.shake;
        (
            (sinf(w.elapsed * 53.0) * a).clamp(-a, a),
            (sinf(w.elapsed * 61.0 + 1.3) * a).clamp(-a, a),
        )
    } else {
        (0.0, 0.0)
    };

    // Deep space background: one native vertical gradient, a faintly-blue glow
    // up top easing to a darker floor. The host draws the smooth ramp, so there
    // is no seam and no band loop -- the void reads as one continuous space.
    // No shake offset: the void stays put while the action shakes over it.
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect {
            x: 0.0,
            y: 0.0,
            width: WIDTH,
            height: HEIGHT(),
        },
        color(0.067, 0.067, 0.126, 1.0),
        color(0.039, 0.039, 0.071, 1.0),
    )?;

    // ---- starfield ----
    let mut i = 0usize;
    while i < MAX_STARS {
        if let Some(s) = w.stars.get(i) {
            let c = color(s.bright * 0.7, s.bright * 0.85, s.bright, 1.0);
            fill(canvas, sx, sy, s.x, s.y, s.size, s.size, c)?;
        }
        i += 1;
    }

    // ---- player bullets (bright cyan-white streaks with a glow) ----
    let mut i = 0usize;
    while i < w.n_bullets {
        if let Some(b) = w.bullets.get(i) {
            // Long soft trailing glow.
            fill(
                canvas,
                sx,
                sy,
                b.x - 4.0,
                b.y - 12.0,
                8.0,
                28.0,
                color(0.15, 0.75, 1.0, 0.28),
            )?;
            // Tighter cyan glow.
            fill(
                canvas,
                sx,
                sy,
                b.x - 2.5,
                b.y - 10.0,
                5.0,
                22.0,
                color(0.35, 0.95, 1.0, 0.6),
            )?;
            // White-hot core.
            fill(
                canvas,
                sx,
                sy,
                b.x - 1.5,
                b.y - 9.0,
                3.0,
                18.0,
                color(0.95, 1.0, 1.0, 1.0),
            )?;
        }
        i += 1;
    }

    // ---- enemy bullets (hot glowing orbs) ----
    let mut i = 0usize;
    while i < w.n_ebullets {
        if let Some(b) = w.ebullets.get(i) {
            glow(canvas, sx, sy, b.x, b.y, 4.0, 1.0, 0.5, 0.18)?;
        }
        i += 1;
    }

    // ---- enemies ----
    let mut i = 0usize;
    while i < w.n_enemies {
        if let Some(e) = w.enemies.get(i) {
            draw_enemy(canvas, sx, sy, e)?;
        }
        i += 1;
    }

    // ---- particles (round glowing sparks that fade, with a soft halo) ----
    let mut i = 0usize;
    while i < w.n_particles {
        if let Some(p) = w.particles.get(i) {
            let t = (p.life / p.max_life).clamp(0.0, 1.0);
            let radius = p.size * (0.35 + t * 0.65) * 0.7;
            // Soft halo, then a bright core -- a spark that glows rather than a
            // hard square. Halo alpha fades faster than the core.
            disc(
                canvas,
                sx,
                sy,
                p.x,
                p.y,
                radius * 2.0,
                color(p.r, p.g, p.b, t * 0.22),
            )?;
            disc(canvas, sx, sy, p.x, p.y, radius, color(p.r, p.g, p.b, t))?;
            // Hot white center on the freshest sparks.
            if t > 0.55 {
                disc(
                    canvas,
                    sx,
                    sy,
                    p.x,
                    p.y,
                    radius * 0.5,
                    color(1.0, 1.0, 0.92, (t - 0.55) * 2.0),
                )?;
            }
        }
        i += 1;
    }

    // ---- player ship (draw last so it sits on top; blink while invulnerable) ----
    if !w.game_over && (w.invuln <= 0.0 || blink_on) {
        draw_ship(canvas, sx, sy, w.px, w.py, w.muzzle > 0.0)?;
    }

    // ---- score popups ----
    let mut i = 0usize;
    while i < w.n_popups {
        if let Some(p) = w.popups.get(i) {
            let a = (p.life / 0.9).clamp(0.0, 1.0);
            let mut buf = [0u8; 20];
            let s = num_to_bytes_prefixed(b"+", p.value, &mut buf);
            if let Ok(txt) = core::str::from_utf8(s) {
                draw_text(
                    canvas,
                    sx,
                    sy,
                    txt,
                    p.x - 12.0,
                    p.y,
                    text(15.0),
                    color(1.0, 0.95, 0.5, a),
                )?;
            }
        }
        i += 1;
    }

    // ---- HUD ----
    draw_hud(canvas, w)?;

    if w.game_over {
        // Dim overlay + big centered message.
        fill(
            canvas,
            0.0,
            0.0,
            0.0,
            0.0,
            WIDTH,
            HEIGHT(),
            color(0.02, 0.02, 0.05, 0.55),
        )?;
        // Centred by measuring, so the lines stay centred at any text size.
        let (big, mid, small) = (text(40.0), text(20.0), text(14.0));
        let cy = HEIGHT() * 0.5;
        let title = "GAME OVER";
        draw_text(
            canvas,
            0.0,
            0.0,
            title,
            (WIDTH - text_w(canvas, title, big)) * 0.5,
            cy - big * 0.25,
            big,
            color(1.0, 0.35, 0.5, 1.0),
        )?;
        let mut buf = [0u8; 20];
        let s = num_to_bytes_prefixed(b"SCORE  ", w.score, &mut buf);
        if let Ok(txt) = core::str::from_utf8(s) {
            draw_text(
                canvas,
                0.0,
                0.0,
                txt,
                (WIDTH - text_w(canvas, txt, mid)) * 0.5,
                cy - big * 0.25 + mid * 2.0,
                mid,
                color(0.9, 0.95, 1.0, 1.0),
            )?;
        }
        // The best, or the news that this run is it.
        let mut bbuf = [0u8; 20];
        let best = if w.score > 0 && w.score >= w.best {
            Some(&b"NEW BEST"[..])
        } else if w.best > 0 {
            Some(num_to_bytes_prefixed(b"BEST  ", w.best, &mut bbuf))
        } else {
            None
        };
        if let Some(txt) = best.and_then(|b| core::str::from_utf8(b).ok()) {
            draw_text(
                canvas,
                0.0,
                0.0,
                txt,
                (WIDTH - text_w(canvas, txt, small)) * 0.5,
                cy - big * 0.25 + mid * 2.0 + small * 1.6,
                small,
                color(1.0, 0.8, 0.35, 1.0),
            )?;
        }
        let again = "TAP OR PRESS SPACE TO PLAY AGAIN";
        draw_text(
            canvas,
            0.0,
            0.0,
            again,
            (WIDTH - text_w(canvas, again, small)) * 0.5,
            cy - big * 0.25 + mid * 2.0 + small * 3.6,
            small,
            color(0.75, 0.8, 0.95, 0.9),
        )?;
    }

    canvas2d::present(canvas)?;
    Ok(())
}

/// The player ship: a sleek glowing cyan arrowhead with a lit cockpit, glowing
/// wing lights, and round muzzle flashes when firing. Drawn with smooth chords
/// so its edges read as a crisp craft, not a stack of blocks.
fn draw_ship(
    canvas: u64,
    sx: f32,
    sy: f32,
    x: f32,
    y: f32,
    muzzle: bool,
) -> Result<(), gfx::GfxError> {
    let cyan = color(0.20, 0.95, 1.0, 1.0);
    let cyan_deep = color(0.10, 0.55, 0.80, 1.0);
    // Ambient glow beneath the whole craft so it sits in a pool of light.
    disc(
        canvas,
        sx,
        sy,
        x,
        y - 2.0,
        22.0,
        color(0.15, 0.75, 1.0, 0.10),
    )?;
    disc(
        canvas,
        sx,
        sy,
        x,
        y - 2.0,
        15.0,
        color(0.15, 0.80, 1.0, 0.14),
    )?;

    // The hull: an arrowhead. For each row from nose (top) to tail, the half
    // width grows linearly, and each row is one smooth chord. A darker under-
    // hull row behind it gives a hint of depth.
    let nose = y - 17.0;
    let tail = y + 12.0;
    let mut yy = nose as i32;
    while yy <= tail as i32 {
        let f = ((yy as f32) - nose) / (tail - nose); // 0 at nose, 1 at tail
        let half = 2.0 + f * f * 16.0; // narrow nose flaring to wide tail
                                       // Deep under-tone at the outer edge, bright core toward the center.
        fill(
            canvas,
            sx,
            sy,
            x - half,
            yy as f32,
            half * 2.0,
            1.0,
            cyan_deep,
        )?;
        let inner = half * 0.55;
        fill(canvas, sx, sy, x - inner, yy as f32, inner * 2.0, 1.0, cyan)?;
        yy += 1;
    }

    // Swept wing tips: two small glowing lights out at the tail corners.
    disc(canvas, sx, sy, x - 15.0, y + 9.0, 4.0, cyan)?;
    disc(canvas, sx, sy, x + 15.0, y + 9.0, 4.0, cyan)?;

    // Lit cockpit: a bright white-cyan orb near the nose.
    disc(canvas, sx, sy, x, y - 4.0, 4.5, color(0.85, 1.0, 1.0, 1.0))?;
    disc(canvas, sx, sy, x, y - 4.0, 2.2, color(1.0, 1.0, 1.0, 1.0))?;

    if muzzle {
        // Twin round muzzle flashes at the nose.
        glow(canvas, sx, sy, x - 7.0, y - 16.0, 4.0, 0.7, 1.0, 1.0)?;
        glow(canvas, sx, sy, x + 7.0, y - 16.0, 4.0, 0.7, 1.0, 1.0)?;
    }
    Ok(())
}

/// An enemy: a diamond of stacked bars. Magenta grunt or orange diver, with a
/// spawn pop-in scale and a white flash when hit.
fn draw_enemy(canvas: u64, sx: f32, sy: f32, e: &Enemy) -> Result<(), gfx::GfxError> {
    let pop = (e.age * 6.0).min(1.0); // scale in over ~0.16s
    let scale = 0.4 + pop * 0.6;
    let flash = e.hit > 0.0;
    // Orange divers vs magenta grunts, each a glowing orb.
    let (r, g, b) = if e.kind == 1 {
        (1.0, 0.52, 0.12)
    } else {
        (1.0, 0.24, 0.72)
    };
    let radius = (if e.kind == 1 { 15.0 } else { 12.5 }) * scale;

    if flash {
        // On hit: a white flash orb, bigger, so a kill reads instantly.
        glow(canvas, sx, sy, e.x, e.y, radius * 1.15, 1.0, 1.0, 1.0)?;
        return Ok(());
    }

    // The glowing body.
    glow(canvas, sx, sy, e.x, e.y, radius, r, g, b)?;
    // A dark iris so the orb reads as an eye/creature, not a plain dot -- the
    // small detail that makes it feel designed.
    disc(
        canvas,
        sx,
        sy,
        e.x,
        e.y,
        radius * 0.32,
        color(r * 0.25, g * 0.12, b * 0.2, 0.9),
    )?;
    // A tiny offset highlight for a sense of a lit surface.
    disc(
        canvas,
        sx,
        sy,
        e.x - radius * 0.28,
        e.y - radius * 0.28,
        radius * 0.16,
        color(1.0, 1.0, 1.0, 0.85),
    )?;
    Ok(())
}

fn draw_hud(canvas: u64, w: &World) -> Result<(), gfx::GfxError> {
    // Sizes at the person's text size; the rows move down as they grow.
    let (big, label) = (text(30.0), text(12.0));
    let score_y = 11.0 + big * 0.76;
    let label_y = score_y + 4.0 + label;
    // Score top-left, big.
    let mut buf = [0u8; 20];
    let s = num_to_bytes(w.score, &mut buf);
    if let Ok(txt) = core::str::from_utf8(s) {
        draw_text(
            canvas,
            0.0,
            0.0,
            txt,
            16.0,
            score_y,
            big,
            color(0.85, 0.95, 1.0, 1.0),
        )?;
    }
    draw_text(
        canvas,
        0.0,
        0.0,
        "SCORE",
        16.0,
        label_y,
        label,
        color(0.45, 0.6, 0.8, 1.0),
    )?;
    // The best so far, top-centre, once one has been kept.
    if w.best > 0 {
        let mut bbuf = [0u8; 20];
        let b = num_to_bytes_prefixed(b"BEST  ", w.best.max(w.score), &mut bbuf);
        if let Ok(txt) = core::str::from_utf8(b) {
            draw_text(
                canvas,
                0.0,
                0.0,
                txt,
                (WIDTH - text_w(canvas, txt, label)) * 0.5,
                label_y,
                label,
                color(0.45, 0.6, 0.8, 1.0),
            )?;
        }
    }

    // Lives top-right as little cyan chevrons.
    let mut i = 0i32;
    while i < w.lives {
        let x = WIDTH - 28.0 - (i as f32) * 24.0;
        let y = score_y - 10.0;
        fill(
            canvas,
            0.0,
            0.0,
            x - 8.0,
            y + 4.0,
            16.0,
            4.0,
            color(0.2, 0.9, 1.0, 1.0),
        )?;
        fill(
            canvas,
            0.0,
            0.0,
            x - 4.0,
            y - 2.0,
            8.0,
            6.0,
            color(0.2, 0.9, 1.0, 1.0),
        )?;
        fill(
            canvas,
            0.0,
            0.0,
            x - 1.5,
            y - 8.0,
            3.0,
            8.0,
            color(0.7, 1.0, 1.0, 1.0),
        )?;
        i += 1;
    }
    draw_text(
        canvas,
        0.0,
        0.0,
        "LIVES",
        WIDTH - 20.0 - text_w(canvas, "LIVES", label),
        label_y,
        label,
        color(0.45, 0.6, 0.8, 1.0),
    )?;
    Ok(())
}

// ------------------------------------------------------------------
// Small helpers
// ------------------------------------------------------------------

/// Fill a rect with the shake offset applied.
fn fill(
    canvas: u64,
    sx: f32,
    sy: f32,
    x: f32,
    y: f32,
    wdt: f32,
    hgt: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x: x + sx,
            y: y + sy,
            width: wdt,
            height: hgt,
        },
        c,
    )
}

/// A crisp anti-aliased filled circle -- one native runtime call. The host
/// smooths the edge and blends it, so this is a true round dot, not a scanned
/// approximation.
fn disc(
    canvas: u64,
    sx: f32,
    sy: f32,
    cx: f32,
    cy: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(
        canvas,
        gfx::Point {
            x: cx + sx,
            y: cy + sy,
        },
        r,
        c,
    )
}

/// A soft radial glow: one native radial gradient for the bloom halo (the true
/// color easing to transparent, so light falls off smoothly with no banding),
/// a solid saturated body, and a small lifted-brightness core so it reads as
/// lit from within. The gradient is a single host call instead of a stack of
/// discs, so the falloff is continuous.
fn glow(
    canvas: u64,
    sx: f32,
    sy: f32,
    cx: f32,
    cy: f32,
    r: f32,
    red: f32,
    grn: f32,
    blu: f32,
) -> Result<(), gfx::GfxError> {
    let center = gfx::Point {
        x: cx + sx,
        y: cy + sy,
    };
    // Bloom: bright at the core, fading to fully transparent at ~2x the body
    // radius. Smoothstep falloff is done host-side, so the halo is continuous.
    canvas2d::radial_gradient(
        canvas,
        center,
        r * 2.1,
        color(red, grn, blu, 0.55),
        color(red, grn, blu, 0.0),
    )?;
    // Solid saturated body.
    canvas2d::fill_circle(canvas, center, r, color(red, grn, blu, 1.0))?;
    // Hot center, only a touch toward white so it stays vivid.
    let lift = |v: f32| (v * 0.5 + 0.5).min(1.0);
    canvas2d::fill_circle(
        canvas,
        center,
        r * 0.42,
        color(lift(red), lift(grn), lift(blu), 1.0),
    )?;
    Ok(())
}

fn draw_text(
    canvas: u64,
    sx: f32,
    sy: f32,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::draw_text(
        canvas,
        text,
        gfx::Point {
            x: x + sx,
            y: y + sy,
        },
        size,
        c,
    )
}

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Swap-access two distinct indices for the swap-remove idiom, panic-free.
fn swap_get<T>(arr: &mut [T], i: usize, j: usize) -> (Option<&mut T>, Option<&T>) {
    if i == j || i >= arr.len() || j >= arr.len() {
        return (None, None);
    }
    // Split so we can hold a &mut to i and & to j simultaneously.
    if i < j {
        let (a, b) = arr.split_at_mut(j);
        (a.get_mut(i), b.first().map(|x| &*x))
    } else {
        let (a, b) = arr.split_at_mut(i);
        (b.first_mut(), a.get(j).map(|x| &*x))
    }
}

// ---- no_std math (small Taylor/approx, plenty accurate for a game) ----

const PI: f32 = 3.14159265;
const TAU: f32 = 6.2831853;

fn sinf(x: f32) -> f32 {
    // Reduce to [-PI, PI].
    let mut a = x % TAU;
    if a > PI {
        a -= TAU;
    } else if a < -PI {
        a += TAU;
    }
    // Bhaskara-style approximation, mirrored for the negative half.
    let neg = a < 0.0;
    let a = if neg { -a } else { a };
    // sin(a) ~ 16a(pi-a) / (5pi^2 - 4a(pi-a)) for a in [0, pi].
    let num = 16.0 * a * (PI - a);
    let den = 5.0 * PI * PI - 4.0 * a * (PI - a);
    let s = num / den;
    if neg {
        -s
    } else {
        s
    }
}

fn sin_cos(x: f32) -> (f32, f32) {
    (sinf(x), sinf(x + PI * 0.5))
}

fn sqrtf(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    // Newton's method seeded off the bit-halving trick.
    let mut g = x;
    let mut i = 0;
    while i < 6 {
        g = 0.5 * (g + x / g);
        i += 1;
    }
    g
}

// ---- number formatting into byte buffers, panic-free ----

fn num_to_bytes(value: u32, buf: &mut [u8; 20]) -> &[u8] {
    num_core(value, buf)
}

fn num_to_bytes_prefixed<'a>(prefix: &[u8], value: u32, buf: &'a mut [u8; 20]) -> &'a [u8] {
    let mut pos = 0usize;
    let mut i = 0usize;
    while i < prefix.len() && pos < buf.len() {
        if let (Some(src), Some(dst)) = (prefix.get(i), buf.get_mut(pos)) {
            *dst = *src;
            pos += 1;
        }
        i += 1;
    }
    let mut tmp = [0u8; 12];
    let digits = num_core12(value, &mut tmp);
    let mut j = 0usize;
    while j < digits.len() && pos < buf.len() {
        if let (Some(src), Some(dst)) = (digits.get(j), buf.get_mut(pos)) {
            *dst = *src;
            pos += 1;
        }
        j += 1;
    }
    buf.get(..pos).unwrap_or(b"")
}

fn num_core(value: u32, buf: &mut [u8; 20]) -> &[u8] {
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

fn num_core12(value: u32, buf: &mut [u8; 12]) -> &[u8] {
    if value == 0 {
        if let Some(slot) = buf.get_mut(0) {
            *slot = b'0';
        }
        return buf.get(..1).unwrap_or(b"0");
    }
    let mut scratch = [0u8; 12];
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

// ------------------------------------------------------------------
// The best score, kept in store.kv
// ------------------------------------------------------------------

/// The best score saved on this or any of the person's devices; 0 when
/// nothing is saved yet or the store is not granted.
fn load_best() -> u32 {
    let Ok(Some(bytes)) = krate::store::get(BEST_KEY) else {
        return 0;
    };
    let mut n = 0u32;
    for b in bytes.iter() {
        if !b.is_ascii_digit() {
            return 0;
        }
        n = n.saturating_mul(10).saturating_add((b - b'0') as u32);
    }
    n
}

/// Keep this game's score if it beats the best. Re-reads the store first, so
/// a better score another device synced in meanwhile is never overwritten,
/// and writes nothing when the best stands.
fn bank_best(w: &mut World) {
    let stored = load_best();
    w.best = w.best.max(stored);
    if w.score > w.best {
        w.best = w.score;
        let mut buf = [0u8; 20];
        let _ = krate::store::set(BEST_KEY, num_to_bytes(w.score, &mut buf));
    }
}

// ------------------------------------------------------------------
// Entry point
// ------------------------------------------------------------------

impl krate::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT() as u32,
        };
        let Ok(win) = window::create("Krate Nova", size) else {
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
        // The app's own coordinate system: keep drawing in these numbers
        // and the host scales them to any window, centred, never stretched
        // out of proportion (K-096).
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT(),
            },
        );

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        // The world's height from the screen before anything is placed in it.
        fit_world(win, canvas);
        let started = clock::monotonic_nanos();
        let mut world = World::new((started as u32) ^ 0xA53C_1FE9, quick);
        // The automated shot neither reads nor writes the best: its score is
        // staged, and its picture must not depend on who ran it.
        if !quick {
            world.best = load_best();
        }
        // Whether this game's score has been offered to the store yet.
        let mut banked = false;

        // The host captures the FIRST presented frame. So for the automated
        // shot, build a full, lively scene and let it settle for a few steps
        // BEFORE the first draw -- staged enemies, a rain of bullets, the ship's
        // own fire in flight, and explosions caught mid-burst -- so the very
        // first frame is peak action, not an empty opening.
        if quick {
            world.stage_showtime();
            let mut warm = 0u32;
            while warm < 14 {
                // Fire from the ship and drift everything so bullets streak and
                // particles spread before the shutter.
                let (mut l, mut r) = (false, false);
                if let Some(tx) = world.target_x() {
                    if tx < world.px - 6.0 {
                        l = true;
                    } else if tx > world.px + 6.0 {
                        r = true;
                    }
                }
                // Hold off the random spawner during staging so no half-spawned
                // enemy sits clipped at the very top when the shutter fires; the
                // scene is composed entirely by stage_showtime.
                world.spawn_timer = 5.0;
                world.update(0.016, l, r, false, false, true);
                // Keep the field full: top up enemies the ship clears out.
                if warm == 7 {
                    world.stage_showtime();
                }
                warm += 1;
            }
        }

        let frame_cap = if quick { QUICK_FRAMES } else { MAX_FRAMES };
        let seed = (started as u32) ^ 0xA53C_1FE9;
        let mut last = clock::monotonic_nanos();
        let mut frames = 0u32;
        let mut blink_acc = 0.0f32;
        let mut blink_on = true;

        while frames < frame_cap {
            // The phone turned, or a bar came or went: the world follows.
            let grew = fit_world(win, canvas);
            if grew != 0.0 {
                world.grown(grew);
            }
            let now = clock::monotonic_nanos();
            let real_dt = (now.saturating_sub(last) as f32 / 1_000_000_000.0).min(0.05);
            last = now;

            // In quick mode, advance with a fixed lively timestep so the shot
            // lands on mid-action regardless of headless wall-clock speed.
            let dt = if quick { 0.016 } else { real_dt.max(0.0001) };

            // Blink cadence for the invuln flash.
            blink_acc += dt;
            if blink_acc > 0.08 {
                blink_acc = 0.0;
                blink_on = !blink_on;
            }

            // Every event this frame, not one: a moving finger sends many.
            let mut closing = false;
            let mut tapped = false;
            while let Some(event) = events::poll() {
                match event {
                    types::Event::CloseRequested(id) if id == win => closing = true,
                    types::Event::Pointer(p) if p.window == win && p.pressed => tapped = true,
                    _ => {}
                }
            }
            if closing {
                break;
            }
            // A game that just ended is kept once, the moment it ends -- not
            // every frame, and not only when the window closes.
            if world.game_over && !quick && !banked {
                bank_best(&mut world);
                banked = true;
            }
            // After GAME OVER, a tap or Space starts a new game.
            if world.game_over
                && !quick
                && (tapped || events::key_held("Space") || events::key_held(" "))
            {
                let best = world.best;
                world = World::new(seed ^ frames, false);
                world.best = best;
                banked = false;
            }
            // A finger on a phone, or a held mouse button, where it is in the
            // game's own coordinates. A phone has no arrow keys, so the ship
            // follows the finger and fires while it is down.
            let finger = if pointer::button_held(types::PointerButton::Primary) {
                pointer::position(win)
            } else {
                None
            };

            let (left, right, up, down, firing) = if quick {
                // Auto-pilot for the automated shot: slide under the lowest
                // enemy and hold fire, so the captured frame is full of bullets
                // in flight, enemies mid-screen, and explosions bursting.
                let (mut l, mut r) = (false, false);
                if let Some(tx) = world.target_x() {
                    if tx < world.px - 6.0 {
                        l = true;
                    } else if tx > world.px + 6.0 {
                        r = true;
                    }
                }
                (l, r, false, false, true)
            } else if let Some((fx, fy)) = finger {
                // Toward a point a thumb's width above the finger, so the
                // finger never covers the ship.
                let (tx, ty) = (fx, fy - FINGER_LEAD);
                (
                    tx < world.px - 6.0,
                    tx > world.px + 6.0,
                    ty < world.py - 6.0,
                    ty > world.py + 6.0,
                    true,
                )
            } else {
                (
                    events::key_held("ArrowLeft") || events::key_held("a"),
                    events::key_held("ArrowRight") || events::key_held("d"),
                    events::key_held("ArrowUp") || events::key_held("w"),
                    events::key_held("ArrowDown") || events::key_held("s"),
                    events::key_held("Space") || events::key_held(" "),
                )
            };

            world.update(dt, left, right, up, down, firing);

            if draw(canvas, &world, blink_on).is_err() {
                break;
            }
            frames += 1;
        }

        // Closed mid-game: a best reached before the close still counts.
        if !quick && !banked {
            bank_best(&mut world);
        }

        // Report on exit.
        let out = stdio::stdout();
        let _ = out.write(b"nova:ok\n");
        let _ = out.write(b"score:");
        let mut buf = [0u8; 20];
        let _ = out.write(num_to_bytes(world.score, &mut buf));
        let _ = out.write(b"\n");
        let _ = out.write(b"best:");
        let _ = out.write(num_to_bytes(world.best, &mut buf));
        let _ = out.write(b"\n");

        let _ = window::close(win);
        0
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
