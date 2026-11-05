//! The GPU canvas: vello over wgpu over Metal.
//!
//! This is what "works like a real phone app" requires and what K-090's
//! numbers demanded: the CPU rasterized 1.4 million pixels per frame in
//! ~37 ms, and no amount of trimming reaches a native feel. Here the
//! runtime's recorded display list becomes a vello scene and the phone's
//! GPU does the rasterизing at full native density -- the 2x quality cap
//! dies with the CPU path.
//!
//! The renderer lives on the guest thread. The CAMetalLayer it draws to
//! is created on the main thread once and handed over as a raw pointer;
//! wgpu's Metal backend is thread-safe from there. vello renders into an
//! intermediate storage texture (surface textures cannot be storage
//! bound), and a trivial blit pipeline copies that onto the swapchain.
//!
//! Text ops are skipped in this first light -- the CPU path already
//! proved the text stack, and glyph rendering joins in the next pass.

#![cfg(target_os = "ios")]

use std::collections::HashMap;

use krate_adapter_common::canvas_list::{CanvasList, CanvasOp};
use vello::kurbo::{Affine, Arc as KurboArc, Circle, Point, Rect, RoundedRect, RoundedRectRadii};
use vello::peniko::{
    Blob, Brush, Color, Fill, Gradient, ImageAlphaType, ImageBrush, ImageData, ImageFormat,
};
use vello::wgpu;

pub struct GpuCanvas {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_format: wgpu::TextureFormat,
    surface_config: wgpu::SurfaceConfiguration,
    /// The layer's contents scale: the 2x cap, or the panel's full scale
    /// while the frames are whole pictures.
    layer_scale: f32,
    renderer: vello::Renderer,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    blit: BlitPipeline,
    width: u32,
    height: u32,
    scale: f32,
    /// Shaping contexts plus a cache of finished glyph runs: the same
    /// strings redraw every frame, and shaping is the expensive half.
    font_cx: parley::FontContext,
    layout_cx: parley::LayoutContext<()>,
    glyph_cache: HashMap<TextKey, Vec<ShapedRun>>,
    /// Uploaded images keyed by the source Arc's address: gram's photos
    /// are generated once and drawn every frame, and re-uploading 429 KB
    /// per photo per frame would waste the bus the GPU just freed.
    /// Pictures on the GPU, by the address of the picture they came from --
    /// which the cache holds, so the address cannot be reused while the
    /// entry exists. Without the hold, an app that replaced a picture each
    /// frame (a camera preview) got its new frame at the old frame's freed
    /// address and was shown the FIRST frame for ever.
    images: HashMap<
        usize,
        (
            std::sync::Arc<krate_adapter_common::ui::ImagePixels>,
            ImageBrush,
        ),
    >,
    /// The texture a whole-frame picture is written into, at its own size.
    picture: Option<(wgpu::Texture, wgpu::TextureView)>,
}

