//! Vector renderer for the drawn-widget fallback (ADR-0015 renderer slice).
//!
//! Paints the whole frame — background, widget fills, and antialiased
//! text laid out by parley from real system fonts — through `vello_cpu`
//! into the same `0xAARRGGBB` framebuffer the bitmap painter targets.
//! The bitmap painter remains the zero-dependency fallback: callers use
//! [`try_paint_placements`] and fall back when it returns `false`
//! (oversized surface, or a host with no usable system fonts).

use std::cell::RefCell;

use parley::{
    Alignment, AlignmentOptions, FontContext, GenericFamily, Layout, LayoutContext,
    PositionedLayoutItem, StyleProperty,
};
use vello_cpu::color::AlphaColor;
use vello_cpu::kurbo::{Circle, Rect, RoundedRect, Shape};
use vello_cpu::{Glyph, Pixmap, RenderContext, Resources};

use crate::painter::{
    button_fill_color, intersect_rects, PaintInteraction, COLOR_BACKGROUND, COLOR_BUTTON,
    COLOR_BUTTON_LABEL, COLOR_FIELD_BORDER, COLOR_FIELD_FILL, COLOR_FIELD_TEXT, COLOR_KNOB,
    COLOR_SELECTION, COLOR_TEXT, COLOR_TRACK,
};
use crate::ui::{kind_is_selectable, WidgetKind, WidgetPlacement};

/// Logical font size for widget labels; multiplied by the scale factor
/// through parley's display scale.
const LABEL_FONT_SIZE: f32 = 13.0;

thread_local! {
    static TEXT_ENGINE: RefCell<TextEngine> = RefCell::new(TextEngine::new());
}

struct TextEngine {
    font_cx: FontContext,
    layout_cx: LayoutContext<()>,
}

impl TextEngine {
    fn new() -> Self {
        Self {
            font_cx: FontContext::new(),
            layout_cx: LayoutContext::new(),
        }
    }

    /// Lay out one line of label text at the given display scale.
    fn layout_label(&mut self, text: &str, scale: f32) -> Layout<()> {
        self.layout_text(text, scale, None)
    }

    /// Lay out text, wrapping to `max_width` when one is given.
    ///
    /// A single-line field passes `None` and gets the old behavior. A text
    /// area passes its inner width, and parley breaks lines to fit.
    fn layout_text(&mut self, text: &str, scale: f32, max_width: Option<f32>) -> Layout<()> {
        let mut builder = self
            .layout_cx
            .ranged_builder(&mut self.font_cx, text, scale, true);
        builder.push_default(GenericFamily::SansSerif);
        builder.push_default(StyleProperty::FontSize(LABEL_FONT_SIZE));
        let mut layout = builder.build(text);
        layout.break_all_lines(max_width);
        layout.align(Alignment::Start, AlignmentOptions::default());
        layout
    }
}

fn argb(color: u32) -> AlphaColor<vello_cpu::color::Srgb> {
    AlphaColor::from_rgba8(
        ((color >> 16) & 0xFF) as u8,
        ((color >> 8) & 0xFF) as u8,
        (color & 0xFF) as u8,
        ((color >> 24) & 0xFF) as u8,
    )
}

fn fill(ctx: &mut RenderContext, color: u32, x: f32, y: f32, w: f32, h: f32) {
    ctx.set_paint(argb(color));
    ctx.fill_rect(&Rect::new(
        x as f64,
        y as f64,
        (x + w) as f64,
        (y + h) as f64,
    ));
}

/// Fill a circle centered at (cx, cy).
fn fill_circle(ctx: &mut RenderContext, color: u32, cx: f32, cy: f32, radius: f32) {
    ctx.set_paint(argb(color));
    let circle = Circle::new((cx as f64, cy as f64), radius as f64);
    ctx.fill_path(&circle.to_path(0.25));
}

/// Fill a rounded rectangle (radius in physical pixels).
fn fill_rounded(ctx: &mut RenderContext, color: u32, x: f32, y: f32, w: f32, h: f32, radius: f32) {
    ctx.set_paint(argb(color));
    let rrect = RoundedRect::new(
        x as f64,
        y as f64,
        (x + w) as f64,
        (y + h) as f64,
        radius as f64,
    );
    ctx.fill_path(&rrect.to_path(0.25));
}

