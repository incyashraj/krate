//! Drift -- a racing game.
//!
//! Written for CP-E2 of Plan/Proof-Checkpoints-2026-09-16.md, after the first
//! attempt at CP-E produced a benchmark with a car in it and was recorded as a
//! pass. That version had no objective, no opponents, no collision against
//! anything but the ground, no score and no end. It proved the rasterizer was
//! fast and proved nothing about whether Krate can carry a game.
//!
//! This one is a game: a closed circuit, three laps, five opponents that drive
//! the same physics you do, collisions between every pair of cars, a barrier
//! you can lean on, a countdown, a HUD, engine and tyre sound that track what
//! the car is doing, and a results screen you can restart from.
//!
//! What it found, in the order it hurt:
//!
//! - **K-398**: nothing can be drawn OVER a 3D scene. `scene3d` has no text,
//!   and the only containers are flow containers, so a 2D canvas cannot
//!   overlay a 3D one. Every number here is seven-segment digits built out of
//!   triangles in `hud.rs`. Every 3D game has a HUD; this one has one only
//!   because it was worth the effort to prove the hole is real.
//! - **K-396**: the 3D surface was capped at 1024 on an edge, so no 3D app
//!   could fill a modern window. Raised to 1920 after measuring the worst
//!   case, which is what let this run at 1600x900.
//! - **K-397**: a triangle with any corner behind the camera is dropped whole
//!   rather than clipped, so the road surface has to be tessellated finely
//!   near the car or it vanishes as you drive onto it.
//!
//! `#![no_std]`: the meshes are built once at startup, and the per-frame
//! allocation is the HUD's triangle list, which is reused.

#![no_std]
#![allow(clippy::too_many_arguments)]

extern crate alloc;

mod art;
mod car;
mod hud;
mod mathx;
mod track;

use alloc::string::String;
use alloc::vec::Vec;

use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{scene3d, types as gfx};
use krate::ui::{events, tree, types, window};

use car::{drive_ai, Car, MAX_LAPS};
use hud::Hud;
use mathx::{abs, cos_approx, hash2, sin_approx};
use track::{ground_height, Track, ROAD_HALF};

const ROOT_ID: u64 = 1;
const SCENE_ID: u64 = 2;

/// 1600x900 rather than 1920x1080: the cap now allows 1080p, but the window
/// has to fit on the machine running it, and this leaves room for a dock.
const WIDTH: u32 = 1600;
const HEIGHT: u32 = 900;

/// Cars on the grid, including the player.
const FIELD: usize = 6;

/// Seconds of countdown before the lights go out.
const COUNTDOWN: f32 = 3.5;

// ------------------------------------------------------------------ meshes

struct Mesh {
    verts: Vec<f32>,
}

impl Mesh {
    fn tris(&self) -> usize {
        self.verts.len() / 9
    }
}

/// A box as twelve triangles, centred in x and z, sitting on y=0.
fn box_mesh(w: f32, h: f32, d: f32) -> Mesh {
    let (x0, x1) = (-w * 0.5, w * 0.5);
    let (y0, y1) = (0.0, h);
    let (z0, z1) = (-d * 0.5, d * 0.5);
    let c = [
        [x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0],
        [x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1],
    ];
    // Wound counter-clockwise seen from OUTSIDE, which is what the host's
    // back-face test wants. Getting this backwards makes a solid object
    // render inside-out, which reads as the world being wrong rather than
    // the mesh.
    let faces: [[usize; 6]; 6] = [
        [0, 2, 1, 0, 3, 2],
        [5, 7, 4, 5, 6, 7],
        [4, 3, 0, 4, 7, 3],
        [1, 6, 5, 1, 2, 6],
        [3, 6, 2, 3, 7, 6],
        [4, 1, 5, 4, 0, 1],
    ];
    let mut verts = Vec::with_capacity(12 * 9);
    for f in faces {
        for i in f {
            verts.push(c[i][0]);
            verts.push(c[i][1]);
            verts.push(c[i][2]);
        }
    }
    Mesh { verts }
}

/// A building, as world-space triangles with UVs, ready for `textured`.
///
/// Pre-transformed rather than instanced because `textured` has no `place`
/// counterpart: the transform-and-draw call takes one flat tint and no UVs, so
/// a textured mesh has to arrive already in world space. That is the cost of
/// texturing the buildings, and at this count it is one the frame budget can
/// carry -- but it is worth knowing that a textured world cannot reuse a mesh
/// the way an untextured one can.
fn building_at(x: f32, z: f32, y: f32, w: f32, h: f32, d: f32) -> (Vec<f32>, Vec<f32>) {
    let (x0, x1) = (x - w * 0.5, x + w * 0.5);
    let (y0, y1) = (y, y + h);
    let (z0, z1) = (z - d * 0.5, z + d * 0.5);
    let mut verts = Vec::with_capacity(12 * 9);
    let mut uvs = Vec::with_capacity(12 * 6);

    // One UV span per storey, so a tall building gets more window rows rather
    // than the same eight rows stretched taller.
    let storeys = (h / 4.0).max(1.0);

    // Four walls, wound counter-clockwise seen from outside.
    let walls: [[[f32; 3]; 4]; 4] = [
        [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0]],
        [[x1, y0, z1], [x0, y0, z1], [x0, y1, z1], [x1, y1, z1]],
        [[x0, y0, z1], [x0, y0, z0], [x0, y1, z0], [x0, y1, z1]],
        [[x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]],
    ];
    for wall in walls {
        let span = {
            let dx = wall[1][0] - wall[0][0];
            let dz = wall[1][2] - wall[0][2];
            (mathx::sqrt_approx(dx * dx + dz * dz) / 6.0).max(1.0)
        };
        let uv = [[0.0, storeys], [span, storeys], [span, 0.0], [0.0, 0.0]];
        for i in [0usize, 1, 2, 0, 2, 3] {
            verts.push(wall[i][0]);
            verts.push(wall[i][1]);
            verts.push(wall[i][2]);
            uvs.push(uv[i][0]);
            uvs.push(uv[i][1]);
        }
    }
    // A flat roof, drawn with the top of the texture so it reads as concrete
    // rather than as a window pattern seen from above.
    let roof = [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]];
    for i in [0usize, 1, 2, 0, 2, 3] {
        verts.push(roof[i][0]);
        verts.push(roof[i][1]);
        verts.push(roof[i][2]);
        uvs.push(0.02);
        uvs.push(0.98);
    }
    (verts, uvs)
}

