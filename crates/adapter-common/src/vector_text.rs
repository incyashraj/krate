//! Vector renderer for the drawn-widget fallback (ADR-0015 renderer slice).
//!
//! Paints the whole frame — background, widget fills, and antialiased
//! text laid out by parley from real system fonts — through `vello_cpu`
//! into the same `0xAARRGGBB` framebuffer the bitmap painter targets.
//! The bitmap painter remains the zero-dependency fallback: callers use
//! [`try_paint_placements`] and fall back when it returns `false`
//! (oversized surface, or a host with no usable system fonts).

use std::cell::RefCell;
use std::collections::HashMap;

use parley::{
    Alignment, AlignmentOptions, FontContext, GenericFamily, Layout, LayoutContext,
    PositionedLayoutItem, StyleProperty,
};
use vello_cpu::color::AlphaColor;
use vello_cpu::kurbo::{Circle, Rect, RoundedRect, Shape};
use vello_cpu::{Glyph, Pixmap, RenderContext, Resources};

use crate::painter::{
    button_fill_color, button_interaction_wash, intersect_rects, PaintInteraction,
    COLOR_BACKGROUND, COLOR_BUTTON, COLOR_BUTTON_LABEL, COLOR_FIELD_BORDER, COLOR_FIELD_FILL,
    COLOR_FIELD_TEXT, COLOR_KNOB, COLOR_SELECTION, COLOR_TEXT, COLOR_TRACK,
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
    /// Shaped metrics, keyed by exactly what determines them.
    ///
    /// Measuring a string costs about 70x drawing it -- 77-87us against 1.1us,
    /// measured by `apps/krate-callcost` (K-414) -- because it runs the full
    /// parley shaping pass and then throws the layout away. Any app that right
    /// -aligns or centres text measures before drawing, so a table or a list
    /// pays that on nearly every string, every frame, for strings that have not
    /// changed.
    static MEASURE_CACHE: RefCell<MeasureCache> = RefCell::new(MeasureCache::new());
}

/// How many measured runs to keep.
///
/// A screenful of a dense table is a few hundred distinct strings; a thousand
/// covers that with room for the headers, chrome and a scroll's worth of
/// churn. Small enough that the whole thing is a few tens of KB.
const MEASURE_CACHE_CAP: usize = 1024;

/// The part of a measurement request that determines its answer.
///
/// Font size is quantised to 1/64th of a pixel so that two requests for
/// "12.0" and "12.000001" share an entry rather than each taking a slot: an
/// f32 is not hashable and its exact bits are not what the answer depends on.
#[derive(PartialEq, Eq, Hash, Clone)]
struct MeasureKey {
    text: String,
    font_size_64ths: u32,
    weight: u16,
    italic: bool,
    letter_spacing_64ths: i32,
    family: u8,
}

/// A bounded cache of shaped-run metrics.
///
/// Cleared wholesale when full rather than evicting one entry at a time. A
/// least-recently-used order would need a second structure and a touch on
/// every hit; the thing being protected is a ~80us shaping pass, and a rare
/// full clear costs one frame of re-measuring the strings still on screen.
struct MeasureCache {
    entries: HashMap<MeasureKey, Option<CanvasTextMetrics>>,
}

impl MeasureCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    fn get(&self, key: &MeasureKey) -> Option<Option<CanvasTextMetrics>> {
        self.entries.get(key).copied()
    }

    fn put(&mut self, key: MeasureKey, value: Option<CanvasTextMetrics>) {
        if self.entries.len() >= MEASURE_CACHE_CAP {
            self.entries.clear();
        }
        self.entries.insert(key, value);
    }
}

/// Build the font stack now, so the first frame does not pay for it.
///
/// Font discovery and the first shaping pass cost about 58 milliseconds, and
/// they land on whichever text call happens first -- measured by timing inside
/// the host: the first 200 calls took 58,445,121 ns between them and the next
/// 200 took 53,957 ns, 0.27us each (K-414). On a 16.6ms frame budget that is a
/// 3.5x overrun on the first frame of any app that draws text, and for an app
/// whose first frame is its only frame it is the entire measurement.
///
/// Call this when a window is created, before anything draws. It is safe to
/// call more than once: the work happens on the first call and later ones find
/// the engine already built.
///
/// The probe string is not empty, because an empty run takes the "Xg" path and
/// would not exercise shaping of real characters.
pub fn warm_text_engine() {
    let _ = measure_canvas_text_uncached("Xg", 12.0, CanvasTextStyle::default());
}

/// Throw away every cached measurement.
///
/// The metrics depend on the fonts the system offers, so anything that can
/// change those -- a font installed or removed while the app runs -- has to
/// invalidate this or the app keeps laying out to a face it is no longer
/// drawing with.
pub fn clear_measure_cache() {
    MEASURE_CACHE.with(|cache| cache.borrow_mut().entries.clear());
}

fn measure_key(text: &str, font_size: f32, style: CanvasTextStyle) -> MeasureKey {
    MeasureKey {
        text: String::from(text),
        // Quantised, and NaN folds to zero rather than never matching itself.
        font_size_64ths: if font_size.is_nan() {
            0
        } else {
            (font_size * 64.0) as u32
        },
        weight: style.weight,
        italic: style.italic,
        letter_spacing_64ths: if style.letter_spacing.is_nan() {
            0
        } else {
            (style.letter_spacing * 64.0) as i32
        },
        family: style.family as u8,
    }
}

