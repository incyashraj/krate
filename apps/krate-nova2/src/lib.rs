//! Krate Nova 2 -- a textured-sprite space shooter.
//!
//! The second hero: where Nova 1 proves "a real game in 13 KB", this proves the
//! ceiling is high -- metallic ships with baked lighting, painted nebula, real
//! sprites that rotate to face their heading, on all three systems from one
//! sandboxed file. The art is real RGBA loaded from the bundle; the runtime
//! draws it rotated and alpha-blended with the new draw_sprite primitive.
//!
//! no_std, fixed-capacity entity pools, assets read as raw RGBA (no in-guest
//! decoder), so the whole thing imports only krate:*.

#![no_std]

extern crate alloc;
extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::vec::Vec;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::resources::assets;
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const WIDTH: f32 = 540.0;
const HEIGHT: f32 = 720.0;

const MAX_ENEMIES: usize = 24;
const MAX_BULLETS: usize = 48;

const QUICK_FRAMES: u32 = 80;
const MAX_FRAMES: u32 = 6000;

struct Component;

/// A decoded raw-RGBA asset: dimensions plus the pixel bytes.
struct Sprite {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

impl Sprite {
    fn missing() -> Self {
        Self {
            w: 0,
            h: 0,
            rgba: Vec::new(),
        }
    }
    fn ok(&self) -> bool {
        self.w > 0 && self.h > 0
    }
}

/// Read a raw-RGBA asset (8-byte LE w,h header + w*h*4 bytes). No decoding.
fn load(path: &str) -> Sprite {
    let Ok(bytes) = assets::read(path) else {
        return Sprite::missing();
    };
    if bytes.len() < 8 {
        return Sprite::missing();
    }
    let w = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let h = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let need = (w as usize) * (h as usize) * 4;
    match bytes.get(8..8 + need) {
        Some(body) => Sprite {
            w,
            h,
            rgba: body.to_vec(),
        },
        None => Sprite::missing(),
    }
}

#[derive(Clone, Copy)]
struct Enemy {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    angle: f32,
    kind: u8, // 0 = enemy_a, 1 = enemy_b
    alive: bool,
}

#[derive(Clone, Copy)]
struct Bullet {
    x: f32,
    y: f32,
    alive: bool,
}

struct Art {
    nebula: Sprite,
    player: Sprite,
    enemy_a: Sprite,
    enemy_b: Sprite,
    projectile: Sprite,
}

struct World {
    px: f32,
    py: f32,
    p_angle: f32,
    score: u32,
    lives: i32,
    enemies: [Enemy; MAX_ENEMIES],
    bullets: [Bullet; MAX_BULLETS],
    fire_cd: f32,
    spawn_cd: f32,
    // Two scroll offsets so the nebula parallax-scrolls.
    scroll: f32,
    rng: u32,
    elapsed: f32,
}

impl World {
    fn new() -> Self {
        Self {
            px: WIDTH * 0.5,
            py: HEIGHT - 90.0,
            p_angle: 0.0,
            score: 0,
            lives: 3,
            enemies: [Enemy {
                x: 0.0,
                y: 0.0,
                vx: 0.0,
                vy: 0.0,
                angle: 0.0,
                kind: 0,
                alive: false,
            }; MAX_ENEMIES],
            bullets: [Bullet {
                x: 0.0,
                y: 0.0,
                alive: false,
            }; MAX_BULLETS],
            fire_cd: 0.0,
            spawn_cd: 0.6,
            scroll: 0.0,
            rng: 0x1357_9bdf,
            elapsed: 0.0,
        }
    }

    fn rand(&mut self) -> f32 {
        // xorshift32 -> 0..1
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x & 0x00FF_FFFF) as f32 / 16_777_216.0
    }

    fn spawn_enemy(&mut self) {
        // Draw the randoms first so we do not borrow self mutably twice (once
        // for rand, once for the enemy slot).
        let r_kind = self.rand();
        let r_x = self.rand();
        let r_vx = self.rand();
        let r_vy = self.rand();
        let elapsed = self.elapsed;
        for e in self.enemies.iter_mut() {
            if !e.alive {
                e.x = 40.0 + r_x * (WIDTH - 80.0);
                e.y = -50.0;
                e.vx = (r_vx - 0.5) * 40.0;
                e.vy = 55.0 + r_vy * 45.0 + elapsed * 1.5;
                // Enemies point DOWN (toward the player): angle = pi. A little
                // wobble is added per frame from vx so they bank as they drift.
                e.angle = 3.14159;
                e.kind = if r_kind < 0.5 { 0 } else { 1 };
                e.alive = true;
                return;
            }
        }
    }

