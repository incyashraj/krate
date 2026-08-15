//! S4: the nova-shaped benchmark, headless.
//!
//! Forty moving sprites, a score line, a progress bar -- the workload of the
//! game users compared across machines -- rendered offscreen for 300 frames.
//! Offscreen means no vsync: this measures what the renderer can DO, while
//! the windowed stats (KRATE_FRAME_STATS=1) measure how it PACES. Both
//! numbers go to evidence/perf/ per platform per release.

use std::sync::Arc;
use std::time::Instant;

use krate_adapter_common::painter::PaintInteraction;
use krate_adapter_common::ui::{ImagePixels, WidgetId, WidgetKind, WidgetPlacement};

fn sprite() -> Arc<ImagePixels> {
    // A 32x32 radial blob, nova's projectile shape.
    let n = 32u32;
    let mut rgba = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let d = (dx * dx + dy * dy).sqrt() / 16.0;
            let a = ((1.0 - d).max(0.0) * 255.0) as u8;
            rgba.extend_from_slice(&[255, 140, 40, a]);
        }
    }
    Arc::new(ImagePixels::new(n, n, rgba).expect("sprite"))
}

fn main() {
    let (w, h) = (960u32, 640u32);
    let mut gpu = krate_presenter_gpu::OffscreenPresenter::new().expect("gpu");
    let mut cache = krate_presenter_gpu::SceneCache::new();
    let blob = sprite();
    let interaction = PaintInteraction {
        hovered: None,
        pressed: None,
    };

    let frames = 300u32;
    let mut times = Vec::with_capacity(frames as usize);
    for f in 0..frames {
        let t = f as f32 / 60.0;
        let mut placements = Vec::new();
        for i in 0..40u64 {
            let phase = i as f32 * 0.7 + t * (1.0 + (i % 7) as f32 * 0.3);
            let mut p = WidgetPlacement {
                widget: WidgetId::new(100 + i).unwrap(),
                kind: WidgetKind::Image,
                label: None,
                checked: None,
                value: None,
                selection: None,
                text_cursor: None,
                clip: None,
                role: None,
                pixels: Some(blob.clone()),
                clickable: false,
                x: 60.0 + (phase.sin() * 0.5 + 0.5) * 800.0,
                y: 40.0 + (phase.cos() * 0.5 + 0.5) * 520.0,
                width: 32.0,
                height: 32.0,
            };
            p.x = p.x.round();
            p.y = p.y.round();
            placements.push(p);
        }
        let mut score = WidgetPlacement {
            widget: WidgetId::new(1).unwrap(),
            kind: WidgetKind::Text,
            label: Some(format!("SCORE {f:05}")),
            checked: None,
            value: None,
            selection: None,
            text_cursor: None,
            clip: None,
            role: None,
            pixels: None,
            clickable: false,
            x: 16.0,
            y: 12.0,
            width: 200.0,
            height: 20.0,
        };
        placements.push(score.clone());
        score.widget = WidgetId::new(2).unwrap();
        score.kind = WidgetKind::Progress;
        score.label = None;
        score.value = Some((f % 60) as f32 / 60.0);
        score.y = 610.0;
        score.width = 928.0;
        placements.push(score);

        let started = Instant::now();
        let scene = krate_presenter_gpu::build_scene(&mut cache, &placements, 1.5, interaction);
        let px = gpu.render(&scene, w, h).expect("render");
        std::hint::black_box(&px);
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.total_cmp(b));
    let p = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
    println!(
        "bench-nova: {w}x{h} @1.5x, 40 sprites+text+bar, {frames} frames: p50 {:.2}ms p90 {:.2}ms p99 {:.2}ms (raw render+readback, no vsync)",
        p(0.5),
        p(0.9),
        p(0.99)
    );
}
