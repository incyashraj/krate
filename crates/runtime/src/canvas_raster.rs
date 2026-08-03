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