impl GpuCanvas {
    /// Build the whole GPU stack against a CAMetalLayer pointer. Blocking
    /// is fine: this runs once, on the guest thread, at first present.
    pub fn new(
        metal_layer: *mut std::ffi::c_void,
        physical_width: u32,
        physical_height: u32,
        scale: f32,
    ) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        // SAFETY: the layer outlives the surface -- it belongs to the view
        // the adapter keeps for the app's whole life.
        let surface = unsafe {
            instance
                .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(metal_layer))
        }
        .map_err(|e| format!("surface: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|e| format!("adapter: {e}"))?;

        // Validate up front instead of letting wgpu panic mid-create: the
        // release profile aborts on panic, so a validation failure would
        // kill the app. Real Apple GPUs have indirect execution; the
        // SIMULATOR's paravirtual Metal does not, and it gets the CPU
        // fallback by this early, polite refusal.
        let downlevel = adapter.get_downlevel_capabilities();
        if !downlevel
            .flags
            .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION)
        {
            return Err("device lacks indirect execution (simulator?)".to_string());
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("krate-gpu-canvas"),
            // The adapter's own limits, not wgpu's defaults: the iOS
            // SIMULATOR's Metal reports one notch below the default in
            // places (max_inter_stage_shader_variables 15 vs 16), and a
            // request above what exists is a refusal.
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|e| format!("device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm))
            .unwrap_or(caps.formats[0]);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: physical_width,
            height: physical_height,
            // FIFO, and it is the ONLY pacer: get_current_texture
            // blocks until the panel is ready, which is exactly the
            // wait a frame loop should do. Immediate was tried and made
            // things worse (acquire 12 ms -> 27 ms, 60 fps -> 30):
            // without vsync backpressure the queue filled and the wait
            // moved rather than disappeared. The double-pacing this was
            // meant to fix is removed on the other side -- the display
            // link no longer gates the frame loop as well.
            present_mode: wgpu::PresentMode::Fifo,
            // Opaque when offered: a compositor blending our alpha
            // with whatever sits beneath the layer reads as flicker.
            alpha_mode: if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
                wgpu::CompositeAlphaMode::Opaque
            } else {
                caps.alpha_modes[0]
            },
            view_formats: vec![],
            // Two frames in flight (see `present`).
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
            .map_err(|e| format!("vello renderer: {e}"))?;
        let (target, target_view) = make_target(&device, physical_width, physical_height);
        let blit = BlitPipeline::new(&device, surface_format);

        Ok(GpuCanvas {
            device,
            queue,
            surface,
            surface_format,
            surface_config,
            layer_scale: scale,
            renderer,
            target,
            target_view,
            blit,
            width: physical_width,
            height: physical_height,
            scale,
            images: HashMap::new(),
            picture: None,
            font_cx: parley::FontContext::new(),
            layout_cx: parley::LayoutContext::new(),
            glyph_cache: HashMap::new(),
        })
    }

    /// The scale the layer is shown at now.
    pub fn layer_scale(&self) -> f32 {
        self.layer_scale
    }

    pub fn set_layer_scale(&mut self, scale: f32) {
        self.layer_scale = scale;
    }

    /// Follow the screen to a new physical size (it turned).
    pub fn resize(&mut self, width: u32, height: u32) {
        if (width, height) == (self.width, self.height) || width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        let (target, target_view) = make_target(&self.device, width, height);
        self.target = target;
        self.target_view = target_view;
        self.width = width;
        self.height = height;
    }

    /// Render one recorded frame and present it.
    pub fn present(&mut self, list: &CanvasList) -> Result<(), String> {
        if let Some(image) = whole_frame_picture(list) {
            return self.present_picture(image);
        }
        // Two frames in flight, as for pictures. With one, a frame that
        // took 3 ms to draw still waited 30 ms for the panel to show the
        // last one: the store scrolled at 31 fps on an iPhone 13 Pro.
        if self.surface_config.desired_maximum_frame_latency != 2 {
            self.surface_config.desired_maximum_frame_latency = 2;
            self.surface.configure(&self.device, &self.surface_config);
        }
        let t_scene = std::time::Instant::now();
        let mut scene = vello::Scene::new();
        // An app that set a design size draws in its own coordinates; map
        // them onto the canvas uniformly and centred, exactly as the CPU
        // raster does. Without this the picture sat unscaled while taps were
        // mapped into design space: a calculator drawn too small and
        // off-centre, its buttons answering touches in the wrong places.
        let screen = Affine::scale(self.scale as f64);
        let (k, ox, oy) = list.design_transform();
        let design_base =
            screen * Affine::translate((ox as f64, oy as f64)) * Affine::scale(k as f64);
        let mut base = design_base;
        // The spare space around a letterboxed design takes the app's own
        // edge colours, so it reads as filling the screen, not as a card
        // between black bars.
        if let (Some(edges), Some((dw, dh))) = (list.edge_colors(), list.design) {
            let (cw, ch) = (list.logical_width as f64, list.logical_height as f64);
            let (x0, y0) = (ox as f64, oy as f64);
            let (x1, y1) = (x0 + dw as f64 * k as f64, y0 + dh as f64 * k as f64);
            for (color, area) in [
                (edges.top, Rect::new(0.0, 0.0, cw, y0 + 0.5)),
                (edges.bottom, Rect::new(0.0, y1 - 0.5, cw, ch)),
                (edges.left, Rect::new(0.0, y0, x0 + 0.5, y1)),
                (edges.right, Rect::new(x1 - 0.5, y0, cw, y1)),
            ] {
                if area.width() > 0.5 && area.height() > 0.5 {
                    scene.fill(
                        Fill::NonZero,
                        screen,
                        &Brush::Solid(unpack(color)),
                        None,
                        &area,
                    );
                }
            }
        }
        let mut clip_depth = 0usize;

        for op in &list.ops {
            match op {
                CanvasOp::Clear(color) => {
                    // The whole canvas, letterbox bars included.
                    scene.fill(
                        Fill::NonZero,
                        screen,
                        &Brush::Solid(unpack(*color)),
                        None,
                        &Rect::new(
                            0.0,
                            0.0,
                            list.logical_width as f64,
                            list.logical_height as f64,
                        ),
                    );
                }
                CanvasOp::ScreenSpace(on) => {
                    base = if *on { screen } else { design_base };
                }
                CanvasOp::SetClip(rect) => {
                    while clip_depth > 0 {
                        scene.pop_layer();
                        clip_depth -= 1;
                    }
                    if let Some((x, y, w, h)) = rect {
                        scene.push_layer(
                            Fill::NonZero,
                            vello::peniko::BlendMode::default(),
                            1.0,
                            base,
                            &Rect::new(*x as f64, *y as f64, (*x + *w) as f64, (*y + *h) as f64),
                        );
                        clip_depth += 1;
                    }
                }
                CanvasOp::FillRect { rect, color } => {
                    scene.fill(
                        Fill::NonZero,
                        base,
                        &Brush::Solid(unpack(*color)),
                        None,
                        &to_rect(*rect),
                    );
                }
                CanvasOp::StrokeRect { rect, width, color } => {
                    scene.stroke(
                        &vello::kurbo::Stroke::new(*width as f64),
                        base,
                        &Brush::Solid(unpack(*color)),
                        None,
                        &to_rect(*rect),
                    );
                }
                CanvasOp::FillRoundRect { rect, radii, color } => {
                    scene.fill(
                        Fill::NonZero,
                        base,
                        &Brush::Solid(unpack(*color)),
                        None,
                        &to_round_rect(*rect, *radii),
                    );
                }
                CanvasOp::StrokeRoundRect {
                    rect,
                    radii,
                    width,
                    color,
                } => {
                    scene.stroke(
                        &vello::kurbo::Stroke::new(*width as f64),
                        base,
                        &Brush::Solid(unpack(*color)),
                        None,
                        &to_round_rect(*rect, *radii),
                    );
                }
                CanvasOp::DropShadowRoundRect {
                    rect,
                    radii,
                    blur,
                    color,
                } => {
                    // vello's analytic blurred rounded rect: one radius, so
                    // take the largest -- shadows are soft by definition.
                    let r = radii.0.max(radii.1).max(radii.2).max(radii.3) as f64;
                    scene.draw_blurred_rounded_rect(
                        base,
                        to_rect(*rect),
                        unpack(*color),
                        r,
                        *blur as f64,
                    );
                }
                CanvasOp::LinearGradient { rect, top, bottom } => {
                    let brush = Gradient::new_linear(
                        (rect.0 as f64, rect.1 as f64),
                        (rect.0 as f64, (rect.1 + rect.3) as f64),
                    )
                    .with_stops([unpack(*top), unpack(*bottom)]);
                    scene.fill(
                        Fill::NonZero,
                        base,
                        &Brush::Gradient(brush),
                        None,
                        &to_rect(*rect),
                    );
                }
                CanvasOp::LinearGradientStops {
                    rect,
                    angle_degrees,
                    stops,
                } => {
                    let (x, y, w, h) = *rect;
                    let theta = (*angle_degrees as f64).to_radians();
                    let (dx, dy) = (theta.cos(), theta.sin());
                    // Project the rect's corners on the axis so offsets 0
                    // and 1 land on the extremes, matching the CPU raster.
                    let corners = [
                        (x as f64, y as f64),
                        ((x + w) as f64, y as f64),
                        (x as f64, (y + h) as f64),
                        ((x + w) as f64, (y + h) as f64),
                    ];
                    let ts: Vec<f64> = corners.iter().map(|(px, py)| px * dx + py * dy).collect();
                    let lo = ts.iter().cloned().fold(f64::MAX, f64::min);
                    let hi = ts.iter().cloned().fold(f64::MIN, f64::max);
                    let start = (dx * lo, dy * lo);
                    let end = (dx * hi, dy * hi);
                    let pairs: Vec<(f32, Color)> = stops
                        .iter()
                        .map(|(offset, color)| (*offset, unpack(*color)))
                        .collect();
                    let gradient = Gradient::new_linear(start, end).with_stops(pairs.as_slice());
                    scene.fill(
                        Fill::NonZero,
                        base,
                        &Brush::Gradient(gradient),
                        None,
                        &to_rect(*rect),
                    );
                }
                CanvasOp::RadialGradient {
                    center,
                    radius,
                    inner,
                    outer,
                } => {
                    let gradient =
                        Gradient::new_radial((center.0 as f64, center.1 as f64), *radius)
                            .with_stops([unpack(*inner), unpack(*outer)]);
                    scene.fill(
                        Fill::NonZero,
                        base,
                        &Brush::Gradient(gradient),
                        None,
                        &Circle::new((center.0 as f64, center.1 as f64), *radius as f64),
                    );
                }
                CanvasOp::FillCircle {
                    center,
                    radius,
                    color,
                } => {
                    scene.fill(
                        Fill::NonZero,
                        base,
                        &Brush::Solid(unpack(*color)),
                        None,
                        &Circle::new((center.0 as f64, center.1 as f64), *radius as f64),
                    );
                }
                CanvasOp::StrokeCircle {
                    center,
                    radius,
                    width,
                    color,
                } => {
                    scene.stroke(
                        &vello::kurbo::Stroke::new(*width as f64),
                        base,
                        &Brush::Solid(unpack(*color)),
                        None,
                        &Circle::new((center.0 as f64, center.1 as f64), *radius as f64),
                    );
                }
                CanvasOp::StrokeArc {
                    center,
                    radius,
                    start_degrees,
                    sweep_degrees,
                    width,
                    color,
                } => {
                    let arc = KurboArc::new(
                        Point::new(center.0 as f64, center.1 as f64),
                        (*radius as f64, *radius as f64),
                        (*start_degrees as f64).to_radians(),
                        (*sweep_degrees as f64).to_radians(),
                        0.0,
                    );
                    scene.stroke(
                        &vello::kurbo::Stroke::new(*width as f64),
                        base,
                        &Brush::Solid(unpack(*color)),
                        None,
                        &arc,
                    );
                }
                CanvasOp::Pixels { rect, image } => {
                    let gpu_image = self.upload(image);
                    let (x, y, w, h) = *rect;
                    let sx = w as f64 / image.width.max(1) as f64;
                    let sy = h as f64 / image.height.max(1) as f64;
                    scene.draw_image(
                        &gpu_image,
                        base * Affine::translate((x as f64, y as f64))
                            * Affine::scale_non_uniform(sx, sy),
                    );
                }
                CanvasOp::PixelsRound { rect, radii, image } => {
                    let gpu_image = self.upload(image);
                    scene.push_layer(
                        Fill::NonZero,
                        vello::peniko::BlendMode::default(),
                        1.0,
                        base,
                        &to_round_rect(*rect, *radii),
                    );
                    let (x, y, w, h) = *rect;
                    // Cover, center-cropped, like the CPU path.
                    let scale =
                        (w / image.width.max(1) as f32).max(h / image.height.max(1) as f32) as f64;
                    let dw = image.width as f64 * scale;
                    let dh = image.height as f64 * scale;
                    let ox = x as f64 + (w as f64 - dw) / 2.0;
                    let oy = y as f64 + (h as f64 - dh) / 2.0;
                    scene.draw_image(
                        &gpu_image,
                        base * Affine::translate((ox, oy)) * Affine::scale(scale),
                    );
                    scene.pop_layer();
                }
                CanvasOp::Sprite {
                    center,
                    dst,
                    angle,
                    image,
                } => {
                    let gpu_image = self.upload(image);
                    let sx = dst.0 as f64 / image.width.max(1) as f64;
                    let sy = dst.1 as f64 / image.height.max(1) as f64;
                    let transform = base
                        * Affine::translate((center.0 as f64, center.1 as f64))
                        * Affine::rotate(*angle as f64)
                        * Affine::scale_non_uniform(sx, sy)
                        * Affine::translate((
                            -(image.width as f64) / 2.0,
                            -(image.height as f64) / 2.0,
                        ));
                    scene.draw_image(&gpu_image, transform);
                }
                CanvasOp::Text {
                    origin,
                    font_size,
                    color,
                    weight,
                    italic,
                    letter_spacing,
                    family,
                    text,
                } => {
                    let runs = self
                        .shape(text, *font_size, *weight, *italic, *letter_spacing, *family)
                        .clone();
                    let transform = base * Affine::translate((origin.0 as f64, origin.1 as f64));
                    let brush = Brush::Solid(unpack(*color));
                    for run in &runs {
                        scene
                            .draw_glyphs(&run.font)
                            .font_size(run.font_size)
                            .normalized_coords(&run.coords)
                            .transform(transform)
                            .brush(&brush)
                            .draw(Fill::NonZero, run.glyphs.iter().copied());
                    }
                }
            }
        }
        while clip_depth > 0 {
            scene.pop_layer();
            clip_depth -= 1;
        }

        let scene_ms = t_scene.elapsed().as_secs_f64() * 1000.0;
        let t_render = std::time::Instant::now();
        self.renderer
            .render_to_texture(
                &self.device,
                &self.queue,
                &scene,
                &self.target_view,
                &vello::RenderParams {
                    base_color: Color::BLACK,
                    width: self.width,
                    height: self.height,
                    antialiasing_method: vello::AaConfig::Area,
                },
            )
            .map_err(|e| format!("render: {e}"))?;
        if let Some(path) = crate::uikit_native::preview_due() {
            self.save_preview(&path);
        }

        let render_ms = t_render.elapsed().as_secs_f64() * 1000.0;
        let t_acquire = std::time::Instant::now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            other => return Err(format!("acquire: {other:?}")),
        };
        let acquire_ms = t_acquire.elapsed().as_secs_f64() * 1000.0;
        let t_submit = std::time::Instant::now();
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.blit.blit(
            &self.device,
            &mut encoder,
            &self.target_view,
            &frame_view,
            false,
        );
        self.queue.submit([encoder.finish()]);
        frame.present();
        let submit_ms = t_submit.elapsed().as_secs_f64() * 1000.0;
        crate::uikit_native::diag(&format!(
            "frame scene={scene_ms:.1} render={render_ms:.1} acquire={acquire_ms:.1} submit={submit_ms:.1} ops={}",
            list.ops.len()
        ));
        Ok(())
    }

    /// A frame that is one picture covering the canvas -- a game's or an
    /// emulator's framebuffer -- goes straight into a texture and is drawn
    /// scaled to the screen in one pass. Through vello it went into the
    /// image atlas again every frame (a new picture each time) and through
    /// its compute passes: Counter-Strike on an iPhone 13 Pro was laggy.
    /// Read the frame just rendered back from the GPU and write it as the
    /// app's preview. Once per app, so the stall does not matter.
    fn save_preview(&self, path: &std::path::Path) {
        let (width, height) = (self.width, self.height);
        let bytes_per_row = (width * 4).next_multiple_of(256);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("krate-preview"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
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
        if !matches!(rx.recv(), Ok(Ok(()))) {
            return;
        }
        let data = slice.get_mapped_range();
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for row in 0..height {
            let start = (row * bytes_per_row) as usize;
            rgba.extend_from_slice(&data[start..start + (width * 4) as usize]);
        }
        drop(data);
        buffer.unmap();
        crate::uikit_native::write_preview(path, &rgba, width, height);
    }

    fn present_picture(
        &mut self,
        image: &krate_adapter_common::ui::ImagePixels,
    ) -> Result<(), String> {
        if let Some(path) = crate::uikit_native::preview_due() {
            // A whole-frame picture is its own preview.
            crate::uikit_native::write_preview(&path, &image.rgba, image.width, image.height);
        }
        let t = std::time::Instant::now();
        let (w, h) = (image.width.max(1), image.height.max(1));
        let stale = self
            .picture
            .as_ref()
            .is_none_or(|(tex, _)| tex.width() != w || tex.height() != h);
        if stale {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("krate-picture"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.picture = Some((texture, view));
        }
        // Two frames in flight. With one, a game whose frame takes 13 ms
        // waited for the panel to show the last frame before it could hand
        // over the next, and landed every other refresh: 30 fps on an
        // iPhone 13 Pro that could draw 70.
        if self.surface_config.desired_maximum_frame_latency != 2 {
            self.surface_config.desired_maximum_frame_latency = 2;
            self.surface.configure(&self.device, &self.surface_config);
        }
        let (texture, view) = self.picture.as_ref().expect("picture texture made above");
        let t_upload = std::time::Instant::now();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let upload_ms = t_upload.elapsed().as_secs_f64() * 1000.0;
        let t_acquire = std::time::Instant::now();
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            other => return Err(format!("acquire: {other:?}")),
        };
        let acquire_ms = t_acquire.elapsed().as_secs_f64() * 1000.0;
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        // Crisp when the picture fills the screen by a whole number.
        let (sw, sh) = (self.width, self.height);
        let crisp = sw % w == 0 && sh % h == 0 && sw / w == sh / h;
        self.blit
            .blit(&self.device, &mut encoder, view, &frame_view, crisp);
        self.queue.submit([encoder.finish()]);
        frame.present();
        crate::uikit_native::diag(&format!(
            "frame picture {w}x{h} {:.1}ms upload={upload_ms:.1} acquire={acquire_ms:.1}",
            t.elapsed().as_secs_f64() * 1000.0
        ));
        Ok(())
    }

    fn upload(
        &mut self,
        image: &std::sync::Arc<krate_adapter_common::ui::ImagePixels>,
    ) -> ImageBrush {
        let key = std::sync::Arc::as_ptr(image) as usize;
        if let Some((_, cached)) = self.images.get(&key) {
            return cached.clone();
        }
        let gpu_image = ImageBrush::new(ImageData {
            data: Blob::from(image.rgba.clone()),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: image.width,
            height: image.height,
        });
        // Let go of pictures nobody holds any more (the cache's own hold is
        // the only one left), then cap what remains.
        self.images
            .retain(|_, (held, _)| std::sync::Arc::strong_count(held) > 1);
        if self.images.len() >= 64 {
            self.images.clear();
        }
        self.images.insert(key, (image.clone(), gpu_image.clone()));
        gpu_image
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct TextKey {
    text: String,
    size_bits: u32,
    weight: u16,
    italic: bool,
    spacing_bits: u32,
    family: u8,
}

#[derive(Clone)]
struct ShapedRun {
    font: vello::peniko::FontData,
    font_size: f32,
    coords: Vec<i16>,
    glyphs: Vec<vello::Glyph>,
}

impl GpuCanvas {
    /// Shape a canvas text run: parley layout, glyphs positioned so the
    /// draw origin is the first line's baseline -- the canvas draw-text
    /// contract, matching the CPU raster.
    fn shape(
        &mut self,
        text: &str,
        font_size: f32,
        weight: u16,
        italic: bool,
        letter_spacing: f32,
        family: u8,
    ) -> &Vec<ShapedRun> {
        let key = TextKey {
            text: text.to_string(),
            size_bits: font_size.to_bits(),
            weight,
            italic,
            spacing_bits: letter_spacing.to_bits(),
            family,
        };
        if !self.glyph_cache.contains_key(&key) {
            let mut builder = self
                .layout_cx
                .ranged_builder(&mut self.font_cx, text, 1.0, true);
            builder.push_default(match family {
                1 => parley::GenericFamily::Serif,
                2 => parley::GenericFamily::Monospace,
                _ => parley::GenericFamily::SansSerif,
            });
            builder.push_default(parley::StyleProperty::FontSize(font_size));
            builder.push_default(parley::StyleProperty::FontWeight(parley::FontWeight::new(
                (weight as f32).clamp(100.0, 900.0),
            )));
            if italic {
                builder.push_default(parley::StyleProperty::FontStyle(parley::FontStyle::Italic));
            }
            if letter_spacing != 0.0 {
                builder.push_default(parley::StyleProperty::LetterSpacing(letter_spacing));
            }
            let mut layout: parley::Layout<()> = builder.build(text);
            layout.break_all_lines(None);
            layout.align(
                parley::Alignment::Start,
                parley::AlignmentOptions::default(),
            );

            let mut runs = Vec::new();
            let mut first_baseline: Option<f32> = None;
            for line in layout.lines() {
                for item in line.items() {
                    let parley::PositionedLayoutItem::GlyphRun(glyph_run) = item else {
                        continue;
                    };
                    let run = glyph_run.run();
                    let baseline = glyph_run.baseline();
                    let shift = *first_baseline.get_or_insert(baseline);
                    let mut x = glyph_run.offset();
                    let glyphs: Vec<vello::Glyph> = glyph_run
                        .glyphs()
                        .map(|g| {
                            let glyph = vello::Glyph {
                                id: g.id as u32,
                                x: x + g.x,
                                y: baseline - shift + g.y,
                            };
                            x += g.advance;
                            glyph
                        })
                        .collect();
                    runs.push(ShapedRun {
                        font: run.font().clone(),
                        font_size: run.font_size(),
                        coords: run.normalized_coords().to_vec(),
                        glyphs,
                    });
                }
            }
            if self.glyph_cache.len() >= 512 {
                self.glyph_cache.clear();
            }
            self.glyph_cache.insert(key.clone(), runs);
        }
        self.glyph_cache.get(&key).expect("just inserted")
    }
}

fn make_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("vello-target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        // COPY_SRC: the app's preview is read back from here (once per app).
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    (target, view)
}

fn to_rect(r: (f32, f32, f32, f32)) -> Rect {
    Rect::new(
        r.0 as f64,
        r.1 as f64,
        (r.0 + r.2) as f64,
        (r.1 + r.3) as f64,
    )
}

fn to_round_rect(r: (f32, f32, f32, f32), radii: (f32, f32, f32, f32)) -> RoundedRect {
    RoundedRect::new(
        r.0 as f64,
        r.1 as f64,
        (r.0 + r.2) as f64,
        (r.1 + r.3) as f64,
        RoundedRectRadii::new(
            radii.0 as f64,
            radii.1 as f64,
            radii.2 as f64,
            radii.3 as f64,
        ),
    )
}

fn unpack(argb: u32) -> Color {
    Color::from_rgba8(
        (argb >> 16) as u8,
        (argb >> 8) as u8,
        argb as u8,
        (argb >> 24) as u8,
    )
}

/// A fullscreen-triangle copy from the vello target to the swapchain: the
/// storage texture vello writes cannot be a surface texture itself.
struct BlitPipeline {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    nearest: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
}

impl BlitPipeline {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("krate-blit"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VsOut {
    var out: VsOut;
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    out.pos = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(src, samp, in.uv);
}
"#
                .into(),
            ),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("krate-blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        // Linear for a picture scaled by an odd ratio; nearest for one
        // scaled by a whole number, so each of its pixels lands as a crisp
        // block. At the same size (vello's own target) the two agree.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let nearest = device.create_sampler(&wgpu::SamplerDescriptor::default());
        BlitPipeline {
            pipeline,
            sampler,
            nearest,
            layout,
        }
    }

    fn blit(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
        crisp: bool,
    ) {
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(if crisp {
                        &self.nearest
                    } else {
                        &self.sampler
                    }),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("krate-blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            multiview_mask: None,
            occlusion_query_set: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// The one picture a frame consists of, if that is all it is: an opaque
/// image covering the whole canvas, after at most a clear.
pub(crate) fn whole_frame_picture(
    list: &CanvasList,
) -> Option<&krate_adapter_common::ui::ImagePixels> {
    // The picture must cover what the app draws in: its design when it set
    // one, the canvas otherwise. A design of another shape is letterboxed,
    // and the spare space is the vector path's to fill.
    let (cw, ch) = match list.design {
        None => (list.logical_width as f32, list.logical_height as f32),
        Some((dw, dh)) => {
            let (sw, sh) = (list.logical_width as f32, list.logical_height as f32);
            // Fitted inside covered edges, a design no longer covers the
            // whole frame, so its picture cannot stand for the frame.
            if dw <= 0.0
                || dh <= 0.0
                || ((dw / dh) / (sw / sh) - 1.0).abs() > 0.01
                || list.design_inset != (0.0, 0.0, 0.0, 0.0)
            {
                return None;
            }
            (dw, dh)
        }
    };
    let mut picture = None;
    for op in &list.ops {
        match op {
            CanvasOp::Clear(_) | CanvasOp::SetClip(None) if picture.is_none() => {}
            CanvasOp::Pixels { rect, image } if picture.is_none() => {
                let (x, y, w, h) = *rect;
                let covers = x <= 0.5
                    && y <= 0.5
                    && (x + w - cw).abs() <= cw * 0.01 + 0.5
                    && (y + h - ch).abs() <= ch * 0.01 + 0.5;
                if !covers || image.rgba.chunks_exact(4).any(|p| p[3] != 255) {
                    return None;
                }
                picture = Some(image.as_ref());
            }
            _ => return None,
        }
    }
    picture
}
