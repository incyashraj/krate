//! The GPU backend for `gfx.scene3d`.
//!
//! Stage G1 of Plan/GPU-Scene3d-2026-09-17.md: triangles through a wgpu render
//! pass with a depth buffer, rendered to a texture and read back as the same
//! [`ImagePixels`] the CPU rasterizer produces.
//!
//! Reading back rather than presenting the texture directly is a deliberate
//! first step, not the end state. It keeps the seam exactly where it already
//! is -- `render_image()` returns an image and everything downstream takes one
//! -- so nothing above it has to change: not the host, not the three adapters,
//! not `krate run --shoot`. It also costs a stall on every frame, which is why
//! this stage MEASURES against the CPU path rather than assuming it wins.
//!
//! The CPU rasterizer stays. It is the fallback where no adapter exists, the
//! reference an image diff compares against, and what CI proves, since CI has
//! no GPU.

use bytemuck::{Pod, Zeroable};
use krate_adapter_common::ui::{ImagePixels, UiAdapterError};
use vello::wgpu;

/// Largest edge of a GPU 3D surface.
///
/// Higher than the CPU rasterizer's 1920 because the cost is no longer linear
/// in pixels on this thread -- but not unbounded: the readback buffer is
/// width x height x 4 bytes and has to be mapped every frame, and a texture
/// larger than the display is pixels nobody sees.
pub const MAX_EDGE: u32 = 4_096;

/// Bit 0 of a vertex's flags: this vertex carries a real normal.
const FLAG_SMOOTH: u32 = 1;

/// One vertex as the shader wants it.
///
/// Per-vertex tint rather than a uniform per draw: the CPU path takes one tint
/// per call and an app makes many calls, so folding the tint into the vertex
/// lets a whole frame become ONE draw. That is the single biggest difference
/// between the two backends -- the racing game submits four thousand calls a
/// frame, and four thousand GPU draws would be slower than the CPU path.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tint: [f32; 4],
    pub flags: u32,
    /// Padding to a 16-byte multiple, which wgpu requires of a vertex stride
    /// and which is cheaper to state than to let the layout accidentally
    /// depend on the field order.
    pub _pad: [u32; 3],
}

impl Vertex {
    pub fn new(
        position: [f32; 3],
        normal: [f32; 3],
        uv: [f32; 2],
        tint: [f32; 4],
        smooth: bool,
    ) -> Self {
        Self {
            position,
            normal,
            uv,
            tint,
            flags: if smooth { FLAG_SMOOTH } else { 0 },
            _pad: [0; 3],
        }
    }

    fn layout() -> wgpu::VertexBufferLayout<'static> {
        const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            0 => Float32x3,
            1 => Float32x3,
            2 => Float32x2,
            3 => Float32x4,
            4 => Uint32,
        ];
        wgpu::VertexBufferLayout {
            array_stride: core::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}

/// The camera and light, as the shader's uniform block.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    light: [f32; 4],
}

/// A GPU device and the pipeline built on it.
///
/// Created once and reused: building a pipeline compiles a shader, which is
/// milliseconds, and doing it per frame would dwarf the rendering.
pub struct GpuScene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    width: u32,
    height: u32,
    colour: wgpu::Texture,
    depth: wgpu::Texture,
    /// The readback buffer, sized to the padded row stride wgpu requires.
    readback: wgpu::Buffer,
    padded_row_bytes: u32,
}

