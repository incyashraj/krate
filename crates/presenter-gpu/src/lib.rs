//! The GPU presenter: placements in, vsynced frames out.
//!
//! Stage S1 of Plan/GPU-Presenter.md. This crate owns exactly two things:
//!
//! 1. [`build_scene`]: the placements-to-vello-Scene translation. Geometry
//!    and per-kind styling are ported branch-for-branch from the CPU bitmap
//!    painter and share its color constants, so the two backends cannot
//!    drift in palette. Text and images are the next slice: the GPU vello
//!    line pairs parley 0.7 while the CPU line is on 0.11, so glyph runs
//!    need their own shaping here, the way adapter-ios does it.
//! 2. [`OffscreenPresenter`]: headless wgpu render-to-texture with readback,
//!    so every visual claim is checkable by image diff against the CPU
//!    painter on this Mac before any Windows hardware is involved.
//!
//! Nothing in this crate may know about winit; the windowed surface (S2)
//! builds on the same `build_scene` behind that seam.

use std::collections::HashMap;

use krate_adapter_common::painter::{
    button_fill_color, PaintInteraction, COLOR_BACKGROUND, COLOR_BUTTON, COLOR_BUTTON_LABEL,
    COLOR_FIELD_BORDER, COLOR_FIELD_FILL, COLOR_FIELD_TEXT, COLOR_IMAGE_BACKDROP, COLOR_KNOB,
    COLOR_TEXT, COLOR_TRACK,
};
use krate_adapter_common::ui::{ImagePixels, WidgetKind, WidgetPlacement};
use parley::{
    Alignment, AlignmentOptions, FontContext, Layout, LayoutContext, PositionedLayoutItem,
};
use vello::kurbo::{Affine, Rect};
use vello::peniko::{Blob, Color, Fill, ImageAlphaType, ImageBrush, ImageData, ImageFormat};

/// Matches the CPU vector path's LABEL_FONT_SIZE: the two paths share parley,
/// so equal size means equal layout. Change them together.
const LABEL_FONT_SIZE: f32 = 13.0;

/// Convert the painter's 0xAARRGGBB into a vello color, so there is exactly
/// one palette in the codebase.
fn argb(color: u32) -> Color {
    Color::from_rgba8(
        (color >> 16) as u8,
        (color >> 8) as u8,
        color as u8,
        (color >> 24) as u8,
    )
}

fn intersect(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> Option<(f32, f32, f32, f32)> {
    let x = a.0.max(b.0);
    let y = a.1.max(b.1);
    let r = (a.0 + a.2).min(b.0 + b.2);
    let btm = (a.1 + a.3).min(b.1 + b.3);
    (r > x && btm > y).then_some((x, y, r - x, btm - y))
}

fn rect(r: (f32, f32, f32, f32)) -> Rect {
    Rect::new(
        f64::from(r.0),
        f64::from(r.1),
        f64::from(r.0 + r.2),
        f64::from(r.1 + r.3),
    )
}

/// Per-presenter state that outlives a frame: shaped-text contexts and
/// uploaded images. Images are keyed by their pixel buffer's address, so a
/// canvas that pushes a new frame re-uploads and a static logo uploads once
/// -- re-uploading per frame is the classic port mistake and is the K-062
/// family.
#[derive(Default)]
pub struct SceneCache {
    font_cx: Option<FontContext>,
    layout_cx: LayoutContext<()>,
    images: HashMap<usize, ImageBrush>,
}

impl SceneCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn layout(&mut self, text: &str, scale: f32, max_width: Option<f32>) -> Layout<()> {
        let font_cx = self.font_cx.get_or_insert_with(FontContext::new);
        let mut builder = self.layout_cx.ranged_builder(font_cx, text, scale, true);
        builder.push_default(parley::GenericFamily::SansSerif);
        builder.push_default(parley::StyleProperty::FontSize(LABEL_FONT_SIZE));
        let mut layout = builder.build(text);
        layout.break_all_lines(max_width);
        layout.align(Alignment::Start, AlignmentOptions::default());
        layout
    }

    fn image(&mut self, pixels: &ImagePixels) -> ImageBrush {
        let key = pixels.rgba.as_ptr() as usize;
        self.images
            .entry(key)
            .or_insert_with(|| {
                ImageBrush::new(ImageData {
                    data: Blob::from(pixels.rgba.clone()),
                    format: ImageFormat::Rgba8,
                    alpha_type: ImageAlphaType::Alpha,
                    width: pixels.width,
                    height: pixels.height,
                })
            })
            .clone()
    }
}

