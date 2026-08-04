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
use crate::ui::{kind_is_selectable, ImagePixels, WidgetKind, WidgetPlacement};

/// Logical font size for widget labels; multiplied by the scale factor
/// through parley's display scale.
const LABEL_FONT_SIZE: f32 = 13.0;

/// One image or canvas queued to blit over the finished vello frame.
/// `rect` and `clip` are in physical pixels, ready for `draw_image`.
struct ImageBlit<'a> {
    rect: (f32, f32, f32, f32),
    clip: Option<(f32, f32, f32, f32)>,
    image: &'a ImagePixels,
}

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

    /// Width of the first `chars` characters of `text`, laid out unwrapped.
    /// Used to place the caret and selection edges: the note is short and
    /// single line in practice, so an unwrapped prefix measures the exact x.
    fn prefix_width(&mut self, text: &str, chars: usize, scale: f32) -> f32 {
        if chars == 0 {
            return 0.0;
        }
        let end = text
            .char_indices()
            .nth(chars)
            .map(|(index, _)| index)
            .unwrap_or(text.len());
        let prefix = text.get(..end).unwrap_or(text);
        if prefix.is_empty() {
            return 0.0;
        }
        self.layout_text(prefix, scale, None).width()
    }

    /// Height of one text line at this scale, for sizing the caret and wash.
    fn line_height(&mut self, scale: f32) -> f32 {
        // "Xg" spans an ascender and descender, a stable line-box proxy.
        self.layout_text("Xg", scale, None).height()
    }

    /// Lay out canvas text at an explicit font size (display scale 1).
    ///
    /// The widget path fixes the size at `LABEL_FONT_SIZE` and varies the
    /// display scale; a canvas run is the opposite -- the guest names the size
    /// in the draw-text call, so it is pushed directly.
    fn layout_canvas(&mut self, text: &str, font_size: f32) -> Layout<()> {
        let mut builder = self
            .layout_cx
            .ranged_builder(&mut self.font_cx, text, 1.0, true);
        builder.push_default(GenericFamily::SansSerif);
        builder.push_default(StyleProperty::FontSize(font_size));
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        layout
    }
}

/// The canvas being drawn into: the pixel buffer and its dimensions.
///
/// These three always travel together and are meaningless apart -- a buffer
/// without its width cannot be indexed. Grouping them also keeps
/// `draw_canvas_text` inside clippy's argument limit without silencing the
/// lint, which was pointing at a real signature smell.
pub struct CanvasTarget<'a> {
    /// `0xAARRGGBB` pixels, `width * height` long.
    pub buffer: &'a mut [u32],
    pub width: u32,
    pub height: u32,
}

/// Draw one antialiased text run into an `0xAARRGGBB` canvas buffer.
///
/// `(x, baseline_y)` follows the canvas draw-text contract: the origin is the
/// text baseline. Glyphs are laid out by parley from real system fonts at the
/// exact `font_size` and rasterized by vello_cpu with antialiasing, then
/// source-over blended onto the existing canvas content -- so text sits cleanly
/// on whatever the app drew underneath.
///
/// Returns `false` when the text produces no glyphs (a host with no usable
/// system fonts); the caller then falls back to the 5x7 bitmap font, so text
/// never silently disappears. This is what replaced the bitmap font as the
/// canvas default: at anything above small sizes the 5x7 face reads as
/// pixel-art, and every canvas app looked blocky no matter how carefully it
/// was designed.
pub fn draw_canvas_text(
    target: CanvasTarget<'_>,
    text: &str,
    x: f32,
    baseline_y: f32,
    font_size: f32,
    color: u32,
) -> bool {
    let CanvasTarget {
        buffer,
        width,
        height,
    } = target;
    if text.is_empty() || width == 0 || height == 0 {
        return true;
    }
    let font_size = font_size.clamp(4.0, 256.0);
    TEXT_ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let layout = engine.layout_canvas(text, font_size);
        let Some(first_line) = layout.lines().next() else {
            // Whitespace-only text: nothing to draw, nothing to fall back for.
            return true;
        };
        let first_baseline = first_line.metrics().baseline;

        // Rasterize into a pixmap just large enough for the run, padded so
        // antialiased edges are not clipped.
        let pad = 2.0f32;
        let pm_w = (layout.width() + pad * 2.0).ceil() as u32;
        let pm_h = (layout.height() + pad * 2.0).ceil() as u32;
        let (Ok(w16), Ok(h16)) = (u16::try_from(pm_w), u16::try_from(pm_h)) else {
            return false;
        };
        if w16 == 0 || h16 == 0 {
            return true;
        }
        let mut ctx = RenderContext::new(w16, h16);
        let mut resources = Resources::new();
        if draw_layout(&mut ctx, &mut resources, &layout, color, pad, pad) == 0 {
            return false;
        }
        ctx.flush();
        let mut pixmap = Pixmap::new(w16, h16);
        ctx.render_to_pixmap(&mut resources, &mut pixmap);

        // Blend the run over the canvas. The layout's top-left lands at
        // (x, baseline_y - first_baseline); the pad shifts both back.
        let dst_x0 = (x - pad).floor() as i64;
        let dst_y0 = (baseline_y - first_baseline - pad).floor() as i64;
        let data = pixmap.data();
        let pm_w = w16 as usize;
        for row in 0..h16 as i64 {
            let by = dst_y0 + row;
            if by < 0 || by >= height as i64 {
                continue;
            }
            for col in 0..pm_w as i64 {
                let bx = dst_x0 + col;
                if bx < 0 || bx >= width as i64 {
                    continue;
                }
                let src = data[row as usize * pm_w + col as usize];
                if src.a == 0 {
                    continue;
                }
                let di = by as usize * width as usize + bx as usize;
                let dst = buffer[di];
                // Source-over: src is premultiplied, dst is opaque 0xAARRGGBB.
                let inv = 255 - src.a as u32;
                let r = (((dst >> 16) & 0xFF) * inv / 255 + src.r as u32).min(255);
                let g = (((dst >> 8) & 0xFF) * inv / 255 + src.g as u32).min(255);
                let b = ((dst & 0xFF) * inv / 255 + src.b as u32).min(255);
                buffer[di] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
            }
        }
        true
    })
}

