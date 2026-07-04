//! Shared CPU painter for the drawn-widget fallback.
//!
//! Both winit backends (Linux and Windows) present the same pixels: they
//! acquire a framebuffer from softbuffer and hand it here. Keeping the
//! painting in one place means the renderer slice (vello + real
//! typography) swaps a single implementation, not one per platform.
//!
//! Pixel format is 0xAARRGGBB in a row-major `&mut [u32]`, matching what
//! softbuffer presents on both hosts.

use crate::drawtext;
use crate::ui::{WidgetKind, WidgetPlacement};

/// Widget palette for the drawn pass (0xAARRGGBB).
pub const COLOR_BACKGROUND: u32 = 0xFFF2F2F2;
pub const COLOR_BUTTON: u32 = 0xFF3B82F6;
pub const COLOR_BUTTON_LABEL: u32 = 0xFFFFFFFF;
pub const COLOR_FIELD_FILL: u32 = 0xFFFFFFFF;
pub const COLOR_FIELD_BORDER: u32 = 0xFF9CA3AF;
pub const COLOR_FIELD_TEXT: u32 = 0xFF1F2937;
pub const COLOR_TEXT: u32 = 0xFF111827;

/// Fill an axis-aligned rectangle, clipped to the buffer bounds.
pub fn fill_rect(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    rect: (f32, f32, f32, f32),
    color: u32,
) {
    let (x, y, w, h) = rect;
    let x0 = x.max(0.0) as u32;
    let y0 = y.max(0.0) as u32;
    let x1 = ((x + w).max(0.0) as u32).min(width);
    let y1 = ((y + h).max(0.0) as u32).min(height);
    for row in y0..y1 {
        let start = (row * width + x0) as usize;
        let end = (row * width + x1) as usize;
        for pixel in &mut buffer[start..end] {
            *pixel = color;
        }
    }
}

/// Paint the lowered widget placements into a framebuffer.
///
/// `scale` is the window's device scale factor; placements arrive in
/// logical coordinates and are painted in physical pixels.
pub fn paint_placements(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    scale: f32,
    placements: &[WidgetPlacement],
) {
    buffer.fill(COLOR_BACKGROUND);
    let text_scale = (scale.round() as u32).max(1);
    for placement in placements {
        let (px, py) = (placement.x * scale, placement.y * scale);
        let (pw, ph) = (placement.width * scale, placement.height * scale);
        let label = placement.label.as_deref().unwrap_or("");
        let th = drawtext::text_height(text_scale) as f32;
        match placement.kind {
            WidgetKind::Button => {
                fill_rect(buffer, width, height, (px, py, pw, ph), COLOR_BUTTON);
                let tw = drawtext::text_width(label, text_scale) as f32;
                drawtext::draw_text(
                    buffer,
                    width,
                    height,
                    ((px + (pw - tw) / 2.0) as i32, (py + (ph - th) / 2.0) as i32),
                    text_scale,
                    COLOR_BUTTON_LABEL,
                    label,
                );
            }
            WidgetKind::TextField | WidgetKind::TextArea => {
                fill_rect(buffer, width, height, (px, py, pw, ph), COLOR_FIELD_BORDER);
                fill_rect(
                    buffer,
                    width,
                    height,
                    (
                        px + 1.0 * scale,
                        py + 1.0 * scale,
                        (pw - 2.0 * scale).max(0.0),
                        (ph - 2.0 * scale).max(0.0),
                    ),
                    COLOR_FIELD_FILL,
                );
                drawtext::draw_text(
                    buffer,
                    width,
                    height,
                    ((px + 4.0 * scale) as i32, (py + (ph - th) / 2.0) as i32),
                    text_scale,
                    COLOR_FIELD_TEXT,
                    label,
                );
            }
            WidgetKind::Text => {
                drawtext::draw_text(
                    buffer,
                    width,
                    height,
                    (px as i32, (py + (ph - th) / 2.0) as i32),
                    text_scale,
                    COLOR_TEXT,
                    label,
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(kind: WidgetKind, label: &str, x: f32, y: f32, w: f32, h: f32) -> WidgetPlacement {
        WidgetPlacement {
            widget: crate::ui::WidgetId::new(1).unwrap(),
            kind,
            label: Some(label.to_string()),
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn paints_button_field_and_text_pixels() {
        let (w, h) = (200u32, 120u32);
        let mut buffer = vec![0u32; (w * h) as usize];
        let placements = [
            placement(WidgetKind::Button, "Click me", 10.0, 10.0, 100.0, 30.0),
            placement(WidgetKind::TextField, "hello", 10.0, 50.0, 150.0, 24.0),
            placement(WidgetKind::Text, "Title", 10.0, 84.0, 100.0, 16.0),
        ];
        paint_placements(&mut buffer, w, h, 1.0, &placements);

        let at = |x: u32, y: u32| buffer[(y * w + x) as usize];
        // Background fills untouched space.
        assert_eq!(at(w - 1, h - 1), COLOR_BACKGROUND);
        // Button body is filled; its centered label leaves white pixels.
        assert_eq!(at(12, 12), COLOR_BUTTON);
        let button_rows = 10..40u32;
        assert!(button_rows
            .flat_map(|y| (10..110u32).map(move |x| (x, y)))
            .any(|(x, y)| at(x, y) == COLOR_BUTTON_LABEL));
        // Field has a border, a white fill, and dark label pixels.
        assert_eq!(at(10, 50), COLOR_FIELD_BORDER);
        assert_eq!(at(80, 60), COLOR_FIELD_FILL);
        assert!((50..74u32)
            .flat_map(|y| (10..160u32).map(move |x| (x, y)))
            .any(|(x, y)| at(x, y) == COLOR_FIELD_TEXT));
        // Text widget renders glyph pixels straight on the background.
        assert!((84..100u32)
            .flat_map(|y| (10..110u32).map(move |x| (x, y)))
            .any(|(x, y)| at(x, y) == COLOR_TEXT));
    }

    #[test]
    fn scale_doubles_physical_geometry() {
        let (w, h) = (100u32, 60u32);
        let mut buffer = vec![0u32; (w * h) as usize];
        let placements = [placement(WidgetKind::Button, "", 10.0, 10.0, 20.0, 10.0)];
        paint_placements(&mut buffer, w, h, 2.0, &placements);
        let at = |x: u32, y: u32| buffer[(y * w + x) as usize];
        // Logical (10,10)-(30,20) becomes physical (20,20)-(60,40).
        assert_eq!(at(21, 21), COLOR_BUTTON);
        assert_eq!(at(59, 39), COLOR_BUTTON);
        assert_eq!(at(19, 21), COLOR_BACKGROUND);
        assert_eq!(at(61, 41), COLOR_BACKGROUND);
    }
}