/// Draw one laid-out label with its top-left corner at `(x, y)`.
fn draw_layout(
    ctx: &mut RenderContext,
    resources: &mut Resources,
    layout: &Layout<()>,
    color: u32,
    x: f32,
    y: f32,
) -> usize {
    let mut drawn = 0usize;
    for line in layout.lines() {
        for item in line.items() {
            if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                let mut run_x = glyph_run.offset();
                let run_y = glyph_run.baseline();
                let glyphs: Vec<Glyph> = glyph_run
                    .glyphs()
                    .map(|g| {
                        let gx = x + run_x + g.x;
                        let gy = y + run_y - g.y;
                        run_x += g.advance;
                        Glyph {
                            id: g.id,
                            x: gx,
                            y: gy,
                        }
                    })
                    .collect();
                drawn += glyphs.len();
                let run = glyph_run.run();
                let font = run.font();
                let font_size = run.font_size();
                ctx.set_paint(argb(color));
                ctx.glyph_run(resources, font)
                    .font_size(font_size)
                    .hint(true)
                    .fill_glyphs(glyphs.into_iter());
            }
        }
    }
    drawn
}

/// Paint the placements with vector fills and antialiased text.
///
/// Returns `false` without touching `buffer` when the surface exceeds
/// `u16` pixmap limits or when a non-empty label produces no glyphs
/// (no usable system fonts) — callers then use the bitmap painter.
pub fn try_paint_placements(
    buffer: &mut [u32],
    width: u32,
    height: u32,
    scale: f32,
    placements: &[WidgetPlacement],
    interaction: PaintInteraction,
) -> bool {
    let (Ok(w16), Ok(h16)) = (u16::try_from(width), u16::try_from(height)) else {
        return false;
    };
    if w16 == 0 || h16 == 0 {
        return false;
    }

    TEXT_ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let mut ctx = RenderContext::new(w16, h16);
        let mut resources = Resources::new();
        fill(
            &mut ctx,
            COLOR_BACKGROUND,
            0.0,
            0.0,
            width as f32,
            height as f32,
        );

        for placement in placements {
            let (px, py) = (placement.x * scale, placement.y * scale);
            let (pw, ph) = (placement.width * scale, placement.height * scale);
            // Scroll clipping mirrors the bitmap painter: fully hidden
            // widgets skip; clipped widgets draw as plain intersected
            // fills (rounded corners resume when unclipped); labels
            // render only when the widget rect fits inside the clip.
            let clip_px = placement
                .clip
                .map(|(cx, cy, cw, ch)| (cx * scale, cy * scale, cw * scale, ch * scale));
            if let Some(clip) = clip_px {
                if intersect_rects((px, py, pw, ph), clip).is_none() {
                    continue;
                }
            }
            let fully_visible = clip_px
                .map(|clip| intersect_rects((px, py, pw, ph), clip) == Some((px, py, pw, ph)))
                .unwrap_or(true);
            if !fully_visible {
                if let Some(clip) = clip_px {
                    // A selectable container is mostly empty space, so a
                    // partially visible one must still paint whatever of
                    // its selection wash survives the clip; falling
                    // through to the flat background fill would erase it.
                    if kind_is_selectable(placement.kind) {
                        if let Some((sx, sy, sw, sh)) = placement.selection {
                            if let Some((ix, iy, iw, ih)) = intersect_rects(
                                (sx * scale, sy * scale, sw * scale, sh * scale),
                                clip,
                            ) {
                                fill(&mut ctx, COLOR_SELECTION, ix, iy, iw, ih);
                            }
                        }
                        continue;
                    }
                    if let Some((ix, iy, iw, ih)) = intersect_rects((px, py, pw, ph), clip) {
                        let color = match placement.kind {
                            WidgetKind::Button => button_fill_color(placement.widget, interaction),
                            WidgetKind::TextField | WidgetKind::TextArea => COLOR_FIELD_FILL,
                            WidgetKind::Switch | WidgetKind::Slider | WidgetKind::Progress => {
                                COLOR_TRACK
                            }
                            _ => COLOR_BACKGROUND,
                        };
                        fill(&mut ctx, color, ix, iy, iw, ih);
                    }
                }
                continue;
            }
            let label = placement.label.as_deref().unwrap_or("");
            let (text_color, inset) = match placement.kind {
                WidgetKind::Button => {
                    let color = button_fill_color(placement.widget, interaction);
                    fill_rounded(&mut ctx, color, px, py, pw, ph, 6.0 * scale);
                    (COLOR_BUTTON_LABEL, None)
                }
                WidgetKind::TextField => {
                    fill_rounded(&mut ctx, COLOR_FIELD_BORDER, px, py, pw, ph, 4.0 * scale);
                    fill_rounded(
                        &mut ctx,
                        COLOR_FIELD_FILL,
                        px + scale,
                        py + scale,
                        (pw - 2.0 * scale).max(0.0),
                        (ph - 2.0 * scale).max(0.0),
                        3.0 * scale,
                    );
                    (COLOR_FIELD_TEXT, Some(4.0 * scale))
                }
                WidgetKind::TextArea => {
                    // Same chrome as a field, but the text wraps to the inner
                    // width and starts at the top rather than sitting on one
                    // centered line. Handled here rather than falling through
                    // to the shared label path, which centers a single line.
                    fill_rounded(&mut ctx, COLOR_FIELD_BORDER, px, py, pw, ph, 4.0 * scale);
                    fill_rounded(
                        &mut ctx,
                        COLOR_FIELD_FILL,
                        px + scale,
                        py + scale,
                        (pw - 2.0 * scale).max(0.0),
                        (ph - 2.0 * scale).max(0.0),
                        3.0 * scale,
                    );
                    if !label.is_empty() {
                        let inset = 4.0 * scale;
                        let inner_width = (pw - inset * 2.0).max(1.0);
                        let layout = engine.layout_text(label, scale, Some(inner_width));
                        let _ = draw_layout(
                            &mut ctx,
                            &mut resources,
                            &layout,
                            COLOR_FIELD_TEXT,
                            px + inset,
                            py + inset,
                        );
                    }
                    continue;
                }
                WidgetKind::Text => (COLOR_TEXT, Some(0.0)),
                WidgetKind::ListView | WidgetKind::TreeView => {
                    // Rows paint themselves as child Text placements; the
                    // container contributes only the selection wash, and
                    // never a label of its own.
                    if let Some((sx, sy, sw, sh)) = placement.selection {
                        fill_rounded(
                            &mut ctx,
                            COLOR_SELECTION,
                            sx * scale,
                            sy * scale,
                            sw * scale,
                            sh * scale,
                            3.0 * scale,
                        );
                    }
                    (COLOR_TEXT, None)
                }
                WidgetKind::Checkbox => {
                    let side = (ph.min(18.0 * scale)).max(0.0);
                    let by = py + (ph - side) / 2.0;
                    fill_rounded(
                        &mut ctx,
                        COLOR_FIELD_BORDER,
                        px,
                        by,
                        side,
                        side,
                        3.0 * scale,
                    );
                    fill_rounded(
                        &mut ctx,
                        COLOR_FIELD_FILL,
                        px + scale,
                        by + scale,
                        (side - 2.0 * scale).max(0.0),
                        (side - 2.0 * scale).max(0.0),
                        2.0 * scale,
                    );
                    if placement.checked == Some(true) {
                        let inset = 3.5 * scale;
                        fill_rounded(
                            &mut ctx,
                            COLOR_BUTTON,
                            px + inset,
                            by + inset,
                            (side - 2.0 * inset).max(0.0),
                            (side - 2.0 * inset).max(0.0),
                            1.5 * scale,
                        );
                    }
                    if !label.is_empty() {
                        let layout = engine.layout_label(label, scale);
                        let th = layout.height();
                        let _ = draw_layout(
                            &mut ctx,
                            &mut resources,
                            &layout,
                            COLOR_TEXT,
                            px + side + 8.0 * scale,
                            py + (ph - th) / 2.0,
                        );
                    }
                    continue;
                }
                WidgetKind::Radio => {
                    let side = (ph.min(18.0 * scale)).max(0.0);
                    let r = side / 2.0;
                    let (cx, cy) = (px + r, py + ph / 2.0);
                    fill_circle(&mut ctx, COLOR_FIELD_BORDER, cx, cy, r);
                    fill_circle(&mut ctx, COLOR_FIELD_FILL, cx, cy, (r - scale).max(0.0));
                    if placement.checked == Some(true) {
                        fill_circle(&mut ctx, COLOR_BUTTON, cx, cy, (r - 4.0 * scale).max(0.0));
                    }
                    if !label.is_empty() {
                        let layout = engine.layout_label(label, scale);
                        let th = layout.height();
                        let _ = draw_layout(
                            &mut ctx,
                            &mut resources,
                            &layout,
                            COLOR_TEXT,
                            px + side + 8.0 * scale,
                            py + (ph - th) / 2.0,
                        );
                    }
                    continue;
                }
                WidgetKind::Switch => {
                    let track_w = (36.0 * scale).min(pw);
                    let track_h = (20.0 * scale).min(ph);
                    let ty = py + (ph - track_h) / 2.0;
                    let on = placement.checked == Some(true);
                    let track_color = if on { COLOR_BUTTON } else { COLOR_TRACK };
                    fill_rounded(
                        &mut ctx,
                        track_color,
                        px,
                        ty,
                        track_w,
                        track_h,
                        track_h / 2.0,
                    );
                    let r = (track_h / 2.0 - 2.0 * scale).max(0.0);
                    let cx = if on {
                        px + track_w - r - 2.0 * scale
                    } else {
                        px + r + 2.0 * scale
                    };
                    fill_circle(&mut ctx, COLOR_KNOB, cx, ty + track_h / 2.0, r);
                    continue;
                }
                WidgetKind::Slider | WidgetKind::Progress => {
                    let fraction = placement.value.unwrap_or(0.0).clamp(0.0, 1.0);
                    let groove_h = if placement.kind == WidgetKind::Slider {
                        4.0 * scale
                    } else {
                        6.0 * scale
                    };
                    let gy = py + (ph - groove_h) / 2.0;
                    fill_rounded(&mut ctx, COLOR_TRACK, px, gy, pw, groove_h, groove_h / 2.0);
                    if fraction > 0.0 {
                        fill_rounded(
                            &mut ctx,
                            COLOR_BUTTON,
                            px,
                            gy,
                            pw * fraction,
                            groove_h,
                            groove_h / 2.0,
                        );
                    }
                    if placement.kind == WidgetKind::Slider {
                        let r = (8.0 * scale).min(ph / 2.0);
                        let cx = px + r + (pw - 2.0 * r) * fraction;
                        fill_circle(&mut ctx, COLOR_FIELD_BORDER, cx, py + ph / 2.0, r);
                        fill_circle(
                            &mut ctx,
                            COLOR_KNOB,
                            cx,
                            py + ph / 2.0,
                            (r - scale).max(0.0),
                        );
                    }
                    continue;
                }
                _ => continue,
            };
            if label.is_empty() {
                continue;
            }
            let layout = engine.layout_label(label, scale);
            let (tw, th) = (layout.width(), layout.height());
            let tx = match inset {
                Some(inset) => px + inset,
                None => px + (pw - tw) / 2.0,
            };
            let ty = py + (ph - th) / 2.0;
            if draw_layout(&mut ctx, &mut resources, &layout, text_color, tx, ty) == 0 {
                return false;
            }
        }

        ctx.flush();
        let mut pixmap = Pixmap::new(w16, h16);
        ctx.render_to_pixmap(&mut resources, &mut pixmap);
        for (dst, src) in buffer.iter_mut().zip(pixmap.data().iter()) {
            *dst = 0xFF00_0000 | ((src.r as u32) << 16) | ((src.g as u32) << 8) | (src.b as u32);
        }
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::WidgetId;

    #[test]
    fn vector_labels_are_antialiased() {
        let (w, h) = (200u32, 60u32);
        let mut buffer = vec![0u32; (w * h) as usize];
        let placements = [WidgetPlacement {
            widget: WidgetId::new(1).unwrap(),
            kind: WidgetKind::Button,
            label: Some("Click me".to_string()),
            checked: None,
            value: None,
            selection: None,
            clip: None,
            x: 10.0,
            y: 10.0,
            width: 160.0,
            height: 32.0,
            clickable: false,
            role: None,
        }];
        if !try_paint_placements(
            &mut buffer,
            w,
            h,
            1.0,
            &placements,
            PaintInteraction::default(),
        ) {
            // Host without system fonts: the bitmap fallback covers it.
            eprintln!("skipping: no usable system fonts on this host");
            return;
        }
        // Antialiasing blends label pixels over the button fill: the
        // button area must show more shades than flat fill + flat label.
        let mut shades = std::collections::BTreeSet::new();
        for y in 10..42u32 {
            for x in 10..170u32 {
                shades.insert(buffer[(y * w + x) as usize]);
            }
        }
        assert!(
            shades.len() > 3,
            "expected antialiased blends, found {} shades",
            shades.len()
        );
    }

    #[test]
    fn oversized_surfaces_fall_back() {
        let mut buffer = vec![0u32; 4];
        assert!(!try_paint_placements(
            &mut buffer,
            70_000,
            1,
            1.0,
            &[],
            PaintInteraction::default()
        ));
        assert_eq!(buffer, vec![0u32; 4]);
    }

    #[test]
    fn corners_are_rounded_and_hover_changes_fill() {
        let (w, h) = (200u32, 60u32);
        let button = WidgetPlacement {
            widget: WidgetId::new(1).unwrap(),
            kind: WidgetKind::Button,
            label: None,
            checked: None,
            value: None,
            selection: None,
            clip: None,
            x: 10.0,
            y: 10.0,
            width: 160.0,
            height: 32.0,
            clickable: false,
            role: None,
        };
        let placements = [button];
        let mut plain = vec![0u32; (w * h) as usize];
        if !try_paint_placements(
            &mut plain,
            w,
            h,
            1.0,
            &placements,
            PaintInteraction::default(),
        ) {
            eprintln!("skipping: no usable system fonts on this host");
            return;
        }
        let at = |b: &Vec<u32>, x: u32, y: u32| b[(y * w + x) as usize];
        // The exact rectangle corner sits outside the 6px rounding, so it
        // keeps the background; the button interior is filled.
        assert_eq!(at(&plain, 10, 10), crate::painter::COLOR_BACKGROUND);
        assert_ne!(at(&plain, 20, 20), crate::painter::COLOR_BACKGROUND);

        let mut hovered = vec![0u32; (w * h) as usize];
        let interaction = PaintInteraction {
            hovered: Some(placements[0].widget),
            pressed: None,
        };
        assert!(try_paint_placements(
            &mut hovered,
            w,
            h,
            1.0,
            &placements,
            interaction
        ));
        assert_ne!(
            at(&plain, 20, 20),
            at(&hovered, 20, 20),
            "hover must change the button fill"
        );
    }
}

