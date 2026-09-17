//! The circuit: a closed racing line, the road surface built around it, and
//! the queries a car needs to know where it is on the lap.
//!
//! The track is a centreline of points, not a heightmapped wilderness. That
//! choice is what makes the rest of the game possible: a lap has an order, so
//! there is a way to be ahead or behind; the road has edges, so leaving it can
//! cost something; and an opponent has a line to follow rather than a
//! destination to blunder toward.

use alloc::vec::Vec;

use crate::mathx::{cos_approx, sin_approx, sqrt_approx};

/// Half-width of the driveable road, in world units.
pub const ROAD_HALF: f32 = 11.0;
/// Where the grass stops being merely slow and the barrier begins.
pub const VERGE_HALF: f32 = 15.0;

/// One point on the racing line.
#[derive(Clone, Copy)]
pub struct Node {
    pub x: f32,
    pub z: f32,
    pub y: f32,
    /// Distance from the start line to this node, along the road.
    pub along: f32,
    /// Unit vector pointing to the next node.
    pub dir_x: f32,
    pub dir_z: f32,
}

pub struct Track {
    pub nodes: Vec<Node>,
    /// Total length of one lap.
    pub length: f32,
    /// Triangles for the road surface, built once.
    pub road: Vec<f32>,
    /// UV per road vertex. `v` runs along the lap so the asphalt scrolls under
    /// the car rather than repeating per segment, which is what gives a sense
    /// of travelling rather than of the same tile flashing past.
    pub road_uv: Vec<f32>,
    /// A normal per road vertex -- straight up, since the road is flat-ish --
    /// and the direction the texture's +u runs, which a normal map needs to
    /// know which way "across the road" points.
    pub road_normals: Vec<f32>,
    pub road_tangents: Vec<f32>,
    /// Triangles for the kerbs either side, and their UVs.
    pub kerb: Vec<f32>,
    pub kerb_uv: Vec<f32>,
}

/// Height of the land at a point, so the circuit rises and falls.
///
/// Deliberately gentle and smooth: a racing line over noise is unreadable and
/// launches the car at every crest. Two long sine waves give the track
/// elevation change you can see and feel without any of that.
pub fn ground_height(x: f32, z: f32) -> f32 {
    let a = sin_approx(x * 0.004) * 9.0;
    let b = cos_approx(z * 0.0055) * 7.0;
    a + b
}

/// Build the circuit as a closed loop.
///
/// The shape comes from summing a few harmonics onto a circle, which gives a
/// closed curve with straights, a hairpin and some sweepers without anyone
/// hand-placing a control point. The seed decides the layout.
pub fn build(seed: u32, points: usize) -> Track {
    let mut nodes: Vec<Node> = Vec::with_capacity(points);
    let base = 300.0_f32;

    // Harmonic amplitudes, small enough that the curve never self-intersects.
    let s = seed as f32 * 0.37;
    let a2 = 0.13 + (sin_approx(s) * 0.5 + 0.5) * 0.10;
    let a3 = 0.09 + (cos_approx(s * 1.7) * 0.5 + 0.5) * 0.08;
    let a5 = 0.05 + (sin_approx(s * 2.3) * 0.5 + 0.5) * 0.05;

    for i in 0..points {
        let t = i as f32 / points as f32 * core::f32::consts::PI * 2.0;
        let r = base * (1.0 + a2 * sin_approx(t * 2.0 + s) + a3 * cos_approx(t * 3.0 - s * 0.6)
            - a5 * sin_approx(t * 5.0 + s * 1.3));
        let x = sin_approx(t) * r;
        let z = cos_approx(t) * r;
        nodes.push(Node {
            x,
            z,
            y: ground_height(x, z),
            along: 0.0,
            dir_x: 0.0,
            dir_z: 0.0,
        });
    }

    // Directions and cumulative distance, wrapping at the end so the loop is
    // closed: the last node points at the first.
    let mut along = 0.0_f32;
    for i in 0..points {
        let n = nodes[(i + 1) % points];
        let c = nodes[i];
        let dx = n.x - c.x;
        let dz = n.z - c.z;
        let len = sqrt_approx(dx * dx + dz * dz).max(0.0001);
        nodes[i].dir_x = dx / len;
        nodes[i].dir_z = dz / len;
        nodes[i].along = along;
        along += len;
    }
    let length = along;

    let built = surface(&nodes);
    Track {
        nodes,
        length,
        road: built.0,
        road_uv: built.1,
        road_normals: built.4,
        road_tangents: built.5,
        kerb: built.2,
        kerb_uv: built.3,
    }
}

