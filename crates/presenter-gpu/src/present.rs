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
    /// Set by the device's uncaptured-error handler. wgpu delivers most
    /// failures (swapchain creation included) asynchronously, and its
    /// DEFAULT handler panics the process -- which is how "no usable GPU"
    /// crashed an app on a GPU-less VM instead of falling back to the CPU
    /// painter. Checked after every render; sets the path to retire.
    device_failed: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
        // A software adapter is not a GPU. WARP and friends rasterize on the
        // CPU behind a DX12 mask -- slower than our own CPU painter, and on a
        // session with no display their swapchain creation fails anyway.
        // Declining here is what routes GPU-less machines to the fallback.
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu {
            return Err(format!("{} is a software adapter", info.name));
        }
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| format!("device: {e}"))?;
        // One line of truth per window: which silicon and driver stack is
        // actually drawing. "The game lags" debugging starts by knowing
        // whether this ran on the GPU at all, and on which one.
        eprintln!(
            "krate: GPU presenter on {} ({:?}, {:?})",
            info.name, info.backend, info.device_type
        );
        // Route async errors into a flag instead of wgpu's default handler,
        // which aborts the process. Any recorded failure retires this window
        // to the CPU painter on the next frame.
        let device_failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let flag = std::sync::Arc::clone(&device_failed);
            device.on_uncaptured_error(std::sync::Arc::new(move |err| {
                eprintln!("krate: GPU device error, retiring to CPU painter: {err}");
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }));
        }
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
            device_failed,
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
        // A device error recorded asynchronously since the last frame --
        // swapchain refused, validation failure, device lost -- means this
        // window's GPU path is over. Err retires it to the CPU painter.
        if self.device_failed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("GPU device reported an error".to_string());
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

/// Present raw RGBA frames -- a canvas app's raster -- on a vsynced GPU
/// surface attached to a native view.
///
/// This is S5's macOS piece: the AppKit adapter fed each canvas frame
/// through a fresh NSImage into an NSImageView, an unsynchronized CPU
/// composite that could put a half-swapped state on the glass (K-114's
/// vanish/reappear) and burned a full-buffer copy per frame. Here the frame
/// is one write_texture and a blit, presented by the compositor on vsync.
/// No scene building: a canvas raster is already pixels.
pub struct PixelPresenter {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    blitter: wgpu::util::TextureBlitter,
    surface_format: wgpu::TextureFormat,
    configured: (u32, u32),
    upload: Option<(wgpu::Texture, u32, u32)>,
    stats: FrameStats,
    device_failed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl PixelPresenter {
    /// Attach to a native AppKit view.
    ///
    /// # Safety
    /// `ns_view` must point to a valid NSView that outlives this presenter,
    /// and this must be called on the main thread.
    #[cfg(target_os = "macos")]
    pub unsafe fn new_from_ns_view(ns_view: *mut std::ffi::c_void) -> Result<Self, String> {
        use wgpu::rwh;
        let view = std::ptr::NonNull::new(ns_view).ok_or("null NSView")?;
        let instance = wgpu::Instance::default();
        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(rwh::RawDisplayHandle::AppKit(
                    rwh::AppKitDisplayHandle::new(),
                )),
                raw_window_handle: rwh::RawWindowHandle::AppKit(rwh::AppKitWindowHandle::new(view)),
            })
        }
        .map_err(|e| format!("surface: {e}"))?;
        Self::with_surface(instance, surface)
    }

    // Platform-neutral by design -- its only constructor today is the macOS
    // raw-handle one, so off-mac builds see it as dead until another
    // platform grows a raw-surface path.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn with_surface(
        instance: wgpu::Instance,
        surface: wgpu::Surface<'static>,
    ) -> Result<Self, String> {
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("adapter: {e}"))?;
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu {
            return Err(format!("{} is a software adapter", info.name));
        }
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| format!("device: {e}"))?;
        eprintln!(
            "krate: canvas presents on {} ({:?}, {:?})",
            info.name, info.backend, info.device_type
        );
        let device_failed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let flag = std::sync::Arc::clone(&device_failed);
            device.on_uncaptured_error(std::sync::Arc::new(move |err| {
                eprintln!("krate: GPU device error, retiring to CPU composite: {err}");
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            }));
        }
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
        let blitter = wgpu::util::TextureBlitter::new(&device, surface_format);
        Ok(Self {
            surface,
            device,
            queue,
            blitter,
            surface_format,
            configured: (0, 0),
            upload: None,
            stats: FrameStats::new(),
            device_failed,
        })
    }

    /// Upload one RGBA frame and present it, vsync-paced by the compositor.
    /// `Err` retires this surface; the caller falls back to the CPU path.
    pub fn present_pixels(&mut self, rgba: &[u8], width: u32, height: u32) -> Result<(), String> {
        let started = Instant::now();
        if width == 0 || height == 0 || rgba.len() < (width as usize * height as usize * 4) {
            return Ok(());
        }
        if self.device_failed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("GPU device reported an error".to_string());
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
            self.upload = None;
        }
        // One persistent upload texture, rewritten in place each frame --
        // never a new texture per frame (the K-116 board neighbor, and the
        // classic port mistake the plan warns about).
        if self
            .upload
            .as_ref()
            .map(|(_, w, h)| (*w, *h) != (width, height))
            .unwrap_or(true)
        {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("krate canvas frame"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            self.upload = Some((texture, width, height));
        }
        let (texture, ..) = self.upload.as_ref().expect("upload texture just ensured");
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba[..(width as usize * height as usize * 4)],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        use wgpu::CurrentSurfaceTexture;
        let frame = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) | CurrentSurfaceTexture::Suboptimal(frame) => {
                frame
            }
            // Occluded/minimized: skip quietly, the path is fine.
            CurrentSurfaceTexture::Timeout | CurrentSurfaceTexture::Occluded => return Ok(()),
            // Outdated/lost: force a reconfigure next frame; a truly dead
            // surface errors there and THAT retires the path.
            _ => {
                self.configured = (0, 0);
                return Ok(());
            }
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("krate canvas blit"),
            });
        let src = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let dst = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.blitter.copy(&self.device, &mut encoder, &src, &dst);
        self.queue.submit([encoder.finish()]);
        frame.present();
        if self.device_failed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("GPU device reported an error".to_string());
        }
        self.stats.record(started, None);
        Ok(())
    }
}
