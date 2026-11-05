//! Host-side rasterizer for the `gfx.canvas2d` interface.
//!
//! A canvas is an RGBA picture the app draws with commands instead of sending
//! pixels. Everything downstream of the drawing reuses the image-widget path:
//! the raster publishes `ImagePixels`, and the same code that already puts a
//! photo on screen on all three systems puts the canvas there too. No new
//! adapter code, no new WIT — which is exactly why this stopped being "weeks".
//!
//! The buffer is `0xAARRGGBB` words, the drawn painter's own format, so its
//! `fill_rect` and 5x7 `drawtext` are used directly rather than reimplemented.

use krate_adapter_common::drawtext;
use krate_adapter_common::painter::{draw_image, fill_rect};
use krate_adapter_common::ui::{ImagePixels, UiAdapterError};

/// Largest canvas edge, in logical pixels. Same spirit as the image widget's
/// pixel cap: a width and height multiplied together must not become an
/// allocation that takes the host down.
const MAX_CANVAS_EDGE: u32 = 8_192;

/// One bound canvas: a CPU pixel buffer the app draws into with commands.
pub struct CanvasSurface {
    /// Where drawing is allowed, as (x, y, w, h) in logical pixels.
    ///
    /// Without this a scrolling list paints over its own header, and every
    /// app that scrolls has to skip rows by hand -- which works until rows
    /// have different heights, and then it cannot be done at all. One rect
    /// checked in `blend` is enough, because every draw call lands there.
    clip: Option<(f32, f32, f32, f32)>,
    width: u32,
    height: u32,
    /// The size the app sees and draws in. The buffer above is this times
    /// `scale`: on a 2.6x phone a 411-wide canvas rasters 1080 columns, so
    /// the pixels are native-sharp while every app coordinate stays logical
    /// (K-088). At scale 1 the two sizes are identical and nothing changes.
    logical_width: u32,
    logical_height: u32,
    scale: f32,
    /// A fixed coordinate system the app draws in, if it asked for one.
    /// Draw calls are mapped from here onto the real canvas: scaled
    /// uniformly and centred, so proportions survive any window (K-096).
    design: Option<(f32, f32)>,
    /// The edges a phone covers, top, right, bottom, left (logical): the
    /// design is fitted inside them.
    design_inset: (f32, f32, f32, f32),
    /// `0xAARRGGBB`, row-major from the top — the drawn painter's format.
    buffer: Vec<u32>,
    /// A frame that is one opaque picture covering the whole canvas -- a
    /// game's finished frame, an emulator's screen -- kept as the app gave
    /// it instead of rastered at window resolution. It goes to the screen
    /// at its own size and the display scales it: a 960x600 frame was
    /// upscaled on the CPU to 2560x1600 on a Retina window, copied, and
    /// uploaded every frame. Any other drawing rasters it first, so the
    /// result is exactly what drawing it would have made.
    pending: Option<ImagePixels>,
}

// ---------------------------------------------------------------- op timing
//
// Frame-production time was going somewhere no report could see: publish
// timing said 8ms while the guest produced 5 frames a second. These count
// milliseconds spent inside the hot canvas ops between two presents, taken
// and printed with the publish stats when KRATE_FRAME_STATS=1.
thread_local! {
    static OP_MS: std::cell::Cell<(f32, f32, f32, f32)> = const { std::cell::Cell::new((0.0, 0.0, 0.0, 0.0)) };
}

fn add_op_ms(slot: u8, started: std::time::Instant) {
    let ms = started.elapsed().as_secs_f32() * 1000.0;
    OP_MS.with(|cell| {
        let (mut fills, mut text, mut measure, mut effects) = cell.get();
        match slot {
            0 => fills += ms,
            1 => text += ms,
            2 => measure += ms,
            _ => effects += ms,
        }
        cell.set((fills, text, measure, effects));
    });
    if slot >= 10 {
        EFFECT_MS.with(|cell| {
            let mut each = cell.get();
            each[(slot - 10) as usize % 7] += ms;
            cell.set(each);
        });
    }
}

thread_local! {
    /// The effects' milliseconds one by one: shadow, gradient, radial,
    /// vertical gradient, pixels, sprite, owned pixels.
    static EFFECT_MS: std::cell::Cell<[f32; 7]> = const { std::cell::Cell::new([0.0; 7]) };
}

/// Take and reset the effects' milliseconds one by one, named.
pub fn take_effect_millis() -> [(&'static str, f32); 7] {
    let each = EFFECT_MS.with(|cell| cell.replace([0.0; 7]));
    let names = [
        "shadow",
        "gradient",
        "radial",
        "vgradient",
        "pixels",
        "sprite",
        "owned",
    ];
    core::array::from_fn(|i| (names[i], each[i]))
}

/// A hot op's timer. A browser's wasm32 has no clock `Instant` can read
/// (it panics), so there the timer measures nothing: the Player reports
/// its own frame times from the page.
struct OpTimer(u8, Option<std::time::Instant>);
impl OpTimer {
    fn start(slot: u8) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            Self(slot, None)
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self(slot, Some(std::time::Instant::now()))
        }
    }
}
impl Drop for OpTimer {
    fn drop(&mut self) {
        if let Some(started) = self.1 {
            add_op_ms(self.0, started);
        }
    }
}

/// Take and reset the accumulated (fills, text, measure, effects)
/// milliseconds. Effects are what cost by area: shadows, gradients and
/// pictures drawn into the canvas.
pub fn take_op_millis() -> (f32, f32, f32, f32) {
    OP_MS.with(|cell| cell.replace((0.0, 0.0, 0.0, 0.0)))
}

/// A color in linear floats, packed to the painter's `0xAARRGGBB`.
///
/// Straight alpha, clamped: a guest can send any float, including NaN, and a
/// color must come out the other side rather than undefined behavior.
pub fn pack_color(r: f32, g: f32, b: f32, a: f32) -> u32 {
    let channel = |value: f32| -> u32 {
        if value.is_nan() {
            0
        } else {
            (value.clamp(0.0, 1.0) * 255.0).round() as u32
        }
    };
    (channel(a) << 24) | (channel(r) << 16) | (channel(g) << 8) | channel(b)
}

/// Bilinearly sample a straight-RGBA image at floating texel coordinates,
/// returning `0xAARRGGBB`. Smoothing four neighbouring texels stops a rotated
/// or scaled sprite from shimmering into hard stair-steps as it moves. Samples
/// off the image edge clamp to the nearest edge texel.
fn sample_bilinear(image: &ImagePixels, u: f32, v: f32) -> u32 {
    let w = image.width as i32;
    let h = image.height as i32;
    if w <= 0 || h <= 0 {
        return 0;
    }
    // Texel centres are at +0.5; shift so integer floor picks the lower-left.
    let fx = u - 0.5;
    let fy = v - 0.5;
    let x0 = fx.floor() as i32;
    let y0 = fy.floor() as i32;
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;

    let at = |xi: i32, yi: i32| -> (f32, f32, f32, f32) {
        let x = xi.clamp(0, w - 1);
        let y = yi.clamp(0, h - 1);
        let i = ((y * w + x) * 4) as usize;
        match image.rgba.get(i..i + 4) {
            Some(px) => (px[0] as f32, px[1] as f32, px[2] as f32, px[3] as f32),
            None => (0.0, 0.0, 0.0, 0.0),
        }
    };

    let c00 = at(x0, y0);
    let c10 = at(x0 + 1, y0);
    let c01 = at(x0, y0 + 1);
    let c11 = at(x0 + 1, y0 + 1);

    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let mix = |i: usize| -> u32 {
        let top = lerp(
            [c00.0, c00.1, c00.2, c00.3][i],
            [c10.0, c10.1, c10.2, c10.3][i],
            tx,
        );
        let bot = lerp(
            [c01.0, c01.1, c01.2, c01.3][i],
            [c11.0, c11.1, c11.2, c11.3][i],
            tx,
        );
        (lerp(top, bot, ty) + 0.5) as u32 & 0xFF
    };
    (mix(3) << 24) | (mix(0) << 16) | (mix(1) << 8) | mix(2)
}

/// Linearly interpolate two `0xAARRGGBB` colors, `t` in 0..=1 (0 = a, 1 = b),
/// all four channels including alpha. Used by the gradient primitives.
fn lerp_color(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |shift: u32| -> u32 {
        let ca = ((a >> shift) & 0xFF) as f32;
        let cb = ((b >> shift) & 0xFF) as f32;
        (ca + (cb - ca) * t + 0.5) as u32 & 0xFF
    };
    (mix(24) << 24) | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// Interpolate two colors in float and quantize with a per-pixel ordered
/// dither. Plain 8-bit quantization turns any slow gradient into visible
/// bands ("looks pixely, not HD" -- caught by eye on a purple-to-amber
/// band and confirmed by 3-row color steps in the capture); a half-LSB
/// Bayer offset trades the bands for noise the eye cannot see.
/// Smoothstep of the distance, by squared distance (0 at the centre, 1 at
/// the rim), as a 12-bit fraction: built once.
fn radial_falloff() -> &'static [u16; 4097] {
    static TABLE: std::sync::OnceLock<[u16; 4097]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0u16; 4097];
        for (i, slot) in table.iter_mut().enumerate() {
            let t = (i as f32 / 4096.0).sqrt();
            let t = t * t * (3.0 - 2.0 * t);
            *slot = (t * 4096.0).round().clamp(0.0, 4096.0) as u16;
        }
        table
    })
}

