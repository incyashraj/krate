//! The same scene through both backends, compared pixel by pixel.
//!
//! This is what makes "parity" a claim rather than a hope. The GPU shader was
//! written to reproduce the CPU rasterizer's choices exactly -- the two-sided
//! face term, the one-sided term when normals are supplied, both ambient
//! floors, the clamp after the tint multiply, the camera basis, the depth
//! convention -- and the only way to know it does is to render the same thing
//! twice and subtract.
//!
//! Exact equality is not the bar and should not be. A hardware rasterizer
//! fills a pixel by its own coverage rule, samples a texture with its own
//! filter, and interpolates in its own order; a software one does all three
//! differently. What must agree is the PICTURE: the same shapes in the same
//! places at the same brightness, with disagreement confined to edges.

use krate_adapter_common::ui::ImagePixels;
use krate_runtime::scene3d::Scene as CpuScene;
use krate_scene3d_gpu::GpuScene;

const W: u32 = 128;
const H: u32 = 128;

fn pixel(image: &ImagePixels, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * image.width + x) * 4) as usize;
    [
        image.rgba[i],
        image.rgba[i + 1],
        image.rgba[i + 2],
        image.rgba[i + 3],
    ]
}

/// How far apart two renderings are, ignoring the edges of shapes.
///
/// Edges are where the two rasterizers are ENTITLED to disagree: a pixel whose
/// centre falls on a triangle boundary is in or out by a rule each backend
/// picks for itself, and the CPU path's own comment records that even two
/// evaluation orders of the same expression disagree on 16.5% of exactly-on
/// points. Counting those would measure the coverage rule rather than the
/// shading, so a pixel is only compared when its neighbours on the CPU side
/// agree with it -- which is to say, when it is in the middle of a surface.
fn interior_difference(a: &ImagePixels, b: &ImagePixels) -> (f64, u32, [u8; 4], [u8; 4]) {
    let mut total = 0u64;
    let mut counted = 0u32;
    let mut worst = 0u32;
    let mut worst_pair = ([0u8; 4], [0u8; 4]);
    for y in 1..H - 1 {
        for x in 1..W - 1 {
            let here = pixel(a, x, y);
            let flat = [(1u32, 0u32), (0, 1)].iter().all(|(dx, dy)| {
                let l = pixel(a, x - dx, y - dy);
                let r = pixel(a, x + dx, y + dy);
                (0..3).all(|c| l[c].abs_diff(here[c]) < 12 && r[c].abs_diff(here[c]) < 12)
            });
            if !flat {
                continue;
            }
            let there = pixel(b, x, y);
            let diff = (0..3)
                .map(|c| u32::from(here[c].abs_diff(there[c])))
                .max()
                .unwrap_or(0);
            total += u64::from(diff);
            counted += 1;
            if diff > worst {
                worst = diff;
                worst_pair = (here, there);
            }
        }
    }
    let mean = if counted == 0 {
        0.0
    } else {
        total as f64 / f64::from(counted)
    };
    (mean, worst, worst_pair.0, worst_pair.1)
}

/// Build the same scene on both backends and compare.
fn compare(name: &str, build_cpu: impl Fn(&mut CpuScene), build_gpu: impl Fn(&mut GpuScene)) {
    let Some(mut gpu) = GpuScene::new(W, H) else {
        eprintln!("no GPU adapter on this machine; skipping");
        return;
    };
    let mut cpu = CpuScene::new(W, H).expect("cpu scene");
    build_cpu(&mut cpu);
    build_gpu(&mut gpu);
    let cpu_image = cpu.render_image().expect("cpu frame");
    let gpu_image = gpu.render_image().expect("gpu frame");

    let (mean, worst, a, b) = interior_difference(&cpu_image, &gpu_image);
    eprintln!("{name}: interior mean {mean:.2}, worst {worst} (cpu {a:?} vs gpu {b:?})");
    assert!(
        mean < 12.0,
        "{name}: the two backends should agree across a surface -- mean \
         {mean:.2}, worst {worst}, cpu {a:?} vs gpu {b:?}"
    );
}

/// A quad facing the camera at a given depth, as two triangles.
fn facing_quad(half: f32, z: f32) -> Vec<f32> {
    vec![
        -half, -half, z, half, -half, z, half, half, z, //
        -half, -half, z, half, half, z, -half, half, z,
    ]
}

/// The CPU path's default clear colour, and the same value as floats.
const CLEAR_WORD: u32 = 0xFF10_1420;
const CLEAR_RGBA: [f32; 4] = [0.063, 0.078, 0.125, 1.0];

#[test]
fn a_flat_colour_surface_matches_between_backends() {
    compare(
        "flat colour",
        |cpu| {
            cpu.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
            cpu.set_light([0.0, 0.0, -1.0]);
            cpu.clear(CLEAR_WORD);
            cpu.triangles(&facing_quad(2.0, 0.0), (0.8, 0.3, 0.4, 1.0));
        },
        |gpu| {
            gpu.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
            gpu.set_light([0.0, 0.0, -1.0]);
            gpu.clear(CLEAR_RGBA);
            gpu.triangles(&facing_quad(2.0, 0.0), [0.8, 0.3, 0.4, 1.0]);
        },
    );
}

