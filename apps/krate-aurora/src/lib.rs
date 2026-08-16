//! Krate Aurora — a living northern-lights scene.
//!
//! The aurora itself is not a gradient or a sprite: it is computed per pixel,
//! every frame, into an RGBA buffer that is handed to the canvas in one call.
//! Three curtains of light, each a flowing sine field with its own speed and
//! hue, are summed additively the way real light adds — so where two curtains
//! cross, the colour goes brighter and whiter rather than muddier. That single
//! decision is what separates this from a picture of an aurora.
//!
//! Around it, drawn with the vector primitives: a star field that twinkles on
//! its own clock, two mountain ridges in silhouette, and a lake that reflects
//! the sky above it, mirrored and dimmed and disturbed by a slow ripple.
//!
//! Everything is time-based, nothing is random per frame, and the whole scene
//! is drawn in a fixed design space that the host scales to any window.

#![no_std]

extern crate krate as _krate_runtime;
extern crate alloc;

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

/// The design space. The host scales these numbers to whatever window the
/// person opens, centred and never stretched out of proportion.
const WIDTH: f32 = 900.0;
const HEIGHT: f32 = 600.0;

/// Where the water starts. Sky above, reflection below. The ridges are drawn
/// upwards from this line, so it is the shoreline as well as the waterline --
/// land and water meet here with no gap between them.
const HORIZON: f32 = 330.0;

/// The aurora buffer is computed at a lower resolution than the screen and
/// scaled up when drawn. Light has no hard edges, so the softness costs
/// nothing visually and buys a large amount of headroom per frame.
const SKY_W: u32 = 300;
const SKY_H: u32 = 150;

/// The band of the sky buffer that actually carries the curtains. Only this
/// part is worth mirroring into the lake.
const BAND: u32 = SKY_H * 2 / 3;

/// Frames the `quick` run draws before reporting. A handful is enough to
/// prove the scene animates; the headless check has a fuel budget, not a
/// wall clock, so drawing more only risks exhausting it.
const QUICK_FRAMES: u32 = 6;

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> gfx::Rect {
    gfx::Rect { x, y, width, height }
}

fn point(x: f32, y: f32) -> gfx::Point {
    gfx::Point { x, y }
}

fn style(weight: u16, spacing: f32) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: spacing,
        family: gfx::FontFamily::Sans,
    }
}

fn radii(all: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: all,
        top_right: all,
        bottom_right: all,
        bottom_left: all,
    }
}

// ---------------------------------------------------------------------------
// Deterministic noise
// ---------------------------------------------------------------------------

/// A cheap hash to a stable 0..1. Used for star placement and twinkle phase,
/// so the sky is identical every launch instead of jittering per frame.
fn hash01(n: u32) -> f32 {
    let mut x = n.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
    x = ((x >> ((x >> 28).wrapping_add(4))) ^ x).wrapping_mul(277_803_737);
    x = (x >> 22) ^ x;
    (x & 0xFFFF) as f32 / 65_535.0
}

/// Smooth 1-D value noise: the shape of the curtains. Interpolating between
/// hashed lattice points with a smoothstep gives a wandering line that never
/// repeats visibly, which a single sine cannot do.
fn noise1(x: f32, seed: u32) -> f32 {
    let i = libm::floorf(x);
    let f = x - i;
    let a = hash01((i as i32 as u32).wrapping_add(seed.wrapping_mul(9973)));
    let b = hash01(((i as i32 + 1) as u32).wrapping_add(seed.wrapping_mul(9973)));
    let u = f * f * (3.0 - 2.0 * f);
    a + (b - a) * u
}