/// Draw a shaped layout into the scene at (x, y) physical, one glyph run at
/// a time -- the same iteration the CPU vector path uses, aimed at the GPU.
fn draw_layout(scene: &mut vello::Scene, layout: &Layout<()>, color: u32, x: f32, y: f32) {
    for line in layout.lines() {
        for item in line.items() {
            let PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                continue;
            };
            let mut run_x = glyph_run.offset();
            let run_y = glyph_run.baseline();
            let glyphs: Vec<vello::Glyph> = glyph_run
                .glyphs()
                .map(|g| {
                    let gx = x + run_x + g.x;
                    let gy = y + run_y - g.y;
                    run_x += g.advance;
                    vello::Glyph {
                        id: u32::from(g.id),
                        x: gx,
                        y: gy,
                    }
                })
                .collect();
            let run = glyph_run.run();
            scene
                .draw_glyphs(run.font())
                .font_size(run.font_size())
                .normalized_coords(run.normalized_coords())
                .brush(argb(color))
                .transform(Affine::IDENTITY)
                .draw(Fill::NonZero, glyphs.into_iter());
        }
    }
}

/// Draw a picture fit-scaled and centred in a rect: the same math as the CPU
/// painter's draw_image, so the two paths frame a photo identically.
fn draw_image_fit(
    scene: &mut vello::Scene,
    image: &ImageBrush,
    (rx, ry, rw, rh): (f32, f32, f32, f32),
) {
    if image.image.width == 0 || image.image.height == 0 || rw <= 0.0 || rh <= 0.0 {
        return;
    }
    let scale = (rw / image.image.width as f32).min(rh / image.image.height as f32);
    let (dw, dh) = (
        image.image.width as f32 * scale,
        image.image.height as f32 * scale,
    );
    let (ox, oy) = (rx + (rw - dw) / 2.0, ry + (rh - dh) / 2.0);
    let transform =
        Affine::translate((f64::from(ox), f64::from(oy))) * Affine::scale(f64::from(scale));
    scene.draw_image(image, transform);
}

fn fill_clipped(
    scene: &mut vello::Scene,
    color: u32,
    r: (f32, f32, f32, f32),
    clip: Option<(f32, f32, f32, f32)>,
) {
    fill_rounded_clipped(scene, color, r, clip, 0.0);
}

/// Rounded fill with the same clip discipline. Radii mirror the CPU vector
/// path (buttons 6, field chrome 4 over 3, all x scale), because that path
/// is what Windows and Linux users have been looking at -- the GPU must land
/// on the same picture, not the bitmap fallback's squares.
fn fill_rounded_clipped(
    scene: &mut vello::Scene,
    color: u32,
    r: (f32, f32, f32, f32),
    clip: Option<(f32, f32, f32, f32)>,
    radius: f32,
) {
    let clipped = match clip {
        Some(c) => intersect(r, c),
        None => Some(r),
    };
    let Some(r) = clipped else { return };
    if radius <= 0.0 {
        scene.fill(Fill::NonZero, Affine::IDENTITY, argb(color), None, &rect(r));
    } else {
        let shape = vello::kurbo::RoundedRect::from_rect(rect(r), f64::from(radius));
        scene.fill(Fill::NonZero, Affine::IDENTITY, argb(color), None, &shape);
    }
}

