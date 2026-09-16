//! A HUD made of triangles, because a 3D scene cannot have anything drawn
//! over it (K-398).
//!
//! `scene3d` has no text, and a 2D canvas cannot overlay a 3D one -- the only
//! containers are flow containers, so two canvases split the window rather
//! than stacking. Every number on screen is therefore built here out of quads
//! placed just in front of the camera, in camera space, so they track the view
//! exactly and sit in front of the world.
//!
//! This is a workaround, and an expensive one in effort: a seven-segment
//! font, hand-placed, to show a lap counter. It is written down as evidence
//! for K-398 rather than as a thing other apps should copy.

use alloc::vec::Vec;

use crate::mathx::{cos_approx, sin_approx};

/// Which segments of a seven-segment cell each digit lights.
///
/// Order: top, top-left, top-right, middle, bottom-left, bottom-right, bottom.
const DIGITS: [[bool; 7]; 10] = [
    [true, true, true, false, true, true, true],     // 0
    [false, false, true, false, false, true, false], // 1
    [true, false, true, true, true, false, true],    // 2
    [true, false, true, true, false, true, true],    // 3
    [false, true, true, true, false, true, false],   // 4
    [true, true, false, true, false, true, true],    // 5
    [true, true, false, true, true, true, true],     // 6
    [true, false, true, false, false, true, false],  // 7
    [true, true, true, true, true, true, true],      // 8
    [true, true, true, true, false, true, true],     // 9
];

/// A HUD element in camera space: x right, y up, both in units at the plane
/// `DIST` in front of the eye.
pub struct Hud {
    /// Quads accumulated this frame, in camera space.
    quads: Vec<[f32; 3]>,
}

/// How far in front of the eye the HUD plane sits. Near enough that nothing in
/// the world can poke through it, far enough to clear the near plane.
const DIST: f32 = 3.0;

impl Hud {
    pub fn new() -> Self {
        Self {
            quads: Vec::with_capacity(2048),
        }
    }

    pub fn clear(&mut self) {
        self.quads.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.quads.is_empty()
    }

    /// One axis-aligned rectangle on the HUD plane.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let (x0, x1) = (x, x + w);
        let (y0, y1) = (y, y + h);
        // Wound so the face survives back-face culling, which the game leaves
        // ON for the world.
        //
        // The obvious order -- (x0,y0), (x1,y0), (x1,y1) -- is counter-
        // clockwise in camera space with y up, but the projection flips y for
        // the screen, so it arrives as a clockwise (negative-area) triangle
        // and is culled as back-facing. The whole HUD was being built, sent,
        // and silently thrown away: nothing on screen, no error, and the
        // triangle count looked right.
        for p in [
            [x0, y0, DIST],
            [x1, y1, DIST],
            [x1, y0, DIST],
            [x0, y0, DIST],
            [x0, y1, DIST],
            [x1, y1, DIST],
        ] {
            self.quads.push(p);
        }
    }

    /// One seven-segment digit, `w` by `h`, bottom-left at (x, y).
    pub fn digit(&mut self, d: u32, x: f32, y: f32, w: f32, h: f32) {
        let seg = DIGITS[(d % 10) as usize];
        let t = (w * 0.20).min(h * 0.10);
        let half = h * 0.5;
        // top
        if seg[0] {
            self.rect(x + t, y + h - t, w - t * 2.0, t);
        }
        // top-left
        if seg[1] {
            self.rect(x, y + half, t, half - t);
        }
        // top-right
        if seg[2] {
            self.rect(x + w - t, y + half, t, half - t);
        }
        // middle
        if seg[3] {
            self.rect(x + t, y + half - t * 0.5, w - t * 2.0, t);
        }
        // bottom-left
        if seg[4] {
            self.rect(x, y + t, t, half - t);
        }
        // bottom-right
        if seg[5] {
            self.rect(x + w - t, y + t, t, half - t);
        }
        // bottom
        if seg[6] {
            self.rect(x + t, y, w - t * 2.0, t);
        }
    }

    /// A whole number, right-aligned so it does not jitter as it changes width.
    pub fn number(&mut self, mut v: u32, right: f32, y: f32, w: f32, h: f32, min_digits: u32) {
        let gap = w * 0.30;
        let mut x = right - w;
        let mut shown = 0;
        loop {
            self.digit(v % 10, x, y, w, h);
            v /= 10;
            shown += 1;
            if v == 0 && shown >= min_digits {
                break;
            }
            x -= w + gap;
        }
    }

    /// A colon, for a clock.
    pub fn colon(&mut self, x: f32, y: f32, w: f32, h: f32) {
        let t = w * 0.9;
        self.rect(x, y + h * 0.25, t, t);
        self.rect(x, y + h * 0.62, t, t);
    }

    /// Turn the accumulated camera-space quads into world triangles.
    ///
    /// The HUD is authored in camera space and transformed here by the same
    /// basis the host builds for the camera, so it lands exactly where it was
    /// placed regardless of where the car is or which way it faces.
    pub fn to_world(&self, eye: [f32; 3], look_at: [f32; 3]) -> Vec<f32> {
        // Rebuild the camera basis. This MUST match the host's derivation in
        // crates/runtime/src/scene3d.rs: right = world_up x forward, then
        // up = forward x right. Getting it wrong puts the HUD on its side or
        // mirrors it, and neither is obvious from one frame.
        let fx = look_at[0] - eye[0];
        let fy = look_at[1] - eye[1];
        let fz = look_at[2] - eye[2];
        let fl = (fx * fx + fy * fy + fz * fz).max(0.0001);
        let inv = 1.0 / crate::mathx::sqrt_approx(fl);
        let f = [fx * inv, fy * inv, fz * inv];

        // world_up x forward
        let rx = 1.0 * f[2] - 0.0 * f[1];
        let ry = 0.0 * f[0] - 0.0 * f[2];
        let rz = 0.0 * f[1] - 1.0 * f[0];
        let rl = crate::mathx::sqrt_approx(rx * rx + ry * ry + rz * rz).max(0.0001);
        let r = [rx / rl, ry / rl, rz / rl];

        // forward x right
        let u = [
            f[1] * r[2] - f[2] * r[1],
            f[2] * r[0] - f[0] * r[2],
            f[0] * r[1] - f[1] * r[0],
        ];

        let mut out = Vec::with_capacity(self.quads.len() * 3);
        for p in &self.quads {
            let (cx, cy, cz) = (p[0], p[1], p[2]);
            out.push(eye[0] + r[0] * cx + u[0] * cy + f[0] * cz);
            out.push(eye[1] + r[1] * cx + u[1] * cy + f[1] * cz);
            out.push(eye[2] + r[2] * cx + u[2] * cy + f[2] * cz);
        }
        out
    }
}

/// A flat ring of triangles, used for the countdown lights.
pub fn disc(cx: f32, cy: f32, radius: f32, segments: usize, out: &mut Hud) {
    for i in 0..segments {
        let a0 = i as f32 / segments as f32 * core::f32::consts::PI * 2.0;
        let a1 = (i + 1) as f32 / segments as f32 * core::f32::consts::PI * 2.0;
        let (x0, y0) = (cx + cos_approx(a0) * radius, cy + sin_approx(a0) * radius);
        let (x1, y1) = (cx + cos_approx(a1) * radius, cy + sin_approx(a1) * radius);
        // Same winding as `rect`, and for the same reason.
        for p in [[cx, cy, DIST], [x1, y1, DIST], [x0, y0, DIST]] {
            out.quads.push(p);
        }
    }
}