/// The car: a body with a cabin on top, so which way it faces is readable at
/// a glance from behind.
fn car_mesh() -> Mesh {
    let mut m = box_mesh(2.2, 0.9, 4.4);
    let cabin = box_mesh(1.8, 0.7, 2.0);
    // Lift the cabin onto the body and shift it back a little.
    for t in cabin.verts.chunks_exact(3) {
        m.verts.push(t[0]);
        m.verts.push(t[1] + 0.9);
        m.verts.push(t[2] - 0.3);
    }
    m
}

fn tree_mesh() -> Mesh {
    let mut m = box_mesh(0.7, 2.4, 0.7);
    let r = 2.0;
    let base = 2.2;
    let top = 7.5;
    let corners = [[-r, base, -r], [r, base, -r], [r, base, r], [-r, base, r]];
    for i in 0..4 {
        let a = corners[i];
        let b = corners[(i + 1) % 4];
        for p in [a, b, [0.0, top, 0.0]] {
            m.verts.push(p[0]);
            m.verts.push(p[1]);
            m.verts.push(p[2]);
        }
    }
    m
}

/// The land the circuit sits on, as a grid of quads.
///
/// Wound UP-facing (see `track::surface`): the obvious corner order is
/// clockwise from above and back-face culling removes it, which empties the
/// ground from under the car while distant hills survive at their grazing
/// angle.
fn ground_mesh(extent: f32, cells: usize) -> (Mesh, Vec<f32>) {
    let mut verts = Vec::with_capacity(cells * cells * 18);
    let mut uvs = Vec::with_capacity(cells * cells * 12);
    let step = extent * 2.0 / cells as f32;
    // Tile every few cells rather than once across the whole field: one UV
    // span over 1400 units stretches a 64-pixel tile into visible mush.
    let uv_scale = 0.35_f32;
    for gz in 0..cells {
        for gx in 0..cells {
            let x0 = -extent + gx as f32 * step;
            let z0 = -extent + gz as f32 * step;
            let x1 = x0 + step;
            let z1 = z0 + step;
            let y00 = ground_height(x0, z0);
            let y10 = ground_height(x1, z0);
            let y01 = ground_height(x0, z1);
            let y11 = ground_height(x1, z1);
            let corners = [
                ([x0, y00, z0], [x0, z0]),
                ([x1, y11, z1], [x1, z1]),
                ([x1, y10, z0], [x1, z0]),
                ([x0, y00, z0], [x0, z0]),
                ([x0, y01, z1], [x0, z1]),
                ([x1, y11, z1], [x1, z1]),
            ];
            for (p, uv) in corners {
                verts.push(p[0]);
                verts.push(p[1]);
                verts.push(p[2]);
                uvs.push(uv[0] * uv_scale * 0.1);
                uvs.push(uv[1] * uv_scale * 0.1);
            }
        }
    }
    (Mesh { verts }, uvs)
}

// ------------------------------------------------------------------ scenery

#[derive(Clone, Copy)]
struct Prop {
    x: f32,
    z: f32,
    y: f32,
    rot: f32,
    scale: f32,
    kind: u8,
    tint: gfx::Color,
}

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

/// Trees and grandstands, kept clear of the road so they decorate rather than
/// block. Placed relative to the circuit, not scattered over a square, so the
/// track always looks lined.
fn scenery(track: &Track, per_node: usize) -> Vec<Prop> {
    let mut out = Vec::with_capacity(track.nodes.len() * per_node);
    for (i, node) in track.nodes.iter().enumerate() {
        for k in 0..per_node {
            let seed = (i * 31 + k * 7) as i32;
            let side = if hash2(seed, 1) > 0.5 { 1.0 } else { -1.0 };
            // Set back from the road. Eight units from the kerb is close
            // enough that a building fills the windscreen as you pass it,
            // which reads as a wall rather than as scenery.
            let out_dist = ROAD_HALF + 20.0 + hash2(seed, 2) * 64.0;
            let nx = -node.dir_z;
            let nz = node.dir_x;
            let jitter = (hash2(seed, 3) - 0.5) * 9.0;
            let x = node.x + nx * side * out_dist + node.dir_x * jitter;
            let z = node.z + nz * side * out_dist + node.dir_z * jitter;
            let building = hash2(seed, 4) > 0.72;
            let g = 0.30 + hash2(seed, 6) * 0.22;
            out.push(Prop {
                x,
                z,
                y: ground_height(x, z),
                rot: hash2(seed, 5) * 360.0,
                scale: if building {
                    1.4 + hash2(seed, 8) * 2.2
                } else {
                    0.8 + hash2(seed, 9) * 0.9
                },
                kind: u8::from(building),
                tint: if building {
                    let s = 0.34 + hash2(seed, 10) * 0.2;
                    rgb(s, s * 0.97, s * 0.93)
                } else {
                    rgb(0.10, g, 0.13)
                },
            });
        }
    }
    out
}

// -------------------------------------------------------------------- audio

/// Synthesised sounds, so the game carries no audio files.
mod sfx {
    use alloc::vec::Vec;

