//! Orbit -- a live globe of an edge network.
//!
//! A rotating Earth drawn as a point cloud, a latitude/longitude graticule,
//! great-circle links from a hub to eleven cities with packets travelling
//! along them, and a node list down the right with latency and status.
//!
//! The globe is software-rendered: the shaded sphere and its atmosphere are
//! computed once at launch into a base buffer, and every frame copies that
//! base and plots the moving parts (graticule, land points, links, nodes) on
//! top, then hands the whole thing to the canvas with a single `draw-pixels`.
//! One host call for the picture instead of thousands -- K-227 measured a
//! canvas call at ~138us, so drawing the point cloud as primitives would cost
//! the whole frame budget many times over. Only text and the flat panel chrome
//! use canvas primitives, and there are fewer than a hundred of those.
//!
//! Time comes from the runtime's monotonic clock, so a headless shoot taken
//! after N milliseconds shows the globe exactly N milliseconds into its turn.

#![no_std]

extern crate alloc;
extern crate krate as _krate_runtime;

use alloc::vec;
use alloc::vec::Vec;

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// Design space, scaled by the host to whatever window opens.
const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;

const TITLEBAR_H: f32 = 48.0;
const PANEL_W: f32 = 372.0;
const STATUS_H: f32 = 34.0;

/// The globe buffer: square, drawn 1:1 into design space.
const GLOBE_PX: u32 = 640;
const GLOBE_AREA: f32 = GLOBE_PX as f32;
const HALF: f32 = GLOBE_AREA * 0.5;
/// Sphere radius in buffer pixels. Leaves room for the atmosphere glow inside
/// the buffer so its edges stay transparent and never show as a seam.
const RAD: f32 = HALF * 0.835;
/// Where the atmosphere ends, as a multiple of the sphere radius.
const ATMO: f32 = 1.11;

const GLOBE_CX: f32 = (WIDTH - PANEL_W) * 0.5;
const GLOBE_CY: f32 = TITLEBAR_H + (HEIGHT - TITLEBAR_H - STATUS_H) * 0.5 + 6.0;

/// Rotation: one full turn every 30 seconds.
const SPIN: f32 = 0.2094;
/// The axis leans toward the viewer so the north pole reads as a pole.
const TILT: f32 = 0.42;

/// Frames the `quick` run draws before reporting. The headless check has a
/// fuel budget, not a wall clock, so a few frames prove motion and no more.
const QUICK_FRAMES: u32 = 4;

// ---- palette ---------------------------------------------------------------
// The same ground as Query and Trace: near-black, one saturated colour.

const BG: gfx::Color = rgb(0.051, 0.059, 0.075);
const RAIL: gfx::Color = rgb(0.071, 0.082, 0.102);
const LINE: gfx::Color = rgb(0.137, 0.157, 0.192);
const LINE_SOFT: gfx::Color = rgb(0.102, 0.118, 0.149);
const ROW_TINT: gfx::Color = rgb(0.090, 0.102, 0.128);
const SEL_WASH: gfx::Color = rgb(0.110, 0.157, 0.267);

const INK: gfx::Color = rgb(0.906, 0.925, 0.957);
const INK_DIM: gfx::Color = rgb(0.573, 0.616, 0.686);
const INK_QUIET: gfx::Color = rgb(0.365, 0.404, 0.471);

/// Krate blue, #3b75ff.
const ACCENT: gfx::Color = rgb(0.231, 0.459, 1.0);
const ACCENT_INK: gfx::Color = rgb(0.478, 0.647, 1.0);
const GREEN: gfx::Color = rgb(0.310, 0.827, 0.549);
const AMBER: gfx::Color = rgb(0.976, 0.694, 0.267);

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

const fn rgba(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

/// Colours inside the pixel buffer, as linear-ish floats.
const BLUE: [f32; 3] = [0.231, 0.459, 1.0];
const BLUE_SOFT: [f32; 3] = [0.40, 0.58, 1.0];
const LAND_PX: [f32; 3] = [0.74, 0.82, 0.98];
const WHITE: [f32; 3] = [0.96, 0.98, 1.0];
const AMBER_PX: [f32; 3] = [0.976, 0.694, 0.267];

// ---- the network -----------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Health {
    Hub,
    Healthy,
    Degraded,
}

struct Node {
    name: &'static str,
    code: &'static str,
    region: &'static str,
    lat: f32,
    lon: f32,
    /// Round-trip from the hub, in milliseconds, before jitter.
    base_ms: u32,
    health: Health,
}

const NODES: [Node; 12] = [
    Node {
        name: "Frankfurt",
        code: "fra",
        region: "eu-central",
        lat: 50.1,
        lon: 8.7,
        base_ms: 0,
        health: Health::Hub,
    },
    Node {
        name: "London",
        code: "lhr",
        region: "eu-west",
        lat: 51.5,
        lon: -0.1,
        base_ms: 9,
        health: Health::Healthy,
    },
    Node {
        name: "Stockholm",
        code: "arn",
        region: "eu-north",
        lat: 59.3,
        lon: 18.1,
        base_ms: 24,
        health: Health::Healthy,
    },
    Node {
        name: "New York",
        code: "jfk",
        region: "us-east",
        lat: 40.7,
        lon: -74.0,
        base_ms: 78,
        health: Health::Healthy,
    },
    Node {
        name: "Toronto",
        code: "yyz",
        region: "ca-central",
        lat: 43.7,
        lon: -79.4,
        base_ms: 92,
        health: Health::Healthy,
    },
    Node {
        name: "San Francisco",
        code: "sfo",
        region: "us-west",
        lat: 37.8,
        lon: -122.4,
        base_ms: 142,
        health: Health::Healthy,
    },
    Node {
        name: "S\u{e3}o Paulo",
        code: "gru",
        region: "sa-east",
        lat: -23.5,
        lon: -46.6,
        base_ms: 196,
        health: Health::Degraded,
    },
    Node {
        name: "Johannesburg",
        code: "jnb",
        region: "af-south",
        lat: -26.2,
        lon: 28.0,
        base_ms: 171,
        health: Health::Healthy,
    },
    Node {
        name: "Mumbai",
        code: "bom",
        region: "ap-south",
        lat: 19.1,
        lon: 72.9,
        base_ms: 118,
        health: Health::Healthy,
    },
    Node {
        name: "Singapore",
        code: "sin",
        region: "ap-southeast",
        lat: 1.3,
        lon: 103.8,
        base_ms: 161,
        health: Health::Healthy,
    },
    Node {
        name: "Tokyo",
        code: "nrt",
        region: "ap-northeast",
        lat: 35.7,
        lon: 139.7,
        base_ms: 224,
        health: Health::Healthy,
    },
    Node {
        name: "Sydney",
        code: "syd",
        region: "ap-southeast-2",
        lat: -33.9,
        lon: 151.2,
        base_ms: 268,
        health: Health::Healthy,
    },
];

