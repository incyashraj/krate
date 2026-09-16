//! Some builders here are unused by the current scene and kept because the
//! point of this crate is to show what the renderer can be made to do.
#![allow(dead_code)]

//! Mesh building for the showcase: smooth surfaces out of many small
//! triangles, because that is how this renderer gets smooth shading.
//!
//! The shading model is `0.35 + 0.65 * |n . l|` computed PER TRIANGLE, from
//! the triangle's own face normal. There are no vertex normals, so a curved
//! surface cannot be smooth-shaded the usual way. What it can be is
//! subdivided: a sphere of 4,000 facets has 4,000 slightly different normals,
//! and at that density the facets are smaller than the shading difference
//! between them. The renderer does the rest for free.
//!
//! This is the whole trick behind the scene. A 1997-looking game is not what
//! this rasterizer is limited to; it is what you get when every object is a
//! six-sided box.

use alloc::vec::Vec;

use crate::mathx::{cos_approx, sin_approx, sqrt_approx};

/// Triangles plus their UVs, the pair `scene3d::textured` wants.
#[derive(Default)]
pub struct Mesh {
    pub verts: Vec<f32>,
    pub uvs: Vec<f32>,
}

impl Mesh {
    pub fn tris(&self) -> usize {
        self.verts.len() / 9
    }

    pub fn push(&mut self, p: [f32; 3], uv: [f32; 2]) {
        self.verts.push(p[0]);
        self.verts.push(p[1]);
        self.verts.push(p[2]);
        self.uvs.push(uv[0]);
        self.uvs.push(uv[1]);
    }

    /// A quad as two triangles, wound counter-clockwise seen from outside.
    pub fn quad(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3], uv: [[f32; 2]; 4]) {
        self.push(a, uv[0]);
        self.push(b, uv[1]);
        self.push(c, uv[2]);
        self.push(a, uv[0]);
        self.push(c, uv[2]);
        self.push(d, uv[3]);
    }

    pub fn extend(&mut self, other: &Mesh) {
        self.verts.extend_from_slice(&other.verts);
        self.uvs.extend_from_slice(&other.uvs);
    }

    /// Move every vertex, for placing a built mesh in the world. `textured`
    /// has no `place` counterpart, so a textured mesh has to arrive in world
    /// space and this is how it gets there.
    pub fn translated(&self, dx: f32, dy: f32, dz: f32) -> Mesh {
        let mut out = Mesh {
            verts: Vec::with_capacity(self.verts.len()),
            uvs: self.uvs.clone(),
        };
        for p in self.verts.chunks_exact(3) {
            out.verts.push(p[0] + dx);
            out.verts.push(p[1] + dy);
            out.verts.push(p[2] + dz);
        }
        out
    }

    /// Rotate about Y, then translate.
    pub fn placed(&self, dx: f32, dy: f32, dz: f32, yaw: f32) -> Mesh {
        let (s, c) = (sin_approx(yaw), cos_approx(yaw));
        let mut out = Mesh {
            verts: Vec::with_capacity(self.verts.len()),
            uvs: self.uvs.clone(),
        };
        for p in self.verts.chunks_exact(3) {
            out.verts.push(p[0] * c + p[2] * s + dx);
            out.verts.push(p[1] + dy);
            out.verts.push(-p[0] * s + p[2] * c + dz);
        }
        out
    }
}

/// A sphere built from stacked rings.
///
/// `rings` x `segments` x 2 triangles. At 40x40 that is 3,200 facets, which
/// shades as a smooth ball rather than a die.
pub fn sphere(radius: f32, rings: usize, segments: usize) -> Mesh {
    let mut m = Mesh::default();
    const PI: f32 = core::f32::consts::PI;
    for ring in 0..rings {
        let t0 = ring as f32 / rings as f32;
        let t1 = (ring + 1) as f32 / rings as f32;
        let (p0, p1) = (t0 * PI, t1 * PI);
        let (y0, r0) = (cos_approx(p0) * radius, sin_approx(p0) * radius);
        let (y1, r1) = (cos_approx(p1) * radius, sin_approx(p1) * radius);
        for seg in 0..segments {
            let u0 = seg as f32 / segments as f32;
            let u1 = (seg + 1) as f32 / segments as f32;
            let (a0, a1) = (u0 * PI * 2.0, u1 * PI * 2.0);
            let (s0, c0) = (sin_approx(a0), cos_approx(a0));
            let (s1, c1) = (sin_approx(a1), cos_approx(a1));
            m.quad(
                [c0 * r0, y0, s0 * r0],
                [c0 * r1, y1, s0 * r1],
                [c1 * r1, y1, s1 * r1],
                [c1 * r0, y0, s1 * r0],
                [[u0, t0], [u0, t1], [u1, t1], [u1, t0]],
            );
        }
    }
    m
}

