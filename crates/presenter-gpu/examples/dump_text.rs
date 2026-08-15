//! Dev aid: dump the CPU-vector and GPU renders of the text corpus to PNGs.
use krate_adapter_common::painter::PaintInteraction;
use krate_adapter_common::ui::{WidgetId, WidgetKind, WidgetPlacement};

fn main() {
    let (w, h) = (240u32, 60u32);
    let button = WidgetPlacement {
        widget: WidgetId::new(1).unwrap(),
        kind: WidgetKind::Button,
        label: Some("Flip the coin".into()),
        checked: None,
        value: None,
        selection: None,
        text_cursor: None,
        clip: None,
        role: None,
        pixels: None,
        clickable: false,
        x: 10.0,
        y: 10.0,
        width: 180.0,
        height: 32.0,
    };
    let placements = vec![button];
    let interaction = PaintInteraction {
        hovered: None,
        pressed: None,
    };

    let mut cpu = vec![0u32; (w * h) as usize];
    let ok = krate_adapter_common::vector_text::try_paint_placements(
        &mut cpu,
        w,
        h,
        1.0,
        &placements,
        interaction,
    );
    eprintln!("vector path ok: {ok}");
    let rgba: Vec<u8> = cpu
        .iter()
        .flat_map(|c| [(c >> 16) as u8, (c >> 8) as u8, *c as u8, 255])
        .collect();
    save("/tmp/text-cpu.png", w, h, &rgba);

    let mut gpu = krate_presenter_gpu::OffscreenPresenter::new().unwrap();
    let mut cache = krate_presenter_gpu::SceneCache::new();
    let scene = krate_presenter_gpu::build_scene(&mut cache, &placements, 1.0, interaction);
    let px = gpu.render(&scene, w, h).unwrap();
    save("/tmp/text-gpu.png", w, h, &px);
}

fn save(path: &str, w: u32, h: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Rgba);
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(rgba).unwrap();
    eprintln!("wrote {path}");
}
