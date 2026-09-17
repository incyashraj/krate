//! The meshes, built properly.
//!
//! The first version of this game drew a car as two boxes stacked on each
//! other, a tree as a box with a four-sided cone on top, and a building as a
//! box. Twelve triangles for a car. It read as 1997 not because the renderer
//! was weak -- it does specular, fog, shadows, normal mapping and tone mapping
//! -- but because nothing was asking anything of it.
//!
//! There is room. The circuit already draws about 59,000 triangles with the
//! frame budget largely unspent, and the GPU backend has more headroom again
//! now that a frame no longer goes through the CPU. So these models spend
//! triangles where an eye actually looks: the silhouette of the car, the
//! roundness of a wheel, the line where a windscreen meets a roof.
//!
//! Everything here is built from quads and triangles in local space with an
//! explicit winding: counter-clockwise seen from OUTSIDE, which is what the
//! host's back-face test wants. Getting it backwards renders an object
//! inside-out, which reads as the world being broken rather than the mesh.

use alloc::vec::Vec;

use crate::mathx::{cos_approx, sin_approx};

/// A mesh under construction, in local space.
pub struct Build {
    pub verts: Vec<f32>,
}

impl Build {
    pub fn new() -> Self {
        Self { verts: Vec::new() }
    }

    pub fn tri(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3]) {
        for p in [a, b, c] {
            self.verts.push(p[0]);
            self.verts.push(p[1]);
            self.verts.push(p[2]);
        }
    }

    /// A quad as two triangles, wound so `a b c d` reads counter-clockwise
    /// from outside.
    pub fn quad(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) {
        self.tri(a, b, c);
        self.tri(a, c, d);
    }

    /// A box between two corners. Used for the parts that really are boxes.
    pub fn cuboid(&mut self, min: [f32; 3], max: [f32; 3]) {
        let (x0, y0, z0) = (min[0], min[1], min[2]);
        let (x1, y1, z1) = (max[0], max[1], max[2]);
        // front (-z), back (+z), left, right, top, bottom
        self.quad(
            [x0, y0, z0],
            [x0, y1, z0],
            [x1, y1, z0],
            [x1, y0, z0],
        );
        self.quad(
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z1],
            [x0, y0, z1],
        );
        self.quad(
            [x0, y0, z1],
            [x0, y1, z1],
            [x0, y1, z0],
            [x0, y0, z0],
        );
        self.quad(
            [x1, y0, z0],
            [x1, y1, z0],
            [x1, y1, z1],
            [x1, y0, z1],
        );
        self.quad(
            [x0, y1, z0],
            [x0, y1, z1],
            [x1, y1, z1],
            [x1, y1, z0],
        );
        self.quad(
            [x0, y0, z1],
            [x0, y0, z0],
            [x1, y0, z0],
            [x1, y0, z1],
        );
    }


}