// ---- the continents ---------------------------------------------------------
// Simplified outlines as (lon, lat) in whole degrees. Coarse on purpose: they
// are rasterised to a point cloud a couple of degrees apart, where finer
// detail would never show. Antarctica is the band below 71 S.

type Ring = &'static [(i16, i16)];

const NORTH_AMERICA: Ring = &[
    (-165, 68),
    (-156, 71),
    (-140, 70),
    (-128, 70),
    (-110, 68),
    (-95, 68),
    (-82, 69),
    (-78, 62),
    (-64, 60),
    (-56, 52),
    (-65, 47),
    (-70, 42),
    (-74, 39),
    (-76, 35),
    (-81, 31),
    (-80, 25),
    (-83, 29),
    (-88, 30),
    (-94, 29),
    (-97, 26),
    (-97, 21),
    (-91, 19),
    (-87, 21),
    (-88, 16),
    (-83, 15),
    (-83, 10),
    (-79, 9),
    (-80, 7),
    (-85, 11),
    (-92, 14),
    (-98, 16),
    (-105, 20),
    (-106, 23),
    (-109, 23),
    (-114, 30),
    (-117, 32),
    (-120, 34),
    (-124, 40),
    (-124, 46),
    (-125, 49),
    (-128, 51),
    (-132, 54),
    (-137, 58),
    (-145, 60),
    (-152, 58),
    (-158, 56),
    (-163, 55),
    (-165, 60),
    (-166, 64),
];
const HUDSON_BAY: Ring = &[
    (-95, 60),
    (-85, 55),
    (-78, 52),
    (-78, 58),
    (-88, 64),
    (-94, 64),
];
const GREENLAND: Ring = &[
    (-55, 60),
    (-45, 60),
    (-40, 65),
    (-22, 70),
    (-18, 76),
    (-25, 82),
    (-45, 83),
    (-60, 80),
    (-70, 77),
    (-55, 70),
    (-52, 65),
];
const BAFFIN: Ring = &[(-80, 63), (-62, 66), (-70, 72), (-85, 70), (-90, 66)];
const CUBA: Ring = &[(-85, 22), (-75, 20), (-74, 20), (-84, 23)];
const HISPANIOLA: Ring = &[(-74, 18), (-68, 18), (-69, 20), (-73, 20)];
const SOUTH_AMERICA: Ring = &[
    (-78, 8),
    (-72, 12),
    (-62, 11),
    (-52, 5),
    (-50, 0),
    (-45, -2),
    (-35, -5),
    (-35, -9),
    (-39, -14),
    (-40, -22),
    (-48, -26),
    (-53, -33),
    (-58, -38),
    (-62, -40),
    (-65, -46),
    (-68, -52),
    (-66, -55),
    (-73, -53),
    (-75, -47),
    (-72, -40),
    (-72, -30),
    (-70, -20),
    (-76, -14),
    (-81, -5),
    (-80, 0),
];
const EURASIA: Ring = &[
    (-9, 43),
    (-9, 37),
    (-6, 36),
    (-2, 37),
    (0, 39),
    (3, 42),
    (4, 43),
    (8, 44),
    (11, 44),
    (12, 42),
    (16, 38),
    (17, 39),
    (18, 40),
    (16, 42),
    (14, 45),
    (19, 42),
    (20, 40),
    (22, 37),
    (24, 38),
    (26, 40),
    (28, 41),
    (27, 38),
    (28, 37),
    (30, 36),
    (36, 36),
    (35, 33),
    (34, 31),
    (34, 29),
    (35, 28),
    (37, 24),
    (39, 21),
    (43, 13),
    (45, 13),
    (52, 17),
    (57, 20),
    (59, 22),
    (57, 24),
    (56, 26),
    (52, 24),
    (50, 26),
    (48, 29),
    (50, 30),
    (53, 27),
    (57, 26),
    (61, 25),
    (66, 25),
    (70, 21),
    (73, 18),
    (75, 12),
    (77, 8),
    (80, 13),
    (82, 17),
    (87, 21),
    (90, 22),
    (92, 21),
    (94, 16),
    (97, 17),
    (98, 10),
    (100, 6),
    (103, 1),
    (104, 3),
    (102, 6),
    (100, 8),
    (100, 13),
    (103, 11),
    (107, 10),
    (109, 13),
    (108, 17),
    (106, 20),
    (110, 21),
    (113, 22),
    (117, 24),
    (120, 27),
    (122, 30),
    (121, 32),
    (119, 35),
    (122, 37),
    (118, 39),
    (121, 40),
    (125, 39),
    (126, 35),
    (129, 35),
    (129, 39),
    (131, 43),
    (135, 44),
    (140, 50),
    (141, 53),
    (137, 55),
    (139, 59),
    (148, 59),
    (155, 59),
    (156, 52),
    (163, 56),
    (160, 60),
    (163, 62),
    (175, 64),
    (180, 66),
    (180, 71),
    (170, 70),
    (160, 70),
    (150, 72),
    (140, 72),
    (130, 71),
    (122, 73),
    (112, 75),
    (104, 77),
    (95, 76),
    (88, 74),
    (80, 72),
    (72, 72),
    (68, 68),
    (60, 69),
    (55, 68),
    (45, 68),
    (41, 65),
    (37, 64),
    (35, 68),
    (30, 70),
    (24, 71),
    (18, 70),
    (13, 67),
    (7, 63),
    (5, 60),
    (5, 58),
    (8, 58),
    (10, 59),
    (12, 56),
    (15, 56),
    (16, 57),
    (18, 59),
    (18, 62),
    (21, 64),
    (24, 66),
    (22, 63),
    (21, 60),
    (25, 60),
    (30, 60),
    (24, 59),
    (22, 57),
    (21, 55),
    (19, 54),
    (14, 54),
    (11, 54),
    (10, 56),
    (10, 57),
    (8, 57),
    (8, 55),
    (9, 54),
    (7, 53),
    (5, 53),
    (4, 51),
    (2, 51),
    (0, 49),
    (-2, 49),
    (-5, 48),
    (-1, 46),
    (-2, 43),
    (-8, 43),
];
const BRITAIN: Ring = &[
    (-5, 50),
    (1, 51),
    (2, 53),
    (0, 54),
    (-2, 56),
    (-2, 58),
    (-5, 59),
    (-6, 57),
    (-5, 55),
    (-3, 54),
    (-5, 52),
];
const IRELAND: Ring = &[(-10, 52), (-6, 52), (-6, 55), (-8, 55), (-10, 54)];
const ICELAND: Ring = &[(-24, 64), (-14, 64), (-14, 67), (-22, 67)];
const JAPAN: Ring = &[
    (130, 31),
    (131, 34),
    (135, 34),
    (140, 35),
    (141, 38),
    (142, 41),
    (140, 42),
    (145, 44),
    (142, 45),
    (140, 41),
    (137, 37),
    (133, 35),
    (130, 33),
];
const SRI_LANKA: Ring = &[(80, 6), (82, 7), (81, 10), (80, 9)];
const PHILIPPINES: Ring = &[
    (120, 18),
    (122, 18),
    (126, 7),
    (125, 6),
    (122, 10),
    (120, 15),
];
const BORNEO: Ring = &[
    (109, 2),
    (110, -3),
    (114, -4),
    (117, -1),
    (119, 1),
    (118, 5),
    (115, 6),
    (110, 3),
];
const SUMATRA: Ring = &[
    (95, 5),
    (98, 4),
    (104, -2),
    (106, -6),
    (103, -5),
    (100, -1),
    (96, 3),
];
const JAVA: Ring = &[(105, -6), (114, -8), (113, -9), (106, -8)];
const NEW_GUINEA: Ring = &[
    (131, -1),
    (138, -2),
    (147, -6),
    (150, -10),
    (146, -8),
    (141, -9),
    (135, -4),
    (132, -3),
];
const AUSTRALIA: Ring = &[
    (114, -22),
    (114, -26),
    (115, -34),
    (118, -35),
    (124, -34),
    (130, -31),
    (134, -33),
    (137, -35),
    (140, -38),
    (144, -38),
    (147, -39),
    (150, -37),
    (153, -33),
    (153, -28),
    (153, -25),
    (150, -22),
    (146, -19),
    (145, -15),
    (143, -11),
    (142, -11),
    (141, -15),
    (137, -16),
    (136, -12),
    (131, -12),
    (129, -15),
    (125, -14),
    (122, -17),
    (119, -20),
];
const TASMANIA: Ring = &[(145, -41), (148, -41), (148, -43), (146, -43)];
const NZ_NORTH: Ring = &[
    (173, -35),
    (178, -38),
    (177, -40),
    (175, -41),
    (173, -39),
    (172, -35),
];
const NZ_SOUTH: Ring = &[
    (173, -41),
    (174, -42),
    (171, -45),
    (168, -47),
    (166, -46),
    (168, -44),
    (171, -42),
];
const AFRICA: Ring = &[
    (-6, 36),
    (-10, 30),
    (-15, 25),
    (-17, 21),
    (-17, 15),
    (-16, 12),
    (-13, 9),
    (-8, 5),
    (-3, 5),
    (2, 6),
    (6, 4),
    (9, 4),
    (9, 1),
    (12, -5),
    (13, -9),
    (12, -15),
    (12, -17),
    (15, -22),
    (15, -27),
    (17, -31),
    (18, -34),
    (20, -35),
    (26, -34),
    (30, -31),
    (33, -27),
    (35, -24),
    (35, -20),
    (40, -15),
    (40, -10),
    (39, -6),
    (41, -2),
    (43, 2),
    (48, 5),
    (51, 11),
    (48, 11),
    (43, 12),
    (41, 15),
    (39, 18),
    (37, 22),
    (34, 27),
    (33, 31),
    (30, 31),
    (25, 32),
    (20, 31),
    (15, 33),
    (11, 34),
    (10, 37),
    (3, 37),
    (0, 36),
    (-5, 36),
];
const MADAGASCAR: Ring = &[
    (44, -25),
    (48, -25),
    (50, -16),
    (49, -12),
    (47, -15),
    (44, -20),
];

