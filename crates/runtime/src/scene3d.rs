//! Software 3D for the `gfx.scene3d` interface.
//!
//! A scene is a colour buffer and a depth buffer. Triangles are transformed to
//! screen space, lit by one directional light, and filled with a depth test so
//! nearer surfaces win. Presenting hands the colour buffer to the image
//! pipeline, exactly as a 2D canvas does — so 3D reaches all three operating
//! systems through code that was already proven, with no GPU dependency and no
//! new adapter work.
//!
//! Why CPU rather than wgpu: the existing rasterizer sustains roughly 400
//! million pixels a second on a laptop, which is 640x480 at sixty frames with
//! room left over. A GPU abstraction is weeks of work and assumes every
//! machine has a working driver stack. This ships now; the same interface can
//! sit in front of a GPU later.

use krate_adapter_common::ui::{ImagePixels, UiAdapterError};

/// Largest edge of a 3D surface, in pixels.
///
/// This was 1024, which refused every modern screen size: 1280x720 and up
/// were all rejected at bind, so a 3D app could not fill a window anyone
/// actually uses. The old value came with a comment estimating that 1024x1024
/// was "the sensible ceiling for a renderer with no GPU behind it". Measured,
/// that estimate was far too conservative.
///
/// The bound is pixels, so the case that decides it is not a big scene but a
/// small one drawn over itself: eight full-screen quads back to front, where
/// the depth test rejects nothing and every pixel is written eight times.
/// Measured on an M4, release, with the rasterizer's existing per-core banding:
///
/// | size | pixels | avg | worst | fps |
/// |---|---:|---:|---:|---:|
/// | 1024x640 | 0.66 M | 3.83 ms | 9.33 ms | 261 |
/// | 1280x720 | 0.92 M | 4.65 ms | 5.53 ms | 215 |
/// | 1600x900 | 1.44 M | 7.18 ms | 7.91 ms | 139 |
/// | 1920x1080 | 2.07 M | 10.37 ms | 12.46 ms | 96 |
///
/// 1080p holds 60fps in the worst case with room to spare, and a real scene is
/// nowhere near eight times overdraw -- the driving game measures 2.8 ms at
/// 1024x640 with 59,684 triangles.
///
/// The cap sits at 1920 rather than at whatever the largest display happens to
/// be, because the cost is per pixel and unbounded upward: memory is the other
/// half of it, and 1920x1080 is already 8.3 MB of colour plus 8.3 MB of depth.
/// Raising it further should come with the same measurement, not a guess.
const MAX_EDGE: u32 = 1_920;

/// Largest edge of an uploaded texture, in pixels.
///
/// Separate from the surface cap because the two limit different things: a
/// surface costs per-frame rasterization, a texture costs one upload and then
/// cache pressure while sampling. They shared a constant only because they
/// happened to want the same number, and raising the surface cap for 1080p is
/// no reason to let a guest upload a 1920x1920 image.
const MAX_TEXTURE_EDGE: u32 = 1_024;

/// A 3D point or direction. Deliberately plain: the guest sends flat floats,
/// and this is the only place they become anything else.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Vec3 {
    x: f32,
    y: f32,
    z: f32,
}

impl Vec3 {
    fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    /// Unit length, or zero when the vector has no direction to preserve.
    fn normalized(self) -> Self {
        let length = self.length();
        if length <= f32::EPSILON {
            Self::new(0.0, 0.0, 0.0)
        } else {
            Self::new(self.x / length, self.y / length, self.z / length)
        }
    }
}

/// One bound 3D scene: colour, depth, camera, and light.
pub struct Scene {
    width: u32,
    height: u32,
    /// `0xAARRGGBB`, the drawn painter's format, so presenting is a copy.
    colour: Vec<u32>,
    /// Distance from the camera per pixel. Larger is farther; a fresh frame is
    /// infinity so the first triangle always wins.
    depth: Vec<f32>,
    eye: Vec3,
    look_at: Vec3,
    fov_degrees: f32,
    light: Vec3,
    /// Whether triangles facing away from the camera are skipped.
    ///
    /// Off by default because it is only correct for closed meshes: a flat
    /// floor culled from underneath simply disappears, and an app author
    /// looking at a hole in the world has no reason to suspect a setting they
    /// never turned on.
    cull_back_faces: bool,
    /// Triangles projected this frame, filled together at `present`.
    queued: Vec<Queued>,
    /// Reusable RGBA scratch for `to_image`, so presenting does not allocate
    /// a megabyte every frame.
    rgba: Vec<u8>,
    /// Uploaded images, keyed by the handle the guest was given. Owned by the
    /// scene so they go away with it and cannot outlive the run.
    textures: Vec<Texture>,
}

/// One triangle waiting to be filled, already projected and shaded.
///
/// Queued rather than filled immediately so a whole frame's geometry can be
/// rasterized in parallel horizontal bands. Projection and lighting are cheap
/// and happen once on the calling thread; filling is the expensive part and is
/// what gets split.
struct Queued {
    /// Screen-space corners with depth: (x, y, z).
    a: (f32, f32, f32),
    b: (f32, f32, f32),
    c: (f32, f32, f32),
    area: f32,
    /// Flat colour, or `None` when this triangle is textured.
    packed: Option<u32>,
    /// How this triangle is textured, if it is.
    texture: Option<TexturedFace>,
    /// Coverage, 0..1. Below 1 the triangle blends over what is already there
    /// and does NOT write depth -- two panes of glass both have to be visible,
    /// and a pane that wrote depth would hide whatever came after it.
    alpha: f32,
    /// Depth at the centroid, used to sort the blended pass back to front.
    /// Sorting per triangle rather than per pixel is what a rasterizer without
    /// order-independent transparency has to do, and it is why two blended
    /// surfaces that interpenetrate still resolve wrongly.
    sort_z: f32,
}

/// The texturing half of a queued triangle.
#[derive(Clone, Copy)]
struct TexturedFace {
    /// Index into the scene's uploaded textures.
    index: usize,
    /// UV per corner, in the same order as the corners.
    uv: [[f32; 2]; 3],
    /// Tint multiplied into every sample.
    tint: (f32, f32, f32, f32),
    /// Lambertian shading for the whole face, used when `corner_shade` is
    /// `None`.
    shade: f32,
    /// Shading at each corner, interpolated across the triangle. `Some` only
    /// for a mesh drawn through `smooth`, which carries a normal per vertex.
    ///
    /// The shade is computed per CORNER at queue time rather than the normal
    /// being interpolated and the dot product taken per pixel: the light does
    /// not change across a triangle, so the two give the same answer to within
    /// the interpolation, and one costs three dot products per triangle while
    /// the other costs one per pixel.
    corner_shade: Option<[f32; 3]>,
}

/// One uploaded image, kept in the sampling format rather than the guest's.
struct Texture {
    width: u32,
    height: u32,
    /// `0xAARRGGBB`, so sampling is a lookup rather than four byte reads and a
    /// shift on every pixel of every triangle.
    pixels: Vec<u32>,
    /// Whether any texel is less than fully opaque, decided once at upload.
    ///
    /// A triangle wearing such a texture has to go in the BLENDED pass even
    /// when its tint is opaque -- it must not write depth, and it must be
    /// sorted back to front. Asking the texture rather than sampling it at
    /// queue time is the difference between one scan at upload and a lookup
    /// per triangle.
    has_transparency: bool,
}

impl Texture {
    /// Sample at `u,v`, wrapping outside 0..1 so a small image tiles.
    fn sample(&self, u: f32, v: f32) -> u32 {
        // `rem_euclid` rather than `%`: a negative coordinate must wrap to the
        // far edge, not mirror back across zero.
        let wrapped_u = if u.is_finite() {
            u.rem_euclid(1.0)
        } else {
            0.0
        };
        let wrapped_v = if v.is_finite() {
            v.rem_euclid(1.0)
        } else {
            0.0
        };
        let x = ((wrapped_u * self.width as f32) as u32).min(self.width.saturating_sub(1));
        let y = ((wrapped_v * self.height as f32) as u32).min(self.height.saturating_sub(1));
        let index = y as usize * self.width as usize + x as usize;
        self.pixels.get(index).copied().unwrap_or(0xFFFF_00FF)
    }
}