struct TextEngine {
    font_cx: FontContext,
    layout_cx: LayoutContext<()>,
    /// Finished canvas layouts, keyed by everything that shapes them.
    canvas_layouts: std::collections::HashMap<CanvasLayoutKey, std::rc::Rc<Layout<()>>>,
    /// Finished canvas RASTERS -- the rendered pixmap of a whole run --
    /// keyed by everything that shapes it plus the color it was inked in.
    ///
    /// Shaping was cached (K-090) and rasterization was not, so every frame
    /// re-rendered every visible line's bezier outlines from scratch: an
    /// editor page cost ~208ms a frame, 4.8 fps under a scroll wheel, on an
    /// M4. Scrolling redraws the same runs merely shifted, which is exactly
    /// what a raster cache turns into a blend-only frame.
    canvas_rasters: std::collections::HashMap<CanvasRasterKey, std::rc::Rc<CanvasRasterRun>>,
    /// Bytes of coverage held in `canvas_rasters`, kept as a running total.
    ///
    /// Previously recomputed on every cache miss by summing `coverage.len()`
    /// over the whole map. That is O(entries) per miss and it reads like a
    /// quadratic trap -- but measured, it is not one: 200 distinct runs
    /// against 800 cost 4.09x with the sum and 4.00x without, both linear.
    /// Rasterising a run dwarfs adding up a few hundred integers.
    ///
    /// Kept anyway because carrying the number is simpler than deriving it and
    /// the bound stays correct as the map grows, but it bought no measurable
    /// speed and nothing here should claim it did.
    canvas_raster_bytes: usize,
}

/// A rendered text run's COVERAGE -- alpha only, one byte per pixel --
/// ready to be inked in any color at blend time.
///
/// Storing colored premultiplied pixels made a full editor page ~57 MB of
/// cache, which slammed into the byte bound and wholesale-cleared nearly
/// every frame: the cache existed and never hit (measured: 194ms/frame in
/// text ops, unchanged). Coverage is a quarter the bytes and one entry
/// serves every ink color.
struct CanvasRasterRun {
    coverage: Vec<u8>,
    width: u32,
    height: u32,
    /// Baseline of the first line, from the pixmap's padded top edge.
    first_baseline: f32,
    /// Whether the run produced any glyphs at all (a false means the caller
    /// must fall back to the bitmap face, cached so the answer is instant).
    drew: bool,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct CanvasRasterKey {
    layout: CanvasLayoutKey,
}

impl TextEngine {
    fn new() -> Self {
        Self {
            font_cx: FontContext::new(),
            layout_cx: LayoutContext::new(),
            canvas_layouts: std::collections::HashMap::new(),
            canvas_rasters: std::collections::HashMap::new(),
            canvas_raster_bytes: 0,
        }
    }

    /// Lay out one line of label text at the given display scale.
    fn layout_label(&mut self, text: &str, scale: f32) -> Layout<()> {
        self.layout_text(text, scale, None)
    }

    /// Lay out one line of label text at an explicit font size.
    ///
    /// The widget path normally fixes the size at `LABEL_FONT_SIZE` and varies
    /// the display scale. A label that names its own size (K-402) needs both:
    /// the size it asked for, still multiplied by the display scale, so a HUD
    /// reading 28px looks the same on a retina panel as on a plain one.
    fn layout_label_sized(&mut self, text: &str, scale: f32, size: f32, bold: bool) -> Layout<()> {
        let mut builder = self
            .layout_cx
            .ranged_builder(&mut self.font_cx, text, scale, true);
        builder.push_default(GenericFamily::SansSerif);
        builder.push_default(StyleProperty::FontSize(size));
        if bold {
            builder.push_default(StyleProperty::FontWeight(parley::FontWeight::BOLD));
        }
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        layout
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
    fn layout_canvas_styled(
        &mut self,
        text: &str,
        font_size: f32,
        style: CanvasTextStyle,
    ) -> std::rc::Rc<Layout<()>> {
        // A canvas app redraws the same strings every frame, and shaping is
        // the expensive, resolution-independent half of text. Cache the
        // finished layout keyed by everything that shapes it; a phone frame
        // was paying ~12 fresh shapes per frame for text that had not
        // changed since the last one (K-090).
        let key = CanvasLayoutKey {
            text: text.to_string(),
            font_size_bits: font_size.to_bits(),
            weight: style.weight,
            italic: style.italic,
            letter_spacing_bits: style.letter_spacing.to_bits(),
            family: style.family,
        };
        if let Some(cached) = self.canvas_layouts.get(&key) {
            // An Rc bump, not a deep clone: cloning the layout itself cost
            // more than the shaping it saved.
            return cached.clone();
        }
        let layout = std::rc::Rc::new(self.layout_canvas_styled_uncached(text, font_size, style));
        // A bounded cache: a guest that generates unbounded distinct
        // strings (a counter, a clock) must not grow the host without
        // limit. Clearing wholesale is fine -- one frame of re-shaping.
        if self.canvas_layouts.len() >= 512 {
            self.canvas_layouts.clear();
        }
        self.canvas_layouts.insert(key, layout.clone());
        layout
    }

    fn layout_canvas_styled_uncached(
        &mut self,
        text: &str,
        font_size: f32,
        style: CanvasTextStyle,
    ) -> Layout<()> {
        let mut builder = self
            .layout_cx
            .ranged_builder(&mut self.font_cx, text, 1.0, true);
        builder.push_default(match style.family {
            CanvasFontFamily::Sans => GenericFamily::SansSerif,
            CanvasFontFamily::Serif => GenericFamily::Serif,
            CanvasFontFamily::Mono => GenericFamily::Monospace,
        });
        builder.push_default(StyleProperty::FontSize(font_size));
        builder.push_default(StyleProperty::FontWeight(parley::FontWeight::new(
            // CSS-style hundreds, clamped to the range every font maps.
            (style.weight as f32).clamp(100.0, 900.0),
        )));
        if style.italic {
            builder.push_default(StyleProperty::FontStyle(parley::FontStyle::Italic));
        }
        if style.letter_spacing != 0.0 {
            builder.push_default(StyleProperty::LetterSpacing(style.letter_spacing));
        }
        let mut layout = builder.build(text);
        layout.break_all_lines(None);
        layout.align(Alignment::Start, AlignmentOptions::default());
        layout
    }
}

/// Everything that decides a canvas layout's shape, as a cache key.
#[derive(Clone, PartialEq, Eq, Hash)]
struct CanvasLayoutKey {
    text: String,
    font_size_bits: u32,
    weight: u16,
    italic: bool,
    letter_spacing_bits: u32,
    family: CanvasFontFamily,
}

/// The generic families the canvas exposes, resolved by fontique against
/// whatever the system has -- the same contract as a web font stack.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CanvasFontFamily {
    #[default]
    Sans,
    Serif,
    Mono,
}

/// Style for a canvas text run. `Default` reproduces plain `draw-text`
/// exactly: regular weight, upright, no tracking, sans.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasTextStyle {
    pub weight: u16,
    pub italic: bool,
    pub letter_spacing: f32,
    pub family: CanvasFontFamily,
}