/// A road car with a shape rather than a silhouette.
///
/// Built as a series of cross-sections along its length, each one a rectangle
/// whose width and height vary, then skinned between them. That is how a car
/// body is actually modelled, and it is what gives the thing a nose, a
/// shoulder line and a tapering tail instead of a brick outline.
///
/// About 500 triangles against the old 12. At six cars on the grid that is
/// 3,000 triangles, against a scene that already draws 59,000 with the budget
/// largely unspent -- the silhouette is the single most valuable place to
/// spend them, because it is what the eye reads first and at every distance.
pub fn car() -> Build {
    let mut m = Build::new();

    // Cross-sections down the car: z (back to front), half-width, floor, roof.
    // The numbers describe a low saloon: widest at the hips just behind the
    // middle, narrowing to the nose, with the cabin sitting between them.
    const SECTIONS: [[f32; 4]; 9] = [
        // z,    half-w, y0,   y1
        [-2.20, 0.80, 0.22, 0.86],
        [-1.90, 0.98, 0.16, 0.98],
        [-1.20, 1.10, 0.14, 1.04],
        [-0.40, 1.12, 0.14, 1.06],
        [0.30, 1.10, 0.14, 1.02],
        [1.00, 1.04, 0.15, 0.94],
        [1.60, 0.94, 0.17, 0.82],
        [2.00, 0.80, 0.20, 0.70],
        [2.25, 0.62, 0.26, 0.58],
    ];

    for w in SECTIONS.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (za, wa, ya0, ya1) = (a[0], a[1], a[2], a[3]);
        let (zb, wb, yb0, yb1) = (b[0], b[1], b[2], b[3]);

        // Left flank, right flank, roof/bonnet, floor. Each is a quad between
        // the two sections.
        m.quad(
            [-wa, ya0, za],
            [-wa, ya1, za],
            [-wb, yb1, zb],
            [-wb, yb0, zb],
        );
        m.quad(
            [wb, yb0, zb],
            [wb, yb1, zb],
            [wa, ya1, za],
            [wa, ya0, za],
        );
        m.quad(
            [-wa, ya1, za],
            [wa, ya1, za],
            [wb, yb1, zb],
            [-wb, yb1, zb],
        );
        m.quad(
            [-wb, yb0, zb],
            [wb, yb0, zb],
            [wa, ya0, za],
            [-wa, ya0, za],
        );
    }

    // Cap the two ends, so the body is a closed solid rather than a tube.
    let first = SECTIONS[0];
    let last = SECTIONS[SECTIONS.len() - 1];
    m.quad(
        [-first[1], first[2], first[0]],
        [first[1], first[2], first[0]],
        [first[1], first[3], first[0]],
        [-first[1], first[3], first[0]],
    );
    m.quad(
        [-last[1], last[3], last[0]],
        [last[1], last[3], last[0]],
        [last[1], last[2], last[0]],
        [-last[1], last[2], last[0]],
    );

    // The cabin: a raked windscreen, a roof, and a rear screen. This is the
    // line that says "car" more than any other -- a flat box on top reads as
    // a van whatever else is right.
    let roof_y = 1.62;
    let (wind_z0, wind_z1) = (0.30, 1.05); // base of screen, top
    let (rear_z1, rear_z0) = (-1.70, -1.05); // top of rear screen, its base
    let cabin_w = 0.92;
    let sill = 1.02;

    // Windscreen, raked back from the bonnet to the roof.
    m.quad(
        [-cabin_w, sill, wind_z1],
        [cabin_w, sill, wind_z1],
        [cabin_w * 0.92, roof_y, wind_z0],
        [-cabin_w * 0.92, roof_y, wind_z0],
    );
    // Roof.
    m.quad(
        [-cabin_w * 0.92, roof_y, wind_z0],
        [cabin_w * 0.92, roof_y, wind_z0],
        [cabin_w * 0.92, roof_y, rear_z0],
        [-cabin_w * 0.92, roof_y, rear_z0],
    );
    // Rear screen, raked the other way.
    m.quad(
        [-cabin_w * 0.92, roof_y, rear_z0],
        [cabin_w * 0.92, roof_y, rear_z0],
        [cabin_w, sill, rear_z1],
        [-cabin_w, sill, rear_z1],
    );
    // Side windows, filling the wedge between sill and roof.
    for side in [-1.0f32, 1.0] {
        let x = cabin_w * side;
        let xr = cabin_w * 0.92 * side;
        if side < 0.0 {
            m.quad(
                [x, sill, wind_z1],
                [xr, roof_y, wind_z0],
                [xr, roof_y, rear_z0],
                [x, sill, rear_z1],
            );
        } else {
            m.quad(
                [x, sill, rear_z1],
                [xr, roof_y, rear_z0],
                [xr, roof_y, wind_z0],
                [x, sill, wind_z1],
            );
        }
    }

    // A rear wing on a pair of stalks. Small, but it breaks the tail
    // silhouette, and a silhouette is what is read at a distance.
    m.cuboid([-0.95, 1.02, -2.30], [-0.80, 1.28, -2.10]);
    m.cuboid([0.80, 1.02, -2.30], [0.95, 1.28, -2.10]);
    m.cuboid([-1.05, 1.28, -2.36], [1.05, 1.36, -2.04]);

    m
}