/// Several octaves of the above, which is what makes the edge of a curtain
/// look torn and organic rather than like a wave.
fn fbm(x: f32, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    for octave in 0..4u32 {
        sum += noise1(x * freq, seed.wrapping_add(octave)) * amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    sum
}

// ---------------------------------------------------------------------------
// The aurora
// ---------------------------------------------------------------------------

/// One curtain's description: where it sits, how it moves, what colour it is.
struct Curtain {
    seed: u32,
    /// Height of the curtain's centre line, in buffer rows.
    base: f32,
    /// How far the centre line wanders.
    sway: f32,
    /// Horizontal drift per second.
    drift: f32,
    /// Vertical thickness of the glow.
    thickness: f32,
    /// Horizontal scale of the noise — small is billowy, large is streaky.
    scale: f32,
    rgb: (f32, f32, f32),
    strength: f32,
}

const CURTAINS: [Curtain; 3] = [
    // The deep one at the back: wide, slow, blue-violet.
    Curtain {
        seed: 3,
        base: 74.0,
        sway: 20.0,
        drift: 0.020,
        thickness: 34.0,
        scale: 2.1,
        rgb: (0.35, 0.45, 1.00),
        strength: 0.75,
    },
    // The main green sheet, the one the eye reads as "the aurora".
    Curtain {
        seed: 11,
        base: 58.0,
        sway: 26.0,
        drift: 0.035,
        thickness: 22.0,
        scale: 3.4,
        rgb: (0.20, 1.00, 0.62),
        strength: 1.15,
    },
    // A thin fast ribbon on top, teal going white where it overlaps.
    Curtain {
        seed: 29,
        base: 44.0,
        sway: 30.0,
        drift: 0.062,
        thickness: 12.0,
        scale: 5.2,
        rgb: (0.55, 0.95, 1.00),
        strength: 0.95,
    },
];

/// Render the curtains into `buf` as straight RGBA.
///
/// The buffer is transparent where there is no light, so the starfield and the
/// night gradient painted underneath show through untouched. Light is summed
/// rather than blended, then tone-mapped at the end — that is why crossings go
/// bright instead of dark.
fn render_aurora(buf: &mut [u8], t: f32) {
    // The centre line and the ray banding depend only on the column, not the
    // row. Computing them once per column instead of once per pixel takes the
    // noise work down by a factor of SKY_H -- the difference between a scene
    // that animates freely and one that eats its whole frame budget.
    let mut centres = [[0.0f32; CURTAINS.len()]; SKY_W as usize];
    let mut rays = [[0.0f32; CURTAINS.len()]; SKY_W as usize];
    for col in 0..SKY_W as usize {
        let x = col as f32 / SKY_W as f32;
        for (slot, curtain) in CURTAINS.iter().enumerate() {
            let phase = x * curtain.scale + t * curtain.drift * 6.0;
            let wander = (fbm(phase, curtain.seed) - 0.5) * 2.0;
            if let Some(cell) = centres.get_mut(col).and_then(|c| c.get_mut(slot)) {
                *cell = curtain.base + wander * curtain.sway;
            }
            // Vertical rays: the fine banding real curtains have. Two
            // frequencies multiplied, because one alone reads as a smooth
            // wobble rather than as the ribbed structure of real light.
            let coarse = noise1(
                x * 14.0 + t * curtain.drift * 2.0,
                curtain.seed.wrapping_add(77),
            );
            let fine = noise1(
                x * 47.0 + t * curtain.drift * 4.5,
                curtain.seed.wrapping_add(131),
            );
            let ray = (0.42 + 0.58 * coarse) * (0.60 + 0.40 * fine);
            if let Some(cell) = rays.get_mut(col).and_then(|c| c.get_mut(slot)) {
                *cell = ray;
            }
        }
    }

    for row in 0..SKY_H {
        let y = row as f32;
        // Curtains fade towards the bottom of their travel, the way the real
        // thing thins out as it descends. Constant across the row.
        let depth = 1.0 - (y / SKY_H as f32).min(1.0) * 0.35;

        for col in 0..SKY_W {
            let (mut r, mut g, mut b) = (0.0f32, 0.0f32, 0.0f32);

            for (slot, curtain) in CURTAINS.iter().enumerate() {
                let centre = centres
                    .get(col as usize)
                    .and_then(|c| c.get(slot))
                    .copied()
                    .unwrap_or(curtain.base);

                // Distance from the centre line, as a soft falloff. Skip the
                // exp entirely once we are far enough out to contribute
                // nothing visible -- most pixels are, so this is most of the
                // saving.
                let d = (y - centre) / curtain.thickness;
                // Cull what cannot contribute. The trail reaches further
                // upward than downward, so the window is asymmetric too.
                if d < -3.4 || d > 2.6 {
                    continue;
                }
                let ray = rays
                    .get(col as usize)
                    .and_then(|c| c.get(slot))
                    .copied()
                    .unwrap_or(1.0);
                // Asymmetric falloff: tight above the centre line, long and
                // soft below it. A real curtain has a defined lower edge and
                // trails upward, which a symmetric blob never looks like.
                let spread = if d < 0.0 { 2.6 } else { 0.55 };
                let mut fall = libm::expf(-d * d * spread) * ray;

                // A brighter rim right at the lower edge, where the curtain
                // terminates and the light piles up.
                if d > -0.35 && d < 0.55 {
                    fall += libm::expf(-(d - 0.1) * (d - 0.1) * 26.0) * 0.55 * ray;
                }

                let amount = fall * curtain.strength * depth;

                r += curtain.rgb.0 * amount;
                g += curtain.rgb.1 * amount;
                b += curtain.rgb.2 * amount;
            }

            // Overall alpha from how much light landed here, then a gentle
            // tone-map so bright crossings roll off to white instead of
            // clipping into a flat slab of colour.
            let luma = (r * 0.35 + g * 0.5 + b * 0.15).min(2.4);
            let tone = |v: f32| (v / (1.0 + v * 0.55)).min(1.0);

            // Fade to nothing at every edge of the buffer. Without this the
            // buffer's rectangle is visible as a hard seam against the sky --
            // light has no straight edges, so neither can this.
            let u = col as f32 / SKY_W as f32;
            let v = row as f32 / SKY_H as f32;
            let edge_x = (u / 0.18).min((1.0 - u) / 0.18).min(1.0).max(0.0);
            let edge_y = (v / 0.10).min((1.0 - v) / 0.30).min(1.0).max(0.0);
            let edge = edge_x * edge_x * (3.0 - 2.0 * edge_x) * edge_y;

            let alpha = (luma * 0.85).min(1.0) * edge;

            let index = ((row * SKY_W + col) * 4) as usize;
            if let Some(px) = buf.get_mut(index..index + 4) {
                px[0] = (tone(r) * 255.0) as u8;
                px[1] = (tone(g) * 255.0) as u8;
                px[2] = (tone(b) * 255.0) as u8;
                px[3] = (alpha * 255.0) as u8;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The scene
// ---------------------------------------------------------------------------

/// Night sky behind everything: a vertical gradient from deep space blue down
/// to a faint warm haze at the horizon.
fn draw_sky(canvas: u64) -> Result<(), gfx::GfxError> {
    canvas2d::linear_gradient_stops(
        canvas,
        rect(0.0, 0.0, WIDTH, HORIZON),
        90.0,
        &[
            gfx::GradientStop { offset: 0.0, color: color(0.016, 0.024, 0.075, 1.0) },
            gfx::GradientStop { offset: 0.55, color: color(0.031, 0.055, 0.14, 1.0) },
            gfx::GradientStop { offset: 1.0, color: color(0.055, 0.10, 0.20, 1.0) },
        ],
    )
}

/// A fixed star field that twinkles. Position comes from the hash so the sky
/// is the same every launch; only brightness moves.
fn draw_stars(canvas: u64, t: f32) -> Result<(), gfx::GfxError> {
    for i in 0..150u32 {
        let x = hash01(i.wrapping_mul(3) + 1) * WIDTH;
        let y = hash01(i.wrapping_mul(7) + 2) * (HORIZON - 30.0);

        // Stars thin out towards the horizon, as haze takes them.
        let height_fade = 1.0 - (y / HORIZON) * 0.65;

        let phase = hash01(i.wrapping_mul(11) + 3) * 6.283;
        let twinkle = 0.55 + 0.45 * libm::sinf(t * 1.7 + phase);
        let radius = 0.5 + hash01(i.wrapping_mul(13) + 4) * 1.3;
        let alpha = twinkle * height_fade * 0.9;

        canvas2d::fill_circle(canvas, point(x, y), radius, color(1.0, 0.98, 0.92, alpha))?;
    }
    Ok(())
}

/// A ridge line, drawn as a filled band of vertical slices. Two of these at
/// different darkness and height read instantly as depth.
fn draw_ridge(
    canvas: u64,
    seed: u32,
    height: f32,
    roughness: f32,
    fill: gfx::Color,
    rim: gfx::Color,
) -> Result<(), gfx::GfxError> {
    // 3px slices: fine enough that the silhouette reads as a solid edge.
    let step = 3.0;
    let mut x = 0.0;
    while x < WIDTH {
        let n = fbm(x / WIDTH * roughness, seed);
        let top = HORIZON - height * (0.45 + n * 0.85);
        // Fill only a shallow skirt below the crest, not all the way down to
        // the waterline: a ridge that fills every pixel beneath itself turns
        // the whole middle of the picture into one black slab.
        let base = HORIZON + 4.0;
        canvas2d::fill_rect(canvas, rect(x, top, step + 1.0, base - top), fill)?;
        // A lit crest: the aurora above is a light source, so the ridge line
        // catches it. Without this the near ridge is a black shape against a
        // near-black sky and the whole lower half reads as a void.
        canvas2d::fill_rect(canvas, rect(x, top, step + 1.0, 1.6), rim)?;
        x += step;
    }
    Ok(())
}

/// The lake: the aurora buffer drawn again upside down and dimmed, then
/// banded with horizontal ripples so it reads as water rather than a mirror.
fn draw_water(canvas: u64, sky: &[u8], t: f32) -> Result<(), gfx::GfxError> {
    let water = rect(0.0, HORIZON, WIDTH, HEIGHT - HORIZON);

    // The water body itself, darker than the sky it reflects.
    canvas2d::linear_gradient_stops(
        canvas,
        water,
        90.0,
        &[
            gfx::GradientStop { offset: 0.0, color: color(0.035, 0.075, 0.13, 1.0) },
            gfx::GradientStop { offset: 1.0, color: color(0.010, 0.020, 0.045, 1.0) },
        ],
    )?;

    // The reflection. Only the top of the sky buffer carries the curtains, so
    // mirroring the whole thing yields a faint smudge floating in the middle
    // of the lake. Take that band alone and flip it, so the brightest light
    // lands hard against the waterline where a real reflection starts.
    let mut mirrored = vec![0u8; (SKY_W * BAND * 4) as usize];
    for row in 0..BAND {
        let source_row = BAND - 1 - row;
        // Fade out with depth, and lose the colour faster than the light --
        // water keeps brightness longer than hue.
        let fade = 1.0 - (row as f32 / BAND as f32) * 0.80;
        for col in 0..SKY_W {
            let from = ((source_row * SKY_W + col) * 4) as usize;
            let to = ((row * SKY_W + col) * 4) as usize;
            let (Some(src), Some(dst)) = (
                sky.get(from..from + 4).map(|s| [s[0], s[1], s[2], s[3]]),
                mirrored.get_mut(to..to + 4),
            ) else {
                continue;
            };
            dst[0] = (src[0] as f32 * 0.75) as u8;
            dst[1] = (src[1] as f32 * 0.80) as u8;
            dst[2] = src[2];
            dst[3] = (src[3] as f32 * 0.55 * fade) as u8;
        }
    }
    // The reflection starts exactly at the waterline and is compressed
    // vertically, the way a reflection foreshortens as it recedes.
    canvas2d::draw_pixels(
        canvas,
        rect(0.0, HORIZON, WIDTH, (HEIGHT - HORIZON) * 0.78),
        SKY_W,
        BAND,
        &mirrored,
    )?;

    // Ripples: translucent horizontal lines whose spacing opens up towards
    // the viewer, which is what sells a flat band as a receding surface.
    // Kept very faint -- at any real strength these read as scanlines across
    // the picture rather than as water.
    let mut i = 0u32;
    loop {
        let f = i as f32;
        let y = HORIZON + f * f * 0.62 + f * 1.4;
        if y > HEIGHT {
            break;
        }
        let sway = libm::sinf(t * 0.8 + f * 0.55) * 0.5 + 0.5;
        // Fade the ripples out towards the horizon so they do not cut across
        // the reflection's brightest part.
        let depth = ((y - HORIZON) / (HEIGHT - HORIZON)).min(1.0);
        let alpha = (0.012 + sway * 0.022) * depth;
        canvas2d::fill_rect(
            canvas,
            rect(0.0, y, WIDTH, 1.2),
            color(0.75, 0.92, 1.0, alpha),
        )?;
        i += 1;
    }

    // Glints on the near water: a few bright specks riding the ripples, which
    // give the empty foreground something to hold the eye without competing
    // with the sky.
    for i in 0..26u32 {
        let gx = hash01(i.wrapping_mul(17) + 5) * WIDTH;
        let depth = hash01(i.wrapping_mul(23) + 6);
        let gy = HORIZON + 30.0 + depth * depth * (HEIGHT - HORIZON - 30.0);
        let bob = libm::sinf(t * 1.1 + gx * 0.03 + depth * 5.0);
        let alpha = (0.10 + 0.16 * (bob * 0.5 + 0.5)) * (0.35 + depth * 0.65);
        // Glints stretch horizontally on water, so draw them wide and flat.
        let w = 5.0 + depth * 16.0;
        canvas2d::fill_round_rect(
            canvas,
            rect(gx - w * 0.5, gy, w, 1.6),
            radii(0.8),
            color(0.68, 0.94, 0.92, alpha),
        )?;
    }

    // A soft glow where sky meets water, hiding the seam.
    canvas2d::linear_gradient_stops(
        canvas,
        rect(0.0, HORIZON - 12.0, WIDTH, 26.0),
        90.0,
        &[
            gfx::GradientStop { offset: 0.0, color: color(0.35, 0.85, 0.70, 0.0) },
            gfx::GradientStop { offset: 0.5, color: color(0.40, 0.90, 0.75, 0.14) },
            gfx::GradientStop { offset: 1.0, color: color(0.35, 0.85, 0.70, 0.0) },
        ],
    )?;
    Ok(())
}

/// The title, and a caption pill. Deliberately restrained: the scene is the
/// subject, the type just has to look deliberate.
fn draw_title(canvas: u64, t: f32) -> Result<(), gfx::GfxError> {
    // A breath on the title's glow ties it to the motion behind it.
    let breath = 0.5 + 0.5 * libm::sinf(t * 0.45);

    canvas2d::radial_gradient(
        canvas,
        point(80.0, 68.0),
        150.0,
        color(0.25, 0.95, 0.75, 0.10 + breath * 0.06),
        color(0.25, 0.95, 0.75, 0.0),
    )?;

    canvas2d::draw_text_styled(
        canvas,
        "AURORA",
        point(52.0, 78.0),
        44.0,
        color(0.96, 0.99, 1.0, 0.96),
        style(700, 7.0),
    )?;

    canvas2d::draw_text_styled(
        canvas,
        "68\u{00B0} 21' N  \u{2014}  live",
        point(54.0, 104.0),
        14.0,
        color(0.62, 0.80, 0.88, 0.85),
        style(500, 2.4),
    )?;

    // A small glass caption in the corner, so the frame has a second focal
    // point and the composition is not all weight on the left.
    let card = rect(WIDTH - 250.0, HEIGHT - 86.0, 198.0, 54.0);
    let r = radii(14.0);
    canvas2d::drop_shadow_round_rect(
        canvas,
        rect(card.x, card.y + 5.0, card.width, card.height),
        r,
        16.0,
        color(0.0, 0.0, 0.0, 0.40),
    )?;
    canvas2d::fill_round_rect(canvas, card, r, color(1.0, 1.0, 1.0, 0.06))?;
    canvas2d::stroke_round_rect(canvas, card, r, 1.0, color(1.0, 1.0, 1.0, 0.13))?;

    // A live dot that pulses with the same breath.
    canvas2d::fill_circle(
        canvas,
        point(card.x + 22.0, card.y + 27.0),
        4.0,
        color(0.35, 1.0, 0.70, 0.55 + breath * 0.45),
    )?;
    canvas2d::draw_text_styled(
        canvas,
        "Kp 7 \u{2014} strong",
        point(card.x + 38.0, card.y + 24.0),
        13.0,
        color(0.92, 0.97, 1.0, 0.92),
        style(600, 0.0),
    )?;
    canvas2d::draw_text_styled(
        canvas,
        "visible overhead",
        point(card.x + 38.0, card.y + 41.0),
        11.5,
        color(0.60, 0.76, 0.85, 0.85),
        style(400, 0.0),
    )?;
    Ok(())
}

/// One whole frame. `t` is seconds since launch.
fn draw(canvas: u64, sky: &mut [u8], t: f32) -> Result<(), gfx::GfxError> {
    draw_sky(canvas)?;
    draw_stars(canvas, t)?;

    // The aurora, computed then composited over the stars.
    render_aurora(sky, t);
    canvas2d::draw_pixels(canvas, rect(0.0, 0.0, WIDTH, HORIZON), SKY_W, SKY_H, sky)?;

    // Ridges: the far one hazier and taller, the near one near-black. The
    // gap in value between them is what makes the distance read, and each
    // catches a little of the light above it along its crest.
    draw_ridge(
        canvas,
        41,
        70.0,
        3.0,
        color(0.075, 0.115, 0.20, 1.0),
        color(0.34, 0.72, 0.72, 0.30),
    )?;
    draw_ridge(
        canvas,
        67,
        44.0,
        5.0,
        color(0.020, 0.034, 0.062, 1.0),
        color(0.26, 0.58, 0.62, 0.22),
    )?;

    draw_water(canvas, sky, t)?;

    // A vignette along the bottom edge: it settles the foreground and keeps
    // the caption legible over whatever the water is doing behind it.
    canvas2d::linear_gradient_stops(
        canvas,
        rect(0.0, HEIGHT - 150.0, WIDTH, 150.0),
        90.0,
        &[
            gfx::GradientStop { offset: 0.0, color: color(0.004, 0.008, 0.020, 0.0) },
            gfx::GradientStop { offset: 1.0, color: color(0.004, 0.008, 0.020, 0.75) },
        ],
    )?;

    draw_title(canvas, t)?;

    canvas2d::present(canvas)
}

fn out(line: &str) {
    let stdout = stdio::stdout();
    let _ = stdout.write(line.as_bytes());
    let _ = stdout.write(b"\n");
}

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
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
            "Aurora",
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
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            out("tree:no");
            return 1;
        }
        let _ = tree::upsert_node(win, &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas));
        let _ = window::show(win);

        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => {
                out("bind:no");
                return 1;
            }
        };
        // Draw in design coordinates forever; the host scales them to the
        // real window, centred and in proportion, at any size.
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        // One buffer, reused every frame. Allocating this per frame is how an
        // otherwise fine animation turns into a stutter.
        let mut sky: Vec<u8> = vec![0u8; (SKY_W * SKY_H * 4) as usize];

        let started = clock::monotonic_nanos();
        let mut frames: u32 = 0;

        loop {
            let t = if quick {
                frames as f32 / 30.0
            } else {
                (clock::monotonic_nanos().saturating_sub(started)) as f32 / 1_000_000_000.0
            };

            if draw(canvas, &mut sky, t).is_err() {
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

            // Drain everything queued before drawing again, so a burst of
            // resize or pointer events can never build a backlog.
            let mut closing = false;
            loop {
                match events::poll() {
                    Some(types::Event::CloseRequested(_)) => {
                        closing = true;
                        break;
                    }
                    Some(_) => {}
                    None => break,
                }
            }
            if closing {
                break;
            }

            let _ = window::request_redraw(win);
            match events::wait(Some(16)) {
                Some(types::Event::CloseRequested(_)) => break,
                Some(_) | None => {}
            }
        }

        out("curtains:3");
        out("aurora:live");
        let _ = window::close(win);
        0
    }
}

bindings::export!(Component with_types_in bindings);