impl Default for CanvasTextStyle {
    fn default() -> Self {
        Self {
            weight: 400,
            italic: false,
            letter_spacing: 0.0,
            family: CanvasFontFamily::Sans,
        }
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
    draw_canvas_text_styled(
        target,
        text,
        x,
        baseline_y,
        font_size,
        color,
        CanvasTextStyle::default(),
    )
}

/// `draw_canvas_text` with weight, italic, tracking and family. The plain
/// call is the styled call at defaults, so there is exactly one text path.
#[allow(clippy::too_many_arguments)]
pub fn draw_canvas_text_styled(
    target: CanvasTarget<'_>,
    text: &str,
    x: f32,
    baseline_y: f32,
    font_size: f32,
    color: u32,
    style: CanvasTextStyle,
) -> bool {
    draw_canvas_text_clipped(target, text, x, baseline_y, font_size, color, style, None)
}

/// `draw_canvas_text_styled` bounded to a clip rectangle in canvas pixels.
///
/// The clip belongs INSIDE the blend: the caller used to guard clipping by
/// cloning the whole canvas before every text run and walking every canvas
/// pixel after it to restore the outside -- ~1.5ms per run, ~200ms for an
/// editor page, 5 fps under a scroll wheel. Here it is a pair of loop
/// bounds.
#[allow(clippy::too_many_arguments)]
pub fn draw_canvas_text_clipped(
    target: CanvasTarget<'_>,
    text: &str,
    x: f32,
    baseline_y: f32,
    font_size: f32,
    color: u32,
    style: CanvasTextStyle,
    clip: Option<(f32, f32, f32, f32)>,
) -> bool {
    let CanvasTarget {
        buffer,
        width,
        height,
    } = target;
    if text.is_empty() || width == 0 || height == 0 {
        return true;
    }
    // Degenerate clip: nothing can land, and drawing nothing is a success.
    if let Some((_, _, cw, ch)) = clip {
        if cw <= 0.0 || ch <= 0.0 {
            return true;
        }
    }
    let font_size = font_size.clamp(4.0, 256.0);
    // The pixmap's padding, so antialiased edges are not clipped.
    const PAD: f32 = 2.0;
    TEXT_ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        let raster_key = CanvasRasterKey {
            layout: CanvasLayoutKey {
                text: text.to_string(),
                font_size_bits: font_size.to_bits(),
                weight: style.weight,
                italic: style.italic,
                letter_spacing_bits: style.letter_spacing.to_bits(),
                family: style.family,
            },
        };
        let run = match engine.canvas_rasters.get(&raster_key) {
            Some(run) => run.clone(),
            None => {
                let layout = engine.layout_canvas_styled(text, font_size, style);
                let Some(first_line) = layout.lines().next() else {
                    // Whitespace-only: nothing to draw, nothing to fall
                    // back for.
                    return true;
                };
                let first_baseline = first_line.metrics().baseline;
                let pm_w = (layout.width() + PAD * 2.0).ceil() as u32;
                let pm_h = (layout.height() + PAD * 2.0).ceil() as u32;
                let (Ok(w16), Ok(h16)) = (u16::try_from(pm_w), u16::try_from(pm_h)) else {
                    return false;
                };
                if w16 == 0 || h16 == 0 {
                    return true;
                }
                let mut ctx = RenderContext::new(w16, h16);
                let mut resources = Resources::new();
                // Rendered fully opaque white: what gets kept is coverage
                // alone, and the requested ink is applied at blend time.
                let drew =
                    draw_layout(&mut ctx, &mut resources, &layout, 0xFFFF_FFFF, PAD, PAD) != 0;
                let run = if drew {
                    ctx.flush();
                    let mut pixmap = Pixmap::new(w16, h16);
                    ctx.render_to_pixmap(&mut resources, &mut pixmap);
                    CanvasRasterRun {
                        coverage: pixmap.data().iter().map(|px| px.a).collect(),
                        width: pm_w,
                        height: pm_h,
                        first_baseline,
                        drew: true,
                    }
                } else {
                    // Cache the failure too: the caller falls back to the
                    // bitmap face every frame, and re-proving "no glyphs"
                    // by rasterizing nothing is pure waste.
                    CanvasRasterRun {
                        coverage: Vec::new(),
                        width: 0,
                        height: 0,
                        first_baseline,
                        drew: false,
                    }
                };
                // Bounded by bytes, not entries. An editor page of coverage
                // at full backing scale is ~14 MB, so 64 MB holds several
                // screenfuls; clearing wholesale costs one frame of raster.
                //
                // The total is CARRIED rather than recomputed. Measured, this
                // is not faster (4.00x vs 4.09x for 4x the runs -- both
                // linear); it is just simpler than walking the map to derive a
                // number we already know.
                if engine.canvas_raster_bytes > 64 * 1024 * 1024 {
                    engine.canvas_rasters.clear();
                    engine.canvas_raster_bytes = 0;
                }
                let run = std::rc::Rc::new(run);
                engine.canvas_raster_bytes += run.coverage.len();
                if let Some(evicted) = engine.canvas_rasters.insert(raster_key, run.clone()) {
                    // Replacing an entry frees its coverage, so the total has
                    // to come back down or it drifts upward until every frame
                    // clears the cache.
                    engine.canvas_raster_bytes = engine
                        .canvas_raster_bytes
                        .saturating_sub(evicted.coverage.len());
                }
                run
            }
        };
        if !run.drew {
            return false;
        }

        // Blend the run over the canvas, inking the cached coverage in the
        // requested color. The layout's top-left lands at
        // (x, baseline_y - first_baseline); the pad shifts both back.
        let dst_x0 = (x - PAD).floor() as i64;
        let dst_y0 = (baseline_y - run.first_baseline - PAD).floor() as i64;
        let coverage = &run.coverage;
        let pm_w = run.width as usize;
        let ink_a = (color >> 24) & 0xFF;
        let ink_r = (color >> 16) & 0xFF;
        let ink_g = (color >> 8) & 0xFF;
        let ink_b = color & 0xFF;
        // The writable window: the canvas intersected with the clip, as loop
        // bounds rather than per-pixel tests.
        let (win_x0, win_y0, win_x1, win_y1) = match clip {
            Some((cx, cy, cw, ch)) => (
                cx.floor().max(0.0) as i64,
                cy.floor().max(0.0) as i64,
                ((cx + cw).ceil() as i64).min(width as i64),
                ((cy + ch).ceil() as i64).min(height as i64),
            ),
            None => (0, 0, width as i64, height as i64),
        };
        for row in 0..run.height as i64 {
            let by = dst_y0 + row;
            if by < win_y0 || by >= win_y1 {
                continue;
            }
            for col in 0..pm_w as i64 {
                let bx = dst_x0 + col;
                if bx < win_x0 || bx >= win_x1 {
                    continue;
                }
                let cov = coverage[row as usize * pm_w + col as usize] as u32;
                if cov == 0 {
                    continue;
                }
                // Effective alpha: glyph coverage scaled by the ink's own.
                let a = cov * ink_a / 255;
                if a == 0 {
                    continue;
                }
                let di = by as usize * width as usize + bx as usize;
                let dst = buffer[di];
                // Source-over with premultiplied source; dst is opaque.
                let inv = 255 - a;
                let r = (((dst >> 16) & 0xFF) * inv / 255 + ink_r * a / 255).min(255);
                let g = (((dst >> 8) & 0xFF) * inv / 255 + ink_g * a / 255).min(255);
                let b = ((dst & 0xFF) * inv / 255 + ink_b * a / 255).min(255);
                buffer[di] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
            }
        }
        true
    })
}

