// Nova 2 procedural art generator.
//
// Standalone std host binary. Writes game-ready sprite/background art as:
//   * <name>.png   -- for humans to preview
//   * <name>.rgba  -- raw asset the wasm guest bundles and reads with zero decoding.
//                     8-byte LE header (u32 width, u32 height) then width*height*4 straight-RGBA bytes.
//
// Everything is generated from math: value-noise / fBm for the nebula, and
// polygon/analytic hull silhouettes shaded by a fake surface normal for metallic ships.
//
// Run: cd apps/krate-nova2/art-gen && cargo run --release

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

// ----------------------------------------------------------------------------
// Image buffer
// ----------------------------------------------------------------------------

struct Image {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>, // linear-ish RGBA, 0..1, straight (non-premultiplied)
}

impl Image {
    fn new(w: usize, h: usize) -> Self {
        Image { w, h, px: vec![[0.0, 0.0, 0.0, 0.0]; w * h] }
    }

    #[inline]
    fn idx(&self, x: usize, y: usize) -> usize {
        y * self.w + x
    }

    #[inline]
    fn set(&mut self, x: usize, y: usize, c: [f32; 4]) {
        let i = self.idx(x, y);
        self.px[i] = c;
    }

    #[inline]
    fn get(&self, x: usize, y: usize) -> [f32; 4] {
        self.px[self.idx(x, y)]
    }

    // Alpha-composite src (straight rgba) over the existing pixel.
    #[inline]
    fn over(&mut self, x: usize, y: usize, src: [f32; 4]) {
        if x >= self.w || y >= self.h {
            return;
        }
        let dst = self.get(x, y);
        let sa = src[3];
        let da = dst[3];
        let out_a = sa + da * (1.0 - sa);
        if out_a <= 1e-6 {
            self.set(x, y, [0.0, 0.0, 0.0, 0.0]);
            return;
        }
        let mut out = [0.0f32; 4];
        for c in 0..3 {
            out[c] = (src[c] * sa + dst[c] * da * (1.0 - sa)) / out_a;
        }
        out[3] = out_a;
        self.set(x, y, out);
    }

    // Additive blend, useful for glows/stars over an opaque background.
    #[inline]
    fn add(&mut self, x: usize, y: usize, rgb: [f32; 3], a: f32) {
        if x >= self.w || y >= self.h {
            return;
        }
        let d = self.get(x, y);
        self.set(
            x,
            y,
            [
                (d[0] + rgb[0] * a).min(1.0),
                (d[1] + rgb[1] * a).min(1.0),
                (d[2] + rgb[2] * a).min(1.0),
                (d[3] + a).min(1.0).max(d[3]),
            ],
        );
    }

    fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.w * self.h * 4);
        for p in &self.px {
            for c in 0..4 {
                let v = (p[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                out.push(v);
            }
        }
        out
    }
}

// ----------------------------------------------------------------------------
// Output: PNG + raw .rgba (both from the same RGBA8 buffer)
// ----------------------------------------------------------------------------

fn write_png(path: &Path, w: usize, h: usize, data: &[u8]) {
    let file = File::create(path).unwrap_or_else(|e| panic!("create {:?}: {}", path, e));
    let bw = BufWriter::new(file);
    let mut enc = png::Encoder::new(bw, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Best);
    let mut writer = enc.write_header().unwrap();
    writer.write_image_data(data).unwrap();
}

fn write_rgba(path: &Path, w: usize, h: usize, data: &[u8]) {
    use std::io::Write;
    let file = File::create(path).unwrap_or_else(|e| panic!("create {:?}: {}", path, e));
    let mut bw = BufWriter::new(file);
    bw.write_all(&(w as u32).to_le_bytes()).unwrap();
    bw.write_all(&(h as u32).to_le_bytes()).unwrap();
    bw.write_all(data).unwrap();
}

fn emit(dir: &Path, name: &str, img: &Image) {
    let data = img.to_rgba8();
    let png_path = dir.join(format!("{name}.png"));
    let rgba_path = dir.join(format!("{name}.rgba"));
    write_png(&png_path, img.w, img.h, &data);
    write_rgba(&rgba_path, img.w, img.h, &data);
    let png_bytes = std::fs::metadata(&png_path).map(|m| m.len()).unwrap_or(0);
    let rgba_bytes = std::fs::metadata(&rgba_path).map(|m| m.len()).unwrap_or(0);
    println!(
        "  {name:<14} {w}x{h}   png {png_kb:>6} KB   rgba {rgba_kb:>6} KB",
        w = img.w,
        h = img.h,
        png_kb = png_bytes / 1024,
        rgba_kb = rgba_bytes / 1024,
    );
}

// ----------------------------------------------------------------------------
// Math helpers
// ----------------------------------------------------------------------------

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