#[test]
fn depth_ordering_matches_between_backends() {
    // Far queued first, near second, so only the depth test decides.
    compare(
        "depth",
        |cpu| {
            cpu.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
            cpu.set_light([0.0, 0.0, -1.0]);
            cpu.clear(CLEAR_WORD);
            cpu.triangles(&facing_quad(2.5, -2.0), (0.9, 0.2, 0.2, 1.0));
            cpu.triangles(&facing_quad(1.5, 2.0), (0.2, 0.8, 0.3, 1.0));
        },
        |gpu| {
            gpu.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
            gpu.set_light([0.0, 0.0, -1.0]);
            gpu.clear(CLEAR_RGBA);
            gpu.triangles(&facing_quad(2.5, -2.0), [0.9, 0.2, 0.2, 1.0]);
            gpu.triangles(&facing_quad(1.5, 2.0), [0.2, 0.8, 0.3, 1.0]);
        },
    );
}

#[test]
fn a_lit_angled_surface_matches_between_backends() {
    // The shading term itself, on a surface the light strikes at an angle --
    // which is where an ambient floor or a sign error would show.
    let slanted: Vec<f32> = vec![
        -2.0, -2.0, 1.5, 2.0, -2.0, -1.5, 2.0, 2.0, -1.5, //
        -2.0, -2.0, 1.5, 2.0, 2.0, -1.5, -2.0, 2.0, 1.5,
    ];
    compare(
        "angled and lit",
        |cpu| {
            cpu.set_camera([0.0, 0.0, 7.0], [0.0, 0.0, 0.0], 60.0);
            cpu.set_light([-0.5, -0.6, -0.6]);
            cpu.clear(CLEAR_WORD);
            cpu.triangles(&slanted, (0.7, 0.7, 0.75, 1.0));
        },
        |gpu| {
            gpu.set_camera([0.0, 0.0, 7.0], [0.0, 0.0, 0.0], 60.0);
            gpu.set_light([-0.5, -0.6, -0.6]);
            gpu.clear(CLEAR_RGBA);
            gpu.triangles(&slanted, [0.7, 0.7, 0.75, 1.0]);
        },
    );
}

#[test]
fn smooth_shading_matches_between_backends() {
    // Corner normals and the one-sided light term, which is where the two
    // shading paths could most easily drift apart.
    let quad = facing_quad(2.0, 0.0);
    let normals: Vec<f32> = core::iter::repeat_n([0.2_f32, 0.3, 0.93], 6)
        .flatten()
        .collect();
    let uvs: Vec<f32> = vec![0.5; 12];
    compare(
        "smooth",
        |cpu| {
            cpu.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
            cpu.set_light([0.0, 0.0, -1.0]);
            cpu.clear(CLEAR_WORD);
            let white = cpu
                .upload_texture(1, 1, &[255, 255, 255, 255])
                .expect("texture");
            cpu.smooth(&quad, &normals, &uvs, white, (0.8, 0.5, 0.3, 1.0));
        },
        |gpu| {
            gpu.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
            gpu.set_light([0.0, 0.0, -1.0]);
            gpu.clear(CLEAR_RGBA);
            let white = gpu
                .upload_texture(1, 1, &[255, 255, 255, 255])
                .expect("texture");
            gpu.smooth(&quad, &normals, &uvs, white, [0.8, 0.5, 0.3, 1.0]);
        },
    );
}

#[test]
fn an_over_bright_tint_is_clamped_the_same_way() {
    // An app can pass a tint above 1.0 -- the showcase's sky dome passes 2.6,
    // trying to defeat the ambient floor. The CPU clamps the TINT to 0..1
    // before multiplying and then clamps the product again; clamping only the
    // product lets the tint brighten instead, and the GPU did exactly that
    // until this test existed. The sky came out a saturated white sheet where
    // the CPU showed a gradient.
    let quad = facing_quad(2.0, 0.0);
    let uvs: Vec<f32> = vec![0.5; 12];
    compare(
        "over-bright tint",
        |cpu| {
            cpu.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
            cpu.set_light([0.0, 0.0, -1.0]);
            cpu.clear(CLEAR_WORD);
            let grey = cpu
                .upload_texture(1, 1, &[120, 130, 150, 255])
                .expect("texture");
            cpu.textured(&quad, &uvs, grey, (2.6, 2.6, 2.6, 1.0));
        },
        |gpu| {
            gpu.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
            gpu.set_light([0.0, 0.0, -1.0]);
            gpu.clear(CLEAR_RGBA);
            let grey = gpu
                .upload_texture(1, 1, &[120, 130, 150, 255])
                .expect("texture");
            gpu.textured(&quad, &uvs, grey, [2.6, 2.6, 2.6, 1.0]);
        },
    );
}

#[test]
fn blending_matches_between_backends() {
    // An opaque wall with a half-covering pane in front: the blend arithmetic
    // and the back-to-front ordering, both at once.
    compare(
        "blend",
        |cpu| {
            cpu.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
            cpu.set_light([0.0, 0.0, -1.0]);
            cpu.clear(CLEAR_WORD);
            cpu.triangles(&facing_quad(2.5, -1.0), (0.9, 0.2, 0.2, 1.0));
            cpu.triangles(&facing_quad(1.6, 2.0), (0.2, 0.3, 0.9, 0.5));
        },
        |gpu| {
            gpu.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
            gpu.set_light([0.0, 0.0, -1.0]);
            gpu.clear(CLEAR_RGBA);
            gpu.triangles(&facing_quad(2.5, -1.0), [0.9, 0.2, 0.2, 1.0]);
            gpu.triangles(&facing_quad(1.6, 2.0), [0.2, 0.3, 0.9, 0.5]);
        },
    );
}