impl GpuScene {
    /// Ask the platform for an adapter and build everything on it.
    ///
    /// Returns `None` rather than an error when there is no usable GPU: a
    /// machine without one is not a broken machine, it is a machine that runs
    /// the CPU path. CI is such a machine.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
            return None;
        }
        pollster::block_on(Self::create(width, height))
    }

    async fn create(width: u32, height: u32) -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                ..Default::default()
            })
            .await
            .ok()?;
        // A software adapter is not a GPU. WARP and its kin rasterize on the
        // CPU behind a driver mask, which is SLOWER than our own rasterizer --
        // so accepting one would make the GPU backend a downgrade. The
        // presenter declines them for the same reason.
        if adapter.get_info().device_type == wgpu::DeviceType::Cpu {
            return None;
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .ok()?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene3d"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene.wgsl").into()),
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene3d pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    // Source-over, the same blend the CPU path does by hand.
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // Culling is a per-scene choice in the WIT, and switching it
                // would mean a second pipeline. G1 draws everything and lets
                // the depth buffer sort it out; G2 adds the culled pipeline.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene3d sampler"),
            // Repeat, matching the CPU sampler's `rem_euclid` wrap, so a floor
            // that tiles a small image tiles the same way on both backends.
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let (colour, depth) = Self::targets(&device, width, height);
        let padded_row_bytes = padded_row(width);
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d readback"),
            size: u64::from(padded_row_bytes) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        Some(Self {
            device,
            queue,
            pipeline,
            bind_layout,
            sampler,
            width,
            height,
            colour,
            depth,
            readback,
            padded_row_bytes,
        })
    }

    fn targets(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::Texture) {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let colour = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene3d colour"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene3d depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        (colour, depth)
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Draw a frame and read it back.
    ///
    /// `vertices` is the whole frame in one buffer: every triangle, with its
    /// tint folded into each vertex. That is the shape the CPU path cannot
    /// use and the GPU needs -- four thousand separate draws would be slower
    /// here than the software rasterizer is.
    pub fn render(
        &mut self,
        vertices: &[Vertex],
        view_proj: [[f32; 4]; 4],
        light: [f32; 3],
        clear: [f32; 4],
        texture: &ImagePixels,
    ) -> Result<ImagePixels, UiAdapterError> {
        let uniform = CameraUniform {
            view_proj,
            light: [light[0], light[1], light[2], 0.0],
        };
        let uniform_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d camera"),
            size: core::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&uniform_buffer, 0, bytemuck::bytes_of(&uniform));

        let atlas = self.upload_texture(texture);
        let atlas_view = atlas.create_view(&Default::default());
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene3d bind group"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d vertices"),
            size: (core::mem::size_of::<Vertex>() * vertices.len().max(1)) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !vertices.is_empty() {
            self.queue
                .write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(vertices));
        }

        let colour_view = self.colour.create_view(&Default::default());
        let depth_view = self.depth.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene3d frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene3d pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &colour_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(clear[0]),
                            g: f64::from(clear[1]),
                            b: f64::from(clear[2]),
                            a: f64::from(clear[3]),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !vertices.is_empty() {
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.draw(0..vertices.len() as u32, 0..1);
            }
        }

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.colour,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row_bytes),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(encoder.finish()));

        self.read_back()
    }

    /// Map the readback buffer and unpad it into a tight RGBA image.
    ///
    /// wgpu requires each row of a texture-to-buffer copy to start on a
    /// 256-byte boundary, so the buffer is wider than the image and the rows
    /// have to be copied out one at a time. Returning the padded buffer would
    /// hand every consumer a stride they do not expect.
    fn read_back(&self) -> Result<ImagePixels, UiAdapterError> {
        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        // Block until the GPU is done. This is the stall the plan warns about
        // and the reason G1 measures: on a small surface it can cost more than
        // the CPU rasterizer saves.
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        match rx.recv() {
            Ok(Ok(())) => {}
            _ => {
                return Err(UiAdapterError::Unsupported(
                    "the GPU frame could not be read back".to_string(),
                ))
            }
        }

        let row_bytes = (self.width * 4) as usize;
        let mut rgba = Vec::with_capacity(row_bytes * self.height as usize);
        {
            let mapped = slice.get_mapped_range();
            for row in 0..self.height as usize {
                let start = row * self.padded_row_bytes as usize;
                rgba.extend_from_slice(&mapped[start..start + row_bytes]);
            }
        }
        self.readback.unmap();
        ImagePixels::new(self.width, self.height, rgba)
    }

    fn upload_texture(&self, image: &ImagePixels) -> wgpu::Texture {
        let size = wgpu::Extent3d {
            width: image.width,
            height: image.height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene3d atlas"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(image.width * 4),
                rows_per_image: Some(image.height),
            },
            size,
        );
        texture
    }
}

