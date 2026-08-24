//! Minimize and close controls drawn OVER a full-bleed window.
//!
//! Full-bleed on Windows and Linux is an undecorated window: no title bar,
//! and with it no close or minimize button. macOS overlays its traffic
//! lights on the app's own drawing, so a full-bleed Mac app stays closable;
//! the same app on Windows was a bare rectangle a person could only leave
//! through Alt-F4 -- which most people do not know. The founder's chess app
//! was reported exactly so: "opens without close and minimize buttons".
//!
//! This module is the shared geometry and pixels for the adapter-drawn
//! answer: two small buttons in the top-right corner, painted after the app's
//! frame, hit-tested before the app sees the click. It has no opinions about
//! HOW they get onto the screen -- the CPU painter blends the sprite, the
//! GPU paths draw or upload it -- only WHERE they are and what they look
//! like, so every path agrees with the hit test by construction.

/// One button's logical width.
pub const BUTTON_W: f32 = 40.0;
/// One button's logical height.
pub const BUTTON_H: f32 = 30.0;
/// Buttons in the cluster: minimize, close.
pub const BUTTONS: u32 = 2;

/// The cluster's logical rectangle in a window of this logical width:
/// `(x, y, width, height)`, anchored to the top-right corner.
pub fn cluster_rect(window_logical_w: f32) -> (f32, f32, f32, f32) {
    let w = BUTTON_W * BUTTONS as f32;
    ((window_logical_w - w).max(0.0), 0.0, w, BUTTON_H)
}

/// Which control a logical point lands on, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlHit {
    Minimize,
    Close,
}

pub fn hit(window_logical_w: f32, x: f32, y: f32) -> Option<ControlHit> {
    let (cx, cy, cw, ch) = cluster_rect(window_logical_w);
    if x < cx || x >= cx + cw || y < cy || y >= cy + ch {
        return None;
    }
    if x < cx + BUTTON_W {
        Some(ControlHit::Minimize)
    } else {
        Some(ControlHit::Close)
    }
}

/// The cluster rasterized as straight-alpha RGBA at a pixel density.
///
/// `px_per_logical` is how many pixels one logical unit spans wherever the
/// sprite will land -- the window scale factor for a direct blend, or the
/// canvas-pixels-per-logical ratio when blended into a scaled canvas frame.
/// Returns `(rgba, width, height)`.
pub fn sprite(px_per_logical: f32) -> (Vec<u8>, u32, u32) {
    let s = px_per_logical.max(0.5);
    let w = (BUTTON_W * BUTTONS as f32 * s).round().max(1.0) as u32;
    let h = (BUTTON_H * s).round().max(1.0) as u32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];

    let put = |rgba: &mut [u8], x: i64, y: i64, color: [u8; 4]| {
        if x < 0 || y < 0 || x >= i64::from(w) || y >= i64::from(h) {
            return;
        }
        let i = ((y as u32 * w + x as u32) * 4) as usize;
        // Keep the strongest alpha: glyph strokes overwrite the pill.
        if color[3] >= rgba[i + 3] {
            rgba[i..i + 4].copy_from_slice(&color);
        }
    };

    // A soft dark pill behind each glyph, so the controls read on any app
    // background without stealing much of it.
    let pill = [12u8, 12, 14, 150];
    let glyph = [235u8, 235, 238, 255];

    for button in 0..BUTTONS {
        let bx = (button as f32 * BUTTON_W * s) as i64;
        let bw = (BUTTON_W * s) as i64;
        let bh = (BUTTON_H * s) as i64;
        let radius = (6.0 * s) as i64;
        for y in 0..bh {
            for x in 0..bw {
                // Rounded corners by the cheap metric: inside unless in a
                // corner square and outside its quarter circle.
                let (cx, cy) = (
                    if x < radius {
                        radius - x
                    } else if x >= bw - radius {
                        x - (bw - 1 - radius)
                    } else {
                        0
                    },
                    if y < radius {
                        radius - y
                    } else if y >= bh - radius {
                        y - (bh - 1 - radius)
                    } else {
                        0
                    },
                );
                if cx * cx + cy * cy <= radius * radius {
                    put(&mut rgba, bx + x, y, pill);
                }
            }
        }
        // Glyphs: a dash for minimize, a cross for close, stroked about
        // 1.6 logical units thick.
        let t = (1.6 * s).max(1.0) as i64;
        let (gx, gy) = (bx + bw / 2, bh / 2);
        let reach = (5.5 * s) as i64;
        if button == 0 {
            for x in -reach..=reach {
                for dy in 0..t {
                    put(&mut rgba, gx + x, gy + dy - t / 2, glyph);
                }
            }
        } else {
            for d in -reach..=reach {
                for o in 0..t {
                    put(&mut rgba, gx + d + o - t / 2, gy + d, glyph);
                    put(&mut rgba, gx + d + o - t / 2, gy - d, glyph);
                }
            }
        }
    }
    (rgba, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_map_to_the_right_buttons() {
        // A 1000-wide window: cluster spans x 920..1000.
        assert_eq!(hit(1000.0, 500.0, 10.0), None);
        assert_eq!(hit(1000.0, 930.0, 10.0), Some(ControlHit::Minimize));
        assert_eq!(hit(1000.0, 990.0, 10.0), Some(ControlHit::Close));
        assert_eq!(hit(1000.0, 990.0, 40.0), None, "below the cluster");
    }

    #[test]
    fn sprite_has_pixels_at_both_glyph_centres() {
        let (rgba, w, h) = sprite(2.0);
        assert!(w > 0 && h > 0);
        let sample = |fx: f32| {
            let x = (fx * w as f32) as u32;
            let y = h / 2;
            rgba[((y * w + x) * 4 + 3) as usize]
        };
        // Centre of each button is opaque glyph; the seam between pills is
        // pill-level alpha at most.
        assert_eq!(sample(0.25), 255, "minimize glyph");
        assert_eq!(sample(0.75), 255, "close glyph");
    }
}