/// `lerp_color_dithered` in fixed point: `t` a 12-bit fraction (0..=4096),
/// the same ordered dither, the same rounding, for loops that mix millions of
/// pixels a frame.
fn channel_mixer(a: u32, b: u32) -> impl Fn(u16, u32, u32) -> u32 {
    // Bayer thresholds, already in twelfths of a level: d = cell/16 - 0.5.
    const BAYER: [[i32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let channels = [24u32, 16, 8, 0].map(|shift| {
        let ca = ((a >> shift) & 0xFF) as i32;
        let cb = ((b >> shift) & 0xFF) as i32;
        (shift, ca, cb - ca)
    });
    move |t: u16, px: u32, py: u32| {
        let t = i32::from(t);
        // (cell / 16 - 0.5) + 0.5, in 1/4096 of a level.
        let offset = BAYER[(py & 3) as usize][(px & 3) as usize] * 256;
        let mut out = 0u32;
        for (shift, ca, delta) in channels {
            let v = ((ca << 12) + delta * t + offset) >> 12;
            out |= (v.clamp(0, 255) as u32) << shift;
        }
        out
    }
}

fn lerp_color_dithered(a: u32, b: u32, t: f32, px: u32, py: u32) -> u32 {
    const BAYER: [[f32; 4]; 4] = [
        [0.0, 8.0, 2.0, 10.0],
        [12.0, 4.0, 14.0, 6.0],
        [3.0, 11.0, 1.0, 9.0],
        [15.0, 7.0, 13.0, 5.0],
    ];
    let d = BAYER[(py & 3) as usize][(px & 3) as usize] / 16.0 - 0.5;
    let t = t.clamp(0.0, 1.0);
    let mix = |shift: u32| -> u32 {
        let ca = ((a >> shift) & 0xFF) as f32;
        let cb = ((b >> shift) & 0xFF) as f32;
        ((ca + (cb - ca) * t + 0.5 + d).clamp(0.0, 255.0)) as u32
    };
    (mix(24) << 24) | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// `sample_stops` with the dither applied at the final quantization.
fn sample_stops_dithered(stops: &[(f32, u32)], t: f32, px: u32, py: u32) -> u32 {
    let first = stops[0];
    let last = stops[stops.len() - 1];
    if t <= first.0 {
        return first.1;
    }
    if t >= last.0 {
        return last.1;
    }
    for pair in stops.windows(2) {
        let (o0, c0) = pair[0];
        let (o1, c1) = pair[1];
        if t <= o1 {
            let f = if o1 > o0 { (t - o0) / (o1 - o0) } else { 1.0 };
            return lerp_color_dithered(c0, c1, f, px, py);
        }
    }
    last.1
}

/// Integer scale for the 5x7 bitmap face at a requested font size. One place,
/// so drawing and measuring cannot pick different scales.
fn bitmap_scale(font_size: f32) -> u32 {
    ((font_size / 7.0).round() as u32).clamp(1, 16)
}

/// What a text run will occupy once drawn: advance width, line height, and the
/// split of that height either side of the baseline. The canvas draw-text
/// origin is the baseline, which is why `ascent` is reported separately.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TextMetrics {
    pub width: f32,
    pub height: f32,
    pub ascent: f32,
    pub descent: f32,
}

impl CanvasSurface {
    /// A 1:1 surface. Used by tests and by the Linux winit adapter, which is
    /// behind a feature flag -- so a macOS-only build sees no caller and
    /// clippy's dead-code pass fires on CI while local builds stay quiet.
    /// The method is real API, so it is kept rather than deleted.
    #[allow(dead_code)]
    pub fn new(width: u32, height: u32) -> Result<Self, UiAdapterError> {
        Self::new_scaled(width, height, 1.0)
    }

    /// A surface whose buffer is `scale` times its logical size, for HiDPI
    /// hosts. The app draws in logical coordinates; every public draw call
    /// multiplies by `scale` on the way in, and `to_image` hands the
    /// physical-resolution pixels to the painter, which blits them 1:1.
    pub fn new_scaled(width: u32, height: u32, scale: f32) -> Result<Self, UiAdapterError> {
        if width == 0 || height == 0 || width > MAX_CANVAS_EDGE || height > MAX_CANVAS_EDGE {
            return Err(UiAdapterError::Unsupported(format!(
                "a canvas must be between 1x1 and {MAX_CANVAS_EDGE}x{MAX_CANVAS_EDGE}, got {width}x{height}"
            )));
        }
        // A hostile or broken scale must not make the buffer explode or
        // vanish; the clamp bounds are wider than any real display.
        let scale = if scale.is_finite() {
            scale.clamp(0.25, 8.0)
        } else {
            1.0
        };
        let phys_w = ((width as f32 * scale).round() as u32).clamp(1, MAX_CANVAS_EDGE);
        let phys_h = ((height as f32 * scale).round() as u32).clamp(1, MAX_CANVAS_EDGE);
        Ok(Self {
            clip: None,
            width: phys_w,
            height: phys_h,
            logical_width: width,
            logical_height: height,
            scale,
            design: None,
            design_inset: (0.0, 0.0, 0.0, 0.0),
            // Made on first draw (see `materialize`): an app that only ever
            // hands over whole pictures never needs a window-sized raster,
            // which on a Retina window is 16 MB written white for nothing.
            buffer: Vec::new(),
            pending: None,
        })
    }

    /// Re-fit the surface to a new size, discarding the old pixels.
    ///
    /// A window is resizable, so the widget rect a canvas was bound to is not
    /// the rect it keeps. Without this the buffer stays frozen at its bind-time
    /// size: `canvas_size` reports a stale answer, an app that lays out from it
    /// draws to the wrong extent, and every hit-box is off. Returns whether the
    /// size actually changed, so callers can skip redundant work.
    #[allow(dead_code)] // see `new`: tests and the feature-gated Linux adapter
    pub fn resize(&mut self, width: u32, height: u32) -> Result<bool, UiAdapterError> {
        self.resize_scaled(width, height, self.scale)
    }

    /// `resize` that can also change the raster scale, for a window that
    /// moved to a display with a different density.
    pub fn resize_scaled(
        &mut self,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<bool, UiAdapterError> {
        if width == self.logical_width
            && height == self.logical_height
            && (scale - self.scale).abs() < f32::EPSILON
        {
            return Ok(false);
        }
        let mut fitted = Self::new_scaled(width, height, scale)?;
        // A resize changes the canvas, never the coordinate system the app
        // chose to draw in. Replacing the whole surface silently dropped the
        // design size, so an app that letterboxes correctly looked, one
        // frame later, exactly like an app that ignores the window (K-096).
        fitted.design = self.design;
        fitted.design_inset = self.design_inset;
        *self = fitted;
        Ok(true)
    }

    /// The raster scale this surface was built with.
    #[allow(dead_code)] // see `new`: tests and the feature-gated Linux adapter
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Draw in a fixed coordinate system from now on.
    pub fn set_design_size(&mut self, width: f32, height: f32) {
        if width > 0.0 && height > 0.0 {
            self.design = Some((width, height));
        } else {
            self.design = None;
        }
    }

    /// The design system's scale and offset onto the real canvas: uniform,
    /// centred, letterboxed. `(1.0, 0.0, 0.0)` when no design size is set,
    /// so every draw path can apply it unconditionally.
    fn design_transform(&self) -> (f32, f32, f32) {
        let Some(design) = self.design else {
            return (1.0, 0.0, 0.0);
        };
        krate_adapter_common::canvas_list::fit_design(
            (self.logical_width as f32, self.logical_height as f32),
            design,
            self.design_inset,
        )
    }

    /// The edges a phone covers, for fitting a design inside them (top,
    /// right, bottom, left, logical pixels).
    pub fn set_design_inset(&mut self, inset: (f32, f32, f32, f32)) {
        self.design_inset = inset;
    }

    pub fn design_inset(&self) -> (f32, f32, f32, f32) {
        self.design_inset
    }

    /// The design's scale and offset onto this canvas, for mapping a point
    /// back into the design: None without a design size.
    pub fn design_fit(&self) -> Option<(f32, f32, f32)> {
        self.design.map(|_| self.design_transform())
    }

    /// Paint the spare space around a letterboxed design with the colour
    /// along each of its edges, so an app designed for one shape reads as
    /// filling a screen of another instead of sitting between bars. The
    /// iPhone's GPU path does the same from the app's background call
    /// (`CanvasList::edge_colors`).
    fn blend_letterbox(&self, rgba: &mut [u8]) {
        if self.design.is_none() {
            return;
        }
        let (k, ox, oy) = self.design_transform();
        let Some((dw, dh)) = self.design else {
            return;
        };
        let (w, h) = (self.width as usize, self.height as usize);
        let s = self.width as f32 / self.logical_width.max(1) as f32;
        // The design's edges by fill_rect's own rule (an edge covers the
        // pixel it falls in, so the far edge rounds down): the columns and
        // rows sampled below are ones the app's fills actually painted.
        // Rounded to nearest, the far edge at 2073.6 sampled column 2073,
        // which no fill reached -- still white, and copied across the whole
        // right side of a design-size game on a wide window (K-1155).
        let x0 = ((ox * s).floor().max(0.0) as usize).min(w);
        let y0 = ((oy * s).floor().max(0.0) as usize).min(h);
        let x1 = (((ox + dw * k) * s).floor().max(0.0) as usize).clamp(x0, w);
        let y1 = (((oy + dh * k) * s).floor().max(0.0) as usize).clamp(y0, h);
        if x1 <= x0 || y1 <= y0 || (x0 == 0 && y0 == 0 && x1 == w && y1 == h) {
            return;
        }
        let avg = |rgba: &[u8], points: &mut dyn Iterator<Item = (usize, usize)>| {
            let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
            for (x, y) in points {
                let i = (y * w + x) * 4;
                r += rgba[i] as u64;
                g += rgba[i + 1] as u64;
                b += rgba[i + 2] as u64;
                n += 1;
            }
            let n = n.max(1);
            [(r / n) as u8, (g / n) as u8, (b / n) as u8, 255u8]
        };
        let top = avg(rgba, &mut (x0..x1).map(|x| (x, y0)));
        let bottom = avg(rgba, &mut (x0..x1).map(|x| (x, y1 - 1)));
        let left = avg(rgba, &mut (y0..y1).map(|y| (x0, y)));
        let right = avg(rgba, &mut (y0..y1).map(|y| (x1 - 1, y)));
        let mut fill = |xs: std::ops::Range<usize>, ys: std::ops::Range<usize>, c: [u8; 4]| {
            for y in ys {
                for x in xs.clone() {
                    let i = (y * w + x) * 4;
                    rgba[i..i + 4].copy_from_slice(&c);
                }
            }
        };
        fill(0..w, 0..y0, top);
        fill(0..w, y1..h, bottom);
        fill(0..x0, y0..y1, left);
        fill(x1..w, y0..y1, right);
    }

    /// Paint the spare space around the design into the surface itself, as
    /// `to_image` would into its picture. For drawing something over the
    /// whole screen afterwards -- the host's on-screen pad (IC-914) -- with
    /// the design switched off, when `to_image` will paint no bands.
    pub fn bake_letterbox(&mut self) {
        if self.design.is_none() || self.buffer.is_empty() {
            return;
        }
        let mut rgba = vec![0u8; self.buffer.len() * 4];
        for (chunk, word) in rgba.chunks_exact_mut(4).zip(self.buffer.iter()) {
            chunk[0] = (word >> 16) as u8;
            chunk[1] = (word >> 8) as u8;
            chunk[2] = *word as u8;
            chunk[3] = (word >> 24) as u8;
        }
        self.blend_letterbox(&mut rgba);
        for (word, chunk) in self.buffer.iter_mut().zip(rgba.chunks_exact(4)) {
            *word = (u32::from(chunk[3]) << 24)
                | (u32::from(chunk[0]) << 16)
                | (u32::from(chunk[1]) << 8)
                | u32::from(chunk[2]);
        }
    }

    /// Map one design-space point onto the canvas.
    fn map_point(&self, x: f32, y: f32) -> (f32, f32) {
        let (k, ox, oy) = self.design_transform();
        (x * k + ox, y * k + oy)
    }

    /// Map a design-space length.
    fn map_len(&self, v: f32) -> f32 {
        v * self.design_transform().0
    }

    /// The design size an app is drawing in, if it set one.
    pub fn design_size(&self) -> Option<(f32, f32)> {
        self.design
    }

    /// Restrict drawing to a rectangle. Anything outside is dropped.
    ///
    /// Deliberately a single rect rather than a stack: nested clips are what
    /// a widget toolkit needs, and a canvas app needs "this list, this
    /// region". Adding a stack later does not break this.
    pub fn set_clip(&mut self, rect: Option<(f32, f32, f32, f32)>) {
        let k = self.scale;
        self.clip = rect.map(|(x, y, w, h)| {
            let (mx, my) = self.map_point(x, y);
            // To the edge, as fill_rect is (see `to_edges`).
            self.to_edges(mx * k, my * k, self.map_len(w) * k, self.map_len(h) * k)
        });
    }

    /// Whether a pixel may be written. Used by the per-pixel primitives.
    fn allowed(&self, x: u32, y: u32) -> bool {
        match self.clip {
            None => true,
            Some((cx, cy, cw, ch)) => {
                let px = x as f32;
                let py = y as f32;
                px >= cx && py >= cy && px < cx + cw && py < cy + ch
            }
        }
    }

    /// Trim a rectangle to the clip, returning nothing when it falls outside.
    ///
    /// Rect-shaped draws are clipped by shrinking the rect rather than by
    /// testing every pixel: same result, and it keeps the fast path fast.
    fn clipped(&self, x: f32, y: f32, w: f32, h: f32) -> Option<(f32, f32, f32, f32)> {
        let Some((cx, cy, cw, ch)) = self.clip else {
            return Some((x, y, w, h));
        };
        let left = x.max(cx);
        let top = y.max(cy);
        let right = (x + w).min(cx + cw);
        let bottom = (y + h).min(cy + ch);
        if right <= left || bottom <= top {
            return None;
        }
        Some((left, top, right - left, bottom - top))
    }

    pub fn clear(&mut self, color: u32) {
        self.pending = None;
        self.ensure_buffer();
        let __op_timer = OpTimer::start(0);
        let _ = &__op_timer;
        // Clear respects the clip too, so "clear this region" works -- which
        // is what a scrolling list needs before redrawing its rows.
        match self.clipped(0.0, 0.0, self.width as f32, self.height as f32) {
            Some((0.0, 0.0, w, h)) if w >= self.width as f32 && h >= self.height as f32 => {
                self.buffer.fill(color);
            }
            Some((x, y, w, h)) => self.fill_rect_unclipped(x, y, w, h, color),
            None => {}
        }
    }

    fn fill_rect_unclipped(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        fill_rect(
            &mut self.buffer,
            self.width,
            self.height,
            (x, y, w, h),
            color,
        );
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        self.materialize();
        let __op_timer = OpTimer::start(0);
        let _ = &__op_timer;
        let (x, y) = self.map_point(x, y);
        let (w, h) = (self.map_len(w), self.map_len(h));
        let k = self.scale;
        let (x, y, w, h) = self.to_edges(x * k, y * k, w * k, h * k);
        let Some((x, y, w, h)) = self.clipped(x, y, w, h) else {
            return;
        };
        fill_rect(
            &mut self.buffer,
            self.width,
            self.height,
            (x, y, w, h),
            color,
        );
    }

    /// A rectangle (device pixels) that ends within a pixel of the canvas's
    /// right or bottom edge ends at it. A canvas is a whole number of points
    /// on a screen that is not: 411 points at 2.625 is 1078.875 pixels of a
    /// 1079-pixel canvas, so a background filled "to the edge" left its last
    /// column part-painted each frame, keeping blends of earlier frames -- a
    /// speckled sliver down the right of every app that lays out from the
    /// screen. Only without a design size: a design is letterboxed on
    /// purpose, and its edges are not the canvas's.
    fn to_edges(&self, x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
        if self.design.is_some() {
            return (x, y, w, h);
        }
        let (cw, ch) = (self.width as f32, self.height as f32);
        let w = if x + w >= cw - 1.0 && x + w < cw {
            cw - x
        } else {
            w
        };
        let h = if y + h >= ch - 1.0 && y + h < ch {
            ch - y
        } else {
            h
        };
        (x, y, w, h)
    }

    /// A filled circle with an anti-aliased edge, alpha-blended over the
    /// canvas. Each pixel's coverage is its distance from the edge, so the rim
    /// is smooth rather than stair-stepped -- the primitive that makes round
    /// things look drawn instead of plotted.
    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: u32) {
        self.materialize();
        let (cx, cy) = self.map_point(cx, cy);
        let radius = self.map_len(radius);
        let k = self.scale;
        let (cx, cy, radius) = (cx * k, cy * k, radius * k);
        if radius <= 0.0 {
            return;
        }
        let (w, h) = (self.width, self.height);
        let x0 = ((cx - radius).floor().max(0.0) as u32).min(w);
        let x1 = (((cx + radius).ceil()).max(0.0) as u32).min(w);
        let y0 = ((cy - radius).floor().max(0.0) as u32).min(h);
        let y1 = (((cy + radius).ceil()).max(0.0) as u32).min(h);
        let base_a = (color >> 24) & 0xFF;
        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - cx;
                let dy = py as f32 + 0.5 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                // Coverage: 1 inside, fading to 0 across a 1px edge band.
                let coverage = (radius - dist + 0.5).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                let a = ((base_a as f32) * coverage) as u32;
                if a == 0 {
                    continue;
                }
                let px_color = (a << 24) | (color & 0x00FF_FFFF);
                let idx = (py * w + px) as usize;
                if !self.allowed(px, py) {
                    continue;
                }
                if let Some(slot) = self.buffer.get_mut(idx) {
                    *slot = krate_adapter_common::painter::blend_over(px_color, *slot);
                }
            }
        }
    }

    /// Stroke a circle's edge: a ring of `width` pixels centred on `radius`.
    ///
    /// Anti-aliased the same way `fill_circle` is, by coverage across a
    /// one-pixel band, so a thin rim reads as a smooth curve rather than a
    /// staircase.
    ///
    /// This exists because an AI asked for one and could not have it. Told to
    /// put "a thin rim" on a bubble, with only `fill_circle` and `stroke_rect`
    /// available, it reached for `stroke_rect` -- and every round bubble in
    /// that screensaver got a visible square box around it. The model did the
    /// best it could with what we exposed; the gap was ours.
    pub fn stroke_circle(&mut self, cx: f32, cy: f32, radius: f32, width: f32, color: u32) {
        self.materialize();
        let (cx, cy) = self.map_point(cx, cy);
        let (radius, width) = (self.map_len(radius), self.map_len(width));
        let k = self.scale;
        let (cx, cy, radius, width) = (cx * k, cy * k, radius * k, width * k);
        if radius <= 0.0 || width <= 0.0 {
            return;
        }
        let half = width * 0.5;
        let outer = radius + half;
        let (w, h) = (self.width, self.height);
        let x0 = ((cx - outer).floor().max(0.0) as u32).min(w);
        let x1 = (((cx + outer).ceil()).max(0.0) as u32).min(w);
        let y0 = ((cy - outer).floor().max(0.0) as u32).min(h);
        let y1 = (((cy + outer).ceil()).max(0.0) as u32).min(h);
        let base_a = (color >> 24) & 0xFF;
        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - cx;
                let dy = py as f32 + 0.5 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                // Distance from the ring's centre line. Coverage fades across
                // a one-pixel band on both sides, so a sub-pixel width still
                // draws as a faint line rather than vanishing.
                let from_ring = (dist - radius).abs();
                let coverage = (half - from_ring + 0.5).clamp(0.0, 1.0);
                if coverage <= 0.0 {
                    continue;
                }
                let a = ((base_a as f32) * coverage) as u32;
                if a == 0 {
                    continue;
                }
                if !self.allowed(px, py) {
                    continue;
                }
                let px_color = (a << 24) | (color & 0x00FF_FFFF);
                let idx = (py * w + px) as usize;
                if let Some(slot) = self.buffer.get_mut(idx) {
                    *slot = krate_adapter_common::painter::blend_over(px_color, *slot);
                }
            }
        }
    }

    /// Signed distance from a point to a rounded rectangle's edge, negative
    /// inside. Per-corner radii by quadrant selection: the corner whose
    /// quadrant the point falls in decides the radius -- exact for radii that
    /// do not overlap, which the clamp below guarantees.
    fn round_rect_sdf(
        px: f32,
        py: f32,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radii: (f32, f32, f32, f32),
    ) -> f32 {
        let (tl, tr, br, bl) = radii;
        let qx = px - (x + w * 0.5);
        let qy = py - (y + h * 0.5);
        let r = if qx < 0.0 {
            if qy < 0.0 {
                tl
            } else {
                bl
            }
        } else if qy < 0.0 {
            tr
        } else {
            br
        }
        .clamp(0.0, (w * 0.5).min(h * 0.5));
        let dx = qx.abs() - (w * 0.5 - r);
        let dy = qy.abs() - (h * 0.5 - r);
        let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
        outside + dx.max(dy).min(0.0) - r
    }

    /// Blend one coverage-weighted pixel: the shared tail of every
    /// anti-aliased primitive here.
    #[inline]
    fn blend_coverage(&mut self, px: u32, py: u32, color: u32, coverage: f32) {
        if coverage <= 0.0 {
            return;
        }
        let a = (((color >> 24) & 0xFF) as f32 * coverage) as u32;
        if a == 0 || !self.allowed(px, py) {
            return;
        }
        let px_color = (a << 24) | (color & 0x00FF_FFFF);
        let idx = (py * self.width + px) as usize;
        if let Some(slot) = self.buffer.get_mut(idx) {
            *slot = krate_adapter_common::painter::blend_over(px_color, *slot);
        }
    }

    /// The pixel rows and columns a draw may touch, trimmed to the clip so
    /// a card mostly hidden by it costs only what shows. Same pixels as
    /// testing `allowed` on each: a pixel is in when `cx <= px < cx + cw`.
    fn bounds_in_clip(&self, x0: u32, x1: u32, y0: u32, y1: u32) -> (u32, u32, u32, u32) {
        let Some((cx, cy, cw, ch)) = self.clip else {
            return (x0, x1, y0, y1);
        };
        let lo = |v: f32| v.ceil().max(0.0) as u32;
        let (cx0, cx1, cy0, cy1) = (lo(cx), lo(cx + cw), lo(cy), lo(cy + ch));
        (
            x0.max(cx0),
            x1.min(cx1).max(x0.max(cx0)),
            y0.max(cy0),
            y1.min(cy1).max(y0.max(cy0)),
        )
    }

    /// For one pixel row of a rounded rectangle, the columns whose coverage
    /// is surely full: centres at least `inset` inside every edge and clear
    /// of the corner bands, by a small margin so the answer is exactly the
    /// one the distance would give. `None` when no column is.
    #[allow(clippy::too_many_arguments)]
    fn full_span(
        py: u32,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rmax: f32,
        inset: f32,
    ) -> Option<(u32, u32)> {
        const EPS: f32 = 1e-3;
        let (cx, cy, hw, hh) = (x + w * 0.5, y + h * 0.5, w * 0.5, h * 0.5);
        let qy = (py as f32 + 0.5 - cy).abs();
        let band = rmax.max(inset);
        let half = if qy <= hh - band - EPS {
            hw - inset - EPS
        } else if qy <= hh - inset - EPS {
            hw - band - EPS
        } else {
            return None;
        };
        if half <= 0.0 {
            return None;
        }
        let first = (cx - half - 0.5).ceil();
        let last = (cx + half - 0.5).floor();
        if last < first || last < 0.0 {
            return None;
        }
        Some((first.max(0.0) as u32, last as u32))
    }

    /// A filled rounded rectangle with anti-aliased corners: the card
    /// primitive. Panels, buttons, chips and list rows are all this shape in
    /// any current design, and faking it from rects and circles never
    /// survives a close look.
    pub fn fill_round_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radii: (f32, f32, f32, f32),
        color: u32,
    ) {
        self.materialize();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (x, y) = self.map_point(x, y);
        let (w, h) = (self.map_len(w), self.map_len(h));
        let k = self.scale;
        let (x, y, w, h) = (x * k, y * k, w * k, h * k);
        let radii = (radii.0 * k, radii.1 * k, radii.2 * k, radii.3 * k);
        let (bw, bh) = (self.width, self.height);
        let x0 = (x.floor().max(0.0) as u32).min(bw);
        let x1 = ((x + w).ceil().max(0.0) as u32).min(bw);
        let y0 = (y.floor().max(0.0) as u32).min(bh);
        let y1 = ((y + h).ceil().max(0.0) as u32).min(bh);
        // Only what the clip shows, and inside the shape no distance at all:
        // a phone card is mostly middle, and every pixel of its box used to
        // pay for a square root (Query and Trace drew theirs in pieces to
        // stay inside a phone's frame time, 2026-11-03). The pixels are the
        // ones the distance gives, to the bit.
        let (x0, x1, y0, y1) = self.bounds_in_clip(x0, x1, y0, y1);
        let rmax = Self::max_radius(w, h, radii);
        for py in y0..y1 {
            let span = Self::full_span(py, x, y, w, h, rmax, 0.5);
            let mut px = x0;
            while px < x1 {
                if let Some((a, b)) = span.filter(|&(a, b)| px >= a && px <= b) {
                    // The row's full middle in one fill: the clip is already
                    // in the bounds, so every pixel here may be written.
                    let end = (b + 1).min(x1);
                    self.fill_span(px, end, py, color);
                    px = end.max(a + 1);
                    continue;
                }
                let d = Self::round_rect_sdf(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, radii);
                self.blend_coverage(px, py, color, (0.5 - d).clamp(0.0, 1.0));
                px += 1;
            }
        }
    }

    /// Columns `x0..x1` of row `y` at full coverage: what `blend_coverage`
    /// with coverage 1 does to each, a row at a time.
    fn fill_span(&mut self, x0: u32, x1: u32, y: u32, color: u32) {
        if x1 <= x0 {
            return;
        }
        fill_rect(
            &mut self.buffer,
            self.width,
            self.height,
            (x0 as f32, y as f32, (x1 - x0) as f32, 1.0),
            color,
        );
    }

    /// The largest corner radius as the distance uses it (clamped to half
    /// the shorter side).
    fn max_radius(w: f32, h: f32, radii: (f32, f32, f32, f32)) -> f32 {
        let cap = (w * 0.5).min(h * 0.5);
        [radii.0, radii.1, radii.2, radii.3]
            .into_iter()
            .map(|r| r.clamp(0.0, cap))
            .fold(0.0, f32::max)
    }

    /// Stroke a rounded rectangle's edge, `width` pixels thick, anti-aliased
    /// on both sides the way `stroke_circle` is.
    // Eight arguments because the shape genuinely has eight degrees of
    // freedom (position, size, four radii collapse to one tuple, thickness
    // or blur, color); a params struct here would make every call site
    // longer for no clarity.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_round_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radii: (f32, f32, f32, f32),
        width: f32,
        color: u32,
    ) {
        self.materialize();
        if w <= 0.0 || h <= 0.0 || width <= 0.0 {
            return;
        }
        let (x, y) = self.map_point(x, y);
        let (w, h, width) = (self.map_len(w), self.map_len(h), self.map_len(width));
        let k = self.scale;
        let (x, y, w, h, width) = (x * k, y * k, w * k, h * k, width * k);
        let radii = (radii.0 * k, radii.1 * k, radii.2 * k, radii.3 * k);
        let half = width * 0.5;
        let (bw, bh) = (self.width, self.height);
        let x0 = ((x - half).floor().max(0.0) as u32).min(bw);
        let x1 = ((x + w + half).ceil().max(0.0) as u32).min(bw);
        let y0 = ((y - half).floor().max(0.0) as u32).min(bh);
        let y1 = ((y + h + half).ceil().max(0.0) as u32).min(bh);
        let (x0, x1, y0, y1) = self.bounds_in_clip(x0, x1, y0, y1);
        for py in y0..y1 {
            for px in x0..x1 {
                let d = Self::round_rect_sdf(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, radii);
                self.blend_coverage(px, py, color, (half - d.abs() + 0.5).clamp(0.0, 1.0));
            }
        }
    }

    /// A soft shadow: the rounded silhouette with alpha falling off smoothly
    /// across `blur` pixels either side of the edge. Analytic -- a smoothstep
    /// over signed distance -- so it costs one pass and no buffers, and at
    /// card sizes reads the same as a true Gaussian. Draw it first, then the
    /// card over it: the depth cue that lifts a panel off the background.
    // Eight arguments because the shape genuinely has eight degrees of
    // freedom (position, size, four radii collapse to one tuple, thickness
    // or blur, color); a params struct here would make every call site
    // longer for no clarity.
    #[allow(clippy::too_many_arguments)]
    pub fn drop_shadow_round_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radii: (f32, f32, f32, f32),
        blur: f32,
        color: u32,
    ) {
        let _effects_timer = OpTimer::start(10);
        self.materialize();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (x, y) = self.map_point(x, y);
        let (w, h, blur) = (self.map_len(w), self.map_len(h), self.map_len(blur));
        let k = self.scale;
        let (x, y, w, h, blur) = (x * k, y * k, w * k, h * k, blur * k);
        let blur = blur.max(0.5);
        let (bw, bh) = (self.width, self.height);
        let x0 = ((x - blur).floor().max(0.0) as u32).min(bw);
        let x1 = ((x + w + blur).ceil().max(0.0) as u32).min(bw);
        let y0 = ((y - blur).floor().max(0.0) as u32).min(bh);
        let y1 = ((y + h + blur).ceil().max(0.0) as u32).min(bh);
        // As fill_round_rect: only what the clip shows, and where the shadow
        // is surely full strength (a blur's width inside) no distance.
        let (x0, x1, y0, y1) = self.bounds_in_clip(x0, x1, y0, y1);
        let rmax = Self::max_radius(w, h, radii);
        for py in y0..y1 {
            let span = Self::full_span(py, x, y, w, h, rmax, blur);
            let mut px = x0;
            while px < x1 {
                if let Some((a, b)) = span.filter(|&(a, b)| px >= a && px <= b) {
                    let end = (b + 1).min(x1);
                    self.fill_span(px, end, py, color);
                    px = end.max(a + 1);
                    continue;
                }
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                // Beside a straight edge -- away from the corners' squares --
                // the distance to a rounded rectangle is the distance to that
                // edge: one subtraction, not the corner maths. Most of a big
                // card's soft edge is there (K-1212).
                let d = if (fy >= y + rmax && fy <= y + h - rmax)
                    || (fx >= x + rmax && fx <= x + w - rmax)
                {
                    (x - fx).max(fx - (x + w)).max(y - fy).max(fy - (y + h))
                } else {
                    Self::round_rect_sdf(fx, fy, x, y, w, h, radii)
                };
                let t = ((blur - d) / (2.0 * blur)).clamp(0.0, 1.0);
                self.blend_coverage(px, py, color, t * t * (3.0 - 2.0 * t));
                px += 1;
            }
        }
    }

    /// Fill a rectangle with a gradient through any number of stops at any
    /// angle (0 degrees is left-to-right, 90 top-to-bottom). Stops are
    /// (offset, packed color), sorted. Two stops is a plain fade; three or
    /// more is the rich background every current design reference uses.
    pub fn linear_gradient_stops(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        angle_degrees: f32,
        stops: &[(f32, u32)],
    ) {
        let _effects_timer = OpTimer::start(11);
        self.materialize();
        if stops.is_empty() {
            return;
        }
        if stops.len() == 1 {
            // Delegate on the raw arguments: fill_rect scales them itself.
            self.fill_rect(x, y, w, h, stops[0].1);
            return;
        }
        let (x, y) = self.map_point(x, y);
        let (w, h) = (self.map_len(w), self.map_len(h));
        let k = self.scale;
        let (x, y, w, h) = (x * k, y * k, w * k, h * k);
        let Some((cx0, cy0, cw, ch)) = self.clipped(x, y, w, h) else {
            return;
        };
        let theta = angle_degrees.to_radians();
        let (dir_x, dir_y) = (theta.cos(), theta.sin());
        // Project the rect's corners onto the axis so offsets 0 and 1 land
        // on the rect's extremes at any angle, matching CSS.
        let corners = [(x, y), (x + w, y), (x, y + h), (x + w, y + h)];
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for (px, py) in corners {
            let t = px * dir_x + py * dir_y;
            lo = lo.min(t);
            hi = hi.max(t);
        }
        let span = (hi - lo).max(1e-6);
        let (bw, bh) = (self.width, self.height);
        let x0 = (cx0.floor().max(0.0) as u32).min(bw);
        let x1 = ((cx0 + cw).ceil().max(0.0) as u32).min(bw);
        let y0 = (cy0.floor().max(0.0) as u32).min(bh);
        let y1 = ((cy0 + ch).ceil().max(0.0) as u32).min(bh);
        for py in y0..y1 {
            for px in x0..x1 {
                let t = ((px as f32 + 0.5) * dir_x + (py as f32 + 0.5) * dir_y - lo) / span;
                self.blend_coverage(px, py, sample_stops_dithered(stops, t, px, py), 1.0);
            }
        }
    }

    /// Stroke part of a circle: the progress-ring primitive. Angles in
    /// degrees, 0 at 3 o'clock, increasing clockwise on screen; `sweep` runs
    /// clockwise from `start`. Radially anti-aliased like `stroke_circle`,
    /// and feathered at both angular ends so the arc tips look cut, not torn.
    #[allow(clippy::too_many_arguments)]
    pub fn stroke_arc(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        start_degrees: f32,
        sweep_degrees: f32,
        width: f32,
        color: u32,
    ) {
        self.materialize();
        if radius <= 0.0 || width <= 0.0 || sweep_degrees == 0.0 {
            return;
        }
        // Normalise to a clockwise sweep in [0, 360].
        let (start, sweep) = if sweep_degrees < 0.0 {
            (start_degrees + sweep_degrees, -sweep_degrees)
        } else {
            (start_degrees, sweep_degrees)
        };
        if sweep >= 360.0 {
            // Delegate on the raw arguments: stroke_circle scales them.
            self.stroke_circle(cx, cy, radius, width, color);
            return;
        }
        let (cx, cy) = self.map_point(cx, cy);
        let (radius, width) = (self.map_len(radius), self.map_len(width));
        let k = self.scale;
        let (cx, cy, radius, width) = (cx * k, cy * k, radius * k, width * k);
        let half = width * 0.5;
        let outer = radius + half;
        let (w, h) = (self.width, self.height);
        let x0 = ((cx - outer).floor().max(0.0) as u32).min(w);
        let x1 = ((cx + outer).ceil().max(0.0) as u32).min(w);
        let y0 = ((cy - outer).floor().max(0.0) as u32).min(h);
        let y1 = ((cy + outer).ceil().max(0.0) as u32).min(h);
        // One pixel of angular feather, expressed in degrees at this radius.
        let feather_deg = (1.0 / radius.max(1.0)).to_degrees();
        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - cx;
                let dy = py as f32 + 0.5 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                let ring = (half - (dist - radius).abs() + 0.5).clamp(0.0, 1.0);
                if ring <= 0.0 {
                    continue;
                }
                // Screen y grows downward, so atan2(dy, dx) already runs
                // clockwise visually; degrees into the sweep from `start`.
                let ang = dy.atan2(dx).to_degrees();
                let into = (ang - start).rem_euclid(360.0);
                let angular = ((sweep - into) / feather_deg + 1.0)
                    .min(into / feather_deg + 1.0)
                    .clamp(0.0, 1.0);
                self.blend_coverage(px, py, color, ring * angular);
            }
        }
    }

    /// A radial gradient disc: `inner` color at the center easing to `outer`
    /// color (typically transparent) at `radius`. This is the real glow/bloom
    /// primitive -- a soft light falloff instead of a flat disc, which is the
    /// single biggest difference between a modern look and a flat one.
    pub fn radial_gradient(&mut self, cx: f32, cy: f32, radius: f32, inner: u32, outer: u32) {
        let _effects_timer = OpTimer::start(12);
        self.materialize();
        let (cx, cy) = self.map_point(cx, cy);
        let radius = self.map_len(radius);
        let k = self.scale;
        let (cx, cy, radius) = (cx * k, cy * k, radius * k);
        if radius <= 0.0 || !radius.is_finite() || !cx.is_finite() || !cy.is_finite() {
            return;
        }
        let (w, h) = (self.width, self.height);
        let mut x0 = ((cx - radius).floor().max(0.0) as u32).min(w);
        let mut x1 = (((cx + radius).ceil()).max(0.0) as u32).min(w);
        let mut y0 = ((cy - radius).floor().max(0.0) as u32).min(h);
        let mut y1 = (((cy + radius).ceil()).max(0.0) as u32).min(h);
        // The clip once, as a range, rather than asked of every pixel.
        if let Some((clx, cly, clw, clh)) = self.clip {
            x0 = x0.max(clx.ceil().max(0.0) as u32);
            y0 = y0.max(cly.ceil().max(0.0) as u32);
            x1 = x1.min((clx + clw).ceil().max(0.0) as u32);
            y1 = y1.min((cly + clh).ceil().max(0.0) as u32);
        }
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        // Across a phone screen this is millions of pixels a frame: a full-
        // screen glow behind Krate's permission sheet took 264 ms of every
        // frame on a Galaxy S22 Ultra (K-1212). So the falloff -- smoothstep
        // of the distance -- is read from a table indexed by the squared
        // distance (no square root per pixel), and the colour is mixed in
        // fixed point. Within one level of the float version per channel.
        let falloff = radial_falloff();
        let inv_r2 = 1.0 / (radius * radius);
        let steps = (falloff.len() - 1) as f32;
        let mix = channel_mixer(inner, outer);
        for py in y0..y1 {
            let dy = py as f32 + 0.5 - cy;
            let dy2 = dy * dy * inv_r2;
            if dy2 > 1.0 {
                continue;
            }
            let row = (py * w) as usize;
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - cx;
                let s2 = dx * dx * inv_r2 + dy2;
                if s2 > 1.0 {
                    continue;
                }
                let t = falloff[(s2 * steps) as usize];
                let c = mix(t, px, py);
                if (c >> 24) == 0 {
                    continue;
                }
                if let Some(slot) = self.buffer.get_mut(row + px as usize) {
                    *slot = krate_adapter_common::painter::blend_over(c, *slot);
                }
            }
        }
    }

    /// A vertical linear gradient filling a rectangle: `top` color at `y`
    /// easing to `bottom` at `y + h`. For skies, panels, backdrops.
    pub fn linear_gradient_v(&mut self, x: f32, y: f32, w: f32, h: f32, top: u32, bottom: u32) {
        let _effects_timer = OpTimer::start(13);
        self.materialize();
        // One band per device row, each handed to `fill_rect` in the app's
        // OWN coordinates, because `fill_rect` maps and scales itself. This
        // used to map and scale first and pass the result on -- the double
        // transform K-425 found in strokes -- so in a design-size app or on
        // a dense screen the gradient landed at the wrong place and size,
        // and the screen around a letterboxed design stayed blank.
        //
        // The bands are cut in DEVICE rows, one fill per row. They used to be
        // cut in app units (h / rows each) and mapped one by one, and after a
        // fractional design scale the end of one band and the start of the
        // next could round to different rows: a row with no band at all, the
        // cleared colour showing through, every few rows -- horizontal
        // stripes across Ice Climber's sky at 1280x800 (K-1152).
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let k = self.scale;
        let (x0, y0) = self.map_point(x, y);
        let (x1, y1) = self.map_point(x + w, y + h);
        let (x0, y0, x1, y1) = (x0 * k, y0 * k, x1 * k, y1 * k);
        // fill_rect's own rule, which the letterbox shares: an edge covers
        // the row it falls in, so a far edge at 1051.5 leaves row 1051 to
        // the spare space, which is painted with the design's edge colour.
        let (first, last) = (y0.floor(), y1.floor());
        if last <= first {
            return;
        }
        let span = (y1 - y0).max(f32::EPSILON);
        let mut row = first;
        while row < last {
            let t = ((row + 0.5 - y0) / span).clamp(0.0, 1.0);
            let c = lerp_color(top, bottom, t);
            if let Some(band) = self.clipped(x0, row, x1 - x0, 1.0) {
                fill_rect(&mut self.buffer, self.width, self.height, band, c);
            }
            row += 1.0;
        }
    }

    /// Four thin fills; a stroke is its edges.
    /// The four sides of a rectangle, each drawn as a filled bar.
    ///
    /// The sides are handed to `fill_rect` in the app's OWN coordinates,
    /// because `fill_rect` maps and scales them itself. This function used
    /// to map and scale them first and pass the result on, so every value
    /// went through the transform twice: at 1x that is identity and nothing
    /// showed, and at 2x every border landed at double its position and
    /// double its size. A frame around a plot came out starting at the
    /// canvas edge with its bottom and right sides off-screen, and it took
    /// a screenshot at 2x (K-425) to make a two-year-old bug visible.
    ///
    /// The minimum one-pixel stroke is the one thing that must be computed
    /// in device pixels, so it is converted back into app coordinates.
    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, stroke: f32, color: u32) {
        self.materialize();
        // A hairline must still cover a whole device pixel. `map_len` and
        // the backing scale are what a length is multiplied by on its way
        // down, so dividing by them turns "one device pixel" back into the
        // app-space length that produces one.
        let per_unit = self.map_len(1.0) * self.scale;
        let min = if per_unit > 0.0 { 1.0 / per_unit } else { 1.0 };
        let stroke = stroke.max(min);
        self.fill_rect(x, y, w, stroke, color);
        self.fill_rect(x, y + h - stroke, w, stroke, color);
        self.fill_rect(x, y, stroke, h, color);
        self.fill_rect(x + w - stroke, y, stroke, h, color);
    }

    /// Draw a text run with real antialiased vector type.
    ///
    /// Text is laid out by parley from system fonts at the exact requested size
    /// and rasterized with antialiasing -- the same engine the drawn widget
    /// painter uses. This is what makes canvas apps read as modern instead of
    /// pixel-art: the old 5x7 bitmap face, integer-scaled, turned every label
    /// blocky at anything above small sizes. The bitmap font remains as the
    /// fallback for a host with no usable system fonts, so text never silently
    /// disappears.
    ///
    /// `font-family` is accepted and ignored: one good face everywhere beats
    /// honoring a font name on one system.
    pub fn text(&mut self, text: &str, x: f32, y: f32, font_size: f32, color: u32) {
        self.materialize();
        let __op_timer = OpTimer::start(1);
        let _ = &__op_timer;
        self.text_styled(
            text,
            x,
            y,
            font_size,
            color,
            krate_adapter_common::vector_text::CanvasTextStyle::default(),
        )
    }

    /// `text()` with weight, italic, tracking and family. One text path:
    /// the plain call is this call at defaults.
    pub fn text_styled(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        font_size: f32,
        color: u32,
        style: krate_adapter_common::vector_text::CanvasTextStyle,
    ) {
        self.materialize();
        let __op_timer = OpTimer::start(1);
        let _ = &__op_timer;
        let (x, y) = self.map_point(x, y);
        let font_size = self.map_len(font_size);
        let k = self.scale;
        let (x, y, font_size) = (x * k, y * k, font_size * k);
        // The vector path takes the clip as blend bounds. The old guard here
        // cloned the WHOLE canvas before every clipped text run and walked
        // every canvas pixel after it -- ~1.5ms per run, ~200ms for an
        // editor page, 5 fps under a scroll wheel (K-114's second half).
        let clip = self.clip;
        if krate_adapter_common::vector_text::draw_canvas_text_clipped(
            krate_adapter_common::vector_text::CanvasTarget {
                buffer: &mut self.buffer,
                width: self.width,
                height: self.height,
            },
            text,
            x,
            y,
            font_size,
            color,
            style,
            clip,
        ) {
            return;
        }
        // The 5x7 bitmap fallback (no usable system fonts) is not clip-aware;
        // the snapshot guard stays for exactly this rare path.
        let saved = self.clip.map(|_| self.buffer.clone());
        self.draw_bitmap_text_unclipped(text, x, y, font_size, color);
        if let Some(before) = saved {
            for y in 0..self.height {
                for x in 0..self.width {
                    if self.allowed(x, y) {
                        continue;
                    }
                    let idx = (y * self.width + x) as usize;
                    if let (Some(slot), Some(old)) = (self.buffer.get_mut(idx), before.get(idx)) {
                        *slot = *old;
                    }
                }
            }
        }
    }

    fn draw_bitmap_text_unclipped(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        font_size: f32,
        color: u32,
    ) {
        let scale = bitmap_scale(font_size);
        // The WIT origin is the baseline; drawtext takes the top-left corner.
        let top = y - drawtext::text_height(scale) as f32;
        drawtext::draw_text(
            &mut self.buffer,
            self.width,
            self.height,
            (x as i32, top as i32),
            scale,
            color,
            text,
        );
    }

    /// How big `text` will be once `text()` has drawn it.
    ///
    /// Deliberately reads the same two paths in the same order `text()` does:
    /// ask parley first, and when parley cannot produce glyphs -- a host with
    /// no usable system fonts -- measure the 5x7 bitmap face that will draw
    /// instead. A measurement that disagreed with the pixels would be worse
    /// than none, because an app cannot tell it is wrong.
    pub fn measure_text(&self, text: &str, font_size: f32) -> TextMetrics {
        let __op_timer = OpTimer::start(2);
        let _ = &__op_timer;
        self.measure_text_styled(
            text,
            font_size,
            krate_adapter_common::vector_text::CanvasTextStyle::default(),
        )
    }

    /// `measure_text` for a styled run, reading the same layout the styled
    /// draw uses -- a measurement that disagreed with the pixels would be
    /// worse than none.
    pub fn measure_text_styled(
        &self,
        text: &str,
        font_size: f32,
        style: krate_adapter_common::vector_text::CanvasTextStyle,
    ) -> TextMetrics {
        let __op_timer = OpTimer::start(2);
        let _ = &__op_timer;
        if let Some(m) =
            krate_adapter_common::vector_text::measure_canvas_text_styled(text, font_size, style)
        {
            return TextMetrics {
                width: m.width,
                height: m.height,
                ascent: m.ascent,
                descent: m.descent,
            };
        }
        // Bitmap fallback. `text()` puts the cell's bottom on the baseline, so
        // the whole cell is ascent and there is no descent below the line.
        let scale = bitmap_scale(font_size);
        let height = drawtext::text_height(scale) as f32;
        TextMetrics {
            width: drawtext::text_width(text, scale) as f32,
            height,
            ascent: height,
            descent: 0.0,
        }
    }

    /// Draw decoded RGBA into a rectangle, scaled to fit and centred.
    ///
    /// Reuses the painter's own `draw_image`, the same routine that puts a
    /// photo in an image widget -- so a sprite lands with identical scaling
    /// and alpha blending on all three systems, and there is one place where
    /// that behaviour can ever drift.
    pub fn draw_pixels(&mut self, x: f32, y: f32, w: f32, h: f32, image: &ImagePixels) {
        let _effects_timer = OpTimer::start(14);
        self.materialize();
        let (x, y) = self.map_point(x, y);
        let (w, h) = (self.map_len(w), self.map_len(h));
        let k = self.scale;
        let (x, y, w, h) = (x * k, y * k, w * k, h * k);
        // The clip, like every other draw call: an image in a scrolled list
        // painted over the list's header without it (K-1014).
        draw_image(
            &mut self.buffer,
            self.width,
            self.height,
            (x, y, w, h),
            image,
            self.clip,
        );
    }

    /// Blit an image clipped to a rounded rectangle: the photo-card
    /// primitive. Fills the whole `area` (cover, centre-cropped, like CSS
    /// `object-fit: cover`) rather than letterboxing -- a photo card wants
    /// its frame full. Corners take the SDF's coverage, so the crop is
    /// anti-aliased like every other rounded edge here.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_pixels_round(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radii: (f32, f32, f32, f32),
        image: &ImagePixels,
    ) {
        self.materialize();
        if w <= 0.0 || h <= 0.0 || image.width == 0 || image.height == 0 {
            return;
        }
        let (x, y) = self.map_point(x, y);
        let (w, h) = (self.map_len(w), self.map_len(h));
        let k = self.scale;
        let (x, y, w, h) = (x * k, y * k, w * k, h * k);
        let radii = (radii.0 * k, radii.1 * k, radii.2 * k, radii.3 * k);
        // Cover: scale so the image fills the frame, cropping the overflow
        // equally on both sides.
        let scale = (w / image.width as f32).max(h / image.height as f32);
        let src_w = w / scale;
        let src_h = h / scale;
        let src_x = (image.width as f32 - src_w) / 2.0;
        let src_y = (image.height as f32 - src_h) / 2.0;

        let (bw, bh) = (self.width, self.height);
        let x0 = (x.floor().max(0.0) as u32).min(bw);
        let x1 = ((x + w).ceil().max(0.0) as u32).min(bw);
        let y0 = (y.floor().max(0.0) as u32).min(bh);
        let y1 = ((y + h).ceil().max(0.0) as u32).min(bh);
        // Corners only exist in the top and bottom radius bands. Rows
        // between them are plain rectangles: coverage is the distance to
        // the straight edges, no SDF -- and a fully covered, fully opaque
        // sample is a direct store. At phone resolution this loop is the
        // photo card's whole cost, and the SDF per interior pixel was most
        // of it (K-090).
        let top_band = y + radii.0.max(radii.1);
        let bottom_band = y + h - radii.2.max(radii.3);
        for py in y0..y1 {
            let fy = py as f32 + 0.5;
            let plain_row = fy >= top_band && fy <= bottom_band;
            for px in x0..x1 {
                let fx = px as f32 + 0.5;
                let coverage = if plain_row {
                    let d = (x - fx).max(fx - (x + w)).max(y - fy).max(fy - (y + h));
                    (0.5 - d).clamp(0.0, 1.0)
                } else {
                    let d = Self::round_rect_sdf(fx, fy, x, y, w, h, radii);
                    (0.5 - d).clamp(0.0, 1.0)
                };
                if coverage <= 0.0 {
                    continue;
                }
                let u = src_x + (fx - x) / w * src_w;
                let v = src_y + (fy - y) / h * src_h;
                let sampled = sample_bilinear(image, u, v);
                if coverage >= 1.0 && sampled >> 24 == 0xFF && self.allowed(px, py) {
                    self.buffer[(py * self.width + px) as usize] = sampled;
                } else {
                    self.blend_coverage(px, py, sampled, coverage);
                }
            }
        }
    }

    /// Draw a decoded RGBA sprite centred at `(cx, cy)`, scaled to `dst_w` x
    /// `dst_h`, rotated by `angle` radians (clockwise), and alpha-blended over
    /// the canvas. This is the primitive a real sprite game needs: a ship that
    /// points where it flies, a spinning asteroid, a rotating shield.
    ///
    /// Implemented by inverse mapping -- for each destination pixel in the
    /// rotated bounding box, rotate it back into the sprite's own texture space
    /// and sample there. That guarantees every destination pixel is written at
    /// most once (no gaps or double-blends a forward map would leave) and lets
    /// the source be sampled with bilinear smoothing so the sprite does not
    /// shimmer as it turns.
    pub fn draw_sprite(
        &mut self,
        cx: f32,
        cy: f32,
        dst_w: f32,
        dst_h: f32,
        angle: f32,
        image: &ImagePixels,
    ) {
        let _effects_timer = OpTimer::start(15);
        self.materialize();
        let (cx, cy) = self.map_point(cx, cy);
        let (dst_w, dst_h) = (self.map_len(dst_w), self.map_len(dst_h));
        let k = self.scale;
        let (cx, cy, dst_w, dst_h) = (cx * k, cy * k, dst_w * k, dst_h * k);
        let (iw, ih) = (image.width, image.height);
        if iw == 0 || ih == 0 || dst_w <= 0.0 || dst_h <= 0.0 {
            return;
        }
        // A hostile or absurd request draws nothing rather than allocating or
        // looping forever: cap the drawn size to the canvas plus a margin.
        let max_edge = (self.width.max(self.height) as f32) * 2.0 + 4.0;
        if !dst_w.is_finite()
            || !dst_h.is_finite()
            || !angle.is_finite()
            || !cx.is_finite()
            || !cy.is_finite()
            || dst_w > max_edge
            || dst_h > max_edge
        {
            return;
        }

        let (sin_a, cos_a) = (angle.sin(), angle.cos());
        // The rotated sprite's axis-aligned bounding box, half extents.
        let hw = dst_w * 0.5;
        let hh = dst_h * 0.5;
        let bb_x = hw * cos_a.abs() + hh * sin_a.abs();
        let bb_y = hw * sin_a.abs() + hh * cos_a.abs();
        let x0 = ((cx - bb_x).floor().max(0.0) as u32).min(self.width);
        let x1 = (((cx + bb_x).ceil()).max(0.0) as u32).min(self.width);
        let y0 = ((cy - bb_y).floor().max(0.0) as u32).min(self.height);
        let y1 = (((cy + bb_y).ceil()).max(0.0) as u32).min(self.height);

        // Scale from destination pixels back to source texels.
        let sx_scale = iw as f32 / dst_w;
        let sy_scale = ih as f32 / dst_h;

        for py in y0..y1 {
            for px in x0..x1 {
                // Destination offset from the sprite centre.
                let ddx = px as f32 + 0.5 - cx;
                let ddy = py as f32 + 0.5 - cy;
                // Inverse-rotate into the sprite's own (unrotated) frame.
                let lx = ddx * cos_a + ddy * sin_a;
                let ly = -ddx * sin_a + ddy * cos_a;
                // Only pixels inside the sprite's own rectangle.
                if lx < -hw || lx >= hw || ly < -hh || ly >= hh {
                    continue;
                }
                // Map to source texel coordinates.
                let su = (lx + hw) * sx_scale;
                let sv = (ly + hh) * sy_scale;
                let sample = sample_bilinear(image, su, sv);
                if (sample >> 24) == 0 {
                    continue;
                }
                let idx = (py * self.width + px) as usize;
                if !self.allowed(px, py) {
                    continue;
                }
                if let Some(slot) = self.buffer.get_mut(idx) {
                    *slot = krate_adapter_common::painter::blend_over(sample, *slot);
                }
            }
        }
    }

    /// The canvas as the image pipeline's pixel format.
    /// The surface's size in pixels, which is its size in logical points too
    /// -- the canvas renders at 1x, and the display scale is applied when the
    /// host lifts the image onto the screen.
    /// The logical size -- the coordinate space the app draws in. The
    /// physical buffer behind it is this times `scale`.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.logical_width, self.logical_height)
    }

    #[allow(dead_code)]
    fn physical_dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Draw a picture the host owns, keeping it whole when it is the frame.
    ///
    /// The fast path needs all of: no clip, the picture covering the whole
    /// buffer once mapped (no letterbox bars it would leave unpainted), and
    /// every pixel opaque (nothing beneath it can show through). Otherwise
    /// it is an ordinary `draw_pixels`.
    pub fn draw_pixels_owned(&mut self, x: f32, y: f32, w: f32, h: f32, image: ImagePixels) {
        let _effects_timer = OpTimer::start(16);
        // Same proportions as the area, too: drawn, a picture keeps its
        // shape and letterboxes inside the area, while a display stretches a
        // frame to fit -- so only a picture that already fits may skip it.
        let same_shape = w > 0.0
            && h > 0.0
            && ((image.width as f32 / image.height.max(1) as f32) / (w / h) - 1.0).abs() < 0.01;
        if self.clip.is_none()
            && same_shape
            && self.covers_buffer(x, y, w, h)
            && image.rgba.chunks_exact(4).all(|px| px[3] == 255)
        {
            self.pending = Some(image);
            return;
        }
        self.draw_pixels(x, y, w, h, &image);
    }

    fn covers_buffer(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        let (x, y) = self.map_point(x, y);
        let (w, h) = (self.map_len(w), self.map_len(h));
        let k = self.scale;
        x * k <= 0.5
            && y * k <= 0.5
            && (x + w) * k >= self.width as f32 - 0.5
            && (y + h) * k >= self.height as f32 - 0.5
    }

    /// The raster, made on first use: opaque white, so a canvas an app
    /// forgets to clear reads as a blank sheet rather than a black hole in
    /// the window.
    fn ensure_buffer(&mut self) {
        if self.buffer.is_empty() {
            self.buffer = vec![0xFFFF_FFFF; self.width as usize * self.height as usize];
        }
    }

    /// Raster a kept picture before anything is drawn over it.
    fn materialize(&mut self) {
        self.ensure_buffer();
        if let Some(image) = self.pending.take() {
            draw_image(
                &mut self.buffer,
                self.width,
                self.height,
                (0.0, 0.0, self.width as f32, self.height as f32),
                &image,
                None,
            );
        }
    }

    pub fn to_image(&self) -> Result<ImagePixels, UiAdapterError> {
        if let Some(image) = &self.pending {
            return Ok(image.clone());
        }
        if self.buffer.is_empty() {
            // Nothing drawn yet: the blank white sheet the buffer would be.
            let rgba = vec![0xFF; self.width as usize * self.height as usize * 4];
            return ImagePixels::new(self.width, self.height, rgba);
        }
        // Chunked writes instead of four pushes per pixel: every push
        // carries a capacity check, and this conversion runs once per
        // frame over the whole canvas -- it was the single hottest line
        // in a full-window game's profile. The chunked form vectorizes.
        let mut rgba = vec![0u8; self.buffer.len() * 4];
        for (chunk, word) in rgba.chunks_exact_mut(4).zip(self.buffer.iter()) {
            chunk[0] = (word >> 16) as u8;
            chunk[1] = (word >> 8) as u8;
            chunk[2] = *word as u8;
            chunk[3] = (word >> 24) as u8;
        }
        self.blend_letterbox(&mut rgba);
        ImagePixels::new(self.width, self.height, rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A background filled to the canvas's edge reaches its last pixel
    /// column and row, though the canvas is a fraction of a point wider than
    /// the whole points it was laid out in (2.625 pixels a point).
    #[test]
    fn a_fill_to_the_edge_reaches_the_last_pixel() {
        let mut surface = CanvasSurface::new_scaled(411, 20, 2.625).expect("surface");
        let (pw, ph) = (surface.width, surface.height);
        surface.clear(0xFF_00_00_FF);
        surface.fill_rect(0.0, 0.0, 411.0, 20.0, 0xFF_FF_FF_FF);
        let image = surface.to_image().expect("image");
        let last = ((ph as usize - 1) * pw as usize + pw as usize - 1) * 4;
        assert_eq!(
            &image.rgba[last..last + 4],
            &[255, 255, 255, 255],
            "the last pixel is filled"
        );
        // A fill well short of the edge stays where it was put.
        surface.fill_rect(0.0, 0.0, 200.0, 20.0, 0xFF_00_FF_00);
        let image = surface.to_image().expect("image");
        assert_eq!(&image.rgba[last..last + 4], &[255, 255, 255, 255]);
    }

    /// K-1212: the table-driven radial gradient paints what the per-pixel
    /// float version painted, within a couple of levels per channel, at
    /// any clip, and draws nothing outside its circle.
    #[test]
    fn the_fast_radial_gradient_matches_the_float_one() {
        let (w, h) = (97u32, 61u32);
        let (cx, cy, r) = (40.3f32, 22.7f32, 37.5f32);
        let (inner, outer) = (0xB0_3A_4D_F2u32, 0x00_3A_4D_F2u32);
        let mut surface = CanvasSurface::new(w, h).expect("surface");
        surface.clear(0xFF_F0_F1_F4);
        surface.radial_gradient(cx, cy, r, inner, outer);
        let fast = surface.to_image().expect("image").rgba;
        // The float version, as it was.
        let mut want = vec![0xFF_F0_F1_F4u32; (w * h) as usize];
        for py in 0..h {
            for px in 0..w {
                let (dx, dy) = (px as f32 + 0.5 - cx, py as f32 + 0.5 - cy);
                let dist = (dx * dx + dy * dy).sqrt();
                if dist > r {
                    continue;
                }
                let t = dist / r;
                let t = t * t * (3.0 - 2.0 * t);
                let c = lerp_color_dithered(inner, outer, t, px, py);
                if (c >> 24) == 0 {
                    continue;
                }
                let slot = &mut want[(py * w + px) as usize];
                *slot = krate_adapter_common::painter::blend_over(c, *slot);
            }
        }
        let mut worst = 0i32;
        for (i, px) in want.iter().enumerate() {
            for (c, shift) in [16u32, 8, 0].iter().enumerate() {
                let a = ((px >> shift) & 0xFF) as i32;
                let b = i32::from(fast[i * 4 + c]);
                worst = worst.max((a - b).abs());
            }
        }
        assert!(worst <= 2, "worst channel difference {worst}");
        assert_ne!(
            fast[(22 * w + 40) as usize * 4 + 2],
            0xF4,
            "the fixture must paint the centre"
        );
    }

    /// A stroked rectangle is closed, and stays inside the canvas at 2x.
    ///
    /// `stroke_rect` used to map and scale its own coordinates and then
    /// hand them to `fill_rect`, which maps and scales them again. At 1x
    /// the transform is identity and the double application showed
    /// nothing, so this survived every test and every screenshot until a
    /// 2x shot was taken (K-425). At 2x a frame around a plot was drawn at
    /// double its position and size: it began at the canvas edge and its
    /// bottom and right sides fell off the surface entirely.
    ///
    /// Asserted on all four sides, because the failure left TWO of them
    /// looking perfectly correct.
    /// An image respects the clip like every other draw call (K-1014).
    #[test]
    fn an_image_stays_inside_the_clip() {
        let mut s = CanvasSurface::new_scaled(96, 60, 2.0).expect("surface");
        s.clear(pack_color(0.0, 0.0, 0.0, 1.0));
        s.set_clip(Some((0.0, 0.0, 50.0, 60.0)));
        let rgba = (0..96 * 60).flat_map(|_| [200u8, 40, 10, 255]).collect();
        let image = ImagePixels::new(96, 60, rgba).expect("image");
        s.draw_pixels(0.0, 0.0, 96.0, 60.0, &image);
        let out = s.to_image().expect("image");
        let at = |x: usize, y: usize| &out.rgba[(y * 192 + x) * 4..(y * 192 + x) * 4 + 4];
        assert_eq!(
            at(20, 20),
            &[200, 40, 10, 255],
            "inside the clip is painted"
        );
        assert_eq!(at(150, 20), &[0, 0, 0, 255], "outside the clip is not");
    }

    /// A frame that is one opaque picture covering the canvas goes out at
    /// the picture's own size; anything else, or anything drawn after it,
    /// rasters exactly as before.
    /// A design-size app looks and hit-tests the same on every path: the
    /// list the iPhone's GPU draws from maps design space exactly as this
    /// raster does. They once disagreed -- the GPU drew design coordinates
    /// unscaled while taps were mapped into design space -- and a calculator
    /// on a phone was too small and answered touches in the wrong places.
    /// Smart fit: an app designed at 320x470 on a phone-shaped 390x844
    /// canvas does not sit between bars. The spare space above takes the
    /// colour along the design's top edge and the space below its bottom
    /// edge, on the CPU raster and in the list the iPhone's GPU draws from.
    /// The rounded fill's and the shadow's fast inside path and clip bounds
    /// paint exactly the pixels the per-pixel distance painted, for every
    /// shape, radius, colour, clip and scale tried.
    #[test]
    fn rounded_cards_and_shadows_paint_the_same_pixels_faster() {
        // A card: x, y, w, h, corner radii, blur, colour.
        type Card = (f32, f32, f32, f32, (f32, f32, f32, f32), f32, u32);
        // The old loops, kept here as the oracle.
        fn reference(s: &mut CanvasSurface, shadow: bool, r: Card) {
            let (x, y, w, h, radii, blur, color) = r;
            s.materialize();
            let (x, y) = s.map_point(x, y);
            let k = s.scale;
            let (x, y, w, h) = (x * k, y * k, s.map_len(w) * k, s.map_len(h) * k);
            let blur = (s.map_len(blur) * k).max(0.5);
            let radii = if shadow {
                radii
            } else {
                (radii.0 * k, radii.1 * k, radii.2 * k, radii.3 * k)
            };
            let pad = if shadow { blur } else { 0.0 };
            let (bw, bh) = (s.width, s.height);
            let x0 = ((x - pad).floor().max(0.0) as u32).min(bw);
            let x1 = ((x + w + pad).ceil().max(0.0) as u32).min(bw);
            let y0 = ((y - pad).floor().max(0.0) as u32).min(bh);
            let y1 = ((y + h + pad).ceil().max(0.0) as u32).min(bh);
            for py in y0..y1 {
                for px in x0..x1 {
                    let d = CanvasSurface::round_rect_sdf(
                        px as f32 + 0.5,
                        py as f32 + 0.5,
                        x,
                        y,
                        w,
                        h,
                        radii,
                    );
                    let c = if shadow {
                        let t = ((blur - d) / (2.0 * blur)).clamp(0.0, 1.0);
                        t * t * (3.0 - 2.0 * t)
                    } else {
                        (0.5 - d).clamp(0.0, 1.0)
                    };
                    s.blend_coverage(px, py, color, c);
                }
            }
        }
        let shapes = [
            (
                10.3,
                7.7,
                200.0,
                120.0,
                (12.0, 12.0, 12.0, 12.0),
                8.0,
                0xFF22_3344u32,
            ),
            (
                0.0,
                0.0,
                320.0,
                64.0,
                (20.0, 4.0, 0.0, 30.0),
                3.0,
                0x8811_AA55,
            ),
            (
                -30.0,
                50.5,
                120.0,
                400.0,
                (60.0, 60.0, 60.0, 60.0),
                0.3,
                0xFFFF_FFFF,
            ),
            (
                40.25,
                40.75,
                3.0,
                2.0,
                (1.0, 1.0, 1.0, 1.0),
                12.0,
                0x40FF_0000,
            ),
            (
                5.0,
                5.0,
                300.0,
                300.0,
                (0.0, 0.0, 0.0, 0.0),
                1.0,
                0xFF00_00FF,
            ),
        ];
        for scale in [1.0f32, 1.5, 2.0] {
            for clip in [None, Some((33.3, 21.0, 150.7, 90.2))] {
                for shadow in [false, true] {
                    for shape in shapes {
                        let mut fast = CanvasSurface::new_scaled(360, 480, scale).expect("surface");
                        let mut slow = CanvasSurface::new_scaled(360, 480, scale).expect("surface");
                        for s in [&mut fast, &mut slow] {
                            s.clear(0xFF10_1010);
                            s.set_clip(clip);
                        }
                        let (x, y, w, h, radii, blur, color) = shape;
                        if shadow {
                            fast.drop_shadow_round_rect(x, y, w, h, radii, blur, color);
                        } else {
                            fast.fill_round_rect(x, y, w, h, radii, color);
                        }
                        reference(&mut slow, shadow, shape);
                        let diff = fast
                            .buffer
                            .iter()
                            .zip(&slow.buffer)
                            .filter(|(a, b)| a != b)
                            .count();
                        assert_eq!(
                            diff, 0,
                            "scale {scale} clip {clip:?} shadow {shadow} shape {shape:?}"
                        );
                    }
                }
            }
        }
    }

    /// K-1152: a sky drawn with linear_gradient_v under a fractional design
    /// scale has no row left unpainted. Cut in app units and mapped band by
    /// band, the cleared colour showed through every few rows: white stripes
    /// across Ice Climber at 1280x800.
    #[test]
    fn a_scaled_gradient_paints_every_row() {
        // (logical size, backing scale, design): Ice Climber's own 256x240
        // on a 1280x744 window at 2x is the case that striped.
        for (w, h, scale, dw, dh) in [
            (1280, 744, 2.0, 256.0, 240.0),
            (1280, 744, 1.0, 256.0, 240.0),
            (1280, 800, 1.0, 420.0, 760.0),
            (1003, 701, 1.5, 333.0, 517.0),
        ] {
            let mut surface = CanvasSurface::new_scaled(w, h, scale).expect("surface");
            surface.clear(0xFFFF_FFFF);
            surface.set_design_size(dw, dh);
            surface.linear_gradient_v(0.0, 0.0, dw, dh, 0xFF10_2040, 0xFF20_4080);
            let image = surface.to_image().expect("image");
            let x = image.width / 2;
            let white: Vec<u32> = (0..image.height)
                .filter(|&y| {
                    let i = ((y * image.width + x) * 4) as usize;
                    image.rgba[i] == 255 && image.rgba[i + 1] == 255 && image.rgba[i + 2] == 255
                })
                .collect();
            assert!(
                white.is_empty(),
                "{w}x{h} design {dw}x{dh}: unpainted rows {white:?}"
            );
        }
    }

    /// K-1155: a 256x240 design on a 1280x744 window at 2x fills its spare
    /// space with the colour of its own edges on both sides, never with a
    /// column no fill reached.
    #[test]
    fn both_sides_of_a_wide_letterbox_take_the_design_edge() {
        let mut surface = CanvasSurface::new_scaled(1280, 744, 2.0).expect("surface");
        surface.clear(0xFFFF_FFFF);
        surface.set_design_size(256.0, 240.0);
        surface.fill_rect(0.0, 0.0, 256.0, 240.0, 0xFF10_2040);
        let image = surface.to_image().expect("image");
        let px = |x: u32, y: u32| {
            let i = ((y * image.width + x) * 4) as usize;
            [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]]
        };
        for y in [10, image.height / 2, image.height - 10] {
            assert_eq!(px(5, y), [0x10, 0x20, 0x40], "left of the design, row {y}");
            assert_eq!(
                px(image.width - 5, y),
                [0x10, 0x20, 0x40],
                "right of the design, row {y}"
            );
        }
    }

    #[test]
    fn the_spare_space_around_a_design_takes_its_edge_colours() {
        let (top, bottom) = (0xFF13_1620u32, 0xFF0B_0D14u32);
        let mut surface = CanvasSurface::new(390, 844).expect("surface");
        surface.set_design_size(320.0, 470.0);
        surface.linear_gradient_v(0.0, 0.0, 320.0, 470.0, top, bottom);
        let image = surface.to_image().expect("image");
        let px = |x: u32, y: u32| {
            let i = ((y * image.width + x) * 4) as usize;
            [image.rgba[i], image.rgba[i + 1], image.rgba[i + 2]]
        };
        let near = |p: [u8; 3], c: u32| {
            let want = [(c >> 16) as u8, (c >> 8) as u8, c as u8];
            p.iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 3)
        };
        // Above and below the design, across the full width.
        assert!(near(px(2, 2), top), "top bar {:?}", px(2, 2));
        assert!(near(px(387, 2), top), "top bar right {:?}", px(387, 2));
        assert!(near(px(2, 841), bottom), "bottom bar {:?}", px(2, 841));

        let list = krate_adapter_common::canvas_list::CanvasList {
            logical_width: 390,
            logical_height: 844,
            design: Some((320.0, 470.0)),
            design_inset: (0.0, 0.0, 0.0, 0.0),
            ops: vec![
                krate_adapter_common::canvas_list::CanvasOp::LinearGradient {
                    rect: (0.0, 0.0, 320.0, 470.0),
                    top,
                    bottom,
                },
            ],
        };
        let edges = list
            .edge_colors()
            .expect("the background call gives the edges");
        assert_eq!((edges.top, edges.bottom), (top, bottom));
    }

    #[test]
    fn the_gpu_list_maps_design_space_exactly_as_the_raster_does() {
        for (canvas, design) in [((390, 844), (360.0, 560.0)), ((844, 390), (360.0, 560.0))] {
            let mut surface = CanvasSurface::new(canvas.0, canvas.1).expect("surface");
            surface.set_design_size(design.0, design.1);
            // With covered edges too (K-1210): both paths fit inside them.
            surface.set_design_inset((47.0, 0.0, 34.0, 12.0));
            let list = krate_adapter_common::canvas_list::CanvasList {
                logical_width: canvas.0,
                logical_height: canvas.1,
                design: surface.design_size(),
                design_inset: surface.design_inset(),
                ops: Vec::new(),
            };
            assert_eq!(
                list.design_transform(),
                surface.design_transform(),
                "{canvas:?}"
            );
            assert_ne!(
                list.design_transform(),
                (1.0, 0.0, 0.0),
                "the fixture must letterbox"
            );
        }
    }

    #[test]
    fn a_whole_frame_picture_is_kept_at_its_own_size() {
        let frame = |w: u32, h: u32, alpha: u8| {
            let rgba = (0..w * h).flat_map(|_| [200u8, 40, 10, alpha]).collect();
            ImagePixels::new(w, h, rgba).expect("image")
        };

        // A game's frame on a 2x canvas: kept, published at 96x60.
        let mut s = CanvasSurface::new_scaled(128, 80, 2.0).expect("surface");
        s.set_design_size(96.0, 60.0);
        s.draw_pixels_owned(0.0, 0.0, 96.0, 60.0, frame(96, 60, 255));
        let out = s.to_image().expect("image");
        assert_eq!((out.width, out.height), (96, 60), "kept at its own size");
        assert_eq!(&out.rgba[..4], &[200, 40, 10, 255]);
        assert!(s.buffer.is_empty(), "a kept frame never needs the raster");

        // Something drawn over it: the picture is rastered first, then the
        // rect, at the physical size -- what drawing it would have made.
        s.fill_rect(0.0, 0.0, 10.0, 10.0, pack_color(0.0, 0.0, 1.0, 1.0));
        let out = s.to_image().expect("image");
        assert_eq!(
            (out.width, out.height),
            (256, 160),
            "rastered once drawn over"
        );
        let far = ((150 * 256 + 250) * 4) as usize;
        assert_eq!(
            &out.rgba[far..far + 4],
            &[200, 40, 10, 255],
            "the picture is under it"
        );
        assert_eq!(&out.rgba[..4], &[0, 0, 255, 255], "and the rect on top");

        // A translucent picture shows what is beneath: not kept.
        let mut s = CanvasSurface::new_scaled(96, 60, 2.0).expect("surface");
        s.draw_pixels_owned(0.0, 0.0, 96.0, 60.0, frame(96, 60, 128));
        assert_eq!(s.to_image().expect("image").width, 192);

        // A design size of another shape leaves letterbox bars the picture
        // does not cover: not kept.
        let mut s = CanvasSurface::new_scaled(200, 100, 1.0).expect("surface");
        s.set_design_size(100.0, 100.0);
        s.draw_pixels_owned(0.0, 0.0, 100.0, 100.0, frame(100, 100, 255));
        assert_eq!(s.to_image().expect("image").width, 200);

        // A picture of another shape than its area letterboxes when drawn:
        // not kept, or the display would stretch it instead.
        let mut s = CanvasSurface::new_scaled(96, 60, 2.0).expect("surface");
        s.draw_pixels_owned(0.0, 0.0, 96.0, 60.0, frame(60, 60, 255));
        assert_eq!(s.to_image().expect("image").width, 192);

        // A clip limits what may be painted: not kept.
        let mut s = CanvasSurface::new_scaled(96, 60, 2.0).expect("surface");
        s.set_clip(Some((0.0, 0.0, 50.0, 60.0)));
        s.draw_pixels_owned(0.0, 0.0, 96.0, 60.0, frame(96, 60, 255));
        assert_eq!(s.to_image().expect("image").width, 192);
        assert!(s.pending.is_none(), "a clipped canvas is never kept");
    }

    #[test]
    fn a_stroked_rect_is_closed_and_in_bounds_at_two_x() {
        let mut s = CanvasSurface::new_scaled(100, 50, 2.0).expect("surface");
        // A frame inset ten logical pixels from every edge.
        s.stroke_rect(10.0, 10.0, 80.0, 30.0, 1.0, 0xFF00_0000);
        let image = s.to_image().expect("image");
        assert_eq!((image.width, image.height), (200, 100));
        let px = |x: usize, y: usize| image.rgba[(y * 200 + x) * 4];

        // Every side is on the physical pixel its logical position names.
        assert_eq!(px(100, 20), 0, "the top side is drawn");
        assert_eq!(px(100, 79), 0, "the bottom side is drawn");
        assert_eq!(px(20, 50), 0, "the left side is drawn");
        assert_eq!(px(179, 50), 0, "the right side is drawn");

        // And the middle is empty: a stroke is not a fill.
        assert_eq!(px(100, 50), 255, "the inside is not filled");

        // Nothing is drawn outside the frame it asked for. The doubled
        // transform put the right side at x=360 on a 200px buffer, so it
        // vanished; this is the assertion that would have caught it.
        assert_eq!(px(5, 50), 255, "nothing left of the frame");
        assert_eq!(px(195, 50), 255, "nothing right of the frame");
    }

    /// K-088's lock: a scaled surface reports logical size, rasters
    /// physical pixels, and puts a logical-coordinate fill exactly where
    /// the physical buffer says it should be.
    #[test]
    fn a_scaled_surface_rasters_physical_and_reports_logical() {
        let mut s = CanvasSurface::new_scaled(100, 50, 2.0).expect("surface");
        assert_eq!(s.dimensions(), (100, 50), "apps see logical size");
        assert_eq!(s.physical_dimensions(), (200, 100), "buffer is physical");

        // A fill at logical (10, 10, 5, 5) lands at physical (20, 20, 10, 10).
        s.fill_rect(10.0, 10.0, 5.0, 5.0, 0xFF00_0000);
        let image = s.to_image().expect("image");
        assert_eq!((image.width, image.height), (200, 100));
        let px = |x: usize, y: usize| image.rgba[(y * 200 + x) * 4];
        assert_eq!(px(25, 25), 0, "inside the scaled fill is black");
        assert_eq!(px(15, 25), 255, "left of the scaled fill is untouched");
        assert_eq!(px(35, 25), 255, "right of the scaled fill is untouched");

        // The one-stop gradient delegates to fill_rect on raw arguments;
        // if scaling ever runs twice the fill lands at (40, 40) instead.
        let mut g = CanvasSurface::new_scaled(100, 50, 2.0).expect("surface");
        g.linear_gradient_stops(10.0, 10.0, 5.0, 5.0, 0.0, &[(0.0, 0xFF00_0000)]);
        let gi = g.to_image().expect("image");
        let gpx = |x: usize, y: usize| gi.rgba[(y * 200 + x) * 4];
        assert_eq!(gpx(25, 25), 0, "delegation scales exactly once");
        assert_eq!(gpx(45, 45), 255, "a double-scale would land here");
    }

    /// The arc lights its sweep and only its sweep: a quarter arc from 12
    /// o'clock leaves 9 o'clock dark. Full sweep degrades to the circle.
    #[test]
    fn an_arc_covers_its_sweep_and_nothing_else() {
        let mut s = CanvasSurface::new(100, 100).expect("surface");
        s.clear(0xFF00_0000);
        // From 12 o'clock (-90), a quarter turn clockwise ends at 3 o'clock.
        s.stroke_arc(50.0, 50.0, 30.0, -90.0, 90.0, 5.0, 0xFFFF_FFFF);
        let at = |x: u32, y: u32| s.buffer[(y * 100 + x) as usize] & 0x00FF_FFFF;
        assert!(at(50, 20) > 0x0080_8080, "12 o'clock is on the arc");
        assert!(at(80, 50) > 0x0080_8080, "3 o'clock is on the arc");
        assert_eq!(at(20, 50), 0, "9 o'clock is dark");
        assert_eq!(at(50, 80), 0, "6 o'clock is dark");
    }

    /// Styled text really changes the pixels: bold covers more than thin at
    /// the same size, and tracking widens the measured run. Guarded so a
    /// host with no system fonts (bitmap fallback) skips rather than lies.
    #[test]
    fn text_style_reaches_the_glyphs() {
        use krate_adapter_common::vector_text::{CanvasFontFamily, CanvasTextStyle};
        let s = CanvasSurface::new(300, 60).expect("surface");
        let plain = s.measure_text("Workouts", 24.0);
        if plain.width == 0.0 {
            return; // no system fonts here; the fallback has no weights
        }
        let spaced = s.measure_text_styled(
            "Workouts",
            24.0,
            CanvasTextStyle {
                letter_spacing: 3.0,
                ..CanvasTextStyle::default()
            },
        );
        assert!(
            spaced.width > plain.width + 10.0,
            "tracking must widen the run: {} vs {}",
            spaced.width,
            plain.width
        );
        let mono = s.measure_text_styled(
            "iiii",
            24.0,
            CanvasTextStyle {
                family: CanvasFontFamily::Mono,
                ..CanvasTextStyle::default()
            },
        );
        let sans = s.measure_text("iiii", 24.0);
        assert!(
            (mono.width - sans.width).abs() > 1.0,
            "mono and sans must differ on iiii: {} vs {}",
            mono.width,
            sans.width
        );
    }

    /// The rounded blit crops the corner and fills the frame: corner pixel
    /// untouched, centre carrying image color, cover-scaled.
    #[test]
    fn a_rounded_blit_crops_its_corners() {
        let mut s = CanvasSurface::new(100, 100).expect("surface");
        s.clear(0xFF00_0000);
        // A solid green source image.
        let rgba = alloc_image(8, 8, [0u8, 255, 0, 255]);
        let image = ImagePixels::new(8, 8, rgba).expect("image");
        s.draw_pixels_round(10.0, 10.0, 80.0, 80.0, (20.0, 20.0, 20.0, 20.0), &image);
        let at = |x: u32, y: u32| s.buffer[(y * 100 + x) as usize];
        assert_eq!((at(50, 50) >> 8) & 0xFF, 255, "centre is image green");
        assert_eq!(at(11, 11) & 0x00FF_FFFF, 0, "corner cropped");
        assert_eq!((at(50, 11) >> 8) & 0xFF, 255, "edge midpoint filled");
    }

    fn alloc_image(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            v.extend_from_slice(&px);
        }
        v
    }

    /// The card primitive must actually round its corners: the very corner
    /// pixel stays untouched while the centre and edge midpoints fill.
    #[test]
    fn a_round_rect_fills_the_middle_and_spares_the_corner() {
        let at = |s: &CanvasSurface, x: u32, y: u32| s.buffer[(y * 100 + x) as usize];
        let mut s = CanvasSurface::new(100, 100).expect("surface");
        s.clear(0xFF00_0000);
        s.fill_round_rect(
            10.0,
            10.0,
            80.0,
            80.0,
            (20.0, 20.0, 20.0, 20.0),
            0xFFFF_FFFF,
        );
        assert_eq!(at(&s, 50, 50) & 0x00FF_FFFF, 0x00FF_FFFF, "centre filled");
        assert_eq!(
            at(&s, 50, 11) & 0x00FF_FFFF,
            0x00FF_FFFF,
            "edge midpoint filled"
        );
        assert_eq!(at(&s, 11, 11) & 0x00FF_FFFF, 0x0000_0000, "corner spared");

        // Per-corner: square off only the top-left and that corner fills.
        let mut sq = CanvasSurface::new(100, 100).expect("surface");
        sq.clear(0xFF00_0000);
        sq.fill_round_rect(10.0, 10.0, 80.0, 80.0, (0.0, 20.0, 20.0, 20.0), 0xFFFF_FFFF);
        assert_eq!(
            at(&sq, 11, 11) & 0x00FF_FFFF,
            0x00FF_FFFF,
            "squared corner fills"
        );
    }

    /// The stroke is a ring: on the edge yes, inside the card no.
    #[test]
    fn a_round_rect_stroke_is_hollow() {
        let at = |s: &CanvasSurface, x: u32, y: u32| s.buffer[(y * 100 + x) as usize];
        let mut s = CanvasSurface::new(100, 100).expect("surface");
        s.clear(0xFF00_0000);
        s.stroke_round_rect(
            10.0,
            10.0,
            80.0,
            80.0,
            (12.0, 12.0, 12.0, 12.0),
            3.0,
            0xFFFF_FFFF,
        );
        assert_eq!(at(&s, 50, 10) & 0x00FF_FFFF, 0x00FF_FFFF, "on the edge");
        assert_eq!(at(&s, 50, 50) & 0x00FF_FFFF, 0x0000_0000, "hollow centre");
    }

    /// A shadow must fade with distance, monotonically: strongest under the
    /// card, weaker outward, gone past the blur. A flat blob fails this.
    #[test]
    fn a_drop_shadow_fades_outward() {
        let mut s = CanvasSurface::new(200, 200).expect("surface");
        s.clear(0xFF00_0000);
        s.drop_shadow_round_rect(
            60.0,
            60.0,
            80.0,
            80.0,
            (12.0, 12.0, 12.0, 12.0),
            16.0,
            0xFFFF_FFFF,
        );
        let red = |x: u32| (s.buffer[(100 * 200 + x) as usize] >> 16) & 0xFF;
        assert!(red(100) > red(52), "under the card beats near the edge");
        assert!(red(52) > red(40), "near beats far");
        assert_eq!(red(20), 0, "past the blur there is nothing");
    }

    /// Three stops at 0 degrees: each colour lands where its offset says,
    /// left to right. One stop degrades to a plain fill, not a panic.
    #[test]
    fn gradient_stops_land_where_their_offsets_say() {
        let mut s = CanvasSurface::new(100, 100).expect("surface");
        s.clear(0xFF00_0000);
        let stops = [
            (0.0, 0xFFFF_0000u32),
            (0.5, 0xFF00_FF00),
            (1.0, 0xFF00_00FF),
        ];
        s.linear_gradient_stops(0.0, 0.0, 100.0, 100.0, 0.0, &stops);
        let px = |x: u32, y: u32| s.buffer[(y * 100 + x) as usize];
        assert!((px(1, 50) >> 16) & 0xFF > 240, "left is red");
        assert!((px(50, 50) >> 8) & 0xFF > 240, "middle is green");
        assert!(px(98, 50) & 0xFF > 240, "right is blue");

        let mut one = CanvasSurface::new(10, 10).expect("surface");
        one.linear_gradient_stops(0.0, 0.0, 10.0, 10.0, 0.0, &[(0.0, 0xFFAB_CDEF)]);
        assert_eq!(
            one.buffer[55] & 0x00FF_FFFF,
            0x00AB_CDEF,
            "one stop is a fill"
        );
    }

    #[test]
    fn resize_refits_the_buffer_and_reports_the_new_size() {
        // A canvas is bound once and the window is resizable, so the surface
        // has to follow the widget. When it did not, `canvas_size` kept
        // answering with the bind-time size forever: an app that lays out from
        // that answer draws to the wrong extent and every hit-box is off. That
        // is the resize bug, and this is the line that stops it coming back.
        let mut surface = CanvasSurface::new(440, 620).expect("surface");
        assert_eq!(surface.dimensions(), (440, 620));

        assert!(surface.resize(900, 500).expect("grow"));
        assert_eq!(surface.dimensions(), (900, 500));
        // The raster really is the new size, not just the reported number:
        // what is published, and what a draw writes into.
        let image = surface.to_image().expect("image");
        assert_eq!((image.width, image.height), (900, 500));
        surface.fill_rect(0.0, 0.0, 1.0, 1.0, 0xFF00_0000);
        assert_eq!(surface.buffer.len(), 900 * 500);

        assert!(surface.resize(320, 760).expect("shrink"));
        assert_eq!(surface.dimensions(), (320, 760));
        surface.fill_rect(0.0, 0.0, 1.0, 1.0, 0xFF00_0000);
        assert_eq!(surface.buffer.len(), 320 * 760);

        // Same size is a no-op, so a redraw does not reallocate every frame.
        assert!(!surface.resize(320, 760).expect("same"));

        // A hostile size is refused rather than allocating the world.
        assert!(surface.resize(0, 10).is_err());
        assert!(surface.resize(MAX_CANVAS_EDGE + 1, 10).is_err());
        // ...and a refused resize leaves the surface usable at its old size.
        assert_eq!(surface.dimensions(), (320, 760));
    }

    #[test]
    fn hostile_guest_draw_calls_never_panic() {
        // The 2D twin of the scene3d fuzz test. A guest can hand the canvas a
        // NaN rectangle, an infinite stroke, a rectangle the size of the
        // observable universe, or one with negative dimensions. The rasterizer
        // clips against the real buffer, so none of that may index out of
        // bounds or trap -- a bad app draws nothing, it does not crash the
        // host.
        let poison = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30, 0.0];
        let mut surface = CanvasSurface::new(40, 30).expect("surface");

        for &bad in &poison {
            surface.clear(0xFF00_2040);
            surface.fill_rect(bad, bad, bad, bad, 0xFFFF_FFFF);
            surface.fill_rect(bad, 0.0, 20.0, bad, 0xFF00_FF00);
            surface.stroke_rect(bad, bad, bad, bad, bad, 0xFFFF_0000);
            surface.stroke_rect(0.0, 0.0, 100.0, 100.0, bad, 0xFF00_00FF);
        }

        // draw_pixels with a valid tiny image at hostile positions.
        let tiny = ImagePixels::new(2, 2, vec![200; 16]).expect("tiny image");
        for &bad in &poison {
            surface.draw_pixels(bad, bad, bad, bad, &tiny);
            surface.draw_pixels(-1000.0, -1000.0, 10.0, 10.0, &tiny);
            surface.draw_pixels(1e9, 1e9, 10.0, 10.0, &tiny);
        }

        // draw_sprite with hostile center, size, and angle: a NaN angle, an
        // infinite size, a sprite the size of the universe -- none may trap.
        for &bad in &poison {
            surface.draw_sprite(bad, bad, bad, bad, bad, &tiny);
            surface.draw_sprite(20.0, 15.0, 30.0, 30.0, bad, &tiny);
            surface.draw_sprite(bad, bad, 10.0, 10.0, 0.7, &tiny);
        }

        // The buffer must still be the size it started at, and readable.
        assert_eq!(surface.dimensions(), (40, 30));
        let _ = surface.to_image().expect("image after hostile input");
    }

    #[test]
    fn a_rotated_sprite_lands_rotated_and_blended() {
        // A 2x2 opaque red sprite drawn large and rotated must put red pixels on
        // the canvas around its centre -- proving the rotate-blit path samples
        // and writes, and that a rotation does not send every pixel off-image.
        let mut surface = CanvasSurface::new(60, 60).expect("surface");
        surface.clear(0xFF00_0000); // opaque black
        let red = ImagePixels::new(
            2,
            2,
            vec![
                255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255,
            ],
        )
        .expect("red sprite");
        // 40x40 at the centre, rotated 30 degrees (pi/6).
        surface.draw_sprite(30.0, 30.0, 40.0, 40.0, core::f32::consts::FRAC_PI_6, &red);
        let img = surface.to_image().expect("image");
        // Centre pixel must be red (the sprite covers the middle at any angle).
        let c = 30 * 60 + 30;
        let px = &img.rgba[c * 4..c * 4 + 4];
        assert_eq!(
            px,
            &[255, 0, 0, 255],
            "the rotated sprite covers the centre"
        );
        // A far corner must still be the black background (the 40x40 rotated
        // sprite does not reach the 60x60 corners).
        let corner = &img.rgba[0..4];
        assert_eq!(corner, &[0, 0, 0, 255], "the corner stays background");
    }

    #[test]
    fn a_color_survives_packing_and_publishing() {
        let mut canvas = CanvasSurface::new(4, 4).expect("canvas");
        canvas.clear(pack_color(1.0, 0.0, 0.0, 1.0));
        let image = canvas.to_image().expect("image");
        assert_eq!((image.width, image.height), (4, 4));
        // First pixel: red, opaque, in RGBA order.
        assert_eq!(&image.rgba[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn nan_and_out_of_range_channels_become_colors_not_chaos() {
        // A guest float is untrusted input; NaN must not reach the buffer.
        assert_eq!(pack_color(f32::NAN, 2.0, -1.0, 1.0), 0xFF00_FF00);
    }

    #[test]
    fn a_stroke_is_edges_and_not_a_fill() {
        let mut canvas = CanvasSurface::new(10, 10).expect("canvas");
        canvas.clear(pack_color(0.0, 0.0, 0.0, 1.0));
        canvas.stroke_rect(0.0, 0.0, 10.0, 10.0, 1.0, pack_color(0.0, 1.0, 0.0, 1.0));
        let image = canvas.to_image().expect("image");
        let at = |x: usize, y: usize| &image.rgba[(y * 10 + x) * 4..(y * 10 + x) * 4 + 4];
        assert_eq!(at(0, 0), &[0, 255, 0, 255], "corner is stroked");
        assert_eq!(at(5, 0), &[0, 255, 0, 255], "top edge is stroked");
        assert_eq!(at(5, 5), &[0, 0, 0, 255], "the middle is untouched");
    }

    #[test]
    fn a_sprite_blends_over_the_canvas_instead_of_covering_it() {
        // A sprite's transparent pixels must show what is behind them. If they
        // did not, every sprite would be a rectangle with a picture in it --
        // which is the difference between a game and a slideshow.
        let mut canvas = CanvasSurface::new(8, 8).expect("canvas");
        canvas.clear(pack_color(0.0, 1.0, 0.0, 1.0));

        // 2x2 sprite: fully transparent except one opaque red pixel.
        let mut rgba = vec![0_u8; 16];
        rgba[0] = 255; // R of the first pixel
        rgba[3] = 255; // A of the first pixel
        let sprite = ImagePixels::new(2, 2, rgba).expect("sprite");
        canvas.draw_pixels(0.0, 0.0, 8.0, 8.0, &sprite);

        let image = canvas.to_image().expect("image");
        let at = |x: usize, y: usize| &image.rgba[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4];
        assert_eq!(at(1, 1), &[255, 0, 0, 255], "the opaque pixel is drawn");
        assert_eq!(
            at(6, 6),
            &[0, 255, 0, 255],
            "a transparent pixel leaves the canvas showing through"
        );
    }

    #[test]
    fn an_unreasonable_canvas_size_is_refused() {
        assert!(CanvasSurface::new(0, 10).is_err());
        assert!(CanvasSurface::new(10, MAX_CANVAS_EDGE + 1).is_err());
    }

    #[test]
    fn text_lands_inside_the_canvas() {
        let mut canvas = CanvasSurface::new(64, 24).expect("canvas");
        canvas.clear(pack_color(1.0, 1.0, 1.0, 1.0));
        canvas.text("hi", 2.0, 20.0, 7.0, pack_color(0.0, 0.0, 0.0, 1.0));
        let image = canvas.to_image().expect("image");
        // Some pixel became ink; where exactly is the font's business. With
        // antialiased vector text a tiny glyph may cover no pixel fully, so
        // "ink" means clearly darker than the white ground, not pure black.
        assert!(
            image
                .rgba
                .chunks(4)
                .any(|px| px[0] < 128 && px[1] < 128 && px[2] < 128 && px[3] == 255),
            "drawing text must change at least one pixel"
        );
    }

    /// The whole of K-002 in one assertion.
    ///
    /// Seven shipped apps measured text as `chars * size * constant`, which
    /// returns the *same* width for "iiii" and "WWWW" because both are four
    /// characters. On the real proportional face they differ several times
    /// over. If this assertion ever passes trivially again, the measurement
    /// has gone back to counting characters.
    #[test]
    fn narrow_and_wide_strings_do_not_measure_the_same() {
        let canvas = CanvasSurface::new(400, 80).expect("canvas");
        let narrow = canvas.measure_text("iiii", 32.0);
        let wide = canvas.measure_text("WWWW", 32.0);

        // The old guess: identical, because both are four characters.
        let guess = |s: &str| (s.chars().count() as f32) * 32.0 * 0.52;
        assert_eq!(
            guess("iiii"),
            guess("WWWW"),
            "the constant-per-character guess cannot tell these apart -- that is the bug"
        );

        assert!(
            wide.width > narrow.width * 2.0,
            "W is far wider than i: measured iiii={} WWWW={}",
            narrow.width,
            wide.width
        );
        assert!(narrow.width > 0.0, "a non-empty string has a width");
    }

    /// Measurement has to describe the pixels, not an idea of them: draw the
    /// run and compare the reported width against the actual inked extent.
    #[test]
    fn a_measured_width_matches_the_width_that_gets_drawn() {
        for text in ["iiii", "WWWW", "Hello, world", "1234567890"] {
            let size = 24.0;
            let mut canvas = CanvasSurface::new(600, 60).expect("canvas");
            canvas.clear(pack_color(1.0, 1.0, 1.0, 1.0));
            let m = canvas.measure_text(text, size);
            let x0 = 20.0f32;
            canvas.text(text, x0, 44.0, size, pack_color(0.0, 0.0, 0.0, 1.0));

            let image = canvas.to_image().expect("image");
            let (w, h) = (image.width as usize, image.height as usize);
            let mut min_x = usize::MAX;
            let mut max_x = 0usize;
            for y in 0..h {
                for x in 0..w {
                    let px = &image.rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
                    // Any pixel meaningfully darker than the white ground.
                    if px[0] < 200 && px[1] < 200 && px[2] < 200 {
                        min_x = min_x.min(x);
                        max_x = max_x.max(x);
                    }
                }
            }
            assert!(min_x != usize::MAX, "{text:?} drew nothing to measure");

            // Inked extent is the ink, and advance width includes the side
            // bearings the ink does not cover, so inked <= advance. Both ends
            // are bounded: the ink must not overflow the reported width, and
            // must not fall far short of it either.
            let inked = (max_x + 1) as f32 - min_x as f32;
            assert!(
                inked <= m.width + 4.0,
                "{text:?} inked {inked}px past its reported width {}",
                m.width
            );
            assert!(
                inked >= m.width - size * 0.6,
                "{text:?} reported {} but only inked {inked}px",
                m.width
            );
            // Ink starts at the pen position, within a bearing of it.
            assert!(
                (min_x as f32) >= x0 - 2.0 && (min_x as f32) <= x0 + size * 0.5,
                "{text:?} ink starts at {min_x}, pen was at {x0}"
            );
        }
    }

    /// The baseline contract: `draw_text` takes a baseline, so `ascent` has to
    /// be the distance from the top of the drawn ink up to it. Draw at a known
    /// baseline and check the ink sits inside `[baseline - ascent, baseline +
    /// descent]`.
    #[test]
    fn ascent_and_descent_bracket_the_drawn_ink() {
        let size = 32.0;
        let baseline = 60.0f32;
        let mut canvas = CanvasSurface::new(400, 100).expect("canvas");
        canvas.clear(pack_color(1.0, 1.0, 1.0, 1.0));
        let m = canvas.measure_text("Hgjy", size);
        canvas.text("Hgjy", 10.0, baseline, size, pack_color(0.0, 0.0, 0.0, 1.0));

        let image = canvas.to_image().expect("image");
        let (w, h) = (image.width as usize, image.height as usize);
        let mut min_y = usize::MAX;
        let mut max_y = 0usize;
        for y in 0..h {
            for x in 0..w {
                let px = &image.rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
                if px[0] < 200 && px[1] < 200 && px[2] < 200 {
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        assert!(min_y != usize::MAX, "nothing drawn");
        assert!(
            (min_y as f32) >= baseline - m.ascent - 2.0,
            "ink starts at {min_y}, above the reported ascent top {}",
            baseline - m.ascent
        );
        assert!(
            (max_y as f32) <= baseline + m.descent + 2.0,
            "ink ends at {max_y}, below the reported descent bottom {}",
            baseline + m.descent
        );
        assert!(m.height >= m.ascent + m.descent - 0.01);
    }

    /// An empty string has no width but still has a line: an app placing a
    /// caret in an empty field needs the height of the line about to hold text.
    #[test]
    fn an_empty_string_has_no_width_but_a_real_line_height() {
        let canvas = CanvasSurface::new(100, 40).expect("canvas");
        let m = canvas.measure_text("", 20.0);
        assert_eq!(m.width, 0.0);
        assert!(m.height > 0.0 && m.ascent > 0.0);
    }

    /// Whitespace draws nothing but still advances the pen, and an app laying
    /// out words needs that advance or every space collapses.
    #[test]
    fn a_space_has_width_even_though_it_inks_nothing() {
        let canvas = CanvasSurface::new(200, 40).expect("canvas");
        let one = canvas.measure_text("a a", 20.0).width;
        let two = canvas.measure_text("a  a", 20.0).width;
        assert!(
            two > one,
            "an extra space must widen the run: {one} then {two}"
        );
    }

    /// The caret case, which is most of why apps measure at all. Someone types
    /// "hello " and the caret belongs after the space, not back on the "o".
    /// The text layout engine's plain width drops trailing whitespace, so this
    /// is a real trap and not a hypothetical one.
    #[test]
    fn trailing_space_counts_toward_the_measured_width() {
        let canvas = CanvasSurface::new(300, 40).expect("canvas");
        let bare = canvas.measure_text("hello", 20.0).width;
        let trailing = canvas.measure_text("hello ", 20.0).width;
        assert!(
            trailing > bare,
            "a trailing space must move the pen: \"hello\"={bare} \"hello \"={trailing}"
        );
    }

    /// A run too wide for the vector rasterizer's pixmap is drawn with the
    /// bitmap face instead. Measurement has to switch faces on exactly the
    /// same runs, or a very long string is measured in one face and drawn in
    /// another -- the precise failure this whole change exists to remove.
    #[test]
    fn an_enormous_run_is_measured_in_the_face_that_will_draw_it() {
        let canvas = CanvasSurface::new(64, 32).expect("canvas");
        let huge: String = core::iter::repeat_n('W', 4000).collect();
        let m = canvas.measure_text(&huge, 256.0);
        // The bitmap face is a fixed cell per character, so its width is
        // exactly cell * scale * chars. Landing on that number proves the
        // measurement took the same fallback the drawing will.
        let scale = bitmap_scale(256.0);
        let bitmap = krate_adapter_common::drawtext::text_width(&huge, scale) as f32;
        assert_eq!(
            m.width, bitmap,
            "a run past the vector rasterizer's limit must be measured with the bitmap face"
        );
    }

    /// Font size is clamped identically on both paths, so an app that asks for
    /// an absurd heading is told the size it will actually be drawn at rather
    /// than the size it asked for.
    #[test]
    fn measurement_clamps_font_size_the_same_way_drawing_does() {
        let canvas = CanvasSurface::new(400, 60).expect("canvas");
        assert_eq!(
            canvas.measure_text("ab", 4000.0).width,
            canvas.measure_text("ab", 256.0).width,
            "an oversized size is clamped to the size that will be drawn"
        );
        assert_eq!(
            canvas.measure_text("ab", 0.5).width,
            canvas.measure_text("ab", 4.0).width,
            "an undersized size is clamped to the size that will be drawn"
        );
    }
}

#[cfg(test)]
mod clip_tests {
    #[test]
    fn a_stroked_circle_is_a_ring_not_a_disc() {
        // The gap that put square boxes around round bubbles in a shipped
        // screensaver: an app wanting a rim had only stroke_rect to reach for.
        let mut c = CanvasSurface::new(80, 80).expect("surface");
        c.clear(0xFF00_0000);
        c.stroke_circle(40.0, 40.0, 20.0, 3.0, 0xFFFF_FFFF);

        let at = |x: u32, y: u32| c.buffer[(y * 80 + x) as usize];
        // On the ring: bright.
        assert!(
            at(60, 40) & 0x00FF_FFFF > 0x0080_8080,
            "right of the ring is drawn"
        );
        assert!(
            at(20, 40) & 0x00FF_FFFF > 0x0080_8080,
            "left of the ring is drawn"
        );
        assert!(
            at(40, 20) & 0x00FF_FFFF > 0x0080_8080,
            "top of the ring is drawn"
        );
        // The middle must stay empty -- that is what makes it a ring.
        assert_eq!(at(40, 40), 0xFF00_0000, "the centre is not filled");
        // And the corner: a circle must not paint where a rect would.
        assert_eq!(at(22, 22), 0xFF00_0000, "no square corner");
    }

    #[test]
    fn a_stroked_circle_ignores_nonsense_sizes() {
        let mut c = CanvasSurface::new(20, 20).expect("surface");
        c.clear(0xFF00_0000);
        c.stroke_circle(10.0, 10.0, -5.0, 2.0, 0xFFFF_FFFF);
        c.stroke_circle(10.0, 10.0, 5.0, 0.0, 0xFFFF_FFFF);
        assert!(
            c.buffer.iter().all(|p| *p == 0xFF00_0000),
            "a negative radius or zero width draws nothing"
        );
    }

    use super::*;

    fn pixel(s: &CanvasSurface, x: u32, y: u32) -> u32 {
        s.to_image().expect("image").rgba[((y * s.width + x) * 4) as usize] as u32
    }

    #[test]
    fn a_clip_keeps_drawing_inside_its_rectangle() {
        let mut s = CanvasSurface::new(40, 40).expect("surface");
        s.clear(0xFF00_0000);
        // Allow only the lower half, then fill the whole canvas.
        s.set_clip(Some((0.0, 20.0, 40.0, 20.0)));
        s.fill_rect(0.0, 0.0, 40.0, 40.0, 0xFFFF_FFFF);
        // The header region is exactly what a scrolling list must protect.
        assert_eq!(pixel(&s, 5, 5), 0, "drew above the clip");
        assert_ne!(pixel(&s, 5, 30), 0, "did not draw inside the clip");
    }

    #[test]
    fn clearing_the_clip_restores_the_whole_canvas() {
        let mut s = CanvasSurface::new(20, 20).expect("surface");
        s.set_clip(Some((0.0, 10.0, 20.0, 10.0)));
        s.set_clip(None);
        s.fill_rect(0.0, 0.0, 20.0, 20.0, 0xFFFF_FFFF);
        assert_ne!(pixel(&s, 2, 2), 0, "clip was not cleared");
    }

    #[test]
    fn a_circle_is_clipped_too() {
        // Circles are drawn per pixel rather than as a rect, so they take a
        // different path and need their own proof.
        let mut s = CanvasSurface::new(40, 40).expect("surface");
        // The canvas starts opaque white, so clear to black first -- otherwise
        // "still white" and "drew white" are the same reading.
        s.clear(0xFF00_0000);
        s.set_clip(Some((0.0, 20.0, 40.0, 20.0)));
        s.fill_circle(20.0, 20.0, 15.0, 0xFFFF_FFFF);
        assert_eq!(pixel(&s, 20, 8), 0, "circle drew above the clip");
        assert_ne!(pixel(&s, 20, 30), 0, "circle missing inside the clip");
    }
}
