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
use crate::ui::{WidgetId, WidgetKind, WidgetPlacement};

/// Widget palette for the drawn pass (0xAARRGGBB).
pub const COLOR_BACKGROUND: u32 = 0xFFF2F2F2;
pub const COLOR_BUTTON: u32 = 0xFF3B82F6;
pub const COLOR_BUTTON_HOVER: u32 = 0xFF5C93F8;
pub const COLOR_BUTTON_PRESSED: u32 = 0xFF2563EB;
pub const COLOR_BUTTON_LABEL: u32 = 0xFFFFFFFF;
pub const COLOR_FIELD_FILL: u32 = 0xFFFFFFFF;
pub const COLOR_TRACK: u32 = 0xFFD1D5DB;
pub const COLOR_KNOB: u32 = 0xFFFFFFFF;
pub const COLOR_FIELD_BORDER: u32 = 0xFF9CA3AF;
pub const COLOR_FIELD_TEXT: u32 = 0xFF1F2937;
pub const COLOR_TEXT: u32 = 0xFF111827;
/// Fill behind the selected row of a selectable container. A tinted
/// wash of the button blue, so selection reads as the same accent
/// without competing with a real button for attention.
pub const COLOR_SELECTION: u32 = 0xFFBFD8FD;

/// Transient pointer state a backend reports so widgets can render
/// hover and pressed feedback. Purely visual: event routing is separate.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PaintInteraction {
    /// Widget currently under the cursor, if any.
    pub hovered: Option<WidgetId>,
    /// Widget the primary button is currently held down on, if any.
    pub pressed: Option<WidgetId>,
}

/// Whether the drawn-fallback painters can render this widget kind.
/// The winit hosts use this to filter placements, so painting support
/// and placement filtering can never drift apart again.
pub fn drawn_kind(kind: WidgetKind) -> bool {
    matches!(
        kind,
        WidgetKind::Button
            | WidgetKind::TextField
            | WidgetKind::TextArea
            | WidgetKind::Text
            | WidgetKind::Checkbox
            | WidgetKind::Radio
            | WidgetKind::Switch
            | WidgetKind::Slider
            | WidgetKind::Progress
            | WidgetKind::ListView
            | WidgetKind::TreeView
    )
}

/// Resolve the fill color for a button under the given interaction.
pub fn button_fill_color(widget: WidgetId, interaction: PaintInteraction) -> u32 {
    if interaction.pressed == Some(widget) {
        COLOR_BUTTON_PRESSED
    } else if interaction.hovered == Some(widget) {
        COLOR_BUTTON_HOVER
    } else {
        COLOR_BUTTON
    }
}

/// Topmost interactive widget (buttons for now) containing the logical
/// point, honoring paint order: later placements draw on top.
pub fn topmost_interactive_at(placements: &[WidgetPlacement], x: f32, y: f32) -> Option<WidgetId> {
    placements
        .iter()
        .rev()
        .find(|placement| {
            placement.kind == WidgetKind::Button
                && x >= placement.x
                && y >= placement.y
                && x < placement.x + placement.width
                && y < placement.y + placement.height
        })
        .map(|placement| placement.widget)
}

/// Intersect two logical rectangles; None when they do not overlap.
pub fn intersect_rects(
    a: (f32, f32, f32, f32),
    b: (f32, f32, f32, f32),
) -> Option<(f32, f32, f32, f32)> {
    let x0 = a.0.max(b.0);
    let y0 = a.1.max(b.1);
    let x1 = (a.0 + a.2).min(b.0 + b.2);
    let y1 = (a.1 + a.3).min(b.1 + b.3);
    if x1 > x0 && y1 > y0 {
        Some((x0, y0, x1 - x0, y1 - y0))
    } else {
        None
    }
}

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
/// logical coordinates and are painted in physical pixels. With the
/// `vector-text` feature the frame renders through vello with
/// antialiased parley text, falling back to the bitmap painter when the
/// host has no usable fonts or `KRATE_BITMAP_TEXT` is set.
pub fn paint_placements(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    scale: f32,
    placements: &[WidgetPlacement],
    interaction: PaintInteraction,
) {
    #[cfg(feature = "vector-text")]
    if std::env::var_os("KRATE_BITMAP_TEXT").is_none()
        && crate::vector_text::try_paint_placements(
            buffer,
            width,
            height,
            scale,
            placements,
            interaction,
        )
    {
        return;
    }
    paint_placements_bitmap(buffer, width, height, scale, placements, interaction);
}

