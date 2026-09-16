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
    /// Triangles for the kerbs either side, alternating colour by segment.
    pub kerb_a: Vec<f32>,
    pub kerb_b: Vec<f32>,
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

    let (road, kerb_a, kerb_b) = surface(&nodes);
    Track {
        nodes,
        length,
        road,
        kerb_a,
        kerb_b,
    }
}

/// Build the road surface as a triangle strip around the centreline, plus a
/// kerb either side.
///
/// Wound so the UP face is the front face: the obvious corner order is
/// clockwise seen from above, which back-face culling drops, and the ground
/// then vanishes from under the car while distant geometry survives at its
/// grazing angle. That failure looks like a camera bug and is not one.
fn surface(nodes: &[Node]) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let n = nodes.len();
    let mut road = Vec::with_capacity(n * 18);
    let mut kerb_a = Vec::with_capacity(n * 18);
    let mut kerb_b = Vec::with_capacity(n * 18);

    // Slightly above the land so the road is never z-fighting with the grass.
    let lift = 0.12_f32;
    let kerb_w = 1.6_f32;

    for i in 0..n {
        let c = nodes[i];
        let d = nodes[(i + 1) % n];
        // Left normal in the XZ plane is (-dir_z, dir_x).
        let (cnx, cnz) = (-c.dir_z, c.dir_x);
        let (dnx, dnz) = (-d.dir_z, d.dir_x);

        let quad = |w0: f32, w1: f32, out: &mut Vec<f32>| {
            let al = [c.x + cnx * w0, c.y + lift, c.z + cnz * w0];
            let ar = [c.x + cnx * w1, c.y + lift, c.z + cnz * w1];
            let bl = [d.x + dnx * w0, d.y + lift, d.z + dnz * w0];
            let br = [d.x + dnx * w1, d.y + lift, d.z + dnz * w1];
            for p in [al, bl, ar, ar, bl, br] {
                out.push(p[0]);
                out.push(p[1]);
                out.push(p[2]);
            }
        };

        quad(ROAD_HALF, -ROAD_HALF, &mut road);
        // Kerbs alternate every few segments so corners read as striped.
        let striped = (i / 3) % 2 == 0;
        let out_a = if striped { &mut kerb_a } else { &mut kerb_b };
        quad(ROAD_HALF + kerb_w, ROAD_HALF, out_a);
        let out_b = if striped { &mut kerb_a } else { &mut kerb_b };
        quad(-ROAD_HALF, -ROAD_HALF - kerb_w, out_b);
    }
    (road, kerb_a, kerb_b)
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