/// Clamp a character offset to a string's character count, so a caret past the
/// end of the text lands at the end rather than out of range.
fn clamp_char_offset(text: &str, offset: usize) -> usize {
    offset.min(text.chars().count())
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

        // Image and canvas pixels are blitted into the framebuffer after the
        // vello pixmap is copied over -- vello_cpu has no image primitive here,
        // and the shared `draw_image` already scales, blends, and clips exactly
        // as every other host does. Collected in the same pass so a scene draws
        // in the same z-order it was placed. Without this, a 3D scene or a 2D
        // canvas fell into the match's catch-all and drew nothing: the window
        // came up blank on Windows and Linux while macOS painted it natively.
        let mut blits: Vec<ImageBlit<'_>> = Vec::new();

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
                    let inset = 4.0 * scale;
                    let inner_width = (pw - inset * 2.0).max(1.0);

                    // Selection wash and caret sit under the glyphs. Parley is
                    // proportional, so positions come from measuring prefixes
                    // rather than a fixed cell. The note is short and single
                    // line in practice; measuring the prefix width places the
                    // caret exactly there, and the selection spans between two
                    // such measurements.
                    if let Some((cursor, anchor)) = placement.text_cursor {
                        let cursor = clamp_char_offset(label, cursor as usize);
                        let anchor = clamp_char_offset(label, anchor as usize);
                        let sel_start = cursor.min(anchor);
                        let sel_end = cursor.max(anchor);
                        let line_top = py + inset;
                        let line_h = engine.line_height(scale);

                        if sel_start != sel_end {
                            let x0 = px + inset + engine.prefix_width(label, sel_start, scale);
                            let x1 = px + inset + engine.prefix_width(label, sel_end, scale);
                            fill(
                                &mut ctx,
                                COLOR_SELECTION,
                                x0,
                                line_top,
                                (x1 - x0).max(1.0),
                                line_h,
                            );
                        }

                        let caret_x = px + inset + engine.prefix_width(label, cursor, scale);
                        fill(
                            &mut ctx,
                            COLOR_FIELD_TEXT,
                            caret_x,
                            line_top,
                            scale.max(1.0),
                            line_h,
                        );
                    }

                    if !label.is_empty() {
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
                WidgetKind::Image | WidgetKind::Canvas => {
                    // A backdrop under an image (so a picture that does not
                    // fill its rect sits on a panel, matching the bitmap
                    // painter), then the pixels themselves queued for the
                    // post-pixmap blit. A canvas with no pixels yet draws
                    // nothing rather than a stray panel.
                    if placement.kind == WidgetKind::Image || placement.pixels.is_some() {
                        fill(&mut ctx, COLOR_FIELD_FILL, px, py, pw, ph);
                    }
                    if let Some(image) = placement.pixels.as_deref() {
                        blits.push(ImageBlit {
                            rect: (px, py, pw, ph),
                            clip: clip_px,
                            image,
                        });
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
        // Composite images over the finished vello frame with the shared
        // rasterizer, so a scene or canvas lands with the same scaling and
        // alpha blending on every host.
        for blit in blits {
            crate::painter::draw_image(buffer, width, height, blit.rect, blit.image, blit.clip);
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
            text_cursor: None,
            clip: None,
            x: 10.0,
            y: 10.0,
            width: 160.0,
            height: 32.0,
            clickable: false,
            role: None,
            pixels: None,
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
    fn a_canvas_draws_its_pixels_not_a_blank_frame() {
        // The regression that made 3D and 2D-canvas apps come up as a white
        // window on Windows and Linux: the vello painter had no arm for Image
        // or Canvas, so a scene's pixels were dropped and the frame stayed at
        // the background colour. A solid-red canvas must put red on screen.
        let (w, h) = (64u32, 48u32);
        let mut buffer = vec![0u32; (w * h) as usize];
        let red = ImagePixels::new(
            w,
            h,
            std::iter::repeat_n([255u8, 0, 0, 255], (w * h) as usize)
                .flatten()
                .collect(),
        )
        .expect("red image");
        let placements = [WidgetPlacement {
            widget: WidgetId::new(1).unwrap(),
            kind: WidgetKind::Canvas,
            label: None,
            checked: None,
            value: None,
            selection: None,
            text_cursor: None,
            clip: None,
            x: 0.0,
            y: 0.0,
            width: w as f32,
            height: h as f32,
            clickable: false,
            role: None,
            pixels: Some(std::sync::Arc::new(red)),
        }];
        assert!(try_paint_placements(
            &mut buffer,
            w,
            h,
            1.0,
            &placements,
            PaintInteraction::default(),
        ));
        // Centre pixel is opaque red, not the background wash.
        let centre = buffer[((h / 2) * w + w / 2) as usize];
        assert_eq!(
            centre & 0x00FF_FFFF,
            0x00FF_0000,
            "the canvas pixels must reach the framebuffer, got {centre:#010x}"
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
            text_cursor: None,
            clip: None,
            x: 10.0,
            y: 10.0,
            width: 160.0,
            height: 32.0,
            clickable: false,
            role: None,
            pixels: None,
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
            text_cursor: None,
            clip: None,
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
            clickable: false,
            role: None,
            pixels: None,
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
