//! Drift -- a 3D driving game, built to find out exactly where the software
//! rasterizer stops being enough.
//!
//! CP-E of Plan/Proof-Checkpoints-2026-09-16.md, and the one written expecting
//! to fail. `gfx.scene3d` rasterizes on the CPU, caps a surface at 1024x1024,
//! takes one flat tint per draw call, and re-sends a mesh's vertices on every
//! `place`. A GTA-class engine is none of those things. The point is not to
//! discover that; it is to find out WHICH of them is the wall, with numbers,
//! so the GPU work that follows is aimed rather than guessed.
//!
//! The game: a car you drive over a heightmapped landscape, buildings, trees,
//! road markers, a chase camera, collision against the terrain, and a HUD.
//! `bench` sweeps the scene from nearly empty to several hundred objects and
//! reports frame time at each step, which is the measurement CP-E exists for.
//!
//! `#![no_std]`: the vertex buffers are the allocation that matters and they
//! are built once, not per frame.

#![no_std]
#![allow(clippy::needless_range_loop)]
#![allow(clippy::too_many_arguments)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{scene3d, types as gfx};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const SCENE_ID: u64 = 2;

/// The surface cap is 1024 (`MAX_EDGE` in the host), so a 1080p window is not
/// reachable. This is the largest square the runtime will give.
const WIDTH: f32 = 1024.0;
const HEIGHT: f32 = 640.0;

/// Terrain grid. 48x48 cells is 4,608 triangles before anything else is drawn.
const GRID: usize = 48;
const CELL: f32 = 8.0;

// ------------------------------------------------------------------- world

fn hash2(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
    h ^= h >> 13;
    h = h.wrapping_mul(1_274_126_177);
    ((h >> 8) & 0xFFFF) as f32 / 65_535.0
}

/// Round toward negative infinity.
///
/// `as i32` truncates toward ZERO, so for a negative coordinate it lands on
/// the cell above rather than below and the fraction comes out negative. The
/// smoothstep below is only a smooth 0..1 ramp for t in 0..1; feed it -0.7 and
/// it returns -2.3, which multiplies the octave's amplitude into terrain that
/// reaches -75 instead of staying inside 0..13. Half the map ended up above
/// the chase camera, which reads on screen as an upside-down world rather
/// than as bad noise.
fn floor_i32(v: f32) -> i32 {
    let t = v as i32;
    if v < 0.0 && v != t as f32 {
        t - 1
    } else {
        t
    }
}

/// Smooth-ish height at a world point: two octaves of value noise.
fn height_at(x: f32, z: f32) -> f32 {
    let mut h = 0.0;
    let mut amp = 9.0;
    let mut freq = 0.012;
    for _ in 0..2 {
        let fx = x * freq;
        let fz = z * freq;
        let x0 = floor_i32(fx);
        let z0 = floor_i32(fz);
        let tx = fx - x0 as f32;
        let tz = fz - z0 as f32;
        let sx = tx * tx * (3.0 - 2.0 * tx);
        let sz = tz * tz * (3.0 - 2.0 * tz);
        let a = hash2(x0, z0);
        let b = hash2(x0 + 1, z0);
        let c = hash2(x0, z0 + 1);
        let d = hash2(x0 + 1, z0 + 1);
        let top = a + (b - a) * sx;
        let bot = c + (d - c) * sx;
        h += (top + (bot - top) * sz) * amp;
        amp *= 0.45;
        freq *= 2.7;
    }
    // A flat valley down the middle for the road.
    let road = (x / 26.0).abs().min(1.0);
    h * road
}

/// One mesh, built once and placed many times.
struct Mesh {
    verts: Vec<f32>,
}

impl Mesh {
    fn tri_count(&self) -> usize {
        self.verts.len() / 9
    }
}