    fn fire(&mut self) {
        for b in self.bullets.iter_mut() {
            if !b.alive {
                b.x = self.px;
                b.y = self.py - 32.0;
                b.alive = true;
                return;
            }
        }
    }
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        let out = stdio::stdout();
        let art = Art {
            nebula: load("nebula.rgba"),
            player: load("ship_player.rgba"),
            enemy_a: load("ship_enemy_a.rgba"),
            enemy_b: load("ship_enemy_b.rgba"),
            projectile: load("projectile.rgba"),
        };
        if !art.player.ok() || !art.nebula.ok() {
            let _ = out.write(b"assets:missing\n");
            return 40;
        }
        let _ = out.write(b"assets:loaded\n");

        let Ok(win) = window::create(
            "Nova 2",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) else {
            return 30;
        };
        let _ = window::show(win);
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err()
            || tree::upsert_node(win, &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas))
                .is_err()
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
        // A real session ends when the person closes the window, never on a
        // count (K-092). `quick` keeps its bound so headless checks cannot
        // hang.
        let frame_cap = if quick { QUICK_FRAMES } else { u32::MAX };

        let mut w = World::new();

        // Quick mode: stage a lively frame -- some enemies already descending,
        // a few bullets in flight -- so the first captured shot shows action.
        if quick {
            let mut i = 0;
            while i < 5 {
                w.spawn_enemy();
                // scatter them down the field
                if let Some(e) = w.enemies.get_mut(i) {
                    e.y = 80.0 + i as f32 * 90.0;
                }
                i += 1;
            }
            w.fire();
            w.py = HEIGHT - 120.0;
        }

        let started = clock::monotonic_nanos();
        let mut last = started;
        let mut frames = 0u32;

        while frames < frame_cap {
            let now = clock::monotonic_nanos();
            let dt = ((now.saturating_sub(last)) as f32 / 1_000_000_000.0).min(0.05);
            last = now;
            w.elapsed += dt;

            update(&mut w, dt);
            if draw(canvas, &w, &art).is_err() {
                break;
            }
            frames += 1;

            if let Some(types::Event::CloseRequested(id)) = events::poll() {
                if id == win {
                    break;
                }
            }
            if w.lives <= 0 {
                break;
            }
        }

        let _ = out.write(b"nova2:ok\n");
        let _ = out.write(b"score:");
        let _ = out.write(u32_bytes(w.score, &mut [0u8; 12]));
        let _ = out.write(b"\n");
        let _ = window::close(win);
        0
    }
}

fn update(w: &mut World, dt: f32) {
    w.scroll += 24.0 * dt;

    // Input: arrows / WASD move the ship; the ship banks toward its motion.
    let mut mvx = 0.0;
    let mut mvy = 0.0;
    if events::key_held("ArrowLeft") || events::key_held("a") {
        mvx -= 1.0;
    }
    if events::key_held("ArrowRight") || events::key_held("d") {
        mvx += 1.0;
    }
    if events::key_held("ArrowUp") || events::key_held("w") {
        mvy -= 1.0;
    }
    if events::key_held("ArrowDown") || events::key_held("s") {
        mvy += 1.0;
    }
    let speed = 320.0;
    w.px = clampf(w.px + mvx * speed * dt, 30.0, WIDTH - 30.0);
    w.py = clampf(w.py + mvy * speed * dt, HEIGHT * 0.4, HEIGHT - 40.0);
    // Bank: tilt toward horizontal motion for life.
    w.p_angle = mvx * 0.35;

    // Auto-fire on a cooldown (and space held).
    w.fire_cd -= dt;
    let firing = events::key_held("Space") || true; // always fire: it's a shmup
    if firing && w.fire_cd <= 0.0 {
        w.fire();
        w.fire_cd = 0.18;
    }

    // Bullets.
    for b in w.bullets.iter_mut() {
        if b.alive {
            b.y -= 640.0 * dt;
            if b.y < -20.0 {
                b.alive = false;
            }
        }
    }

    // Spawn enemies over time.
    w.spawn_cd -= dt;
    if w.spawn_cd <= 0.0 {
        w.spawn_enemy();
        w.spawn_cd = (0.9 - w.elapsed * 0.02).max(0.35);
    }

    // Enemies move; bank with their vx; leave the field at the bottom.
    let mut hits: u32 = 0;
    for e in w.enemies.iter_mut() {
        if !e.alive {
            continue;
        }
        e.x += e.vx * dt;
        e.y += e.vy * dt;
        e.angle = 3.14159 + (e.vx * 0.004);
        if e.y > HEIGHT + 60.0 {
            e.alive = false;
            continue;
        }
        // Reached the player row: cost a life (once), then gone.
        if e.y > w.py - 24.0 && e.y < w.py + 24.0 && (e.x - w.px).abs() < 34.0 {
            e.alive = false;
            w.lives -= 1;
        }
    }

    // Bullet vs enemy collisions.
    for b in w.bullets.iter_mut() {
        if !b.alive {
            continue;
        }
        for e in w.enemies.iter_mut() {
            if e.alive && (e.x - b.x).abs() < 30.0 && (e.y - b.y).abs() < 30.0 {
                e.alive = false;
                b.alive = false;
                hits += 1;
                break;
            }
        }
    }
    w.score += hits * 100;
}