/// Land rings, then the water rings that cut holes in them.
const LAND: &[Ring] = &[
    NORTH_AMERICA,
    GREENLAND,
    BAFFIN,
    CUBA,
    HISPANIOLA,
    SOUTH_AMERICA,
    EURASIA,
    BRITAIN,
    IRELAND,
    ICELAND,
    JAPAN,
    SRI_LANKA,
    PHILIPPINES,
    BORNEO,
    SUMATRA,
    JAVA,
    NEW_GUINEA,
    AUSTRALIA,
    TASMANIA,
    NZ_NORTH,
    NZ_SOUTH,
    AFRICA,
    MADAGASCAR,
];
const WATER: &[Ring] = &[HUDSON_BAY];

/// Angular spacing of the point cloud, in degrees.
const CLOUD_STEP: f32 = 1.45;

fn inside(ring: Ring, lon: f32, lat: f32) -> bool {
    // Even-odd ray cast, after a bounding-box reject so the whole test is
    // cheap for the thousands of grid points nowhere near this ring.
    let (mut min_lon, mut max_lon, mut min_lat, mut max_lat) =
        (999.0f32, -999.0f32, 999.0f32, -999.0f32);
    for &(x, y) in ring {
        let (x, y) = (x as f32, y as f32);
        min_lon = min_lon.min(x);
        max_lon = max_lon.max(x);
        min_lat = min_lat.min(y);
        max_lat = max_lat.max(y);
    }
    if lon < min_lon || lon > max_lon || lat < min_lat || lat > max_lat {
        return false;
    }
    let mut hit = false;
    let n = ring.len();
    let mut j = n.saturating_sub(1);
    for i in 0..n {
        let (Some(&(xi, yi)), Some(&(xj, yj))) = (ring.get(i), ring.get(j)) else {
            break;
        };
        let (xi, yi, xj, yj) = (xi as f32, yi as f32, xj as f32, yj as f32);
        if (yi > lat) != (yj > lat) {
            let cross = (xj - xi) * (lat - yi) / (yj - yi) + xi;
            if lon < cross {
                hit = !hit;
            }
        }
        j = i;
    }
    hit
}

fn is_land(lon: f32, lat: f32) -> bool {
    if lat < -71.0 {
        return true;
    }
    if WATER.iter().any(|ring| inside(ring, lon, lat)) {
        return false;
    }
    LAND.iter().any(|ring| inside(ring, lon, lat))
}

/// A unit vector for a place on the sphere. y is up through the poles, z is
/// toward the viewer at longitude 0.
fn unit(lat_deg: f32, lon_deg: f32) -> [f32; 3] {
    let lat = lat_deg * (core::f32::consts::PI / 180.0);
    let lon = lon_deg * (core::f32::consts::PI / 180.0);
    let c = libm::cosf(lat);
    [c * libm::sinf(lon), libm::sinf(lat), c * libm::cosf(lon)]
}

/// Build the point cloud once. Rows are spaced evenly in latitude and each
/// row is spaced by the same arc length, so the dots sit evenly on the sphere
/// instead of bunching at the poles.
fn build_cloud() -> Vec<[f32; 3]> {
    let mut cloud = Vec::new();
    let rows = (180.0 / CLOUD_STEP) as i32;
    for r in 0..rows {
        let lat = -90.0 + (r as f32 + 0.5) * CLOUD_STEP;
        let c = libm::cosf(lat * (core::f32::consts::PI / 180.0)).max(0.02);
        let count = ((360.0 * c) / CLOUD_STEP).max(1.0) as i32;
        let step = 360.0 / count as f32;
        let offset = if r % 2 == 0 { 0.0 } else { step * 0.5 };
        for k in 0..count {
            let lon = -180.0 + offset + k as f32 * step;
            if is_land(lon, lat) {
                cloud.push(unit(lat, lon));
            }
        }
    }
    cloud
}