#[cfg(test)]
mod text_area_tests {
    use super::*;
    use crate::ui::WidgetId;

    fn area(label: &str, w: f32, h: f32) -> WidgetPlacement {
        WidgetPlacement {
            widget: WidgetId::new(1).unwrap(),
            kind: WidgetKind::TextArea,
            label: Some(label.to_string()),
            checked: None,
            value: None,
            selection: None,
            clip: None,
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
            clickable: false,
            role: None,
        }
    }

    /// Count bands of text rows. A wrapped paragraph occupies several bands
    /// separated by leading; a single line occupies one.
    ///
    /// Rows are counted only when they carry several dark pixels. Descenders
    /// like `q` and `y` leave one or two stray pixels in the gap between
    /// lines, which would otherwise bridge two bands into one.
    fn text_row_bands(buffer: &[u32], w: u32, h: u32) -> usize {
        const MIN_DARK_PIXELS: usize = 8;
        let mut bands = 0;
        let mut in_band = false;
        for y in 0..h {
            let dark = (0..w)
                .filter(|&x| {
                    let px = buffer[(y * w + x) as usize];
                    // Field text is dark; the fill behind it is white.
                    let (r, g, b) = ((px >> 16) & 0xFF, (px >> 8) & 0xFF, px & 0xFF);
                    r < 0x80 && g < 0x80 && b < 0x80
                })
                .count();
            let row_has_text = dark >= MIN_DARK_PIXELS;
            if row_has_text && !in_band {
                bands += 1;
            }
            in_band = row_has_text;
        }
        bands
    }

