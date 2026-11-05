//! The canvas display list: what an app drew, as data.
//!
//! The CPU raster path consumes draw calls immediately and publishes
//! pixels. A GPU adapter wants the opposite: the calls themselves, late,
//! so it can turn them into GPU work at the display's own rhythm. This
//! list is that second consumer's food -- recorded beside the CPU raster
//! only when the adapter asks for it (`supports_canvas_lists`), so the
//! desktop path pays nothing.
//!
//! Ops mirror the WIT canvas surface one-to-one, in logical pixels, in
//! draw order. The consumer owns scaling to physical.

use std::sync::Arc;

use crate::ui::ImagePixels;

/// One recorded draw call, colors packed 0xAARRGGBB like the raster path.
#[derive(Clone, Debug)]
pub enum CanvasOp {
    Clear(u32),
    /// The ops that follow are in the screen's own points, not the app's
    /// design (true), or back in the design (false): the host's on-screen
    /// pad for a keyboard game is drawn over the app, unscaled (IC-914).
    ScreenSpace(bool),
    SetClip(Option<(f32, f32, f32, f32)>),
    FillRect {
        rect: (f32, f32, f32, f32),
        color: u32,
    },
    StrokeRect {
        rect: (f32, f32, f32, f32),
        width: f32,
        color: u32,
    },
    FillRoundRect {
        rect: (f32, f32, f32, f32),
        radii: (f32, f32, f32, f32),
        color: u32,
    },
    StrokeRoundRect {
        rect: (f32, f32, f32, f32),
        radii: (f32, f32, f32, f32),
        width: f32,
        color: u32,
    },
    DropShadowRoundRect {
        rect: (f32, f32, f32, f32),
        radii: (f32, f32, f32, f32),
        blur: f32,
        color: u32,
    },
    LinearGradient {
        rect: (f32, f32, f32, f32),
        top: u32,
        bottom: u32,
    },
    LinearGradientStops {
        rect: (f32, f32, f32, f32),
        angle_degrees: f32,
        stops: Vec<(f32, u32)>,
    },
    RadialGradient {
        center: (f32, f32),
        radius: f32,
        inner: u32,
        outer: u32,
    },
    FillCircle {
        center: (f32, f32),
        radius: f32,
        color: u32,
    },
    StrokeCircle {
        center: (f32, f32),
        radius: f32,
        width: f32,
        color: u32,
    },
    StrokeArc {
        center: (f32, f32),
        radius: f32,
        start_degrees: f32,
        sweep_degrees: f32,
        width: f32,
        color: u32,
    },
    Text {
        origin: (f32, f32),
        font_size: f32,
        color: u32,
        weight: u16,
        italic: bool,
        letter_spacing: f32,
        /// 0 sans, 1 serif, 2 mono -- the WIT family enum's order.
        family: u8,
        text: String,
    },
    Pixels {
        rect: (f32, f32, f32, f32),
        image: Arc<ImagePixels>,
    },
    PixelsRound {
        rect: (f32, f32, f32, f32),
        radii: (f32, f32, f32, f32),
        image: Arc<ImagePixels>,
    },
    Sprite {
        center: (f32, f32),
        dst: (f32, f32),
        angle: f32,
        image: Arc<ImagePixels>,
    },
}

/// One frame's recording for one canvas, in logical pixels.
#[derive(Clone, Debug, Default)]
pub struct CanvasList {
    pub logical_width: u32,
    pub logical_height: u32,
    /// The size the app designs in (canvas2d set-design-size), if any. Its
    /// ops are in those coordinates; a consumer maps them onto the canvas
    /// with [`CanvasList::design_transform`], as the CPU raster does.
    pub design: Option<(f32, f32)>,
    /// The edges of the canvas a phone covers (top, right, bottom, left, in
    /// logical pixels): a design is fitted inside them (see [`fit_design`]).
    pub design_inset: (f32, f32, f32, f32),
    pub ops: Vec<CanvasOp>,
}