// Deterministic hash -> [0,1). Integer lattice hash for value noise.
#[inline]
fn hash2(mut x: i32, mut y: i32, seed: u32) -> f32 {
    x = x.wrapping_mul(374761393);
    y = y.wrapping_mul(668265263);
    let mut h = (x ^ y ^ seed as i32) as u32;
    h = h.wrapping_mul(1274126177);
    h ^= h >> 15;
    h = h.wrapping_mul(2246822519);
    h ^= h >> 13;
    (h & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
}

// Smooth value noise at (x, y) with integer lattice + smoothstep interpolation.
fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let xf = x - xi;
    let yf = y - yi;
    let (ix, iy) = (xi as i32, yi as i32);

    let v00 = hash2(ix, iy, seed);
    let v10 = hash2(ix + 1, iy, seed);
    let v01 = hash2(ix, iy + 1, seed);
    let v11 = hash2(ix + 1, iy + 1, seed);

    let u = xf * xf * (3.0 - 2.0 * xf);
    let v = yf * yf * (3.0 - 2.0 * yf);

    let a = lerp(v00, v10, u);
    let b = lerp(v01, v11, u);
    lerp(a, b, v)
}

// Fractal Brownian motion: sum of octaves at increasing frequency, decreasing amplitude.
fn fbm(mut x: f32, mut y: f32, octaves: u32, seed: u32) -> f32 {
    let mut amp = 0.5;
    let mut sum = 0.0;
    let mut norm = 0.0;
    for o in 0..octaves {
        sum += amp * value_noise(x, y, seed.wrapping_add(o * 1013));
        norm += amp;
        amp *= 0.5;
        // rotate + scale each octave a touch to avoid axis-aligned artifacts
        let (nx, ny) = (x * 2.02 + 5.1, y * 2.02 - 3.7);
        x = nx * 0.9239 - ny * 0.3827;
        y = nx * 0.3827 + ny * 0.9239;
    }
    sum / norm
}

// ----------------------------------------------------------------------------
// Color-ramp helpers
// ----------------------------------------------------------------------------

// Piecewise-linear color ramp over a set of (position, rgb) stops.
fn ramp(stops: &[(f32, [f32; 3])], t: f32) -> [f32; 3] {
    let t = clamp01(t);
    if t <= stops[0].0 {
        return stops[0].1;
    }
    let last = stops.len() - 1;
    if t >= stops[last].0 {
        return stops[last].1;
    }
    for i in 0..last {
        let (p0, c0) = stops[i];
        let (p1, c1) = stops[i + 1];
        if t >= p0 && t <= p1 {
            let k = (t - p0) / (p1 - p0).max(1e-6);
            return [
                lerp(c0[0], c1[0], k),
                lerp(c0[1], c1[1], k),
                lerp(c0[2], c1[2], k),
            ];
        }
    }
    stops[last].1
}

// ----------------------------------------------------------------------------
// NEBULA
// ----------------------------------------------------------------------------

