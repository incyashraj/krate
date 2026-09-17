//! Textures, generated in the guest.
//!
//! The first version of this game drew everything as flat untinted polygons
//! and looked about 1997 -- which was the app's fault, not the runtime's:
//! `upload_texture` and `textured` were there the whole time and went unused.
//!
//! Every texture here is synthesised rather than shipped, so the bundle stays
//! small and there are no assets to load. They are small on purpose: the
//! sampler wraps, so a 64x64 asphalt tile covers a kilometre of road, and a
//! small tile stays in cache while the rasterizer is hammering it.

use alloc::vec;
use alloc::vec::Vec;

use crate::mathx::{abs, hash2};

/// One RGBA image, ready for `upload_texture`.
pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn blank(width: u32, height: u32) -> Texture {
    Texture {
        width,
        height,
        rgba: vec![0; (width * height * 4) as usize],
    }
}

fn put(t: &mut Texture, x: u32, y: u32, r: impl Into<f64>, g: impl Into<f64>, b: impl Into<f64>) {
    let (r, g, b) = (
        r.into().clamp(0.0, 255.0) as u8,
        g.into().clamp(0.0, 255.0) as u8,
        b.into().clamp(0.0, 255.0) as u8,
    );
    let i = ((y * t.width + x) * 4) as usize;
    if i + 3 < t.rgba.len() {
        t.rgba[i] = r;
        t.rgba[i + 1] = g;
        t.rgba[i + 2] = b;
        t.rgba[i + 3] = 255;
    }
}

/// Asphalt: dark, speckled, with a lighter worn line down each wheel track.
///
/// The speckle is what does the work. A flat grey road at speed is a moving
/// void with no texture to read motion against; noise at this scale reads as
/// surface and makes the sense of speed come almost entirely from the ground.
pub fn asphalt() -> Texture {
    let mut t = blank(64, 64);
    for y in 0..64u32 {
        for x in 0..64u32 {
            let n = hash2(x as i32, y as i32);
            let base = 44.0 + n * 26.0;
            // Two worn tracks where tyres polish the surface lighter.
            let track = {
                let d1 = abs(x as f32 - 18.0);
                let d2 = abs(x as f32 - 46.0);
                let d = if d1 < d2 { d1 } else { d2 };
                if d < 7.0 {
                    (7.0 - d) / 7.0 * 12.0
                } else {
                    0.0
                }
            };
            let v = (base + track) as u8;
            put(&mut t, x, y, v, v, (v as f32 * 1.04) as u8);
        }
    }
    t
}

/// A normal map for the asphalt: the chippings, as directions rather than
/// shading.
///
/// The colour texture already has speckle painted into it, but painted
/// speckle is the same whichever way the light falls -- it reads as dirt on a
/// flat sheet. A normal map makes each chipping a real bump the light reacts
/// to, so the road looks different driving into the sun and away from it. That
/// reaction is what separates a surface from a picture of one.
///
/// The convention is the usual one: red and green carry the x and y of the
/// direction, offset so 128 is zero, and blue carries z, so flat is
/// (128, 128, 255) and an unused map reads as pale blue.
pub fn asphalt_normals() -> Texture {
    let mut t = blank(64, 64);
    for y in 0..64u32 {
        for x in 0..64u32 {
            // The height field the normals come from: the same hash the colour
            // texture speckles with, so a bump sits where a light chipping is
            // rather than somewhere unrelated.
            let h = |ix: i32, iy: i32| -> f32 { hash2(ix & 63, iy & 63) };
            // Slope from the neighbours. A central difference rather than a
            // forward one, so a bump leans evenly instead of shifting half a
            // texel toward the light.
            let dx = h(x as i32 + 1, y as i32) - h(x as i32 - 1, y as i32);
            let dy = h(x as i32, y as i32 + 1) - h(x as i32, y as i32 - 1);
            // Strength: enough to catch a low sun, not so much that the road
            // looks like gravel.
            let strength = 0.55;
            let nx = -dx * strength;
            let ny = -dy * strength;
            let nz = 1.0;
            let len = crate::mathx::sqrt_approx(nx * nx + ny * ny + nz * nz).max(0.0001);
            put(
                &mut t,
                x,
                y,
                (nx / len * 0.5 + 0.5) * 255.0,
                (ny / len * 0.5 + 0.5) * 255.0,
                (nz / len * 0.5 + 0.5) * 255.0,
            );
        }
    }
    t
}

/// Grass: green noise at two scales, so it does not read as a flat field.
pub fn grass() -> Texture {
    let mut t = blank(64, 64);
    for y in 0..64u32 {
        for x in 0..64u32 {
            let fine = hash2(x as i32, y as i32);
            let coarse = hash2(x as i32 / 8, y as i32 / 8);
            let g = 92.0 + coarse * 38.0 + fine * 22.0;
            let r = g * 0.52;
            let b = g * 0.40;
            put(&mut t, x, y, r as u8, g as u8, b as u8);
        }
    }
    t
}

/// A building face: rows of lit and dark windows in a concrete wall.
///
/// This is the single biggest change to how the world reads. Grey boxes are
/// grey boxes at any triangle count; the same box with windows is a building,
/// and at speed the window rows are what tell you how fast you are passing it.
pub fn facade(seed: i32) -> Texture {
    let mut t = blank(64, 64);
    let wall = 96.0 + hash2(seed, 1) * 54.0;
    for y in 0..64u32 {
        for x in 0..64u32 {
            let grain = hash2(x as i32 + seed, y as i32) * 14.0;
            let v = wall + grain;
            put(&mut t, x, y, v as u8, (v * 0.99) as u8, (v * 0.95) as u8);
        }
    }
    // Windows: 6 columns, 8 rows, with a margin so the wall shows between.
    for row in 0..8u32 {
        for col in 0..6u32 {
            let lit = hash2(seed + col as i32 * 7, row as i32 * 13) > 0.62;
            let (r, g, b) = if lit {
                (236u8, 214u8, 150u8)
            } else {
                (38u8, 44u8, 56u8)
            };
            let x0 = 4 + col * 10;
            let y0 = 3 + row * 8;
            for y in y0..(y0 + 5).min(64) {
                for x in x0..(x0 + 7).min(64) {
                    put(&mut t, x, y, r, g, b);
                }
            }
        }
    }
    t
}

/// The kerb stripe, as one tile: red and white blocks with a dark edge.
pub fn kerb() -> Texture {
    let mut t = blank(32, 32);
    for y in 0..32u32 {
        for x in 0..32u32 {
            let red = (y / 8) % 2 == 0;
            let (r, g, b) = if red {
                (196u8, 42u8, 40u8)
            } else {
                (232u8, 232u8, 230u8)
            };
            // Darken the outer edge so the kerb has a lip rather than being
            // a flat painted strip.
            let edge = if x < 3 { 0.66 } else { 1.0 };
            put(
                &mut t,
                x,
                y,
                (r as f32 * edge) as u8,
                (g as f32 * edge) as u8,
                (b as f32 * edge) as u8,
            );
        }
    }
    t
}
