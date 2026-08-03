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
    width: u32,
    height: u32,
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

impl CanvasSurface {
    pub fn new(width: u32, height: u32) -> Result<Self, UiAdapterError> {
        if width == 0 || height == 0 || width > MAX_CANVAS_EDGE || height > MAX_CANVAS_EDGE {
            return Err(UiAdapterError::Unsupported(format!(
                "a canvas must be between 1x1 and {MAX_CANVAS_EDGE}x{MAX_CANVAS_EDGE}, got {width}x{height}"
            )));
        }
        Ok(Self {
            width,
            height,
            // Opaque white, so a canvas an app forgets to clear reads as a
            // blank sheet rather than a black hole in the window.
            buffer: vec![0xFFFF_FFFF; width as usize * height as usize],
        })
    }

    pub fn clear(&mut self, color: u32) {
        self.buffer.fill(color);
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
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
                if let Some(slot) = self.buffer.get_mut(idx) {
                    *slot = krate_adapter_common::painter::blend_over(px_color, *slot);
                }
            }
        }
    }

    /// A radial gradient disc: `inner` color at the center easing to `outer`
    /// color (typically transparent) at `radius`. This is the real glow/bloom
    /// primitive -- a soft light falloff instead of a flat disc, which is the
    /// single biggest difference between a modern look and a flat one.
    pub fn radial_gradient(&mut self, cx: f32, cy: f32, radius: f32, inner: u32, outer: u32) {
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
                if let Some(slot) = self.buffer.get_mut(idx) {
                    *slot = krate_adapter_common::painter::blend_over(c, *slot);
                }
            }
        }
    }

    /// A vertical linear gradient filling a rectangle: `top` color at `y`
    /// easing to `bottom` at `y + h`. For skies, panels, backdrops.
    pub fn linear_gradient_v(&mut self, x: f32, y: f32, w: f32, h: f32, top: u32, bottom: u32) {
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
        let stroke = stroke.max(1.0);
        self.fill_rect(x, y, w, stroke, color);
        self.fill_rect(x, y + h - stroke, w, stroke, color);
        self.fill_rect(x, y, stroke, h, color);
        self.fill_rect(x + w - stroke, y, stroke, h, color);
    }

    /// Draw a text run with the painter's bitmap font.
    ///
    /// `font-family` is accepted and ignored: the drawn painter has exactly one
    /// face, and honoring the same text on every system beats honoring a font
    /// name on one of them. Size maps to the font's integer scale.
    pub fn text(&mut self, text: &str, x: f32, y: f32, font_size: f32, color: u32) {
        let scale = ((font_size / 7.0).round() as u32).clamp(1, 16);
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

    /// Draw decoded RGBA into a rectangle, scaled to fit and centred.
    ///
    /// Reuses the painter's own `draw_image`, the same routine that puts a
    /// photo in an image widget -- so a sprite lands with identical scaling
    /// and alpha blending on all three systems, and there is one place where
    /// that behaviour can ever drift.
    pub fn draw_pixels(&mut self, x: f32, y: f32, w: f32, h: f32, image: &ImagePixels) {
        draw_image(
            &mut self.buffer,
            self.width,
            self.height,
            (x, y, w, h),
            image,
            None,
        );
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
    pub fn dimensions(&self) -> (u32, u32) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // Some pixel became ink; where exactly is the font's business.
        assert!(
            image.rgba.chunks(4).any(|px| px == [0, 0, 0, 255]),
            "drawing text must change at least one pixel"
        );
    }
}