// ---- the pixel buffer -------------------------------------------------------

struct Fb {
    px: Vec<u8>,
}

impl Fb {
    fn new() -> Fb {
        Fb {
            px: vec![0u8; (GLOBE_PX * GLOBE_PX * 4) as usize],
        }
    }

    /// Source-over one pixel. The buffer is straight RGBA.
    #[inline]
    fn blend(&mut self, x: i32, y: i32, c: [f32; 3], a: f32) {
        if a <= 0.002 || x < 0 || y < 0 || x >= GLOBE_PX as i32 || y >= GLOBE_PX as i32 {
            return;
        }
        let a = a.min(1.0);
        let i = ((y as u32 * GLOBE_PX + x as u32) * 4) as usize;
        let Some(p) = self.px.get_mut(i..i + 4) else {
            return;
        };
        let da = p[3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        if oa <= 0.0 {
            return;
        }
        let k = da * (1.0 - a) / oa;
        let m = a / oa;
        p[0] = ((c[0] * m + p[0] as f32 / 255.0 * k) * 255.0) as u8;
        p[1] = ((c[1] * m + p[1] as f32 / 255.0 * k) * 255.0) as u8;
        p[2] = ((c[2] * m + p[2] as f32 / 255.0 * k) * 255.0) as u8;
        p[3] = (oa * 255.0) as u8;
    }

    /// A sub-pixel dot: bilinear weights over the four pixels under (x, y),
    /// so a point moving a tenth of a pixel per frame slides instead of
    /// jumping.
    fn splat(&mut self, x: f32, y: f32, c: [f32; 3], a: f32) {
        let x0 = libm::floorf(x);
        let y0 = libm::floorf(y);
        let fx = x - x0;
        let fy = y - y0;
        let (xi, yi) = (x0 as i32, y0 as i32);
        self.blend(xi, yi, c, a * (1.0 - fx) * (1.0 - fy));
        self.blend(xi + 1, yi, c, a * fx * (1.0 - fy));
        self.blend(xi, yi + 1, c, a * (1.0 - fx) * fy);
        self.blend(xi + 1, yi + 1, c, a * fx * fy);
    }

    /// A thin anti-aliased line: splats one pixel apart along its length.
    fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: [f32; 3], a: f32) {
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len = libm::sqrtf(dx * dx + dy * dy);
        let n = libm::ceilf(len).max(1.0) as i32;
        for i in 0..n {
            let s = i as f32 / n as f32;
            self.splat(x0 + dx * s, y0 + dy * s, c, a);
        }
    }

    /// A soft-edged filled disc.
    fn disc(&mut self, cx: f32, cy: f32, r: f32, c: [f32; 3], a: f32) {
        let x0 = libm::floorf(cx - r - 1.0) as i32;
        let x1 = libm::ceilf(cx + r + 1.0) as i32;
        let y0 = libm::floorf(cy - r - 1.0) as i32;
        let y1 = libm::ceilf(cy + r + 1.0) as i32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let d = libm::sqrtf(dx * dx + dy * dy);
                let cov = (r + 0.5 - d).max(0.0).min(1.0);
                self.blend(x, y, c, a * cov);
            }
        }
    }

    /// A soft glow: alpha falls off with the square of distance.
    fn glow(&mut self, cx: f32, cy: f32, r: f32, c: [f32; 3], a: f32) {
        let x0 = libm::floorf(cx - r) as i32;
        let x1 = libm::ceilf(cx + r) as i32;
        let y0 = libm::floorf(cy - r) as i32;
        let y1 = libm::ceilf(cy + r) as i32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let d = libm::sqrtf(dx * dx + dy * dy) / r;
                if d >= 1.0 {
                    continue;
                }
                let f = 1.0 - d;
                self.blend(x, y, c, a * f * f);
            }
        }
    }

    /// A soft ring of the given stroke width.
    fn ring(&mut self, cx: f32, cy: f32, r: f32, w: f32, c: [f32; 3], a: f32) {
        let reach = r + w + 1.0;
        let x0 = libm::floorf(cx - reach) as i32;
        let x1 = libm::ceilf(cx + reach) as i32;
        let y0 = libm::floorf(cy - reach) as i32;
        let y1 = libm::ceilf(cy + reach) as i32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                let d = libm::sqrtf(dx * dx + dy * dy);
                let cov = (w * 0.5 + 0.5 - libm::fabsf(d - r)).max(0.0).min(1.0);
                self.blend(x, y, c, a * cov);
            }
        }
    }
}

/// The sphere itself: lit from the upper left, a blue rim where the surface
/// turns away, and a haze of atmosphere past the edge. Nothing here depends
/// on time, so it is computed once and copied every frame.
fn render_base(fb: &mut Fb) {
    let light = {
        let (x, y, z) = (-0.45f32, 0.50f32, 0.74f32);
        let n = libm::sqrtf(x * x + y * y + z * z);
        [x / n, y / n, z / n]
    };
    for py in 0..GLOBE_PX {
        for px in 0..GLOBE_PX {
            let x = (px as f32 + 0.5 - HALF) / RAD;
            let y = (HALF - py as f32 - 0.5) / RAD;
            let d2 = x * x + y * y;
            let d = libm::sqrtf(d2);
            let i = ((py * GLOBE_PX + px) * 4) as usize;
            let Some(p) = fb.px.get_mut(i..i + 4) else {
                continue;
            };

            // Atmosphere: blue haze that thins out with distance from the rim.
            let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
            if d < ATMO {
                let f = ((ATMO - d) / (ATMO - 1.0)).max(0.0).min(1.0);
                let f = f * f * f;
                r = BLUE[0] * 0.8;
                g = BLUE[1] * 0.85;
                b = BLUE[2];
                a = f * 0.42;
            }

            // The body, anti-aliased against the haze at its edge.
            let cov = ((1.0 - d) * RAD + 0.5).max(0.0).min(1.0);
            if cov > 0.0 {
                let nz = libm::sqrtf((1.0 - d2).max(0.0));
                let diff = (x * light[0] + y * light[1] + nz * light[2]).max(0.0);
                let spec = libm::powf(diff, 10.0) * 0.06;
                let rim = libm::powf(1.0 - nz, 4.0) * (0.55 + 0.45 * diff);
                let shade = 0.45 + 0.55 * diff;
                let mut sr = 0.045 * shade + 0.07 * diff + spec;
                let mut sg = 0.065 * shade + 0.12 * diff + spec;
                let mut sb = 0.120 * shade + 0.23 * diff + spec;
                sr += BLUE[0] * rim * 0.7;
                sg += BLUE[1] * rim * 0.7;
                sb += BLUE[2] * rim * 0.7;
                r = r * (1.0 - cov) + sr.min(1.0) * cov;
                g = g * (1.0 - cov) + sg.min(1.0) * cov;
                b = b * (1.0 - cov) + sb.min(1.0) * cov;
                a = a * (1.0 - cov) + cov;
            }

            p[0] = (r * 255.0) as u8;
            p[1] = (g * 255.0) as u8;
            p[2] = (b * 255.0) as u8;
            p[3] = (a * 255.0) as u8;
        }
    }
}