/// The zero-dependency painter: flat fills plus 5x7 bitmap-font labels.
pub fn paint_placements_bitmap(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    scale: f32,
    placements: &[WidgetPlacement],
    interaction: PaintInteraction,
) {
    buffer.fill(COLOR_BACKGROUND);
    let text_scale = (scale.round() as u32).max(1);
    for placement in placements {
        let (px, py) = (placement.x * scale, placement.y * scale);
        let (pw, ph) = (placement.width * scale, placement.height * scale);
        // Scroll clipping: skip fully-hidden widgets; for the rest the
        // fills intersect the clip and labels render only when their own
        // rect fits inside it (partial text rows wait for vello clip
        // layers).
        let clip_px = placement
            .clip
            .map(|(cx, cy, cw, ch)| (cx * scale, cy * scale, cw * scale, ch * scale));
        if let Some(clip) = clip_px {
            if intersect_rects((px, py, pw, ph), clip).is_none() {
                continue;
            }
        }
        let clip_fill = |rect: (f32, f32, f32, f32)| match clip_px {
            Some(clip) => intersect_rects(rect, clip),
            None => Some(rect),
        };
        let label_visible = clip_px
            .map(|clip| intersect_rects((px, py, pw, ph), clip) == Some((px, py, pw, ph)))
            .unwrap_or(true);
        let label = placement.label.as_deref().unwrap_or("");
        let label = if label_visible { label } else { "" };
        let th = drawtext::text_height(text_scale) as f32;
        match placement.kind {
            WidgetKind::Button => {
                let fill = button_fill_color(placement.widget, interaction);
                if let Some(clipped) = clip_fill((px, py, pw, ph)) {
                    fill_rect(buffer, width, height, clipped, fill)
                };
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
            WidgetKind::TextField => {
                if let Some(clipped) = clip_fill((px, py, pw, ph)) {
                    fill_rect(buffer, width, height, clipped, COLOR_FIELD_BORDER)
                };
                if let Some(clipped) = clip_fill((
                    px + 1.0 * scale,
                    py + 1.0 * scale,
                    (pw - 2.0 * scale).max(0.0),
                    (ph - 2.0 * scale).max(0.0),
                )) {
                    fill_rect(buffer, width, height, clipped, COLOR_FIELD_FILL)
                };
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
            WidgetKind::TextArea => {
                if let Some(clipped) = clip_fill((px, py, pw, ph)) {
                    fill_rect(buffer, width, height, clipped, COLOR_FIELD_BORDER)
                };
                if let Some(clipped) = clip_fill((
                    px + 1.0 * scale,
                    py + 1.0 * scale,
                    (pw - 2.0 * scale).max(0.0),
                    (ph - 2.0 * scale).max(0.0),
                )) {
                    fill_rect(buffer, width, height, clipped, COLOR_FIELD_FILL)
                };
                // The bitmap font is a fixed cell, so wrapping is arithmetic
                // rather than shaping: how many glyphs fit across the inner
                // width. Lines fill downward from the top, like a note.
                let inset = 4.0 * scale;
                let cell = drawtext::text_width("x", text_scale).max(1) as f32;
                let per_line = (((pw - inset * 2.0) / cell).floor() as usize).max(1);
                let line_h = th + 2.0 * scale;
                let mut line_y = py + inset;
                let mut rest = label;
                while !rest.is_empty() && line_y + th <= py + ph - inset {
                    let take = rest.chars().count().min(per_line);
                    let split = rest
                        .char_indices()
                        .nth(take)
                        .map(|(index, _)| index)
                        .unwrap_or(rest.len());
                    let (line, remainder) = rest.split_at(split);
                    drawtext::draw_text(
                        buffer,
                        width,
                        height,
                        ((px + inset) as i32, line_y as i32),
                        text_scale,
                        COLOR_FIELD_TEXT,
                        line,
                    );
                    rest = remainder;
                    line_y += line_h;
                }
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
            WidgetKind::Checkbox | WidgetKind::Radio => {
                let box_side = ph.min(18.0 * scale);
                let by = py + (ph - box_side) / 2.0;
                if let Some(clipped) = clip_fill((px, by, box_side, box_side)) {
                    fill_rect(buffer, width, height, clipped, COLOR_FIELD_BORDER)
                };
                if let Some(clipped) = clip_fill((
                    px + scale,
                    by + scale,
                    (box_side - 2.0 * scale).max(0.0),
                    (box_side - 2.0 * scale).max(0.0),
                )) {
                    fill_rect(buffer, width, height, clipped, COLOR_FIELD_FILL)
                };
                if placement.checked == Some(true) {
                    let inset = 4.0 * scale;
                    if let Some(clipped) = clip_fill((
                        px + inset,
                        by + inset,
                        (box_side - 2.0 * inset).max(0.0),
                        (box_side - 2.0 * inset).max(0.0),
                    )) {
                        fill_rect(buffer, width, height, clipped, COLOR_BUTTON)
                    };
                }
                drawtext::draw_text(
                    buffer,
                    width,
                    height,
                    (
                        (px + box_side + 8.0 * scale) as i32,
                        (py + (ph - th) / 2.0) as i32,
                    ),
                    text_scale,
                    COLOR_TEXT,
                    label,
                );
            }
            WidgetKind::Switch => {
                let track_w = (36.0 * scale).min(pw);
                let track_h = (20.0 * scale).min(ph);
                let ty = py + (ph - track_h) / 2.0;
                let on = placement.checked == Some(true);
                let track_color = if on { COLOR_BUTTON } else { COLOR_TRACK };
                if let Some(clipped) = clip_fill((px, ty, track_w, track_h)) {
                    fill_rect(buffer, width, height, clipped, track_color)
                };
                let knob_side = (track_h - 4.0 * scale).max(0.0);
                let knob_x = if on {
                    px + track_w - knob_side - 2.0 * scale
                } else {
                    px + 2.0 * scale
                };
                if let Some(clipped) = clip_fill((knob_x, ty + 2.0 * scale, knob_side, knob_side)) {
                    fill_rect(buffer, width, height, clipped, COLOR_KNOB)
                };
            }
            WidgetKind::ListView | WidgetKind::TreeView => {
                // The container itself only paints the selection wash; the
                // rows are their own placements and paint their labels on
                // top, because parents lower before their children.
                if let Some((sx, sy, sw, sh)) = placement.selection {
                    if let Some(clipped) =
                        clip_fill((sx * scale, sy * scale, sw * scale, sh * scale))
                    {
                        fill_rect(buffer, width, height, clipped, COLOR_SELECTION)
                    };
                }
            }
            WidgetKind::Slider | WidgetKind::Progress => {
                let fraction = placement.value.unwrap_or(0.0).clamp(0.0, 1.0);
                let groove_h = if placement.kind == WidgetKind::Slider {
                    4.0 * scale
                } else {
                    6.0 * scale
                };
                let gy = py + (ph - groove_h) / 2.0;
                if let Some(clipped) = clip_fill((px, gy, pw, groove_h)) {
                    fill_rect(buffer, width, height, clipped, COLOR_TRACK)
                };
                if let Some(clipped) = clip_fill((px, gy, pw * fraction, groove_h)) {
                    fill_rect(buffer, width, height, clipped, COLOR_BUTTON)
                };
                if placement.kind == WidgetKind::Slider {
                    let thumb = (16.0 * scale).min(ph);
                    let tx = px + (pw - thumb) * fraction;
                    let ty2 = py + (ph - thumb) / 2.0;
                    if let Some(clipped) = clip_fill((tx, ty2, thumb, thumb)) {
                        fill_rect(buffer, width, height, clipped, COLOR_FIELD_BORDER)
                    };
                    if let Some(clipped) = clip_fill((
                        tx + scale,
                        ty2 + scale,
                        (thumb - 2.0 * scale).max(0.0),
                        (thumb - 2.0 * scale).max(0.0),
                    )) {
                        fill_rect(buffer, width, height, clipped, COLOR_KNOB)
                    };
                }
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
            checked: None,
            value: None,
            selection: None,
            clip: None,
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
        paint_placements_bitmap(
            &mut buffer,
            w,
            h,
            1.0,
            &placements,
            PaintInteraction::default(),
        );

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
    fn pressed_and_hovered_buttons_change_fill() {
        let (w, h) = (100u32, 60u32);
        let mut buffer = vec![0u32; (w * h) as usize];
        let placements = [placement(WidgetKind::Button, "", 10.0, 10.0, 40.0, 20.0)];
        let id = placements[0].widget;
        let at = |b: &Vec<u32>, x: u32, y: u32| b[(y * w + x) as usize];

        let hover = PaintInteraction {
            hovered: Some(id),
            pressed: None,
        };
        paint_placements_bitmap(&mut buffer, w, h, 1.0, &placements, hover);
        assert_eq!(at(&buffer, 20, 20), COLOR_BUTTON_HOVER);

        let pressed = PaintInteraction {
            hovered: Some(id),
            pressed: Some(id),
        };
        paint_placements_bitmap(&mut buffer, w, h, 1.0, &placements, pressed);
        assert_eq!(at(&buffer, 20, 20), COLOR_BUTTON_PRESSED);
    }

    #[test]
    fn hit_test_honors_paint_order_and_kind() {
        let below = placement(WidgetKind::Button, "", 0.0, 0.0, 50.0, 50.0);
        let mut top = placement(WidgetKind::Button, "", 20.0, 20.0, 50.0, 50.0);
        top.widget = crate::ui::WidgetId::new(2).unwrap();
        let text = placement(WidgetKind::Text, "", 0.0, 0.0, 200.0, 200.0);
        let placements = [below.clone(), top.clone(), text];
        assert_eq!(
            topmost_interactive_at(&placements, 30.0, 30.0),
            Some(top.widget)
        );
        assert_eq!(
            topmost_interactive_at(&placements, 5.0, 5.0),
            Some(below.widget)
        );
        assert_eq!(topmost_interactive_at(&placements, 150.0, 150.0), None);
    }

    #[test]
    fn drawn_kinds_cover_every_painted_arm() {
        for kind in [
            WidgetKind::Button,
            WidgetKind::TextField,
            WidgetKind::TextArea,
            WidgetKind::Text,
            WidgetKind::Checkbox,
            WidgetKind::Radio,
            WidgetKind::Switch,
            WidgetKind::Slider,
            WidgetKind::Progress,
            WidgetKind::ListView,
            WidgetKind::TreeView,
        ] {
            assert!(drawn_kind(kind), "{kind:?} must be drawable");
        }
        assert!(!drawn_kind(WidgetKind::Stack));
        assert!(!drawn_kind(WidgetKind::Canvas));
    }

    #[test]
    fn list_view_paints_selection_only_where_selected() {
        let (w, h) = (200u32, 80u32);
        let at = |b: &Vec<u32>, x: u32, y: u32| b[(y * w + x) as usize];

        // No selection: the container contributes no fill of its own.
        let mut empty = vec![0u32; (w * h) as usize];
        let list = placement(WidgetKind::ListView, "", 0.0, 0.0, 200.0, 80.0);
        paint_placements_bitmap(
            &mut empty,
            w,
            h,
            1.0,
            std::slice::from_ref(&list),
            PaintInteraction::default(),
        );
        assert_eq!(
            at(&empty, 10, 10),
            COLOR_BACKGROUND,
            "an unselected list must not tint its own area"
        );

        // Selecting the second row fills that row's rect, and only it.
        let mut painted = vec![0u32; (w * h) as usize];
        let selected = WidgetPlacement {
            selection: Some((0.0, 24.0, 200.0, 24.0)),
            ..list
        };
        paint_placements_bitmap(
            &mut painted,
            w,
            h,
            1.0,
            &[selected],
            PaintInteraction::default(),
        );
        assert_eq!(
            at(&painted, 10, 30),
            COLOR_SELECTION,
            "the selected row rect must carry the selection fill"
        );
        assert_eq!(
            at(&painted, 10, 10),
            COLOR_BACKGROUND,
            "rows above the selection stay unpainted"
        );
        assert_eq!(
            at(&painted, 10, 60),
            COLOR_BACKGROUND,
            "rows below the selection stay unpainted"
        );
    }

    #[test]
    fn list_view_selection_is_clipped_by_a_scroll_ancestor() {
        let (w, h) = (200u32, 80u32);
        let at = |b: &Vec<u32>, x: u32, y: u32| b[(y * w + x) as usize];
        let mut buffer = vec![0u32; (w * h) as usize];
        // Selection spans y 0..40 but the scroll clip only admits y 20..80,
        // so the top half of the highlight must not reach the buffer.
        let list = WidgetPlacement {
            selection: Some((0.0, 0.0, 200.0, 40.0)),
            clip: Some((0.0, 20.0, 200.0, 60.0)),
            ..placement(WidgetKind::ListView, "", 0.0, 0.0, 200.0, 80.0)
        };
        paint_placements_bitmap(&mut buffer, w, h, 1.0, &[list], PaintInteraction::default());
        assert_eq!(
            at(&buffer, 10, 10),
            COLOR_BACKGROUND,
            "highlight above the clip must be cut"
        );
        assert_eq!(
            at(&buffer, 10, 30),
            COLOR_SELECTION,
            "highlight inside the clip must survive"
        );
    }

    #[test]
    fn stateful_widgets_render_their_state() {
        let (w, h) = (200u32, 40u32);
        let at = |b: &Vec<u32>, x: u32, y: u32| b[(y * w + x) as usize];

        // Checkbox: checked fills the inner box with the accent color.
        let mut unchecked = vec![0u32; (w * h) as usize];
        let mut node = placement(WidgetKind::Checkbox, "ok", 10.0, 10.0, 120.0, 20.0);
        paint_placements_bitmap(
            &mut unchecked,
            w,
            h,
            1.0,
            &[node.clone()],
            PaintInteraction::default(),
        );
        let mut checked = vec![0u32; (w * h) as usize];
        node.checked = Some(true);
        paint_placements_bitmap(
            &mut checked,
            w,
            h,
            1.0,
            &[node],
            PaintInteraction::default(),
        );
        assert_eq!(at(&unchecked, 19, 20), COLOR_FIELD_FILL);
        assert_eq!(at(&checked, 19, 20), COLOR_BUTTON);

        // Switch: the knob sits left when off, right when on.
        let mut off = vec![0u32; (w * h) as usize];
        let mut switch = placement(WidgetKind::Switch, "", 10.0, 10.0, 40.0, 20.0);
        switch.checked = Some(false);
        paint_placements_bitmap(
            &mut off,
            w,
            h,
            1.0,
            &[switch.clone()],
            PaintInteraction::default(),
        );
        let mut on = vec![0u32; (w * h) as usize];
        switch.checked = Some(true);
        paint_placements_bitmap(&mut on, w, h, 1.0, &[switch], PaintInteraction::default());
        assert_eq!(at(&off, 14, 20), COLOR_KNOB);
        assert_eq!(at(&on, 14, 20), COLOR_BUTTON);
        assert_eq!(at(&on, 41, 20), COLOR_KNOB);

        // Progress: the filled fraction uses the accent color.
        let mut bar = vec![0u32; (w * h) as usize];
        let mut progress = placement(WidgetKind::Progress, "", 10.0, 10.0, 100.0, 20.0);
        progress.value = Some(0.5);
        paint_placements_bitmap(
            &mut bar,
            w,
            h,
            1.0,
            &[progress],
            PaintInteraction::default(),
        );
        assert_eq!(at(&bar, 30, 20), COLOR_BUTTON);
        assert_eq!(at(&bar, 90, 20), COLOR_TRACK);
    }

    #[test]
    fn clip_limits_fills_and_hides_out_of_view_widgets() {
        let (w, h) = (100u32, 100u32);
        let at = |b: &Vec<u32>, x: u32, y: u32| b[(y * w + x) as usize];
        // Clip region y 20..60: a button half-below the clip bottom only
        // paints inside it, and a button entirely below paints nothing.
        let mut half = placement(WidgetKind::Button, "", 10.0, 50.0, 40.0, 20.0);
        half.clip = Some((0.0, 20.0, 100.0, 40.0));
        let mut hidden = placement(WidgetKind::Button, "", 10.0, 70.0, 40.0, 20.0);
        hidden.clip = Some((0.0, 20.0, 100.0, 40.0));
        let mut buffer = vec![0u32; (w * h) as usize];
        paint_placements_bitmap(
            &mut buffer,
            w,
            h,
            1.0,
            &[half, hidden],
            PaintInteraction::default(),
        );
        assert_eq!(at(&buffer, 20, 55), COLOR_BUTTON, "inside clip paints");
        assert_eq!(at(&buffer, 20, 62), COLOR_BACKGROUND, "below clip is cut");
        assert_eq!(
            at(&buffer, 20, 75),
            COLOR_BACKGROUND,
            "hidden widget skipped"
        );
    }

    #[test]
    fn scale_doubles_physical_geometry() {
        let (w, h) = (100u32, 60u32);
        let mut buffer = vec![0u32; (w * h) as usize];
        let placements = [placement(WidgetKind::Button, "", 10.0, 10.0, 20.0, 10.0)];
        paint_placements_bitmap(
            &mut buffer,
            w,
            h,
            2.0,
            &placements,
            PaintInteraction::default(),
        );
        let at = |x: u32, y: u32| buffer[(y * w + x) as usize];
        // Logical (10,10)-(30,20) becomes physical (20,20)-(60,40).
        assert_eq!(at(21, 21), COLOR_BUTTON);
        assert_eq!(at(59, 39), COLOR_BUTTON);
        assert_eq!(at(19, 21), COLOR_BACKGROUND);
        assert_eq!(at(61, 41), COLOR_BACKGROUND);
    }
}
