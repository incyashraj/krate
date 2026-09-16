//! Textures, painted in the guest at 512 and 1024 pixels.
//!
//! This is where most of the look comes from. The renderer gives one
//! directional light and a per-face term, so everything else -- the sense that
//! a surface is worn, that a corner is darker than a middle, that marble has
//! depth, that a floor reflects -- has to be painted in.
//!
//! Nothing here is loaded from disk. A bundle with no assets is still the
//! point; these are a few hundred lines of arithmetic that happen to look like
//! materials.

use alloc::vec;
use alloc::vec::Vec;

use crate::mathx::{abs, cos_approx, floor_f32, sin_approx};

pub struct Texture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Texture {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            rgba: vec![0; (width * height * 4) as usize],
        }
    }

    fn put(&mut self, x: u32, y: u32, r: f32, g: f32, b: f32) {
        let i = ((y * self.width + x) * 4) as usize;
        if i + 3 < self.rgba.len() {
            self.rgba[i] = r.clamp(0.0, 255.0) as u8;
            self.rgba[i + 1] = g.clamp(0.0, 255.0) as u8;
            self.rgba[i + 2] = b.clamp(0.0, 255.0) as u8;
            self.rgba[i + 3] = 255;
        }
    }
}

/// Hash to 0..1.
fn hash(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
    h ^= h >> 13;
    h = h.wrapping_mul(1_274_126_177);
    ((h >> 8) & 0xFFFF) as f32 / 65_535.0
}

/// Value noise with smooth interpolation, the building block for everything.
fn noise(x: f32, y: f32) -> f32 {
    let x0 = floor_f32(x);
    let y0 = floor_f32(y);
    let tx = x - x0;
    let ty = y - y0;
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let a = hash(ix, iy);
    let b = hash(ix + 1, iy);
    let c = hash(ix, iy + 1);
    let d = hash(ix + 1, iy + 1);
    let top = a + (b - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sy
}

/// Several octaves, which is what turns noise into a material.
fn fbm(x: f32, y: f32, octaves: usize) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for _ in 0..octaves {
        sum += noise(x * freq, y * freq) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.04;
    }
    sum / norm.max(0.0001)
}

/// Polished marble: veins of a lighter mineral through a dark stone, with the
/// vein direction warped by noise so it never reads as stripes.
pub fn marble(size: u32) -> Texture {
    let mut t = Texture::new(size, size);
    let s = size as f32;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 / s * 6.0, y as f32 / s * 6.0);
            // Warp the coordinate before taking the vein function: this is
            // what bends the veins into something geological.
            let warp = fbm(fx * 1.7, fy * 1.7, 4) * 2.2;
            let vein = sin_approx((fx + warp) * 3.1) * 0.5 + 0.5;
            // Thin veins in a pale stone, not pale blobs in a dark one. The
            // first version cubed the vein term and sat it on a near-black
            // base, which reads as camouflage rather than as marble -- real
            // stone is mostly one light tone with a few darker seams.
            let seam = (1.0 - vein).powi(6);
            let grain = fbm(fx * 7.0, fy * 7.0, 4);
            let base = 176.0 + grain * 30.0 - seam * 96.0;
            t.put(x, y, base * 1.0, base * 0.985, base * 0.95);
        }
    }
    t
}

/// Brushed metal: fine anisotropic streaks, plus a broad sheen so it is not
/// uniformly bright. The sheen is the fake specular -- the renderer has no
/// specular term, so a highlight has to be painted where one would fall.
pub fn brushed_metal(size: u32) -> Texture {
    let mut t = Texture::new(size, size);
    let s = size as f32;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 / s, y as f32 / s);
            // Streaks: high frequency across, very low along.
            let streak = noise(fx * 420.0, fy * 2.0) * 0.5 + noise(fx * 90.0, fy * 1.3) * 0.5;
            // A soft band of brightness, as though a light source is smeared
            // along the brush direction.
            let sheen = {
                let d = abs(fy - 0.38);
                (1.0 - (d * 2.4).min(1.0)).powi(2)
            };
            let v = 96.0 + streak * 58.0 + sheen * 74.0;
            t.put(x, y, v * 1.01, v * 1.0, v * 0.97);
        }
    }
    t
}

/// Car paint: a deep flake-metallic, with a lighter top half as though the sky
/// is reflected in the upper surfaces. The gradient IS the reflection -- there
/// is no environment map, so the illusion is painted.
pub fn car_paint(size: u32, tint: (f32, f32, f32)) -> Texture {
    let mut t = Texture::new(size, size);
    let s = size as f32;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 / s, y as f32 / s);
            // Metallic flake, very fine.
            let flake = hash(x as i32, y as i32) * 0.10 + fbm(fx * 180.0, fy * 180.0, 2) * 0.12;
            // Sky reflection falling off downward.
            let sky = (1.0 - fy).powi(3) * 0.55;
            let base = 0.42 + flake + sky;
            t.put(
                x,
                y,
                255.0 * base * tint.0 + sky * 90.0,
                255.0 * base * tint.1 + sky * 104.0,
                255.0 * base * tint.2 + sky * 130.0,
            );
        }
    }
    t
}

