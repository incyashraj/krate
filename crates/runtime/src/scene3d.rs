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
/// Software rendering costs pixels linearly, so this is a performance bound as
/// much as a memory one: at 1024x1024 a full-screen triangle is a million
/// depth tests, which is still inside frame budget but is the sensible ceiling
/// for a renderer with no GPU behind it.
const MAX_EDGE: u32 = 1_024;

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
        })
    }

    pub fn clear(&mut self, sky: u32) {
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
        let right = forward.cross(world_up).normalized();
        let up = right.cross(forward);

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
        let facing = (-normal.dot(self.light)).max(0.0);
        let shade = 0.35 + 0.65 * facing;

        let (Some(pa), Some(pb), Some(pc)) = (self.project(a), self.project(b), self.project(c))
        else {
            return;
        };

        let min_x = pa.0.min(pb.0).min(pc.0).floor().max(0.0) as u32;
        let max_x = (pa.0.max(pb.0).max(pc.0).ceil().min(self.width as f32) as u32).min(self.width);
        let min_y = pa.1.min(pb.1).min(pc.1).floor().max(0.0) as u32;
        let max_y =
            (pa.1.max(pb.1).max(pc.1).ceil().min(self.height as f32) as u32).min(self.height);

        let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (pb.1 - pa.1);
        if area.abs() < 1e-6 {
            return;
        }

        let packed = pack_shaded(tint, shade);

        for y in min_y..max_y {
            for x in min_x..max_x {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let w0 = ((pb.0 - pa.0) * (py - pa.1) - (px - pa.0) * (pb.1 - pa.1)) / area;
                let w1 = ((px - pa.0) * (pc.1 - pa.1) - (pc.0 - pa.0) * (py - pa.1)) / area;
                let w2 = 1.0 - w0 - w1;
                // Inside the triangle when every weight is non-negative. Both
                // winding orders are accepted: an app describing a mesh should
                // not have to know which way the host expects corners to run.
                let inside =
                    (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0);
                if !inside {
                    continue;
                }

                let depth = w2 * pa.2 + w1 * pb.2 + w0 * pc.2;
                let index = y as usize * self.width as usize + x as usize;
                let Some(slot) = self.depth.get_mut(index) else {
                    continue;
                };
                if depth >= *slot {
                    continue;
                }
                *slot = depth;
                if let Some(pixel) = self.colour.get_mut(index) {
                    *pixel = packed;
                }
            }
        }
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

    /// The colour buffer in the image pipeline's format.
    pub fn to_image(&self) -> Result<ImagePixels, UiAdapterError> {
        let mut rgba = Vec::with_capacity(self.colour.len() * 4);
        for word in &self.colour {
            rgba.push(((word >> 16) & 0xFF) as u8);
            rgba.push(((word >> 8) & 0xFF) as u8);
            rgba.push((word & 0xFF) as u8);
            rgba.push(((word >> 24) & 0xFF) as u8);
        }
        ImagePixels::new(self.width, self.height, rgba)
    }
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
        let image = scene.to_image().expect("image");

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

        let image = scene.to_image().expect("image");
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
        let image = scene.to_image().expect("image");
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
        let facing = pixel(&scene.to_image().expect("image"), 24, 26)[0];

        let mut angled = Scene::new(48, 48).expect("scene");
        angled.set_light([1.0, 0.0, 0.0]);
        angled.clear(0xFF00_0000);
        angled.triangles(&facing_triangle(), (1.0, 1.0, 1.0, 1.0));
        let sideways = pixel(&angled.to_image().expect("image"), 24, 26)[0];

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
        let image = scene.to_image().expect("image");
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
        let image = scene.to_image().expect("image");
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

        for (w, h) in [(320u32, 240u32), (640, 480), (800, 600)] {
            let mut scene = Scene::new(w, h).expect("scene");
            scene.set_camera([2.5, 2.0, 3.5], [0.0, 0.0, 0.0], 60.0);
            let frames = 120;
            let start = std::time::Instant::now();
            for _ in 0..frames {
                scene.clear(0xFF10_1420);
                scene.triangles(&cube, (0.4, 0.7, 1.0, 1.0));
                let _ = scene.to_image();
            }
            let secs = start.elapsed().as_secs_f64();
            println!(
                "  {w}x{h}: {:.0} fps ({:.1} ms/frame)",
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
        let image = scene.to_image().expect("image");

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
        let image = scene.to_image().expect("image");
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
        let image = scene.to_image().expect("image");
        assert!(
            image.rgba.chunks(4).any(|px| px[0] > 60),
            "a zero scale falls back to 1.0 rather than drawing nothing"
        );
    }

    #[test]
    fn an_unreasonable_surface_size_is_refused() {
        assert!(Scene::new(0, 10).is_err());
        assert!(Scene::new(10, MAX_EDGE + 1).is_err());
    }
}