/// Build the road surface as a triangle strip around the centreline, plus a
/// kerb either side.
///
/// Wound so the UP face is the front face: the obvious corner order is
/// clockwise seen from above, which back-face culling drops, and the ground
/// then vanishes from under the car while distant geometry survives at its
/// grazing angle. That failure looks like a camera bug and is not one.
type Surfaces = (Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>, Vec<f32>);

fn surface(nodes: &[Node]) -> Surfaces {
    let n = nodes.len();
    let mut road = Vec::with_capacity(n * 18);
    let mut road_uv = Vec::with_capacity(n * 12);
    let mut road_normals = Vec::with_capacity(n * 18);
    let mut road_tangents = Vec::with_capacity(n * 18);
    let mut kerb = Vec::with_capacity(n * 36);
    let mut kerb_uv = Vec::with_capacity(n * 24);

    // Slightly above the land so the road is never z-fighting with the grass.
    let lift = 0.12_f32;
    let kerb_w = 1.6_f32;
    // How many times the tile repeats along one segment. The asphalt tile is
    // 64 units of texture over about 5 m of road, which is roughly the scale
    // of real chippings at this camera height.
    // Tighter than it looks like it should be. At 0.14 the 64-pixel tile
    // covered seven metres of road and the repeat read as a chequerboard at
    // speed -- the eye finds the period long before it finds the grain. At
    // 0.5 the tile is under two metres and reads as surface.
    // A tile every four metres, and `u` repeats to match (see the quad call
    // below), so a texel is SQUARE and about 1.5cm of road.
    //
    // Two numbers have to agree here and getting either alone wrong is
    // visible. `v_per_metre` was 0.5 -- a tile every two metres -- while `u`
    // spanned one tile across twenty-two metres of road: an eleven-to-one
    // stretch, which does not read as a stretched texture but as a BRICK
    // GRID, because the repeat is dense one way and sparse the other. Making
    // them agree at one tile per road-width fixed the grid and left texels
    // nine centimetres across, which at close range is a mosaic of fat
    // squares. Four metres is the scale at which 256 pixels of aggregate
    // actually looks like aggregate.
    const TILE_METRES: f32 = 4.0;
    let v_per_metre = 1.0 / TILE_METRES;

    for i in 0..n {
        let c = nodes[i];
        let d = nodes[(i + 1) % n];
        // Left normal in the XZ plane is (-dir_z, dir_x).
        let (cnx, cnz) = (-c.dir_z, c.dir_x);
        let (dnx, dnz) = (-d.dir_z, d.dir_x);

        // `v` runs with distance along the lap rather than resetting per
        // segment, so the surface scrolls under the car continuously. Reset
        // per segment and the same tile edge flashes past at every node, which
        // reads as strobing rather than as speed.
        let v0 = c.along * v_per_metre;
        let seg = {
            let dx = d.x - c.x;
            let dz = d.z - c.z;
            sqrt_approx(dx * dx + dz * dz)
        };
        let v1 = v0 + seg * v_per_metre;

        let quad = |w0: f32, w1: f32, u0: f32, u1: f32,
                        out: &mut Vec<f32>, out_uv: &mut Vec<f32>| {
            let al = [c.x + cnx * w0, c.y + lift, c.z + cnz * w0];
            let ar = [c.x + cnx * w1, c.y + lift, c.z + cnz * w1];
            let bl = [d.x + dnx * w0, d.y + lift, d.z + dnz * w0];
            let br = [d.x + dnx * w1, d.y + lift, d.z + dnz * w1];
            // Corner order matches the winding below, one UV per vertex.
            let uv = [
                [u0, v0], [u0, v1], [u1, v0],
                [u1, v0], [u0, v1], [u1, v1],
            ];
            for (p, t) in [al, bl, ar, ar, bl, br].iter().zip(uv.iter()) {
                out.push(p[0]);
                out.push(p[1]);
                out.push(p[2]);
                out_uv.push(t[0]);
                out_uv.push(t[1]);
            }
        };

        // `u` repeats at the SAME metres-per-tile as `v`, so a texel is square.
        let before = road.len();
        let u_span = (ROAD_HALF * 2.0) / TILE_METRES;
        quad(ROAD_HALF, -ROAD_HALF, 0.0, u_span, &mut road, &mut road_uv);
        // One normal and one tangent per vertex the quad just pushed. The
        // normal is straight up: the road banks only slightly and treating it
        // as flat costs nothing a driver can see. The tangent is the LEFT
        // normal of the centreline, because `u` runs across the road -- get
        // this wrong and the bumps light from ninety degrees off.
        for _ in 0..(road.len() - before) / 3 {
            road_normals.extend_from_slice(&[0.0, 1.0, 0.0]);
            road_tangents.extend_from_slice(&[cnx, 0.0, cnz]);
        }
        // Both kerbs are one mesh now: the stripe comes from the texture
        // rather than from alternating two differently tinted meshes, which
        // is what the untextured version had to do.
        quad(ROAD_HALF + kerb_w, ROAD_HALF, 0.0, 1.0, &mut kerb, &mut kerb_uv);
        quad(-ROAD_HALF, -ROAD_HALF - kerb_w, 1.0, 0.0, &mut kerb, &mut kerb_uv);
    }
    (road, road_uv, kerb, kerb_uv, road_normals, road_tangents)
}

