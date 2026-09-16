//! Showcase -- one scene, built to find out how good a Krate 3D app can look.
//!
//! The racing game that came before this looked like 1997, and the honest
//! question was whether that was the runtime or the app. This is the answer,
//! built by taking every lever the renderer actually has and pulling it hard:
//!
//! - **Subdivision instead of smooth normals.** The shading term is
//!   `0.35 + 0.65 * |n . l|` computed per TRIANGLE from the face normal, and
//!   there are no vertex normals. So a sphere cannot be smooth-shaded the
//!   usual way -- but a sphere of three thousand facets has three thousand
//!   normals, and at that density the facets are finer than the shading
//!   difference between them. Curvature comes out of triangle count.
//! - **Bevelled edges.** A box has six normals and reads as a box. The same
//!   box with chamfered edges has twenty-six, and the chamfer catches the
//!   light differently from the faces either side. That single change is most
//!   of what separates "blocked out" from "manufactured".
//! - **Everything painted into 1024-pixel textures.** Marble veins, brushed
//!   metal streaks, tile grout, ambient occlusion at tile edges, a sky
//!   gradient with a sun and cloud, metallic flake in the car paint, and a
//!   sheen where a specular highlight would fall. The renderer has one
//!   directional light and no specular, so every other lighting cue is
//!   painted rather than computed.
//! - **A sky dome.** A flat `clear` colour behind geometry is the single
//!   biggest tell that a scene has nothing behind it.
//! - **Fake contact shadows.** No shadow casting and no alpha blending, so a
//!   shadow is opaque geometry a few millimetres above the floor, textured
//!   with a radial darkening. An object with no shadow floats.
//!
//! What it cannot do, and what a real fix would need, is on the board as
//! K-400: no alpha blending (so no glass, smoke or particles), no vertex
//! normals, one light, no cast shadows.

#![no_std]
#![allow(clippy::too_many_arguments)]

extern crate alloc;

mod geom;
mod mathx;
mod paint;

use alloc::string::String;
use alloc::vec::Vec;

use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{scene3d, types as gfx};
use krate::ui::{events, tree, types, window};

use geom::Mesh;
use mathx::{cos_approx, sin_approx};

const ROOT_ID: u64 = 1;
const SCENE_ID: u64 = 2;
const WIDTH: u32 = 1600;
const HEIGHT: u32 = 900;

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
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

fn fail(step: &[u8]) -> i32 {
    let out = stdio::stdout();
    let _ = out.write(b"showcase: failed at ");
    let _ = out.write(step);
    let _ = out.write(b"\n");
    1
}

fn say(s: &str) {
    let out = stdio::stdout();
    let _ = out.write(s.as_bytes());
    let _ = out.write(b"\n");
}

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

/// One drawable: a world-space mesh and the texture it wears.
struct Part {
    mesh: Mesh,
    texture: u64,
    tint: gfx::Color,
}