impl Scene {
    pub fn new(width: u32, height: u32) -> Result<Self, UiAdapterError> {
        if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
            return Err(UiAdapterError::Unsupported(format!(
                "a 3D surface must be between 1x1 and {MAX_EDGE}x{MAX_EDGE}, got {width}x{height}"
            )));
        }
        let pixels = width as usize * height as usize;
        Ok(Self {
            width,
            height,
            colour: vec![0xFF10_1420; pixels],
            depth: vec![f32::INFINITY; pixels],
            // A camera that is looking at something by default, so an app that
            // forgets to place one still draws rather than showing nothing and
            // leaving the author guessing which call was missed.
            eye: Vec3::new(0.0, 0.0, 4.0),
            look_at: Vec3::new(0.0, 0.0, 0.0),
            fov_degrees: 60.0,
            light: Vec3::new(-0.4, -0.7, -0.6).normalized(),
            cull_back_faces: false,
            queued: Vec::new(),
            rgba: Vec::new(),
            textures: Vec::new(),
        })
    }

    pub fn clear(&mut self, sky: u32) {
        // Anything queued and not yet filled belongs to the frame being
        // cleared, so it is dropped rather than drawn over the new sky.
        self.queued.clear();
        self.colour.fill(sky);
        self.depth.fill(f32::INFINITY);
    }

    pub fn set_camera(&mut self, eye: [f32; 3], look_at: [f32; 3], fov_degrees: f32) {
        self.eye = Vec3::new(eye[0], eye[1], eye[2]);
        self.look_at = Vec3::new(look_at[0], look_at[1], look_at[2]);
        // Degenerate fields of view produce a projection that divides by zero
        // or inverts the scene; clamped rather than refused, because an app
        // sweeping a zoom through a bad value should distort, not fail.
        self.fov_degrees = fov_degrees.clamp(5.0, 150.0);
    }

    pub fn set_cull_back_faces(&mut self, enabled: bool) {
        self.cull_back_faces = enabled;
    }

    /// Whether a projected triangle should be skipped as back-facing.
    ///
    /// The signed screen area already says which way the corners wind, so
    /// culling is a sign test rather than another dot product. Negative area
    /// is a back face here, matching counter-clockwise-when-seen-from-outside
    /// as the WIT documents.
    ///
    /// The sign flipped when the camera basis was corrected: `forward x up`
    /// mirrored every scene horizontally, which also reversed screen-space
    /// winding, so the old `area > 0.0` kept the documented behaviour only
    /// because two errors cancelled.
    fn culled(&self, area: f32) -> bool {
        self.cull_back_faces && area < 0.0
    }

    pub fn set_light(&mut self, direction: [f32; 3]) {
        let light = Vec3::new(direction[0], direction[1], direction[2]).normalized();
        // A zero-length direction would light nothing at all and read as a
        // rendering bug; keep the previous light instead.
        if light.length() > 0.0 {
            self.light = light;
        }
    }

    /// Project a world point to screen space plus a depth.
    ///
    /// Returns `None` when the point is behind the camera, where the
    /// perspective divide would fold it back into view upside down.
    fn project(&self, point: Vec3) -> Option<(f32, f32, f32)> {
        let forward = self.look_at.sub(self.eye).normalized();
        let world_up = Vec3::new(0.0, 1.0, 0.0);
        // `up × forward`, not `forward × up`. The other order points "right"
        // at -X, which mirrors the whole scene horizontally: a car at +X, to
        // the player's right, is drawn on the left. The picture still looks
        // plausible -- a road is roughly symmetric -- so it surfaced as
        // steering that went the wrong way, in a racing game where pressing
        // right moved the rider left.
        //
        // Only this axis was wrong. `up` comes out correct either way, which
        // is why nothing looked upside down and the bug hid in a scene that
        // renders fine.
        let right = world_up.cross(forward).normalized();
        let up = forward.cross(right);

        let relative = point.sub(self.eye);
        let camera_x = relative.dot(right);
        let camera_y = relative.dot(up);
        let camera_z = relative.dot(forward);

        if camera_z <= 0.01 {
            return None;
        }

        let half_fov = (self.fov_degrees.to_radians() / 2.0).tan();
        let aspect = self.width as f32 / self.height as f32;
        let ndc_x = camera_x / (camera_z * half_fov * aspect);
        // Screen Y grows downward; world Y grows up.
        let ndc_y = -camera_y / (camera_z * half_fov);

        Some((
            (ndc_x + 1.0) * 0.5 * self.width as f32,
            (ndc_y + 1.0) * 0.5 * self.height as f32,
            camera_z,
        ))
    }

    /// Fill one triangle, depth-tested and lit.
    ///
    /// Barycentric coverage over the triangle's bounding box: simple, exact at
    /// edges, and fast enough that the bound is pixels rather than cleverness.
    fn triangle(&mut self, a: Vec3, b: Vec3, c: Vec3, tint: (f32, f32, f32, f32)) {
        // Lambertian shading from the face normal, with ambient so a surface
        // facing away is dim rather than black -- a solid black facet reads as
        // a hole in the model.
        let normal = b.sub(a).cross(c.sub(a)).normalized();
        // Two-sided: the shade is how aligned the surface is with the light,
        // regardless of which way the triangle is wound. A one-sided term left
        // every face whose normal happened to point away from the light on
        // ambient alone -- and a spinning cube's front faces wind inward, so
        // the whole scene sat at 0.35 and read flat and dark. Absolute value
        // lights a surface the same whether its normal points at the light or
        // straight away, which is what a solid object wants.
        let facing = normal.dot(self.light).abs();
        let shade = 0.35 + 0.65 * facing;

        let (Some(pa), Some(pb), Some(pc)) = (self.project(a), self.project(b), self.project(c))
        else {
            return;
        };

        let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (pb.1 - pa.1);
        if area.abs() < 1e-6 || self.culled(area) {
            return;
        }

        let alpha = if tint.3.is_nan() {
            1.0
        } else {
            tint.3.clamp(0.0, 1.0)
        };
        self.queued.push(Queued {
            a: pa,
            b: pb,
            c: pc,
            area,
            packed: Some(pack_shaded(tint, shade)),
            texture: None,
            alpha,
            sort_z: (pa.2 + pb.2 + pc.2) / 3.0,
        });
    }

    /// Draw a flat list of `x,y,z` triples as triangles. Trailing floats that
    /// do not complete a triangle are ignored rather than refused, so an app
    /// streaming a mesh in chunks cannot break on a boundary.
    pub fn triangles(&mut self, vertices: &[f32], tint: (f32, f32, f32, f32)) {
        for chunk in vertices.chunks_exact(9) {
            let a = Vec3::new(chunk[0], chunk[1], chunk[2]);
            let b = Vec3::new(chunk[3], chunk[4], chunk[5]);
            let c = Vec3::new(chunk[6], chunk[7], chunk[8]);
            self.triangle(a, b, c, tint);
        }
    }

    /// Store an image and return the handle the guest will draw with.
    pub fn upload_texture(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<u64, UiAdapterError> {
        // The guest is untrusted: a buffer shorter than its stated size would
        // read past the end while sampling the last row.
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
        let mut pixels = Vec::with_capacity(width as usize * height as usize);
        for chunk in rgba.chunks_exact(4) {
            pixels.push(
                (u32::from(chunk[3]) << 24)
                    | (u32::from(chunk[0]) << 16)
                    | (u32::from(chunk[1]) << 8)
                    | u32::from(chunk[2]),
            );
        }
        let has_transparency = pixels.iter().any(|p| (p >> 24) != 0xFF);
        self.textures.push(Texture {
            width,
            height,
            pixels,
            has_transparency,
        });
        // Handles start at one so zero can stay an obviously invalid value.
        Ok(self.textures.len() as u64)
    }

    /// Draw one textured triangle, depth-tested and lit.
    ///
    /// UVs are interpolated in perspective -- divided by depth, interpolated,
    /// multiplied back -- rather than linearly across the screen. Linear
    /// interpolation is the classic mistake here: it looks correct on a wall
    /// facing the camera and visibly warps on a floor receding from it, which
    /// is the exact surface every 3D app puts under the player.
    #[allow(clippy::too_many_arguments)]
    fn textured_triangle(
        &mut self,
        a: Vec3,
        b: Vec3,
        c: Vec3,
        uv: [[f32; 2]; 3],
        texture: usize,
        tint: (f32, f32, f32, f32),
    ) {
        let normal = b.sub(a).cross(c.sub(a)).normalized();
        // Two-sided: the shade is how aligned the surface is with the light,
        // regardless of which way the triangle is wound. A one-sided term left
        // every face whose normal happened to point away from the light on
        // ambient alone -- and a spinning cube's front faces wind inward, so
        // the whole scene sat at 0.35 and read flat and dark. Absolute value
        // lights a surface the same whether its normal points at the light or
        // straight away, which is what a solid object wants.
        let facing = normal.dot(self.light).abs();
        let shade = 0.35 + 0.65 * facing;

        let (Some(pa), Some(pb), Some(pc)) = (self.project(a), self.project(b), self.project(c))
        else {
            return;
        };

        let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (pb.1 - pa.1);
        if area.abs() < 1e-6 || self.culled(area) {
            return;
        }

        let mut alpha = if tint.3.is_nan() {
            1.0
        } else {
            tint.3.clamp(0.0, 1.0)
        };
        // A texture with any transparent texel puts its triangle in the
        // blended pass even when the tint is opaque: it must not write depth,
        // and it has to be sorted back to front like any other blend. The
        // per-pixel coverage still comes from the texel, so a mostly-opaque
        // texture is not dimmed by being classified this way.
        if self
            .textures
            .get(texture)
            .is_some_and(|t| t.has_transparency)
        {
            alpha = alpha.min(0.999);
        }
        self.queued.push(Queued {
            a: pa,
            b: pb,
            c: pc,
            area,
            packed: None,
            texture: Some(TexturedFace {
                index: texture,
                uv,
                tint,
                shade,
                corner_shade: None,
            }),
            alpha,
            sort_z: (pa.2 + pb.2 + pc.2) / 3.0,
        });
    }

    /// Queue one triangle whose shading is given per corner.
    #[allow(clippy::too_many_arguments)]
    fn smooth_triangle(
        &mut self,
        a: Vec3,
        b: Vec3,
        c: Vec3,
        uv: [[f32; 2]; 3],
        corner_shade: [f32; 3],
        texture: usize,
        tint: (f32, f32, f32, f32),
    ) {
        let (Some(pa), Some(pb), Some(pc)) = (self.project(a), self.project(b), self.project(c))
        else {
            return;
        };
        let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (pb.1 - pa.1);
        if area.abs() < 1e-6 || self.culled(area) {
            return;
        }
        let mut alpha = if tint.3.is_nan() {
            1.0
        } else {
            tint.3.clamp(0.0, 1.0)
        };
        if self
            .textures
            .get(texture)
            .is_some_and(|t| t.has_transparency)
        {
            alpha = alpha.min(0.999);
        }
        self.queued.push(Queued {
            a: pa,
            b: pb,
            c: pc,
            area,
            packed: None,
            texture: Some(TexturedFace {
                index: texture,
                uv,
                tint,
                // Unused when corner shading is present, but a sane middle
                // value rather than something that would look wrong if the
                // fill ever fell back to it.
                shade: (corner_shade[0] + corner_shade[1] + corner_shade[2]) / 3.0,
                corner_shade: Some(corner_shade),
            }),
            alpha,
            sort_z: (pa.2 + pb.2 + pc.2) / 3.0,
        });
    }

    /// Draw a mesh wearing a texture. Triangles without matching UVs are
    /// skipped rather than drawn with whatever floats follow them.
    pub fn textured(
        &mut self,
        vertices: &[f32],
        uvs: &[f32],
        texture: u64,
        tint: (f32, f32, f32, f32),
    ) {
        let Some(index) = (texture as usize).checked_sub(1) else {
            return;
        };
        if index >= self.textures.len() {
            return;
        }
        for (triangle, uv_chunk) in vertices.chunks_exact(9).zip(uvs.chunks_exact(6)) {
            let a = Vec3::new(triangle[0], triangle[1], triangle[2]);
            let b = Vec3::new(triangle[3], triangle[4], triangle[5]);
            let c = Vec3::new(triangle[6], triangle[7], triangle[8]);
            let uv = [
                [uv_chunk[0], uv_chunk[1]],
                [uv_chunk[2], uv_chunk[3]],
                [uv_chunk[4], uv_chunk[5]],
            ];
            self.textured_triangle(a, b, c, uv, index, tint);
        }
    }

    /// Draw a mesh with a normal per vertex, so curvature shades smoothly.
    ///
    /// The shading is computed per CORNER here and interpolated across the
    /// triangle in the fill, which gives Gouraud shading: the classic way to
    /// make a low-poly sphere look round without a normal lookup per pixel.
    ///
    /// The light term is one-sided, unlike the face-normal path. A face-normal
    /// renderer has to take `.abs()` because a closed mesh's winding decides
    /// which way its normals point and half come out inward; an author who
    /// supplies normals has said which way is out, so the dark side can
    /// actually be dark. That is most of what makes a lit scene look lit.
    pub fn smooth(
        &mut self,
        vertices: &[f32],
        normals: &[f32],
        uvs: &[f32],
        texture: u64,
        tint: (f32, f32, f32, f32),
    ) {
        let Some(index) = (texture as usize).checked_sub(1) else {
            return;
        };
        if index >= self.textures.len() {
            return;
        }
        let triples = vertices
            .chunks_exact(9)
            .zip(normals.chunks_exact(9))
            .zip(uvs.chunks_exact(6));
        for ((triangle, normal_chunk), uv_chunk) in triples {
            let a = Vec3::new(triangle[0], triangle[1], triangle[2]);
            let b = Vec3::new(triangle[3], triangle[4], triangle[5]);
            let c = Vec3::new(triangle[6], triangle[7], triangle[8]);
            let uv = [
                [uv_chunk[0], uv_chunk[1]],
                [uv_chunk[2], uv_chunk[3]],
                [uv_chunk[4], uv_chunk[5]],
            ];
            let corner = |i: usize| -> f32 {
                let n = Vec3::new(
                    normal_chunk[i * 3],
                    normal_chunk[i * 3 + 1],
                    normal_chunk[i * 3 + 2],
                )
                .normalized();
                // One-sided: a surface facing away from the light gets the
                // ambient floor and nothing more.
                let facing = (-n.dot(self.light)).max(0.0);
                0.22 + 0.78 * facing
            };
            self.smooth_triangle(a, b, c, uv, [corner(0), corner(1), corner(2)], index, tint);
        }
    }

    /// Draw triangles moved by a transform, without touching the input.
    ///
    /// Rotation is applied around the model's own origin before translation,
    /// which is what an author means by "spin it and put it there". Doing it
    /// the other way round orbits the object around the world origin instead,
    /// and that difference is a whole evening of confusion for whoever hits it.
    pub fn place(
        &mut self,
        vertices: &[f32],
        translate: [f32; 3],
        rotate_degrees: [f32; 3],
        scale: f32,
        tint: (f32, f32, f32, f32),
    ) {
        let (sx, cx) = rotate_degrees[0].to_radians().sin_cos();
        let (sy, cy) = rotate_degrees[1].to_radians().sin_cos();
        let (sz, cz) = rotate_degrees[2].to_radians().sin_cos();

        let transform = |v: Vec3| -> Vec3 {
            let s = if scale.is_finite() && scale != 0.0 {
                scale
            } else {
                1.0
            };
            let (x, y, z) = (v.x * s, v.y * s, v.z * s);
            // X, then Y, then Z, matching the order the contract states.
            let (y, z) = (y * cx - z * sx, y * sx + z * cx);
            let (x, z) = (x * cy + z * sy, -x * sy + z * cy);
            let (x, y) = (x * cz - y * sz, x * sz + y * cz);
            Vec3::new(x + translate[0], y + translate[1], z + translate[2])
        };

        for chunk in vertices.chunks_exact(9) {
            let a = transform(Vec3::new(chunk[0], chunk[1], chunk[2]));
            let b = transform(Vec3::new(chunk[3], chunk[4], chunk[5]));
            let c = transform(Vec3::new(chunk[6], chunk[7], chunk[8]));
            self.triangle(a, b, c, tint);
        }
    }

    /// Fill every queued triangle, splitting the frame across cores.
    ///
    /// Each thread owns a horizontal band of rows and touches no other band's
    /// pixels, so no lock is needed anywhere in the inner loop -- the depth
    /// test stays exactly as it was in the single-threaded version. Every
    /// thread walks the whole triangle list and skips what does not reach its
    /// rows; that costs a bounds comparison per triangle per band, which is
    /// nothing beside a fill.
    ///
    /// Bands rather than tiles because rows are contiguous in memory: a band
    /// is one slice, and `chunks_mut` hands them out without any unsafe.
    fn flush(&mut self) {
        if self.queued.is_empty() {
            return;
        }

        // One band per core, but never so thin that the per-band overhead
        // dominates. A small window on a many-core machine is faster on two
        // threads than on ten.
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let min_rows = 16;
        let bands = cores.min((self.height as usize).div_ceil(min_rows)).max(1);
        let rows_per_band = (self.height as usize).div_ceil(bands);

        let width = self.width as usize;
        let mut queued = std::mem::take(&mut self.queued);
        let textures = &self.textures;

        // Opaque first, then blended back to front.
        //
        // A blended triangle cannot write depth -- two panes of glass both have
        // to be visible, and a pane that wrote depth would hide whatever was
        // drawn after it. But that means a blended triangle has no depth test
        // against its own kind, so the only thing deciding which of two blends
        // lands on top is the order they are drawn in. Sorting the blended pass
        // furthest-first is what makes that order correct.
        //
        // `sort_unstable_by` on a partition point rather than two vectors: the
        // allocation is reused between frames and splitting it would give that
        // up for nothing.
        queued.sort_unstable_by(|a, b| {
            let a_blend = a.alpha < 1.0;
            let b_blend = b.alpha < 1.0;
            match (a_blend, b_blend) {
                // Opaque before blended.
                (false, true) => core::cmp::Ordering::Less,
                (true, false) => core::cmp::Ordering::Greater,
                // Within the blended pass, far before near.
                (true, true) => b.sort_z.total_cmp(&a.sort_z),
                // Opaque order does not matter; the depth buffer decides.
                (false, false) => core::cmp::Ordering::Equal,
            }
        });

        // `scope` rather than spawn-and-join: the threads borrow the buffers
        // and the texture list directly, so nothing is cloned per frame.
        std::thread::scope(|scope| {
            let colour_bands = self.colour.chunks_mut(rows_per_band * width);
            let depth_bands = self.depth.chunks_mut(rows_per_band * width);
            for (band, (colour, depth)) in colour_bands.zip(depth_bands).enumerate() {
                let first_row = band * rows_per_band;
                let queued = &queued;
                scope.spawn(move || {
                    fill_band(colour, depth, width, first_row, queued, textures);
                });
            }
        });

        // Reuse the allocation next frame rather than growing it again.
        self.queued = queued;
        self.queued.clear();
    }

    /// Fill this frame's triangles and hand back the colour buffer.
    ///
    /// Named `render_image` rather than `to_image` because it is not a
    /// conversion: it rasterizes everything queued since the last clear.
    pub fn render_image(&mut self) -> Result<ImagePixels, UiAdapterError> {
        // Everything drawn this frame is filled here, once, across cores.
        self.flush();
        // Four `push` calls per pixel is four capacity checks per pixel, and
        // at 640x480 that was costing more than the 3D rendering itself --
        // measured at 224 frames a second rendering against 69 with the
        // conversion. Writing into a pre-sized buffer removes the checks; the
        // buffer is kept between frames so the allocation happens once rather
        // than a megabyte per frame.
        let needed = self.colour.len() * 4;
        if self.rgba.len() != needed {
            self.rgba = vec![0; needed];
        }
        for (word, out) in self.colour.iter().zip(self.rgba.chunks_exact_mut(4)) {
            out[0] = ((word >> 16) & 0xFF) as u8;
            out[1] = ((word >> 8) & 0xFF) as u8;
            out[2] = (word & 0xFF) as u8;
            out[3] = ((word >> 24) & 0xFF) as u8;
        }
        ImagePixels::new(self.width, self.height, self.rgba.clone())
    }
}