fn gen_nebula(w: usize, h: usize) -> Image {
    let mut img = Image::new(w, h);

    // Two independent nebula fields (teal/cyan and magenta), each masked by its
    // own low-frequency fBm so the colors live in different regions of the frame,
    // like the reference. A dark blue/purple base fills the rest.

    let base_dark = [0.015, 0.018, 0.045]; // near-black blue
    let base_purple = [0.05, 0.03, 0.10]; // faint purple wash

    // Cloud color ramps: dark -> mid -> bright core. Colors are added additively,
    // scaled by density, so the ramp values here are "glow" contributions.
    let teal_ramp = [
        (0.0, [0.01, 0.06, 0.09]),
        (0.40, [0.03, 0.34, 0.42]),
        (0.68, [0.10, 0.66, 0.74]),
        (0.88, [0.35, 0.86, 0.92]),
        (1.0, [0.75, 0.98, 1.00]),
    ];
    let magenta_ramp = [
        (0.0, [0.08, 0.01, 0.09]),
        (0.40, [0.40, 0.06, 0.42]),
        (0.68, [0.74, 0.14, 0.66]),
        (0.88, [0.92, 0.40, 0.86]),
        (1.0, [1.00, 0.75, 0.98]),
    ];
    // a third, warmer blue/indigo band for depth
    let indigo_ramp = [
        (0.0, [0.02, 0.03, 0.10]),
        (0.5, [0.10, 0.14, 0.42]),
        (0.8, [0.24, 0.30, 0.70]),
        (1.0, [0.5, 0.6, 0.95]),
    ];

    let fw = w as f32;
    let fh = h as f32;

    // Precompute a diagonal gradient axis for placing the color regions.
    for y in 0..h {
        for x in 0..w {
            let u = x as f32 / fw;
            let v = y as f32 / fh;

            // Base: subtle vertical purple wash + a very-low-freq blue variation.
            let base_var = fbm(u * 2.0, v * 2.0, 3, 91);
            let mut col = [
                lerp(base_dark[0], base_purple[0], base_var * (0.4 + v * 0.6)),
                lerp(base_dark[1], base_purple[1], base_var * (0.4 + v * 0.6)),
                lerp(base_dark[2], base_purple[2], base_var),
            ];

            // Shared cloud-density fBm at a comfortable scale (big soft billows).
            let nx = u * 2.8;
            let ny = v * 2.8;

            // Region placement uses a soft radial falloff around chosen centers so
            // each color clearly owns a part of the frame, like the reference.
            let blob = |cx: f32, cy: f32, rx: f32, ry: f32| -> f32 {
                let dx = (u - cx) / rx;
                let dy = (v - cy) / ry;
                (1.0 - (dx * dx + dy * dy)).max(0.0) // 1 at center, 0 at radius, clamped
            };

            // --- Teal / cyan cloud (upper-left, the hero color) ---
            let d_teal = fbm(nx + 10.0, ny + 4.0, 6, 21);
            let region_teal =
                blob(0.22, 0.34, 0.52, 0.50) + 0.5 * blob(0.10, 0.70, 0.40, 0.40);
            let mask_teal = smoothstep(0.05, 0.85, region_teal)
                * smoothstep(0.20, 0.60, fbm(u * 1.4 + 1.0, v * 1.4 + 8.0, 2, 55));
            let teal_density = clamp01((d_teal - 0.28) / 0.44) * mask_teal;
            let teal_density = teal_density.powf(0.75) * 1.7;
            if teal_density > 0.001 {
                let c = ramp(&teal_ramp, teal_density.min(1.0));
                for i in 0..3 {
                    col[i] += c[i] * teal_density.min(1.3);
                }
            }

            // --- Magenta cloud (lower-right, the second hero color) ---
            let d_mag = fbm(nx - 6.0, ny - 12.0, 6, 77);
            let region_mag =
                blob(0.78, 0.68, 0.55, 0.52) + 0.5 * blob(0.9, 0.35, 0.4, 0.4);
            let mask_mag = smoothstep(0.05, 0.85, region_mag)
                * smoothstep(0.20, 0.60, fbm(u * 1.3 - 2.0, v * 1.3 + 3.0, 2, 200));
            let mag_density = clamp01((d_mag - 0.28) / 0.44) * mask_mag;
            let mag_density = mag_density.powf(0.75) * 1.6;
            if mag_density > 0.001 {
                let c = ramp(&magenta_ramp, mag_density.min(1.0));
                for i in 0..3 {
                    col[i] += c[i] * mag_density.min(1.25);
                }
            }

            // --- Indigo band (supporting, fills the center diagonal softly) ---
            let d_ind = fbm(nx * 1.2 + 30.0, ny * 1.2 - 5.0, 5, 130);
            let region_ind = blob(0.5, 0.5, 0.6, 0.7);
            let mask_ind = smoothstep(0.05, 0.7, region_ind)
                * smoothstep(0.42, 0.68, fbm(u * 1.3 + 5.0, v * 1.3 - 4.0, 2, 310));
            let ind_density = clamp01((d_ind - 0.40) / 0.42) * mask_ind;
            let ind_density = ind_density.powf(1.1) * 0.5;
            if ind_density > 0.001 {
                let c = ramp(&indigo_ramp, ind_density.min(1.0));
                for i in 0..3 {
                    col[i] += c[i] * ind_density.min(0.55);
                }
            }

            // A faint high-frequency dust texture over everything for grain.
            let dust = fbm(u * 10.0, v * 10.0, 4, 303);
            let dust_amt = (dust - 0.55).max(0.0) * 0.05;
            let overall = (teal_density + mag_density + ind_density).min(1.0);
            for i in 0..3 {
                // dust is brighter where clouds already glow
                col[i] += dust_amt * (0.3 + overall);
            }

            img.set(x, y, [col[0].min(1.0), col[1].min(1.0), col[2].min(1.0), 1.0]);
        }
    }

    // --- Stars: additive dots of varied brightness, a few with cross glints. ---
    let mut rng = 0x1234_5678u32;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        (rng & 0x00FF_FFFF) as f32 / 0x0100_0000 as f32
    };

    let star_count = (w * h) / 480; // density
    for _ in 0..star_count {
        let sx = next() * fw;
        let sy = next() * fh;
        let r = next();
        // brightness distribution: mostly dim, a few bright
        let bright = if r > 0.985 {
            0.9 + next() * 0.1
        } else if r > 0.9 {
            0.5 + next() * 0.4
        } else {
            0.12 + next() * 0.3
        };
        // slight color temperature variation
        let temp = next();
        let tint = if temp > 0.7 {
            [1.0, 0.92, 0.82] // warm
        } else if temp < 0.25 {
            [0.82, 0.9, 1.0] // cool blue
        } else {
            [1.0, 1.0, 1.0]
        };
        let radius = if bright > 0.85 { 1.6 } else { 1.0 };
        splat_star(&mut img, sx, sy, radius, bright, tint);

        // bright stars get a subtle 4-point glint
        if bright > 0.9 {
            let glint = bright * 0.5;
            for d in 1..5 {
                let f = 1.0 - d as f32 / 5.0;
                img.add(sx as usize + d, sy as usize, tint, glint * f * 0.4);
                img.add(sx as usize - d, sy as usize, tint, glint * f * 0.4);
                img.add(sx as usize, sy as usize + d, tint, glint * f * 0.4);
                img.add(sx as usize, sy as usize - d, tint, glint * f * 0.4);
            }
        }
    }

    img
}

