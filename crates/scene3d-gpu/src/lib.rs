//! The GPU backend for `gfx.scene3d`.
//!
//! Plan/GPU-Scene3d-2026-09-17.md. A wgpu render pass with a depth buffer,
//! rendered to a texture and read back as the same [`ImagePixels`] the CPU
//! rasterizer produces.
//!
//! Reading back rather than presenting the texture directly is a deliberate
//! first step, not the end state. It keeps the seam exactly where it already
//! is -- `render_image()` returns an image and everything downstream takes one
//! -- so nothing above it has to change: not the host, not the three adapters,
//! not `krate run --shoot`. It also costs a stall on every frame, which is why
//! G1 measured rather than assumed. It wins anyway: 250,000 triangles at
//! 1280x720 in 13.9 ms, readback included.
//!
//! The CPU rasterizer stays. It is the fallback where no adapter exists, the
//! reference an image diff compares against, and what CI proves, since CI has
//! no GPU.
//!
//! **The method names and behaviour mirror `krate_runtime::scene3d::Scene`
//! deliberately**, so the host can hold either one behind the same calls.
//! Where this file makes a different choice -- the tint riding the vertex,
//! textures living in an array rather than being bound one at a time -- the
//! comment says why.

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

/// Largest edge of an uploaded texture, matching the CPU path's limit.
///
/// The same number on both backends on purpose: an app that uploads a texture
/// must not have it accepted on one machine and refused on another.
pub const MAX_TEXTURE_EDGE: u32 = 1_024;

/// How far the depth range reaches. The CPU path compares raw camera-space z
/// with no near or far plane at all; a depth buffer needs a bounded range, and
/// this is chosen large enough that no scene a Krate app builds reaches it.
const FAR: f32 = 10_000.0;

/// Edge of the shadow map, in texels.
///
/// 2048 rather than 1024: the map covers a square the app sizes with
/// `shadow_radius`, so every doubling of radius halves the detail. At 2048 a
/// 40-unit plaza gets a texel every 4 cm, which is finer than an eye finds an
/// edge at that distance. It costs 16 MB of depth, once.
const SHADOW_EDGE: u32 = 2_048;

/// Every texture layer is this square. One size for all of them is what an
/// array texture requires, and 1024 matches `MAX_TEXTURE_EDGE` so no upload
/// the CPU path accepts has to be shrunk here.
const LAYER_EDGE: u32 = 1_024;

/// Bit 0 of a vertex's flags: this vertex carries a real normal.
const FLAG_SMOOTH: u32 = 1;
/// Bit 1: this triangle wears a real texture rather than the white fallback.
/// The two CPU shading paths clamp the tint differently and the shader has to
/// know which it is reproducing.
const FLAG_TEXTURED: u32 = 2;

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
    /// Bit 0 is the smooth flag; bits 8 and up are the texture layer.
    pub flags: u32,
    /// Padding to a 16-byte multiple, which wgpu requires of a vertex stride
    /// and which is cheaper to state than to let the layout accidentally
    /// depend on the field order.
    pub _pad: [u32; 3],
}

impl Vertex {
    fn new(
        position: [f32; 3],
        normal: [f32; 3],
        uv: [f32; 2],
        tint: [f32; 4],
        smooth: bool,
        textured: bool,
        layer: u32,
    ) -> Self {
        Self {
            position,
            normal,
            uv,
            tint,
            flags: (if smooth { FLAG_SMOOTH } else { 0 })
                | (if textured { FLAG_TEXTURED } else { 0 })
                | (layer << 8),
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

/// The camera basis and light, as the shader's uniform block.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CameraUniform {
    right: [f32; 4],
    up: [f32; 4],
    forward: [f32; 4],
    eye: [f32; 4],
    lens: [f32; 4],
    light: [f32; 4],
    surface: [f32; 4],
    fog: [f32; 4],
    fill_dir: [f32; 4],
    fill_color: [f32; 4],
    light_view_proj: [[f32; 4]; 4],
    shadow: [f32; 4],
}

/// How a scene is lit, beyond the one directional light.
///
/// The default is exactly what the software rasterizer does, so a scene that
/// never sets this is pixel-identical on both backends. Every field here is
/// something the CPU path cannot do, which is why the default has to be "as
/// though none of it exists".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    /// Brightness a surface facing away from the light still has, for a mesh
    /// drawn through `triangles` or `textured`.
    pub ambient_flat: f32,
    /// The same, for a mesh drawn through `smooth`.
    pub ambient_smooth: f32,
    /// How sharp a highlight the surface takes: 0 is chalk, 1 is polished.
    pub specular: f32,
    /// How tight the highlight is: 8 is a wide sheen, 128 a small hot spot.
    pub shininess: f32,
    /// How quickly distance fades into `fog_color`. Zero is off.
    pub fog_density: f32,
    pub fog_color: [f32; 3],
    /// A second directional light, or `None`.
    pub fill: Option<([f32; 3], [f32; 3])>,
    /// How far around the camera cast shadows reach, in world units. Zero is
    /// off.
    ///
    /// A radius rather than a world box because a shadow map is a fixed grid
    /// of texels spread over whatever area it covers: cover a two-kilometre
    /// circuit and each texel is metres wide, which is a shadow with stairs on
    /// it. Following the camera keeps the detail where somebody is looking.
    pub shadow_radius: f32,
    /// Shadow edge blur, in texels. Zero is a hard edge.
    pub shadow_softness: f32,
}

impl Default for Lighting {
    fn default() -> Self {
        Self {
            // The two shading floors the software rasterizer uses. Changing
            // either default would change every existing scene.
            ambient_flat: 0.35,
            ambient_smooth: 0.22,
            specular: 0.0,
            shininess: 32.0,
            fog_density: 0.0,
            fog_color: [0.6, 0.7, 0.8],
            fill: None,
            shadow_radius: 0.0,
            shadow_softness: 1.0,
        }
    }
}

/// A triangle waiting to be drawn, kept so the blended pass can be sorted.
struct Queued {
    verts: [Vertex; 3],
    /// Coverage below 1 means this triangle blends and must not write depth.
    alpha: f32,
    /// Camera-space depth at the centroid, for the back-to-front sort.
    sort_z: f32,
}

struct LayerInfo {
    pixels: Vec<u8>,
}

/// One GPU-side 3D scene. Mirrors `krate_runtime::scene3d::Scene`.
pub struct GpuScene {
    device: wgpu::Device,
    queue: wgpu::Queue,
    opaque_pipeline: wgpu::RenderPipeline,
    blend_pipeline: wgpu::RenderPipeline,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,

