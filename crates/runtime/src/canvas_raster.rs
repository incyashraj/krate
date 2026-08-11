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
    /// `0xAARRGGBB`, row-major from the top — the drawn painter's format.
    buffer: Vec<u32>,
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
            // Opaque white, so a canvas an app forgets to clear reads as a
            // blank sheet rather than a black hole in the window.
            buffer: vec![0xFFFF_FFFF; phys_w as usize * phys_h as usize],
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
        let Some((dw, dh)) = self.design else {
            return (1.0, 0.0, 0.0);
        };
        let (cw, ch) = (self.logical_width as f32, self.logical_height as f32);
        // The smaller ratio is what fits both axes -- the larger would crop.
        let k = (cw / dw).min(ch / dh);
        ((k), (cw - dw * k) / 2.0, (ch - dh * k) / 2.0)
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
            (mx * k, my * k, self.map_len(w) * k, self.map_len(h) * k)
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
        let (x, y) = self.map_point(x, y);
        let (w, h) = (self.map_len(w), self.map_len(h));
        let k = self.scale;
        let (x, y, w, h) = (x * k, y * k, w * k, h * k);
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

    /// A filled circle with an anti-aliased edge, alpha-blended over the
    /// canvas. Each pixel's coverage is its distance from the edge, so the rim
    /// is smooth rather than stair-stepped -- the primitive that makes round
    /// things look drawn instead of plotted.
    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: u32) {
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
        for py in y0..y1 {
            for px in x0..x1 {
                let d = Self::round_rect_sdf(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, radii);
                self.blend_coverage(px, py, color, (0.5 - d).clamp(0.0, 1.0));
            }
        }
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
        for py in y0..y1 {
            for px in x0..x1 {
                let d = Self::round_rect_sdf(px as f32 + 0.5, py as f32 + 0.5, x, y, w, h, radii);
                let t = ((blur - d) / (2.0 * blur)).clamp(0.0, 1.0);
                self.blend_coverage(px, py, color, t * t * (3.0 - 2.0 * t));
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
                self.blend_coverage(px, py, sample_stops(stops, t), 1.0);
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
        for py in y0..y1 {
            for px in x0..x1 {
                let dx = px as f32 + 0.5 - cx;
                let dy = py as f32 + 0.5 - cy;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist > radius {
                    continue;
                }
                // Smoothstep the interpolation so the falloff looks like light,
                // not a linear ramp.
                let t = dist / radius;
                let t = t * t * (3.0 - 2.0 * t);
                let c = lerp_color(inner, outer, t);
                if (c >> 24) == 0 {
                    continue;
                }
                let idx = (py * w + px) as usize;
                if !self.allowed(px, py) {
                    continue;
                }
                if let Some(slot) = self.buffer.get_mut(idx) {
                    *slot = krate_adapter_common::painter::blend_over(c, *slot);
                }
            }
        }
    }

    /// A vertical linear gradient filling a rectangle: `top` color at `y`
    /// easing to `bottom` at `y + h`. For skies, panels, backdrops.
    pub fn linear_gradient_v(&mut self, x: f32, y: f32, w: f32, h: f32, top: u32, bottom: u32) {
        let (x, y) = self.map_point(x, y);
        let (w, h) = (self.map_len(w), self.map_len(h));
        let k = self.scale;
        let (x, y, w, h) = (x * k, y * k, w * k, h * k);
        if h <= 0.0 || w <= 0.0 {
            return;
        }
        let rows = h.ceil() as i32;
        let mut i = 0i32;
        while i < rows {
            let t = (i as f32 + 0.5) / h;
            let c = lerp_color(top, bottom, t.clamp(0.0, 1.0));
            self.fill_rect(x, y + i as f32, w, 1.0, c);
            i += 1;
        }
    }

    /// Four thin fills; a stroke is its edges.
    pub fn stroke_rect(&mut self, x: f32, y: f32, w: f32, h: f32, stroke: f32, color: u32) {
        let (x, y) = self.map_point(x, y);
        let (w, h, stroke) = (self.map_len(w), self.map_len(h), self.map_len(stroke));
        let k = self.scale;
        let (x, y, w, h, stroke) = (x * k, y * k, w * k, h * k, stroke * k);
        let stroke = stroke.max(1.0);
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
        let (x, y) = self.map_point(x, y);
        let font_size = self.map_len(font_size);
        let k = self.scale;
        let (x, y, font_size) = (x * k, y * k, font_size * k);
        // Text is rendered by a shared painter that writes into the buffer
        // directly, so it cannot be gated per pixel the way the primitives
        // are. Snapshot what lies outside the clip, let the painter run, then
        // put it back -- correct for any glyph shape, and only as expensive as
        // the region actually clipped.
        let saved = self.clip.map(|_| self.buffer.clone());
        self.draw_text_unclipped(text, x, y, font_size, color, style);
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

    fn draw_text_unclipped(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        font_size: f32,
        color: u32,
        style: krate_adapter_common::vector_text::CanvasTextStyle,
    ) {
        if krate_adapter_common::vector_text::draw_canvas_text_styled(
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
        ) {
            return;
        }
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
        let (x, y) = self.map_point(x, y);
        let (w, h) = (self.map_len(w), self.map_len(h));
        let k = self.scale;
        let (x, y, w, h) = (x * k, y * k, w * k, h * k);
        draw_image(
            &mut self.buffer,
            self.width,
            self.height,
            (x, y, w, h),
            image,
            None,
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

    pub fn to_image(&self) -> Result<ImagePixels, UiAdapterError> {
        let mut rgba = Vec::with_capacity(self.buffer.len() * 4);
        for word in &self.buffer {
            rgba.push(((word >> 16) & 0xFF) as u8);
            rgba.push(((word >> 8) & 0xFF) as u8);
            rgba.push((word & 0xFF) as u8);
            rgba.push(((word >> 24) & 0xFF) as u8);
        }
        ImagePixels::new(self.width, self.height, rgba)
    }
}

/// Interpolate a color along sorted gradient stops, clamped at both ends.
fn sample_stops(stops: &[(f32, u32)], t: f32) -> u32 {
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
            return lerp_color(c0, c1, f);
        }
    }
    last.1
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // The buffer really is the new size, not just the reported number.
        assert_eq!(surface.buffer.len(), 900 * 500);

        assert!(surface.resize(320, 760).expect("shrink"));
        assert_eq!(surface.dimensions(), (320, 760));
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
