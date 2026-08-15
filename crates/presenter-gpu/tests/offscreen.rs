//! S1's judge: the GPU presenter against the CPU painter, pixel for pixel.
//!
//! Skips (rather than fails) on machines with no GPU adapter, because the
//! presenter's contract is "fall back, never block an app".

use krate_adapter_common::painter::{paint_placements_bitmap, PaintInteraction};
use krate_adapter_common::ui::{WidgetId, WidgetKind, WidgetPlacement};

fn place(kind: WidgetKind, id: u64, x: f32, y: f32, w: f32, h: f32) -> WidgetPlacement {
    WidgetPlacement {
        widget: WidgetId::new(id).expect("id"),
        kind,
        label: None,
        checked: None,
        value: None,
        selection: None,
        text_cursor: None,
        clip: None,
        role: None,
        pixels: None,
        clickable: false,
        x,
        y,
        width: w,
        height: h,
    }
}

/// The geometry corpus: every kind the S1 slice claims, at awkward
/// fractional positions so antialiasing differences surface.
fn corpus() -> Vec<WidgetPlacement> {
    let mut v = vec![
        place(WidgetKind::Button, 1, 8.0, 8.0, 96.0, 28.0),
        place(WidgetKind::TextField, 2, 8.0, 44.0, 140.0, 24.0),
        place(WidgetKind::TextArea, 3, 8.0, 76.0, 140.0, 48.0),
    ];
    let mut slider = place(WidgetKind::Slider, 4, 8.0, 132.0, 140.0, 20.0);
    slider.value = Some(0.3);
    v.push(slider);
    let mut progress = place(WidgetKind::Progress, 5, 8.0, 160.0, 140.0, 12.0);
    progress.value = Some(0.7);
    v.push(progress);
    let mut clipped = place(WidgetKind::Button, 6, 120.0, 180.0, 96.0, 28.0);
    clipped.clip = Some((0.0, 0.0, 160.0, 196.0));
    v.push(clipped);
    v
}

#[test]
fn gpu_matches_cpu_painter_on_geometry() {
    let mut gpu = match krate_presenter_gpu::OffscreenPresenter::new() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let (w, h) = (224u32, 224u32);
    let placements = corpus();
    let interaction = PaintInteraction {
        hovered: None,
        pressed: None,
    };

    for scale in [1.0f32, 1.5, 2.0] {
        let mut cpu = vec![0u32; (w * h) as usize];
        // The vector path is the shipped Windows/Linux picture, rounded
        // corners and all; the GPU must match IT, not the bitmap fallback.
        if !krate_adapter_common::vector_text::try_paint_placements(
            &mut cpu,
            w,
            h,
            scale,
            &placements,
            interaction,
        ) {
            paint_placements_bitmap(&mut cpu, w, h, scale, &placements, interaction);
        }
        let mut cache = krate_presenter_gpu::SceneCache::new();
        let scene = krate_presenter_gpu::build_scene(&mut cache, &placements, scale, interaction);
        let gpu_px = gpu.render(&scene, w, h).expect("render");

        // The CPU painter fills whole pixels; vello antialiases fractional
        // edges, and at non-integer scales every edge lands mid-pixel. So
        // the judgment is three-part, and together they distinguish "soft
        // edges" from "wrong geometry":
        //
        //  1. INTERIORS EXACT: the centre of every widget must match the CPU
        //     painter to the byte. A displaced or missing rect fails here.
        //  2. BIG DELTAS RARE: pixels that disagree strongly (>60) must be
        //     under 0.5% -- blends at edges disagree mildly, wrong fills
        //     disagree loudly.
        //  3. ANY DELTAS BOUNDED: total disagreement stays under 4%.
        let probe = |x: f32, y: f32| -> (usize, u32) {
            let (px, py) = (
                ((x * scale) as usize).min(w as usize - 1),
                ((y * scale) as usize).min(h as usize - 1),
            );
            let i = py * w as usize + px;
            (i, cpu[i])
        };
        for pl in &placements {
            let (i, c) = probe(pl.x + pl.width / 2.0, pl.y + pl.height / 2.0);
            let (cr, cg, cb) = ((c >> 16) as u8, (c >> 8) as u8, c as u8);
            assert_eq!(
                (gpu_px[i * 4], gpu_px[i * 4 + 1], gpu_px[i * 4 + 2]),
                (cr, cg, cb),
                "scale {scale}: interior of widget {:?} at centre does not match",
                pl.kind
            );
        }
        let (mut mild, mut loud) = (0usize, 0usize);
        for i in 0..(w * h) as usize {
            let c = cpu[i];
            let (cr, cg, cb) = ((c >> 16) as u8, (c >> 8) as u8, c as u8);
            let (gr, gg, gb) = (gpu_px[i * 4], gpu_px[i * 4 + 1], gpu_px[i * 4 + 2]);
            let delta = cr.abs_diff(gr).max(cg.abs_diff(gg)).max(cb.abs_diff(gb));
            if delta > 12 {
                mild += 1;
            }
            if delta > 60 {
                loud += 1;
            }
        }
        let total = f64::from(w * h);
        assert!(
            (loud as f64) / total < 0.005,
            "scale {scale}: {loud} strongly differing pixels -- wrong fills, not edge softness"
        );
        assert!(
            (mild as f64) / total < 0.04,
            "scale {scale}: {mild} differing pixels -- beyond an antialiasing halo"
        );
    }
}