    /// The shadow pass: the scene drawn from the light, depth only.
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_layout: wgpu::BindGroupLayout,
    shadow_map: wgpu::Texture,
    /// A COMPARISON sampler, so the hardware does the depth test and averages
    /// several taps for free -- which is what makes a soft edge cheap.
    shadow_sampler: wgpu::Sampler,

    width: u32,
    height: u32,
    colour: wgpu::Texture,
    depth: wgpu::Texture,
    readback: wgpu::Buffer,
    padded_row_bytes: u32,

    /// Uploaded textures, as layers of one array. The CPU path holds a list
    /// and binds by index per triangle; a GPU draw cannot rebind per triangle
    /// without becoming one draw per triangle, so the layer index rides the
    /// vertex and every texture lives in one array.
    ///
    /// The cost is that every layer must be the same size, so each upload is
    /// scaled into a fixed cell. Stretching rather than padding keeps `uv`
    /// meaning the same fraction of the image it means on the CPU path.
    layers: Vec<LayerInfo>,
    atlas: Option<wgpu::Texture>,
    atlas_dirty: bool,

    eye: [f32; 3],
    look_at: [f32; 3],
    fov_degrees: f32,
    light: [f32; 3],
    cull_back_faces: bool,
    clear: [f32; 4],
    lighting: Lighting,
    queued: Vec<Queued>,
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
                        view_dimension: wgpu::TextureViewDimension::D2Array,
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
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene3d layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            ..Default::default()
        });

        let make_pipeline = |blended: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if blended {
                    "scene3d blended"
                } else {
                    "scene3d opaque"
                }),
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
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // Culling happens on the CPU side by the sign of the
                    // screen-space area, exactly as the software path decides
                    // it. Letting the hardware cull by winding instead would
                    // be a second rule that has to agree with the first, and
                    // the CPU path's comment records that this sign has
                    // already been got wrong once.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    // A blended triangle must NOT write depth: the surface
                    // behind it stays visible through it, so it has to stay
                    // available for anything drawn later. Same rule as the
                    // CPU path, and the reason there are two pipelines.
                    depth_write_enabled: Some(!blended),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let opaque_pipeline = make_pipeline(false);
        let blend_pipeline = make_pipeline(true);

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

        // The shadow pass: its own shader, layout and pipeline. Depth only,
        // so there is no fragment stage and it costs a fraction of the main
        // pass -- no texturing, no lighting, no blending.
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene3d shadow"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shadow.wgsl").into()),
        });
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene3d shadow bindings"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("scene3d shadow layout"),
                bind_group_layouts: &[Some(&shadow_layout)],
                ..Default::default()
            });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene3d shadow pipeline"),
            layout: Some(&shadow_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_main"),
                // The same vertex layout as the main pass, so one buffer
                // serves both. The shadow shader reads only position and
                // ignores the rest.
                buffers: &[Vertex::layout()],
                compilation_options: Default::default(),
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
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
        let shadow_map = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene3d shadow map"),
            size: wgpu::Extent3d {
                width: SHADOW_EDGE,
                height: SHADOW_EDGE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene3d shadow sampler"),
            // Clamp: a point outside the map is outside the radius the app
            // asked for, and repeating would wrap a shadow round the world.
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
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
            opaque_pipeline,
            blend_pipeline,
            bind_layout,
            sampler,
            shadow_pipeline,
            shadow_layout,
            shadow_map,
            shadow_sampler,
            width,
            height,
            colour,
            depth,
            readback,
            padded_row_bytes,
            layers: Vec::new(),
            atlas: None,
            atlas_dirty: true,
            eye: [0.0, 0.0, 4.0],
            look_at: [0.0, 0.0, 0.0],
            fov_degrees: 60.0,
            light: normalize([-0.4, -0.7, -0.6]),
            cull_back_faces: false,
            clear: [0.063, 0.078, 0.125, 1.0],
            lighting: Lighting::default(),
            queued: Vec::new(),
        })
    }

    fn targets(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::Texture) {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        // `Rgba8Unorm`, NOT `Rgba8UnormSrgb`, here and for the texture array.
        //
        // The sRGB format makes the hardware encode linear values on write and
        // decode them on read, and the CPU rasterizer does no such conversion
        // -- it writes the bytes its arithmetic produced. With an sRGB target
        // the two backends disagreed by exactly one gamma curve: the parity
        // test measured cpu 204 against gpu 231 and cpu 16 against gpu 71,
        // which is `1.055*c^(1/2.4)-0.055` to the integer on every channel.
        //
        // Matching the CPU path matters more than being colour-correct here,
        // because the image reaches the same widget either way. If this
        // pipeline ever gains real colour management, both backends should
        // gain it together.
        let colour = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene3d colour"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
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

    // ---------------------------------------------------- the CPU path's API

    /// Clear colour and depth. Call this at the start of every frame.
    pub fn clear(&mut self, sky: [f32; 4]) {
        // Anything queued and not yet drawn belongs to the frame being
        // cleared, so it is dropped rather than drawn over the new sky.
        self.queued.clear();
        self.clear = sky;
    }

    pub fn set_camera(&mut self, eye: [f32; 3], look_at: [f32; 3], fov_degrees: f32) {
        self.eye = eye;
        self.look_at = look_at;
        // Degenerate fields of view produce a projection that divides by zero
        // or inverts the scene; clamped rather than refused, matching the CPU
        // path, because an app sweeping a zoom through a bad value should
        // distort rather than fail.
        self.fov_degrees = fov_degrees.clamp(5.0, 150.0);
    }

    pub fn set_cull_back_faces(&mut self, enabled: bool) {
        self.cull_back_faces = enabled;
    }

    /// Set how the scene is lit beyond the one directional light.
    ///
    /// Sanitised rather than refused: a NaN shininess or a negative fog
    /// density is an app bug, and the honest response is to render something
    /// sensible rather than to fail a frame the author cannot see.
    pub fn set_lighting(&mut self, lighting: Lighting) {
        let clean = |v: f32, fallback: f32| if v.is_finite() { v } else { fallback };
        self.lighting = Lighting {
            ambient_flat: clean(lighting.ambient_flat, 0.35).clamp(0.0, 1.0),
            ambient_smooth: clean(lighting.ambient_smooth, 0.22).clamp(0.0, 1.0),
            specular: clean(lighting.specular, 0.0).clamp(0.0, 4.0),
            // Below 1 the exponent inverts the highlight into a dark patch.
            shininess: clean(lighting.shininess, 32.0).clamp(1.0, 512.0),
            fog_density: clean(lighting.fog_density, 0.0).max(0.0),
            fog_color: lighting.fog_color,
            fill: lighting.fill.map(|(dir, colour)| (normalize(dir), colour)),
            shadow_radius: clean(lighting.shadow_radius, 0.0).max(0.0),
            shadow_softness: clean(lighting.shadow_softness, 1.0).clamp(0.0, 4.0),
        };
    }

    pub fn lighting(&self) -> Lighting {
        self.lighting
    }

    pub fn set_light(&mut self, direction: [f32; 3]) {
        let light = normalize(direction);
        // A zero-length direction would light nothing at all and read as a
        // rendering bug; keep the previous light instead.
        if length(light) > 0.0 {
            self.light = light;
        }
    }

    /// Upload an image and get a handle, one-based so zero stays invalid.
    pub fn upload_texture(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<u64, UiAdapterError> {
        let expected = width as usize * height as usize * 4;
        if width == 0 || height == 0 || rgba.len() != expected {
            return Err(UiAdapterError::Unsupported(format!(
                "a {width}x{height} texture needs exactly {expected} RGBA bytes, got {}",
                rgba.len()
            )));
        }
        if width > MAX_TEXTURE_EDGE || height > MAX_TEXTURE_EDGE {
            return Err(UiAdapterError::Unsupported(format!(
                "a texture may be at most {MAX_TEXTURE_EDGE}x{MAX_TEXTURE_EDGE}, \
                 got {width}x{height}"
            )));
        }
        // Scaled into the array's fixed cell. Every layer of an array texture
        // is the same size, and stretching rather than padding keeps `uv`
        // meaning the same fraction of the image it means on the CPU path.
        self.layers.push(LayerInfo {
            pixels: scale_to(rgba, width, height, LAYER_EDGE),
        });
        self.atlas_dirty = true;
        Ok(self.layers.len() as u64)
    }

    /// Draw a flat list of `x,y,z` triples as triangles, one tint for all.
    pub fn triangles(&mut self, vertices: &[f32], tint: [f32; 4]) {
        for chunk in vertices.chunks_exact(9) {
            let a = [chunk[0], chunk[1], chunk[2]];
            let b = [chunk[3], chunk[4], chunk[5]];
            let c = [chunk[6], chunk[7], chunk[8]];
            self.push_face(a, b, c, None, [[0.0; 2]; 3], 0, tint, false, false);
        }
    }

    /// Draw triangles wearing a texture.
    pub fn textured(&mut self, vertices: &[f32], uvs: &[f32], texture: u64, tint: [f32; 4]) {
        let Some(layer) = self.layer_of(texture) else {
            return;
        };
        for (chunk, uv) in vertices.chunks_exact(9).zip(uvs.chunks_exact(6)) {
            let a = [chunk[0], chunk[1], chunk[2]];
            let b = [chunk[3], chunk[4], chunk[5]];
            let c = [chunk[6], chunk[7], chunk[8]];
            let uv = [[uv[0], uv[1]], [uv[2], uv[3]], [uv[4], uv[5]]];
            self.push_face(a, b, c, None, uv, layer, tint, false, true);
        }
    }

    /// Draw triangles with a normal per vertex, so curvature shades smoothly.
    pub fn smooth(
        &mut self,
        vertices: &[f32],
        normals: &[f32],
        uvs: &[f32],
        texture: u64,
        tint: [f32; 4],
    ) {
        let Some(layer) = self.layer_of(texture) else {
            return;
        };
        let triples = vertices
            .chunks_exact(9)
            .zip(normals.chunks_exact(9))
            .zip(uvs.chunks_exact(6));
        for ((chunk, n), uv) in triples {
            let a = [chunk[0], chunk[1], chunk[2]];
            let b = [chunk[3], chunk[4], chunk[5]];
            let c = [chunk[6], chunk[7], chunk[8]];
            let corner = [[n[0], n[1], n[2]], [n[3], n[4], n[5]], [n[6], n[7], n[8]]];
            let uv = [[uv[0], uv[1]], [uv[2], uv[3]], [uv[4], uv[5]]];
            self.push_face(a, b, c, Some(corner), uv, layer, tint, true, true);
        }
    }

    /// Draw triangles moved by a transform, without touching the input.
    pub fn place(
        &mut self,
        vertices: &[f32],
        translate: [f32; 3],
        rotate_degrees: [f32; 3],
        scale: f32,
        tint: [f32; 4],
    ) {
        let (sx, cx) = rotate_degrees[0].to_radians().sin_cos();
        let (sy, cy) = rotate_degrees[1].to_radians().sin_cos();
        let (sz, cz) = rotate_degrees[2].to_radians().sin_cos();
        let s = if scale.is_finite() && scale != 0.0 {
            scale
        } else {
            1.0
        };
        let transform = |v: [f32; 3]| -> [f32; 3] {
            let (x, y, z) = (v[0] * s, v[1] * s, v[2] * s);
            // X, then Y, then Z, matching the order the contract states.
            let (y, z) = (y * cx - z * sx, y * sx + z * cx);
            let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
            let (x, y) = (x * cz - y * sz, x * sz + y * cz);
            [x + translate[0], y + translate[1], z + translate[2]]
        };
        for chunk in vertices.chunks_exact(9) {
            let a = transform([chunk[0], chunk[1], chunk[2]]);
            let b = transform([chunk[3], chunk[4], chunk[5]]);
            let c = transform([chunk[6], chunk[7], chunk[8]]);
            self.push_face(a, b, c, None, [[0.0; 2]; 3], 0, tint, false, false);
        }
    }

    fn layer_of(&self, texture: u64) -> Option<u32> {
        let index = (texture as usize).checked_sub(1)?;
        if index >= self.layers.len() {
            return None;
        }
        Some(index as u32)
    }

    /// Project, cull and queue one triangle.
    #[allow(clippy::too_many_arguments)]
    fn push_face(
        &mut self,
        a: [f32; 3],
        b: [f32; 3],
        c: [f32; 3],
        corner_normals: Option<[[f32; 3]; 3]>,
        uv: [[f32; 2]; 3],
        layer: u32,
        tint: [f32; 4],
        smooth: bool,
        textured: bool,
    ) {
        let basis = self.basis();
        let (Some(pa), Some(pb), Some(pc)) = (
            self.project(a, &basis),
            self.project(b, &basis),
            self.project(c, &basis),
        ) else {
            // A triangle with any corner behind the camera is dropped whole
            // rather than clipped -- the same hole the CPU path has (K-397),
            // kept deliberately so the two backends agree. Fixing it should
            // fix both at once.
            return;
        };
        let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (pb.1 - pa.1);
        if area.abs() < 1e-6 {
            return;
        }
        // Negative screen area is a back face, matching the CPU path's sign
        // and the winding the WIT documents.
        if self.cull_back_faces && area < 0.0 {
            return;
        }

        let face = face_normal(a, b, c);
        let normals = corner_normals.unwrap_or([face; 3]);
        let alpha = if tint[3].is_nan() {
            1.0
        } else {
            tint[3].clamp(0.0, 1.0)
        };
        self.queued.push(Queued {
            verts: [
                Vertex::new(a, normals[0], uv[0], tint, smooth, textured, layer),
                Vertex::new(b, normals[1], uv[1], tint, smooth, textured, layer),
                Vertex::new(c, normals[2], uv[2], tint, smooth, textured, layer),
            ],
            alpha,
            sort_z: (pa.2 + pb.2 + pc.2) / 3.0,
        });
    }

    /// The camera basis: right, up, forward.
    ///
    /// `up x forward`, not `forward x up`. The other order points "right" at
    /// -X, which mirrors the whole scene horizontally -- the CPU path's
    /// comment records that as a racing game where pressing right went left.
    fn basis(&self) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let forward = normalize(sub(self.look_at, self.eye));
        let world_up = [0.0, 1.0, 0.0];
        let right = normalize(cross(world_up, forward));
        let up = cross(forward, right);
        (right, up, forward)
    }

    /// Project to screen space plus camera depth, or `None` behind the eye.
    fn project(
        &self,
        point: [f32; 3],
        basis: &([f32; 3], [f32; 3], [f32; 3]),
    ) -> Option<(f32, f32, f32)> {
        let (right, up, forward) = *basis;
        let rel = sub(point, self.eye);
        let camera_z = dot(rel, forward);
        if camera_z <= 0.01 {
            return None;
        }
        let half_fov = (self.fov_degrees.to_radians() / 2.0).tan();
        let aspect = self.width as f32 / self.height as f32;
        let ndc_x = dot(rel, right) / (camera_z * half_fov * aspect);
        // Screen Y grows downward; world Y grows up.
        let ndc_y = -dot(rel, up) / (camera_z * half_fov);
        Some((
            (ndc_x + 1.0) * 0.5 * self.width as f32,
            (ndc_y + 1.0) * 0.5 * self.height as f32,
            camera_z,
        ))
    }

    /// Draw everything queued and read the frame back.
    pub fn render_image(&mut self) -> Result<ImagePixels, UiAdapterError> {
        // Opaque first, then blended back to front -- the same two-pass order
        // the CPU path uses, and for the same reason: a blended triangle does
        // not write depth, so nothing but draw order decides which of two
        // blends lands on top.
        let mut queued = core::mem::take(&mut self.queued);
        queued.sort_by(|a, b| {
            let (ab, bb) = (a.alpha < 1.0, b.alpha < 1.0);
            match (ab, bb) {
                (false, true) => core::cmp::Ordering::Less,
                (true, false) => core::cmp::Ordering::Greater,
                (true, true) => b.sort_z.total_cmp(&a.sort_z),
                (false, false) => core::cmp::Ordering::Equal,
            }
        });
        let blend_from = queued.partition_point(|q| q.alpha >= 1.0);

        let mut verts: Vec<Vertex> = Vec::with_capacity(queued.len() * 3);
        for q in &queued {
            verts.extend_from_slice(&q.verts);
        }

        self.ensure_atlas();
        let result = self.draw(&verts, blend_from * 3);
        // Reuse the allocation next frame rather than growing it again.
        queued.clear();
        self.queued = queued;
        result
    }

    /// Build or rebuild the array texture from the uploaded layers.
    fn ensure_atlas(&mut self) {
        if !self.atlas_dirty {
            return;
        }
        // At least one layer, always: a shader that samples an array with no
        // layers is invalid, and a scene that only ever calls `triangles` has
        // uploaded nothing. A white layer makes the untextured path a textured
        // one wearing white, which is exactly what the CPU path's flat-colour
        // arithmetic amounts to.
        let count = self.layers.len().max(1) as u32;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene3d atlas"),
            size: wgpu::Extent3d {
                width: LAYER_EDGE,
                height: LAYER_EDGE,
                depth_or_array_layers: count,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let white = vec![255u8; (LAYER_EDGE * LAYER_EDGE * 4) as usize];
        for layer in 0..count {
            let pixels = self
                .layers
                .get(layer as usize)
                .map(|l| l.pixels.as_slice())
                .unwrap_or(&white);
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(LAYER_EDGE * 4),
                    rows_per_image: Some(LAYER_EDGE),
                },
                wgpu::Extent3d {
                    width: LAYER_EDGE,
                    height: LAYER_EDGE,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.atlas = Some(texture);
        self.atlas_dirty = false;
    }

    fn draw(&self, verts: &[Vertex], blend_from: usize) -> Result<ImagePixels, UiAdapterError> {
        let (right, up, forward) = self.basis();
        let uniform = CameraUniform {
            right: [right[0], right[1], right[2], 0.0],
            up: [up[0], up[1], up[2], 0.0],
            forward: [forward[0], forward[1], forward[2], 0.0],
            eye: [self.eye[0], self.eye[1], self.eye[2], 0.0],
            lens: [
                (self.fov_degrees.to_radians() / 2.0).tan(),
                self.width as f32 / self.height as f32,
                1.0 / FAR,
                0.0,
            ],
            light: [self.light[0], self.light[1], self.light[2], 0.0],
            surface: [
                self.lighting.ambient_flat,
                self.lighting.ambient_smooth,
                self.lighting.specular,
                self.lighting.shininess,
            ],
            fog: [
                self.lighting.fog_color[0],
                self.lighting.fog_color[1],
                self.lighting.fog_color[2],
                self.lighting.fog_density,
            ],
            fill_dir: match self.lighting.fill {
                Some((dir, _)) => [dir[0], dir[1], dir[2], 1.0],
                None => [0.0, 0.0, 0.0, 0.0],
            },
            fill_color: match self.lighting.fill {
                Some((_, colour)) => [colour[0], colour[1], colour[2], 1.0],
                None => [0.0, 0.0, 0.0, 0.0],
            },
            light_view_proj: light_matrix(self.light, self.eye, self.lighting.shadow_radius),
            shadow: [
                if self.lighting.shadow_radius > 0.0 {
                    1.0
                } else {
                    0.0
                },
                self.lighting.shadow_softness,
                // One texel in light clip units, which the shader steps by to
                // blur the edge.
                2.0 / SHADOW_EDGE as f32,
                0.0,
            ],
        };
        let uniform_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d camera"),
            size: core::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&uniform_buffer, 0, bytemuck::bytes_of(&uniform));

        let atlas = self.atlas.as_ref().ok_or_else(|| {
            UiAdapterError::Unsupported("the texture array was not built".to_string())
        })?;
        // The layer count is stated explicitly. Left to the default the view
        // can cover a single layer, and every triangle then samples the same
        // texture whatever layer index it asks for -- which reads as "the
        // layer never reached the shader" and is not that at all.
        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            base_array_layer: 0,
            array_layer_count: Some(self.layers.len().max(1) as u32),
            ..Default::default()
        });
        let shadow_view = self.shadow_map.create_view(&Default::default());
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                },
            ],
        });

        let vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene3d vertices"),
            size: (core::mem::size_of::<Vertex>() * verts.len().max(1)) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if !verts.is_empty() {
            self.queue
                .write_buffer(&vertex_buffer, 0, bytemuck::cast_slice(verts));
        }

        let colour_view = self.colour.create_view(&Default::default());
        let depth_view = self.depth.create_view(&Default::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scene3d frame"),
            });

        // The shadow pass, first: the scene from the light, depth only.
        //
        // The map is cleared and redrawn even when shadows are off, because a
        // stale map bound into the main pass would shadow the scene with
        // whatever was there last -- and a cleared one reads as "nothing in
        // the way", which is what off should look like.
        {
            let shadow_uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("scene3d shadow camera"),
                size: core::mem::size_of::<[[f32; 4]; 4]>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(
                &shadow_uniform,
                0,
                bytemuck::bytes_of(&uniform.light_view_proj),
            );
            let shadow_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("scene3d shadow bind group"),
                layout: &self.shadow_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: shadow_uniform.as_entire_binding(),
                }],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene3d shadow pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &shadow_view,
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
            // Only the OPAQUE triangles cast. A pane of glass casting a solid
            // black shadow is worse than it casting none, and sorting out what
            // a transparent caster should do needs more than a depth buffer.
            if self.lighting.shadow_radius > 0.0 && blend_from > 0 {
                pass.set_pipeline(&self.shadow_pipeline);
                pass.set_bind_group(0, &shadow_bind, &[]);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                pass.draw(0..blend_from as u32, 0..1);
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene3d pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &colour_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(self.clear[0]),
                            g: f64::from(self.clear[1]),
                            b: f64::from(self.clear[2]),
                            a: f64::from(self.clear[3]),
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
            if !verts.is_empty() {
                pass.set_bind_group(0, &bind_group, &[]);
                pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                // Two draws, not two buffers: the vertices are already sorted
                // opaque-then-blended, so the split is an index.
                if blend_from > 0 {
                    pass.set_pipeline(&self.opaque_pipeline);
                    pass.draw(0..blend_from as u32, 0..1);
                }
                if blend_from < verts.len() {
                    pass.set_pipeline(&self.blend_pipeline);
                    pass.draw(blend_from as u32..verts.len() as u32, 0..1);
                }
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
        // and the reason G1 measured: on a small surface it can cost more than
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
}

// ------------------------------------------------------------------- helpers

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn length(v: [f32; 3]) -> f32 {
    dot(v, v).sqrt()
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = length(v);
    if l <= f32::EPSILON {
        [0.0, 0.0, 0.0]
    } else {
        [v[0] / l, v[1] / l, v[2] / l]
    }
}

/// World-to-light-clip for a directional light, orthographic.
///
/// ORTHOGRAPHIC, not perspective: a directional light has no position, its
/// rays are parallel, and a perspective divide would bend them into a point
/// source. The box is centred on `focus` -- the camera, so the detail follows
/// whoever is looking -- and sized by `radius`.
///
/// Returned column-major, which is what WGSL's `mat4x4` reads.
fn light_matrix(direction: [f32; 3], focus: [f32; 3], radius: f32) -> [[f32; 4]; 4] {
    let forward = normalize(direction);
    // Any axis not parallel to the light will do for the cross product; the
    // usual +Y fails for a light pointing straight down, which is exactly
    // where a sun ends up at noon.
    let reference = if forward[1].abs() > 0.95 {
        [0.0, 0.0, 1.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let right = normalize(cross(reference, forward));
    let up = cross(forward, right);

    // Stand the light back far enough that everything inside the radius is in
    // front of it, and reach twice that far, so a tall object outside the
    // radius still casts into it.
    let back = radius * 2.0;
    let eye = [
        focus[0] - forward[0] * back,
        focus[1] - forward[1] * back,
        focus[2] - forward[2] * back,
    ];
    let depth = back * 2.0;
    let inv_r = 1.0 / radius.max(0.001);
    let inv_d = 1.0 / depth.max(0.001);

    // Rows of the view matrix scaled by the orthographic extents, transposed
    // into columns.
    let tx = -dot(right, eye) * inv_r;
    let ty = -dot(up, eye) * inv_r;
    let tz = -dot(forward, eye) * inv_d;
    [
        [right[0] * inv_r, up[0] * inv_r, forward[0] * inv_d, 0.0],
        [right[1] * inv_r, up[1] * inv_r, forward[1] * inv_d, 0.0],
        [right[2] * inv_r, up[2] * inv_r, forward[2] * inv_d, 0.0],
        [tx, ty, tz, 1.0],
    ]
}

fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    normalize(cross(sub(b, a), sub(c, a)))
}

/// Nearest-neighbour scale into a square cell.
///
/// Nearest rather than bilinear on purpose: the CPU sampler is nearest, and a
/// smoother upload here would make the two backends disagree on every texture
/// small enough to be magnified -- which is most of them, since a 2x2 test
/// texture and a 64x64 tile both end up in a 1024 cell.
fn scale_to(rgba: &[u8], width: u32, height: u32, edge: u32) -> Vec<u8> {
    let mut out = vec![0u8; (edge * edge * 4) as usize];
    for y in 0..edge {
        let sy =
            (u64::from(y) * u64::from(height) / u64::from(edge)).min(u64::from(height) - 1) as u32;
        for x in 0..edge {
            let sx = (u64::from(x) * u64::from(width) / u64::from(edge)).min(u64::from(width) - 1)
                as u32;
            let src = ((sy * width + sx) * 4) as usize;
            let dst = ((y * edge + x) * 4) as usize;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    out
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

    fn at(image: &ImagePixels, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * image.width + x) * 4) as usize;
        [
            image.rgba[i],
            image.rgba[i + 1],
            image.rgba[i + 2],
            image.rgba[i + 3],
        ]
    }

    /// A quad facing the camera at a given depth, as two triangles.
    fn facing_quad(half: f32, z: f32) -> Vec<f32> {
        vec![
            -half, -half, z, half, -half, z, half, half, z, //
            -half, -half, z, half, half, z, -half, half, z,
        ]
    }

    #[test]
    fn a_triangle_reaches_the_screen_through_the_gpu() {
        let Some(mut scene) = gpu(64, 64) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        scene.set_camera([0.0, 0.0, 5.0], [0.0, 0.0, 0.0], 60.0);
        scene.set_light([0.0, 0.0, -1.0]);
        scene.clear([0.0, 0.0, 0.0, 1.0]);
        scene.triangles(
            &[-1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 0.0, 1.5, 0.0],
            [1.0, 0.2, 0.2, 1.0],
        );
        let image = scene.render_image().expect("a frame");

        assert_eq!((image.width, image.height), (64, 64));
        let middle = at(&image, 32, 34);
        assert!(
            middle[0] > 60,
            "the triangle should be drawn in red: {middle:?}"
        );
        assert_eq!(
            at(&image, 1, 1),
            [0, 0, 0, 255],
            "a corner keeps the clear colour"
        );
    }

    #[test]
    fn the_nearer_surface_wins_the_depth_test() {
        let Some(mut scene) = gpu(64, 64) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        scene.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
        scene.set_light([0.0, 0.0, -1.0]);
        scene.clear([0.0, 0.0, 0.0, 1.0]);
        // The far one is queued FIRST, so only the depth test can decide this.
        scene.triangles(&facing_quad(2.0, -2.0), [1.0, 0.0, 0.0, 1.0]);
        scene.triangles(&facing_quad(2.0, 2.0), [0.0, 1.0, 0.0, 1.0]);
        let middle = at(&scene.render_image().expect("a frame"), 32, 32);
        assert!(
            middle[1] > middle[0],
            "the nearer green quad must win: {middle:?}"
        );
    }

    #[test]
    fn transparent_surfaces_blend_over_what_is_behind_them() {
        let Some(mut scene) = gpu(64, 64) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        scene.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
        scene.set_light([0.0, 0.0, -1.0]);
        scene.clear([0.0, 0.0, 0.0, 1.0]);
        // An opaque red wall behind, then a half-covering blue pane in front.
        scene.triangles(&facing_quad(3.0, -1.0), [1.0, 0.0, 0.0, 1.0]);
        scene.triangles(&facing_quad(2.0, 2.0), [0.0, 0.0, 1.0, 0.5]);
        let middle = at(&scene.render_image().expect("a frame"), 32, 32);
        assert!(
            middle[0] > 20 && middle[2] > 20,
            "a half-transparent pane over a wall must show BOTH: {middle:?}"
        );
    }

    #[test]
    fn back_faces_are_culled_when_the_scene_asks() {
        let Some(mut scene) = gpu(64, 64) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        // Wound clockwise as seen from the camera, which is the back face the
        // WIT says to drop.
        let away: Vec<f32> = vec![-1.0, -1.0, 0.0, 0.0, 1.5, 0.0, 1.0, -1.0, 0.0];
        scene.set_camera([0.0, 0.0, 5.0], [0.0, 0.0, 0.0], 60.0);
        scene.set_light([0.0, 0.0, -1.0]);

        scene.clear([0.0, 0.0, 0.0, 1.0]);
        scene.triangles(&away, [1.0, 0.2, 0.2, 1.0]);
        let drawn = at(&scene.render_image().expect("a frame"), 32, 34);

        scene.set_cull_back_faces(true);
        scene.clear([0.0, 0.0, 0.0, 1.0]);
        scene.triangles(&away, [1.0, 0.2, 0.2, 1.0]);
        let culled = at(&scene.render_image().expect("a frame"), 32, 34);

        assert!(drawn[0] > 60, "with culling off it draws: {drawn:?}");
        assert_eq!(
            culled,
            [0, 0, 0, 255],
            "with culling on the back face is skipped: {culled:?}"
        );
    }

    #[test]
    fn several_textures_can_be_worn_in_one_frame() {
        // The CPU path binds a texture per triangle; a GPU draw cannot, so
        // every texture lives in one array and the layer rides the vertex.
        // This is the test that the layer index actually reaches the shader.
        let Some(mut scene) = gpu(64, 64) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        let red = scene
            .upload_texture(1, 1, &[255, 0, 0, 255])
            .expect("texture");
        let blue = scene
            .upload_texture(1, 1, &[0, 0, 255, 255])
            .expect("texture");
        scene.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
        scene.set_light([0.0, 0.0, -1.0]);
        scene.clear([0.0, 0.0, 0.0, 1.0]);

        // The two halves do not touch in x AND sit at slightly different
        // depths. Sharing a z made them tie on `depth_compare: Less`, where
        // the first one drawn keeps the pixel -- so the second half vanished
        // and the test read as "the layer index never reached the shader".
        let uv: Vec<f32> = vec![0.5; 12];
        let left: Vec<f32> = vec![
            -2.0, -2.0, 0.1, -0.1, -2.0, 0.1, -0.1, 2.0, 0.1, //
            -2.0, -2.0, 0.1, -0.1, 2.0, 0.1, -2.0, 2.0, 0.1,
        ];
        let right: Vec<f32> = vec![
            0.1, -2.0, 0.0, 2.0, -2.0, 0.0, 2.0, 2.0, 0.0, //
            0.1, -2.0, 0.0, 2.0, 2.0, 0.0, 0.1, 2.0, 0.0,
        ];
        scene.textured(&left, &uv, red, [1.0, 1.0, 1.0, 1.0]);
        scene.textured(&right, &uv, blue, [1.0, 1.0, 1.0, 1.0]);

        let image = scene.render_image().expect("a frame");
        // Read by SCREEN x, and mind that it is mirrored from world x here.
        // With the camera on +Z looking at the origin, `right` is world_up x
        // forward = -X, so a quad at negative world x appears on the right of
        // the screen. The CPU path has exactly the same basis; naming these
        // halves by world x and then reading them by screen x is how this test
        // failed twice while the renderer was correct.
        let screen_left = at(&image, 22, 32);
        let screen_right = at(&image, 42, 32);
        assert!(
            screen_left[2] > screen_left[0],
            "the +x quad wears blue and lands on the screen's LEFT: \
             {screen_left:?}"
        );
        assert!(
            screen_right[0] > screen_right[2],
            "the -x quad wears red and lands on the screen's RIGHT: \
             {screen_right:?}"
        );
    }

    #[test]
    fn corner_normals_light_one_side_only() {
        // The same rule `Scene::smooth` follows: an author who supplies
        // normals has said which way is out, so the dark side can be dark.
        let Some(mut scene) = gpu(64, 64) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        let white = scene
            .upload_texture(1, 1, &[255, 255, 255, 255])
            .expect("texture");
        let quad = facing_quad(2.0, 0.0);
        let uv: Vec<f32> = vec![0.5; 12];

        let mut shade_with = |normal: [f32; 3]| -> u8 {
            scene.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
            // Light travelling away from the camera.
            scene.set_light([0.0, 0.0, -1.0]);
            scene.clear([0.0, 0.0, 0.0, 1.0]);
            let normals: Vec<f32> = core::iter::repeat_n(normal, 6).flatten().collect();
            scene.smooth(&quad, &normals, &uv, white, [1.0, 1.0, 1.0, 1.0]);
            at(&scene.render_image().expect("a frame"), 32, 32)[1]
        };

        let lit = shade_with([0.0, 0.0, 1.0]);
        let dark = shade_with([0.0, 0.0, -1.0]);
        assert!(
            lit > dark + 30,
            "a surface facing the light must be brighter than one facing away: \
             lit {lit}, away {dark}"
        );
    }

    #[test]
    fn a_placed_mesh_lands_where_it_was_put() {
        let Some(mut scene) = gpu(64, 64) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        scene.set_camera([0.0, 0.0, 8.0], [0.0, 0.0, 0.0], 60.0);
        scene.set_light([0.0, 0.0, -1.0]);
        scene.clear([0.0, 0.0, 0.0, 1.0]);
        // A small quad at the origin, moved well to the right.
        scene.place(
            &facing_quad(0.6, 0.0),
            [2.4, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            1.0,
            [1.0, 0.2, 0.2, 1.0],
        );
        let image = scene.render_image().expect("a frame");
        // Moved to +x in the world, which lands on the screen's LEFT: `right`
        // is world_up x forward, and from +Z looking at the origin that points
        // at -X. Same basis as the CPU path.
        let moved = at(&image, 18, 32);
        let centre = at(&image, 32, 32);
        assert!(moved[0] > 60, "the mesh moved where it was put: {moved:?}");
        assert_eq!(centre, [0, 0, 0, 255], "and left the centre empty");
    }

    /// Render one quad and read the middle pixel, with whatever lighting.
    fn lit_pixel(lighting: Lighting, normal: [f32; 3]) -> Option<[u8; 4]> {
        let mut scene = gpu(64, 64)?;
        scene.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
        scene.set_light([0.0, 0.0, -1.0]);
        scene.set_lighting(lighting);
        scene.clear([0.0, 0.0, 0.0, 1.0]);
        let white = scene
            .upload_texture(1, 1, &[255, 255, 255, 255])
            .expect("texture");
        let quad = facing_quad(2.0, 0.0);
        let normals: Vec<f32> = core::iter::repeat_n(normal, 6).flatten().collect();
        let uvs: Vec<f32> = vec![0.5; 12];
        scene.smooth(&quad, &normals, &uvs, white, [0.5, 0.5, 0.5, 1.0]);
        Some(at(&scene.render_image().expect("a frame"), 32, 32))
    }

    #[test]
    fn specular_puts_a_highlight_where_the_eye_sees_the_light() {
        // Without this every surface in a scene is chalk however well it is
        // lit. The quad faces the camera and the light travels away from it,
        // so the reflection comes straight back and the highlight is at its
        // brightest -- which is the easiest case to tell apart from none.
        let Some(matte) = lit_pixel(Lighting::default(), [0.0, 0.0, 1.0]) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        let shiny = lit_pixel(
            Lighting {
                specular: 0.8,
                shininess: 24.0,
                ..Lighting::default()
            },
            [0.0, 0.0, 1.0],
        )
        .expect("a frame");
        assert!(
            shiny[0] > matte[0] + 30,
            "a specular surface must be brighter where the highlight falls: \
             matte {matte:?}, shiny {shiny:?}"
        );
    }

    #[test]
    fn a_fill_light_lifts_the_side_the_key_light_misses() {
        // A key light alone leaves the shadow side at the ambient floor, and
        // a scene lit that way reads as a lamp in a void. The normal here
        // faces AWAY from the key light, so anything above the floor came
        // from the fill.
        let away = [0.0, 0.0, -1.0];
        let Some(unfilled) = lit_pixel(Lighting::default(), away) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        let filled = lit_pixel(
            Lighting {
                // Travelling toward +Z, so it strikes the face the key misses.
                fill: Some(([0.0, 0.0, 1.0], [0.6, 0.6, 0.7])),
                ..Lighting::default()
            },
            away,
        )
        .expect("a frame");
        assert!(
            filled[2] > unfilled[2] + 10,
            "a fill light must lift the side the key misses: unfilled \
             {unfilled:?}, filled {filled:?}"
        );
    }

    #[test]
    fn fog_fades_distance_into_its_colour() {
        let Some(mut scene) = gpu(64, 64) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        scene.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
        scene.set_light([0.0, 0.0, -1.0]);
        scene.set_lighting(Lighting {
            fog_density: 0.08,
            fog_color: [1.0, 1.0, 1.0],
            ..Lighting::default()
        });
        scene.clear([0.0, 0.0, 0.0, 1.0]);
        // Two quads of the same colour at very different depths. Fog is the
        // only thing that can tell them apart.
        scene.triangles(&facing_quad(1.0, 4.0), [0.1, 0.1, 0.1, 1.0]);
        let near = at(&scene.render_image().expect("a frame"), 32, 32);

        scene.clear([0.0, 0.0, 0.0, 1.0]);
        scene.triangles(&facing_quad(1.0, -40.0), [0.1, 0.1, 0.1, 1.0]);
        let far = at(&scene.render_image().expect("a frame"), 32, 32);

        assert!(
            far[0] > near[0] + 40,
            "distance must fade into the fog colour: near {near:?}, far {far:?}"
        );
    }

    #[test]
    fn an_object_casts_a_shadow_on_what_is_below_it() {
        // The single biggest thing missing from a lit scene: an object with no
        // shadow floats, however well it is shaded.
        //
        // A small quad above a large floor, with the light straight down. The
        // floor directly under the quad must be darker than the floor beside
        // it, and only a cast shadow can do that -- both patches have the same
        // normal, the same texture and the same tint, so shading alone cannot
        // tell them apart.
        let Some(mut scene) = gpu(128, 128) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        let floor: Vec<f32> = vec![
            -20.0, 0.0, -20.0, -20.0, 0.0, 20.0, 20.0, 0.0, 20.0, //
            -20.0, 0.0, -20.0, 20.0, 0.0, 20.0, 20.0, 0.0, -20.0,
        ];
        // A blocker well above the middle of it.
        let blocker: Vec<f32> = vec![
            -2.0, 6.0, -2.0, -2.0, 6.0, 2.0, 2.0, 6.0, 2.0, //
            -2.0, 6.0, -2.0, 2.0, 6.0, 2.0, 2.0, 6.0, -2.0,
        ];

        let mut render = |radius: f32| -> ImagePixels {
            scene.set_camera([0.0, 14.0, 22.0], [0.0, 0.0, 0.0], 60.0);
            scene.set_light([0.0, -1.0, 0.0]);
            scene.set_lighting(Lighting {
                shadow_radius: radius,
                shadow_softness: 1.0,
                ..Lighting::default()
            });
            scene.clear([0.0, 0.0, 0.0, 1.0]);
            scene.triangles(&floor, [0.8, 0.8, 0.8, 1.0]);
            scene.triangles(&blocker, [0.9, 0.3, 0.3, 1.0]);
            scene.render_image().expect("a frame")
        };

        let off = render(0.0);
        let on = render(30.0);

        // Sample the floor well to one side of the blocker, and compare the
        // same pixel in both renders: with shadows off nothing changes, and
        // with them on only the shaded patch does.
        let lit_off = at(&off, 20, 100);
        let lit_on = at(&on, 20, 100);
        assert!(
            lit_on[0].abs_diff(lit_off[0]) < 20,
            "floor away from the blocker must be unchanged: {lit_off:?} vs {lit_on:?}"
        );

        // And somewhere under the blocker, the floor must have darkened. The
        // blocker itself is red and the floor grey, so a green channel that
        // dropped is floor that went into shadow.
        let mut darkened = 0;
        for y in 60..120u32 {
            for x in 40..90u32 {
                let a = at(&off, x, y);
                let b = at(&on, x, y);
                // Grey floor: red and green close together. Excludes the
                // red blocker, which is drawn over part of this region.
                let is_floor = a[0].abs_diff(a[1]) < 24 && a[1] > 30;
                if is_floor && b[1] + 25 < a[1] {
                    darkened += 1;
                }
            }
        }
        // 158 was the count when this was first written, with the 4x4 blocker
        // covering much of the region being scanned; 100 leaves room for a
        // driver that rounds an edge differently without letting a shadow that
        // has stopped working through.
        assert!(
            darkened > 100,
            "the floor under the blocker must darken: only {darkened} pixels did"
        );
    }

    #[test]
    fn lighting_defaults_are_what_the_software_rasterizer_does() {
        // The whole opt-in promise in one assertion. If either floor changed,
        // every existing scene would shade differently on the GPU and the
        // parity tests would start failing for a reason nobody intended.
        let default = Lighting::default();
        assert_eq!(default.ambient_flat, 0.35);
        assert_eq!(default.ambient_smooth, 0.22);
        assert_eq!(default.specular, 0.0);
        assert_eq!(default.fog_density, 0.0);
        assert!(default.fill.is_none());
    }

    #[test]
    fn nonsense_lighting_is_sanitised_rather_than_rendered() {
        let Some(mut scene) = gpu(16, 16) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        scene.set_lighting(Lighting {
            ambient_flat: f32::NAN,
            specular: f32::INFINITY,
            // Below 1 an exponent inverts the highlight into a dark patch.
            shininess: 0.2,
            fog_density: -4.0,
            ..Lighting::default()
        });
        let got = scene.lighting();
        assert_eq!(got.ambient_flat, 0.35, "NaN falls back to the default");
        assert!(got.specular.is_finite(), "infinity is clamped");
        assert!(got.shininess >= 1.0, "an inverting exponent is clamped");
        assert_eq!(got.fog_density, 0.0, "negative density is off, not inverse");
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

    #[test]
    fn scaling_a_texture_into_its_cell_keeps_the_corners() {
        // Nearest-neighbour, matching the CPU sampler. A 2x2 red/green/blue/
        // white texture must still have those four colours in those four
        // quadrants after being stretched into the array's cell.
        let rgba = vec![
            255, 0, 0, 255, 0, 255, 0, 255, //
            0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let scaled = scale_to(&rgba, 2, 2, 8);
        let px = |x: u32, y: u32| -> [u8; 4] {
            let i = ((y * 8 + x) * 4) as usize;
            [scaled[i], scaled[i + 1], scaled[i + 2], scaled[i + 3]]
        };
        assert_eq!(px(1, 1), [255, 0, 0, 255], "top-left stays red");
        assert_eq!(px(6, 1), [0, 255, 0, 255], "top-right stays green");
        assert_eq!(px(1, 6), [0, 0, 255, 255], "bottom-left stays blue");
        assert_eq!(px(6, 6), [255, 255, 255, 255], "bottom-right stays white");
    }

    #[test]
    fn a_heavy_frame_is_measured_against_the_cpu_rasterizer() {
        // The plan's open question, answered with a number rather than a
        // guess: readback is the slow path on every GPU, and at small sizes it
        // can cost more than the software rasterizer saves.
        let Some(mut scene) = gpu(1280, 720) else {
            eprintln!("no GPU adapter on this machine; skipping");
            return;
        };
        scene.set_camera([0.0, 0.0, 14.0], [0.0, 0.0, 0.0], 60.0);
        scene.set_light([0.0, 0.0, -1.0]);

        let mut mesh: Vec<f32> = Vec::with_capacity(250_000 * 9);
        for i in 0..250_000u32 {
            let t = i as f32 * 0.000_37;
            let (x, y) = (t.sin() * 6.0, (t * 1.7).cos() * 4.0);
            let z = -((i % 90) as f32) * 0.05 - 1.0;
            mesh.extend_from_slice(&[x, y, z, x + 0.09, y, z, x, y + 0.09, z]);
        }

        let frame = |scene: &mut GpuScene| {
            scene.clear([0.05, 0.06, 0.09, 1.0]);
            scene.triangles(&mesh, [0.6, 0.7, 0.9, 1.0]);
            scene.render_image().expect("a frame")
        };
        // One warm frame: the first builds the pipeline and the atlas.
        let _ = frame(&mut scene);

        let runs = 10;
        let started = std::time::Instant::now();
        for _ in 0..runs {
            let _ = frame(&mut scene);
        }
        let per_frame = started.elapsed() / runs;
        eprintln!(
            "GPU: 250000 triangles at 1280x720 in {per_frame:?} a frame ({:.0} fps), \
             readback included",
            1.0 / per_frame.as_secs_f64()
        );
        // Not a hard threshold on frame time: this runs on whatever machine
        // happens to build it, and a slow shared runner would fail a test
        // about the renderer for a reason that is not the renderer. What IS
        // asserted is that a quarter of a million triangles render at all,
        // which the CPU path cannot do inside a frame.
        assert!(
            per_frame.as_millis() < 2_000,
            "a frame should not take seconds: {per_frame:?}"
        );
    }
}
