//! The windowed presenter: one implementation both winit adapters wrap.
//!
//! Generic over the window handle rather than depending on winit, so this
//! crate stays a renderer. The adapter passes size and scale each frame --
//! it owns the window; this owns the pictures.
//!
//! S3 lives here too: frame pacing and input-to-present latency are
//! measured at the only place every frame passes through. KRATE_FRAME_STATS=1
//! prints a rolling report; the numbers land in evidence/perf/ per release
//! (Plan/GPU-Presenter.md S4).

use std::time::Instant;

use krate_adapter_common::painter::PaintInteraction;
use krate_adapter_common::ui::WidgetPlacement;
use vello::wgpu;

use crate::{background, build_scene, SceneCache};

/// Rolling frame statistics, reported every `REPORT_EVERY` frames when
/// `KRATE_FRAME_STATS=1`.
struct FrameStats {
    enabled: bool,
    last_present: Option<Instant>,
    frame_ms: Vec<f32>,
    interval_ms: Vec<f32>,
    latency_ms: Vec<f32>,
}

const REPORT_EVERY: usize = 120;

impl FrameStats {
    fn new() -> Self {
        Self {
            enabled: std::env::var("KRATE_FRAME_STATS").as_deref() == Ok("1"),
            last_present: None,
            frame_ms: Vec::new(),
            interval_ms: Vec::new(),
            latency_ms: Vec::new(),
        }
    }

    fn record(&mut self, started: Instant, input_at: Option<Instant>) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        self.frame_ms.push(started.elapsed().as_secs_f32() * 1000.0);
        if let Some(last) = self.last_present {
            self.interval_ms
                .push(now.duration_since(last).as_secs_f32() * 1000.0);
        }
        self.last_present = Some(now);
        if let Some(input) = input_at {
            self.latency_ms
                .push(now.duration_since(input).as_secs_f32() * 1000.0);
        }
        if self.frame_ms.len() >= REPORT_EVERY {
            let p = |v: &mut Vec<f32>, q: f32| -> f32 {
                if v.is_empty() {
                    return 0.0;
                }
                v.sort_by(|a, b| a.total_cmp(b));
                v[((v.len() - 1) as f32 * q) as usize]
            };
            eprintln!(
                "krate-frames: render p50 {:.2}ms p99 {:.2}ms | present interval p50 {:.2}ms p99 {:.2}ms | input-to-present p50 {:.2}ms p99 {:.2}ms (n={})",
                p(&mut self.frame_ms, 0.5),
                p(&mut self.frame_ms, 0.99),
                p(&mut self.interval_ms, 0.5),
                p(&mut self.interval_ms, 0.99),
                p(&mut self.latency_ms, 0.5),
                p(&mut self.latency_ms, 0.99),
                self.frame_ms.len(),
            );
            self.frame_ms.clear();
            self.interval_ms.clear();
            self.latency_ms.clear();
        }
    }
}

/// Everything one window needs to present vello frames on its surface.
///
/// The render path is vello's documented one: render the scene to a storage
/// texture, then blit onto the acquired surface frame -- surfaces rarely
/// offer STORAGE_BINDING, so drawing to them directly is not portable.
/// Present mode is AutoVsync: pacing is half of what "feels native" means.
pub struct WindowPresenter {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: vello::Renderer,
    blitter: wgpu::util::TextureBlitter,
    surface_format: wgpu::TextureFormat,
    configured: (u32, u32),
    target: Option<(wgpu::TextureView, u32, u32)>,
    cache: SceneCache,
    stats: FrameStats,
}

impl WindowPresenter {
    pub fn new(window: impl Into<wgpu::SurfaceTarget<'static>>) -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window)
            .map_err(|e| format!("surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("adapter: {e}"))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| format!("device: {e}"))?;
        // One line of truth per window: which silicon and driver stack is
        // actually drawing. "The game lags" debugging starts by knowing
        // whether this ran on the GPU at all, and on which one.
        let info = adapter.get_info();
        eprintln!(
            "krate: GPU presenter on {} ({:?}, {:?})",
            info.name, info.backend, info.device_type
        );
        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(
                    f,
                    wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
                )
            })
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| "surface offers no formats".to_string())?;
        let renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
            .map_err(|e| format!("renderer: {e}"))?;
        let blitter = wgpu::util::TextureBlitter::new(&device, surface_format);
        Ok(Self {
            surface,
            device,
            queue,
            renderer,
            blitter,
            surface_format,
            configured: (0, 0),
            target: None,
            cache: SceneCache::new(),
            stats: FrameStats::new(),
        })
    }

    /// Render and present one frame. `Err` means the GPU path is done for
    /// this window and the caller falls back to the CPU painter.
    ///
    /// `input_at` is when the most recent user input arrived, for the
    /// input-to-present latency S3 measures.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        width: u32,
        height: u32,
        scale: f32,
        placements: &[WidgetPlacement],
        interaction: PaintInteraction,
        input_at: Option<Instant>,
    ) -> Result<(), String> {
        let started = Instant::now();
        if width == 0 || height == 0 {
            return Ok(());
        }
        if self.configured != (width, height) {
            self.surface.configure(
                &self.device,
                &wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: self.surface_format,
                    width,
                    height,
                    present_mode: wgpu::PresentMode::AutoVsync,
                    desired_maximum_frame_latency: 2,
                    alpha_mode: wgpu::CompositeAlphaMode::Auto,
                    view_formats: vec![],
                },
            );
            self.configured = (width, height);
            self.target = None;
        }
        if self.target.as_ref().map(|t| (t.1, t.2)) != Some((width, height)) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("krate-frame"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            self.target = Some((
                texture.create_view(&wgpu::TextureViewDescriptor::default()),
                width,
                height,
            ));
        }
        let scene = build_scene(&mut self.cache, placements, scale, interaction);
        let (target_view, _, _) = self.target.as_ref().expect("target just ensured");
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                &scene,
                target_view,
                &vello::RenderParams {
                    base_color: background(),
                    width,
                    height,
                    antialiasing_method: vello::AaConfig::Area,
                },
            )
            .map_err(|e| format!("render: {e}"))?;
        use wgpu::CurrentSurfaceTexture;
        let frame = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) | CurrentSurfaceTexture::Suboptimal(frame) => {
                frame
            }
            // Skippable states: nothing to show this frame, nothing wrong
            // with the path. Minimized windows sit in Occluded for their
            // whole quiet life; retiring the GPU for that would punish every
            // un-minimize.
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            // Outdated/lost: force a reconfigure next frame; if the surface
            // is truly gone the configure path will error and THAT retires
            // the GPU.
            _ => {
                self.configured = (0, 0);
                return Ok(());
            }
        };
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("krate-blit"),
            });
        self.blitter
            .copy(&self.device, &mut encoder, target_view, &frame_view);
        self.queue.submit([encoder.finish()]);
        frame.present();
        self.stats.record(started, input_at);
        Ok(())
    }
}
