//! Shared with the racing game; not every helper is used by this scene.
#![allow(dead_code)]

//! The floating-point maths a `#![no_std]` guest does not get.
//!
//! `f32::sin`, `cos`, `sqrt`, `floor` and `abs` all live in std, so every
//! Krate app that does geometry brings its own. This has now cost two proof
//! checkpoints (CP-A and CP-E), the second of which lost an evening to a
//! heightmap that ranged -75..+60 because `as i32` truncates toward zero and
//! the cell fraction came out negative.

/// Round toward negative infinity.
///
/// `as i32` truncates toward ZERO, so -3.7 becomes -3, not -4, and the
/// fraction `v - floor(v)` comes out negative. Anything that then feeds that
/// fraction to a smoothstep gets nonsense, because `t*t*(3-2t)` is only a
/// 0..1 ramp for t in 0..1.
pub fn floor_f32(v: f32) -> f32 {
    let t = v as i32;
    if v < 0.0 && v != t as f32 {
        (t - 1) as f32
    } else {
        t as f32
    }
}

/// Round toward positive infinity. `f32::ceil` is std-only, like the rest.
pub fn ceil_f32(v: f32) -> f32 {
    let f = floor_f32(v);
    if v > f {
        f + 1.0
    } else {
        f
    }
}

pub fn abs(v: f32) -> f32 {
    if v < 0.0 {
        -v
    } else {
        v
    }
}

/// Bhaskara's approximation, good to about 0.2% over a full period.
pub fn sin_approx(x: f32) -> f32 {
    const PI: f32 = core::f32::consts::PI;
    const TWO_PI: f32 = PI * 2.0;
    let mut a = x - floor_f32(x / TWO_PI) * TWO_PI;
    if a < 0.0 {
        a += TWO_PI;
    }
    let (a, sign) = if a > PI { (a - PI, -1.0) } else { (a, 1.0) };
    let num = 16.0 * a * (PI - a);
    let den = 5.0 * PI * PI - 4.0 * a * (PI - a);
    sign * num / den
}

pub fn cos_approx(x: f32) -> f32 {
    sin_approx(x + core::f32::consts::FRAC_PI_2)
}

/// Newton's method from a decent seed. Eight rounds is far more than needed
/// for the ranges here and still cheaper than anything clever.
pub fn sqrt_approx(v: f32) -> f32 {
    if v <= 0.0 {
        return 0.0;
    }
    let mut g = v;
    for _ in 0..8 {
        g = 0.5 * (g + v / g);
    }
    g
}

/// Angle of the vector (x, z), in radians, over the full circle.
///
/// Needed to steer an opponent toward a point: the difference between where it
/// is pointing and where it wants to point is the whole of its steering input.
pub fn atan2_approx(z: f32, x: f32) -> f32 {
    const PI: f32 = core::f32::consts::PI;
    if x == 0.0 && z == 0.0 {
        return 0.0;
    }
    let ax = abs(x);
    let az = abs(z);
    // A rational approximation on the octant, mirrored out to the full circle.
    let a = if ax >= az { az / ax } else { ax / az };
    let s = a * a;
    let mut r = ((-0.046_496_47 * s + 0.159_314_22) * s - 0.327_622_764) * s * a + a;
    if az > ax {
        r = PI / 2.0 - r;
    }
    if x < 0.0 {
        r = PI - r;
    }
    if z < 0.0 {
        r = -r;
    }
    r
}

/// Shortest signed angle from `from` to `to`, in -PI..PI.
///
/// Without the wrap a car aiming just past the +PI seam steers the long way
/// round the circle -- it spins on the spot at exactly one point on the lap,
/// which is a confusing thing to debug from the outside.
pub fn angle_delta(from: f32, to: f32) -> f32 {
    const PI: f32 = core::f32::consts::PI;
    const TWO_PI: f32 = PI * 2.0;
    let mut d = to - from;
    while d > PI {
        d -= TWO_PI;
    }
    while d < -PI {
        d += TWO_PI;
    }
    d
}

/// Cheap deterministic hash to a 0..1 float, for scattering scenery.
pub fn hash2(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
    h ^= h >> 13;
    h = h.wrapping_mul(1_274_126_177);
    ((h >> 8) & 0xFFFF) as f32 / 65_535.0
}