/// A capped cylinder along Y.
pub fn cylinder(radius: f32, height: f32, segments: usize, rings: usize) -> Mesh {
    let mut m = Mesh::default();
    const PI: f32 = core::f32::consts::PI;
    for ring in 0..rings {
        let y0 = ring as f32 / rings as f32 * height;
        let y1 = (ring + 1) as f32 / rings as f32 * height;
        let (v0, v1) = (y0 / height, y1 / height);
        for seg in 0..segments {
            let u0 = seg as f32 / segments as f32;
            let u1 = (seg + 1) as f32 / segments as f32;
            let (a0, a1) = (u0 * PI * 2.0, u1 * PI * 2.0);
            let (s0, c0) = (sin_approx(a0) * radius, cos_approx(a0) * radius);
            let (s1, c1) = (sin_approx(a1) * radius, cos_approx(a1) * radius);
            m.quad(
                [c0, y0, s0],
                [c0, y1, s0],
                [c1, y1, s1],
                [c1, y0, s1],
                [[u0, v0], [u0, v1], [u1, v1], [u1, v0]],
            );
        }
    }
    // Caps as fans.
    for seg in 0..segments {
        let u0 = seg as f32 / segments as f32;
        let u1 = (seg + 1) as f32 / segments as f32;
        let (a0, a1) = (u0 * PI * 2.0, u1 * PI * 2.0);
        let (s0, c0) = (sin_approx(a0) * radius, cos_approx(a0) * radius);
        let (s1, c1) = (sin_approx(a1) * radius, cos_approx(a1) * radius);
        m.push([0.0, height, 0.0], [0.5, 0.5]);
        m.push([c0, height, s0], [u0, 0.0]);
        m.push([c1, height, s1], [u1, 0.0]);
        m.push([0.0, 0.0, 0.0], [0.5, 0.5]);
        m.push([c1, 0.0, s1], [u1, 0.0]);
        m.push([c0, 0.0, s0], [u0, 0.0]);
    }
    m
}

