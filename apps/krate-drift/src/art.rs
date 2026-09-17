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

use crate::mathx::{hash2, sin_approx};

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
    // 256 rather than 64. The sampler wraps, so a tile covers a kilometre of
    // road either way -- what the extra resolution buys is that the features
    // in it (a patch, a crack, a worn track) can be BIGGER than the noise,
    // which is what stops a road reading as sandpaper.
    const N: u32 = 256;
    let mut t = blank(N, N);
    for y in 0..N {
        for x in 0..N {
            // Three scales of noise, coarse to fine. One scale alone is
            // static; three is a surface.
            let coarse = hash2(x as i32 / 32, y as i32 / 32);
            let mid = hash2(x as i32 / 8, y as i32 / 8);
            let fine = hash2(x as i32, y as i32);
            // Weighted to the COARSE end. Tarmac at a distance is a tone
            // with a slight mottle, not a field of gravel -- the fine noise
            // is what the eye reads as texture up close and must not dominate
            // the mid-distance, where most of the road on screen is.
            let v = 46.0 + coarse * 12.0 + mid * 7.0 + fine * 5.0;

            // The wheel tracks are drawn in WORLD space by the road mesh's
            // own shading, not baked in here.
            //
            // Baked in, they repeat with the tile: one tile per road width
            // means a pair of tracks every fourteen metres, at whatever angle
            // the road happens to be pointing. Two stripes down a road have to
            // follow the road, and a tile cannot know where the road goes.

            // NO repair patch and no crack here, though both were tried.
            //
            // A distinctive feature in a tile that repeats every road-width
            // becomes a PATTERN: the same patch every few metres reads as
            // wallpaper, which is more obviously wrong than a plain surface.
            // A feature like that has to be painted where it belongs in world
            // space, not baked into a tile. What is left is what genuinely
            // does repeat -- aggregate, and the polish of a wheel track.

            put(&mut t, x, y, v, v, v * 1.05);
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
    // 256, and varied at three scales like the road. A field at speed is
    // mostly peripheral vision, and peripheral vision reads VARIATION -- a
    // single-scale noise field flickers as it scrolls, which is worse than
    // flat.
    const N: u32 = 256;
    let mut t = blank(N, N);
    for y in 0..N {
        for x in 0..N {
            let patch = hash2(x as i32 / 48, y as i32 / 48);
            let clump = hash2(x as i32 / 11, y as i32 / 11);
            let blade = hash2(x as i32, y as i32);
            // Base green, shifted per patch so the field has lighter and
            // darker areas rather than one tone.
            // Graded to the sunset, not left at midday green.
            //
            // The single most out-of-place thing in the frame was vivid green
            // grass under a violet sky. Real grass at this hour is olive and
            // desaturated -- the sun is low and red, so the green it reflects
            // is dulled and warmed. A colour that ignores the light it is
            // under is what makes a scene look assembled rather than lit,
            // whatever else is right.
            let g = 58.0 + patch * 30.0 + clump * 18.0 + blade * 11.0;
            let r = g * (0.72 + patch * 0.14);
            let b = g * 0.42;
            // Dry patches: yellower, where the ground is thin.
            let dry = hash2(x as i32 / 37 + 91, y as i32 / 37 + 13);
            if dry > 0.80 {
                put(&mut t, x, y, g * 1.16, g * 0.94, b * 0.86);
            } else {
                put(&mut t, x, y, r, g, b);
            }
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
    // 256, so a window can have a frame, a sill, a mullion and a reflection
    // instead of being four flat pixels. A building is mostly windows, and
    // whether a city reads as a city is decided by whether its windows have
    // any structure in them at all.
    const N: u32 = 256;
    let mut t = blank(N, N);

    // The wall: concrete with a vertical grain and horizontal floor bands.
    let wall = 88.0 + hash2(seed, 1) * 62.0;
    let warm = 0.92 + hash2(seed, 5) * 0.16;
    for y in 0..N {
        for x in 0..N {
            let grain = hash2(x as i32 + seed, y as i32) * 11.0;
            let streak = hash2(x as i32 / 2 + seed, y as i32 / 64) * 7.0;
            let mut v = wall + grain + streak;
            // A slab edge every floor, catching light on top and shadow
            // under. This is what gives a tower storeys from a distance,
            // when no window is resolvable any more.
            if y % 32 < 2 {
                v += 13.0;
            } else if y % 32 == 2 {
                v -= 16.0;
            }
            put(&mut t, x, y, v * warm, v * 0.99, v * 0.95);
        }
    }

    // Windows: 6 columns of 7, each with a recess, a frame and glass.
    let lit_bias = 0.42 + hash2(seed, 9) * 0.30;
    for row in 0..7u32 {
        for col in 0..6u32 {
            let lit = hash2(seed + col as i32 * 7, row as i32 * 13) > lit_bias;
            let x0 = 12 + col * 40;
            let y0 = 10 + row * 32;
            let (w, h) = (26u32, 20u32);

            for yy in y0..(y0 + h).min(N) {
                for xx in x0..(x0 + w).min(N) {
                    let ix = xx - x0;
                    let iy = yy - y0;
                    // Recess shadow on the top and left inside edge.
                    if ix < 2 || iy < 2 {
                        put(&mut t, xx, yy, 34.0, 36.0, 42.0);
                        continue;
                    }
                    // Frame.
                    if ix >= w - 2 || iy >= h - 2 {
                        put(&mut t, xx, yy, 168.0, 168.0, 164.0);
                        continue;
                    }
                    // Mullion down the middle: two panes, not one hole.
                    if ix == w / 2 || ix == w / 2 + 1 {
                        put(&mut t, xx, yy, 150.0, 150.0, 148.0);
                        continue;
                    }
                    if lit {
                        // A lit room: warm, brighter near the top where the
                        // ceiling light is, with some variation per window.
                        let k = 1.0 - (iy as f32 / h as f32) * 0.35;
                        let tint = hash2(seed + col as i32, row as i32) * 24.0;
                        put(
                            &mut t,
                            xx,
                            yy,
                            (228.0 + tint) * k,
                            (206.0 + tint * 0.7) * k,
                            (146.0 + tint * 0.4) * k,
                        );
                    } else {
                        // Dark glass reflecting sky: darker low, bluer high,
                        // with a diagonal highlight. Flat dark rectangles are
                        // what made the first version read as holes.
                        let sky = 1.0 - (iy as f32 / h as f32);
                        let diag = if (ix as i32 - iy as i32).rem_euclid(14) < 3 {
                            16.0
                        } else {
                            0.0
                        };
                        put(
                            &mut t,
                            xx,
                            yy,
                            30.0 + sky * 26.0 + diag,
                            38.0 + sky * 34.0 + diag,
                            52.0 + sky * 48.0 + diag,
                        );
                    }
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

/// The sky, as a vertical gradient with a sun glow in it.
///
/// This is the texture that decides what the whole scene looks like, because
/// the sky is the light source everything else is graded against. A flat blue
/// fill made every other colour in the frame arbitrary -- pure green grass,
/// grey road, primary-coloured cars, none of them belonging to the same
/// place.
///
/// A sunset was chosen over midday for a plain reason: at midday everything is
/// lit from directly above and the world is flat and shadowless, which is the
/// hardest possible look to make convincing. A low sun gives long shadows,
/// warm light on one side of everything and cool shade on the other, and a
/// horizon with colour in it. It is the cheapest way to make a scene look
/// deliberately lit rather than merely visible.
///
/// `v` runs 0 at the horizon to 1 overhead. `u` runs around the compass, so
/// the sun can sit at one bearing rather than glowing all the way round.
pub fn sky(sun_u: f32) -> Texture {
    const W: u32 = 256;
    const H: u32 = 128;
    let mut t = blank(W, H);

    // The gradient stops, horizon to zenith. Warm at the bottom through
    // orange and pink into a deep blue overhead -- the ordinary progression
    // of a clear evening, which is what makes it read as real rather than as
    // a colour effect.
    const STOPS: [[f32; 4]; 6] = [
        // v,    r,     g,     b
        [0.00, 252.0, 186.0, 120.0],
        [0.10, 246.0, 146.0, 104.0],
        [0.24, 226.0, 108.0, 116.0],
        [0.45, 158.0, 92.0, 148.0],
        [0.70, 86.0, 78.0, 152.0],
        [1.00, 38.0, 46.0, 104.0],
    ];

    for y in 0..H {
        let v = y as f32 / (H - 1) as f32;
        // Find the pair of stops this row sits between and mix them.
        let mut c = [STOPS[0][1], STOPS[0][2], STOPS[0][3]];
        for w in STOPS.windows(2) {
            let (a, b) = (w[0], w[1]);
            if v >= a[0] && v <= b[0] {
                let k = if b[0] > a[0] {
                    (v - a[0]) / (b[0] - a[0])
                } else {
                    0.0
                };
                // Smoothstep rather than linear: a linear ramp shows its
                // stops as visible bands where the slope changes.
                let k = k * k * (3.0 - 2.0 * k);
                c = [
                    a[1] + (b[1] - a[1]) * k,
                    a[2] + (b[2] - a[2]) * k,
                    a[3] + (b[3] - a[3]) * k,
                ];
                break;
            }
        }
        if v > STOPS[STOPS.len() - 1][0] {
            let last = STOPS[STOPS.len() - 1];
            c = [last[1], last[2], last[3]];
        }

        for x in 0..W {
            let u = x as f32 / (W - 1) as f32;
            // The sun's glow: brightest at its bearing and near the horizon,
            // falling away in both directions. Wrapped, so a sun at u=0.98
            // still glows across the seam at u=0.02.
            let mut du = u - sun_u;
            if du > 0.5 {
                du -= 1.0;
            } else if du < -0.5 {
                du += 1.0;
            }
            let across = 1.0 - (du.abs() / 0.30).min(1.0);
            let up = 1.0 - (v / 0.34).min(1.0);
            let glow = across * across * up * up;

            // Thin banded cloud near the horizon, lit from beneath by the
            // sun. Bands rather than noise: an evening sky stratifies, and
            // horizontal streaks are what says "sky" rather than "static".
            let band = sin_approx(v * 46.0 + u * 3.0) * 0.5 + 0.5;
            let cloud = if v < 0.42 {
                let k = (1.0 - v / 0.42) * band * band;
                k * (0.20 + glow * 0.55)
            } else {
                0.0
            };

            let r = c[0] + glow * 96.0 + cloud * 70.0;
            let g = c[1] + glow * 74.0 + cloud * 44.0;
            let b = c[2] + glow * 28.0 + cloud * 30.0;
            put(&mut t, x, y, r, g, b);
        }
    }
    t
}