fn splat_star(img: &mut Image, cx: f32, cy: f32, radius: f32, bright: f32, tint: [f32; 3]) {
    let r = radius.max(0.6);
    let x0 = (cx - r * 2.5).floor() as i32;
    let x1 = (cx + r * 2.5).ceil() as i32;
    let y0 = (cy - r * 2.5).floor() as i32;
    let y1 = (cy + r * 2.5).ceil() as i32;
    for yy in y0..=y1 {
        for xx in x0..=x1 {
            if xx < 0 || yy < 0 || xx as usize >= img.w || yy as usize >= img.h {
                continue;
            }
            let dx = xx as f32 + 0.5 - cx;
            let dy = yy as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            // gaussian-ish falloff
            let a = (-(dist * dist) / (2.0 * r * r)).exp() * bright;
            if a > 0.004 {
                img.add(xx as usize, yy as usize, tint, a);
            }
        }
    }
}

// ----------------------------------------------------------------------------
// SHIP SHADING
// ----------------------------------------------------------------------------

// A hull is defined by a half-width profile: for a given normalized y in [0,1]
// (0 = nose at top, 1 = tail at bottom), return the half-width fraction (0..1)
// of the hull at that row. We build a metallic look by:
//   1. computing signed distance to the hull edge (for a coverage/AA mask),
//   2. faking a rounded cross-section normal (nx from horizontal position within
//      the hull, ny/nz from a dome), then Lambert + specular from an upper-left light.

struct ShipParams {
    // base body color (metal) and a secondary accent
    body: [f32; 3],
    accent: [f32; 3],
    cockpit: [f32; 3],
    engine_glow: [f32; 3],
}

// Evaluate half-width fraction of a hull profile.
// `pts` are (y, halfwidth) control points, linearly interpolated, y ascending.
fn profile(pts: &[(f32, f32)], y: f32) -> f32 {
    if y <= pts[0].0 {
        return pts[0].1;
    }
    let last = pts.len() - 1;
    if y >= pts[last].0 {
        return pts[last].1;
    }
    for i in 0..last {
        let (y0, w0) = pts[i];
        let (y1, w1) = pts[i + 1];
        if y >= y0 && y <= y1 {
            let k = (y - y0) / (y1 - y0).max(1e-6);
            // smooth the interpolation slightly for organic curves
            let ks = k * k * (3.0 - 2.0 * k);
            return lerp(w0, w1, ks);
        }
    }
    pts[last].1
}

// Render a ship given a hull profile and params. size is the square canvas.
// The light comes from upper-left. Nose points up.
fn render_ship(
    size: usize,
    hull: &[(f32, f32)],
    body_max_halfwidth: f32, // fraction of half-canvas the widest part occupies
    params: &ShipParams,
    // optional wing/fin polygons: list of (x,y) in normalized [-1,1]x[0,1] hull space
    fins: &[Vec<(f32, f32)>],
    // panel line rows (normalized y) drawn as subtle darker lines
    panel_rows: &[f32],
    seed: u32,
) -> Image {
    let mut img = Image::new(size, size);
    let fs = size as f32;
    let cx = fs / 2.0;
    // vertical margins
    let top = fs * 0.06;
    let bot = fs * 0.94;
    let span = bot - top;

    // supersample for clean edges
    let ss = 3;
    let light = normalize3([-0.55, -0.5, 0.66]); // upper-left, toward viewer

    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let fx = px as f32 + (sx as f32 + 0.5) / ss as f32;
                    let fy = py as f32 + (sy as f32 + 0.5) / ss as f32;
                    let c = shade_ship_sample(
                        fx, fy, cx, top, span, fs, hull, body_max_halfwidth, params, fins,
                        panel_rows, light, seed,
                    );
                    for i in 0..4 {
                        acc[i] += c[i];
                    }
                }
            }
            let n = (ss * ss) as f32;
            img.set(px, py, [acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n]);
        }
    }
    img
}