/// What a canvas text run will occupy: advance width, line height, and the
/// two halves of that height either side of the baseline.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CanvasTextMetrics {
    /// Advance width of the run in canvas pixels.
    pub width: f32,
    /// Full line height (ascent + descent + leading).
    pub height: f32,
    /// Top of the line box down to the baseline.
    pub ascent: f32,
    /// Baseline down to the bottom of the line box.
    pub descent: f32,
}

/// Measure a canvas text run with the same parley layout `draw_canvas_text`
/// draws it with.
///
/// Returns `None` for exactly the case `draw_canvas_text` returns `false` --
/// a non-empty run that produces no glyphs, meaning this host has no usable
/// system fonts and the caller will fall back to the 5x7 bitmap face. The
/// caller then measures that face instead, so the number an app is told always
/// matches the pixels it will get.
///
/// The font size is clamped identically to `draw_canvas_text`, so an app that
/// asks for a 1000pt heading is measured at the 256pt it will actually be
/// drawn at.
pub fn measure_canvas_text(text: &str, font_size: f32) -> Option<CanvasTextMetrics> {
    measure_canvas_text_styled(text, font_size, CanvasTextStyle::default())
}

/// `measure_canvas_text` for a styled run, sharing its layout exactly.
pub fn measure_canvas_text_styled(
    text: &str,
    font_size: f32,
    style: CanvasTextStyle,
) -> Option<CanvasTextMetrics> {
    let font_size = font_size.clamp(4.0, 256.0);
    // Shaping is the expensive part and the answer only depends on the key, so
    // ask the cache before doing it again (K-414).
    let key = measure_key(text, font_size, style);
    if let Some(hit) = MEASURE_CACHE.with(|cache| cache.borrow().get(&key)) {
        return hit;
    }
    let measured = measure_canvas_text_uncached(text, font_size, style);
    MEASURE_CACHE.with(|cache| cache.borrow_mut().put(key, measured));
    measured
}