fn draw(canvas: u64, w: &World, art: &Art) -> Result<(), gfx::GfxError> {
    // Nebula background, scrolled vertically for a sense of motion. Draw it
    // twice stacked so the scroll wraps seamlessly.
    let off = (w.scroll % HEIGHT) - HEIGHT;
    sprite(canvas, &art.nebula, WIDTH * 0.5, off + HEIGHT * 0.5, WIDTH, HEIGHT, 0.0)?;
    sprite(
        canvas,
        &art.nebula,
        WIDTH * 0.5,
        off + HEIGHT * 1.5,
        WIDTH,
        HEIGHT,
        0.0,
    )?;

    // Enemies (rotated to their heading).
    for e in w.enemies.iter() {
        if !e.alive {
            continue;
        }
        let s = if e.kind == 0 {
            &art.enemy_a
        } else {
            &art.enemy_b
        };
        sprite(canvas, s, e.x, e.y, 62.0, 62.0, e.angle)?;
    }

    // Bullets.
    for b in w.bullets.iter() {
        if b.alive {
            sprite(canvas, &art.projectile, b.x, b.y, 16.0, 32.0, 0.0)?;
        }
    }

    // Player ship, banking.
    sprite(canvas, &art.player, w.px, w.py, 66.0, 66.0, w.p_angle)?;

    // HUD.
    draw_hud(canvas, w)?;

    canvas2d::present(canvas)
}

/// Draw a sprite centred at (cx,cy) scaled to (dw,dh), rotated by angle.
fn sprite(
    canvas: u64,
    s: &Sprite,
    cx: f32,
    cy: f32,
    dw: f32,
    dh: f32,
    angle: f32,
) -> Result<(), gfx::GfxError> {
    if !s.ok() {
        return Ok(());
    }
    canvas2d::draw_sprite(
        canvas,
        gfx::Point { x: cx, y: cy },
        gfx::Size {
            width: dw,
            height: dh,
        },
        angle,
        s.w,
        s.h,
        &s.rgba,
    )
}

fn draw_hud(canvas: u64, w: &World) -> Result<(), gfx::GfxError> {
    // Score, big, top-left.
    let mut buf = [0u8; 12];
    let s = u32_bytes(w.score, &mut buf);
    if let Ok(txt) = core::str::from_utf8(s) {
        canvas2d::draw_text(
            canvas,
            txt,
            gfx::Point { x: 18.0, y: 40.0 },
            30.0,
            color(0.9, 0.97, 1.0, 1.0),
        )?;
    }
    canvas2d::draw_text(
        canvas,
        "SCORE",
        gfx::Point { x: 18.0, y: 56.0 },
        12.0,
        color(0.55, 0.7, 0.9, 1.0),
    )?;
    // Lives as small cyan bars top-right.
    let mut i = 0i32;
    while i < w.lives {
        let x = WIDTH - 30.0 - (i as f32) * 22.0;
        canvas2d::fill_rect(
            canvas,
            gfx::Rect {
                x: x - 7.0,
                y: 24.0,
                width: 14.0,
                height: 8.0,
            },
            color(0.2, 0.95, 1.0, 1.0),
        )?;
        i += 1;
    }
    Ok(())
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

fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

fn u32_bytes(value: u32, buf: &mut [u8; 12]) -> &[u8] {
    if value == 0 {
        if let Some(s) = buf.get_mut(0) {
            *s = b'0';
        }
        return buf.get(..1).unwrap_or(b"0");
    }
    let mut scratch = [0u8; 12];
    let mut n = value;
    let mut count = 0usize;
    while n > 0 && count < scratch.len() {
        if let Some(s) = scratch.get_mut(count) {
            *s = b'0' + (n % 10) as u8;
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
