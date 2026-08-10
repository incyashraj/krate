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
    renderer: vello::Renderer,
    target: wgpu::Texture,
    target_view: wgpu::TextureView,
    blit: BlitPipeline,
    width: u32,
    height: u32,
    scale: f32,
    /// Uploaded images keyed by the source Arc's address: gram's photos
    /// are generated once and drawn every frame, and re-uploading 429 KB
    /// per photo per frame would waste the bus the GPU just freed.
    images: HashMap<usize, ImageBrush>,
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
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::CoreAnimationLayer(
                metal_layer,
            ))
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
        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: surface_format,
                width: physical_width,
                height: physical_height,
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: caps.alpha_modes[0],
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            },
        );

        let renderer = vello::Renderer::new(&device, vello::RendererOptions::default())
            .map_err(|e| format!("vello renderer: {e}"))?;
        let (target, target_view) = make_target(&device, physical_width, physical_height);
        let blit = BlitPipeline::new(&device, surface_format);

        Ok(GpuCanvas {
            device,
            queue,
            surface,
            surface_format,
            renderer,
            target,
            target_view,
            blit,
            width: physical_width,
            height: physical_height,
            scale,
            images: HashMap::new(),
        })
    }

    /// Render one recorded frame and present it.
    pub fn present(&mut self, list: &CanvasList) -> Result<(), String> {
        let mut scene = vello::Scene::new();
        let base = Affine::scale(self.scale as f64);
        let mut clip_depth = 0usize;

        for op in &list.ops {
            match op {
                CanvasOp::Clear(color) => {
                    scene.fill(
                        Fill::NonZero,
                        base,
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
                            &Rect::new(
                                *x as f64,
                                *y as f64,
                                (*x + *w) as f64,
                                (*y + *h) as f64,
                            ),
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
                    let gradient =
                        Gradient::new_linear(start, end).with_stops(pairs.as_slice());
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
                    let gradient = Gradient::new_radial(
                        (center.0 as f64, center.1 as f64),
                        *radius,
                    )
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
                CanvasOp::Text { .. } => {
                    // Glyph rendering joins in the next pass; the frame
                    // stays honest about everything else meanwhile.
                }
            }
        }
        while clip_depth > 0 {
            scene.pop_layer();
            clip_depth -= 1;
        }

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

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            other => return Err(format!("acquire: {other:?}")),
        };
        let frame_view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.blit
            .blit(&self.device, &mut encoder, &self.target_view, &frame_view);
        self.queue.submit([encoder.finish()]);
        frame.present();
        Ok(())
    }

    fn upload(
        &mut self,
        image: &std::sync::Arc<krate_adapter_common::ui::ImagePixels>,
    ) -> ImageBrush {
        let key = std::sync::Arc::as_ptr(image) as usize;
        if let Some(cached) = self.images.get(&key) {
            return cached.clone();
        }
        let gpu_image = ImageBrush::new(ImageData {
            data: Blob::from(image.rgba.clone()),
            format: ImageFormat::Rgba8,
            alpha_type: ImageAlphaType::Alpha,
            width: image.width,
            height: image.height,
        });
        if self.images.len() >= 64 {
            self.images.clear();
        }
        self.images.insert(key, gpu_image.clone());
        gpu_image
    }
}

fn make_target(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
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
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        BlitPipeline {
            pipeline,
            sampler,
            layout,
        }
    }

    fn blit(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::TextureView,
        dst: &wgpu::TextureView,
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
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
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