/// Which renderer a bound scene is actually using.
///
/// The GPU is asked for first and the software rasterizer is the fallback, not
/// the other way round -- but a machine with no usable adapter is a normal
/// machine, not a broken one, and it gets the CPU path with one line saying so
/// rather than a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Gpu,
    Cpu,
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Backend::Gpu => "GPU",
            Backend::Cpu => "CPU",
        }
    }
}

/// A bound 3D scene, on whichever backend this machine can run.
///
/// Every method forwards to one of the two implementations, which is why they
/// were written with the same names and the same behaviour: the host holds
/// this and never learns which is underneath. The parity test in
/// `tests/scene3d_parity.rs` is what makes that substitution honest -- three
/// of its five scenes agree to the exact byte and the other two to within one.
pub enum SceneBackend {
    Gpu(Box<krate_scene3d_gpu::GpuScene>),
    Cpu(Box<Scene>),
}

impl SceneBackend {
    /// Build a scene, preferring the GPU.
    ///
    /// `KRATE_SCENE3D=cpu` forces the software path. That exists because a
    /// backend you cannot turn off is a backend you cannot bisect: when a
    /// scene looks wrong, the first question is whether it looks wrong on
    /// both, and answering it should not need a rebuild.
    pub fn new(width: u32, height: u32) -> Result<Self, UiAdapterError> {
        let forced = std::env::var("KRATE_SCENE3D").unwrap_or_default();
        if !forced.eq_ignore_ascii_case("cpu") {
            if let Some(gpu) = krate_scene3d_gpu::GpuScene::new(width, height) {
                return Ok(SceneBackend::Gpu(Box::new(gpu)));
            }
        }
        Ok(SceneBackend::Cpu(Box::new(Scene::new(width, height)?)))
    }

    pub fn backend(&self) -> Backend {
        match self {
            SceneBackend::Gpu(_) => Backend::Gpu,
            SceneBackend::Cpu(_) => Backend::Cpu,
        }
    }

    pub fn clear(&mut self, sky: u32) {
        match self {
            // The CPU path takes a packed `0xAARRGGBB` word because that is
            // what its colour buffer holds; the GPU wants floats. Unpacking
            // here keeps the host's one call site unaware of either.
            SceneBackend::Gpu(gpu) => gpu.clear(unpack(sky)),
            SceneBackend::Cpu(cpu) => cpu.clear(sky),
        }
    }

    pub fn set_camera(&mut self, eye: [f32; 3], look_at: [f32; 3], fov_degrees: f32) {
        match self {
            SceneBackend::Gpu(gpu) => gpu.set_camera(eye, look_at, fov_degrees),
            SceneBackend::Cpu(cpu) => cpu.set_camera(eye, look_at, fov_degrees),
        }
    }

    pub fn set_cull_back_faces(&mut self, enabled: bool) {
        match self {
            SceneBackend::Gpu(gpu) => gpu.set_cull_back_faces(enabled),
            SceneBackend::Cpu(cpu) => cpu.set_cull_back_faces(enabled),
        }
    }

    pub fn set_light(&mut self, direction: [f32; 3]) {
        match self {
            SceneBackend::Gpu(gpu) => gpu.set_light(direction),
            SceneBackend::Cpu(cpu) => cpu.set_light(direction),
        }
    }