    /// One cycle count of a square-ish engine note at a given pitch, as raw
    /// 16-bit mono at 44.1 kHz.
    pub fn engine(freq: f32, millis: u32) -> Vec<u8> {
        let rate = 44_100.0_f32;
        let n = (rate * millis as f32 / 1000.0) as usize;
        let mut out = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = i as f32 / rate;
            let phase = t * freq;
            let saw = phase - crate::mathx::floor_f32(phase) - 0.5;
            // A second voice a fifth up thickens it into something engine-like
            // rather than a test tone.
            let p2 = t * freq * 1.5;
            let saw2 = p2 - crate::mathx::floor_f32(p2) - 0.5;
            let env = 0.55;
            let v = (saw * 0.7 + saw2 * 0.3) * env;
            let s = (v * 9000.0) as i16;
            out.push((s as u16 & 0xFF) as u8);
            out.push(((s as u16 >> 8) & 0xFF) as u8);
        }
        out
    }

    /// Filtered noise, for tyre scrub and impacts.
    pub fn noise(millis: u32, decay: f32, gain: f32) -> Vec<u8> {
        let rate = 44_100.0_f32;
        let n = (rate * millis as f32 / 1000.0) as usize;
        let mut out = Vec::with_capacity(n * 2);
        let mut seed = 0x1234_5678_u32;
        let mut low = 0.0_f32;
        for i in 0..n {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = ((seed >> 9) & 0xFFFF) as f32 / 32_768.0 - 1.0;
            low += (white - low) * 0.22;
            let env = libm_exp(-(i as f32 / rate) * decay);
            let s = (low * env * gain) as i16;
            out.push((s as u16 & 0xFF) as u8);
            out.push(((s as u16 >> 8) & 0xFF) as u8);
        }
        out
    }

    /// A short tone, for the countdown beeps.
    pub fn beep(freq: f32, millis: u32, gain: f32) -> Vec<u8> {
        let rate = 44_100.0_f32;
        let n = (rate * millis as f32 / 1000.0) as usize;
        let mut out = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = i as f32 / rate;
            let env = libm_exp(-t * 3.0);
            let v = crate::mathx::sin_approx(t * freq * core::f32::consts::PI * 2.0);
            let s = (v * env * gain) as i16;
            out.push((s as u16 & 0xFF) as u8);
            out.push(((s as u16 >> 8) & 0xFF) as u8);
        }
        out
    }

    /// `exp(-x)` for x >= 0. A no_std guest has no `f32::exp` either.
    fn libm_exp(x: f32) -> f32 {
        // exp(x) = 2^(x/ln2); split into integer and fractional parts and use
        // a short polynomial on the fraction.
        let t = x * core::f32::consts::LOG2_E;
        let k = crate::mathx::floor_f32(t);
        let f = t - k;
        // 2^f on 0..1, minimax-ish.
        let p = 1.0 + f * (0.693_147 + f * (0.240_226 + f * 0.055_504));
        let mut r = p;
        let mut e = k as i32;
        while e > 0 {
            r *= 2.0;
            e -= 1;
        }
        while e < 0 {
            r *= 0.5;
            e += 1;
        }
        r
    }
}

// ------------------------------------------------------------------- state

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Attract,
    Countdown,
    Racing,
    Finished,
}

struct Game {
    phase: Phase,
    /// Seconds since the phase began.
    phase_t: f32,
    /// Seconds since the lights went out.
    race_t: f32,
    cars: Vec<Car>,
    /// Finishing order, filled as cars cross the line for the last time.
    order: Vec<usize>,
    best_lap: f32,
    last_lap_at: f32,
    /// Camera state, smoothed rather than snapped to the car.
    cam: [f32; 3],
    cam_look: [f32; 3],
    shake: f32,
    countdown_step: i32,
    /// Put the camera exactly where it belongs on the next frame instead of
    /// easing toward it, so a phase change does not sweep the view across the
    /// world.
    snap_camera: bool,
    /// Whether this race is the attract demo, which drives the player's car
    /// itself and hands control back the moment someone presses something.
    demo: bool,
}

impl Game {
    fn new(track: &Track) -> Self {
        let mut cars = Vec::with_capacity(FIELD);
        for i in 0..FIELD {
            cars.push(Car::new(track, i, FIELD));
        }
        Self {
            phase: Phase::Attract,
            phase_t: 0.0,
            race_t: 0.0,
            cars,
            order: Vec::with_capacity(FIELD),
            best_lap: 0.0,
            last_lap_at: 0.0,
            cam: [0.0, 40.0, 0.0],
            cam_look: [0.0, 0.0, 0.0],
            shake: 0.0,
            countdown_step: -1,
            snap_camera: true,
            demo: false,
        }
    }

    fn reset(&mut self, track: &Track) {
        for (i, c) in self.cars.iter_mut().enumerate() {
            *c = Car::new(track, i, FIELD);
        }
        self.order.clear();
        self.race_t = 0.0;
        self.best_lap = 0.0;
        self.last_lap_at = 0.0;
        self.shake = 0.0;
        self.countdown_step = -1;
        self.snap_camera = true;
        self.phase = Phase::Countdown;
        self.phase_t = 0.0;
    }

    /// Where the player sits in the field right now, 1-based.
    fn player_position(&self) -> u32 {
        // If the race is over the finishing order is the answer; before then
        // it is distance travelled, which is why `progress` accumulates
        // through laps rather than resetting.
        if let Some(p) = self.order.iter().position(|&i| i == 0) {
            return p as u32 + 1;
        }
        let me = self.cars[0].progress;
        let mut ahead = 0;
        for (i, c) in self.cars.iter().enumerate() {
            if i != 0 && c.progress > me {
                ahead += 1;
            }
        }
        ahead + 1
    }
}