#[inline]
fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / m, v[1] / m, v[2] / m]
}

#[inline]
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

// Point-in-polygon for fins (normalized hull space).
fn point_in_poly(poly: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi + 1e-9) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn shade_ship_sample(
    fx: f32,
    fy: f32,
    cx: f32,
    top: f32,
    span: f32,
    fs: f32,
    hull: &[(f32, f32)],
    body_max_halfwidth: f32,
    params: &ShipParams,
    fins: &[Vec<(f32, f32)>],
    panel_rows: &[f32],
    light: [f32; 3],
    seed: u32,
) -> [f32; 4] {
    // normalized hull coords: ny in [0,1] top->bottom, nx in [-1,1] across
    let ny = (fy - top) / span;
    let max_halfwidth_px = body_max_halfwidth * (fs / 2.0);

    if ny < -0.02 || ny > 1.02 {
        // still allow fins slightly outside? fins are within [0,1] so skip.
    }

    // hull coverage
    let hw = if ny >= 0.0 && ny <= 1.0 {
        profile(hull, ny) * max_halfwidth_px
    } else {
        0.0
    };
    let dx = fx - cx;
    let nx = if hw > 1e-3 { dx / hw } else { 10.0 }; // -1..1 inside hull

    let mut in_body = ny >= 0.0 && ny <= 1.0 && nx.abs() <= 1.0;

    // Check fins (accent wings) in normalized space nx in [-1,1] using canvas-relative x.
    // fins defined in normalized space where x is fraction of half-canvas, y is ny.
    let fin_nx = dx / (fs / 2.0);
    let mut in_fin = false;
    for poly in fins {
        if point_in_poly(poly, fin_nx, ny) {
            in_fin = true;
            break;
        }
    }

    if !in_body && !in_fin {
        return [0.0, 0.0, 0.0, 0.0];
    }

    // --- Fake surface normal ---
    // For the body: rounded cross-section dome (nz peaks at center-spine).
    // For a wing that is NOT over the body: treat it as a flat-ish angled panel
    // that tilts outward and slightly up, so it catches the light as a distinct
    // metal surface rather than sharing the body's dome.
    let long_t = (ny - 0.5) * 2.0; // -1..1
    let long_dome = (1.0 - 0.25 * long_t * long_t).max(0.3);

    let (mut normal, dome) = if in_body {
        let nx_c = nx.clamp(-1.0, 1.0);
        let d = (1.0 - nx_c * nx_c).max(0.0).sqrt(); // 1 at center, 0 at edges
        (normalize3([nx_c * 0.85, long_t * 0.28, d * long_dome + 0.15]), d)
    } else {
        // wing panel: normal tilts toward its outboard edge (sign of fin_nx) and
        // gently up-and-back, giving a beveled metal-plate look with a thin bright edge.
        let side = if fin_nx >= 0.0 { 1.0 } else { -1.0 };
        // local across-wing coordinate for a subtle chamfer highlight
        let d = 0.55; // moderate dome so wings aren't dead flat
        (normalize3([side * 0.45, -0.15, 0.82]), d)
    };

    // Surface micro-detail: faint noise perturbation for a brushed-metal feel.
    let n_pert = (value_noise(fx * 0.35, fy * 0.35, seed) - 0.5) * 0.12;
    normal = normalize3([normal[0] + n_pert, normal[1] - n_pert * 0.5, normal[2]]);

    // --- Lighting ---
    let ndl = dot3(normal, light).max(0.0);
    let ambient = 0.28;
    let diffuse = ndl;

    // specular (Blinn-ish): view is straight-on [0,0,1]
    let view = [0.0, 0.0, 1.0];
    let half = normalize3([light[0] + view[0], light[1] + view[1], light[2] + view[2]]);
    let spec = dot3(normal, half).max(0.0).powf(24.0);

    // rim light on the upper-left edge for a metallic pop
    let edge = 1.0 - dome; // near 1 at hull edges
    let rim = smoothstep(0.72, 1.0, edge) * (dot3(normal, light).max(0.0)) * 0.8;

    // base albedo: body or accent (fins)
    let mut albedo = if in_fin && !in_body {
        params.accent
    } else if in_fin && in_body {
        // where fin overlaps body, blend
        [
            (params.body[0] + params.accent[0]) * 0.5,
            (params.body[1] + params.accent[1]) * 0.5,
            (params.body[2] + params.accent[2]) * 0.5,
        ]
    } else {
        params.body
    };

    // longitudinal accent stripe down the spine (a raised painted strip, not a groove)
    if in_body && nx.abs() < 0.13 {
        let s = 1.0 - smoothstep(0.0, 0.13, nx.abs());
        for i in 0..3 {
            // brighten toward a lighter tint of the accent so it reads as a raised ridge
            let bright_accent = [
                (params.accent[0] * 0.6 + 0.35).min(1.0),
                (params.accent[1] * 0.6 + 0.35).min(1.0),
                (params.accent[2] * 0.6 + 0.35).min(1.0),
            ];
            albedo[i] = lerp(albedo[i], bright_accent[i], s * 0.45);
        }
    }

    // panel lines: subtle darkening at specified rows and a couple longitudinal seams
    let mut panel_dark = 0.0f32;
    for &pr in panel_rows {
        let d = (ny - pr).abs();
        if d < 0.012 {
            panel_dark += (1.0 - d / 0.012) * 0.5;
        }
    }
    // longitudinal seams near |nx| ~ 0.5
    if in_body {
        let seam = ((nx.abs() - 0.52).abs()).min((nx.abs() - 0.0).abs());
        let _ = seam;
        let d = (nx.abs() - 0.5).abs();
        if d < 0.03 {
            panel_dark += (1.0 - d / 0.03) * 0.35;
        }
    }
    panel_dark = panel_dark.min(0.8);

    let is_wing = in_fin && !in_body;

    // Wings: darken toward their outboard trailing edge and tone down the
    // specular so they read as solid angled metal plates, not glowing glass.
    let (spec_k, rim_k, wing_shade) = if is_wing {
        // shade from a bit of noise + a gradient so the plate isn't a flat fill
        let plate = 0.72 + 0.18 * (value_noise(fx * 0.25, fy * 0.25, seed ^ 0x55) - 0.5) * 2.0;
        (0.35f32, 0.0f32, plate.clamp(0.5, 1.0))
    } else {
        (0.9f32, 0.9f32, 1.0f32)
    };

    let mut rgb = [0.0f32; 3];
    for i in 0..3 {
        let lit = albedo[i] * (ambient + diffuse * 0.95);
        rgb[i] = (lit + spec * spec_k + rim * rim_k) * wing_shade;
        rgb[i] *= 1.0 - panel_dark;
    }

    // --- Cockpit: a small dark glassy teardrop near the front, on the spine ---
    if in_body {
        let cy = 0.28; // position down the hull (toward the nose)
        // ellipse radii in hull-normalized space; kept small so it doesn't dominate wide hulls
        let rxe = 0.24;
        let rye = 0.11;
        let ex = nx / rxe;
        let ey = (ny - cy) / rye;
        let cd = ex * ex + ey * ey;
        if cd < 1.0 {
            let glass = params.cockpit;
            // glossy highlight toward the upper-left of the canopy
            let hl = smoothstep(0.55, 0.0, (ex + 0.45).powi(2) + (ey + 0.5).powi(2)) * 0.9;
            // faint reflection band lower-right
            let refl = smoothstep(0.4, 0.0, (ex - 0.3).powi(2) + (ey - 0.4).powi(2)) * 0.25;
            let cov = smoothstep(1.0, 0.72, cd);
            for i in 0..3 {
                let g = (glass[i] + hl + refl * 0.6).min(1.0);
                rgb[i] = lerp(rgb[i], g, cov);
            }
        }
    }

    // --- Engine glow at the tail ---
    if in_body {
        let gd = ((ny - 0.98).max(0.0)) ; // near tail
        let _ = gd;
        let tail = smoothstep(0.82, 1.0, ny);
        if tail > 0.0 {
            // a couple of engine nozzles
            let nozzle = (0.5 - (nx.abs() - 0.35).abs() * 4.0).max(0.0);
            let glow = tail * (0.4 + nozzle);
            for i in 0..3 {
                rgb[i] += params.engine_glow[i] * glow * 1.4;
            }
        }
    }

    // ambient occlusion toward edges to seat the form
    let ao = lerp(1.0, 0.82, smoothstep(0.6, 1.0, edge));
    for i in 0..3 {
        rgb[i] *= ao;
        rgb[i] = rgb[i].clamp(0.0, 1.0);
    }

    // coverage / anti-alias alpha: distance to hull edge.
    // approximate alpha from how far nx.abs() or fin coverage is inside.
    let alpha = if in_body {
        // soft edge over ~1px
        let edge_soft = smoothstep(1.0, 0.94, nx.abs());
        edge_soft.max(if in_fin { 1.0 } else { 0.0 })
    } else {
        1.0 // fin fully covered (fin AA handled by supersampling boolean)
    };

    [rgb[0], rgb[1], rgb[2], alpha]
}