    pub fn upload_texture(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<u64, UiAdapterError> {
        match self {
            SceneBackend::Gpu(gpu) => gpu.upload_texture(width, height, rgba),
            SceneBackend::Cpu(cpu) => cpu.upload_texture(width, height, rgba),
        }
    }

    pub fn triangles(&mut self, vertices: &[f32], tint: (f32, f32, f32, f32)) {
        match self {
            SceneBackend::Gpu(gpu) => gpu.triangles(vertices, [tint.0, tint.1, tint.2, tint.3]),
            SceneBackend::Cpu(cpu) => cpu.triangles(vertices, tint),
        }
    }

    pub fn textured(
        &mut self,
        vertices: &[f32],
        uvs: &[f32],
        texture: u64,
        tint: (f32, f32, f32, f32),
    ) {
        match self {
            SceneBackend::Gpu(gpu) => {
                gpu.textured(vertices, uvs, texture, [tint.0, tint.1, tint.2, tint.3])
            }
            SceneBackend::Cpu(cpu) => cpu.textured(vertices, uvs, texture, tint),
        }
    }

    pub fn smooth(
        &mut self,
        vertices: &[f32],
        normals: &[f32],
        uvs: &[f32],
        texture: u64,
        tint: (f32, f32, f32, f32),
    ) {
        match self {
            SceneBackend::Gpu(gpu) => gpu.smooth(
                vertices,
                normals,
                uvs,
                texture,
                [tint.0, tint.1, tint.2, tint.3],
            ),
            SceneBackend::Cpu(cpu) => cpu.smooth(vertices, normals, uvs, texture, tint),
        }
    }

    pub fn place(
        &mut self,
        vertices: &[f32],
        translate: [f32; 3],
        rotate_degrees: [f32; 3],
        scale: f32,
        tint: (f32, f32, f32, f32),
    ) {
        match self {
            SceneBackend::Gpu(gpu) => gpu.place(
                vertices,
                translate,
                rotate_degrees,
                scale,
                [tint.0, tint.1, tint.2, tint.3],
            ),
            SceneBackend::Cpu(cpu) => cpu.place(vertices, translate, rotate_degrees, scale, tint),
        }
    }

    pub fn render_image(&mut self) -> Result<ImagePixels, UiAdapterError> {
        match self {
            SceneBackend::Gpu(gpu) => gpu.render_image(),
            SceneBackend::Cpu(cpu) => cpu.render_image(),
        }
    }
}

/// `0xAARRGGBB` to the four floats the GPU path takes.
fn unpack(word: u32) -> [f32; 4] {
    let channel = |shift: u32| ((word >> shift) & 0xFF) as f32 / 255.0;
    [channel(16), channel(8), channel(0), channel(24)]
}

/// Fill one horizontal band from the queued triangles.
///
/// `first_row` is the band's offset in the full frame, so screen coordinates
/// stay absolute and the projection does not have to know about bands.
fn fill_band(
    colour: &mut [u32],
    depth: &mut [f32],
    width: usize,
    first_row: usize,
    queued: &[Queued],
    textures: &[Texture],
) {
    let rows = colour.len() / width.max(1);
    let band_end = first_row + rows;

    for tri in queued {
        let (pa, pb, pc) = (tri.a, tri.b, tri.c);
        let min_x = pa.0.min(pb.0).min(pc.0).floor().max(0.0) as usize;
        let max_x = (pa.0.max(pb.0).max(pc.0).ceil().max(0.0) as usize).min(width);
        let tri_min_y = pa.1.min(pb.1).min(pc.1).floor().max(0.0) as usize;
        let tri_max_y = pa.1.max(pb.1).max(pc.1).ceil().max(0.0) as usize;

        // Clip the triangle to this band. A triangle that misses it entirely
        // costs only these comparisons.
        let start_y = tri_min_y.max(first_row);
        let end_y = tri_max_y.min(band_end);
        if start_y >= end_y || min_x >= max_x {
            continue;
        }

        let inv = [1.0 / pa.2, 1.0 / pb.2, 1.0 / pc.2];
        let uv_over_z = tri.texture.map(|face| {
            [
                [face.uv[0][0] * inv[0], face.uv[0][1] * inv[0]],
                [face.uv[1][0] * inv[1], face.uv[1][1] * inv[1]],
                [face.uv[2][0] * inv[2], face.uv[2][1] * inv[2]],
            ]
        });

        for y in start_y..end_y {
            let row = y - first_row;
            for x in min_x..max_x {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let w0 = ((pb.0 - pa.0) * (py - pa.1) - (px - pa.0) * (pb.1 - pa.1)) / tri.area;
                let w1 = ((px - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (py - pa.1)) / tri.area;
                let w2 = 1.0 - w0 - w1;
                // A pixel lying exactly on a triangle edge has a barycentric of
                // zero there, and the true value is unrepresentable: whether it
                // computes as +1e-8 or -1e-8 depends on whether the compiler
                // contracted the multiply-subtract above into a fused
                // multiply-add. arm64 and x86_64 make that choice differently.
                //
                // Measured on the edge function this rasterizer uses: of points
                // lying exactly on an edge, 16.5% land on opposite sides of
                // zero under the two evaluation orders. A cube's silhouette is
                // entirely shared edges, which is how the same cube drew 256
                // pixels on macOS and 255 on Linux CI.
                //
                // A tolerance of 1e-6 takes that 16.5% to zero, and 1e-7 does
                // not (109 of 20000 still disagree). It is a coverage decision
                // rather than a fudge: a millionth of a triangle's width is far
                // inside one pixel, so it can only decide pixels already
                // balanced on the boundary -- where either answer is right and
                // only agreeing across platforms matters.
                const EDGE: f32 = 1e-6;
                let inside = (w0 >= -EDGE && w1 >= -EDGE && w2 >= -EDGE)
                    || (w0 <= EDGE && w1 <= EDGE && w2 <= EDGE);
                if !inside {
                    continue;
                }

                let z = w2 * pa.2 + w1 * pb.2 + w0 * pc.2;
                let index = row * width + x;
                let Some(slot) = depth.get_mut(index) else {
                    continue;
                };
                if z >= *slot {
                    continue;
                }

                let value = match (tri.packed, tri.texture, uv_over_z) {
                    (Some(packed), _, _) => packed,
                    (None, Some(face), Some(uvz)) => {
                        let inv_z = w2 * inv[0] + w1 * inv[1] + w0 * inv[2];
                        if inv_z.abs() < 1e-9 {
                            continue;
                        }
                        let u = (w2 * uvz[0][0] + w1 * uvz[1][0] + w0 * uvz[2][0]) / inv_z;
                        let v = (w2 * uvz[0][1] + w1 * uvz[1][1] + w0 * uvz[2][1]) / inv_z;
                        let Some(image) = textures.get(face.index) else {
                            continue;
                        };
                        // Gouraud: interpolate the three corner shades in
                        // SCREEN space rather than perspective-correctly.
                        // Lighting varies slowly across a surface and the
                        // difference is invisible, while a perspective divide
                        // per pixel for it is not free.
                        let shade = match face.corner_shade {
                            Some(k) => w2 * k[0] + w1 * k[1] + w0 * k[2],
                            None => face.shade,
                        };
                        shade_sample(image.sample(u, v), face.tint, shade)
                    }
                    _ => continue,
                };

                // Coverage is the tint's alpha TIMES the texel's. The tint
                // fades a whole surface; the texel's own alpha is what makes a
                // soft shadow soft and a leaf texture leaf-shaped, and reading
                // only the tint threw that away -- the shadow would have been
                // a uniformly grey disc with a hard rim.
                //
                // Only a TEXTURED triangle has a texel. A flat-colour one gets
                // its packed word from `pack_shaded`, which puts the TINT's
                // alpha in the top byte -- so reading it back here multiplied
                // the tint's alpha by itself and a half-covering pane blended
                // at a quarter. Found by rendering the same scene through the
                // GPU backend and subtracting.
                let coverage = match tri.texture {
                    Some(_) => {
                        let texel_alpha = ((value >> 24) & 0xFF) as f32 / 255.0;
                        tri.alpha * texel_alpha
                    }
                    None => tri.alpha,
                };
                if let Some(pixel) = colour.get_mut(index) {
                    if coverage >= 1.0 {
                        *slot = z;
                        *pixel = value;
                    } else if coverage > 0.004 {
                        // Source-over, and NO depth write: the surface behind
                        // this one is still visible through it, so it must
                        // stay available for anything drawn later.
                        *pixel = blend(*pixel, value, coverage);
                    }
                }
            }
        }
    }
}

/// Shade a sampled texel and multiply it by the tint.
fn shade_sample(sampled: u32, tint: (f32, f32, f32, f32), shade: f32) -> u32 {
    let channel = |value: u32, scale: f32| -> u32 {
        let scale = if scale.is_nan() {
            1.0
        } else {
            scale.clamp(0.0, 1.0)
        };
        ((value as f32 * scale * shade).clamp(0.0, 255.0)) as u32
    };
    let alpha = (sampled >> 24) & 0xFF;
    (alpha << 24)
        | (channel((sampled >> 16) & 0xFF, tint.0) << 16)
        | (channel((sampled >> 8) & 0xFF, tint.1) << 8)
        | channel(sampled & 0xFF, tint.2)
}

/// Source-over blend of `src` onto `dst` at coverage `alpha`.
///
/// Integer arithmetic on the packed words rather than unpacking to floats:
/// this runs once per covered pixel of every transparent surface, which on a
/// window full of glass is the whole frame several times over.
#[inline]
fn blend(dst: u32, src: u32, alpha: f32) -> u32 {
    // 0..256 so the multiply-shift below is exact at both ends: 256 gives the
    // source unchanged, 0 gives the destination unchanged.
    let a = (alpha.clamp(0.0, 1.0) * 256.0) as u32;
    let inv = 256 - a;
    let mix = |shift: u32| -> u32 {
        let d = (dst >> shift) & 0xFF;
        let s = (src >> shift) & 0xFF;
        ((s * a + d * inv) >> 8) & 0xFF
    };
    // Alpha channel stays opaque: this is the frame buffer, not a layer.
    0xFF00_0000 | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// Pack a colour scaled by a shading factor into `0xAARRGGBB`.
fn pack_shaded(tint: (f32, f32, f32, f32), shade: f32) -> u32 {
    let channel = |value: f32| -> u32 {
        if value.is_nan() {
            0
        } else {
            ((value * shade).clamp(0.0, 1.0) * 255.0).round() as u32
        }
    };
    let alpha = if tint.3.is_nan() {
        255
    } else {
        (tint.3.clamp(0.0, 1.0) * 255.0).round() as u32
    };
    (alpha << 24) | (channel(tint.0) << 16) | (channel(tint.1) << 8) | channel(tint.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn something_on_the_right_is_drawn_on_the_right() {
        // The bug that made a racing game steer backwards.
        //
        // The camera basis used `forward x up`, which points "right" at -X and
        // mirrors the scene horizontally. A road is roughly symmetric, so the
        // picture still looked correct -- it surfaced as pressing right and
        // watching the rider move left. `up` came out correct either way,
        // which is why nothing appeared upside down and it hid for so long.
        let mut scene = Scene::new(200, 100).expect("scene");
        // Standing at the origin, looking down +Z, up is +Y.
        scene.set_camera([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 60.0);

        let middle = scene.project(Vec3::new(0.0, 0.0, 10.0)).expect("ahead");
        let right = scene
            .project(Vec3::new(5.0, 0.0, 10.0))
            .expect("to the right");
        let left = scene
            .project(Vec3::new(-5.0, 0.0, 10.0))
            .expect("to the left");

        assert!(
            right.0 > middle.0,
            "a point at +X is to the player's right and must draw right of centre \
             (got x={} against centre {})",
            right.0,
            middle.0
        );
        assert!(
            left.0 < middle.0,
            "a point at -X must draw left of centre (got x={} against centre {})",
            left.0,
            middle.0
        );

        // And up must still be up, which the broken basis also satisfied --
        // asserted so a future "fix" cannot trade one axis for the other.
        let above = scene.project(Vec3::new(0.0, 5.0, 10.0)).expect("above");
        assert!(
            above.1 < middle.1,
            "a point at +Y is above and screen y grows downward (got y={} against {})",
            above.1,
            middle.1
        );
    }

    #[test]
    fn hostile_guest_geometry_never_panics_or_hangs() {
        // A guest is untrusted code. It can send a NaN vertex, an infinite
        // camera, a zero field of view, a triangle list whose length is not a
        // multiple of nine, or ten thousand coincident points. None of that
        // may crash the host or spin it forever -- the whole point of the
        // sandbox is that a bad app is a bad app, not a broken runtime. This
        // throws each hostile shape at the full render path and asserts only
        // that it returns.
        let poison = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30, 0.0];
        let mut scene = Scene::new(64, 48).expect("scene");

        for &bad in &poison {
            scene.set_camera([bad, bad, bad], [bad, 0.0, 0.0], bad);
            scene.set_light([bad, bad, bad]);
            scene.clear(0xFF00_0000);
            // A triangle made entirely of the poison value.
            scene.triangles(&[bad; 9], (bad, bad, bad, 1.0));
            // A vertex list that is not a whole number of triangles.
            scene.triangles(&[bad, bad, bad, bad], (0.5, 0.5, 0.5, 1.0));
            // A placed mesh with poison transforms.
            scene.place(&[bad; 9], [bad; 3], [bad; 3], bad, (0.9, 0.2, 0.2, 1.0));
            // Rendering must return rather than trap; the pixels are whatever
            // they are, and this test does not care what -- only that we got
            // here.
            let _ = scene
                .render_image()
                .expect("render must not fail on bad input");
        }

        // A texture with a zero dimension and hostile UVs.
        let _ = scene.upload_texture(0, 0, &[]);
        if let Ok(texture) = scene.upload_texture(2, 2, &[255; 16]) {
            scene.clear(0xFF00_0000);
            scene.textured(
                &[f32::NAN; 9],
                &[f32::INFINITY; 6],
                texture,
                (1.0, 1.0, 1.0, 1.0),
            );
            let _ = scene.render_image().expect("textured render must not fail");
        }

        // An empty frame, and a frame of a single degenerate (zero-area)
        // triangle, are both fine.
        scene.clear(0xFF10_2030);
        let _ = scene.render_image().expect("empty frame");
        scene.triangles(
            &[1.0, 1.0, 5.0, 1.0, 1.0, 5.0, 1.0, 1.0, 5.0],
            (1.0, 1.0, 1.0, 1.0),
        );
        let _ = scene.render_image().expect("degenerate triangle");
    }

    /// A unit cube as twelve triangles, corners counter-clockwise from
    /// outside, which is the winding culling expects.
    fn unit_cube() -> Vec<f32> {
        let c = [
            [-0.5_f32, -0.5, -0.5],
            [0.5, -0.5, -0.5],
            [0.5, 0.5, -0.5],
            [-0.5, 0.5, -0.5],
            [-0.5, -0.5, 0.5],
            [0.5, -0.5, 0.5],
            [0.5, 0.5, 0.5],
            [-0.5, 0.5, 0.5],
        ];
        let faces = [
            [0usize, 1, 2],
            [0, 2, 3],
            [5, 4, 7],
            [5, 7, 6],
            [4, 0, 3],
            [4, 3, 7],
            [1, 5, 6],
            [1, 6, 2],
            [3, 2, 6],
            [3, 6, 7],
            [4, 5, 1],
            [4, 1, 0],
        ];
        let mut mesh = Vec::new();
        for face in faces {
            for corner in face {
                mesh.extend_from_slice(&c[corner]);
            }
        }
        mesh
    }

    /// A triangle filling most of the view, facing the default camera.
    fn facing_triangle() -> Vec<f32> {
        vec![
            -1.0, -1.0, 0.0, //
            1.0, -1.0, 0.0, //
            0.0, 1.0, 0.0,
        ]
    }

    fn pixel(image: &ImagePixels, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * image.width + x) * 4) as usize;
        [
            image.rgba[at],
            image.rgba[at + 1],
            image.rgba[at + 2],
            image.rgba[at + 3],
        ]
    }

    #[test]
    fn a_triangle_lands_in_the_middle_and_leaves_the_corners_alone() {
        let mut scene = Scene::new(64, 64).expect("scene");
        scene.clear(0xFF00_0000);
        scene.triangles(&facing_triangle(), (1.0, 0.0, 0.0, 1.0));
        let image = scene.render_image().expect("image");

        let middle = pixel(&image, 32, 34);
        assert!(middle[0] > 60, "the triangle should be drawn: {middle:?}");
        assert_eq!(
            pixel(&image, 1, 1),
            [0, 0, 0, 255],
            "a corner outside the triangle keeps the cleared sky"
        );
    }

    #[test]
    fn the_nearer_triangle_wins() {
        // This is the whole point of a depth buffer: draw order must not decide
        // what you see. The far triangle is drawn second and must not appear.
        let mut scene = Scene::new(64, 64).expect("scene");
        scene.clear(0xFF00_0000);

        let near: Vec<f32> = vec![-1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 1.0, 1.0];
        let far: Vec<f32> = vec![-1.0, -1.0, -1.0, 1.0, -1.0, -1.0, 0.0, 1.0, -1.0];
        scene.triangles(&near, (0.0, 1.0, 0.0, 1.0));
        scene.triangles(&far, (1.0, 0.0, 0.0, 1.0));

        let image = scene.render_image().expect("image");
        let middle = pixel(&image, 32, 34);
        assert!(
            middle[1] > middle[0],
            "the near green triangle must survive the far red one: {middle:?}"
        );
    }

    #[test]
    fn a_triangle_behind_the_camera_is_not_drawn() {
        // Without the near check, the perspective divide folds points behind
        // the eye back into view, mirrored -- geometry appearing where the
        // camera is not looking.
        let mut scene = Scene::new(32, 32).expect("scene");
        scene.clear(0xFF00_0000);
        let behind: Vec<f32> = vec![-1.0, -1.0, 9.0, 1.0, -1.0, 9.0, 0.0, 1.0, 9.0];
        scene.triangles(&behind, (1.0, 1.0, 1.0, 1.0));
        let image = scene.render_image().expect("image");
        assert!(
            image.rgba.chunks(4).all(|px| px == [0, 0, 0, 255]),
            "nothing behind the camera may be drawn"
        );
    }

    #[test]
    fn lighting_makes_a_facing_surface_brighter_than_an_angled_one() {
        let mut scene = Scene::new(48, 48).expect("scene");
        scene.set_light([0.0, 0.0, -1.0]);
        scene.clear(0xFF00_0000);
        scene.triangles(&facing_triangle(), (1.0, 1.0, 1.0, 1.0));
        let facing = pixel(&scene.render_image().expect("image"), 24, 26)[0];

        let mut angled = Scene::new(48, 48).expect("scene");
        angled.set_light([1.0, 0.0, 0.0]);
        angled.clear(0xFF00_0000);
        angled.triangles(&facing_triangle(), (1.0, 1.0, 1.0, 1.0));
        let sideways = pixel(&angled.render_image().expect("image"), 24, 26)[0];

        assert!(
            facing > sideways,
            "a surface facing the light must be brighter: {facing} vs {sideways}"
        );
        assert!(sideways > 0, "and an unlit surface is dim, not black");
    }

    #[test]
    fn both_winding_orders_draw() {
        // An app describing a mesh should not have to know which way the host
        // expects corners to run.
        let clockwise: Vec<f32> = vec![-1.0, -1.0, 0.0, 0.0, 1.0, 0.0, 1.0, -1.0, 0.0];
        let mut scene = Scene::new(48, 48).expect("scene");
        scene.clear(0xFF00_0000);
        scene.triangles(&clockwise, (0.0, 0.0, 1.0, 1.0));
        let image = scene.render_image().expect("image");
        assert!(
            pixel(&image, 24, 26)[2] > 60,
            "a clockwise triangle must draw too"
        );
    }

    #[test]
    fn a_partial_triangle_is_ignored_rather_than_read_past_its_end() {
        // The guest is untrusted: eight floats is not three corners.
        let mut scene = Scene::new(16, 16).expect("scene");
        scene.clear(0xFF00_0000);
        scene.triangles(&[0.0; 8], (1.0, 1.0, 1.0, 1.0));
        let image = scene.render_image().expect("image");
        assert!(image.rgba.chunks(4).all(|px| px == [0, 0, 0, 255]));
    }

    /// Ignored by default: a throughput measurement, not a correctness check.
    /// Run with `cargo test -p krate-runtime scene3d_throughput -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement, not a check"]
    fn scene3d_throughput() {
        // Twelve triangles is a cube; a real scene is hundreds. Measure the
        // pixel cost, which is what actually bounds a software renderer.
        let mut cube: Vec<f32> = Vec::new();
        let corners = [
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        let faces = [
            [0, 1, 2],
            [0, 2, 3],
            [5, 4, 7],
            [5, 7, 6],
            [4, 0, 3],
            [4, 3, 7],
            [1, 5, 6],
            [1, 6, 2],
            [3, 2, 6],
            [3, 6, 7],
            [4, 5, 1],
            [4, 1, 0],
        ];
        for f in faces {
            for i in f {
                cube.extend_from_slice(&corners[i]);
            }
        }

        for (w, h, cull) in [
            (320u32, 240u32, false),
            (640, 480, false),
            (640, 480, true),
            (800, 600, false),
        ] {
            let mut scene = Scene::new(w, h).expect("scene");
            scene.set_cull_back_faces(cull);
            scene.set_camera([2.5, 2.0, 3.5], [0.0, 0.0, 0.0], 60.0);
            let frames = 120;
            let start = std::time::Instant::now();
            for _ in 0..frames {
                scene.clear(0xFF10_1420);
                scene.triangles(&cube, (0.4, 0.7, 1.0, 1.0));
                let _ = scene.render_image();
            }
            let secs = start.elapsed().as_secs_f64();

            println!(
                "  {w}x{h}{}: {:.0} fps ({:.1} ms/frame)",
                if cull { " culled" } else { "" },
                frames as f64 / secs,
                secs * 1000.0 / frames as f64
            );
        }
    }

    #[test]
    fn a_placed_mesh_moves_without_the_caller_rebuilding_it() {
        // The whole point: one mesh, many positions, and the input untouched
        // so an app can keep a single copy.
        let mesh = facing_triangle();
        let mut scene = Scene::new(64, 64).expect("scene");
        scene.clear(0xFF00_0000);
        scene.place(
            &mesh,
            [2.5, 0.0, 0.0],
            [0.0, 0.0, 0.0],
            1.0,
            (1.0, 0.0, 0.0, 1.0),
        );
        let image = scene.render_image().expect("image");

        // Moved right, so the middle is empty and the right side is not.
        assert_eq!(
            pixel(&image, 32, 34),
            [0, 0, 0, 255],
            "the mesh should have moved out of the middle"
        );
        assert!(
            image.rgba.chunks(4).any(|px| px[0] > 60),
            "and it should still be visible somewhere"
        );
        assert_eq!(
            mesh,
            facing_triangle(),
            "the caller's mesh must be untouched"
        );
    }

    #[test]
    fn rotation_happens_around_the_model_not_the_world() {
        // Rotating around the world origin would swing a translated object
        // across the scene. Rotating a triangle sitting at the origin must
        // leave it at the origin.
        let mut scene = Scene::new(64, 64).expect("scene");
        scene.clear(0xFF00_0000);
        scene.place(
            &facing_triangle(),
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 180.0],
            1.0,
            (0.0, 1.0, 0.0, 1.0),
        );
        let image = scene.render_image().expect("image");
        // Flipped upside down, so ink is now above centre rather than below.
        assert!(
            pixel(&image, 32, 28)[1] > 60,
            "a spun triangle stays where it was put"
        );
    }

    #[test]
    fn a_zero_scale_does_not_collapse_the_model_into_nothing() {
        // A guest sweeping a scale through zero -- an object shrinking away --
        // would otherwise produce degenerate triangles every frame.
        let mut scene = Scene::new(32, 32).expect("scene");
        scene.clear(0xFF00_0000);
        scene.place(
            &facing_triangle(),
            [0.0; 3],
            [0.0; 3],
            0.0,
            (1.0, 1.0, 1.0, 1.0),
        );
        let image = scene.render_image().expect("image");
        assert!(
            image.rgba.chunks(4).any(|px| px[0] > 60),
            "a zero scale falls back to 1.0 rather than drawing nothing"
        );
    }

    /// A 2x2 texture: red, green / blue, white. Small enough that every
    /// sample is identifiable by colour alone.
    /// A plain white 1x1, so a shading test reads the shade and nothing else.
    /// `quad_texture` is four coloured quadrants, which is right for testing
    /// UV orientation and wrong for testing brightness.
    fn white_texture(scene: &mut Scene) -> u64 {
        scene
            .upload_texture(1, 1, &[255, 255, 255, 255])
            .expect("texture")
    }

    fn quad_texture(scene: &mut Scene) -> u64 {
        let rgba = vec![
            255, 0, 0, 255, //
            0, 255, 0, 255, //
            0, 0, 255, 255, //
            255, 255, 255, 255,
        ];
        scene.upload_texture(2, 2, &rgba).expect("texture")
    }

    #[test]
    fn a_texture_lands_on_the_triangle_the_right_way_up() {
        // v of 0 is the top of the image. Getting this inverted is the kind of
        // bug that only shows on an asymmetric texture, so the fixture is
        // deliberately asymmetric.
        let mut scene = Scene::new(64, 64).expect("scene");
        let texture = quad_texture(&mut scene);
        scene.set_light([0.0, 0.0, -1.0]);
        scene.clear(0xFF00_0000);

        // A quad facing the camera, covering most of the view.
        let quad: Vec<f32> = vec![
            -1.5, 1.5, 0.0, 1.5, 1.5, 0.0, -1.5, -1.5, 0.0, //
            1.5, 1.5, 0.0, 1.5, -1.5, 0.0, -1.5, -1.5, 0.0,
        ];
        let uvs: Vec<f32> = vec![
            0.0, 0.0, 1.0, 0.0, 0.0, 1.0, //
            1.0, 0.0, 1.0, 1.0, 0.0, 1.0,
        ];
        scene.textured(&quad, &uvs, texture, (1.0, 1.0, 1.0, 1.0));
        let image = scene.render_image().expect("image");

        // The camera sits at +Z looking back down -Z, so world -X is on the
        // viewer's right -- turn around and left and right swap. uv(0,0) is
        // at world -X, so the red corner is top-RIGHT on screen. This used to
        // read top-left, which passed only because the camera basis was
        // mirrored and every scene was drawn backwards.
        let top_right = pixel(&image, 44, 20);
        let bottom_left = pixel(&image, 20, 44);
        let bottom_right = bottom_left;
        assert!(
            top_right[0] > top_right[2],
            "the top-right of the image is red: {top_right:?}"
        );
        // White is equal parts red, green and blue. Compared as a ratio
        // rather than a brightness, because how lit the surface is depends on
        // the light direction and is not what this test is about.
        assert!(
            bottom_right[0] == bottom_right[1] && bottom_right[1] == bottom_right[2],
            "the bottom-right of the image is white: {bottom_right:?}"
        );
        assert!(bottom_right[0] > 0, "and it was actually drawn");
    }

    #[test]
    fn texture_coordinates_wrap_so_a_small_image_tiles() {
        let mut scene = Scene::new(48, 48).expect("scene");
        let texture = quad_texture(&mut scene);
        scene.set_light([0.0, 0.0, -1.0]);
        scene.clear(0xFF00_0000);

        // UVs from 0 to 4: the same image repeated across the surface.
        let quad: Vec<f32> = vec![-1.5, 1.5, 0.0, 1.5, 1.5, 0.0, -1.5, -1.5, 0.0];
        let uvs: Vec<f32> = vec![0.0, 0.0, 4.0, 0.0, 0.0, 4.0];
        scene.textured(&quad, &uvs, texture, (1.0, 1.0, 1.0, 1.0));

        let image = scene.render_image().expect("image");
        // Tiling means colours alternate across the surface rather than
        // stretching one texel over everything.
        let mut seen = std::collections::BTreeSet::new();
        for x in (8..40).step_by(3) {
            let px = pixel(&image, x, 20);
            if px != [0, 0, 0, 255] {
                seen.insert((px[0] / 64, px[1] / 64, px[2] / 64));
            }
        }
        assert!(
            seen.len() > 1,
            "a wrapped texture must repeat rather than smear: {seen:?}"
        );
    }

    #[test]
    fn a_negative_coordinate_wraps_rather_than_mirroring() {
        // `%` would fold -0.25 to -0.25 and then clamp; rem_euclid puts it at
        // 0.75, which is the far edge -- the difference between a floor that
        // tiles seamlessly and one with a visible mirror line.
        let mut scene = Scene::new(32, 32).expect("scene");
        let texture = quad_texture(&mut scene);
        let sample_at = |u: f32| scene.textures[(texture - 1) as usize].sample(u, 0.25);
        assert_eq!(
            sample_at(-0.25),
            sample_at(0.75),
            "a negative coordinate must land where the same point one tile over does"
        );
    }

    #[test]
    fn an_unknown_texture_draws_nothing_rather_than_guessing() {
        let mut scene = Scene::new(32, 32).expect("scene");
        scene.clear(0xFF00_0000);
        let quad: Vec<f32> = vec![-1.0, 1.0, 0.0, 1.0, 1.0, 0.0, -1.0, -1.0, 0.0];
        let uvs: Vec<f32> = vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        scene.textured(&quad, &uvs, 99, (1.0, 1.0, 1.0, 1.0));
        scene.textured(&quad, &uvs, 0, (1.0, 1.0, 1.0, 1.0));
        let image = scene.render_image().expect("image");
        assert!(image.rgba.chunks(4).all(|px| px == [0, 0, 0, 255]));
    }

    #[test]
    fn a_texture_whose_bytes_do_not_match_its_size_is_refused() {
        let mut scene = Scene::new(32, 32).expect("scene");
        assert!(scene.upload_texture(4, 4, &[0; 8]).is_err());
        assert!(scene.upload_texture(0, 4, &[]).is_err());
    }

    #[test]
    fn triangles_without_matching_uvs_are_skipped() {
        // Two triangles of geometry, one triangle of UVs: the second must not
        // be drawn with whatever floats happen to follow.
        let mut scene = Scene::new(32, 32).expect("scene");
        let texture = quad_texture(&mut scene);
        scene.clear(0xFF00_0000);
        let two: Vec<f32> = vec![
            -1.0, 1.0, 0.0, 1.0, 1.0, 0.0, -1.0, -1.0, 0.0, //
            1.0, 1.0, 0.0, 1.0, -1.0, 0.0, -1.0, -1.0, 0.0,
        ];
        let one_uv: Vec<f32> = vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        scene.textured(&two, &one_uv, texture, (1.0, 1.0, 1.0, 1.0));
        // The first triangle drew; the run did not panic or read past the end.
        let image = scene.render_image().expect("image");
        assert!(image.rgba.chunks(4).any(|px| px != [0, 0, 0, 255]));
    }

    #[test]
    fn a_receding_floor_keeps_its_texture_in_perspective() {
        // The claim worth testing. On a surface angled away from the camera,
        // linear UV interpolation makes texels bunch up wrongly -- the classic
        // warped-floor artefact. Perspective-correct interpolation puts the
        // halfway texel much nearer the far edge on screen, because distance
        // compresses it.
        let mut scene = Scene::new(64, 64).expect("scene");
        // A checker: left half black, right half white.
        let texture = scene
            .upload_texture(2, 1, &[0, 0, 0, 255, 255, 255, 255, 255])
            .expect("texture");
        scene.set_camera([0.0, 1.0, 3.0], [0.0, 0.0, -2.0], 60.0);
        scene.set_light([0.0, -1.0, 0.0]);
        scene.clear(0xFF00_2000);

        // A floor running away from the camera, textured along its length.
        let floor: Vec<f32> = vec![
            -2.0, 0.0, 2.0, 2.0, 0.0, 2.0, -2.0, 0.0, -8.0, //
            2.0, 0.0, 2.0, 2.0, 0.0, -8.0, -2.0, 0.0, -8.0,
        ];
        let uvs: Vec<f32> = vec![
            0.0, 0.0, 0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, 1.0, 0.0,
        ];
        scene.textured(&floor, &uvs, texture, (1.0, 1.0, 1.0, 1.0));
        let image = scene.render_image().expect("image");

        // Count how many screen rows each half of the texture occupies down
        // the middle of the floor. The far half (white, u=1) is compressed by
        // distance; the near half (black, u=0) is stretched toward the viewer.
        // Linear interpolation would split them roughly evenly instead.
        let mut far_rows = 0;
        let mut near_rows = 0;
        for y in 0..64 {
            let px = pixel(&image, 32, y);
            // Skip the cleared sky, which is the only green thing here.
            if px[1] > px[0] && px[1] > px[2] {
                continue;
            }
            if px[0] > 40 {
                far_rows += 1;
            } else {
                near_rows += 1;
            }
        }

        assert!(far_rows > 0, "the far half of the floor must be drawn");
        assert!(near_rows > 0, "and so must the near half");
        assert!(
            near_rows > far_rows * 3,
            "perspective must compress the far half: {far_rows} far rows vs {near_rows} near"
        );
    }

    /// The edge function, evaluated the two ways a compiler may choose.
    ///
    /// Not called by the renderer -- it exists so the test below can measure
    /// the disagreement that a platform difference produces, on this machine,
    /// without needing the other platform.
    fn edge_plain(a: (f32, f32), b: (f32, f32), p: (f32, f32)) -> f32 {
        (b.0 - a.0) * (p.1 - a.1) - (p.0 - a.0) * (b.1 - a.1)
    }

    fn edge_fused(a: (f32, f32), b: (f32, f32), p: (f32, f32)) -> f32 {
        (b.0 - a.0).mul_add(p.1 - a.1, -((p.0 - a.0) * (b.1 - a.1)))
    }

    #[test]
    fn the_edge_tolerance_is_wide_enough_for_the_worst_rounding_difference() {
        // Why this test exists: a cube drew 256 pixels on macOS and 255 on
        // Linux CI, and no test caught it because every test ran on one
        // machine. The cause is that a pixel exactly on a triangle edge has a
        // true barycentric of zero, and whether it computes as slightly
        // positive or slightly negative depends on whether the compiler fused
        // the multiply and subtract -- which arm64 and x86_64 decide
        // differently.
        //
        // Rather than needing both machines, this measures the two evaluation
        // orders here and asserts the tolerance covers the gap between them.
        let a = (3.7_f32, 11.3);
        let b = (41.9_f32, 27.15);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        // A representative triangle area, since the renderer divides by it
        // before comparing against the tolerance.
        let area = 700.0_f32;

        let mut without_tolerance = 0;
        let mut with_tolerance = 0;
        let samples = 20_000;
        for i in 1..samples {
            // A point exactly on the edge: the shared-edge case that a closed
            // mesh silhouette is entirely made of.
            let t = i as f32 / samples as f32;
            let p = (a.0 + dx * t, a.1 + dy * t);
            let plain = edge_plain(a, b, p) / area;
            let fused = edge_fused(a, b, p) / area;
            if (plain >= 0.0) != (fused >= 0.0) {
                without_tolerance += 1;
            }
            const EDGE: f32 = 1e-6;
            if (plain >= -EDGE) != (fused >= -EDGE) {
                with_tolerance += 1;
            }
        }

        assert!(
            without_tolerance > 0,
            "this machine shows no rounding difference at all, so the test \
             below proves nothing -- the sample points must be wrong"
        );
        assert_eq!(
            with_tolerance, 0,
            "the tolerance must absorb every rounding disagreement; \
             {without_tolerance} of {samples} on-edge points disagree without it"
        );
    }

    #[test]
    fn culling_keeps_the_near_face_and_drops_the_far_one() {
        // A closed cube: with culling on, the far side stops being filled and
        // the near side still covers it. The picture must not change.
        let mut without = Scene::new(64, 64).expect("scene");
        without.clear(0xFF00_0000);
        without.place(&unit_cube(), [0.0; 3], [0.0; 3], 1.0, (0.9, 0.5, 0.2, 1.0));
        let plain = without.render_image().expect("image");

        let mut with = Scene::new(64, 64).expect("scene");
        with.set_cull_back_faces(true);
        with.clear(0xFF00_0000);
        with.place(&unit_cube(), [0.0; 3], [0.0; 3], 1.0, (0.9, 0.5, 0.2, 1.0));
        let culled = with.render_image().expect("image");

        // The same silhouette either way: culling a closed mesh removes only
        // surfaces the near side already covers.
        let plain_drawn = plain.rgba.chunks(4).filter(|px| px[0] > 20).count();
        let culled_drawn = culled.rgba.chunks(4).filter(|px| px[0] > 20).count();
        assert_eq!(
            plain_drawn, culled_drawn,
            "culling must not change how much of the cube is visible"
        );

        // Both renders are solidly lit, not near-black. Lighting is two-sided
        // now, so a face is bright whichever way it is wound; this guards the
        // regression that had every cube sitting on ambient alone (max pixel
        // ~64) because front faces wind inward. A well-lit orange cube reaches
        // well past half brightness.
        let plain_bright = plain.rgba.chunks(4).filter(|px| px[0] > 120).count();
        assert!(
            plain_bright > 100,
            "the cube should be lit, not flat: only {plain_bright} bright pixels"
        );
        // Culling no longer changes brightness -- with two-sided shading the
        // back face that used to win a depth fight is as bright as the front,
        // so dropping it is invisible. That is the point: culling is a
        // performance choice, not a fix for a lighting artefact.
        let culled_bright = culled.rgba.chunks(4).filter(|px| px[0] > 120).count();
        assert_eq!(
            plain_bright, culled_bright,
            "two-sided lighting makes culling invisible: {plain_bright} vs {culled_bright}"
        );
    }

    #[test]
    fn culling_is_off_until_an_app_asks_for_it() {
        // A single flat triangle seen from behind is the case that breaks:
        // culling it makes a floor vanish when the camera dips below it. Off
        // by default means an app never loses geometry it did not opt in to
        // losing.
        let facing_away: Vec<f32> = vec![-1.0, -1.0, 0.0, 0.0, 1.0, 0.0, 1.0, -1.0, 0.0];

        let mut default_scene = Scene::new(48, 48).expect("scene");
        default_scene.clear(0xFF00_0000);
        default_scene.triangles(&facing_away, (0.0, 0.0, 1.0, 1.0));
        assert!(
            pixel(&default_scene.render_image().expect("image"), 24, 26)[2] > 60,
            "by default both winding orders draw"
        );

        let mut culled = Scene::new(48, 48).expect("scene");
        culled.set_cull_back_faces(true);
        culled.clear(0xFF00_0000);
        culled.triangles(&facing_away, (0.0, 0.0, 1.0, 1.0));
        assert_eq!(
            pixel(&culled.render_image().expect("image"), 24, 26),
            [0, 0, 0, 255],
            "with culling on, a back-facing triangle is skipped"
        );
    }

    #[test]
    fn a_half_transparent_surface_shows_what_is_behind_it() {
        // The change that unlocks glass, smoke, particles and soft shadows.
        // Before it the rasterizer wrote `*pixel = value` and alpha was
        // carried into the packed word and then ignored, so a 50% pane was
        // indistinguishable from a solid one.
        let mut scene = Scene::new(64, 64).expect("scene");
        scene.set_light([0.0, 0.0, -1.0]);
        scene.clear(0xFF00_0000);

        // The default camera sits at z = 4 looking toward the origin, so a
        // SMALLER z is further away. The red wall goes at z = 0 and the blue
        // pane at z = 2, between it and the eye.
        let back: Vec<f32> = vec![
            -3.0, -3.0, 0.0, 3.0, -3.0, 0.0, 3.0, 3.0, 0.0, //
            -3.0, -3.0, 0.0, 3.0, 3.0, 0.0, -3.0, 3.0, 0.0,
        ];
        let front: Vec<f32> = vec![
            -2.0, -2.0, 2.0, 2.0, -2.0, 2.0, 2.0, 2.0, 2.0, //
            -2.0, -2.0, 2.0, 2.0, 2.0, 2.0, -2.0, 2.0, 2.0,
        ];
        scene.triangles(&back, (1.0, 0.0, 0.0, 1.0));
        scene.triangles(&front, (0.0, 0.0, 1.0, 0.5));

        let image = scene.render_image().expect("image");
        let middle = pixel(&image, 32, 32);
        assert!(
            middle[0] > 30 && middle[2] > 30,
            "a half-transparent blue pane over a red wall must show BOTH: {middle:?}"
        );

        // And the pane must not have written depth: a solid green wall drawn
        // afterwards, between the two, still has to appear.
        let mut later = Scene::new(64, 64).expect("scene");
        later.set_light([0.0, 0.0, -1.0]);
        later.clear(0xFF00_0000);
        later.triangles(&back, (1.0, 0.0, 0.0, 1.0));
        later.triangles(&front, (0.0, 0.0, 1.0, 0.5));
        let between: Vec<f32> = vec![
            -1.0, -1.0, 1.0, 1.0, -1.0, 1.0, 1.0, 1.0, 1.0, //
            -1.0, -1.0, 1.0, 1.0, 1.0, 1.0, -1.0, 1.0, 1.0,
        ];
        later.triangles(&between, (0.0, 1.0, 0.0, 1.0));
        let image = later.render_image().expect("image");
        let centre = pixel(&image, 32, 32);
        // Green sits BEHIND the pane, so the blue is still on top -- what
        // matters is that the green is visible through it at all. Had the
        // pane written depth, the green would have been depth-rejected and
        // this channel would be zero.
        assert!(
            centre[1] > 20,
            "an opaque surface drawn behind a transparent one must still show \
             through it rather than being depth-rejected: {centre:?}"
        );
        assert!(
            centre[0] < centre[1],
            "and it must hide the red wall further back: {centre:?}"
        );
    }

    #[test]
    fn transparent_surfaces_draw_back_to_front() {
        // Two panes at different depths, queued NEAR first. Without sorting,
        // the near one blends into the sky and the far one then blends over
        // it, which puts the far pane on top -- visibly wrong and the classic
        // symptom of a renderer that forgot to sort its blended pass.
        let mut scene = Scene::new(64, 64).expect("scene");
        scene.set_light([0.0, 0.0, -1.0]);
        scene.clear(0xFF00_0000);

        // Nearer the eye at z = 4 means a LARGER z.
        let near: Vec<f32> = vec![
            -2.0, -2.0, 2.0, 2.0, -2.0, 2.0, 2.0, 2.0, 2.0, //
            -2.0, -2.0, 2.0, 2.0, 2.0, 2.0, -2.0, 2.0, 2.0,
        ];
        let far: Vec<f32> = vec![
            -2.0, -2.0, -1.0, 2.0, -2.0, -1.0, 2.0, 2.0, -1.0, //
            -2.0, -2.0, -1.0, 2.0, 2.0, -1.0, -2.0, 2.0, -1.0,
        ];
        // Near is RED and queued first; far is BLUE.
        scene.triangles(&near, (1.0, 0.0, 0.0, 0.5));
        scene.triangles(&far, (0.0, 0.0, 1.0, 0.5));

        let image = scene.render_image().expect("image");
        let middle = pixel(&image, 32, 32);
        // Correct order draws blue first, then red over it, so red dominates.
        assert!(
            middle[0] > middle[2],
            "the NEARER pane must end up on top whatever order it was queued \
             in: {middle:?}"
        );
    }

    #[test]
    fn corner_normals_shade_a_triangle_across_its_face() {
        // The difference between a faceted sphere and a smooth one. With a
        // face normal the whole triangle takes one shade; with corner normals
        // the shade varies across it, so adjacent triangles meet without a
        // visible step.
        let mut scene = Scene::new(64, 64).expect("scene");
        scene.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
        scene.set_light([0.0, -1.0, 0.0]);
        scene.clear(0xFF00_0000);
        let white = white_texture(&mut scene);

        // One big triangle facing the camera, with normals swinging from
        // straight up at the top corner to straight down at the bottom two.
        let tri: Vec<f32> = vec![0.0, 2.5, 0.0, -2.5, -2.0, 0.0, 2.5, -2.0, 0.0];
        let normals: Vec<f32> = vec![0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, -1.0, 0.0];
        let uvs: Vec<f32> = vec![0.5, 0.0, 0.0, 1.0, 1.0, 1.0];
        scene.smooth(&tri, &normals, &uvs, white, (1.0, 1.0, 1.0, 1.0));

        let image = scene.render_image().expect("image");
        // Sample near the top corner and near the bottom edge. The light
        // travels straight down, so the UPWARD-facing normal at the top is
        // lit and the downward ones below are not.
        let high = pixel(&image, 32, 20)[1] as i32;
        let low = pixel(&image, 32, 42)[1] as i32;
        assert!(
            (high - low).abs() > 25,
            "shading must vary across the triangle, not be flat: top {high}, \
             bottom {low}"
        );
    }

    #[test]
    fn corner_normals_light_one_side_only() {
        // The face-normal path takes the absolute value of the light term,
        // because a closed mesh's winding decides which way its normals point
        // and half come out inward. That lights the dark side of everything as
        // brightly as the lit side, which is why a face-shaded scene looks
        // flat. An author who supplies normals has said which way is out, so
        // the dark side can actually be dark.
        let mut scene = Scene::new(64, 64).expect("scene");
        scene.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
        // Light travelling toward -Z, i.e. away from the camera.
        scene.set_light([0.0, 0.0, -1.0]);
        scene.clear(0xFF00_0000);
        let white = white_texture(&mut scene);

        let quad: Vec<f32> = vec![
            -2.0, -2.0, 0.0, 2.0, -2.0, 0.0, 2.0, 2.0, 0.0, //
            -2.0, -2.0, 0.0, 2.0, 2.0, 0.0, -2.0, 2.0, 0.0,
        ];
        let uvs: Vec<f32> = vec![0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0];

        // Facing the light (+Z, back toward the camera): lit.
        let toward: Vec<f32> = core::iter::repeat_n([0.0_f32, 0.0, 1.0], 6)
            .flatten()
            .collect();
        scene.smooth(&quad, &toward, &uvs, white, (1.0, 1.0, 1.0, 1.0));
        let lit = pixel(&scene.render_image().expect("image"), 32, 32)[1] as i32;

        // Facing away (-Z): ambient only.
        let mut away_scene = Scene::new(64, 64).expect("scene");
        away_scene.set_camera([0.0, 0.0, 6.0], [0.0, 0.0, 0.0], 60.0);
        away_scene.set_light([0.0, 0.0, -1.0]);
        away_scene.clear(0xFF00_0000);
        let white2 = white_texture(&mut away_scene);
        let away: Vec<f32> = core::iter::repeat_n([0.0_f32, 0.0, -1.0], 6)
            .flatten()
            .collect();
        away_scene.smooth(&quad, &away, &uvs, white2, (1.0, 1.0, 1.0, 1.0));
        let dark = pixel(&away_scene.render_image().expect("image"), 32, 32)[1] as i32;

        assert!(
            lit > dark + 40,
            "a surface facing the light must be brighter than one facing away: \
             lit {lit}, away {dark}"
        );
    }

    #[test]
    fn coverage_comes_from_the_texel_as_well_as_the_tint() {
        // What makes a soft shadow soft and a leaf texture leaf-shaped: the
        // tint's alpha fades a whole surface uniformly, while the texel's own
        // alpha varies per pixel. Reading only the tint threw that away.
        //
        // Checked on the two pieces directly rather than through a rendered
        // scene: a scene test for this needs a texture whose texels differ, UVs
        // that land on specific texel CENTRES (u = 1.0 wraps back to texel 0),
        // and two correctly wound quads -- three chances to write a fixture
        // that fails for a reason that has nothing to do with the feature, and
        // I took all three before writing this.
        assert_eq!(blend(0xFF00_0000, 0xFFFF_FFFF, 1.0), 0xFFFF_FFFF);
        assert_eq!(blend(0xFF00_0000, 0xFFFF_FFFF, 0.0), 0xFF00_0000);
        let half = blend(0xFF00_0000, 0xFFFF_FFFF, 0.5);
        let channel = (half >> 16) & 0xFF;
        assert!(
            (120..=136).contains(&channel),
            "half coverage should land near the midpoint, got {channel}"
        );

        // And a texture reports whether it carries any transparency at all,
        // which is what puts its triangles in the blended pass even when the
        // tint is opaque.
        let mut scene = Scene::new(8, 8).expect("scene");
        let opaque = scene
            .upload_texture(1, 1, &[10, 20, 30, 255])
            .expect("texture");
        let partly = scene
            .upload_texture(2, 1, &[10, 20, 30, 255, 40, 50, 60, 128])
            .expect("texture");
        assert!(
            !scene.textures[opaque as usize - 1].has_transparency,
            "a fully opaque texture must not force the blended pass"
        );
        assert!(
            scene.textures[partly as usize - 1].has_transparency,
            "one transparent texel is enough to need blending"
        );
    }

    #[test]
    fn an_unreasonable_surface_size_is_refused() {
        assert!(Scene::new(0, 10).is_err());
        assert!(Scene::new(10, MAX_EDGE + 1).is_err());
    }

    #[test]
    fn a_full_hd_surface_is_allowed() {
        // The sizes people actually run windows at. 1024 refused every one of
        // these, which meant a 3D app could not fill a screen anyone owns --
        // the single thing standing between scene3d and a shippable game.
        for (w, h) in [(1280, 720), (1600, 900), (1920, 1080)] {
            assert!(
                Scene::new(w, h).is_ok(),
                "a {w}x{h} 3D surface must be allowed"
            );
        }
    }

    #[test]
    fn a_texture_keeps_its_own_smaller_limit() {
        // Surface and texture limits are separate things: raising the surface
        // cap so a window can be 1080p is no reason to accept a 1920-edge
        // texture, which is an upload and cache cost rather than a
        // rasterization one. They shared a constant only by coincidence.
        let mut scene = Scene::new(64, 64).expect("scene");
        let edge = MAX_TEXTURE_EDGE + 1;
        let rgba = vec![0_u8; edge as usize * 4];
        assert!(
            scene.upload_texture(edge, 1, &rgba).is_err(),
            "a texture wider than {MAX_TEXTURE_EDGE} must still be refused"
        );
        // The two limits being different constants is the point; a runtime
        // assertion comparing them can never fail and says nothing, so the
        // claim lives in the refusal above instead.
        const _: () = assert!(MAX_TEXTURE_EDGE < MAX_EDGE);
    }

    #[test]
    fn a_chase_camera_keeps_the_ground_below_the_horizon() {
        // The arrangement every third-person game uses, and the one that drew
        // krate-drift upside down: the eye is ABOVE the target and looks down
        // at it. A tilted camera exercises the `up` axis, which a camera on
        // the level leaves as exactly world up and so never tests.
        let mut scene = Scene::new(64, 64).expect("scene");
        scene.set_camera([0.0, 12.0, -26.0], [0.0, 2.0, 0.0], 58.0);
        scene.clear(0xFF00_0000);

        // Two markers at the same distance, one high in the world and one low.
        let high: Vec<f32> = vec![-3.0, 14.0, 0.0, 3.0, 14.0, 0.0, 0.0, 20.0, 0.0];
        let low: Vec<f32> = vec![-3.0, -8.0, 0.0, 3.0, -8.0, 0.0, 0.0, -14.0, 0.0];
        scene.triangles(&high, (1.0, 1.0, 1.0, 1.0));
        scene.triangles(&low, (1.0, 1.0, 1.0, 1.0));

        let image = scene.render_image().expect("image");
        let (mut top, mut bottom) = (0, 0);
        for y in 0..64 {
            for x in 0..64 {
                if pixel(&image, x, y)[0] > 40 {
                    if y < 32 {
                        top += 1;
                    } else {
                        bottom += 1;
                    }
                }
            }
        }
        assert!(top > 0 && bottom > 0, "both markers should be in frame");
        assert!(
            top > bottom,
            "the world-high marker must be drawn above the world-low one, \
             not below it: top={top} bottom={bottom}"
        );
    }

    #[test]
    fn ground_under_a_chase_camera_recedes_to_a_horizon_near_the_top() {
        // krate-drift's exact opening camera: 26 behind in -Z, 12 up, looking
        // at a point 2 above the ground.
        //
        // The check is the column directly ahead, NOT a count of lit pixels
        // across the frame. A ground plane running to the horizon puts most of
        // its area in the far, compressed band just under the horizon, so
        // "more lit pixels above the midline than below" is what a CORRECT
        // render of a receding plane looks like. Counting pixels reads that as
        // an upside-down world and accuses the projection of a bug it does not
        // have.
        let mut scene = Scene::new(64, 64).expect("scene");
        scene.set_camera([0.0, 12.0, -26.0], [0.0, 2.0, 0.0], 58.0);
        scene.set_light([0.0, -1.0, 0.0]);
        scene.clear(0xFF00_0000);

        // A ground quad at y = 0, entirely IN FRONT of the camera. A quad that
        // straddles the eye is rejected whole, because a triangle with any
        // corner behind the camera is dropped rather than clipped -- which
        // draws nothing and looks like an orientation bug of its own.
        let ground: Vec<f32> = vec![
            -80.0, 0.0, -24.0, 80.0, 0.0, -24.0, 80.0, 0.0, 400.0, //
            -80.0, 0.0, -24.0, 80.0, 0.0, 400.0, -80.0, 0.0, 400.0,
        ];
        scene.triangles(&ground, (1.0, 1.0, 1.0, 1.0));

        let image = scene.render_image().expect("image");
        // Walk the centre column from the top. Sky first, then ground all the
        // way down: the ground must be CONTIGUOUS to the bottom edge, which is
        // the thing an inverted camera gets wrong however the areas fall.
        let lit = |y: u32| pixel(&image, 32, y)[0] > 40;
        let horizon = (0..64).find(|&y| lit(y)).expect("ground must be in view");
        assert!(
            horizon > 4,
            "the horizon should sit below the top edge, not at it: {horizon}"
        );
        assert!(lit(63), "ground must reach the bottom edge of the frame");
        for y in horizon..64 {
            assert!(
                lit(y),
                "ground must be unbroken from the horizon down; row {y} is sky"
            );
        }
    }
}
