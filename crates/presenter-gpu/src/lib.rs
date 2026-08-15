//! The GPU presenter: placements in, vsynced frames out.
//!
//! Stage S1 of Plan/GPU-Presenter.md. This crate owns exactly two things:
//!
//! 1. [`build_scene`]: the one placements-to-vello-Scene translation, ported
//!    from the proven iOS path (`adapter-ios/src/vello_canvas.rs`) so every
//!    platform that gains the GPU presenter draws identically.
//! 2. [`OffscreenPresenter`]: a headless wgpu render-to-texture with PNG
//!    readback. It exists so every visual claim this presenter ever makes is
//!    checkable by image diff against the CPU painter on a Mac, before any
//!    Windows hardware is involved.
//!
//! The windowed surface (S2) builds on the same `build_scene`; nothing in
//! this crate may know about winit, so the seam stays a seam.

use krate_adapter_common::painter::PaintInteraction;
use krate_adapter_common::ui::WidgetPlacement;
use vello::kurbo::{Affine, Rect};
use vello::peniko::{Color, Fill};

/// Background matches the CPU painter's clear color, so image diffs compare
/// content rather than conventions.
const BACKGROUND: Color = Color::from_rgb8(0x0a, 0x0a, 0x0a);

/// Translate placements into a vello scene at the given scale.
///
/// Logical-to-physical happens here and only here, mirroring the CPU
/// painter's contract: placements arrive logical, the scene is physical.
pub fn build_scene(
    placements: &[WidgetPlacement],
    scale: f32,
    _interaction: PaintInteraction,
) -> vello::Scene {
    let mut scene = vello::Scene::new();
    let s = f64::from(scale);
    for placement in placements {
        let rect = Rect::new(
            f64::from(placement.x) * s,
            f64::from(placement.y) * s,
            f64::from(placement.x + placement.width) * s,
            f64::from(placement.y + placement.height) * s,
        );
        // S1 draws structure: fills for every placement kind, so the diff
        // harness has real geometry to judge. Text, images, canvas pixels
        // and per-kind styling port from vello_canvas.rs next, behind this
        // same signature.
        scene.fill(
            Fill::NonZero,
            Affine::IDENTITY,
            Color::from_rgb8(0x2a, 0x2d, 0x33),
            None,
            &rect,
        );
    }
    scene
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
                    base_color: BACKGROUND,
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
