//! Krate cubes — the first app that draws in 3D.
//!
//! Nine cubes on a floor, spinning at different rates, lit by one directional
//! light, with a camera the person drives using the arrow keys. It exists to
//! prove the whole 3D path through the WIT boundary: bind, camera, light,
//! placed meshes, depth test, present — and held-key input, which is what
//! makes it a thing you control rather than a thing you watch.
//!
//! One cube mesh is sent nine times with different transforms. That is the
//! point of `place`: the vertices never change, so the app keeps a single copy
//! and the host does the moving.
//!
//! `quick` renders a fixed number of frames and reports what it drew, so a
//! regression in the 3D path fails the nightly replay rather than waiting for
//! somebody to look at a window.


#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{scene3d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const SCENE_ID: u64 = 2;

const WIDTH: f32 = 320.0;
const HEIGHT: f32 = 240.0;
/// Frames the `quick` run draws before reporting.
const QUICK_FRAMES: u32 = 30;
/// How fast the camera orbits, in degrees a second, when a key is held.
const TURN_RATE: f32 = 70.0;
/// How far a stick must move before it counts, so a worn controller resting at
/// 0.02 does not slowly spin the camera on its own.
const STICK_DEADZONE: f32 = 0.15;

struct Component;

/// The eight corners of a unit cube, as twelve triangles.
///
/// A fixed array rather than a `Vec`, and that is not a style choice: a `Vec`
/// grown inside a loop keeps std's reallocation path reachable, and with it
/// the out-of-memory handler that drags the entire `wasi:*` import surface
/// into the component. Measured -- the same function built on `Vec` imports
/// thirty-three wasi functions, and this one imports none.
fn cube() -> [f32; 108] {
    let c = [
        [-0.5_f32, -0.5, -0.5],
        [0.5, -0.5, -0.5],
        [0.5, 0.5, -0.5],
        [-0.5, 0.5, -0.5],
        [-0.5, -0.5, 0.5],
        [0.5, -0.5, 0.5],
        [0.5, 0.5, 0.5],
        [-0.5, 0.5, 0.5],
    ];
    let faces = [
        [0_usize, 1, 2],
        [0, 2, 3], // back
        [5, 4, 7],
        [5, 7, 6], // front
        [4, 0, 3],
        [4, 3, 7], // left
        [1, 5, 6],
        [1, 6, 2], // right
        [3, 2, 6],
        [3, 6, 7], // top
        [4, 5, 1],
        [4, 1, 0], // bottom
    ];
    let mut mesh = [0.0_f32; 108];
    let mut at = 0;
    for face in faces {
        for corner in face {
            if let Some(point) = c.get(corner) {
                for value in point {
                    if let Some(slot) = mesh.get_mut(at) {
                        *slot = *value;
                    }
                    at += 1;
                }
            }
        }
    }
    mesh
}

/// A wide flat slab for the ground, so the cubes have something to sit on and
/// the depth test has something to prove.
fn floor() -> [f32; 18] {
    [
        -6.0, -0.75, -6.0, //
        6.0, -0.75, -6.0, //
        6.0, -0.75, 6.0, //
        -6.0, -0.75, -6.0, //
        6.0, -0.75, 6.0, //
        -6.0, -0.75, 6.0,
    ]
}

fn colour(r: f32, g: f32, b: f32) -> gfx::Color {
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

fn out(text: &str) {
    let handle = stdio::stdout();
    let _ = handle.write(text.as_bytes());
    let _ = handle.write(b"\n");
}

/// Write a small unsigned number without `format!`.
///
/// `format!` and `.to_string()` reach std's allocation failure path, and any
/// reachable panic makes std's whole failure path reachable -- which is
/// `wasi:cli`, `wasi:filesystem` and `wasi:io` arriving together. Every index
/// here goes through `.get()` for the same reason: a bounds check that can
/// panic is a panic site.
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

/// Sine and cosine without `std`, from a few Taylor terms.
///
/// Accurate to about four decimals over a full turn, which is far finer than a
/// 320-pixel-wide frame can show. Written here rather than pulled from a crate
/// because a maths crate would be one more dependency to keep clean, and this
/// is twelve lines.
fn sin(mut x: f32) -> f32 {
    const TAU: f32 = 6.283_185_5;
    while x > TAU {
        x -= TAU;
    }
    while x < 0.0 {
        x += TAU;
    }
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0))))
}