/// A polished floor with a grid of inlaid tiles, darkened toward each tile's
/// edge. That edge darkening is cheap ambient occlusion, painted: it is what
/// stops a flat floor reading as a flat sheet.
pub fn floor_tiles(size: u32) -> Texture {
    let mut t = Texture::new(size, size);
    let s = size as f32;
    let cells = 8.0;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 / s * cells, y as f32 / s * cells);
            let (cx, cy) = (fx - floor_f32(fx), fy - floor_f32(fy));
            // Distance to the nearest tile edge, 0 at the grout.
            let edge = {
                let dx = cx.min(1.0 - cx);
                let dy = cy.min(1.0 - cy);
                dx.min(dy)
            };
            let grout = (edge / 0.045).min(1.0);
            // Occlusion ramp: darker for the first tenth of the tile.
            let ao = 0.72 + (edge / 0.12).min(1.0) * 0.28;
            let stone = fbm(fx * 3.0, fy * 3.0, 4);
            let tile_seed = hash(floor_f32(fx) as i32, floor_f32(fy) as i32);
            let base = (150.0 + stone * 48.0 + tile_seed * 16.0) * ao;
            let v = if grout < 1.0 {
                base * (0.34 + grout * 0.66)
            } else {
                base
            };
            t.put(x, y, v * 1.0, v * 0.99, v * 0.96);
        }
    }
    t
}

/// A sky gradient painted as a texture, for a dome around the scene.
///
/// The renderer has no fog and no sky, and a flat `clear` colour behind
/// geometry is the single biggest tell that a scene is 3D-with-nothing-behind
/// it. A dome with a real gradient and a sun glow reads as outdoors.
pub fn sky(size: u32) -> Texture {
    let mut t = Texture::new(size, size);
    let s = size as f32;
    for y in 0..size {
        for x in 0..size {
            let fy = y as f32 / s;
            let fx = x as f32 / s;
            // Height gradient: deep blue at the zenith, pale at the horizon.
            let up = 1.0 - fy;
            let r = 96.0 + (1.0 - up) * 128.0;
            let g = 140.0 + (1.0 - up) * 96.0;
            let b = 214.0 + (1.0 - up) * 30.0;
            // A sun, low and to one side, with a wide glow.
            let (sx, sy) = (0.30, 0.30);
            let d = {
                let dx = (fx - sx) * 2.2;
                let dy = fy - sy;
                (dx * dx + dy * dy).sqrt()
            };
            let glow = (1.0 - (d * 3.4).min(1.0)).powi(3) * 150.0;
            let disc = if d < 0.022 { 190.0 } else { 0.0 };
            // A band of thin cloud, so the sky is not an empty gradient.
            let cloud = {
                let c = fbm(fx * 5.0, fy * 11.0, 5);
                let band = (1.0 - abs(fy - 0.52) * 5.0).max(0.0);
                ((c - 0.52).max(0.0) * 2.6 * band).min(1.0) * 74.0
            };
            t.put(
                x,
                y,
                r + glow + disc + cloud,
                g + glow * 0.92 + disc + cloud,
                b + glow * 0.66 + disc + cloud,
            );
        }
    }
    t
}

/// A dark panel with emissive strips, for the pillars. Painted bright enough
/// that the shading floor of 0.35 still leaves them glowing.
pub fn panel(size: u32) -> Texture {
    let mut t = Texture::new(size, size);
    let s = size as f32;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 / s, y as f32 / s);
            let grain = fbm(fx * 16.0, fy * 16.0, 3);
            let mut v = 30.0 + grain * 26.0;
            let mut rgb = (v, v * 1.02, v * 1.1);
            // Horizontal light strips.
            let band = (fy * 9.0) - floor_f32(fy * 9.0);
            if (0.44..0.56).contains(&band) {
                let heat = 1.0 - abs(band - 0.5) * 16.0;
                v = 150.0 + heat * 190.0;
                rgb = (v * 0.42, v * 0.86, v * 1.0);
            }
            t.put(x, y, rgb.0, rgb.1, rgb.2);
        }
    }
    t
}

/// `x.sqrt()` and `x.powi()` come from std; a no_std guest needs its own.
trait FloatExt {
    fn sqrt(self) -> f32;
    fn powi(self, n: i32) -> f32;
}

impl FloatExt for f32 {
    fn sqrt(self) -> f32 {
        crate::mathx::sqrt_approx(self)
    }
    fn powi(self, n: i32) -> f32 {
        let mut r = 1.0;
        for _ in 0..n {
            r *= self;
        }
        r
    }
}

/// Keep the trig imports used; the sky and marble both need them.
#[allow(dead_code)]
fn _uses(x: f32) -> f32 {
    cos_approx(x) + sin_approx(x)
}