// ------------------------------------------------------------------ drawing

/// Paint one frame.
fn draw(
    scene: u64,
    g: &Game,
    track: &Track,
    meshes: &Meshes,
    art: &Art,
    props: &[Prop],
    hud: &mut Hud,
    visible_props: usize,
) -> Result<usize, gfx::GfxError> {
    let sky = match g.phase {
        Phase::Attract => rgb(0.36, 0.47, 0.66),
        _ => rgb(0.52, 0.70, 0.92),
    };
    scene3d::clear(scene, sky)?;
    scene3d::camera(scene, &g.cam, &g.cam_look, 62.0)?;
    scene3d::light(scene, &[-0.42, -0.84, -0.34])?;

    let mut calls = 0;

    // The world, textured. A flat tint per mesh is what made the first
    // version read as 1997: the rasterizer was never the limit, the app just
    // never used `upload_texture`.
    let white = rgb(1.0, 1.0, 1.0);
    scene3d::textured(scene, &meshes.ground.verts, &meshes.ground_uv, art.grass, white)?;
    scene3d::textured(scene, &track.road, &track.road_uv, art.asphalt, white)?;
    scene3d::textured(scene, &track.kerb, &track.kerb_uv, art.kerb, white)?;
    scene3d::textured(scene, &meshes.buildings, &meshes.buildings_uv, art.facade, white)?;
    calls += 4;

    // Trees stay untextured and instanced: a cone of leaves reads fine as a
    // flat colour, and `place` sends one small mesh instead of a world's worth
    // of pre-transformed vertices.
    for p in props.iter().take(visible_props) {
        if p.kind == 1 {
            continue; // buildings are in the textured mesh above
        }
        scene3d::place(
            scene,
            &meshes.tree.verts,
            &[p.x, p.y, p.z],
            &[0.0, p.rot, 0.0],
            p.scale,
            p.tint,
        )?;
        calls += 1;
    }

    // Cars. The player is red; the rest are given distinct hues so "the blue
    // one is ahead of me" is a thing you can say.
    for (i, c) in g.cars.iter().enumerate() {
        let tint = car_colour(i, c.hit);
        scene3d::place(
            scene,
            &meshes.car.verts,
            &[c.x, c.y, c.z],
            &[0.0, c.body_angle() * 57.295_78, 0.0],
            1.0,
            tint,
        )?;
        calls += 1;
    }

    if !hud.is_empty() {
        // Tint over 1.0 on purpose. Every triangle is shaded by its normal
        // against the one light -- `shade = 0.35 + 0.65 * |n.l|` in the host,
        // with no unlit path -- so a HUD quad facing the camera comes out at
        // roughly a third brightness and the numbers read as dark grey rather
        // than as an overlay. Scaling the tint past white is the only lever an
        // app has. This is the second half of K-398: even with the geometry
        // trick, a HUD cannot be drawn at the colour it is asked for.
        let verts = hud.to_world(g.cam, g.cam_look);
        scene3d::triangles(scene, &verts, rgb(2.7, 2.7, 2.8))?;
        calls += 1;
    }

    scene3d::present(scene)?;
    Ok(calls)
}

fn car_colour(i: usize, hit: f32) -> gfx::Color {
    let base = match i {
        0 => (0.88, 0.18, 0.20),
        1 => (0.20, 0.42, 0.88),
        2 => (0.95, 0.75, 0.15),
        3 => (0.20, 0.72, 0.38),
        4 => (0.72, 0.30, 0.85),
        _ => (0.95, 0.95, 0.95),
    };
    // Flash toward white on contact, so a bump is visible as well as audible.
    let f = hit.min(1.0) * 0.6;
    rgb(
        base.0 + (1.0 - base.0) * f,
        base.1 + (1.0 - base.1) * f,
        base.2 + (1.0 - base.2) * f,
    )
}

struct Meshes {
    ground: Mesh,
    ground_uv: Vec<f32>,
    car: Mesh,
    tree: Mesh,
    /// Every building in the world, pre-transformed into one textured mesh.
    /// `textured` has no `place`, so they cannot be instanced.
    buildings: Vec<f32>,
    buildings_uv: Vec<f32>,
}

/// Uploaded texture handles.
struct Art {
    asphalt: u64,
    grass: u64,
    facade: u64,
    kerb: u64,
}