/// Where a point sits relative to the track.
pub struct Where {
    /// Index of the nearest centreline node.
    pub node: usize,
    /// Signed distance from the centreline; positive is left of travel.
    pub offset: f32,
    /// Distance along the lap at that node.
    pub along: f32,
}

impl Track {
    /// Nearest point on the racing line, searched near a hint.
    ///
    /// The hint is the node the car was at last frame. Searching the whole
    /// centreline every frame for every car is the obvious version and it is
    /// also the one that turns into the frame budget once there are opponents;
    /// a car moves a few metres per frame, so a window around the last answer
    /// is both faster and more stable -- it cannot jump to the far side of a
    /// hairpin the way a global search can.
    pub fn locate(&self, x: f32, z: f32, hint: usize) -> Where {
        let n = self.nodes.len();
        let mut best = hint % n;
        let mut best_d2 = f32::INFINITY;
        let span = 24_usize;
        for k in 0..span * 2 {
            let i = (hint + n + k - span) % n;
            let node = self.nodes[i];
            let dx = x - node.x;
            let dz = z - node.z;
            let d2 = dx * dx + dz * dz;
            if d2 < best_d2 {
                best_d2 = d2;
                best = i;
            }
        }
        let node = self.nodes[best];
        // Left normal, so a positive offset is to the left of travel.
        let nx = -node.dir_z;
        let nz = node.dir_x;
        let offset = (x - node.x) * nx + (z - node.z) * nz;
        Where {
            node: best,
            offset,
            along: node.along,
        }
    }

    /// Height of the road surface under a point near the centreline.
    pub fn surface_height(&self, w: &Where) -> f32 {
        self.nodes[w.node].y
    }

    /// A point on the racing line, `ahead` units further round the lap.
    pub fn look_ahead(&self, node: usize, ahead: f32) -> (f32, f32) {
        let n = self.nodes.len();
        // Nodes are roughly evenly spaced, so stepping by count is close
        // enough for an opponent's aim point and costs nothing.
        let spacing = self.length / n as f32;
        let steps = (ahead / spacing.max(0.001)) as usize;
        let t = self.nodes[(node + steps) % n];
        (t.x, t.z)
    }
}