/// A gentle bowl so the ground is not a flat sheet. Kept very shallow: the
/// scene is a plaza, and the point of the height is that the light falls
/// differently across it, not that it looks like terrain.
fn ground_height(x: f32, z: f32) -> f32 {
    sin_approx(x * 0.012) * 0.5 + cos_approx(z * 0.014) * 0.4 - (x * x + z * z) * 0.000_02
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let has = |n: &[u8]| raw.as_bytes().split(|b| *b == b'\n').any(|a| a == n);
        let quick = has(b"quick") || has(b"--quick");

        let win = match window::create(
            "Showcase",
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
        // OFF, deliberately. The sky dome is seen from the inside, and the
        // contact shadows are single-sided quads; culling either of those out
        // costs more than culling saves on a scene this size.
        let _ = scene3d::cull_back_faces(scene, false);

        // Ask for everything the renderer can do beyond one flat light. This
        // is refused on a machine with no GPU, and the scene still draws --
        // it just draws the way it did before, which is the point of the
        // feature being opt-in rather than a new default.
        let lit = scene3d::set_lighting(
            scene,
            &scene3d::Lighting {
                // A little darker than the 0.35 default: with a fill light
                // doing the work the floor no longer has to, and a lower floor
                // is what lets a shadow side actually read as dark.
                ambient: 0.16,
                // Enough to see, not enough to look like wet plastic.
                specular: 0.55,
                shininess: 48.0,
                // Gentle: the plaza is about 200 units across and this should
                // put air between the near pillars and the far ones without
                // hiding anything.
                fog_density: 0.0042,
                fog_color: rgb(0.62, 0.70, 0.84),
                // A cool fill from the opposite side of the key light, which
                // is what the sky does outdoors.
                fill_direction: alloc::vec![0.55, 0.42, 0.72],
                fill_color: rgb(0.34, 0.42, 0.58),
                // The plaza is about 90 units across at its widest, so 70
                // covers everything that can cast onto what the camera sees
                // while keeping the map's texels small.
                shadow_radius: 70.0,
                shadow_softness: 1.4,
            },
        )
        .is_ok();
        if lit {
            say("showcase: specular, fog, a fill light and cast shadows are on");
        } else {
            say("showcase: this renderer has one flat light; drawing without");
        }

        // ---- textures
        let t0 = clock::monotonic_nanos();
        let upload = |t: paint::Texture| -> u64 {
            scene3d::upload_texture(scene, t.width, t.height, &t.rgba).unwrap_or(0)
        };
        let tex_sky = upload(paint::sky(512));
        let tex_floor = upload(paint::floor_tiles(1024));
        let tex_marble = upload(paint::marble(1024));
        let tex_metal = upload(paint::brushed_metal(512));
        let tex_panel = upload(paint::panel(512));
        let tex_red = upload(paint::car_paint(512, (1.0, 0.22, 0.20)));
        let tex_gold = upload(paint::car_paint(512, (1.0, 0.78, 0.30)));
        let tex_shadow = upload(paint::soft_shadow(256));
        let paint_ms = (clock::monotonic_nanos() - t0) / 1_000_000;

        // ---- the scene
        let mut parts: Vec<Part> = Vec::new();
        let white = rgb(1.0, 1.0, 1.0);

        // A sky dome, big enough that nothing reaches it. Drawn from inside,
        // which is why culling is off.
        // Normals flipped INWARD: a sky dome is seen from inside, and with
        // one-sided lighting an outward normal means the whole sky faces away
        // from the light and renders at the ambient floor. This is the
        // geometry that smooth shading changed the meaning of.
        let mut sky_dome = geom::sphere(900.0, 24, 48);
        for n in sky_dome.normals.iter_mut() {
            *n = -*n;
        }
        parts.push(Part {
            mesh: sky_dome,
            texture: tex_sky,
            // Over 1.0: the shading floor is 0.35 and a sky should not be
            // lit at all. This is the only lever an app has (K-400).
            tint: rgb(2.6, 2.6, 2.6),
        });

        // The floor: a big subdivided plane so the light falls across it.
        parts.push(Part {
            mesh: geom::ground(200.0, 64, FLOOR_UV, ground_height),
            texture: tex_floor,
            tint: white,
        });


        // The floor's UV scale, shared with the shadow patches so they sample
        // the same texture the floor does.
        const FLOOR_UV: f32 = 26.0;
        // A painted shadow blob under each object, used ONLY when the
        // renderer cannot cast a real one.
        //
        // This has now been three things. First a grid of 361 opaque patches
        // sampling the floor at a darkening tint, which was the workaround for
        // having no alpha at all. Then one blended quad, once the rasterizer
        // could blend. Now a fallback, because the GPU casts real shadows and
        // drawing both puts two shadows under everything.
        let add_shadow = |parts: &mut Vec<Part>, x: f32, y: f32, z: f32, size: f32| {
            if lit {
                return;
            }
            let mut m = Mesh::default();
            let h = size * 0.5;
            m.quad(
                [x - h, y, z - h],
                [x - h, y, z + h],
                [x + h, y, z + h],
                [x + h, y, z - h],
                [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
            );
            m.fill_face_normals();
            parts.push(Part {
                mesh: m,
                texture: tex_shadow,
                tint: rgb(1.0, 1.0, 1.0),
            });
        };

        // A ring of pillars, each a bevelled column with a marble shaft and a
        // metal cap. Eight of them, so the scene has depth cues at several
        // distances rather than one object in the middle of nowhere.
        for i in 0..8 {
            let a = i as f32 / 8.0 * core::f32::consts::PI * 2.0;
            let (px, pz) = (cos_approx(a) * 34.0, sin_approx(a) * 34.0);
            let py = ground_height(px, pz);
            parts.push(Part {
                mesh: geom::bevel_box(3.4, 11.0, 3.4, 0.28).placed(px, py, pz, a),
                texture: tex_marble,
                tint: white,
            });
            parts.push(Part {
                mesh: geom::bevel_box(4.4, 0.7, 4.4, 0.18).placed(px, py + 11.0, pz, a),
                texture: tex_metal,
                tint: white,
            });
            parts.push(Part {
                mesh: geom::bevel_box(4.8, 0.5, 4.8, 0.14).placed(px, py - 0.02, pz, a),
                texture: tex_panel,
                tint: white,
            });
            // Contact shadow under each pillar.
            add_shadow(&mut parts, px, py + 0.03, pz, 8.0);
        }

        // The hero: a large polished sphere on a plinth, which is the object
        // that proves curvature. 56x72 rings is 8,064 facets.
        let hero_y = ground_height(0.0, 0.0);
        parts.push(Part {
            mesh: geom::sphere(6.0, 56, 72).translated(0.0, hero_y + 9.4, 0.0),
            texture: tex_red,
            tint: white,
        });
        parts.push(Part {
            mesh: geom::bevel_box(9.0, 3.4, 9.0, 0.5).translated(0.0, hero_y, 0.0),
            texture: tex_marble,
            tint: white,
        });
        add_shadow(&mut parts, 0.0, hero_y + 0.04, 0.0, 17.0);

        // A torus orbiting it, gold, because a curved closed surface with a
        // hole is the shape that most obviously is not a box.
        parts.push(Part {
            mesh: geom::torus(11.0, 1.3, 64, 28).translated(0.0, hero_y + 9.4, 0.0),
            texture: tex_gold,
            tint: white,
        });

        // Smaller spheres scattered round the plaza, each on its own shadow,
        // so the eye has something to judge scale and distance against.
        for i in 0..6 {
            let a = (i as f32 + 0.5) / 6.0 * core::f32::consts::PI * 2.0;
            let r = 17.0;
            let (sx, sz) = (cos_approx(a) * r, sin_approx(a) * r);
            let sy = ground_height(sx, sz);
            let radius = 1.6 + (i % 3) as f32 * 0.5;
            parts.push(Part {
                mesh: geom::sphere(radius, 28, 36).translated(sx, sy + radius, sz),
                texture: if i % 2 == 0 { tex_metal } else { tex_gold },
                tint: white,
            });
            add_shadow(&mut parts, sx, sy + 0.03, sz, radius * 3.2);
        }

        // Anything built without its own normals gets the face normal, so it
        // draws through `smooth` looking exactly as it did through `textured`.
        // Only the sphere and the torus know their real normals, and only they
        // change.
        for p in parts.iter_mut() {
            p.mesh.fill_face_normals();
        }

        let tris: usize = parts.iter().map(|p| p.mesh.tris()).sum();
        {
            let mut s = String::new();
            s.push_str("showcase: ");
            s.push_str(&u64_str(tris as u64));
            s.push_str(" triangles, ");
            s.push_str(&u64_str(parts.len() as u64));
            s.push_str(" draw calls, textures painted in ");
            s.push_str(&u64_str(paint_ms));
            s.push_str(" ms");
            say(&s);
        }

        // ---- loop
        let mut frames: u64 = 0;
        let mut samples: Vec<u32> = Vec::with_capacity(2048);
        let mut t = 0.0_f32;
        let mut last = clock::monotonic_nanos();
        let cap = if quick { 30 } else { u64::MAX };

        loop {
            let now = clock::monotonic_nanos();
            let mut dt = (now.saturating_sub(last)) as f32 / 1_000_000_000.0;
            last = now;
            if !(0.0..=0.1).contains(&dt) {
                dt = 1.0 / 60.0;
            }
            t += dt;

            // A slow orbit, because the way light moves across a surface as
            // the camera turns is most of what says "this is lit" rather than
            // "this is coloured".
            let a = t * 0.16;
            let eye = [
                cos_approx(a) * 46.0,
                14.0 + sin_approx(t * 0.11) * 5.0,
                sin_approx(a) * 46.0,
            ];
            let look = [0.0, hero_y + 7.0, 0.0];

            let frame_start = clock::monotonic_nanos();
            if scene3d::clear(scene, rgb(0.42, 0.56, 0.78)).is_err() {
                break;
            }
            if scene3d::camera(scene, &eye, &look, 56.0).is_err() {
                break;
            }
            // Low sun, matching where the sky texture paints one.
            // Low and raking rather than overhead. A light from above lights
            // every upward face equally and flattens the scene; a low one
            // makes the sides of things differ from their tops, which is what
            // gives objects form under a single-light model.
            if scene3d::light(scene, &[-0.62, -0.34, -0.70]).is_err() {
                break;
            }
            let mut failed = false;
            for p in &parts {
                if scene3d::smooth(
                    scene,
                    &p.mesh.verts,
                    &p.mesh.normals,
                    &p.mesh.uvs,
                    p.texture,
                    p.tint,
                )
                .is_err()
                {
                    failed = true;
                    break;
                }
            }
            if failed {
                say("showcase: a draw call failed, stopping");
                break;
            }
            let submit = clock::monotonic_nanos().saturating_sub(frame_start);
            if scene3d::present(scene).is_err() {
                break;
            }
            samples.push((submit / 1_000) as u32);

            frames += 1;
            if frames >= cap {
                break;
            }
            if let Some(types::Event::CloseRequested(id)) = events::poll() {
                let _ = window::close(id);
                break;
            }
        }

        if !samples.is_empty() {
            samples.sort_unstable();
            let pick = |q: f32| samples[((samples.len() - 1) as f32 * q) as usize];
            let mut s = String::new();
            s.push_str("showcase: frames ");
            s.push_str(&u64_str(frames));
            s.push_str("  submit us p50 ");
            s.push_str(&u64_str(pick(0.50) as u64));
            s.push_str(" p95 ");
            s.push_str(&u64_str(pick(0.95) as u64));
            s.push_str(" p99 ");
            s.push_str(&u64_str(pick(0.99) as u64));
            say(&s);
        }
        0
    }
}

krate::export!(Component);