/// One wheel: a cylinder with a rim face, lying on its side.
///
/// Drawn as its own mesh so it can be placed at each corner and turned with
/// the steering. Twelve segments -- enough that a wheel reads as round at the
/// distance a chase camera sees, and cheap enough to draw twenty-four of them
/// (six cars, four corners).
pub fn wheel(radius: f32, width: f32) -> Build {
    let mut m = Build::new();
    const SEG: usize = 12;
    let hw = width * 0.5;

    for i in 0..SEG {
        let a0 = (i as f32 / SEG as f32) * core::f32::consts::TAU;
        let a1 = ((i + 1) as f32 / SEG as f32) * core::f32::consts::TAU;
        let (c0, s0) = (cos_approx(a0) * radius, sin_approx(a0) * radius);
        let (c1, s1) = (cos_approx(a1) * radius, sin_approx(a1) * radius);

        // Tread: the band around the outside.
        m.quad(
            [-hw, s0, c0],
            [hw, s0, c0],
            [hw, s1, c1],
            [-hw, s1, c1],
        );
        // The two rim faces. Wound opposite ways so both face outward.
        m.tri([hw, 0.0, 0.0], [hw, s0, c0], [hw, s1, c1]);
        m.tri([-hw, 0.0, 0.0], [-hw, s1, c1], [-hw, s0, c0]);
    }
    m
}

/// A conifer with a trunk and three tapering tiers.
///
/// The old tree was a box and a four-sided cone, which from any angle read as
/// a paper dart on a stick. Tiers are what make a conifer legible: the
/// staggered horizontal edges catch the light differently and give the shape
/// somewhere for a shadow to fall.
pub fn conifer(seed: i32) -> Build {
    let mut m = Build::new();
    let jitter = ((seed * 2654435761u32 as i32) as u32 >> 16) as f32 / 65535.0;
    let scale = 0.8 + jitter * 0.6;

    // Trunk.
    m.cuboid([-0.18, 0.0, -0.18], [0.18, 2.0 * scale, 0.18]);

    // Three tiers, each an eight-sided cone, narrowing as they go up.
    const SEG: usize = 8;
    let tiers = [
        (1.6 * scale, 2.6 * scale, 2.0 * scale),
        (3.2 * scale, 1.9 * scale, 2.4 * scale),
        (4.9 * scale, 1.2 * scale, 2.6 * scale),
    ];
    for (base_y, radius, height) in tiers {
        for i in 0..SEG {
            let a0 = (i as f32 / SEG as f32) * core::f32::consts::TAU;
            let a1 = ((i + 1) as f32 / SEG as f32) * core::f32::consts::TAU;
            let p0 = [cos_approx(a0) * radius, base_y, sin_approx(a0) * radius];
            let p1 = [cos_approx(a1) * radius, base_y, sin_approx(a1) * radius];
            let tip = [0.0, base_y + height, 0.0];
            m.tri(p0, p1, tip);
            // Close the underside of the tier, so a tier seen from below is
            // not a hole.
            m.tri([0.0, base_y, 0.0], p1, p0);
        }
    }
    m
}