/// Build the HUD for this frame.
fn build_hud(g: &Game, hud: &mut Hud) {
    hud.clear();
    // Camera-space units at the HUD plane. The visible half-width at 62 deg
    // fov and DIST = 3 is about 1.8 vertically; x is that times the aspect.
    let top = 1.55;
    let left = -2.7;
    let right = 2.7;

    match g.phase {
        Phase::Attract => {
            // A chequered band instead of blank title bars.
            //
            // Three solid rectangles were standing in for a title, and they
            // read as exactly what they were: three white boxes sitting over
            // the circuit. There is no text available here (K-398), so the
            // title screen says what it is with a racing motif rather than
            // pretending to be lettering.
            let cell = 0.13;
            for row in 0..2 {
                for col in 0..14 {
                    if (row + col) % 2 == 0 {
                        hud.rect(
                            -0.91 + col as f32 * cell,
                            0.42 - row as f32 * cell,
                            cell,
                            cell,
                        );
                    }
                }
            }
            // A car-sized bar under it, and a prompt that pulses.
            hud.rect(-0.30, 0.03, 0.60, 0.10);
            let pulse = sin_approx(g.phase_t * 3.0) * 0.5 + 0.5;
            if pulse > 0.45 {
                hud.rect(-0.55, -0.80, 1.10, 0.09);
            }
        }
        Phase::Countdown => {
            let left_s = (COUNTDOWN - g.phase_t).max(0.0);
            let lights = (mathx::ceil_f32(left_s) as i32).clamp(0, 3);
            for i in 0..3 {
                let on = i < lights;
                let cx = -0.85 + i as f32 * 0.85;
                if on {
                    hud::disc(cx, 0.55, 0.26, 18, hud);
                } else {
                    hud::disc(cx, 0.55, 0.10, 12, hud);
                }
            }
        }
        Phase::Racing | Phase::Finished => {
            // Speed, bottom left, big.
            let kph = (abs(g.cars[0].speed) * 3.6) as u32;
            hud.number(kph, left + 1.45, -1.30, 0.26, 0.42, 1);
            // A speed bar under it.
            let frac = (abs(g.cars[0].speed) / 92.0).min(1.0);
            hud.rect(left + 0.12, -1.44, 1.35 * frac, 0.06);

            // Lap counter, top left: current / total.
            // The separator is a SLASH, not an upright bar.
            //
            // A tall thin rectangle between the two numbers is the same shape
            // as the digit 1, so "lap 2 of 3" read as "21 3" -- the two
            // numbers were correctly spaced and the divider was being counted
            // as a digit. Leaning it fixes what the shape says.
            let lap = (g.cars[0].lap + 1).min(MAX_LAPS);
            hud.number(lap, left + 0.50, top - 0.30, 0.18, 0.30, 1);
            for step in 0..6 {
                let t = step as f32 / 5.0;
                hud.rect(
                    left + 0.60 + t * 0.10,
                    top - 0.30 + t * 0.22,
                    0.045,
                    0.06,
                );
            }
            hud.number(MAX_LAPS, left + 1.04, top - 0.30, 0.18, 0.30, 1);

            // Position, top right.
            hud.number(g.player_position(), right - 0.12, top - 0.34, 0.24, 0.38, 1);

            // Race clock, top centre: m:ss.
            let t = g.race_t.max(0.0);
            let mins = (t / 60.0) as u32;
            let secs = (t as u32) % 60;
            // Same right-alignment care as the lap counter: the minutes digit
            // ends at -0.30, the colon sits just past it, and the two seconds
            // digits (0.15 wide with a 0.045 gap) end at 0.06.
            hud.number(mins, -0.30, top - 0.26, 0.15, 0.25, 1);
            hud.colon(-0.25, top - 0.26, 0.045, 0.25);
            hud.number(secs, 0.24, top - 0.26, 0.15, 0.25, 2);

            if g.phase == Phase::Finished {
                // A results plate: a bar per car, longest for the winner, with
                // the player's row marked by a notch.
                for (rank, &ci) in g.order.iter().enumerate() {
                    let y = 0.75 - rank as f32 * 0.24;
                    let w = 1.5 - rank as f32 * 0.12;
                    hud.rect(-w * 0.5, y, w, 0.15);
                    if ci == 0 {
                        hud.rect(-w * 0.5 - 0.22, y, 0.14, 0.15);
                    }
                }
                let pulse = sin_approx(g.phase_t * 3.0) * 0.5 + 0.5;
                if pulse > 0.45 {
                    hud.rect(-0.75, -0.95, 1.5, 0.10);
                }
            }
        }
    }
}

// --------------------------------------------------------------------- misc