    #[test]
    fn a_text_area_wraps_long_text_onto_several_lines() {
        let (w, h) = (160u32, 120u32);
        let long = "the quick brown fox jumps over the lazy dog and keeps running";

        let mut wrapped = vec![0u32; (w * h) as usize];
        if !try_paint_placements(
            &mut wrapped,
            w,
            h,
            1.0,
            &[area(long, 150.0, 110.0)],
            PaintInteraction::default(),
        ) {
            eprintln!("skipping: no usable system fonts");
            return;
        }

        let bands = text_row_bands(&wrapped, w, h);
        assert!(
            bands >= 2,
            "long text in a narrow area must wrap onto multiple lines, saw {bands} band(s)"
        );
    }

    #[test]
    fn a_text_area_starts_at_the_top_not_the_middle() {
        let (w, h) = (160u32, 120u32);
        let mut buffer = vec![0u32; (w * h) as usize];
        if !try_paint_placements(
            &mut buffer,
            w,
            h,
            1.0,
            &[area("one short line", 150.0, 110.0)],
            PaintInteraction::default(),
        ) {
            eprintln!("skipping: no usable system fonts");
            return;
        }

        // A note editor fills downward from the top. Centering one line would
        // put the first row of text near the middle of the box.
        let first_text_row = (0..h).find(|&y| {
            (0..w).any(|x| {
                let px = buffer[(y * w + x) as usize];
                let (r, g, b) = ((px >> 16) & 0xFF, (px >> 8) & 0xFF, px & 0xFF);
                r < 0x80 && g < 0x80 && b < 0x80
            })
        });
        let first = first_text_row.expect("some text should be painted");
        assert!(
            first < h / 3,
            "text should start near the top, first painted row was {first} of {h}"
        );
    }
}