/// `measure_canvas_text_styled` without the cache: the shaping pass itself.
fn measure_canvas_text_uncached(
    text: &str,
    font_size: f32,
    style: CanvasTextStyle,
) -> Option<CanvasTextMetrics> {
    TEXT_ENGINE.with(|engine| {
        let engine = &mut *engine.borrow_mut();
        // An empty run has no width, but it still has a line: an app sizing an
        // empty input field or placing a caret in it needs the height of the
        // line that is about to hold text. "Xg" spans ascender and descender
        // and is the same proxy the widget path uses.
        let probe = if text.is_empty() { "Xg" } else { text };
        let layout = engine.layout_canvas_styled(probe, font_size, style);
        let line = layout.lines().next()?;
        let metrics = line.metrics();
        if !text.is_empty() && !layout_has_glyphs(&layout) {
            // No usable fonts: the drawing path will fall back to the bitmap
            // face, so refuse to answer with vector numbers it will not honor.
            return None;
        }
        // The drawing path rasterizes into a `u16`-indexed pixmap and gives up
        // on the vector face when the run does not fit, falling back to the
        // bitmap font. Measurement has to give up on exactly the same runs, or
        // a very long string would be measured in one face and drawn in
        // another. Same arithmetic as `draw_canvas_text`.
        let pad = 2.0f32;
        let pm_w = (layout.width() + pad * 2.0).ceil() as u32;
        let pm_h = (layout.height() + pad * 2.0).ceil() as u32;
        if u16::try_from(pm_w).is_err() || u16::try_from(pm_h).is_err() {
            return None;
        }
        Some(CanvasTextMetrics {
            // `full_width`, not `width`: parley's `width()` drops trailing
            // whitespace, which is exactly wrong for the commonest caller. An
            // app placing a caret after someone has typed "hello " needs the
            // pen position, and `width()` would put the caret back on the "o".
            width: if text.is_empty() {
                0.0
            } else {
                layout.full_width()
            },
            height: metrics.line_height,
            ascent: metrics.ascent,
            descent: metrics.descent,
        })
    })
}