/// The view for one frame: spin about the pole, then lean the pole toward
/// the viewer.
struct View {
    st: f32,
    ct: f32,
    sa: f32,
    ca: f32,
}

impl View {
    fn at(t: f32) -> View {
        let theta = t * SPIN;
        View {
            st: libm::sinf(theta),
            ct: libm::cosf(theta),
            sa: libm::sinf(TILT),
            ca: libm::cosf(TILT),
        }
    }

    /// Rotate a unit vector into view space. Returns (screen x, screen y,
    /// depth toward the viewer) in buffer pixels.
    #[inline]
    fn project(&self, p: [f32; 3]) -> (f32, f32, f32) {
        let x = p[0] * self.ct + p[2] * self.st;
        let z = -p[0] * self.st + p[2] * self.ct;
        let y = p[1] * self.ca - z * self.sa;
        let z = p[1] * self.sa + z * self.ca;
        (HALF + x * RAD, HALF - y * RAD, z)
    }
}

/// One projected node, kept for the labels drawn after the buffer.
#[derive(Clone, Copy)]
struct Spot {
    x: f32,
    y: f32,
    z: f32,
}

/// The graticule: latitude rings and meridians, front hemisphere only,
/// fading as they turn away.
fn draw_graticule(fb: &mut Fb, view: &View) {
    let step = 3.0f32;
    let plot = |fb: &mut Fb, a: [f32; 3], b: [f32; 3]| {
        let (x0, y0, z0) = view.project(a);
        let (x1, y1, z1) = view.project(b);
        let z = (z0 + z1) * 0.5;
        if z <= 0.0 {
            return;
        }
        fb.line(x0, y0, x1, y1, BLUE_SOFT, 0.04 + 0.10 * libm::sqrtf(z));
    };
    for lat in [-60.0f32, -30.0, 0.0, 30.0, 60.0] {
        let mut lon = -180.0f32;
        while lon < 180.0 {
            plot(fb, unit(lat, lon), unit(lat, lon + step));
            lon += step;
        }
    }
    let mut lon = -180.0f32;
    while lon < 180.0 {
        let mut lat = -87.0f32;
        while lat < 87.0 {
            plot(fb, unit(lat, lon), unit(lat + step, lon));
            lat += step;
        }
        lon += 30.0;
    }
}

fn draw_cloud(fb: &mut Fb, view: &View, cloud: &[[f32; 3]]) {
    for &p in cloud {
        let (x, y, z) = view.project(p);
        if z <= 0.0 {
            continue;
        }
        // Brighter where the sphere faces us, dim at the limb, so the cloud
        // reads as a surface and not as a flat scatter.
        let a = 0.30 + 0.70 * z;
        fb.splat(x, y, LAND_PX, a);
    }
}

/// Great-circle links from the hub to every other node, lifted off the
/// surface, with one packet travelling each way on its own schedule.
fn draw_links(fb: &mut Fb, view: &View, t: f32, points: &[[f32; 3]; NODES.len()]) {
    let Some(&hub) = points.first() else { return };
    for (i, &p) in points.iter().enumerate().skip(1) {
        let dot = (hub[0] * p[0] + hub[1] * p[1] + hub[2] * p[2])
            .max(-1.0)
            .min(1.0);
        let omega = libm::acosf(dot);
        let so = libm::sinf(omega);
        if so < 0.001 {
            continue;
        }
        let steps = 40;
        // How high the arc bulges scales with how far apart the ends are.
        let lift = 0.04 + 0.16 * (omega / core::f32::consts::PI);
        let colour = if NODES.get(i).map(|n| n.health) == Some(Health::Degraded) {
            AMBER_PX
        } else {
            BLUE
        };
        let at = |s: f32| -> [f32; 3] {
            let wa = libm::sinf((1.0 - s) * omega) / so;
            let wb = libm::sinf(s * omega) / so;
            let h = 1.0 + lift * libm::sinf(s * core::f32::consts::PI);
            [
                (hub[0] * wa + p[0] * wb) * h,
                (hub[1] * wa + p[1] * wb) * h,
                (hub[2] * wa + p[2] * wb) * h,
            ]
        };
        let mut prev = view.project(at(0.0));
        for k in 1..=steps {
            let s = k as f32 / steps as f32;
            let cur = view.project(at(s));
            let z = (prev.2 + cur.2) * 0.5;
            if z > -0.05 {
                let a = 0.10 + 0.30 * (z + 0.05).max(0.0).min(1.0);
                fb.line(prev.0, prev.1, cur.0, cur.1, colour, a);
            }
            prev = cur;
        }
        // The packet: out from the hub, then back, on a period set by the
        // link's length so a long hop visibly takes longer.
        let period = 2.4 + omega * 1.2;
        let phase = (t / period + i as f32 * 0.173) % 1.0;
        let s = if phase < 0.5 {
            phase * 2.0
        } else {
            2.0 - phase * 2.0
        };
        let (x, y, z) = view.project(at(s));
        if z > 0.0 {
            fb.glow(x, y, 7.0, colour, 0.55 * z);
            fb.disc(x, y, 1.8, WHITE, 0.95 * z);
        }
    }
}

fn draw_nodes(
    fb: &mut Fb,
    view: &View,
    t: f32,
    points: &[[f32; 3]; NODES.len()],
    spots: &mut [Spot; NODES.len()],
) {
    for (i, (node, &p)) in NODES.iter().zip(points.iter()).enumerate() {
        let (x, y, z) = view.project(p);
        if let Some(spot) = spots.get_mut(i) {
            *spot = Spot { x, y, z };
        }
        if z <= 0.0 {
            continue;
        }
        let colour = match node.health {
            Health::Degraded => AMBER_PX,
            _ => BLUE,
        };
        let fade = (z * 1.6).min(1.0);
        // A slow pulse ring expanding out of each node, staggered so they
        // never fire together.
        let phase = (t * 0.55 + i as f32 * 0.29) % 1.0;
        let rr = 4.0 + phase * 16.0;
        fb.ring(
            x,
            y,
            rr,
            1.4,
            colour,
            (1.0 - phase) * (1.0 - phase) * 0.7 * fade,
        );
        fb.glow(x, y, 11.0, colour, 0.45 * fade);
        let core = if node.health == Health::Hub { 3.6 } else { 2.8 };
        fb.disc(x, y, core + 1.2, colour, 0.9 * fade);
        fb.disc(x, y, core - 0.8, WHITE, fade);
    }
}