/// Translate placements into a vello scene at the given scale.
///
/// Logical-to-physical happens here and only here, mirroring the CPU
/// painter's contract: placements arrive logical, the scene is physical.
/// The branch structure follows `paint_placements_bitmap` deliberately --
/// when that painter changes, this file changes in the same review.
pub fn build_scene(
    cache: &mut SceneCache,
    placements: &[WidgetPlacement],
    scale: f32,
    interaction: PaintInteraction,
) -> vello::Scene {
    let mut scene = vello::Scene::new();

    for placement in placements {
        let (px, py) = (placement.x * scale, placement.y * scale);
        let (pw, ph) = (placement.width * scale, placement.height * scale);
        let clip = placement
            .clip
            .map(|(cx, cy, cw, ch)| (cx * scale, cy * scale, cw * scale, ch * scale));
        if let Some(c) = clip {
            if intersect((px, py, pw, ph), c).is_none() {
                continue;
            }
        }
        match placement.kind {
            WidgetKind::Button => {
                fill_rounded_clipped(
                    &mut scene,
                    button_fill_color(placement.widget, interaction),
                    (px, py, pw, ph),
                    clip,
                    6.0 * scale,
                );
                if let Some(label) = placement.label.as_deref().filter(|l| !l.is_empty()) {
                    let layout = cache.layout(label, scale, None);
                    let (lw, lh) = (layout.width(), layout.height());
                    draw_layout(
                        &mut scene,
                        &layout,
                        COLOR_BUTTON_LABEL,
                        px + (pw - lw) / 2.0,
                        py + (ph - lh) / 2.0,
                    );
                }
            }
            WidgetKind::TextField | WidgetKind::TextArea => {
                fill_rounded_clipped(
                    &mut scene,
                    COLOR_FIELD_BORDER,
                    (px, py, pw, ph),
                    clip,
                    4.0 * scale,
                );
                fill_rounded_clipped(
                    &mut scene,
                    COLOR_FIELD_FILL,
                    (
                        px + scale,
                        py + scale,
                        (pw - 2.0 * scale).max(0.0),
                        (ph - 2.0 * scale).max(0.0),
                    ),
                    clip,
                    3.0 * scale,
                );
                if let Some(label) = placement.label.as_deref().filter(|l| !l.is_empty()) {
                    let inset = 4.0 * scale;
                    let wrap = (placement.kind == WidgetKind::TextArea)
                        .then_some((pw - inset * 2.0).max(1.0));
                    let layout = cache.layout(label, scale, wrap);
                    let ly = if placement.kind == WidgetKind::TextArea {
                        py + inset
                    } else {
                        py + (ph - layout.height()) / 2.0
                    };
                    draw_layout(&mut scene, &layout, COLOR_FIELD_TEXT, px + inset, ly);
                }
            }
            WidgetKind::Slider | WidgetKind::Progress => {
                let fraction = placement.value.unwrap_or(0.0).clamp(0.0, 1.0);
                let groove_h = if placement.kind == WidgetKind::Slider {
                    4.0 * scale
                } else {
                    6.0 * scale
                };
                let gy = py + (ph - groove_h) / 2.0;
                fill_clipped(&mut scene, COLOR_TRACK, (px, gy, pw, groove_h), clip);
                fill_clipped(
                    &mut scene,
                    COLOR_BUTTON,
                    (px, gy, pw * fraction, groove_h),
                    clip,
                );
                if placement.kind == WidgetKind::Slider {
                    let thumb = (16.0 * scale).min(ph);
                    let tx = px + (pw - thumb) * fraction;
                    let ty = py + (ph - thumb) / 2.0;
                    fill_clipped(&mut scene, COLOR_FIELD_BORDER, (tx, ty, thumb, thumb), clip);
                    fill_clipped(
                        &mut scene,
                        COLOR_KNOB,
                        (
                            tx + scale,
                            ty + scale,
                            (thumb - 2.0 * scale).max(0.0),
                            (thumb - 2.0 * scale).max(0.0),
                        ),
                        clip,
                    );
                }
            }
            WidgetKind::Text => {
                if let Some(label) = placement.label.as_deref().filter(|l| !l.is_empty()) {
                    let layout = cache.layout(label, scale, None);
                    draw_layout(
                        &mut scene,
                        &layout,
                        COLOR_TEXT,
                        px,
                        py + (ph - layout.height()) / 2.0,
                    );
                }
            }
            // Anything carrying pixels -- Image, Canvas, a 3D scene's frame --
            // draws a backdrop and the picture, fit-scaled and centred like
            // the CPU painter. This is the branch whose absence was the K-062
            // white screen; it exists here from day one.
            _ => {
                let backdrop = placement.kind == WidgetKind::Image || placement.pixels.is_some();
                if backdrop {
                    fill_clipped(&mut scene, COLOR_IMAGE_BACKDROP, (px, py, pw, ph), clip);
                }
                if let Some(pixels) = placement.pixels.as_deref() {
                    let image = cache.image(pixels);
                    draw_image_fit(&mut scene, &image, (px, py, pw, ph));
                }
            }
        }
    }
    scene
}

/// The painter's clear color, as vello sees it.
pub fn background() -> Color {
    argb(COLOR_BACKGROUND)
}

/// Render a scene headless and hand back RGBA bytes, for the golden-image
/// harness. Runs on any wgpu backend, which is what makes Windows behavior
/// provable from a Mac.
pub struct OffscreenPresenter {
    device: vello::wgpu::Device,
    queue: vello::wgpu::Queue,
    renderer: vello::Renderer,
}

impl OffscreenPresenter {
    pub fn new() -> Result<Self, String> {
        let instance = vello::wgpu::Instance::default();
        let adapter = pollster::block_on(
            instance.request_adapter(&vello::wgpu::RequestAdapterOptions::default()),
        )
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&vello::wgpu::DeviceDescriptor::default()))
                .map_err(|e| format!("no GPU device: {e}"))?;
        let renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
            .map_err(|e| format!("vello renderer: {e}"))?;
        Ok(Self {
            device,
            queue,
            renderer,
        })
    }

    pub fn render(
        &mut self,
        scene: &vello::Scene,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, String> {
        use vello::wgpu;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("krate-offscreen"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                scene,
                &view,
                &vello::RenderParams {
                    base_color: background(),
                    width,
                    height,
                    antialiasing_method: vello::AaConfig::Area,
                },
            )
            .map_err(|e| format!("render: {e}"))?;

        let bytes_per_row = (width * 4).next_multiple_of(256);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("krate-readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv()
            .map_err(|_| "readback lost".to_string())?
            .map_err(|e| format!("map: {e:?}"))?;
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * bytes_per_row) as usize;
            out.extend_from_slice(&data[start..start + (width * 4) as usize]);
        }
        Ok(out)
    }
}
pub mod present;
pub use present::WindowPresenter;
pub use vello;
