//! Krate Nova — a neon top-down arcade space shooter.
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

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

// Linked purely for its no_std runtime lang items -- the global allocator, the
// trapping panic handler, and the memory intrinsics a wasm guest needs when std
// is not linked. Not called directly; the underscore keeps the import.
extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 480.0;
const HEIGHT: f32 = 640.0;

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
const BULLET_SPEED: f32 = 620.0;
const FIRE_COOLDOWN: f32 = 0.14;
const EBULLET_SPEED: f32 = 230.0;

const QUICK_FRAMES: u32 = 240;
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
                y: rng.range(0.0, HEIGHT),
                speed,
                bright,
                size,
            };
            i += 1;
        }
        World {
            rng,
            px: WIDTH / 2.0,
            py: HEIGHT - 70.0,
            lives: 3,
            // The automated shot depicts a run already in progress, so it opens
            // on a believable mid-game score rather than zero.
            score: if quick { 12_800 } else { 0 },
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
        self.py = clampf(self.py, HEIGHT * 0.45, HEIGHT - PLAYER_H * 0.5 - 6.0);
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
                if s.y > HEIGHT {
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
                if b.y > HEIGHT + 12.0 || b.x < -12.0 || b.x > WIDTH + 12.0 {
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
                if e.y > HEIGHT + 24.0 {
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

    // Deep space background: a smooth vertical gradient, dim glow up top easing
    // to a darker floor. Drawn as a stack of thin bands so there is no hard
    // seam between two flat rectangles -- the eye reads one continuous void.
    canvas2d::clear(canvas, color(0.039, 0.039, 0.071, 1.0))?;
    let bands = 48u32;
    let mut bi = 0u32;
    while bi < bands {
        let t = bi as f32 / bands as f32;
        // Lighter, faintly blue at the top (t=0) easing to the base color at
        // the bottom. A gentle ease so the top glow falls off naturally.
        let fade = (1.0 - t) * (1.0 - t);
        let r = 0.039 + fade * 0.028;
        let g = 0.039 + fade * 0.028;
        let b = 0.071 + fade * 0.055;
        let band_h = HEIGHT / bands as f32 + 1.0;
        // No shake offset on the background: the void stays put while the
        // action shakes over it, so screen shake never bares a screen edge.
        fill(canvas, 0.0, 0.0, 0.0, t * HEIGHT, WIDTH, band_h, color(r, g, b, 1.0))?;
        bi += 1;
    }

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
            fill(canvas, sx, sy, b.x - 4.0, b.y - 12.0, 8.0, 28.0, color(0.15, 0.75, 1.0, 0.28))?;
            // Tighter cyan glow.
            fill(canvas, sx, sy, b.x - 2.5, b.y - 10.0, 5.0, 22.0, color(0.35, 0.95, 1.0, 0.6))?;
            // White-hot core.
            fill(canvas, sx, sy, b.x - 1.5, b.y - 9.0, 3.0, 18.0, color(0.95, 1.0, 1.0, 1.0))?;
        }
        i += 1;
    }

    // ---- enemy bullets (hot pink-orange dots) ----
    let mut i = 0usize;
    while i < w.n_ebullets {
        if let Some(b) = w.ebullets.get(i) {
            fill(canvas, sx, sy, b.x - 4.0, b.y - 4.0, 8.0, 8.0, color(1.0, 0.4, 0.2, 0.5))?;
            fill(canvas, sx, sy, b.x - 2.0, b.y - 2.0, 4.0, 4.0, color(1.0, 0.85, 0.4, 1.0))?;
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

    // ---- particles (additive-feeling warm sparks that fade) ----
    let mut i = 0usize;
    while i < w.n_particles {
        if let Some(p) = w.particles.get(i) {
            let t = (p.life / p.max_life).clamp(0.0, 1.0);
            let a = t;
            let sz = p.size * (0.4 + t * 0.6);
            fill(
                canvas,
                sx,
                sy,
                p.x - sz * 0.5,
                p.y - sz * 0.5,
                sz,
                sz,
                color(p.r, p.g, p.b, a),
            )?;
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
                draw_text(canvas, sx, sy, txt, p.x - 12.0, p.y, 15.0, color(1.0, 0.95, 0.5, a))?;
            }
        }
        i += 1;
    }

    // ---- HUD ----
    draw_hud(canvas, w)?;

    if w.game_over {
        // Dim overlay + big centered message.
        fill(canvas, 0.0, 0.0, 0.0, 0.0, WIDTH, HEIGHT, color(0.02, 0.02, 0.05, 0.55))?;
        draw_text(canvas, 0.0, 0.0, "GAME OVER", WIDTH * 0.5 - 96.0, HEIGHT * 0.5 - 10.0, 40.0, color(1.0, 0.35, 0.5, 1.0))?;
        let mut buf = [0u8; 20];
        let s = num_to_bytes_prefixed(b"SCORE  ", w.score, &mut buf);
        if let Ok(txt) = core::str::from_utf8(s) {
            draw_text(canvas, 0.0, 0.0, txt, WIDTH * 0.5 - 70.0, HEIGHT * 0.5 + 30.0, 20.0, color(0.9, 0.95, 1.0, 1.0))?;
        }
    }

    canvas2d::present(canvas)?;
    Ok(())
}

/// The player ship: a cyan triangle built from stacked rectangles, with a bright
/// core, wing tips, and a muzzle flash when firing.
fn draw_ship(canvas: u64, sx: f32, sy: f32, x: f32, y: f32, muzzle: bool) -> Result<(), gfx::GfxError> {
    let cyan = color(0.2, 0.95, 1.0, 1.0);
    let cyan_dim = color(0.1, 0.55, 0.75, 1.0);
    // Outer glow.
    fill(canvas, sx, sy, x - 18.0, y - 16.0, 36.0, 34.0, color(0.15, 0.7, 0.9, 0.16))?;
    // Nose to tail as a stack of narrowing/widening bars -> triangle silhouette.
    // rows from top (nose) to bottom (wings)
    let rows: [(f32, f32); 6] = [
        (-15.0, 4.0),
        (-11.0, 8.0),
        (-6.0, 14.0),
        (-1.0, 20.0),
        (4.0, 26.0),
        (9.0, 22.0),
    ];
    let mut i = 0usize;
    while i < rows.len() {
        if let Some((oy, wdt)) = rows.get(i).copied() {
            let c = if i < 3 { cyan } else { cyan_dim };
            fill(canvas, sx, sy, x - wdt * 0.5, y + oy, wdt, 5.0, c)?;
        }
        i += 1;
    }
    // Bright cockpit core.
    fill(canvas, sx, sy, x - 3.0, y - 8.0, 6.0, 12.0, color(0.9, 1.0, 1.0, 1.0))?;
    // Wing tips.
    fill(canvas, sx, sy, x - 15.0, y + 6.0, 5.0, 8.0, cyan)?;
    fill(canvas, sx, sy, x + 10.0, y + 6.0, 5.0, 8.0, cyan)?;
    if muzzle {
        // Twin muzzle flashes.
        fill(canvas, sx, sy, x - 10.0, y - 18.0, 6.0, 8.0, color(0.8, 1.0, 1.0, 0.9))?;
        fill(canvas, sx, sy, x + 4.0, y - 18.0, 6.0, 8.0, color(0.8, 1.0, 1.0, 0.9))?;
    }
    Ok(())
}

/// An enemy: a diamond of stacked bars. Magenta grunt or orange diver, with a
/// spawn pop-in scale and a white flash when hit.
fn draw_enemy(canvas: u64, sx: f32, sy: f32, e: &Enemy) -> Result<(), gfx::GfxError> {
    let pop = (e.age * 6.0).min(1.0); // scale in over ~0.16s
    let scale = 0.4 + pop * 0.6;
    let flash = e.hit > 0.0;
    let (base, glow) = if e.kind == 1 {
        (color(1.0, 0.5, 0.1, 1.0), color(1.0, 0.45, 0.1, 0.18))
    } else {
        (color(1.0, 0.2, 0.75, 1.0), color(1.0, 0.15, 0.7, 0.18))
    };
    let body = if flash { color(1.0, 1.0, 1.0, 1.0) } else { base };
    let s = if e.kind == 1 { 18.0 } else { 15.0 } * scale;
    // Glow.
    fill(canvas, sx, sy, e.x - s, e.y - s, s * 2.0, s * 2.0, glow)?;
    // Diamond via three horizontal bars (narrow, wide, narrow) + verticals.
    fill(canvas, sx, sy, e.x - s * 0.35, e.y - s, s * 0.7, s * 2.0, body)?;
    fill(canvas, sx, sy, e.x - s, e.y - s * 0.35, s * 2.0, s * 0.7, body)?;
    fill(canvas, sx, sy, e.x - s * 0.7, e.y - s * 0.7, s * 1.4, s * 1.4, body)?;
    // Dark core so it reads as a shape, not a blob.
    if !flash {
        let core = if e.kind == 1 {
            color(0.5, 0.15, 0.02, 1.0)
        } else {
            color(0.45, 0.05, 0.3, 1.0)
        };
        fill(canvas, sx, sy, e.x - s * 0.28, e.y - s * 0.28, s * 0.56, s * 0.56, core)?;
    }
    Ok(())
}

fn draw_hud(canvas: u64, w: &World) -> Result<(), gfx::GfxError> {
    // Score top-left, big.
    let mut buf = [0u8; 20];
    let s = num_to_bytes(w.score, &mut buf);
    if let Ok(txt) = core::str::from_utf8(s) {
        draw_text(canvas, 0.0, 0.0, txt, 16.0, 34.0, 30.0, color(0.85, 0.95, 1.0, 1.0))?;
    }
    draw_text(canvas, 0.0, 0.0, "SCORE", 16.0, 50.0, 12.0, color(0.45, 0.6, 0.8, 1.0))?;

    // Lives top-right as little cyan chevrons.
    let mut i = 0i32;
    while i < w.lives {
        let x = WIDTH - 28.0 - (i as f32) * 24.0;
        let y = 24.0;
        fill(canvas, 0.0, 0.0, x - 8.0, y + 4.0, 16.0, 4.0, color(0.2, 0.9, 1.0, 1.0))?;
        fill(canvas, 0.0, 0.0, x - 4.0, y - 2.0, 8.0, 6.0, color(0.2, 0.9, 1.0, 1.0))?;
        fill(canvas, 0.0, 0.0, x - 1.5, y - 8.0, 3.0, 8.0, color(0.7, 1.0, 1.0, 1.0))?;
        i += 1;
    }
    draw_text(canvas, 0.0, 0.0, "LIVES", WIDTH - 92.0, 50.0, 12.0, color(0.45, 0.6, 0.8, 1.0))?;
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
        gfx::Point { x: x + sx, y: y + sy },
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
// Entry point
// ------------------------------------------------------------------

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
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

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        let started = clock::monotonic_nanos();
        let mut world = World::new((started as u32) ^ 0xA53C_1FE9, quick);

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
        let mut last = clock::monotonic_nanos();
        let mut frames = 0u32;
        let mut blink_acc = 0.0f32;
        let mut blink_on = true;

        while frames < frame_cap {
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

            if let Some(types::Event::CloseRequested(id)) = events::poll() {
                if id == win {
                    break;
                }
            }
        }

        // Report on exit.
        let out = stdio::stdout();
        let _ = out.write(b"nova:ok\n");
        let _ = out.write(b"score:");
        let mut buf = [0u8; 20];
        let _ = out.write(num_to_bytes(world.score, &mut buf));
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
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

bindings::export!(Component with_types_in bindings);