// ----------------------------------------------------------------------------
// PROJECTILE
// ----------------------------------------------------------------------------

fn gen_projectile(w: usize, h: usize, core: [f32; 3], glow: [f32; 3]) -> Image {
    let mut img = Image::new(w, h);
    let cx = w as f32 / 2.0;
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            // vertical bolt: capsule shape, bright core, soft glow
            let dx = (fx - cx).abs();
            // taper the ends
            let ny = y as f32 / h as f32;
            let taper = smoothstep(0.0, 0.12, ny) * smoothstep(1.0, 0.82, ny);
            let core_w = 2.6 * taper;
            let glow_w = 8.0 * taper;

            let core_a = smoothstep(core_w, 0.0, dx) * taper;
            let glow_a = smoothstep(glow_w, 0.0, dx) * taper * 0.6;

            let mut rgb = [0.0f32; 3];
            let mut a = 0.0f32;
            for i in 0..3 {
                rgb[i] = glow[i] * glow_a + core[i] * core_a;
            }
            a = (glow_a + core_a).min(1.0);
            // white-hot center
            for i in 0..3 {
                rgb[i] = (rgb[i] + core_a * 0.6).min(1.0);
            }
            img.set(x, y, [rgb[0], rgb[1], rgb[2], a]);
        }
    }
    img
}