/// Where a design of size `design` sits on a canvas of size `canvas` (both
/// logical): the largest uniform scale that fits it inside the canvas less
/// `inset` -- the edges a phone's status bar, camera cutout and navigation
/// cover, top, right, bottom, left -- centred in the space left, as
/// `(scale, x offset, y offset)`.
///
/// The one rule for every path -- the CPU raster, a GPU list, and the
/// mapping of a finger back into the design -- so a fixed-size app looks and
/// hit-tests the same everywhere. Fitted to the whole canvas, a wide app on
/// a turned phone ran under the status bar and the home indicator (K-1210).
/// Insets that would leave less than half the canvas are ignored: no
/// screen covers that much, and a bad report must not shrink an app away.
pub fn fit_design(
    canvas: (f32, f32),
    design: (f32, f32),
    inset: (f32, f32, f32, f32),
) -> (f32, f32, f32) {
    let (cw, ch) = canvas;
    let (dw, dh) = design;
    if dw <= 0.0 || dh <= 0.0 {
        return (1.0, 0.0, 0.0);
    }
    let (top, right, bottom, left) = inset;
    let clean = |v: f32| if v.is_finite() { v.max(0.0) } else { 0.0 };
    let (top, right, bottom, left) = (clean(top), clean(right), clean(bottom), clean(left));
    let (mut x0, mut y0, mut w, mut h) = (left, top, cw - left - right, ch - top - bottom);
    if w < cw / 2.0 || h < ch / 2.0 {
        (x0, y0, w, h) = (0.0, 0.0, cw, ch);
    }
    let k = (w / dw).min(h / dh);
    (k, x0 + (w - dw * k) / 2.0, y0 + (h - dh * k) / 2.0)
}

/// The colours along an app's four edges, for the spare space around a
/// letterboxed design: packed 0xAARRGGBB, top, bottom, left, right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeColors {
    pub top: u32,
    pub bottom: u32,
    pub left: u32,
    pub right: u32,
}

impl EdgeColors {
    fn all(color: u32) -> Self {
        EdgeColors {
            top: color,
            bottom: color,
            left: color,
            right: color,
        }
    }
}

/// The average colour of some RGBA pixels, packed opaque.
fn average(pixels: impl Iterator<Item = [u8; 4]>) -> u32 {
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for p in pixels {
        r += p[0] as u64;
        g += p[1] as u64;
        b += p[2] as u64;
        n += 1;
    }
    let n = n.max(1);
    0xFF00_0000 | (((r / n) as u32) << 16) | (((g / n) as u32) << 8) | (b / n) as u32
}