/// A box as twelve triangles, centred on the origin, sitting on y=0.
fn box_mesh(w: f32, h: f32, d: f32) -> Mesh {
    let (x0, x1) = (-w * 0.5, w * 0.5);
    let (y0, y1) = (0.0, h);
    let (z0, z1) = (-d * 0.5, d * 0.5);
    let c = [
        [x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0],
        [x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1],
    ];
    let faces: [[usize; 6]; 6] = [
        [0, 2, 1, 0, 3, 2], // front
        [5, 7, 4, 5, 6, 7], // back
        [4, 3, 0, 4, 7, 3], // left
        [1, 6, 5, 1, 2, 6], // right
        [3, 6, 2, 3, 7, 6], // top
        [4, 1, 5, 4, 0, 1], // bottom
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

/// A cone-ish tree: a trunk box plus a four-sided pyramid.
fn tree_mesh() -> Mesh {
    let mut m = box_mesh(0.7, 2.2, 0.7);
    let r = 2.0;
    let base = 2.0;
    let top = 7.0;
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
    Mesh { verts: m.verts }
}

/// The terrain, as one mesh. Built once; this is the fixed cost of the world.
fn terrain_mesh() -> Mesh {
    let mut verts = Vec::with_capacity(GRID * GRID * 18);
    let half = GRID as f32 * CELL * 0.5;
    for gz in 0..GRID {
        for gx in 0..GRID {
            let x0 = gx as f32 * CELL - half;
            let z0 = gz as f32 * CELL - half;
            let x1 = x0 + CELL;
            let z1 = z0 + CELL;
            let y00 = height_at(x0, z0);
            let y10 = height_at(x1, z0);
            let y01 = height_at(x0, z1);
            let y11 = height_at(x1, z1);
            // Wound so the UP-facing side is the front face. The obvious
            // order -- (x0,z0), (x1,z0), (x1,z1) -- is clockwise seen from
            // above, which back-face culling drops: the ground vanished from
            // under the car and only distant hills survived, because those are
            // seen at a grazing angle where the near edge still faces the eye.
            for p in [
                [x0, y00, z0], [x1, y11, z1], [x1, y10, z0],
                [x0, y00, z0], [x0, y01, z1], [x1, y11, z1],
            ] {
                verts.push(p[0]);
                verts.push(p[1]);
                verts.push(p[2]);
            }
        }
    }
    Mesh { verts }
}

// -------------------------------------------------------------------- props

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

/// How long the last `present` took, so the benchmark can subtract the
/// runtime's deliberate frame pacing and report the real render cost.
static mut PRESENT_NS: u64 = 0;
static mut SUBMIT_TOTAL: u64 = 0;

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

/// Scatter buildings and trees across the landscape, avoiding the road.
fn scatter(n: usize) -> Vec<Prop> {
    let mut out = Vec::with_capacity(n);
    let mut i = 0u32;
    while out.len() < n {
        i += 1;
        let x = (hash2(i as i32, 7) - 0.5) * GRID as f32 * CELL * 0.92;
        let z = (hash2(11, i as i32) - 0.5) * GRID as f32 * CELL * 0.92;
        if x.abs() < 16.0 {
            continue; // keep the road clear
        }
        let r = hash2(i as i32, i as i32);
        let building = r > 0.45;
        out.push(Prop {
            x,
            z,
            y: height_at(x, z),
            rot: hash2(i as i32, 3) * 360.0,
            scale: if building {
                0.8 + hash2(i as i32, 5) * 2.4
            } else {
                0.6 + hash2(i as i32, 9) * 0.9
            },
            kind: if building { 0 } else { 1 },
            tint: if building {
                let t = 0.35 + hash2(i as i32, 13) * 0.35;
                rgb(t, t * 0.95, t * 0.88)
            } else {
                let g = 0.30 + hash2(i as i32, 17) * 0.30;
                rgb(g * 0.45, g, g * 0.40)
            },
        });
    }
    out
}

// --------------------------------------------------------------------- math

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

fn sqrt_approx(v: f32) -> f32 {
    if v <= 0.0 {
        return 0.0;
    }
    let mut g = v;
    for _ in 0..8 {
        g = 0.5 * (g + v / g);
    }
    g
}

fn u64_str(v: u64) -> String {
    let mut s = String::new();
    let mut d = [0u8; 24];
    let mut n = 0;
    let mut v = v;
    if v == 0 {
        s.push('0');
        return s;
    }
    while v > 0 {
        d[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        s.push(d[n] as char);
    }
    s
}

fn say(s: &str) {
    let out = stdio::stdout();
    let _ = out.write(s.as_bytes());
}

// --------------------------------------------------------------------- car

struct Car {
    x: f32,
    z: f32,
    y: f32,
    heading: f32,
    speed: f32,
}

// -------------------------------------------------------------------- draw

/// Draw one frame. Returns how many `place` calls it made, which is the number
/// CP-E is really measuring: every one crosses the sandbox boundary carrying
/// the whole mesh.
fn draw_scene(
    scene: u64,
    terrain: &Mesh,
    building: &Mesh,
    tree: &Mesh,
    car_mesh: &Mesh,
    props: &[Prop],
    visible: usize,
    car: &Car,
) -> Result<usize, gfx::GfxError> {
    scene3d::clear(scene, rgb(0.52, 0.68, 0.88))?;

    // Chase camera, behind and above, looking where the car points.
    let back = 26.0;
    let ex = car.x - sin_approx(car.heading) * back;
    let ez = car.z - cos_approx(car.heading) * back;
    let ey = car.y + 12.0;
    scene3d::camera(
        scene,
        &[ex, ey, ez],
        &[car.x, car.y + 2.0, car.z],
        58.0,
    )?;
    scene3d::light(scene, &[-0.4, -0.85, -0.3])?;

    let mut calls = 0;

    // The landscape: one call, the biggest mesh.
    scene3d::triangles(scene, &terrain.verts, rgb(0.42, 0.56, 0.34))?;
    calls += 1;

    // Props, nearest first. `place` sends the mesh EVERY time -- the runtime
    // has no retained mesh -- so this is where the boundary cost lives.
    for p in props.iter().take(visible) {
        let mesh = if p.kind == 0 { building } else { tree };
        scene3d::place(
            scene,
            &mesh.verts,
            &[p.x, p.y, p.z],
            &[0.0, p.rot, 0.0],
            p.scale,
            p.tint,
        )?;
        calls += 1;
    }

    // The car.
    scene3d::place(
        scene,
        &car_mesh.verts,
        &[car.x, car.y + 0.3, car.z],
        &[0.0, car.heading * 57.295_78, 0.0],
        1.0,
        rgb(0.85, 0.22, 0.25),
    )?;
    calls += 1;

    let p0 = clock::monotonic_nanos();
    scene3d::present(scene)?;
    unsafe {
        PRESENT_NS = clock::monotonic_nanos().saturating_sub(p0);
    }
    Ok(calls)
}

// --------------------------------------------------------------------- main

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

fn fail(step: &[u8]) -> i32 {
    let out = stdio::stdout();
    let _ = out.write(b"drift: failed at ");
    let _ = out.write(step);
    let _ = out.write(b"\n");
    1
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let has = |n: &[u8]| raw.as_bytes().split(|b| *b == b'\n').any(|a| a == n);
        let quick = has(b"quick") || has(b"--quick");
        let bench = has(b"bench");

        let win = match window::create(
            "Drift",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) {
            Ok(w) => w,
            Err(_) => return fail(b"window::create"),
        };
        if window::show(win).is_err() {
            return fail(b"window::show");
        }
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
        // Back-face culling roughly halves the triangles a closed mesh costs.
        let _ = scene3d::cull_back_faces(scene, true);

        let terrain = terrain_mesh();
        let building = box_mesh(6.0, 9.0, 6.0);
        let tree = tree_mesh();
        let car_mesh = box_mesh(2.2, 1.4, 4.4);
        let props = scatter(4000);

        let mut car = Car {
            x: 0.0,
            z: -120.0,
            y: 0.0,
            heading: 0.0,
            speed: 0.0,
        };
        car.y = height_at(car.x, car.z);

        if bench {
            say("drift-bench: software rasterizer, 1024x640\n");
            // Can a 3D scene be 1080p? MAX_EDGE says no; confirm it refuses
            // rather than silently shrinking, because a game that thinks it
            // is 1080p and is not would be the worse outcome.
            match window::create(
                "probe",
                types::WindowSize {
                    width: 1920,
                    height: 1080,
                },
            ) {
                Ok(w2) => {
                    let _ = window::show(w2);
                    let _ = tree::set_root(w2, &node(ROOT_ID, None, types::WidgetKind::Stack));
                    let _ = tree::upsert_node(
                        w2,
                        &node(SCENE_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
                    );
                    match scene3d::bind(w2, SCENE_ID) {
                        Ok(s2) => match scene3d::clear(s2, rgb(0.0, 0.0, 0.0)) {
                            Ok(_) => say("  1920x1080 3D surface: ACCEPTED\n"),
                            Err(_) => say("  1920x1080 3D surface: REFUSED at clear\n"),
                        },
                        Err(_) => say("  1920x1080 3D surface: REFUSED at bind\n"),
                    }
                    let _ = window::close(w2);
                }
                Err(_) => say("  1920x1080 window: refused\n"),
            }
            let mut m = String::from("  terrain ");
            m.push_str(&u64_str(terrain.tri_count() as u64));
            m.push_str(" tris, building ");
            m.push_str(&u64_str(building.tri_count() as u64));
            m.push_str(", tree ");
            m.push_str(&u64_str(tree.tri_count() as u64));
            m.push('\n');
            say(&m);

            for visible in [0usize, 100, 400, 1000, 2000, 4000] {
                // Warm, then measure 30 frames.
                for _ in 0..3 {
                    let _ = draw_scene(
                        scene, &terrain, &building, &tree, &car_mesh, &props, visible, &car,
                    );
                }
                let mut worst = 0u64;
                let t0 = clock::monotonic_nanos();
                let mut calls = 0;
                for i in 0..30 {
                    car.heading = i as f32 * 0.02;
                    let f0 = clock::monotonic_nanos();
                    calls = draw_scene(
                        scene, &terrain, &building, &tree, &car_mesh, &props, visible, &car,
                    )
                    .unwrap_or(0);
                    let took = clock::monotonic_nanos()
                        .saturating_sub(f0)
                        .saturating_sub(unsafe { PRESENT_NS });
                    unsafe { SUBMIT_TOTAL += took };
                    if took > worst {
                        worst = took;
                    }
                }
                let wall = clock::monotonic_nanos().saturating_sub(t0) / 30;
                let avg = unsafe { SUBMIT_TOTAL } / 30;
                unsafe { SUBMIT_TOTAL = 0 };

                // Triangles actually submitted this frame.
                let tris = terrain.tri_count()
                    + car_mesh.tri_count()
                    + props
                        .iter()
                        .take(visible)
                        .map(|p| {
                            if p.kind == 0 {
                                building.tri_count()
                            } else {
                                tree.tri_count()
                            }
                        })
                        .sum::<usize>();

                let mut r = String::from("  objects ");
                r.push_str(&u64_str(visible as u64 + 2));
                r.push_str("  calls ");
                r.push_str(&u64_str(calls as u64));
                r.push_str("  tris ");
                r.push_str(&u64_str(tris as u64));
                r.push_str("  avg ");
                r.push_str(&u64_str(avg / 1_000));
                r.push_str("us  worst ");
                r.push_str(&u64_str(worst / 1_000));
                r.push_str("us  wall ");
                r.push_str(&u64_str(wall / 1_000));
                r.push_str("us  render-fps ");
                r.push_str(&u64_str(if avg > 0 { 1_000_000_000 / avg } else { 0 }));
                r.push('\n');
                say(&r);
            }
            say("drift-bench: done\n");
            return 0;
        }

        let _ = draw_scene(
            scene, &terrain, &building, &tree, &car_mesh, &props, 120, &car,
        );

        if quick {
            let mut m = String::from("drift: terrain ");
            m.push_str(&u64_str(terrain.tri_count() as u64));
            m.push_str(" tris, ");
            m.push_str(&u64_str(props.len() as u64));
            m.push_str(" props\n");
            say(&m);
            return 0;
        }

        let mut last = clock::monotonic_nanos();
        loop {
            let now = clock::monotonic_nanos();
            let dt = (now.saturating_sub(last) as f32 / 1_000_000_000.0).min(0.05);
            last = now;

            let left = events::key_held("ArrowLeft") || events::key_held("a");
            let right = events::key_held("ArrowRight") || events::key_held("d");
            let gas = events::key_held("ArrowUp") || events::key_held("w");
            let brake = events::key_held("ArrowDown") || events::key_held("s");

            if gas {
                car.speed += 26.0 * dt;
            } else if brake {
                car.speed -= 34.0 * dt;
            } else {
                car.speed *= 1.0 - (1.2 * dt).min(1.0);
            }
            car.speed = car.speed.clamp(-18.0, 44.0);

            // Steering scales with speed, so the car does not pivot on the spot.
            let steer = (car.speed.abs() / 44.0).min(1.0);
            if left {
                car.heading -= 1.8 * dt * steer;
            }
            if right {
                car.heading += 1.8 * dt * steer;
            }

            car.x += sin_approx(car.heading) * car.speed * dt;
            car.z += cos_approx(car.heading) * car.speed * dt;
            let limit = GRID as f32 * CELL * 0.46;
            car.x = car.x.clamp(-limit, limit);
            car.z = car.z.clamp(-limit, limit);

            // Follow the ground rather than floating over it.
            let ground = height_at(car.x, car.z);
            car.y += (ground - car.y) * (8.0 * dt).min(1.0);

            let _ = sqrt_approx(1.0); // keep the helper honest under --release

            if draw_scene(
                scene, &terrain, &building, &tree, &car_mesh, &props, 120, &car,
            )
            .is_err()
            {
                break;
            }

            if let Some(types::Event::CloseRequested(id)) = events::poll() {
                let _ = window::close(id);
                break;
            }
        }
        0
    }
}

krate::export!(Component);