fn cos(x: f32) -> f32 {
    sin(x + 1.570_796_3)
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw.split_whitespace().any(|word| word == "quick");

        let win = match window::create(
            "Cubes",
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
        let _ = tree::upsert_node(win, &node(SCENE_ID, Some(ROOT_ID), types::WidgetKind::Canvas));

        let scene = match scene3d::bind(win, SCENE_ID) {
            Ok(scene) => scene,
            Err(_) => {
                out("bind:no");
                return 1;
            }
        };

        let mesh = cube();
        let ground = floor();

        // Camera orbits a fixed distance from the middle; the arrow keys move
        // the angle and the height rather than the position, which is what
        // makes an orbit feel like looking at something.
        let mut angle = 0.6_f32;
        let mut height = 3.0_f32;
        let mut spin = 0.0_f32;

        let started = clock::monotonic_nanos();
        let mut last = started;
        let mut frames: u32 = 0;

        loop {
            let now = clock::monotonic_nanos();
            let dt = ((now.saturating_sub(last)) as f32 / 1_000_000_000.0).min(0.05);
            last = now;

            // Held keys, not key events: this is a thing you steer.
            if events::key_held("ArrowLeft") {
                angle -= TURN_RATE.to_radians() * dt;
            }
            if events::key_held("ArrowRight") {
                angle += TURN_RATE.to_radians() * dt;
            }
            if events::key_held("ArrowUp") {
                height += 3.0 * dt;
            }
            if events::key_held("ArrowDown") {
                height -= 3.0 * dt;
            }

            // The same camera, driven by a stick when somebody has one. Asking
            // first and falling back to the keyboard is the pattern every app
            // should copy: a person without a controller is the common case,
            // and the sticks read centred for them rather than drifting.
            if events::gamepad_connected() {
                let turn = events::gamepad_axis("left-x");
                let rise = events::gamepad_axis("left-y");
                if turn > STICK_DEADZONE || turn < -STICK_DEADZONE {
                    angle += TURN_RATE.to_radians() * turn * dt;
                }
                if rise > STICK_DEADZONE || rise < -STICK_DEADZONE {
                    height += 3.0 * rise * dt;
                }
                // South is A on an Xbox pad and B on a Nintendo one. The app
                // asks for the button under the thumb and gets it on both.
                if events::gamepad_held("south") {
                    spin += 60.0 * dt;
                }
            }
            if height < 0.5 {
                height = 0.5;
            }
            if height > 8.0 {
                height = 8.0;
            }
            spin += 40.0 * dt;

            let radius = 7.0_f32;
            let eye = [cos(angle) * radius, height, sin(angle) * radius];
            if scene3d::camera(scene, &eye, &[0.0, 0.0, 0.0], 60.0).is_err() {
                out("camera:no");
                return 1;
            }

            // A headlight: the light points from the camera toward the middle,
            // so the faces you are looking at are the faces that catch it. A
            // fixed light cannot do this while the camera orbits -- it lit the
            // far side and left every visible face on ambient alone, which read
            // as flat and dim. Nudged downward so the tops stay brighter than
            // the sides and the cubes keep their shape.
            let _ = scene3d::light(scene, &[-eye[0], -eye[1] - 3.0, -eye[2]]);

            if scene3d::clear(scene, colour(0.05, 0.07, 0.12)).is_err() {
                out("clear:no");
                return 1;
            }

            // The floor first, then the cubes on top of it. Order does not
            // matter -- that is what the depth buffer is for -- but drawing
            // the ground first reads more clearly.
            let _ = scene3d::triangles(scene, &ground, colour(0.18, 0.2, 0.26));

            // Nine cubes in a grid, each spinning at its own rate. One mesh,
            // nine placements: the vertex list never changes.
            let mut drawn = 0_u32;
            for row in 0..3_i32 {
                for column in 0..3_i32 {
                    let x = (column - 1) as f32 * 2.2;
                    let z = (row - 1) as f32 * 2.2;
                    let rate = 1.0 + (row * 3 + column) as f32 * 0.35;
                    let tint = colour(
                        0.35 + 0.2 * column as f32,
                        0.55,
                        0.9 - 0.15 * row as f32,
                    );
                    if scene3d::place(
                        scene,
                        &mesh,
                        &[x, 0.0, z],
                        &[0.0, spin * rate, spin * 0.4],
                        1.0,
                        tint,
                    )
                    .is_err()
                    {
                        out("place:no");
                        return 1;
                    }
                    drawn += 1;
                }
            }

            if scene3d::present(scene).is_err() {
                out("present:no");
                return 1;
            }
            frames += 1;

            if quick {
                if frames >= QUICK_FRAMES {
                    out_number("cubes:", drawn as u64);
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

        let elapsed = clock::monotonic_nanos().saturating_sub(started);
        out_number("frames:", frames as u64);
        let fps_centi = if elapsed > 0 {
            (frames as u64 * 100 * 1_000_000_000) / elapsed
        } else {
            0
        };
        out_number("fps-centi:", fps_centi);
        out("rendered3d:yes");

        let _ = window::close(win);
        0
    }
}

bindings::export!(Component with_types_in bindings);