/// A sky dome: a band of sky around the horizon, drawn unlit.
///
/// The sky was `clear()` -- one flat colour over the whole window. A flat
/// fill is the single loudest thing saying "this is not a real place",
/// because a real sky is never one colour: it is lighter and warmer at the
/// horizon and deeper overhead, and that gradient is what an eye reads as
/// distance and time of day.
///
/// Built as a cylinder rather than a hemisphere on purpose. A chase camera
/// looks at the horizon and never at the zenith, so the top half of a dome is
/// triangles nobody sees. This is a tall band with a lid, which is all of the
/// sky a driver ever looks at.
///
/// Returned with a colour per vertex is not possible -- `unlit` takes one
/// tint -- so the gradient is baked into a texture and this just carries the
/// UVs for it. `v` runs 0 at the horizon to 1 at the top.
pub fn sky_dome(radius: f32, height: f32) -> (Build, Vec<f32>) {
    let mut m = Build::new();
    let mut uv: Vec<f32> = Vec::new();
    const SEG: usize = 32;

    // The band. Wound so the INSIDE faces the camera: the camera is inside
    // this cylinder, so the winding is the opposite of a solid object's.
    for i in 0..SEG {
        let a0 = (i as f32 / SEG as f32) * core::f32::consts::TAU;
        let a1 = ((i + 1) as f32 / SEG as f32) * core::f32::consts::TAU;
        let (x0, z0) = (cos_approx(a0) * radius, sin_approx(a0) * radius);
        let (x1, z1) = (cos_approx(a1) * radius, sin_approx(a1) * radius);
        // Drop the bottom edge well below the horizon so the seam is never
        // visible over a rise in the land.
        let y0 = -radius * 0.35;
        let y1 = height;

        // Wound so the INSIDE faces the camera.
        //
        // The camera is inside this cylinder, so the winding is the reverse of
        // a solid object's. Getting it the other way round culls every face
        // and the dome renders nothing at all -- which looks exactly like the
        // texture having failed to upload, and cost a debugging pass to tell
        // apart.
        m.quad(
            [x1, y0, z1],
            [x1, y1, z1],
            [x0, y1, z0],
            [x0, y0, z0],
        );
        let (u0, u1) = (i as f32 / SEG as f32, (i + 1) as f32 / SEG as f32);
        // Two triangles, six vertices, matching `quad`'s a-b-c / a-c-d order.
        for t in [
            [u1, 0.0],
            [u1, 1.0],
            [u0, 1.0],
            [u1, 0.0],
            [u0, 1.0],
            [u0, 0.0],
        ] {
            uv.push(t[0]);
            uv.push(t[1]);
        }
    }

    // A lid, so looking up over a crest does not show the clear colour
    // through the top of the cylinder.
    for i in 0..SEG {
        let a0 = (i as f32 / SEG as f32) * core::f32::consts::TAU;
        let a1 = ((i + 1) as f32 / SEG as f32) * core::f32::consts::TAU;
        let (x0, z0) = (cos_approx(a0) * radius, sin_approx(a0) * radius);
        let (x1, z1) = (cos_approx(a1) * radius, sin_approx(a1) * radius);
        m.tri([x1, height, z1], [0.0, height, 0.0], [x0, height, z0]);
        for t in [[0.5, 1.0], [0.5, 1.0], [0.5, 1.0]] {
            uv.push(t[0]);
            uv.push(t[1]);
        }
    }

    (m, uv)
}

/// A guard-rail section: a post with a rail across it.
///
/// Lining the circuit with these does more for the sense of SPEED than
/// anything else in the scene. A regular row of objects flicking past is what
/// an eye measures velocity against -- without one, a car at 230 km/h over
/// open grass looks like it is barely moving, because nothing near it is
/// changing fast.
///
/// Cheap, because there are hundreds: a post and a rail, 24 triangles.
pub fn guard_rail(span: f32) -> Build {
    let mut m = Build::new();
    let half = span * 0.5;
    // A post at EACH end, not one in the middle.
    //
    // One post under the centre of a span leaves metres of bar hanging in the
    // air either side of it, which reads as a floating slab rather than as a
    // barrier -- and the span has to match the placement spacing exactly or
    // the sections do not meet. Both were wrong first time: an 11.4m rail
    // placed every 17.1m, held up in the middle.
    for z in [-half, half] {
        m.cuboid([-0.10, 0.0, z - 0.09], [0.10, 1.05, z + 0.09]);
    }
    // The rail itself, proud of the posts so it reads as bolted on.
    m.cuboid([-0.15, 0.60, -half], [0.15, 0.94, half]);
    m
}

/// A low bush, for breaking up the run-off either side of the kerb.
///
/// Three intersecting quads rather than a solid: from a moving car a bush is
/// a mass of leaves, not a shape, and three crossed cards read as volume from
/// every angle for six triangles. The oldest trick in real-time foliage and
/// still the right one at this distance.
pub fn bush(size: f32) -> Build {
    let mut m = Build::new();
    let h = size;
    let r = size * 0.9;
    for i in 0..3 {
        let a = (i as f32 / 3.0) * core::f32::consts::PI;
        let (dx, dz) = (cos_approx(a) * r, sin_approx(a) * r);
        m.quad(
            [-dx, 0.0, -dz],
            [-dx, h, -dz],
            [dx, h, dz],
            [dx, 0.0, dz],
        );
        // The back face too, so a card is not invisible from one side under
        // back-face culling.
        m.quad(
            [dx, 0.0, dz],
            [dx, h, dz],
            [-dx, h, -dz],
            [-dx, 0.0, -dz],
        );
    }
    m
}