fn mix(a: u32, b: u32) -> u32 {
    let ch = |shift: u32| (((a >> shift) & 0xFF) + ((b >> shift) & 0xFF)) / 2;
    0xFF00_0000 | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

impl CanvasList {
    /// The colours at the edges of the app's design area, read from the
    /// first call that paints all of it -- its background. Where a design
    /// is letterboxed onto a screen of another shape, the spare space is
    /// painted with these, so the app reads as filling the screen instead
    /// of sitting between black bars. None without a design size, when a
    /// `clear` already paints the whole canvas, or when no call covers the
    /// design area first.
    pub fn edge_colors(&self) -> Option<EdgeColors> {
        let (dw, dh) = self.design?;
        let covers = |(x, y, w, h): (f32, f32, f32, f32)| {
            x <= 0.5 && y <= 0.5 && x + w >= dw - 0.5 && y + h >= dh - 0.5
        };
        for op in &self.ops {
            return match op {
                CanvasOp::SetClip(None) => continue,
                CanvasOp::Clear(_) => None,
                CanvasOp::FillRect { rect, color }
                | CanvasOp::FillRoundRect { rect, color, .. }
                    if covers(*rect) =>
                {
                    Some(EdgeColors::all(*color | 0xFF00_0000))
                }
                CanvasOp::LinearGradient { rect, top, bottom } if covers(*rect) => {
                    let side = mix(*top, *bottom);
                    Some(EdgeColors {
                        top: *top | 0xFF00_0000,
                        bottom: *bottom | 0xFF00_0000,
                        left: side,
                        right: side,
                    })
                }
                CanvasOp::LinearGradientStops { rect, stops, .. } if covers(*rect) => {
                    let first = stops.first()?.1 | 0xFF00_0000;
                    let last = stops.last()?.1 | 0xFF00_0000;
                    let side = mix(first, last);
                    Some(EdgeColors {
                        top: first,
                        bottom: last,
                        left: side,
                        right: side,
                    })
                }
                CanvasOp::Pixels { rect, image } | CanvasOp::PixelsRound { rect, image, .. }
                    if covers(*rect) && image.width > 0 && image.height > 0 =>
                {
                    let (w, h) = (image.width as usize, image.height as usize);
                    let px = |x: usize, y: usize| {
                        let i = (y * w + x) * 4;
                        [
                            image.rgba[i],
                            image.rgba[i + 1],
                            image.rgba[i + 2],
                            image.rgba[i + 3],
                        ]
                    };
                    Some(EdgeColors {
                        top: average((0..w).map(|x| px(x, 0))),
                        bottom: average((0..w).map(|x| px(x, h - 1))),
                        left: average((0..h).map(|y| px(0, y))),
                        right: average((0..h).map(|y| px(w - 1, y))),
                    })
                }
                _ => None,
            };
        }
        None
    }

    /// Scale and offset from design space onto the canvas: uniform,
    /// centred, letterboxed -- the same formula as the CPU raster's, so a
    /// design-size app looks and hit-tests the same on every path.
    /// `(1.0, 0.0, 0.0)` without a design size.
    pub fn design_transform(&self) -> (f32, f32, f32) {
        let Some(design) = self.design else {
            return (1.0, 0.0, 0.0);
        };
        fit_design(
            (self.logical_width as f32, self.logical_height as f32),
            design,
            self.design_inset,
        )
    }

    pub fn clear_for_frame(&mut self, width: u32, height: u32) {
        self.logical_width = width;
        self.logical_height = height;
        self.ops.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// K-1210: a fixed-size design is fitted inside what a phone covers,
    /// never under its status bar or navigation, and centred in what is
    /// left; nothing covered fits the whole canvas as before.
    #[test]
    fn a_design_fits_inside_the_edges_a_phone_covers() {
        // World Clock (880x560) on a phone turned landscape: 891x411 points,
        // a 24-point status bar, a 48-point cutout on the left, a 24-point
        // gesture bar.
        let (k, ox, oy) = fit_design((891.0, 411.0), (880.0, 560.0), (24.0, 0.0, 24.0, 48.0));
        let (x1, y1) = (ox + 880.0 * k, oy + 560.0 * k);
        assert!(
            oy >= 24.0 - 1e-3 && y1 <= 411.0 - 24.0 + 1e-3,
            "top {oy} bottom {y1}"
        );
        assert!(
            ox >= 48.0 - 1e-3 && x1 <= 891.0 + 1e-3,
            "left {ox} right {x1}"
        );
        assert!(
            (oy - 24.0).abs() < 1e-3 || (ox - 48.0).abs() < 1e-3,
            "it fills one axis"
        );
        assert_eq!(
            fit_design((400.0, 800.0), (200.0, 400.0), (0.0, 0.0, 0.0, 0.0)),
            (2.0, 0.0, 0.0)
        );
        // A report that would leave less than half the screen is not one.
        assert_eq!(
            fit_design((400.0, 800.0), (200.0, 400.0), (500.0, 0.0, 0.0, 0.0)),
            (2.0, 0.0, 0.0)
        );
        assert_eq!(
            fit_design((400.0, 800.0), (0.0, 400.0), (0.0, 0.0, 0.0, 0.0)),
            (1.0, 0.0, 0.0)
        );
    }
}