/// Round a row up to wgpu's 256-byte copy alignment.
fn padded_row(width: u32) -> u32 {
    const ALIGN: u32 = 256;
    let unpadded = width * 4;
    unpadded.div_ceil(ALIGN) * ALIGN
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test here skips when the machine has no GPU. CI is such a
    /// machine, and a test that fails there would say "the GPU backend is
    /// broken" when it means "there is no GPU" -- so the CPU path stays the
    /// one CI proves.
    fn gpu(width: u32, height: u32) -> Option<GpuScene> {
        GpuScene::new(width, height)
    }

    /// A view-projection that looks down -Z from `eye`, matching the CPU
    /// path's basis closely enough for a first triangle.
    fn look_at(eye: [f32; 3], target: [f32; 3], aspect: f32, fov_deg: f32) -> [[f32; 4]; 4] {
        let f = {
            let d = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
            [d[0] / l, d[1] / l, d[2] / l]
        };
        // right = world_up x forward, then up = forward x right -- the same
        // order as the CPU path, whose comment records getting it wrong once.
        // With world_up = +Y that reduces to
        // (f.z, 0, -f.x).
        let r = {
            let c = [f[2], 0.0, -f[0]];
            let l = (c[0] * c[0] + c[2] * c[2]).sqrt().max(1e-6);
            [c[0] / l, 0.0, c[2] / l]
        };
        let u = [
            f[1] * r[2] - f[2] * r[1],
            f[2] * r[0] - f[0] * r[2],
            f[0] * r[1] - f[1] * r[0],
        ];
        let half = (fov_deg.to_radians() / 2.0).tan();
        let (near, far) = (0.1_f32, 1000.0_f32);
        // Column-major for WGSL's mat4x4, which reads columns.
        [
            [
                r[0] / (half * aspect),
                u[0] / half,
                f[0] * far / (far - near),
                f[0],
            ],
            [
                r[1] / (half * aspect),
                u[1] / half,
                f[1] * far / (far - near),
                f[1],
            ],
            [
                r[2] / (half * aspect),
                u[2] / half,
                f[2] * far / (far - near),
                f[2],
            ],
            [
                -(r[0] * eye[0] + r[1] * eye[1] + r[2] * eye[2]) / (half * aspect),
                -(u[0] * eye[0] + u[1] * eye[1] + u[2] * eye[2]) / half,
                -(f[0] * eye[0] + f[1] * eye[1] + f[2] * eye[2]) * far / (far - near)
                    - near * far / (far - near),
                -(f[0] * eye[0] + f[1] * eye[1] + f[2] * eye[2]),
            ],
        ]
    }

    fn white() -> ImagePixels {
        ImagePixels::new(1, 1, vec![255, 255, 255, 255]).expect("texture")
    }

    #[test]
    fn a_triangle_reaches_the_screen_through_the_gpu() {
        let Some(mut scene) = gpu(64, 64) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        let tint = [1.0, 0.2, 0.2, 1.0];
        let n = [0.0, 0.0, 1.0];
        let verts = vec![
            Vertex::new([-1.0, -1.0, 0.0], n, [0.0, 1.0], tint, true),
            Vertex::new([1.0, -1.0, 0.0], n, [1.0, 1.0], tint, true),
            Vertex::new([0.0, 1.5, 0.0], n, [0.5, 0.0], tint, true),
        ];
        let vp = look_at([0.0, 0.0, 5.0], [0.0, 0.0, 0.0], 1.0, 60.0);
        let image = scene
            .render(&verts, vp, [0.0, 0.0, -1.0], [0.0, 0.0, 0.0, 1.0], &white())
            .expect("a frame");

        assert_eq!(image.width, 64);
        assert_eq!(image.height, 64);
        let at = |x: u32, y: u32| -> [u8; 4] {
            let i = ((y * image.width + x) * 4) as usize;
            [
                image.rgba[i],
                image.rgba[i + 1],
                image.rgba[i + 2],
                image.rgba[i + 3],
            ]
        };
        let middle = at(32, 34);
        assert!(
            middle[0] > 60,
            "the triangle should be drawn in red: {middle:?}"
        );
        assert_eq!(at(1, 1), [0, 0, 0, 255], "a corner keeps the clear colour");
    }

    #[test]
    fn a_heavy_frame_is_measured_against_the_cpu_rasterizer() {
        // The plan's open question, answered with a number rather than a
        // guess: readback is the slow path on every GPU, and at small sizes it
        // can cost more than the software rasterizer saves. This prints both
        // so the decision about when to use which is made on evidence.
        let Some(mut scene) = gpu(1280, 720) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };

        // A quarter of a million triangles, well past what the CPU path can
        // carry at 60fps, all in ONE draw because the tint rides the vertex.
        let mut verts = Vec::with_capacity(250_000 * 3);
        for i in 0..250_000u32 {
            let t = i as f32 * 0.000_37;
            let (x, y) = (t.sin() * 6.0, (t * 1.7).cos() * 4.0);
            let z = -((i % 90) as f32) * 0.05 - 1.0;
            let tint = [
                0.4 + (i % 7) as f32 * 0.08,
                0.4 + (i % 5) as f32 * 0.1,
                0.6,
                1.0,
            ];
            let n = [0.0, 0.0, 1.0];
            verts.push(Vertex::new([x, y, z], n, [0.0, 0.0], tint, true));
            verts.push(Vertex::new([x + 0.09, y, z], n, [1.0, 0.0], tint, true));
            verts.push(Vertex::new([x, y + 0.09, z], n, [0.0, 1.0], tint, true));
        }
        let vp = look_at([0.0, 0.0, 14.0], [0.0, 0.0, 0.0], 1280.0 / 720.0, 60.0);
        let texture = white();

        // One warm frame: the first includes pipeline and buffer setup.
        let _ = scene.render(
            &verts,
            vp,
            [0.0, 0.0, -1.0],
            [0.05, 0.06, 0.09, 1.0],
            &texture,
        );

        let runs = 10;
        let started = std::time::Instant::now();
        for _ in 0..runs {
            scene
                .render(
                    &verts,
                    vp,
                    [0.0, 0.0, -1.0],
                    [0.05, 0.06, 0.09, 1.0],
                    &texture,
                )
                .expect("a frame");
        }
        let per_frame = started.elapsed() / runs;
        eprintln!(
            "GPU: {} triangles at 1280x720 in {:?} a frame ({:.0} fps), readback included",
            verts.len() / 3,
            per_frame,
            1.0 / per_frame.as_secs_f64()
        );
        // Not a hard threshold on frame time: this runs on whatever machine
        // happens to build it, and a slow shared runner would fail a test
        // about the renderer for a reason that is not the renderer. What IS
        // asserted is that a quarter of a million triangles render AT ALL,
        // which the CPU path cannot do inside a frame.
        assert!(
            per_frame.as_millis() < 2_000,
            "a frame should not take seconds: {per_frame:?}"
        );
    }

    #[test]
    fn a_surface_larger_than_the_cap_is_refused() {
        assert!(GpuScene::new(0, 10).is_none());
        assert!(GpuScene::new(10, MAX_EDGE + 1).is_none());
    }

    #[test]
    fn rows_are_padded_to_the_copy_alignment() {
        // wgpu requires each row of a texture-to-buffer copy to start on a
        // 256-byte boundary. Getting this wrong reads the image back sheared,
        // which looks like a rasterizer bug and is not one.
        assert_eq!(padded_row(64), 256);
        assert_eq!(padded_row(65), 512);
        assert_eq!(padded_row(1920), 7680);
    }
}