// ----------------------------------------------------------------------------
// ASTEROID
// ----------------------------------------------------------------------------

fn gen_asteroid(size: usize, seed: u32) -> Image {
    let mut img = Image::new(size, size);
    let fs = size as f32;
    let cx = fs / 2.0;
    let cy = fs / 2.0;
    let base_r = fs * 0.40;
    let ss = 3;
    let light = normalize3([-0.55, -0.5, 0.62]);

    // lumpy radius function via angular fbm
    let lump = |ang: f32| -> f32 {
        let x = ang.cos() * 1.6 + 3.0;
        let y = ang.sin() * 1.6 + 3.0;
        0.72 + fbm(x, y, 4, seed) * 0.5
    };

    // a few craters
    let mut rng = seed | 1;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        (rng & 0xFFFF) as f32 / 65535.0
    };
    let mut craters = Vec::new();
    for _ in 0..6 {
        let ca = next() * std::f32::consts::TAU;
        let cr = next() * 0.55;
        let crad = 0.10 + next() * 0.16;
        craters.push((cr * ca.cos(), cr * ca.sin(), crad));
    }

    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f32; 4];
            for sdy in 0..ss {
                for sdx in 0..ss {
                    let fx = px as f32 + (sdx as f32 + 0.5) / ss as f32;
                    let fy = py as f32 + (sdy as f32 + 0.5) / ss as f32;
                    let dx = fx - cx;
                    let dy = fy - cy;
                    let dist = (dx * dx + dy * dy).sqrt();
                    let ang = dy.atan2(dx);
                    let r = base_r * lump(ang);
                    if dist > r {
                        continue;
                    }
                    // spherical-ish normal
                    let un = dx / r;
                    let vn = dy / r;
                    let k = (1.0 - un * un - vn * vn).max(0.0).sqrt();
                    let mut normal = normalize3([un, vn, k + 0.2]);

                    // rocky surface perturbation
                    let rough = (fbm(fx * 0.08, fy * 0.08, 5, seed ^ 0x9e37) - 0.5) * 0.9;
                    normal = normalize3([normal[0] + rough, normal[1] - rough * 0.7, normal[2]]);

                    // craters: darken center, bright far-rim toward light
                    let mut crater_shade = 1.0f32;
                    let nxp = dx / base_r;
                    let nyp = dy / base_r;
                    for &(ccx, ccy, crad) in &craters {
                        let ddx = nxp - ccx;
                        let ddy = nyp - ccy;
                        let dd = (ddx * ddx + ddy * ddy).sqrt();
                        if dd < crad {
                            let t = dd / crad;
                            // depression: darker in the light-facing side, subtle rim highlight
                            crater_shade *= lerp(0.55, 1.0, smoothstep(0.6, 1.0, t));
                        }
                    }

                    let ndl = dot3(normal, light).max(0.0);
                    let ambient = 0.22;
                    let base_grey = 0.42 + (fbm(fx * 0.05, fy * 0.05, 3, seed) - 0.5) * 0.14;
                    // slight brown/warm tint
                    let albedo = [base_grey * 1.02, base_grey * 0.97, base_grey * 0.9];
                    let mut rgb = [0.0f32; 3];
                    for i in 0..3 {
                        rgb[i] = (albedo[i] * (ambient + ndl * 0.95) * crater_shade).clamp(0.0, 1.0);
                    }
                    // edge AO
                    let edge = smoothstep(0.86, 1.0, dist / r);
                    for i in 0..3 {
                        rgb[i] *= lerp(1.0, 0.7, edge);
                    }
                    let alpha = smoothstep(1.0, 0.94, dist / r);
                    acc[0] += rgb[0];
                    acc[1] += rgb[1];
                    acc[2] += rgb[2];
                    acc[3] += alpha;
                }
            }
            let n = (ss * ss) as f32;
            img.set(px, py, [acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n]);
        }
    }
    img
}