/// A box with BEVELLED edges, built as a subdivided surface.
///
/// A plain box has six faces and six normals, and reads as a plain box. The
/// same box with a chamfer has twenty-six, and the chamfer catches the light
/// differently from the faces either side of it -- which is most of what makes
/// a rendered object look manufactured rather than blocked out.
pub fn bevel_box(w: f32, h: f32, d: f32, bevel: f32) -> Mesh {
    let mut m = Mesh::default();
    let b = bevel.min(w.min(h.min(d)) * 0.45);
    let (x0, x1) = (-w * 0.5, w * 0.5);
    let (y0, y1) = (0.0, h);
    let (z0, z1) = (-d * 0.5, d * 0.5);

    // The six faces, inset by the bevel.
    let faces: [([[f32; 3]; 4], [[f32; 2]; 4]); 6] = [
        // front (-z)
        (
            [
                [x0 + b, y0 + b, z0],
                [x1 - b, y0 + b, z0],
                [x1 - b, y1 - b, z0],
                [x0 + b, y1 - b, z0],
            ],
            [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        ),
        // back (+z)
        (
            [
                [x1 - b, y0 + b, z1],
                [x0 + b, y0 + b, z1],
                [x0 + b, y1 - b, z1],
                [x1 - b, y1 - b, z1],
            ],
            [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        ),
        // left (-x)
        (
            [
                [x0, y0 + b, z1 - b],
                [x0, y0 + b, z0 + b],
                [x0, y1 - b, z0 + b],
                [x0, y1 - b, z1 - b],
            ],
            [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        ),
        // right (+x)
        (
            [
                [x1, y0 + b, z0 + b],
                [x1, y0 + b, z1 - b],
                [x1, y1 - b, z1 - b],
                [x1, y1 - b, z0 + b],
            ],
            [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
        ),
        // top (+y)
        (
            [
                [x0 + b, y1, z0 + b],
                [x1 - b, y1, z0 + b],
                [x1 - b, y1, z1 - b],
                [x0 + b, y1, z1 - b],
            ],
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        ),
        // bottom (-y)
        (
            [
                [x0 + b, y0, z1 - b],
                [x1 - b, y0, z1 - b],
                [x1 - b, y0, z0 + b],
                [x0 + b, y0, z0 + b],
            ],
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        ),
    ];
    for (quad, uv) in faces {
        m.quad(quad[0], quad[1], quad[2], quad[3], uv);
    }

    // Four vertical chamfers. These are the strips that catch the light.
    let verticals: [([f32; 2], [f32; 2]); 4] = [
        ([x0 + b, z0], [x0, z0 + b]),
        ([x1, z0 + b], [x1 - b, z0]),
        ([x1 - b, z1], [x1, z1 - b]),
        ([x0, z1 - b], [x0 + b, z1]),
    ];
    for (p, q) in verticals {
        m.quad(
            [p[0], y0 + b, p[1]],
            [q[0], y0 + b, q[1]],
            [q[0], y1 - b, q[1]],
            [p[0], y1 - b, p[1]],
            [[0.0, 1.0], [0.3, 1.0], [0.3, 0.0], [0.0, 0.0]],
        );
    }

    // Top chamfers, one per side.
    let tops: [([f32; 3], [f32; 3], [f32; 3], [f32; 3]); 4] = [
        (
            [x0 + b, y1 - b, z0],
            [x1 - b, y1 - b, z0],
            [x1 - b, y1, z0 + b],
            [x0 + b, y1, z0 + b],
        ),
        (
            [x1 - b, y1 - b, z1],
            [x0 + b, y1 - b, z1],
            [x0 + b, y1, z1 - b],
            [x1 - b, y1, z1 - b],
        ),
        (
            [x0, y1 - b, z1 - b],
            [x0, y1 - b, z0 + b],
            [x0 + b, y1, z0 + b],
            [x0 + b, y1, z1 - b],
        ),
        (
            [x1, y1 - b, z0 + b],
            [x1, y1 - b, z1 - b],
            [x1 - b, y1, z1 - b],
            [x1 - b, y1, z0 + b],
        ),
    ];
    for (a, bb, c, d2) in tops {
        m.quad(a, bb, c, d2, [[0.0, 1.0], [1.0, 1.0], [1.0, 0.7], [0.0, 0.7]]);
    }
    m
}

/// A ground plane subdivided into `cells` squares each way, with a gentle
/// height field so it is not a flat sheet.
pub fn ground(extent: f32, cells: usize, uv_tiles: f32, height: impl Fn(f32, f32) -> f32) -> Mesh {
    let mut m = Mesh::default();
    let step = extent * 2.0 / cells as f32;
    for gz in 0..cells {
        for gx in 0..cells {
            let x0 = -extent + gx as f32 * step;
            let z0 = -extent + gz as f32 * step;
            let x1 = x0 + step;
            let z1 = z0 + step;
            let u0 = gx as f32 / cells as f32 * uv_tiles;
            let u1 = (gx + 1) as f32 / cells as f32 * uv_tiles;
            let v0 = gz as f32 / cells as f32 * uv_tiles;
            let v1 = (gz + 1) as f32 / cells as f32 * uv_tiles;
            m.quad(
                [x0, height(x0, z0), z0],
                [x0, height(x0, z1), z1],
                [x1, height(x1, z1), z1],
                [x1, height(x1, z0), z0],
                [[u0, v0], [u0, v1], [u1, v1], [u1, v0]],
            );
        }
    }
    m
}

/// A torus, for something unmistakably curved in the scene.
pub fn torus(major: f32, minor: f32, rings: usize, segments: usize) -> Mesh {
    let mut m = Mesh::default();
    const PI: f32 = core::f32::consts::PI;
    for ring in 0..rings {
        let t0 = ring as f32 / rings as f32;
        let t1 = (ring + 1) as f32 / rings as f32;
        let (a0, a1) = (t0 * PI * 2.0, t1 * PI * 2.0);
        for seg in 0..segments {
            let u0 = seg as f32 / segments as f32;
            let u1 = (seg + 1) as f32 / segments as f32;
            let (b0, b1) = (u0 * PI * 2.0, u1 * PI * 2.0);
            let point = |a: f32, b: f32| -> [f32; 3] {
                let r = major + minor * cos_approx(b);
                [cos_approx(a) * r, minor * sin_approx(b), sin_approx(a) * r]
            };
            m.quad(
                point(a0, b0),
                point(a0, b1),
                point(a1, b1),
                point(a1, b0),
                [[t0, u0], [t0, u1], [t1, u1], [t1, u0]],
            );
        }
    }
    m
}

/// Normalise, used when a caller needs a direction.
pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = sqrt_approx(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).max(0.0001);
    [v[0] / l, v[1] / l, v[2] / l]
}