// ---- canvas helpers ----------------------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) {
    let _ = canvas2d::fill_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        c,
    );
}

fn rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) {
    let _ = canvas2d::fill_round_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        gfx::CornerRadii {
            top_left: r,
            top_right: r,
            bottom_right: r,
            bottom_left: r,
        },
        c,
    );
}

fn dot(canvas: u64, x: f32, y: f32, r: f32, c: gfx::Color) {
    let _ = canvas2d::fill_circle(canvas, gfx::Point { x, y }, r, c);
}

fn style(weight: u16, spacing: f32) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: spacing,
        family: gfx::FontFamily::Sans,
    }
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, c);
}

fn text_w(
    canvas: u64,
    s: &str,
    x: f32,
    y: f32,
    size: f32,
    c: gfx::Color,
    weight: u16,
    spacing: f32,
) {
    let _ = canvas2d::draw_text_styled(
        canvas,
        s,
        gfx::Point { x, y },
        size,
        c,
        style(weight, spacing),
    );
}

fn measure(canvas: u64, s: &str, size: f32) -> f32 {
    canvas2d::measure_text(canvas, s, size)
        .map(|m| m.width)
        .unwrap_or(0.0)
}

/// Format an unsigned integer into `buf`, returning the used slice.
fn fmt_u32(buf: &mut [u8; 12], mut v: u32) -> &str {
    let mut tmp = [0u8; 12];
    let mut n = 0usize;
    loop {
        if let Some(slot) = tmp.get_mut(n) {
            *slot = b'0' + (v % 10) as u8;
            n += 1;
        }
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for i in 0..n {
        if let (Some(dst), Some(src)) = (buf.get_mut(i), tmp.get(n - 1 - i)) {
            *dst = *src;
        }
    }
    core::str::from_utf8(buf.get(..n).unwrap_or(b"0")).unwrap_or("0")
}

/// "224 ms" into `buf`.
fn fmt_ms(buf: &mut [u8; 16], v: u32) -> &str {
    let mut digits = [0u8; 12];
    let d = fmt_u32(&mut digits, v);
    let n = d.len();
    for (i, b) in d.bytes().enumerate() {
        if let Some(slot) = buf.get_mut(i) {
            *slot = b;
        }
    }
    for (i, b) in b" ms".iter().enumerate() {
        if let Some(slot) = buf.get_mut(n + i) {
            *slot = *b;
        }
    }
    core::str::from_utf8(buf.get(..n + 3).unwrap_or(b"0 ms")).unwrap_or("0 ms")
}

/// Two digits, zero padded, at `buf[at..at+2]`.
fn two(buf: &mut [u8; 12], at: usize, v: u64) {
    if let Some(slot) = buf.get_mut(at) {
        *slot = b'0' + ((v / 10) % 10) as u8;
    }
    if let Some(slot) = buf.get_mut(at + 1) {
        *slot = b'0' + (v % 10) as u8;
    }
}

/// A stable 0..1 from an integer, for latency jitter that ticks rather than
/// flickers: the same second always gets the same number.
fn hash01(n: u32) -> f32 {
    let mut x = n.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    x = ((x >> ((x >> 28).wrapping_add(4))) ^ x).wrapping_mul(277_803_737);
    x = (x >> 22) ^ x;
    (x & 0xFFFF) as f32 / 65_535.0
}

/// The latency this node shows right now: its base plus a jitter that
/// changes about once a second, proportional to the distance.
fn latency_now(i: usize, node: &Node, t: f32) -> u32 {
    if node.health == Health::Hub {
        return 0;
    }
    let tick = (t * 0.8) as u32;
    let j = hash01(tick.wrapping_mul(97).wrapping_add(i as u32 * 131));
    let spread = (node.base_ms as f32 * 0.10).max(2.0);
    let extra = if node.health == Health::Degraded {
        // A degraded node wanders much further and never comes in low.
        node.base_ms as f32 * 0.25 * hash01(tick.wrapping_mul(53).wrapping_add(7))
    } else {
        0.0
    };
    (node.base_ms as f32 + (j - 0.5) * 2.0 * spread + extra).max(1.0) as u32
}

// ---- the scene ---------------------------------------------------------------

/// Everything the frame loop keeps between frames.
struct Scene {
    base: Fb,
    frame: Fb,
    cloud: Vec<[f32; 3]>,
    points: [[f32; 3]; NODES.len()],
    /// Measured widths of the latency strings, refreshed only when a string
    /// changes: measuring twelve strings a frame is twelve host calls.
    ms_cache: [(u32, f32); NODES.len()],
}

impl Scene {
    fn new() -> Scene {
        let mut base = Fb::new();
        render_base(&mut base);
        let mut points = [[0.0f32; 3]; NODES.len()];
        for (slot, node) in points.iter_mut().zip(NODES.iter()) {
            *slot = unit(node.lat, node.lon);
        }
        Scene {
            base,
            frame: Fb::new(),
            cloud: build_cloud(),
            points,
            ms_cache: [(u32::MAX, 0.0); NODES.len()],
        }
    }
}

fn draw(canvas: u64, scene: &mut Scene, t: f32) -> Result<(), gfx::GfxError> {
    let view = View::at(t);

    // The globe, into the buffer.
    scene.frame.px.copy_from_slice(&scene.base.px);
    draw_graticule(&mut scene.frame, &view);
    draw_cloud(&mut scene.frame, &view, &scene.cloud);
    draw_links(&mut scene.frame, &view, t, &scene.points);
    let mut spots = [Spot {
        x: 0.0,
        y: 0.0,
        z: -1.0,
    }; NODES.len()];
    draw_nodes(&mut scene.frame, &view, t, &scene.points, &mut spots);

    // Latencies for this frame, and the two summary numbers.
    let mut ms = [0u32; NODES.len()];
    for (i, node) in NODES.iter().enumerate() {
        if let Some(slot) = ms.get_mut(i) {
            *slot = latency_now(i, node, t);
        }
    }
    let mut sorted = ms;
    for i in 1..sorted.len() {
        let mut j = i;
        while j > 0 && sorted.get(j - 1).copied().unwrap_or(0) > sorted.get(j).copied().unwrap_or(0)
        {
            sorted.swap(j - 1, j);
            j -= 1;
        }
    }
    // The hub's own zero is not a measurement; the median is over the peers.
    let p50 = sorted.get(NODES.len() / 2).copied().unwrap_or(0);
    let p99 = sorted.last().copied().unwrap_or(0);

    // The ground.
    fill(canvas, 0.0, 0.0, WIDTH, HEIGHT, BG);

    // A faint radial lift behind the globe so it sits in space, not on a slab.
    let _ = canvas2d::radial_gradient(
        canvas,
        gfx::Point {
            x: GLOBE_CX,
            y: GLOBE_CY,
        },
        HALF * 1.25,
        rgba(0.231, 0.459, 1.0, 0.07),
        rgba(0.231, 0.459, 1.0, 0.0),
    );

    let _ = canvas2d::draw_pixels(
        canvas,
        gfx::Rect {
            x: GLOBE_CX - HALF,
            y: GLOBE_CY - HALF,
            width: GLOBE_AREA,
            height: GLOBE_AREA,
        },
        GLOBE_PX,
        GLOBE_PX,
        &scene.frame.px,
    );

    // Labels beside the nodes that face us. Text is a host call each, so
    // only the front ones get one, fading in as they come round the limb.
    for (i, (node, spot)) in NODES.iter().zip(spots.iter()).enumerate() {
        if spot.z < 0.18 {
            continue;
        }
        let a = ((spot.z - 0.18) / 0.35).min(1.0);
        let x = GLOBE_CX - HALF + spot.x;
        let y = GLOBE_CY - HALF + spot.y;
        let ink = match node.health {
            Health::Degraded => rgba(AMBER.r, AMBER.g, AMBER.b, a),
            _ => rgba(INK.r, INK.g, INK.b, a * 0.92),
        };
        // Alternate sides on the crowded European cluster so London,
        // Frankfurt and Stockholm do not stack on one another.
        let left = matches!(i, 1 | 4 | 7);
        let size = if node.health == Health::Hub {
            12.5
        } else {
            11.5
        };
        if left {
            let w = measure(canvas, node.name, size);
            text_w(
                canvas,
                node.name,
                x - 11.0 - w,
                y + 4.0,
                size,
                ink,
                600,
                0.0,
            );
        } else {
            text_w(canvas, node.name, x + 11.0, y + 4.0, size, ink, 600, 0.0);
        }
    }

    draw_titlebar(canvas, p50, p99);
    draw_panel(canvas, scene, &ms);
    draw_status(canvas, t);

    canvas2d::present(canvas)
}

fn draw_titlebar(canvas: u64, p50: u32, p99: u32) {
    fill(canvas, 0.0, 0.0, WIDTH, TITLEBAR_H, RAIL);
    fill(canvas, 0.0, TITLEBAR_H - 1.0, WIDTH, 1.0, LINE);

    // The app mark: a small blue disc with an orbit ring through it.
    rounded(canvas, 20.0, 15.0, 18.0, 18.0, 5.0, ACCENT);
    dot(canvas, 29.0, 24.0, 4.0, rgba(1.0, 1.0, 1.0, 0.95));
    text_w(canvas, "Orbit", 48.0, 30.0, 17.0, INK, 600, -0.2);
    text(canvas, "edge network", 105.0, 30.0, 13.0, INK_QUIET);

    // The live pill, centred.
    let pill_w = 128.0;
    let px = (WIDTH - pill_w) * 0.5;
    rounded(canvas, px, 11.0, pill_w, 26.0, 13.0, ROW_TINT);
    dot(canvas, px + 18.0, 24.0, 3.5, GREEN);
    text(
        canvas,
        "12 nodes \u{b7} live",
        px + 30.0,
        28.5,
        12.5,
        INK_DIM,
    );

    // Summary numbers on the right, label over value like Trace.
    let mut buf = [0u8; 16];
    let right = WIDTH - 24.0;
    let p99s = fmt_ms(&mut buf, p99);
    let w = measure(canvas, p99s, 14.0);
    text_w(canvas, p99s, right - w, 33.0, 14.0, AMBER, 600, 0.0);
    let lw = measure(canvas, "p99", 10.5);
    text(canvas, "p99", right - lw, 18.0, 10.5, INK_QUIET);

    let right = right - w.max(44.0) - 30.0;
    let mut buf = [0u8; 16];
    let p50s = fmt_ms(&mut buf, p50);
    let w = measure(canvas, p50s, 14.0);
    text_w(canvas, p50s, right - w, 33.0, 14.0, INK, 600, 0.0);
    let lw = measure(canvas, "p50", 10.5);
    text(canvas, "p50", right - lw, 18.0, 10.5, INK_QUIET);

    let right = right - w.max(44.0) - 30.0;
    let w = measure(canvas, "99.98%", 14.0);
    text_w(canvas, "99.98%", right - w, 33.0, 14.0, GREEN, 600, 0.0);
    let lw = measure(canvas, "uptime", 10.5);
    text(canvas, "uptime", right - lw, 18.0, 10.5, INK_QUIET);
}

fn draw_panel(canvas: u64, scene: &mut Scene, ms: &[u32; NODES.len()]) {
    let x0 = WIDTH - PANEL_W;
    fill(
        canvas,
        x0,
        TITLEBAR_H,
        PANEL_W,
        HEIGHT - TITLEBAR_H - STATUS_H,
        RAIL,
    );
    fill(
        canvas,
        x0,
        TITLEBAR_H,
        1.0,
        HEIGHT - TITLEBAR_H - STATUS_H,
        LINE,
    );

    let pad = 22.0;
    text_w(
        canvas,
        "NODES",
        x0 + pad,
        TITLEBAR_H + 34.0,
        11.0,
        INK_QUIET,
        600,
        1.4,
    );
    text(
        canvas,
        "latency from fra",
        x0 + PANEL_W - pad - measure(canvas, "latency from fra", 11.0),
        TITLEBAR_H + 34.0,
        11.0,
        INK_QUIET,
    );

    let row_h = 50.0;
    let first = TITLEBAR_H + 52.0;
    for (i, node) in NODES.iter().enumerate() {
        let y = first + i as f32 * row_h;
        if node.health == Health::Hub {
            rounded(
                canvas,
                x0 + 10.0,
                y + 4.0,
                PANEL_W - 20.0,
                row_h - 8.0,
                8.0,
                SEL_WASH,
            );
            fill(canvas, x0 + 10.0, y + 12.0, 3.0, row_h - 24.0, ACCENT);
        } else {
            fill(
                canvas,
                x0 + pad,
                y + row_h - 1.0,
                PANEL_W - pad * 2.0,
                1.0,
                LINE_SOFT,
            );
        }

        let (c, label) = match node.health {
            Health::Hub => (ACCENT_INK, "hub"),
            Health::Healthy => (GREEN, "healthy"),
            Health::Degraded => (AMBER, "degraded"),
        };
        dot(canvas, x0 + pad + 8.0, y + 20.0, 3.5, c);
        text_w(
            canvas,
            node.name,
            x0 + pad + 24.0,
            y + 24.5,
            13.5,
            INK,
            600,
            0.0,
        );

        // "fra · eu-central · healthy" as one quiet run.
        let mut sub = [0u8; 48];
        let mut n = 0usize;
        let sep = " \u{b7} ".as_bytes();
        for part in [
            node.code.as_bytes(),
            sep,
            node.region.as_bytes(),
            sep,
            label.as_bytes(),
        ] {
            for &b in part {
                if let Some(slot) = sub.get_mut(n) {
                    *slot = b;
                    n += 1;
                }
            }
        }
        let sub = core::str::from_utf8(sub.get(..n).unwrap_or(b"")).unwrap_or("");
        text(canvas, sub, x0 + pad + 24.0, y + 40.5, 11.0, INK_QUIET);

        let v = ms.get(i).copied().unwrap_or(0);
        let mut buf = [0u8; 16];
        let s: &str = if node.health == Health::Hub {
            "origin"
        } else {
            fmt_ms(&mut buf, v)
        };
        let size = 13.5;
        let w = match scene.ms_cache.get(i).copied() {
            Some((cached, w)) if cached == v => w,
            _ => {
                let w = measure(canvas, s, size);
                if let Some(slot) = scene.ms_cache.get_mut(i) {
                    *slot = (v, w);
                }
                w
            }
        };
        let ink = match node.health {
            Health::Hub => INK_DIM,
            Health::Healthy => INK,
            Health::Degraded => AMBER,
        };
        text_w(
            canvas,
            s,
            x0 + PANEL_W - pad - w,
            y + 29.0,
            size,
            ink,
            500,
            0.0,
        );
    }
}

fn draw_status(canvas: u64, t: f32) {
    let y0 = HEIGHT - STATUS_H;
    fill(canvas, 0.0, y0, WIDTH, STATUS_H, RAIL);
    fill(canvas, 0.0, y0, WIDTH, 1.0, LINE);
    let base = y0 + 22.0;

    let mut pen = 20.0;
    text(canvas, "11 peers", pen, base, 11.5, INK_DIM);
    pen += measure(canvas, "11 peers", 11.5);
    let rest = "  \u{b7}  1 degraded  \u{b7}  polling every 1s";
    text(canvas, rest, pen, base, 11.5, INK_QUIET);

    // Uptime, driven by the monotonic clock rather than wall time, so a
    // headless frame shot N milliseconds in reads the same on any machine at
    // any hour -- footage assembled from separate runs must not flicker.
    let up = 1_231_961u64 + t as u64;
    let mut up_buf = *b"up 14d 06:12:41";
    let days = up / 86_400;
    up_buf[3] = b'0' + ((days / 10) % 10) as u8;
    up_buf[4] = b'0' + (days % 10) as u8;
    let mut clock_buf = [0u8; 12];
    two(&mut clock_buf, 0, (up / 3600) % 24);
    two(&mut clock_buf, 2, (up / 60) % 60);
    two(&mut clock_buf, 4, up % 60);
    up_buf[7] = clock_buf[0];
    up_buf[8] = clock_buf[1];
    up_buf[10] = clock_buf[2];
    up_buf[11] = clock_buf[3];
    up_buf[13] = clock_buf[4];
    up_buf[14] = clock_buf[5];
    let clock_s = core::str::from_utf8(&up_buf).unwrap_or("up --d --:--:--");
    let w = measure(canvas, clock_s, 11.5);
    text(canvas, clock_s, WIDTH - 20.0 - w, base, 11.5, INK_QUIET);

    // Seconds into the session, so a viewer can see the clock is live.
    let mut buf = [0u8; 12];
    let s = fmt_u32(&mut buf, t as u32);
    let sw = measure(canvas, s, 11.5);
    let label = "session ";
    let lw = measure(canvas, label, 11.5);
    let x = WIDTH - 20.0 - w - 28.0 - sw - lw - 12.0;
    text(canvas, label, x, base, 11.5, INK_QUIET);
    text(canvas, s, x + lw, base, 11.5, INK_DIM);
    text(canvas, " s", x + lw + sw, base, 11.5, INK_QUIET);
}

// ---- scaffolding ---------------------------------------------------------------

fn out(line: &str) {
    let stdout = stdio::stdout();
    let _ = stdout.write(line.as_bytes());
    let _ = stdout.write(b"\n");
}

fn node_widget(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 1.0,
            padding: 0.0,
            text: None,
            place: None,
            box_: None,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw.split_whitespace().any(|word| word == "quick");

        let win = match window::create(
            "Orbit",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) {
            Ok(win) => win,
            Err(_) => {
                out("window:no");
                return 1;
            }
        };
        if tree::set_root(win, &node_widget(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            out("tree:no");
            return 1;
        }
        let _ = tree::upsert_node(
            win,
            &node_widget(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        );
        let _ = window::show(win);

        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => {
                out("bind:no");
                return 1;
            }
        };
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        let mut scene = Scene::new();

        let started = clock::monotonic_nanos();
        let mut frames: u32 = 0;

        const FRAME_NANOS: u64 = 1_000_000_000 / 60;
        let mut next_frame = clock::monotonic_nanos();

        loop {
            let now = clock::monotonic_nanos();
            let t = if quick {
                frames as f32 / 30.0
            } else {
                now.saturating_sub(started) as f32 / 1_000_000_000.0
            };

            if draw(canvas, &mut scene, t).is_err() {
                out("draw:no");
                return 1;
            }
            frames += 1;

            if quick {
                if frames >= QUICK_FRAMES {
                    break;
                }
                continue;
            }

            // Pace against the clock; never request-redraw from a loop that
            // already draws (the pack's redraw rule).
            next_frame = next_frame.saturating_add(FRAME_NANOS);
            let after_draw = clock::monotonic_nanos();
            if next_frame < after_draw {
                next_frame = after_draw;
            }
            let mut closing = false;
            loop {
                let now = clock::monotonic_nanos();
                let remaining = next_frame.saturating_sub(now);
                if remaining == 0 {
                    break;
                }
                let millis = (remaining / 1_000_000) as u32;
                match events::wait(Some(millis.max(1))) {
                    Some(types::Event::CloseRequested(_)) => {
                        closing = true;
                        break;
                    }
                    Some(_) | None => {}
                }
            }
            if closing {
                break;
            }
        }

        let mut buf = [0u8; 12];
        let mut line = *b"points:            ";
        let n = fmt_u32(&mut buf, scene.cloud.len() as u32);
        for (i, b) in n.bytes().enumerate() {
            if let Some(slot) = line.get_mut(7 + i) {
                *slot = b;
            }
        }
        out(
            core::str::from_utf8(line.get(..7 + n.len()).unwrap_or(b"points:?"))
                .unwrap_or("points:?"),
        );
        out("nodes:12");
        out("orbit:live");
        let _ = window::close(win);
        0
    }
}

bindings::export!(Component with_types_in bindings);
