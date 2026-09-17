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
mod models;
mod text_hud;
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
use text_hud::Ui;
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

/// Where the sun sits, as a fraction around the compass.
///
/// 0.12 rather than 0.62: at 0.62 the sun sat behind the camera, so the sky
/// showed only its cool half and none of the glow the texture spends most of
/// its range on. A sunset the player cannot see is a colour grade, not a
/// sunset.
const SUN_BEARING: f32 = 0.12;

/// Rows on the front menu: RACE, LAPS, RIVALS, QUIT.
const MENU_ROWS: usize = 4;

// ------------------------------------------------------------------ meshes

struct Mesh {
    verts: Vec<f32>,
}

impl Mesh {
    fn tris(&self) -> usize {
        self.verts.len() / 9
    }
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
            // Buildings CLUSTER, trees scatter.
            //
            // Each was independently thrown somewhere in a 64-unit band, so
            // every tower stood alone on open grass -- which is what made them
            // read as slabs rather than as a city. Quantising the distance
            // into a few rows puts them shoulder to shoulder in blocks, with
            // the gaps between rows reading as streets.
            let is_building = hash2(seed, 4) > 0.72;
            let out_dist = if is_building {
                // Set well back. At 30 units the first row filled the
                // windscreen and the circuit read as a trench; a city is
                // something you race PAST, so the near row starts beyond the
                // trees and the rows recede from there.
                let row = (hash2(seed, 2) * 4.0) as i32;
                ROAD_HALF + 85.0 + row as f32 * 34.0 + hash2(seed, 12) * 8.0
            } else {
                ROAD_HALF + 14.0 + hash2(seed, 2) * 62.0
            };
            let nx = -node.dir_z;
            let nz = node.dir_x;
            let jitter = (hash2(seed, 3) - 0.5) * 9.0;
            let x = node.x + nx * side * out_dist + node.dir_x * jitter;
            let z = node.z + nz * side * out_dist + node.dir_z * jitter;
            let building = is_building;
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
                    // NOT USED for buildings: they are drawn as one
                    // pre-transformed mesh with a single tint at the draw
                    // call, so this value goes nowhere. Kept because `Prop`
                    // is shared with the trees, which do use it.
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
    /// The race, stopped. The scene still renders; nothing advances.
    ///
    /// A separate phase rather than a flag on Racing, because every place
    /// that asks "are we racing" would otherwise have to remember to ask "and
    /// not paused" -- and the one that forgot would be the race clock, which
    /// is exactly the thing a pause has to stop.
    Paused,
    Finished,
}

/// What the menu can change, carried into the race.
///
/// A struct rather than loose fields because these travel together: a race is
/// started FROM a settings block, and the header reads the same block back.
/// Two of them drifting apart is how a game ends up saying "LAP 1 / 3" in a
/// five-lap race.
#[derive(Clone, Copy)]
struct Settings {
    laps: u32,
    /// Cars on the grid INCLUDING the player, so 1 rival is a field of 2.
    field: usize,
}

impl Settings {
    const LAP_CHOICES: [u32; 3] = [3, 5, 10];
    const MIN_FIELD: usize = 2;
    const MAX_FIELD: usize = 6;

    fn default() -> Self {
        Self {
            laps: MAX_LAPS,
            field: FIELD,
        }
    }

    fn cycle_laps(&mut self) {
        let next = Self::LAP_CHOICES
            .iter()
            .position(|&l| l == self.laps)
            .map(|i| (i + 1) % Self::LAP_CHOICES.len())
            .unwrap_or(0);
        self.laps = Self::LAP_CHOICES[next];
    }

    fn cycle_field(&mut self) {
        self.field = if self.field >= Self::MAX_FIELD {
            Self::MIN_FIELD
        } else {
            self.field + 1
        };
    }
}