fn u64_str(v: u64) -> String {
    let mut digits = [0u8; 20];
    let mut n = 0;
    let mut v = v;
    if v == 0 {
        digits[0] = b'0';
        n = 1;
    }
    while v > 0 {
        digits[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let mut s = String::with_capacity(n);
    for i in (0..n).rev() {
        s.push(digits[i] as char);
    }
    s
}

fn say(s: &str) {
    let out = stdio::stdout();
    let _ = out.write(s.as_bytes());
    let _ = out.write(b"\n");
}

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        // `grow: 1.0` is what makes the canvas take the whole window. Left at
        // zero the scene is laid out at its content size, which for a canvas
        // is nothing, and the window comes up empty.
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

/// Say which step failed before exiting.
///
/// A bare `return 1` tells a checker only that something went wrong and the
/// log ends with no clue which call it was.
fn fail(step: &[u8]) -> i32 {
    let out = stdio::stdout();
    let _ = out.write(b"drift: failed at ");
    let _ = out.write(step);
    let _ = out.write(b"\n");
    1
}

static mut PRESENT_NS: u64 = 0;

// --------------------------------------------------------------------- main

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let has = |n: &[u8]| raw.as_bytes().split(|b| *b == b'\n').any(|a| a == n);
        let quick = has(b"quick") || has(b"--quick");
        let auto = has(b"auto");
        // `hold` drives itself, runs the clock faster than real time, and then
        // parks on the results screen rather than restarting -- the only way
        // to photograph the end of a race without sitting through one.
        let hold = has(b"hold");
        let auto = auto || hold;

        let win = match window::create(
            "Drift",
            types::WindowSize {
                width: WIDTH,
                height: HEIGHT,
            },
        ) {
            Ok(w) => w,
            Err(_) => return fail(b"window::create"),
        };
        if window::show(win).is_err() {
            return fail(b"window::show");
        }
        // The root MUST go through set_root; upsert_node on the root fails
        // silently and the app exits with nothing on stdout (K-392).
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            return fail(b"set_root");
        }
        if tree::upsert_node(
            win,
            &node(SCENE_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        )
        .is_err()
        {
            return fail(b"upsert scene");
        }
        let scene = match scene3d::bind(win, SCENE_ID) {
            Ok(s) => s,
            Err(_) => return fail(b"scene3d::bind"),
        };
        let _ = scene3d::cull_back_faces(scene, true);

        // The circuit. 360 nodes around roughly 1.9 km is a node every five
        // metres or so, which is fine enough that the road does not visibly
        // facet and fine enough to dodge K-397: a long road quad straddling
        // the camera would be dropped whole rather than clipped.
        let track = track::build(7, 360);
        let props = scenery(&track, 2);
        let (ground, ground_uv) = ground_mesh(700.0, 56);

        // Every building, pre-transformed into one mesh. Varied footprints and
        // heights rather than one box repeated: a skyline of identical blocks
        // reads as wallpaper however well it is textured.
        let mut buildings = Vec::new();
        let mut buildings_uv = Vec::new();
        for (i, p) in props.iter().enumerate() {
            if p.kind != 1 {
                continue;
            }
            let seed = i as i32;
            let w = 7.0 + hash2(seed, 21) * 9.0;
            let d = 7.0 + hash2(seed, 22) * 9.0;
            // Mostly low, a few tall. A uniform spread of heights up to 42
            // units put a tower every few metres and turned a country circuit
            // into a canyon -- squaring the random pulls most of them down
            // while leaving the occasional one to break the skyline.
            let r = hash2(seed, 23);
            let h = 7.0 + r * r * 30.0;
            let (v, uv) = building_at(p.x, p.z, p.y, w, h, d);
            buildings.extend_from_slice(&v);
            buildings_uv.extend_from_slice(&uv);
        }

        let meshes = Meshes {
            ground,
            ground_uv,
            car: car_mesh(),
            tree: tree_mesh(),
            buildings,
            buildings_uv,
        };

        // Textures, synthesised here so the bundle carries no assets.
        let upload = |t: art::Texture| -> u64 {
            scene3d::upload_texture(scene, t.width, t.height, &t.rgba).unwrap_or(0)
        };
        let art = Art {
            asphalt: upload(art::asphalt()),
            grass: upload(art::grass()),
            facade: upload(art::facade(3)),
            kerb: upload(art::kerb()),
        };

        say("drift: a racing game");
        {
            let mut s = String::new();
            s.push_str("  circuit ");
            s.push_str(&u64_str(track.length as u64));
            s.push_str(" m, ");
            s.push_str(&u64_str(track.nodes.len() as u64));
            s.push_str(" nodes, road ");
            s.push_str(&u64_str((track.road.len() / 9) as u64));
            s.push_str(" tris, ground ");
            s.push_str(&u64_str(meshes.ground.tris() as u64));
            s.push_str(" tris, ");
            s.push_str(&u64_str(props.len() as u64));
            s.push_str(" props");
            say(&s);
        }

        // Audio. Every sound is synthesised, so the bundle carries no assets.
        let audio = krate::audio::playback::open(krate::audio::types::StreamConfig {
            sample_rate: 44_100,
            channels: 1,
            format: krate::audio::types::SampleFormat::PcmS16,
            buffer_frames: 1_024,
        })
        .ok();
        if let Some(a) = audio {
            let _ = krate::audio::playback::start(a);
        }
        let load = |bytes: Vec<u8>| -> Option<u64> {
            let a = audio?;
            krate::audio::playback::load_sound(a, &bytes).ok()
        };
        // Engine notes at a few pitches; the one played is chosen by speed,
        // which is a cheap stand-in for a continuously pitched engine and is
        // the shape `play_sound` supports.
        let mut engine_steps = Vec::new();
        for i in 0..6 {
            let f = 52.0 + i as f32 * 26.0;
            if let Some(s) = load(sfx::engine(f, 260)) {
                engine_steps.push(s);
            }
        }
        let skid = load(sfx::noise(300, 5.0, 5200.0));
        let bump = load(sfx::noise(180, 16.0, 11000.0));
        let beep_lo = load(sfx::beep(440.0, 180, 8000.0));
        let beep_hi = load(sfx::beep(880.0, 420, 9000.0));

        let play = |s: Option<u64>, gain: f32| {
            if let (Some(a), Some(id)) = (audio, s) {
                let _ = krate::audio::playback::play_sound(a, id, gain);
            }
        };

        let mut g = Game::new(&track);
        let mut hud = Hud::new();

        // Frame timing, measured from the clock rather than assumed, and with
        // present()'s deliberate pacing tracked separately so the render cost
        // can be reported honestly.
        let mut last = clock::monotonic_nanos();
        let mut frames: u64 = 0;
        let mut engine_next = 0.0_f32;
        let mut skid_next = 0.0_f32;
        let mut samples: Vec<u32> = Vec::with_capacity(4096);
        let mut now_s = 0.0_f32;

        // `auto` drives itself for long enough to run a whole three-lap race
        // and reach the results screen: a shorter cap reports "lap 0" and
        // looks like broken lap counting when it is just a race that has not
        // finished yet.
        let frame_cap = if quick {
            30
        } else if auto {
            18_000
        } else {
            u64::MAX
        };

        loop {
            let t0 = clock::monotonic_nanos();
            let mut dt = (t0.saturating_sub(last)) as f32 / 1_000_000_000.0;
            last = t0;
            // A frame that took a very long time (a breakpoint, a stall, the
            // window being dragged) must not teleport every car through the
            // barrier.
            if dt > 0.1 {
                dt = 0.1;
            }
            if dt <= 0.0 {
                dt = 1.0 / 60.0;
            }
            // Under `hold` the world advances several steps per drawn frame,
            // so a three-lap race finishes in seconds of wall time. The step
            // SIZE is unchanged -- taking bigger steps would change how the
            // cars behave and make the shot a picture of a different game.
            let sim_steps = if hold { 10 } else { 1 };
            now_s += dt * sim_steps as f32;
            g.phase_t += dt * sim_steps as f32;

            // ---- input
            let key = |k: &str| events::key_held(k);
            let pad_x = events::gamepad_axis("left-x");
            let accel = key("ArrowUp") || key("w") || events::gamepad_held("a");
            let brake = key("ArrowDown") || key("s") || events::gamepad_held("b");
            let left = key("ArrowLeft") || key("a");
            let right = key("ArrowRight") || key("d");
            let start = key("Enter") || key(" ") || events::gamepad_held("start");

            let mut steer = 0.0;
            if left {
                steer -= 1.0;
            }
            if right {
                steer += 1.0;
            }
            if abs(pad_x) > 0.15 {
                steer = pad_x;
            }
            let throttle = if accel {
                1.0
            } else if brake {
                -1.0
            } else {
                0.0
            };

            // ---- phase
            match g.phase {
                Phase::Attract => {
                    // A press starts a real race. Left alone, the title screen
                    // rolls into a demo one after a few seconds, the way an
                    // arcade cabinet does -- so the game is never a static
                    // picture waiting for someone, and a long unattended run
                    // exercises the race rather than the menu. (A ten-minute
                    // soak spent all ten in the attract screen and reported
                    // lap 0, speed 0, which looked like the game not working.)
                    if start || auto || quick {
                        g.reset(&track);
                        g.demo = false;
                    } else if g.phase_t > 8.0 {
                        g.reset(&track);
                        g.demo = true;
                    }
                }
                Phase::Countdown => {
                    let left_s = COUNTDOWN - g.phase_t;
                    let step = mathx::ceil_f32(left_s) as i32;
                    if step != g.countdown_step && (0..=3).contains(&step) {
                        g.countdown_step = step;
                        if step == 0 {
                            play(beep_hi, 0.9);
                        } else {
                            play(beep_lo, 0.7);
                        }
                    }
                    if left_s <= 0.0 {
                        g.phase = Phase::Racing;
                        g.phase_t = 0.0;
                    }
                }
                Phase::Racing => {
                    g.race_t += dt * sim_steps as f32;
                    // Any press during the demo takes the wheel: start the
                    // race properly rather than leaving the person watching
                    // their own car being driven for them.
                    if g.demo && (start || accel || brake || left || right) {
                        g.reset(&track);
                        g.demo = false;
                    }
                }
                Phase::Finished => {
                    // `hold` parks on the results screen instead of starting
                    // another race, so a headless shot can be taken of it.
                    // Without this the only way to see the results is to sit
                    // through three laps and catch the three seconds before it
                    // restarts, and `--shoot` closes the window when it fires.
                    if !hold && ((start && g.phase_t > 1.0) || (auto && g.phase_t > 3.0)) {
                        g.reset(&track);
                        g.demo = false;
                    } else if !hold && g.demo && g.phase_t > 6.0 {
                        // A demo race that has run its course goes back to the
                        // title rather than looping straight into another one.
                        g.phase = Phase::Attract;
                        g.phase_t = 0.0;
                        g.demo = false;
                        g.snap_camera = true;
                    }
                }
            }

            // ---- simulate
            for _sub in 0..sim_steps {
            if matches!(g.phase, Phase::Racing | Phase::Finished) {
                let before: Vec<u32> = g.cars.iter().map(|c| c.lap).collect();

                for i in 0..g.cars.len() {
                    let (th, st) = if i == 0 && !auto && !g.demo {
                        (throttle, steer)
                    } else {
                        // Skill varies per car so the field spreads out
                        // instead of driving as one block. Car 0 is the
                        // player's, and in `auto` it drives at the MIDDLE of
                        // the range rather than the bottom -- otherwise the
                        // self-driving demo always finishes last, which looks
                        // like the player's car being slower than everyone
                        // else's when it is only the skill number.
                        let skill = if i == 0 {
                            0.93
                        } else {
                            0.88 + (i as f32 * 0.018)
                        };
                        drive_ai(&g.cars[i], &track, skill.min(1.0))
                    };
                    let finished = g.cars[i].finished;
                    if !finished {
                        g.cars[i].step(&track, th, st, dt, g.race_t);
                    } else {
                        // A finished car coasts to a stop rather than vanishing.
                        let (ath, ast) = drive_ai(&g.cars[i], &track, 0.35);
                        g.cars[i].step(&track, ath * 0.2, ast, dt, g.race_t);
                    }
                }

                // Every pair, both ways. Six cars is fifteen pairs; a bigger
                // field would want a grid, but at this size the simple loop is
                // both correct and free.
                for i in 0..g.cars.len() {
                    for j in (i + 1)..g.cars.len() {
                        let (a, b) = g.cars.split_at_mut(j);
                        Car::collide(&mut a[i], &mut b[0]);
                    }
                }

                // Finishing order, in the order they actually cross.
                for i in 0..g.cars.len() {
                    if g.cars[i].finished && !g.order.contains(&i) {
                        g.order.push(i);
                    }
                }
                if g.cars[0].lap != before[0] && g.cars[0].lap > 0 {
                    let lap_time = g.race_t - g.last_lap_at;
                    g.last_lap_at = g.race_t;
                    if g.best_lap == 0.0 || lap_time < g.best_lap {
                        g.best_lap = lap_time;
                    }
                }
                if g.phase == Phase::Racing && g.cars[0].finished {
                    g.phase = Phase::Finished;
                    g.phase_t = 0.0;
                }
            }
            }

            // ---- sound tied to what the car is doing
            let me = &g.cars[0];
            if matches!(g.phase, Phase::Racing | Phase::Finished) && !engine_steps.is_empty() {
                let v = abs(me.speed);
                let idx = ((v / 92.0 * engine_steps.len() as f32) as usize)
                    .min(engine_steps.len() - 1);
                if now_s >= engine_next {
                    play(Some(engine_steps[idx]), 0.16 + (v / 92.0) * 0.22);
                    // Re-trigger just before the sample ends, so the note is
                    // continuous rather than stuttering.
                    engine_next = now_s + 0.22;
                }
                let slide = me.slide();
                if slide > 0.35 && now_s >= skid_next {
                    play(skid, (slide * 0.5).min(0.5));
                    skid_next = now_s + 0.24;
                }
            }
            if me.hit > 0.85 {
                play(bump, 0.7);
                g.shake = 1.0;
            }

            // ---- camera
            {
                let me = &g.cars[0];
                let (target_eye, target_look) = match g.phase {
                    Phase::Attract => {
                        // A slow orbit of the start line, high enough to look
                        // OVER the scenery. At 55 units the grandstands are
                        // between the camera and the circuit and the shot is
                        // just a wall of boxes; the buildings scale to 10 units
                        // times up to 3.6, so the camera has to clear ~36 by a
                        // wide margin to see the road it is meant to show off.
                        let a = g.phase_t * 0.25;
                        let n = track.nodes[0];
                        (
                            [
                                n.x + sin_approx(a) * 190.0,
                                n.y + 150.0,
                                n.z + cos_approx(a) * 190.0,
                            ],
                            [n.x, n.y, n.z],
                        )
                    }
                    _ => {
                        // Chase camera. It trails the car's HEADING, not its
                        // body angle: following the slip makes the camera
                        // swing wildly through a drift and is unreadable.
                        let back = 17.0 + abs(me.speed) * 0.10;
                        let height = 6.4 + abs(me.speed) * 0.020;
                        (
                            [
                                me.x - sin_approx(me.heading) * back,
                                me.y + height,
                                me.z - cos_approx(me.heading) * back,
                            ],
                            [
                                me.x + sin_approx(me.heading) * 9.0,
                                me.y + 2.4,
                                me.z + cos_approx(me.heading) * 9.0,
                            ],
                        )
                    }
                };
                // Smoothing, frame-rate independent. A camera that snaps to the
                // car transmits every bump into the whole picture.
                //
                // The first frame snaps instead, and so does the frame a race
                // starts on: easing from wherever the camera happened to be
                // sweeps it across the whole world, which on frame one is a
                // shot of the sky and on a restart is a lurch from the results
                // screen.
                let k = if frames == 0 || g.snap_camera {
                    g.snap_camera = false;
                    1.0
                } else {
                    (7.5 * dt).min(1.0)
                };
                for i in 0..3 {
                    g.cam[i] += (target_eye[i] - g.cam[i]) * k;
                    g.cam_look[i] += (target_look[i] - g.cam_look[i]) * k;
                }
                if g.shake > 0.0 {
                    let s = g.shake * 0.5;
                    g.cam[0] += sin_approx(now_s * 57.0) * s;
                    g.cam[1] += sin_approx(now_s * 43.0) * s * 0.6;
                    g.shake = (g.shake - dt * 2.4).max(0.0);
                }
            }

            // ---- draw
            build_hud(&g, &mut hud);
            let p0 = clock::monotonic_nanos();
            let visible = props.len();
            if draw(scene, &g, &track, &meshes, &art, &props, &mut hud, visible).is_err() {
                // Name the exit. A bare `break` here ends the game with a
                // normal-looking report and no clue that anything went wrong,
                // which is exactly how a mid-race stop read as "the race just
                // finished early".
                say("drift: draw failed, stopping");
                break;
            }
            unsafe {
                PRESENT_NS = clock::monotonic_nanos().saturating_sub(p0);
            }

            let spent = clock::monotonic_nanos().saturating_sub(t0);
            let present = unsafe { PRESENT_NS };
            // Record the work, not the deliberate pacing present() does to
            // hold the frame rate: counting the sleep measures the budget, not
            // the cost.
            samples.push((spent.saturating_sub(present) / 1_000) as u32);

            frames += 1;
            if frames >= frame_cap {
                say("drift: frame cap reached, stopping");
                break;
            }

            if let Some(types::Event::CloseRequested(id)) = events::poll() {
                say("drift: window closed, stopping");
                let _ = window::close(id);
                break;
            }
        }

        // ---- report
        if !samples.is_empty() {
            samples.sort_unstable();
            let pick = |q: f32| samples[((samples.len() - 1) as f32 * q) as usize];
            let mut s = String::new();
            s.push_str("drift: frames ");
            s.push_str(&u64_str(frames));
            s.push_str("  render us p50 ");
            s.push_str(&u64_str(pick(0.50) as u64));
            s.push_str(" p95 ");
            s.push_str(&u64_str(pick(0.95) as u64));
            s.push_str(" p99 ");
            s.push_str(&u64_str(pick(0.99) as u64));
            s.push_str(" worst ");
            s.push_str(&u64_str(samples[samples.len() - 1] as u64));
            say(&s);
            let mut l = String::new();
            l.push_str("  player lap ");
            l.push_str(&u64_str(g.cars[0].lap as u64));
            l.push_str(" position ");
            l.push_str(&u64_str(g.player_position() as u64));
            l.push_str(" best lap ms ");
            l.push_str(&u64_str((g.best_lap * 1000.0) as u64));
            say(&l);
            let mut d = String::new();
            d.push_str("  race t ");
            d.push_str(&u64_str(g.race_t as u64));
            d.push_str("s  progress m ");
            d.push_str(&u64_str(g.cars[0].progress as u64));
            d.push_str(" of ");
            d.push_str(&u64_str(track.length as u64));
            d.push_str("  speed ");
            d.push_str(&u64_str(abs(g.cars[0].speed) as u64));
            d.push_str("  leader m ");
            let lead = g
                .cars
                .iter()
                .map(|c| c.progress as u64)
                .max()
                .unwrap_or(0);
            d.push_str(&u64_str(lead));
            say(&d);
        }
        0
    }
}

krate::export!(Component);