// ----------------------------------------------------------------------------
// main
// ----------------------------------------------------------------------------

fn main() {
    let dir = Path::new("../assets");
    std::fs::create_dir_all(dir).unwrap();

    println!("Nova2 art-gen -> {:?}", dir.canonicalize().unwrap_or(dir.to_path_buf()));

    // 1. Nebula
    let neb = gen_nebula(960, 720);
    emit(dir, "nebula", &neb);

    // 2. Player ship: sleek arrow/fighter, cyan/steel.
    let player_hull = [
        (0.00, 0.06), // sharp nose
        (0.14, 0.20),
        (0.34, 0.40),
        (0.52, 0.52),
        (0.70, 0.62), // widest near wings
        (0.86, 0.40),
        (1.00, 0.30), // tail with engines
    ];
    let player_fins = vec![
        // left wing
        vec![(-0.30, 0.52), (-0.92, 0.72), (-0.86, 0.86), (-0.20, 0.72)],
        // right wing (mirror)
        vec![(0.30, 0.52), (0.92, 0.72), (0.86, 0.86), (0.20, 0.72)],
    ];
    let player = render_ship(
        128,
        &player_hull,
        0.72,
        &ShipParams {
            body: [0.52, 0.58, 0.66],   // steel
            accent: [0.16, 0.6, 0.72],  // cyan
            cockpit: [0.05, 0.12, 0.2], // dark blue glass
            engine_glow: [0.3, 0.8, 1.0],
        },
        &player_fins,
        &[0.24, 0.46, 0.66, 0.82],
        1,
    );
    emit(dir, "ship_player", &player);

    // 3a. Enemy A: aggressive red interceptor, swept-back forward wings, narrow.
    let enemy_a_hull = [
        (0.00, 0.10),
        (0.18, 0.30),
        (0.40, 0.34),
        (0.60, 0.46),
        (0.78, 0.66),
        (0.90, 0.44),
        (1.00, 0.34),
    ];
    let enemy_a_fins = vec![
        // forward-swept aggressive wings
        vec![(-0.28, 0.34), (-0.95, 0.30), (-0.9, 0.5), (-0.22, 0.6)],
        vec![(0.28, 0.34), (0.95, 0.30), (0.9, 0.5), (0.22, 0.6)],
    ];
    let enemy_a = render_ship(
        112,
        &enemy_a_hull,
        0.74,
        &ShipParams {
            body: [0.5, 0.14, 0.14],   // dark red metal
            accent: [0.9, 0.25, 0.18], // bright red
            cockpit: [0.2, 0.02, 0.02],
            engine_glow: [1.0, 0.4, 0.2],
        },
        &enemy_a_fins,
        &[0.3, 0.55, 0.78],
        7,
    );
    emit(dir, "ship_enemy_a", &enemy_a);

    // 3b. Enemy B: bulky orange gunship, wide blocky hull, stubby wings.
    let enemy_b_hull = [
        (0.00, 0.34), // blunt nose
        (0.16, 0.5),
        (0.36, 0.72),
        (0.58, 0.82), // very wide body
        (0.76, 0.78),
        (0.90, 0.6),
        (1.00, 0.5),
    ];
    let enemy_b_fins = vec![
        // stubby side pods
        vec![(-0.62, 0.36), (-0.98, 0.44), (-0.98, 0.72), (-0.6, 0.7)],
        vec![(0.62, 0.36), (0.98, 0.44), (0.98, 0.72), (0.6, 0.7)],
    ];
    let enemy_b = render_ship(
        112,
        &enemy_b_hull,
        0.78,
        &ShipParams {
            body: [0.55, 0.34, 0.12],   // dark orange/brown metal
            accent: [0.95, 0.6, 0.12],  // bright orange
            cockpit: [0.15, 0.08, 0.02],
            engine_glow: [1.0, 0.6, 0.15],
        },
        &enemy_b_fins,
        &[0.22, 0.42, 0.62, 0.82],
        13,
    );
    emit(dir, "ship_enemy_b", &enemy_b);

    // 4. Projectile: cyan bolt (matches player energy weapon)
    let proj = gen_projectile(24, 48, [0.7, 0.95, 1.0], [0.1, 0.6, 1.0]);
    emit(dir, "projectile", &proj);

    // 5. Asteroid
    let ast = gen_asteroid(96, 424242);
    emit(dir, "asteroid", &ast);

    println!("done.");
}
