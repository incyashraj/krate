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
        paint_placements_bitmap(&mut cpu, w, h, scale, &placements, interaction);
        let scene = krate_presenter_gpu::build_scene(&placements, scale, interaction);
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