/// Whether a laid-out run produced any glyphs at all. The same emptiness test
/// `draw_layout` reports through its drawn count, without rasterizing.
fn layout_has_glyphs(layout: &Layout<()>) -> bool {
    layout.lines().any(|line| {
        line.items().any(|item| match item {
            PositionedLayoutItem::GlyphRun(run) => run.glyphs().count() > 0,
            PositionedLayoutItem::InlineBox(_) => false,
        })
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

/// Stroke a rounded rectangle's OUTLINE, centred on its edge.
///
/// A border cannot be a second fill: filling the whole box in the border
/// colour and insetting the background over it only works when there IS a
/// background. A border-only box has none, so that trick paints a solid
/// rectangle in the border colour -- which is exactly what a bordered foot
/// strip turned into before this existed.
///
/// Inset by half the width so the stroke lands INSIDE the widget's rect.
/// Centred on the edge, half of it would fall outside and overlap whatever is
/// next to it.
fn stroke_rounded(
    ctx: &mut RenderContext,
    color: u32,
    rect: (f32, f32, f32, f32),
    radius: f32,
    width: f32,
) {
    let (x, y, w, h) = rect;
    if width <= 0.0 || w <= 0.0 || h <= 0.0 {
        return;
    }
    let half = width / 2.0;
    let rrect = RoundedRect::new(
        (x + half) as f64,
        (y + half) as f64,
        (x + w - half) as f64,
        (y + h - half) as f64,
        (radius - half).max(0.0) as f64,
    );
    ctx.set_paint(argb(color));
    ctx.set_stroke(vello_cpu::kurbo::Stroke::new(width as f64));
    ctx.stroke_path(&rrect.to_path(0.25));
}

/// Draw one laid-out label with its top-left corner at `(x, y)`.
/// Draw a label with an outline around it, then the label itself.
///
/// The outline is the same glyph run stroked at a ring of offsets and inked
/// in the outline colour, drawn before the fill so the fill lands on top. A
/// ring of eight is what makes a closed edge: four (up, down, left, right)
/// leaves the diagonals of a glyph bare, and a round `O` shows the gap.
///
/// This is the whole answer to K-402. A label over a 3D scene has no fixed
/// background, so no fixed ink colour can be readable against it -- measured
/// on the racing game, one near-black sat at 3.98:1 against sky and 1.27:1
/// against asphalt. A light glyph in a dark outline reads at about 19:1
/// against its own outline whatever is behind it, and where the outline
/// itself disappears into a dark background the light glyph is already
/// high-contrast there by itself.
///
/// Returns the glyph count of the FILL pass, so a caller can still tell that
/// a host produced no glyphs at all and fall back to the bitmap painter.
#[allow(clippy::too_many_arguments)]
fn draw_layout_styled(
    ctx: &mut RenderContext,
    resources: &mut Resources,
    layout: &Layout<()>,
    color: u32,
    x: f32,
    y: f32,
    outline: Option<(u32, f32)>,
    scale: f32,
) -> usize {
    if let Some((outline_color, width)) = outline {
        // Logical pixels, so an outline is the same thickness on a retina
        // panel as on a plain one.
        let w = width * scale;
        if w > 0.0 {
            const RING: [(f32, f32); 8] = [
                (-1.0, -1.0),
                (0.0, -1.0),
                (1.0, -1.0),
                (-1.0, 0.0),
                (1.0, 0.0),
                (-1.0, 1.0),
                (0.0, 1.0),
                (1.0, 1.0),
            ];
            for (dx, dy) in RING {
                draw_layout(
                    ctx,
                    resources,
                    layout,
                    outline_color,
                    x + dx * w,
                    y + dy * w,
                );
            }
        }
    }
    draw_layout(ctx, resources, layout, color, x, y)
}

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

        // Image and canvas pixels are composited into the framebuffer with the
        // shared `draw_image` rather than drawn through vello: vello_cpu has
        // no image primitive here, and `draw_image` already scales, blends and
        // clips exactly as every other host does. Without this, a 3D scene or
        // a 2D canvas fell into the match's catch-all and drew nothing: the
        // window came up blank on Windows and Linux while macOS painted it
        // natively.
        //
        // Each blit records how many vector ops preceded it, because z-order
        // against TEXT depends on it (K-401). Every image used to be blitted
        // after the whole vello pixmap was copied down, so an image always
        // landed on top of every label however the placements were ordered --
        // a full-window canvas erased an entire text layer, which is what a
        // HUD over a 3D scene is. Painting now runs in layers: a vector layer,
        // the images that follow it, the next vector layer, and so on.
        //
        // The common shapes cost exactly one layer, as before: an app with no
        // images, and an app whose canvas is the first thing placed.
        let mut blits: Vec<ImageBlit<'_>> = Vec::new();
        // Vector work waiting to be rendered, and the layers already closed.
        // A layer closes at every image, so that image composites over the
        // layer below it and the NEXT layer's text composites over the image.
        let mut layers: Vec<(RenderContext, Vec<ImageBlit<'_>>)> = Vec::new();

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
            // The app's own box, under everything the kind draws.
            //
            // Painted here rather than inside the kind arms so it works for a
            // container too: a `stack` draws nothing of its own, and a stack
            // the app gave a background and a radius is a card that has to be
            // filled before its children land on top of it.
            //
            // A border is a rounded rect in the border colour with the
            // background inset inside it -- the same two-fill trick the text
            // field already uses for its chrome, rather than a stroke, so the
            // corners stay consistent with everything else here.
            if let Some(style) = placement.r#box {
                if style.paints_anything() {
                    let radius = style.corner_radius * scale;
                    let bw = style.border_width * scale;
                    // Background first, then the border STROKED on top of its
                    // edge. The background is the full rect: the stroke sits
                    // inside the same bounds and covers the outermost pixels
                    // of it, so there is no seam between the two.
                    if let Some(background) = style.background {
                        fill_rounded(&mut ctx, background.argb(), px, py, pw, ph, radius);
                    }
                    if let Some(border) = style.border {
                        stroke_rounded(&mut ctx, border.argb(), (px, py, pw, ph), radius, bw);
                    }
                }
            }

            let label = placement.label.as_deref().unwrap_or("");
            let (text_color, inset) = match placement.kind {
                WidgetKind::Button => {
                    // An app that named a background has already had it
                    // painted above; painting the host's blue over it would
                    // make `box` do nothing on the one widget people most
                    // want to restyle.
                    //
                    // Hover and press still show, as a wash over whatever the
                    // fill turned out to be, so a restyled button still
                    // answers the pointer.
                    let own_background =
                        placement.r#box.and_then(|style| style.background).is_some();
                    if !own_background {
                        let color = button_fill_color(placement.widget, interaction);
                        fill_rounded(&mut ctx, color, px, py, pw, ph, 6.0 * scale);
                    } else if let Some(wash) =
                        button_interaction_wash(placement.widget, interaction)
                    {
                        let radius = placement
                            .r#box
                            .map(|style| style.corner_radius * scale)
                            .unwrap_or(6.0 * scale);
                        fill_rounded(&mut ctx, wash, px, py, pw, ph, radius);
                    }
                    // The host's white label is right on the host's blue and
                    // wrong on an arbitrary background, so an app that picks
                    // its own fill picks its own ink too (via `text`), and
                    // gets the ordinary label colour if it does not.
                    let ink = if own_background {
                        COLOR_TEXT
                    } else {
                        COLOR_BUTTON_LABEL
                    };
                    (ink, None)
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
                        // Close the layer this image sits above, so anything
                        // drawn after it -- a HUD label, a caption -- lands on
                        // top of it rather than under it (K-401).
                        //
                        // The image is pushed BEFORE the close, so it travels
                        // with the layer it covers. Closing first leaves it in
                        // the next layer's list, where it is composited after
                        // that layer's text and paints over the very labels
                        // this is meant to keep visible. The backdrop fill
                        // just above likewise belongs under the image, which
                        // is why it is drawn before either.
                        //
                        // Unconditional. An earlier version only closed when
                        // something had been drawn into the layer, which was
                        // always true by the line above and so decided
                        // nothing -- clippy caught the dead flag. Closing
                        // every time costs one empty vello pass for an app
                        // that places two images in a row, and keeps the rule
                        // to one sentence.
                        let done = core::mem::replace(&mut ctx, RenderContext::new(w16, h16));
                        layers.push((done, core::mem::take(&mut blits)));
                    }
                    continue;
                }
                _ => continue,
            };
            if label.is_empty() {
                continue;
            }
            // What the app asked for, if it asked. `sanitized` has already
            // bounded every number by the time a placement reaches a painter.
            let style = placement.text;
            let layout = match style.and_then(|s| s.size.map(|size| (size, s.bold))) {
                Some((size, bold)) => engine.layout_label_sized(label, scale, size, bold),
                None => engine.layout_label(label, scale),
            };
            let ink = style
                .and_then(|s| s.color)
                .map(|c| c.argb())
                .unwrap_or(text_color);
            let outline = style.and_then(|s| s.outline.map(|c| (c.argb(), s.outline_width)));
            let (tw, th) = (layout.width(), layout.height());
            let tx = match inset {
                Some(inset) => px + inset,
                None => px + (pw - tw) / 2.0,
            };
            let ty = py + (ph - th) / 2.0;
            if draw_layout_styled(
                &mut ctx,
                &mut resources,
                &layout,
                ink,
                tx,
                ty,
                outline,
                scale,
            ) == 0
            {
                return false;
            }
        }

        layers.push((ctx, blits));

        // Composite the layers bottom to top: each one's vector content, then
        // the images that were placed above it.
        //
        // The first layer is opaque (it opens with the background fill), so it
        // is copied straight down. Every later layer is drawn OVER what is
        // already there, which is why its untouched pixels have to stay
        // untouched -- vello renders them as transparent black, and copying
        // those down would erase the layers below. Only pixels the layer
        // actually painted are taken, blended by their own alpha.
        let mut pixmap = Pixmap::new(w16, h16);
        for (index, (mut layer, layer_blits)) in layers.into_iter().enumerate() {
            layer.flush();
            pixmap
                .data_mut()
                .fill(vello_cpu::peniko::color::PremulRgba8 {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 0,
                });
            layer.render_to_pixmap(&mut resources, &mut pixmap);
            for (dst, src) in buffer.iter_mut().zip(pixmap.data().iter()) {
                if index == 0 {
                    *dst = 0xFF00_0000
                        | ((src.r as u32) << 16)
                        | ((src.g as u32) << 8)
                        | (src.b as u32);
                    continue;
                }
                // Source-over with premultiplied source, which is what
                // `render_to_pixmap` produces. Fully transparent source
                // leaves the destination exactly as it was.
                if src.a == 0 {
                    continue;
                }
                if src.a == 255 {
                    *dst = 0xFF00_0000
                        | ((src.r as u32) << 16)
                        | ((src.g as u32) << 8)
                        | (src.b as u32);
                    continue;
                }
                let inv = 255 - src.a as u32;
                let dr = (*dst >> 16) & 0xFF;
                let dg = (*dst >> 8) & 0xFF;
                let db = *dst & 0xFF;
                let r = (src.r as u32 + (dr * inv + 127) / 255).min(255);
                let g = (src.g as u32 + (dg * inv + 127) / 255).min(255);
                let b = (src.b as u32 + (db * inv + 127) / 255).min(255);
                *dst = 0xFF00_0000 | (r << 16) | (g << 8) | b;
            }
            // Images over the layer they were placed above, with the shared
            // rasterizer so a scene or canvas lands with the same scaling and
            // alpha blending on every host.
            for blit in layer_blits {
                crate::painter::draw_image(buffer, width, height, blit.rect, blit.image, blit.clip);
            }
        }
        true
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn warming_the_engine_makes_the_first_measured_call_cheap() {
        // K-414. Font discovery plus the first shaping pass costs about 58ms
        // and lands on whichever text call happens FIRST, so the first frame
        // of any app that draws text overran a 16.6ms budget by 3.5x. The
        // cost is real and one-time; the bug is that an app's frame paid it.
        //
        // Measured rather than asserted structurally: a test that only checked
        // `warm_text_engine` exists would pass with an empty body.
        use std::time::Instant;

        // Whatever this process has already done to the engine, this is the
        // call that guarantees it is built.
        warm_text_engine();

        // Now a fresh string -- not the warm probe's "Xg", so no cache entry
        // can be standing in for the work.
        let started = Instant::now();
        let _ = measure_canvas_text("a first real measurement", 13.0);
        let warm = started.elapsed();

        // The pre-fix path spent ~58 MILLISECONDS here. A whole millisecond is
        // far above the ~0.3us a warm call takes and far below the failure,
        // so it separates the two without being flaky on a loaded machine.
        assert!(
            warm.as_millis() < 10,
            "a measurement after warming must not pay font-stack init: took {warm:?}"
        );
    }

    #[test]
    fn measuring_the_same_run_twice_gives_the_same_answer() {
        // The cache must not change what an app is told. A wrong key would
        // hand back another string's metrics, which is worse than being slow.
        warm_text_engine();
        let a = measure_canvas_text("the quick brown fox", 14.0);
        let b = measure_canvas_text("the quick brown fox", 14.0);
        assert_eq!(a.map(|m| m.width), b.map(|m| m.width));

        // And two runs that differ in ANY key component must not share an
        // entry. Different text, different size, different style.
        let other = measure_canvas_text("the quick brown foxes", 14.0);
        let bigger = measure_canvas_text("the quick brown fox", 28.0);
        if let (Some(a), Some(other), Some(bigger)) = (a, other, bigger) {
            assert!(
                other.width > a.width,
                "a longer string must measure wider: {} vs {}",
                other.width,
                a.width
            );
            assert!(
                bigger.width > a.width,
                "the same string at twice the size must measure wider: {} vs {}",
                bigger.width,
                a.width
            );
        }
    }
    use super::*;
    use crate::ui::{Color, TextStyle, WidgetId};

    #[test]
    fn an_outlined_label_reads_against_a_background_its_own_colour() {
        // K-402. The worst case for a HUD: white text over a background that
        // is ALSO white. With one fixed ink colour there is nothing to read;
        // the outline is what draws the edge.
        let (w, h) = (200u32, 60u32);
        let white = Color {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        };
        let ink = Color {
            r: 10,
            g: 12,
            b: 18,
            a: 255,
        };

        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            rgba.extend_from_slice(&[255, 255, 255, 255]);
        }
        let canvas = WidgetPlacement {
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
            pixels: Some(std::sync::Arc::new(
                ImagePixels::new(w, h, rgba).expect("white background"),
            )),
            text: None,
            r#box: None,
        };

        let label = |style: Option<TextStyle>| WidgetPlacement {
            widget: WidgetId::new(2).unwrap(),
            kind: WidgetKind::Text,
            label: Some("LAP 1 OF 3".to_string()),
            pixels: None,
            x: 6.0,
            y: 6.0,
            width: 180.0,
            height: 30.0,
            text: style,
            ..canvas.clone()
        };

        let paint = |style: Option<TextStyle>| -> Option<Vec<u32>> {
            let mut buffer = vec![0u32; (w * h) as usize];
            try_paint_placements(
                &mut buffer,
                w,
                h,
                1.0,
                &[canvas.clone(), label(style)],
                PaintInteraction::default(),
            )
            .then_some(buffer)
        };

        // White text, NO outline, on white: nothing to see.
        let plain = TextStyle {
            color: Some(white),
            outline: None,
            outline_width: 0.0,
            size: Some(24.0),
            bold: true,
        };
        let Some(without) = paint(Some(plain.sanitized())) else {
            eprintln!("skipping: no usable system fonts on this host");
            return;
        };
        let dark_without = without
            .iter()
            .filter(|v| ((**v >> 16) & 0xFF) < 128)
            .count();

        // The same text WITH an outline: the edge is now drawn.
        let outlined = TextStyle {
            outline: Some(ink),
            outline_width: 2.0,
            ..plain
        };
        let with = paint(Some(outlined.sanitized())).expect("fonts were available a moment ago");
        let dark_with = with.iter().filter(|v| ((**v >> 16) & 0xFF) < 128).count();

        assert_eq!(
            dark_without, 0,
            "white on white must be invisible, or this test proves nothing"
        );
        assert!(
            dark_with > 200,
            "the outline must draw a readable edge; found {dark_with} dark pixels"
        );
    }

    #[test]
    fn a_text_style_is_bounded_before_it_reaches_a_painter() {
        // A guest names these numbers, so they are clamped rather than
        // trusted. An unbounded outline is unbounded host work per label per
        // frame, and a NaN would travel into a layout.
        let wild = TextStyle {
            color: None,
            outline: Some(Color {
                r: 0,
                g: 0,
                b: 0,
                a: 255,
            }),
            outline_width: 4000.0,
            size: Some(100_000.0),
            bold: false,
        }
        .sanitized();
        assert_eq!(wild.outline_width, TextStyle::MAX_OUTLINE_WIDTH);
        assert_eq!(wild.size, Some(TextStyle::MAX_SIZE));

        let nan = TextStyle {
            outline_width: f32::NAN,
            size: Some(f32::NAN),
            ..wild
        }
        .sanitized();
        assert_eq!(nan.outline_width, 0.0, "a NaN width is a bug, not a width");
        assert_eq!(nan.size, None, "a NaN size falls back to the host default");
        assert!(
            nan.outline.is_none(),
            "a zero-width outline is no outline, whatever colour it names"
        );
    }

    #[test]
    fn a_label_over_a_full_window_canvas_survives_the_blit() {
        // K-401. The vector painter used to render every image AFTER copying
        // the whole vello pixmap down, so an image landed on top of all text
        // whatever the placement order. A HUD over a 3D scene -- a canvas that
        // fills the window with a label above it, which is exactly what the
        // overlay widget is for -- lost its text entirely.
        let (w, h) = (200u32, 60u32);
        // Opaque mid-grey: RGB 0x5A with alpha 0xFF. A uniform `vec![90; n]`
        // would set the ALPHA to 90 too and the scene would blend to something
        // else, which is a fixture bug that reads as a painter bug.
        //
        // Sized to the window's own aspect ratio on purpose: `draw_image` FITS
        // rather than fills, so a square scene in a wide window is letterboxed
        // and the glyphs peek through the bars. That made an earlier version
        // of this test survive the bug it was written to catch.
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            rgba.extend_from_slice(&[0x5A, 0x5A, 0x5A, 0xFF]);
        }
        let pixels = std::sync::Arc::new(ImagePixels::new(w, h, rgba).expect("scene pixels"));
        let scene = WidgetPlacement {
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
            pixels: Some(pixels),
            text: None,
            r#box: None,
        };
        let label = WidgetPlacement {
            widget: WidgetId::new(2).unwrap(),
            kind: WidgetKind::Text,
            label: Some("LAP 1 OF 3".to_string()),
            pixels: None,
            ..scene.clone()
        };
        let label = WidgetPlacement {
            x: 4.0,
            y: 4.0,
            width: 180.0,
            height: 20.0,
            ..label
        };

        let mut buffer = vec![0u32; (w * h) as usize];
        if !try_paint_placements(
            &mut buffer,
            w,
            h,
            1.0,
            &[scene, label],
            PaintInteraction::default(),
        ) {
            eprintln!("skipping: no usable system fonts on this host");
            return;
        }

        // The scene is a flat grey; the label is inked in COLOR_TEXT. Any
        // pixel of exactly that colour is a glyph that survived the blit.
        // Prove the fixture is hostile before trusting the assertion: the
        // scene must actually cover the ground the label sits on, or the test
        // passes for the wrong reason.
        let uncovered = (4..24u32)
            .flat_map(|y| (4..184u32).map(move |x| (x, y)))
            .filter(|(x, y)| buffer[(y * w + x) as usize] & 0x00FF_FFFF != 0x005A_5A5A)
            .filter(|(x, y)| buffer[(y * w + x) as usize] & 0x00FF_FFFF != COLOR_TEXT & 0x00FF_FFFF)
            .count();
        assert!(
            uncovered < 400,
            "the scene must cover the ground the label sits on, or this test \
             passes for the wrong reason: {uncovered} pixels in the label band \
             are neither scene nor glyph"
        );
        let text_pixels = buffer
            .iter()
            .filter(|v| **v & 0x00FF_FFFF == COLOR_TEXT & 0x00FF_FFFF)
            .count();
        assert!(
            text_pixels > 0,
            "the canvas blitted over the label: no text pixels left"
        );
        // And the scene is still there underneath, rather than the label
        // having been achieved by dropping the image.
        let scene_pixels = buffer
            .iter()
            .filter(|v| **v & 0x00FF_FFFF == 0x005A_5A5A)
            .count();
        // `draw_image` fits rather than fills, so a square scene in a wide
        // window is letterboxed and covers a band, not the whole frame. What
        // matters is that it is still there in quantity: the label was not
        // rescued by dropping the image.
        assert!(
            scene_pixels > 1000,
            "the scene should still be painted, found {scene_pixels} of its pixels"
        );
    }

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
            text: None,
            r#box: None,
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
            text: None,
            r#box: None,
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
            text: None,
            r#box: None,
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
            text: None,
            r#box: None,
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