struct Game {
    phase: Phase,
    /// The race's settings, fixed when it starts.
    settings: Settings,
    /// Which menu row the keyboard is on.
    menu_row: usize,
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
            settings: Settings::default(),
            menu_row: 0,
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
        // The grid is sized by the settings, and every car is told how long
        // the race is. `cars` keeps its full capacity and the extras are
        // simply not part of this race -- rebuilding the vector would mean
        // reallocating six cars every restart for nothing.
        let field = self
            .settings
            .field
            .clamp(Settings::MIN_FIELD, Settings::MAX_FIELD);
        self.cars.truncate(field);
        while self.cars.len() < field {
            self.cars.push(Car::new(track, self.cars.len(), field));
        }
        for (i, c) in self.cars.iter_mut().enumerate() {
            *c = Car::new(track, i, field);
            c.race_laps = self.settings.laps;
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
    // The clear colour is the sky's horizon tone, not a blue.
    //
    // It is only seen where the dome does not reach, so it must be the colour
    // the dome's edge is or the seam shows as a band. Everything else in the
    // frame is graded against this.
    scene3d::clear(scene, rgb(0.92, 0.62, 0.44))?;
    scene3d::camera(scene, &g.cam, &g.cam_look, 62.0)?;

    // The sun comes FROM its bearing in the sky, low down.
    //
    // A light direction points the way the light TRAVELS, so this is the
    // negation of the direction toward the sun. Low -- 0.28 down against 1.0
    // across -- because a low sun is what gives long shadows and a lit side
    // and a shaded side, and that is most of what makes a scene look lit
    // rather than merely visible.
    let sun_a = SUN_BEARING * core::f32::consts::TAU;
    let (sun_x, sun_z) = (cos_approx(sun_a), sin_approx(sun_a));
    scene3d::light(scene, &[-sun_x, -0.28, -sun_z])?;


    let mut calls = 0;

    // The world, textured. A flat tint per mesh is what made the first
    // version read as 1997: the rasterizer was never the limit, the app just
    // never used `upload_texture`.
    let white = rgb(1.0, 1.0, 1.0);

    // The sky dome first, unlit and centred on the camera so it never moves
    // relative to the driver -- a sky that parallaxes is a sky with a size,
    // and the sky has no size.
    if art.sky != 0 {
        let mut dome = Vec::with_capacity(meshes.sky.verts.len());
        for p in meshes.sky.verts.chunks_exact(3) {
            dome.push(p[0] + g.cam[0]);
            dome.push(p[1]);
            dome.push(p[2] + g.cam[2]);
        }
        scene3d::unlit(scene, &dome, &meshes.sky_uv, art.sky, white)?;
    }
    scene3d::textured(scene, &meshes.ground.verts, &meshes.ground_uv, art.grass, white)?;
    // The road wears a normal map when the renderer has one, so the chippings
    // are bumps the light reacts to rather than speckle painted on a flat
    // sheet. Falls back to the plain textured path otherwise.
    // The road is tinted DOWN at the draw call, not darkened in its texture.
    //
    // The texture has to be light enough to survive the lighting -- at a true
    // asphalt albedo the road clipped to pure black over 15% of the frame.
    // Lifting it fixed that and turned the road into concrete. The tint is
    // where the two are reconciled: a bright texture carrying its detail,
    // multiplied down to the tone asphalt actually is, with the detail
    // surviving the multiply because it was never crushed in the first place.
    let tarmac = rgb(0.52, 0.52, 0.56);
    if art.road_normals != 0 {
        scene3d::normal_mapped(
            scene,
            &track.road,
            &track.road_normals,
            &track.road_tangents,
            &track.road_uv,
            art.asphalt,
            art.road_normals,
            tarmac,
        )?;
    } else {
        scene3d::textured(scene, &track.road, &track.road_uv, art.asphalt, tarmac)?;
    }
    // Painted markings, over the asphalt and under everything else.
    //
    // Drawn after the road so they land on top of it, and they carry alpha --
    // the clear parts of the texture leave the asphalt showing, which is what
    // makes a dashed line a dashed line rather than a line with grey gaps.
    if art.markings != 0 {
        scene3d::textured(scene, &track.paint, &track.paint_uv, art.markings, white)?;
        calls += 1;
    }
    scene3d::textured(scene, &track.kerb, &track.kerb_uv, art.kerb, white)?;
    // Tinted DOWN, not drawn white.
    //
    // The buildings are one pre-transformed mesh, so they take one tint here
    // -- the per-prop tint computed in `scenery` is never used for them, which
    // is worth knowing because editing it changes nothing at all. (That cost a
    // pass: the facades were darkened there and came out identical.)
    //
    // Dark, because a concrete wall at dusk is in shade: the sun is at the
    // horizon and only faces turned to it catch any. Near-white towers left
    // the lit windows drawn over them with nothing to stand out against --
    // the emissive pass measured +1.9 per channel, which is invisible. A
    // window only reads as lit if the wall around it is dark.
    scene3d::textured(
        scene,
        &meshes.buildings,
        &meshes.buildings_uv,
        art.facade,
        rgb(0.30, 0.29, 0.34),
    )?;
    // A second pass over the SAME geometry, drawing only the lit windows and
    // drawing them UNLIT.
    //
    // This is what a city at dusk actually looks like: the concrete takes the
    // sunset's shading and goes blue, and the windows do not -- they are their
    // own light source and hold their brightness. Without it a skyline at this
    // hour is a row of grey slabs, however good the wall texture is.
    if art.facade_lights != 0 {
        scene3d::unlit(
            scene,
            &meshes.buildings,
            &meshes.buildings_uv,
            art.facade_lights,
            white,
        )?;
        calls += 1;
    }
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

    // Guard rails down both sides, every few nodes.
    //
    // This is the cheapest thing in the scene and it does the most for SPEED.
    // A car at 230 km/h over open grass looks like it is barely moving,
    // because nothing near it changes fast; a regular row of posts flicking
    // past is what an eye actually measures velocity against.
    //
    // Every third node -- about every eleven metres, matching the rail span,
    // so the sections meet end to end and read as a continuous barrier.
    let rail_tint = rgb(0.62, 0.60, 0.58);
    let bush_tint = rgb(0.22, 0.26, 0.13);
    for (i, node) in track.nodes.iter().enumerate().step_by(3) {
        let nx = -node.dir_z;
        let nz = node.dir_x;
        // The heading the rail runs along, so a section lies WITH the road
        // rather than across it.
        let deg = mathx::atan2_approx(node.dir_x, node.dir_z) * 57.295_78;
        for side in [-1.0f32, 1.0] {
            let off = ROAD_HALF + 3.4;
            let (rx, rz) = (node.x + nx * side * off, node.z + nz * side * off);
            scene3d::place(
                scene,
                &meshes.rail.verts,
                &[rx, ground_height(rx, rz), rz],
                &[0.0, deg, 0.0],
                1.0,
                rail_tint,
            )?;
            calls += 1;
        }
        // A bush behind the rail every so often, to break the run-off up.
        if i % 9 == 0 {
            let off = ROAD_HALF + 8.0 + hash2(i as i32, 77) * 7.0;
            let side = if hash2(i as i32, 78) > 0.5 { 1.0 } else { -1.0 };
            let (bx, bz) = (node.x + nx * side * off, node.z + nz * side * off);
            scene3d::place(
                scene,
                &meshes.bush.verts,
                &[bx, ground_height(bx, bz), bz],
                &[0.0, hash2(i as i32, 79) * 360.0, 0.0],
                0.8 + hash2(i as i32, 80) * 0.7,
                bush_tint,
            )?;
            calls += 1;
        }
    }

    // Cars. The player is red; the rest are given distinct hues so "the blue
    // one is ahead of me" is a thing you can say.
    for (i, c) in g.cars.iter().enumerate() {
        let tint = car_colour(i, c.hit);
        let heading = c.body_angle();
        let deg = heading * 57.295_78;
        scene3d::place(
            scene,
            &meshes.car.verts,
            &[c.x, c.y, c.z],
            &[0.0, deg, 0.0],
            1.0,
            tint,
        )?;
        calls += 1;

        // Glass, in its own dark blue-grey rather than the car's paint.
        //
        // On the body mesh the windows were part of the shell and took the
        // paint with them, so every car had opaque coloured panels where its
        // windows should be. That is most of why they read as tinted solids.
        scene3d::place(
            scene,
            &meshes.glass.verts,
            &[c.x, c.y, c.z],
            &[0.0, deg, 0.0],
            1.0,
            rgb(0.10, 0.13, 0.18),
        )?;
        calls += 1;

        // Lights, UNLIT so they hold their brightness whichever way the car
        // faces. A headlight shaded like paint is a white rectangle that dims
        // when the car turns away from the sun; one that glows reads as
        // something switched on, which at a sunset is the difference between
        // a car and a model of a car.
        if art.hud_white != 0 {
            let head_uv = hud_uvs(meshes.head.verts.len());
            let tail_uv = hud_uvs(meshes.tail.verts.len());
            let place_unlit = |verts: &[f32], uvs: &[f32], tint: gfx::Color| {
                let (sh, ch) = (sin_approx(heading), cos_approx(heading));
                let mut world = Vec::with_capacity(verts.len());
                for p in verts.chunks_exact(3) {
                    world.push(c.x + p[0] * ch + p[2] * sh);
                    world.push(c.y + p[1]);
                    world.push(c.z - p[0] * sh + p[2] * ch);
                }
                scene3d::unlit(scene, &world, uvs, art.hud_white, tint)
            };
            place_unlit(
                &meshes.head.verts,
                &head_uv,
                rgb(1.00, 0.96, 0.82),
            )?;
            place_unlit(&meshes.tail.verts, &tail_uv, rgb(0.92, 0.14, 0.10))?;
            calls += 2;
        }

        // Four wheels, placed in world space at the corners of the body.
        //
        // Each is rotated into the car's heading by hand rather than being
        // part of the body mesh, because a wheel that turns with the steering
        // and a body that does not is most of what makes a car look driven
        // rather than slid. `place` takes a position and an angle, so the
        // corner offset has to be rotated here.
        let (sh, ch) = (sin_approx(heading), cos_approx(heading));
        const CORNERS: [[f32; 3]; 4] = [
            [-0.98, 0.36, 1.42],
            [0.98, 0.36, 1.42],
            [-0.98, 0.36, -1.38],
            [0.98, 0.36, -1.38],
        ];
        let tyre = rgb(0.07, 0.07, 0.08);
        for (n, off) in CORNERS.iter().enumerate() {
            // Rotate the local offset into world space.
            let wx = c.x + off[0] * ch + off[2] * sh;
            let wz = c.z - off[0] * sh + off[2] * ch;
            // The front pair steers; the rear pair does not.
            // The front pair turns with the slip angle -- the car has no
            // separate steering state, and slip is what the driver is doing
            // about the corner, which is close enough to read right.
            let steer = if n < 2 { -c.slip * 34.0 } else { 0.0 };
            scene3d::place(
                scene,
                &meshes.wheel.verts,
                &[wx, c.y + off[1], wz],
                &[0.0, deg + steer, 0.0],
                1.0,
                tyre,
            )?;
            calls += 1;
        }
    }

    if !hud.is_empty() {
        // The HUD through the UNLIT path where there is one: an overlay is not
        // lit by the scene's sun, is not behind its air, and is not part of
        // the image a tone curve grades. Before this it was all three, and the
        // white numbers came out grey the moment tone mapping went on.
        let verts = hud.to_world(g.cam, g.cam_look);
        if art.hud_white != 0 {
            scene3d::unlit(scene, &verts, &hud_uvs(verts.len()), art.hud_white, white)?;
            calls += 1;
        } else {
            // The fallback: tint over 1.0 to climb out of the shading
            // floor, which is the only lever an app has on a renderer with no
            // unlit path. It is why K-398 recorded that a HUD cannot be drawn
            // at the colour it is asked for -- true until `unlit` existed.
            scene3d::triangles(scene, &verts, rgb(2.7, 2.7, 2.8))?;
            calls += 1;
        }
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
    /// One wheel, placed four times per car.
    wheel: Mesh,
    /// The sky dome and its UVs, drawn unlit before anything else.
    sky: Mesh,
    sky_uv: Vec<f32>,
    /// One guard-rail section, placed down both sides of the circuit.
    rail: Mesh,
    /// A roadside bush.
    bush: Mesh,
    /// The car's windows, drawn separately from its paint.
    glass: Mesh,
    /// Headlights and tail lights, drawn unlit so they glow.
    head: Mesh,
    tail: Mesh,
}

/// Uploaded texture handles.
struct Art {
    asphalt: u64,
    /// A single white texel, so the HUD can go through the textured `unlit`
    /// path. `unlit` needs a texture because every GPU draw samples one; a
    /// white one multiplies the tint by 1 and changes nothing.
    hud_white: u64,
    /// The road's normal map, or 0 when this renderer has none.
    road_normals: u64,
    grass: u64,
    facade: u64,
    kerb: u64,
    /// The sunset gradient the sky dome is painted with.
    sky: u64,
    /// Painted road markings: dashed centre, solid lane edges.
    markings: u64,
    /// The lit windows alone, drawn unlit over the facades.
    facade_lights: u64,
}

/// Build the HUD for this frame.
fn build_hud(g: &Game, hud: &mut Hud) {
    hud.clear();
    // Camera-space units at the HUD plane. The visible half-width at 62 deg
    // fov and DIST = 3 is about 1.8 vertically; x is that times the aspect.

    let left = -2.7;

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
        Phase::Racing | Phase::Paused | Phase::Finished => {
            // Speed, bottom left, big.
            let kph = (abs(g.cars[0].speed) * 3.6) as u32;
            hud.number(kph, left + 1.45, -1.30, 0.26, 0.42, 1);
            // A speed bar under it.
            let frac = (abs(g.cars[0].speed) / 92.0).min(1.0);
            hud.rect(left + 0.12, -1.44, 1.35 * frac, 0.06);

            // The lap, position and clock are TEXT now, in the header bar.
            //
            // They were seven-segment digits here, welded out of triangles,
            // because there was no way to put text over a 3D scene at all.
            // The overlay widget removed that limit, and a readout that can
            // say "LAP 1 / 3" beats one that says "1 3" with a leaning slash
            // between -- that slash was read as the digit 1 until it was
            // tilted, which is the kind of problem shapes have and words do
            // not.
            //
            // What stays below is what shapes still do better: the speed bar,
            // which moves with the frame, and the results plate.

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

/// What the 2D interface says this frame.
///
/// The header carries the readouts a driver glances at -- lap, position,
/// clock -- in fixed places. The centre carries the one thing that matters
/// right now, big. The foot carries a prompt when there is one.
///
/// Everything here is a WORD or a NUMBER WITH A LABEL. The polygon HUD keeps
/// what shapes do better: the speed bar, the countdown lights.
fn build_text(g: &Game, win: u64, ui: &mut Ui) {
    let mut lap = String::new();
    let mut pos = String::new();
    let mut clock = String::new();
    let mut a = String::new();
    let mut b = String::new();
    let mut foot = String::new();

    match g.phase {
        Phase::Attract => {
            ui.header(win, ["", "", ""]);
            a.push_str("DRIFT");
            // The menu. Four rows, the selected one marked.
            //
            // Marked with a leading caret rather than by colour alone: the
            // rows sit over a moving 3D scene, so a colour difference can land
            // on a background that erases it, and a shape cannot.
            let mut rows: Vec<String> = Vec::with_capacity(4);
            for row in 0..MENU_ROWS {
                let mut line = String::new();
                line.push_str(if row == g.menu_row { "> " } else { "  " });
                match row {
                    0 => line.push_str("RACE"),
                    1 => {
                        line.push_str("LAPS        ");
                        text_hud::push_u32(g.settings.laps, &mut line);
                    }
                    2 => {
                        line.push_str("RIVALS      ");
                        text_hud::push_u32(g.settings.field as u32 - 1, &mut line);
                    }
                    _ => line.push_str("QUIT"),
                }
                rows.push(line);
            }
            let mut lines: Vec<(&str, bool)> = Vec::with_capacity(rows.len() + 1);
            lines.push((&a, false));
            for (i, row) in rows.iter().enumerate() {
                lines.push((row.as_str(), i == g.menu_row));
            }
            ui.centre(win, &lines);
            if sin_approx(g.phase_t * 3.0) > -0.1 {
                foot.push_str("UP/DOWN choose   ENTER select");
            }
            ui.foot(win, &foot);
        }
        Phase::Countdown => {
            header(g, &mut lap, &mut pos, &mut clock);
            ui.header(win, [&lap, &pos, &clock]);
            // One number at a time, big. A countdown is the most tense moment
            // in a race and it deserves the centre of the screen.
            let left = (COUNTDOWN - g.phase_t).max(0.0);
            let n = mathx::ceil_f32(left) as i32;
            if n >= 1 {
                text_hud::push_u32(n.min(3) as u32, &mut a);
            } else {
                a.push_str("GO");
            }
            ui.centre(win, &[(&a, n < 1)]);
            ui.foot(win, "");
        }
        Phase::Racing => {
            header(g, &mut lap, &mut pos, &mut clock);
            ui.header(win, [&lap, &pos, &clock]);
            // The last lap is the one piece of race state a driver acts on,
            // and the seven-segment counter cannot emphasise anything.
            if g.cars[0].lap + 1 >= g.settings.laps {
                a.push_str("FINAL LAP");
                ui.centre(win, &[(&a, true)]);
            } else {
                ui.centre(win, &[]);
            }
            if g.best_lap > 0.0 {
                foot.push_str("BEST ");
                text_hud::push_time(g.best_lap, &mut foot);
            }
            ui.foot(win, &foot);
        }
        Phase::Paused => {
            // The header keeps showing where the race stood, so a pause is
            // somewhere to look rather than a screen that hides the state you
            // paused to think about.
            header(g, &mut lap, &mut pos, &mut clock);
            ui.header(win, [&lap, &pos, &clock]);
            a.push_str("PAUSED");
            b.push_str("ESC  resume");
            let mut c = String::new();
            c.push_str("R  restart");
            let mut d = String::new();
            d.push_str("Q  quit to title");
            ui.centre(win, &[(&a, true), (&b, false), (&c, false), (&d, false)]);
            ui.foot(win, "");
        }
        Phase::Finished => {
            header(g, &mut lap, &mut pos, &mut clock);
            ui.header(win, [&lap, &pos, &clock]);
            a.push_str("FINISHED ");
            text_hud::ordinal(g.player_position(), &mut a);
            // The results table, as a table: position, who, and the gap to
            // the winner. The painted bars behind it show the spread at a
            // glance; these say what the spread IS.
            let winner_t = g
                .order
                .first()
                .map(|&ci| g.cars[ci].finish_time)
                .unwrap_or(0.0);
            let mut rows: Vec<String> = Vec::with_capacity(g.order.len());
            for (rank, &ci) in g.order.iter().enumerate() {
                let mut row = String::new();
                text_hud::ordinal(rank as u32 + 1, &mut row);
                row.push_str(if ci == 0 { "  YOU    " } else { "  RIVAL  " });
                let gap = g.cars[ci].finish_time - winner_t;
                if rank == 0 {
                    text_hud::push_time(g.cars[ci].finish_time, &mut row);
                } else if gap > 0.0 {
                    row.push('+');
                    text_hud::push_time(gap, &mut row);
                }
                rows.push(row);
            }
            let mut lines: Vec<(&str, bool)> = Vec::with_capacity(rows.len() + 1);
            lines.push((&a, true));
            for (i, row) in rows.iter().enumerate() {
                lines.push((row.as_str(), i == 0));
            }
            ui.centre(win, &lines);
            if sin_approx(g.phase_t * 3.0) > -0.1 {
                foot.push_str("Press ENTER to race again");
            }
            ui.foot(win, &foot);
        }
    }
}

/// The three header readouts, each labelled.
///
/// Labelled because a bare number is a puzzle: "2" could be the lap, the
/// position or the seconds. Two words cost nothing and remove the question.
fn header(g: &Game, lap: &mut String, pos: &mut String, clock: &mut String) {
    lap.push_str("LAP ");
    text_hud::push_u32((g.cars[0].lap + 1).min(g.settings.laps), lap);
    lap.push_str(" / ");
    text_hud::push_u32(g.settings.laps, lap);

    pos.push_str("POS ");
    text_hud::push_u32(g.player_position(), pos);
    pos.push_str(" / ");
    text_hud::push_u32(g.cars.len() as u32, pos);

    clock.push_str("TIME ");
    text_hud::push_time(g.race_t.max(0.0), clock);
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
            // The scene canvas and the overlay root carry no label, so no ink.
            text: None,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// One UV per HUD vertex, all pointing at the middle of the white texel.
///
/// `unlit` takes UVs because every GPU draw samples a texture; for a HUD the
/// texture is a single white pixel and the UV is a formality.
fn hud_uvs(vert_floats: usize) -> Vec<f32> {
    let corners = vert_floats / 3;
    let mut uvs = Vec::with_capacity(corners * 2);
    for _ in 0..corners {
        uvs.push(0.5);
        uvs.push(0.5);
    }
    uvs
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
        // `pausecheck` proves the pause actually stops the clock, headlessly.
        //
        // It pauses at four seconds of race time, holds for two hundred
        // frames, then reports the clock before and after. A pause that let
        // the clock run would show the difference, and nothing else in the
        // game can: a screenshot of a paused race looks identical either way.
        let pausecheck = has(b"pausecheck");
        // `menucheck` drives the menu headlessly: step to LAPS, cycle it to 5,
        // go back to RACE, start, and report what the header says.
        //
        // The check from the plan is "start a 5-lap race from the menu and see
        // LAP 1 OF 5". A screenshot proves the rows are DRAWN and proves
        // nothing about whether choosing one reaches the race, which is the
        // only part that can silently not work.
        let menucheck = has(b"menucheck");
        let mut menu_step = 0u32;
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
        // The root is an OVERLAY, not a Stack.
        //
        // A Stack gives each child a share of the window, so the scene canvas
        // and the text panel each got half of it -- the race squashed into the
        // top and the words floating over empty space below. An Overlay gives
        // every child the whole window, which is what puts the text ON the
        // race. That widget is why this file can show a word at all; before it
        // every number here was digits welded out of triangles.
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Overlay)).is_err() {
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
        // The text layer, over the scene. Optional on purpose: if the host
        // refuses the nodes the race still runs with the polygon HUD, which is
        // how this game looked before there was an overlay to put text in.
        let mut words = Ui::new(win, ROOT_ID);
        if words.is_some() {
            say("drift: real text over the scene (overlay widget)");
        } else {
            say("drift: no text layer; the polygon HUD is on its own");
        }
        let _ = scene3d::cull_back_faces(scene, true);

        // Ask for everything beyond one flat light. Refused on a machine with
        // no GPU, where the game still plays -- it just looks the way it did
        // before, which is what makes this worth asking for rather than
        // requiring.
        let rich = scene3d::set_lighting(
            scene,
            &scene3d::Lighting {
                // Lower than the 0.35 default because the fill light now does
                // that work, and a lower floor is what lets a shadow read as
                // shade rather than as a slightly darker grey.
                // Low, because the fill light does this work now and a low
                // floor is what lets a shadow read as shade.
                // Raised from 0.14. A low sun plus a dark albedo left the
                // road and the shaded hillsides with no detail at all -- 15%
                // of the frame measured as pure black. Ambient is what a
                // surface still reflects when nothing is shining on it, and
                // at 0.14 the answer was "nothing".
                // 0.22 rather than 0.30: enough that shade keeps its detail,
                // not so much that the whole picture flattens toward grey.
                ambient: 0.22,
                // Cars are painted metal and the road is wet-looking asphalt;
                // both want a highlight, and it is most of what stops a car
                // reading as a coloured box.
                // Tighter and weaker. At 0.45/40 the sun's highlight spread
                // across whole hillsides as a blown-out white smear rather
                // than a glint -- a specular lobe that wide is not a
                // reflection, it is a wash. A tighter shininess puts the
                // highlight where a highlight belongs: on the cars.
                specular: 0.30,
                shininess: 96.0,
                // The circuit is two kilometres round and the camera sees
                // maybe four hundred units of it. This puts air between the
                // near kerb and the far treeline.
                // Heavier fog, in the sky's own horizon colour.
                //
                // Fog is what joins the world to the sky. When it was a pale
                // blue and the sky was a different pale blue, distant geometry
                // faded toward a colour the sky did not have and the horizon
                // showed as a seam. Matching it to the sunset's horizon tone
                // makes the far treeline dissolve INTO the sky, which is the
                // single cheapest depth cue there is.
                // Back to 0.0030 was too much: with the sun's glow added to
                // it, ground only a few metres to the left washed out to near
                // white. Fog that reaches the foreground is not depth, it is
                // a veil over the whole picture.
                fog_density: 0.0016,
                fog_color: rgb(0.96, 0.64, 0.44),
                // A cool fill from the opposite side of the sky.
                //
                // Warm key, cool fill is the oldest lighting arrangement there
                // is and the reason a sunset scene reads as three-dimensional:
                // the lit side goes orange, the shaded side goes blue, and the
                // difference between them describes the form. A single white
                // light leaves the shaded side simply darker, which describes
                // nothing.
                // Derived from the sun, not hardcoded.
                //
                // This was a fixed vector chosen when the sun was at bearing
                // 0.62. Moving the sun to 0.12 left the fill pointing roughly
                // AT it, so instead of opposing the key light it doubled up
                // on the same side -- which is what blew the left of the frame
                // out to a white smear. A fill light has one job, to come from
                // the other side, so it has to be computed from wherever the
                // key is.
                fill_direction: {
                    let a = SUN_BEARING * core::f32::consts::TAU;
                    alloc::vec![cos_approx(a), 0.62, sin_approx(a)]
                },
                fill_color: rgb(0.24, 0.30, 0.56),
                // Around the car rather than the whole circuit: at 120 units
                // the map's texels are about 12 cm, which holds an edge at the
                // distance a chase camera sees.
                shadow_radius: 120.0,
                shadow_softness: 1.2,
                // Tone mapping rather than a hard clamp, so the sunlit side of
                // a car keeps its shape instead of becoming a white patch.
                // A touch over 1: the sun is low and the lit faces should
                // bloom slightly past white rather than clip flat.
                // Back to 1.1. At 1.25 the sunlit ground near the glow rolled
                // straight past white and took the detail with it.
                exposure: 1.1,
            },
        )
        .is_ok();
        if rich {
            say("drift: specular, fog, a fill light and cast shadows are on");
        } else {
            say("drift: this renderer has one flat light; racing without");
        }

        // The circuit. 360 nodes around roughly 1.9 km is a node every five
        // metres or so, which is fine enough that the road does not visibly
        // facet and fine enough to dodge K-397: a long road quad straddling
        // the camera would be dropped whole rather than clipped.
        let track = track::build(7, 360);
        let props = scenery(&track, 2);
        // 128 cells rather than 56: at 25 units a cell the hills beyond the
        // circuit were faceted into visible plates. 11 units holds a ridge
        // line, for 33k triangles against a scene already drawing 59k.
        let (ground, ground_uv) = ground_mesh(700.0, 128);
        // Inside the ground's 700-unit extent, so the horizon is land meeting
        // sky rather than the dome's bottom edge.
        let (sky_build, sky_uv) = models::sky_dome(640.0, 300.0);
        let sky_mesh = Mesh {
            verts: sky_build.verts,
        };
        let (head_build, tail_build) = models::car_lights();
        let head_verts = head_build.verts;
        let tail_verts = tail_build.verts;

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
            // Lower, and only occasionally tall. The squared random already
            // pulled most down; with the city pushed back it can afford a
            // wider spread without becoming a canyon.
            let r = hash2(seed, 23);
            let h = 6.0 + r * r * 26.0;
            // Sink the base below the terrain under its own footprint.
            //
            // A building is an axis-aligned box and the land is not flat, so
            // setting its base to the height at its CENTRE leaves the downhill
            // corners hanging in the air -- which reads as a tower leaning,
            // not as a gap. Taking the lowest of the four corners and going a
            // little below that buries the base instead. Nobody sees the part
            // underground, and every building stands up.
            // Apply the prop's own scale. It was being computed and thrown
            // away: every building came out the same 7-16 units across
            // whatever `scale` said, which is a second reason they all read
            // alike.
            let (w, d) = (w * p.scale * 0.55, d * p.scale * 0.55);
            let h = h * (0.7 + p.scale * 0.35);
            let (hw, hd) = (w * 0.5, d * 0.5);
            let base = ground_height(p.x - hw, p.z - hd)
                .min(ground_height(p.x + hw, p.z - hd))
                .min(ground_height(p.x - hw, p.z + hd))
                .min(ground_height(p.x + hw, p.z + hd))
                - 1.5;
            let (v, uv) = building_at(p.x, p.z, base, w, h + (p.y - base), d);
            buildings.extend_from_slice(&v);
            buildings_uv.extend_from_slice(&uv);
        }

        let meshes = Meshes {
            ground,
            ground_uv,
            car: Mesh {
                verts: models::car().verts,
            },
            wheel: Mesh {
                verts: models::wheel(0.36, 0.26).verts,
            },
            tree: Mesh {
                verts: models::conifer(7).verts,
            },
            sky: sky_mesh,
            sky_uv,
            rail: Mesh {
                // Span MUST equal the placement spacing below (every 3rd node
                // of 360 around 2052m = 17.1m) or the sections leave gaps.
                verts: models::guard_rail(17.1).verts,
            },
            bush: Mesh {
                verts: models::bush(1.5).verts,
            },
            glass: Mesh {
                verts: models::car_glass().verts,
            },
            head: Mesh { verts: head_verts },
            tail: Mesh { verts: tail_verts },
            buildings,
            buildings_uv,
        };

        // Textures, synthesised here so the bundle carries no assets.
        let upload = |t: art::Texture| -> u64 {
            scene3d::upload_texture(scene, t.width, t.height, &t.rgba).unwrap_or(0)
        };
        let art = Art {
            asphalt: upload(art::asphalt()),
            // Only worth uploading when the renderer can use it. On the
            // software path this stays 0 and the road draws plain.
            road_normals: if rich {
                upload(art::asphalt_normals())
            } else {
                0
            },
            hud_white: if rich {
                scene3d::upload_texture(scene, 1, 1, &[255, 255, 255, 255]).unwrap_or(0)
            } else {
                0
            },
            grass: upload(art::grass()),
            facade: upload(art::facade(3)),
            // Same seed as the facade above, or the lit squares land where
            // that texture's DARK windows are.
            facade_lights: upload(art::facade_lights(3)),
            kerb: upload(art::kerb()),
            markings: upload(art::markings()),
            sky: upload(art::sky(SUN_BEARING)),
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
        // Last frame's state for the edge-triggered keys.
        let mut paused_at = 0.0_f32;
        let mut pause_frames = 0u32;
        let mut up_was = false;
        let mut down_was = false;
        let mut start_was = false;
        let mut esc_was = false;
        let mut restart_was = false;
        let mut quit_was = false;
        let mut guest_samples: Vec<u32> = Vec::with_capacity(4096);
        let mut wall_samples: Vec<u32> = Vec::with_capacity(4096);
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

            // Pause keys are read as EDGES, not as held state.
            //
            // `key_held` is level-triggered: it stays true for as long as the
            // key is down, which at sixty frames a second would toggle the
            // pause sixty times a press and land wherever the release
            // happened. Remembering last frame's state and acting on the
            // change is what makes one press one action.
            let esc_now = key("Escape") || key("p") || events::gamepad_held("start-alt");
            let esc_pressed = esc_now && !esc_was;
            esc_was = esc_now;
            let restart_now = key("r");
            let restart_pressed = restart_now && !restart_was;
            restart_was = restart_now;
            let quit_now = key("q");
            let quit_pressed = quit_now && !quit_was;
            quit_was = quit_now;
            // The menu's keys are edges too, and they SHARE the steering keys.
            //
            // Up and down are the same physical keys as accelerate and brake.
            // On a menu they must step one row per press; in a race they must
            // be held. That is why these exist alongside `accel` and `brake`
            // rather than replacing them -- the same key means two things
            // depending on the phase, which is ordinary for a game and needs
            // both readings to be available at once.
            let up_now = key("ArrowUp") || key("w");
            let up_pressed = up_now && !up_was;
            up_was = up_now;
            let down_now = key("ArrowDown") || key("s");
            let down_pressed = down_now && !down_was;
            down_was = down_now;
            let start_pressed = start && !start_was;
            start_was = start;

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
                    // The scripted walk: one action every ten frames, so each
                    // lands on its own frame the way a press would.
                    if menucheck {
                        menu_step += 1;
                        match menu_step {
                            10 => g.menu_row = 1,
                            // ONE cycle: 3 -> 5. Cycling twice lands on 10,
                            // which is what the first version of this did
                            // while asserting 5 -- a working menu reporting
                            // FAIL because the script miscounted.
                            20 => g.settings.cycle_laps(),
                            40 => {
                                let mut m = String::new();
                                m.push_str("drift: menucheck laps now ");
                                text_hud::push_u32(g.settings.laps, &mut m);
                                say(&m);
                                g.menu_row = 0;
                            }
                            50 => {
                                g.reset(&track);
                                g.demo = false;
                            }
                            _ => {}
                        }
                    }
                    if up_pressed {
                        g.menu_row = (g.menu_row + MENU_ROWS - 1) % MENU_ROWS;
                        g.phase_t = 0.0;
                    }
                    if down_pressed {
                        g.menu_row = (g.menu_row + 1) % MENU_ROWS;
                        g.phase_t = 0.0;
                    }
                    if start_pressed {
                        match g.menu_row {
                            0 => {
                                g.reset(&track);
                                g.demo = false;
                            }
                            1 => {
                                g.settings.cycle_laps();
                                g.phase_t = 0.0;
                            }
                            2 => {
                                g.settings.cycle_field();
                                g.phase_t = 0.0;
                            }
                            _ => {
                                say("drift: quit from the menu");
                                let _ = window::close(win);
                                break;
                            }
                        }
                    } else if auto || quick {
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
                    if menucheck && g.race_t > 0.5 {
                        let mut lap = String::new();
                        let mut pos = String::new();
                        let mut clock = String::new();
                        header(&g, &mut lap, &mut pos, &mut clock);
                        let mut m = String::new();
                        m.push_str("drift: menucheck header says  ");
                        m.push_str(&lap);
                        m.push_str("   ");
                        m.push_str(&pos);
                        say(&m);
                        say(if g.settings.laps == 5 {
                            "drift: menucheck PASS -- the menu's choice reached the race"
                        } else {
                            "drift: menucheck FAIL -- the race ignored the menu"
                        });
                        break;
                    }
                    if pausecheck && g.race_t > 4.0 {
                        say("drift: pausecheck pausing");
                        let mut m = String::new();
                        m.push_str("  race_t before ");
                        text_hud::push_time(g.race_t, &mut m);
                        say(&m);
                        paused_at = g.race_t;
                        g.phase = Phase::Paused;
                        g.phase_t = 0.0;
                        pause_frames = 0;
                    }
                    if matches!(g.phase, Phase::Paused) {
                        // Already paused this frame by the check above; fall
                        // through without touching the clock, exactly as a
                        // real pause does.
                        continue;
                    }
                    // Pause BEFORE advancing the clock, not after.
                    //
                    // The other way round the clock takes one more step on the
                    // frame the pause is pressed -- measured at 16,438us, which
                    // is exactly one frame at 60Hz. Invisible on a display
                    // showing tenths, and wrong: a pause should stop time at
                    // the moment it is pressed, not a frame later.
                    if esc_pressed {
                        g.phase = Phase::Paused;
                        g.phase_t = 0.0;
                    } else {
                        // The clock advances only if this frame did not just
                        // pause. `continue` was tried here and is wrong: it
                        // skips the rest of the frame including the draw, so
                        // the pause would land on a frame that never rendered.
                        g.race_t += dt * sim_steps as f32;
                    }
                    // Any press during the demo takes the wheel: start the
                    // race properly rather than leaving the person watching
                    // their own car being driven for them.
                    if g.demo && (start || accel || brake || left || right) {
                        g.reset(&track);
                        g.demo = false;
                    }
                }
                Phase::Paused => {
                    // Nothing advances. `race_t` is not touched here, which is
                    // the whole point: a pause that lets the clock run is a
                    // pause in name only.
                    if pausecheck {
                        pause_frames += 1;
                        if pause_frames >= 200 {
                            let mut m = String::new();
                            m.push_str("  race_t after 200 paused frames ");
                            text_hud::push_time(g.race_t, &mut m);
                            m.push_str("  drift micros ");
                            text_hud::push_u32(
                                (abs(g.race_t - paused_at) * 1_000_000.0) as u32,
                                &mut m,
                            );
                            say(&m);
                            // The game's own abs, not f32::abs -- a no_std guest has no
                            // f32::abs, and the whole reason this game has a
                            // mathx module is that the missing float methods
                            // are a trap that has now cost three checkpoints.
                            say(if abs(g.race_t - paused_at) < 0.0005 {
                                "drift: pausecheck PASS -- the clock did not move"
                            } else {
                                "drift: pausecheck FAIL -- the clock advanced"
                            });
                            break;
                        }
                    }
                    if esc_pressed {
                        g.phase = Phase::Racing;
                        g.phase_t = 0.0;
                    } else if restart_pressed {
                        g.reset(&track);
                        g.demo = false;
                    } else if quit_pressed {
                        g.phase = Phase::Attract;
                        g.phase_t = 0.0;
                        g.demo = false;
                        g.snap_camera = true;
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
            if let Some(w) = words.as_mut() {
                build_text(&g, win, w);
            }
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
            // TWO numbers, because one of them was a lie (K-406).
            //
            // This used to record `spent - present` alone and call it "render
            // us". The intent was to leave out the pacing sleep, but
            // `present()` renders the scene, reads it back off the GPU and
            // THEN paces -- so subtracting all of it also subtracted the
            // rendering. The app reported 35us a frame while the GPU readback
            // alone measured 4,346us (K-405), about 1/124 of the truth.
            //
            // `guest` is the game's own work: physics, AI, building the mesh
            // lists. `wall` is everything, pacing included, and is what
            // decides whether the game holds 60. Reporting both means neither
            // can flatter: if wall sits at the frame budget the game is
            // pacing, and if it exceeds it the game is late whatever the
            // guest half says.
            guest_samples.push((spent.saturating_sub(present) / 1_000) as u32);
            wall_samples.push((spent / 1_000) as u32);

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
        if !wall_samples.is_empty() {
            guest_samples.sort_unstable();
            wall_samples.sort_unstable();
            let q = |v: &Vec<u32>, p: f32| v[((v.len() - 1) as f32 * p) as usize];
            // Wall first: it is the number that says whether the game holds
            // its frame rate, and putting the smaller number first is how the
            // old report read as three times faster than it was.
            let mut s = String::new();
            s.push_str("drift: frames ");
            s.push_str(&u64_str(frames));
            s.push_str("  wall us p50 ");
            s.push_str(&u64_str(q(&wall_samples, 0.50) as u64));
            s.push_str(" p95 ");
            s.push_str(&u64_str(q(&wall_samples, 0.95) as u64));
            s.push_str(" p99 ");
            s.push_str(&u64_str(q(&wall_samples, 0.99) as u64));
            s.push_str(" worst ");
            s.push_str(&u64_str(wall_samples[wall_samples.len() - 1] as u64));
            say(&s);
            let mut w = String::new();
            w.push_str("  guest us p50 ");
            w.push_str(&u64_str(q(&guest_samples, 0.50) as u64));
            w.push_str(" p95 ");
            w.push_str(&u64_str(q(&guest_samples, 0.95) as u64));
            w.push_str("  (the rest is the host: render, readback, pacing)");
            say(&w);
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