/// Text: same parley on both sides, so shaping is identical and only the
/// rasterizer differs. Judged as ink -- amount and bounding box -- against
/// the CPU vector path, which is the shipped Windows/Linux reference.
#[test]
fn gpu_text_matches_cpu_vector_ink() {
    let mut gpu = match krate_presenter_gpu::OffscreenPresenter::new() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let (w, h) = (240u32, 60u32);
    let mut button = place(WidgetKind::Button, 1, 10.0, 10.0, 180.0, 32.0);
    button.label = Some("Flip the coin".to_string());
    let placements = vec![button];
    let interaction = PaintInteraction {
        hovered: None,
        pressed: None,
    };

    let mut cpu = vec![0u32; (w * h) as usize];
    let vector_ok = krate_adapter_common::vector_text::try_paint_placements(
        &mut cpu,
        w,
        h,
        1.0,
        &placements,
        interaction,
    );
    if !vector_ok {
        eprintln!("skipping: no usable system fonts for the CPU vector reference");
        return;
    }
    let mut cache = krate_presenter_gpu::SceneCache::new();
    let scene = krate_presenter_gpu::build_scene(&mut cache, &placements, 1.0, interaction);
    let gpu_px = gpu.render(&scene, w, h).expect("render");

    // Ink = pixels meaningfully darker than the button fill inside the
    // button rect. Compare counts and horizontal extents.
    let ink = |get: &dyn Fn(usize) -> (u8, u8, u8)| -> (usize, u32, u32) {
        let (mut count, mut min_x, mut max_x) = (0usize, u32::MAX, 0u32);
        // Scan well inside the button, clear of its antialiased rim: blends
        // toward the bright page background along every edge read as "bright
        // non-neutral" and masquerade as ink.
        for y in 16..36u32 {
            for x in 16..184u32 {
                let (r, g, b) = get((y * w + x) as usize);
                // The label is white antialiased into blue; the page
                // background (exactly 242,242,242) shows through the CPU
                // reference's rounded corners inside this scan rect. Ink is
                // therefore "bright, and not that exact neutral": corners are
                // excluded by value, AA'd glyph pixels stay.
                let corner = r == g && g == b && (236..=248).contains(&r);
                if r > 200 && g > 200 && b > 200 && !corner {
                    count += 1;
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        (count, min_x, max_x)
    };
    let (c_count, c_min, c_max) = ink(&|i| {
        let c = cpu[i];
        ((c >> 16) as u8, (c >> 8) as u8, c as u8)
    });
    let (g_count, g_min, g_max) = ink(&|i| (gpu_px[i * 4], gpu_px[i * 4 + 1], gpu_px[i * 4 + 2]));

    assert!(c_count > 50, "reference produced no text ink");
    assert!(g_count > 50, "GPU produced no text ink");
    let ratio = g_count as f64 / c_count as f64;
    assert!(
        (0.6..=1.7).contains(&ratio),
        "ink amount diverged: cpu {c_count}, gpu {g_count}"
    );
    assert!(
        (i64::from(c_min) - i64::from(g_min)).abs() <= 4
            && (i64::from(c_max) - i64::from(g_max)).abs() <= 4,
        "text sits in a different place: cpu {c_min}..{c_max}, gpu {g_min}..{g_max}"
    );
}

/// Images: fit-scale-centre math is copied from the CPU painter, so interior
/// pixels must match closely.
#[test]
fn gpu_image_matches_cpu_painter() {
    let mut gpu = match krate_presenter_gpu::OffscreenPresenter::new() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let (w, h) = (120u32, 120u32);
    // A 2x2 quadrant image scaled up: solid interiors, obvious placement.
    let pixels = krate_adapter_common::ui::ImagePixels::new(
        2,
        2,
        vec![
            255, 0, 0, 255, /**/ 0, 255, 0, 255, //
            0, 0, 255, 255, /**/ 255, 255, 0, 255,
        ],
    )
    .expect("pixels");
    let mut image = place(WidgetKind::Image, 9, 10.0, 10.0, 100.0, 100.0);
    image.pixels = Some(std::sync::Arc::new(pixels));
    let placements = vec![image];
    let interaction = PaintInteraction {
        hovered: None,
        pressed: None,
    };

    let mut cpu = vec![0u32; (w * h) as usize];
    paint_placements_bitmap(&mut cpu, w, h, 1.0, &placements, interaction);
    let mut cache = krate_presenter_gpu::SceneCache::new();
    let scene = krate_presenter_gpu::build_scene(&mut cache, &placements, 1.0, interaction);
    let gpu_px = gpu.render(&scene, w, h).expect("render");

    // Probe the centre of each quadrant.
    for (x, y, want) in [
        (35u32, 35u32, (255u8, 0u8, 0u8)),
        (85, 35, (0, 255, 0)),
        (35, 85, (0, 0, 255)),
        (85, 85, (255, 255, 0)),
    ] {
        let i = (y * w + x) as usize;
        let c = cpu[i];
        assert_eq!(
            ((c >> 16) as u8, (c >> 8) as u8, c as u8),
            want,
            "CPU reference drew the quadrant wrong at {x},{y}"
        );
        let got = (gpu_px[i * 4], gpu_px[i * 4 + 1], gpu_px[i * 4 + 2]);
        let d = got
            .0
            .abs_diff(want.0)
            .max(got.1.abs_diff(want.1))
            .max(got.2.abs_diff(want.2));
        assert!(
            d <= 8,
            "GPU quadrant at {x},{y}: want {want:?}, got {got:?}"
        );
    }
}
