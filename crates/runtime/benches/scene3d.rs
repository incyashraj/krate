//! How fast the software 3D renderer actually draws.
//!
//! Written because the frame rates published on the reports page came from
//! ad-hoc runs, went stale within a day, and could not be re-checked without
//! somebody remembering how they were produced. Timing the shipped app instead
//! does not work either: it renders thirty frames and counts its own startup,
//! which on this machine spreads the answer across 1463 to 2378 fps -- a range
//! too wide to publish.
//!
//! This draws the same scene the `krate-cubes` app does -- nine placed cubes on
//! a floor, one directional light -- and measures only the drawing.

use criterion::{criterion_group, criterion_main, Criterion};
use krate_runtime::scene3d::Scene;
use std::hint::black_box;

/// The eight corners of a unit cube as twelve triangles, matching the app.
fn cube() -> Vec<f32> {
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
        [0, 2, 3],
        [5, 4, 7],
        [5, 7, 6],
        [4, 0, 3],
        [4, 3, 7],
        [1, 5, 6],
        [1, 6, 2],
        [3, 2, 6],
        [3, 6, 7],
        [4, 5, 1],
        [4, 1, 0],
    ];
    let mut mesh = Vec::with_capacity(108);
    for face in faces {
        for corner in face {
            mesh.extend_from_slice(&c[corner]);
        }
    }
    mesh
}

fn floor() -> Vec<f32> {
    vec![
        -6.0, -0.75, -6.0, 6.0, -0.75, -6.0, 6.0, -0.75, 6.0, -6.0, -0.75, -6.0, 6.0, -0.75, 6.0,
        -6.0, -0.75, 6.0,
    ]
}

/// One frame of the cubes scene, drawn and rasterized.
///
/// `render_image` is included because it is what a frame costs in practice:
/// the earlier threading work found that converting the finished frame was
/// costing more than the 3D itself, and a benchmark that skipped it would have
/// hidden exactly that.
fn draw_one_frame(scene: &mut Scene, mesh: &[f32], ground: &[f32], spin: f32) {
    scene.set_camera([5.7, 3.0, 4.0], [0.0, 0.0, 0.0], 60.0);
    scene.clear(0xFF0D_1220);
    scene.triangles(ground, (0.18, 0.2, 0.26, 1.0));
    for row in 0..3_i32 {
        for column in 0..3_i32 {
            let x = (column - 1) as f32 * 2.2;
            let z = (row - 1) as f32 * 2.2;
            let rate = 1.0 + (row * 3 + column) as f32 * 0.35;
            scene.place(
                mesh,
                [x, 0.0, z],
                [0.0, spin * rate, spin * 0.4],
                1.0,
                (
                    0.35 + 0.2 * column as f32,
                    0.55,
                    0.9 - 0.15 * row as f32,
                    1.0,
                ),
            );
        }
    }
    let _ = black_box(scene.render_image());
}

fn scene3d_frame(c: &mut Criterion) {
    let mesh = cube();
    let ground = floor();
    let mut group = c.benchmark_group("scene3d_frame");

    // 320x240 is what the sample app ships at; 640x480 is the resolution worth
    // quoting, because a stylised game at 320x240 is a 1995 screen and nobody
    // should read a headline number without knowing which one it refers to.
    for (label, width, height) in [("320x240", 320, 240), ("640x480", 640, 480)] {
        group.bench_function(label, |b| {
            let mut scene = Scene::new(width, height).expect("scene");
            let mut spin = 0.0_f32;
            b.iter(|| {
                spin += 1.0;
                draw_one_frame(&mut scene, &mesh, &ground, spin);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, scene3d_frame);
criterion_main!(benches);
