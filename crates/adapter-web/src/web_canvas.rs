//! `krate:gfx/canvas2d` and `krate:gfx/canvas-images` in a tab (IC-956).
//!
//! The same rasterizer the desktop draws a canvas with (`krate_lowering::
//! canvas_raster`), bound the same way: a canvas takes its widget's laid-out
//! size at the tab's pixel density, an app that fixed a design size draws in
//! that system (and its pointer events arrive in it), an app that never asked
//! its size gets its window's requested size as its design when the tab gave
//! it less room, and `present` publishes the pixels through the image-widget
//! path the painter already draws.

use std::sync::Arc;

use krate_adapter_common::ui::{ImagePixels, WidgetId, WidgetKind, WindowId};
use krate_adapter_common::vector_text::{CanvasFontFamily, CanvasTextStyle};
use krate_lowering::canvas_raster::{pack_color, CanvasSurface};
use serde_json::{json, Value};

use crate::web_ui::WebUi;

/// The WIT `gfx-error` cases, in jco's shape.
pub fn invalid_target() -> Value {
    json!({ "tag": "invalid-target" })
}
fn unsupported(words: impl Into<String>) -> Value {
    json!({ "tag": "unsupported", "val": words.into() })
}

/// One colour as the WIT carries it: four floats.
#[derive(Clone, Copy, Debug)]
pub struct Rgba(pub f32, pub f32, pub f32, pub f32);

impl Rgba {
    fn packed(self) -> u32 {
        pack_color(self.0, self.1, self.2, self.3)
    }
}

/// A rectangle, `(x, y, width, height)`.
pub type Area = (f32, f32, f32, f32);
/// Corner radii, `(top-left, top-right, bottom-right, bottom-left)`.
pub type Radii = (f32, f32, f32, f32);

pub fn text_style(weight: u16, italic: bool, letter_spacing: f32, family: u8) -> CanvasTextStyle {
    CanvasTextStyle {
        weight,
        italic,
        letter_spacing,
        family: match family {
            1 => CanvasFontFamily::Serif,
            2 => CanvasFontFamily::Mono,
            _ => CanvasFontFamily::Sans,
        },
    }
}

/// What the kept pictures of one run may hold, the desktop's bound.
const KEPT_IMAGE_BYTES: usize = 256 * 1024 * 1024;

impl WebUi {
    fn canvas_rect(&self, window: WindowId, widget: WidgetId) -> Result<(f32, f32), Value> {
        let tree = self.tree_of(window).ok_or_else(invalid_target)?;
        match tree.node(widget) {
            Some(node) if node.kind == WidgetKind::Canvas => {}
            Some(_) => return Err(unsupported("canvas2d binds to a widget of kind canvas")),
            None => return Err(invalid_target()),
        }
        let (tree, layout) = self.layout_of(window).ok_or_else(invalid_target)?;
        let rect =
            krate_layout::absolute_rect(&tree, &layout, widget).ok_or_else(invalid_target)?;
        Ok((rect.width, rect.height))
    }

    /// `canvas2d.bind`: a canvas for the canvas widget `widget`.
    pub fn canvas_bind(&mut self, window: u64, widget: u64) -> Result<u64, Value> {
        let window = self.window_id_of(window).map_err(|_| invalid_target())?;
        let widget = WidgetId::new(widget).map_err(|_| invalid_target())?;
        let (w, h) = self.canvas_rect(window, widget)?;
        let surface = CanvasSurface::new_scaled(w.max(1.0) as u32, h.max(1.0) as u32, self.scale())
            .map_err(|e| unsupported(e.to_string()))?;
        let id = self.canvas.next;
        self.canvas.next += 1;
        self.canvas.surfaces.insert(id, (window, widget, surface));
        Ok(id)
    }

    fn surface(&mut self, canvas: u64) -> Result<&mut CanvasSurface, Value> {
        self.canvas
            .surfaces
            .get_mut(&canvas)
            .map(|(_, _, s)| s)
            .ok_or_else(invalid_target)
    }

    /// Re-fit a canvas to its widget's current rect (the tab may have
    /// resized the window since it was bound).
    fn refit(&mut self, canvas: u64) -> Result<(), Value> {
        let (window, widget) = self
            .canvas
            .surfaces
            .get(&canvas)
            .map(|(w, g, _)| (*w, *g))
            .ok_or_else(invalid_target)?;
        let (w, h) = self.canvas_rect(window, widget)?;
        let scale = self.scale();
        self.surface(canvas)?
            .resize_scaled(w.max(1.0) as u32, h.max(1.0) as u32, scale)
            .map_err(|e| unsupported(e.to_string()))?;
        Ok(())
    }

    /// `canvas2d.canvas-size`: the design size if the app fixed one, else
    /// the canvas's laid-out size. An app that asks is responsive, so a
    /// design the host chose for it goes.
    pub fn canvas_size(&mut self, canvas: u64) -> Result<(f32, f32), Value> {
        self.canvas.asked.insert(canvas);
        if self.canvas.implicit.remove(&canvas) {
            self.surface(canvas)?.set_design_size(0.0, 0.0);
        }
        self.refit(canvas)?;
        let surface = self.surface(canvas)?;
        if let Some(design) = surface.design_size() {
            return Ok(design);
        }
        let (w, h) = surface.dimensions();
        Ok((w as f32, h as f32))
    }

    pub fn canvas_set_design(&mut self, canvas: u64, w: f32, h: f32) -> Result<(), Value> {
        self.surface(canvas)?.set_design_size(w, h);
        self.canvas.implicit.remove(&canvas);
        Ok(())
    }

    pub fn canvas_set_clip(&mut self, canvas: u64, area: Option<Area>) -> Result<(), Value> {
        // A zero or negative rect clips everything away rather than being an
        // error, as on desktop.
        let area = area.map(|(x, y, w, h)| (x, y, w.max(0.0), h.max(0.0)));
        self.surface(canvas)?.set_clip(area);
        Ok(())
    }

    pub fn canvas_clear(&mut self, canvas: u64, c: Rgba) -> Result<(), Value> {
        self.surface(canvas)?.clear(c.packed());
        Ok(())
    }

    pub fn canvas_fill_rect(&mut self, canvas: u64, a: Area, c: Rgba) -> Result<(), Value> {
        self.surface(canvas)?
            .fill_rect(a.0, a.1, a.2, a.3, c.packed());
        Ok(())
    }

    pub fn canvas_stroke_rect(
        &mut self,
        canvas: u64,
        a: Area,
        c: Rgba,
        width: f32,
    ) -> Result<(), Value> {
        self.surface(canvas)?
            .stroke_rect(a.0, a.1, a.2, a.3, width, c.packed());
        Ok(())
    }

    pub fn canvas_fill_round_rect(
        &mut self,
        canvas: u64,
        a: Area,
        r: Radii,
        c: Rgba,
    ) -> Result<(), Value> {
        self.surface(canvas)?
            .fill_round_rect(a.0, a.1, a.2, a.3, r, c.packed());
        Ok(())
    }

    pub fn canvas_stroke_round_rect(
        &mut self,
        canvas: u64,
        a: Area,
        r: Radii,
        width: f32,
        c: Rgba,
    ) -> Result<(), Value> {
        self.surface(canvas)?
            .stroke_round_rect(a.0, a.1, a.2, a.3, r, width, c.packed());
        Ok(())
    }

    pub fn canvas_drop_shadow(
        &mut self,
        canvas: u64,
        a: Area,
        r: Radii,
        blur: f32,
        c: Rgba,
    ) -> Result<(), Value> {
        self.surface(canvas)?
            .drop_shadow_round_rect(a.0, a.1, a.2, a.3, r, blur, c.packed());
        Ok(())
    }

    pub fn canvas_gradient_stops(
        &mut self,
        canvas: u64,
        a: Area,
        angle: f32,
        stops: &[(f32, Rgba)],
    ) -> Result<(), Value> {
        let packed: Vec<(f32, u32)> = stops.iter().map(|(o, c)| (*o, c.packed())).collect();
        self.surface(canvas)?
            .linear_gradient_stops(a.0, a.1, a.2, a.3, angle, &packed);
        Ok(())
    }

    pub fn canvas_linear_gradient(
        &mut self,
        canvas: u64,
        a: Area,
        top: Rgba,
        bottom: Rgba,
    ) -> Result<(), Value> {
        self.surface(canvas)?
            .linear_gradient_v(a.0, a.1, a.2, a.3, top.packed(), bottom.packed());
        Ok(())
    }

    pub fn canvas_radial_gradient(
        &mut self,
        canvas: u64,
        cx: f32,
        cy: f32,
        radius: f32,
        inner: Rgba,
        outer: Rgba,
    ) -> Result<(), Value> {
        self.surface(canvas)?
            .radial_gradient(cx, cy, radius, inner.packed(), outer.packed());
        Ok(())
    }

    pub fn canvas_fill_circle(
        &mut self,
        canvas: u64,
        cx: f32,
        cy: f32,
        radius: f32,
        c: Rgba,
    ) -> Result<(), Value> {
        self.surface(canvas)?
            .fill_circle(cx, cy, radius, c.packed());
        Ok(())
    }

    pub fn canvas_stroke_circle(
        &mut self,
        canvas: u64,
        cx: f32,
        cy: f32,
        radius: f32,
        width: f32,
        c: Rgba,
    ) -> Result<(), Value> {
        self.surface(canvas)?
            .stroke_circle(cx, cy, radius, width, c.packed());
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn canvas_stroke_arc(
        &mut self,
        canvas: u64,
        cx: f32,
        cy: f32,
        radius: f32,
        start: f32,
        sweep: f32,
        width: f32,
        c: Rgba,
    ) -> Result<(), Value> {
        self.surface(canvas)?
            .stroke_arc(cx, cy, radius, start, sweep, width, c.packed());
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn canvas_text(
        &mut self,
        canvas: u64,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        c: Rgba,
        style: Option<CanvasTextStyle>,
    ) -> Result<(), Value> {
        let surface = self.surface(canvas)?;
        match style {
            Some(style) => surface.text_styled(text, x, y, size, c.packed(), style),
            None => surface.text(text, x, y, size, c.packed()),
        }
        Ok(())
    }

    /// `[width, height, ascent, descent]`, measured by the surface that will
    /// draw it, so the answer comes from the face it actually paints with.
    pub fn canvas_measure(
        &mut self,
        canvas: u64,
        text: &str,
        size: f32,
        style: Option<CanvasTextStyle>,
    ) -> Result<[f32; 4], Value> {
        let surface = self.surface(canvas)?;
        let m = match style {
            Some(style) => surface.measure_text_styled(text, size, style),
            None => surface.measure_text(text, size),
        };
        Ok([m.width, m.height, m.ascent, m.descent])
    }

    pub fn canvas_pixels(
        &mut self,
        canvas: u64,
        a: Area,
        radii: Option<Radii>,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), Value> {
        // The guest is the untrusted side: a buffer shorter than its stated
        // size is refused here, once, before any sampling.
        let image =
            ImagePixels::new(width, height, rgba).map_err(|e| unsupported(e.to_string()))?;
        let surface = self.surface(canvas)?;
        match radii {
            Some(r) => surface.draw_pixels_round(a.0, a.1, a.2, a.3, r, &image),
            None => surface.draw_pixels_owned(a.0, a.1, a.2, a.3, image),
        }
        Ok(())
    }

    /// Text turned by quarters (krate:gfx/canvas-text, K-1341): the same
    /// picture the computer draws (`turned_text_image`), at this canvas's
    /// scale, with its top-left at `(x, y)`.
    #[allow(clippy::too_many_arguments)]
    pub fn canvas_text_turned(
        &mut self,
        canvas: u64,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        c: Rgba,
        style: Option<CanvasTextStyle>,
        quarter_turns: u8,
    ) -> Result<(), Value> {
        let surface = self.surface(canvas)?;
        let (image, w, h) = krate_lowering::canvas_raster::turned_text_image(
            surface.scale(),
            text,
            size,
            (c.0, c.1, c.2, c.3),
            style.unwrap_or_default(),
            quarter_turns,
        )
        .map_err(|e| unsupported(e.to_string()))?;
        surface.draw_pixels_owned(x, y, w, h, image);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn canvas_sprite(
        &mut self,
        canvas: u64,
        cx: f32,
        cy: f32,
        dw: f32,
        dh: f32,
        angle: f32,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<(), Value> {
        let image =
            ImagePixels::new(width, height, rgba).map_err(|e| unsupported(e.to_string()))?;
        self.surface(canvas)?
            .draw_sprite(cx, cy, dw, dh, angle, &image);
        Ok(())
    }

    /// `canvas-images.keep`: a picture the host holds for repeated drawing.
    pub fn canvas_keep(
        &mut self,
        canvas: u64,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Result<u64, Value> {
        if !self.canvas.surfaces.contains_key(&canvas) {
            return Err(invalid_target());
        }
        let image =
            ImagePixels::new(width, height, rgba).map_err(|e| unsupported(e.to_string()))?;
        let held: usize = self.canvas.kept.values().map(|i| i.rgba.len()).sum();
        if held + image.rgba.len() > KEPT_IMAGE_BYTES {
            return Err(unsupported("this app keeps too many pictures at once"));
        }
        let handle = self.canvas.next_image;
        self.canvas.next_image += 1;
        self.canvas.kept.insert((canvas, handle), Arc::new(image));
        Ok(handle)
    }

    pub fn canvas_draw_kept(
        &mut self,
        canvas: u64,
        handle: u64,
        a: Area,
        radii: Option<Radii>,
    ) -> Result<(), Value> {
        let image = self
            .canvas
            .kept
            .get(&(canvas, handle))
            .cloned()
            .ok_or_else(invalid_target)?;
        let surface = self.surface(canvas)?;
        match radii {
            Some(r) => surface.draw_pixels_round(a.0, a.1, a.2, a.3, r, &image),
            None => surface.draw_pixels(a.0, a.1, a.2, a.3, &image),
        }
        Ok(())
    }

    pub fn canvas_forget(&mut self, canvas: u64, handle: u64) {
        self.canvas.kept.remove(&(canvas, handle));
    }

    /// `canvas2d.present`: the frame goes on screen. An app that never asked
    /// its canvas's size, in a tab smaller than its window asked for, draws
    /// in the size it asked for and the host fits it (as on desktop).
    pub fn canvas_present(&mut self, canvas: u64) -> Result<(), Value> {
        self.fit_unasked(canvas);
        let (window, widget) = {
            let (window, widget, _) = self
                .canvas
                .surfaces
                .get(&canvas)
                .ok_or_else(invalid_target)?;
            (*window, *widget)
        };
        // Let go of the picture shown last so the surface rewrites only
        // what changed, in place, instead of a new copy of the whole canvas
        // each frame (K-1373). The tab's painter still paints all of it.
        self.forget_image(window, widget);
        let (_, _, surface) = self
            .canvas
            .surfaces
            .get_mut(&canvas)
            .ok_or_else(invalid_target)?;
        let (image, _) = surface
            .present_image()
            .map_err(|e| json!({ "tag": "platform", "val": e.to_string() }))?;
        self.publish_image(window, widget, image);
        Ok(())
    }

    fn fit_unasked(&mut self, canvas: u64) {
        if self.canvas.asked.contains(&canvas) {
            return;
        }
        let implicit = self.canvas.implicit.contains(&canvas);
        let Some((window, _, surface)) = self.canvas.surfaces.get(&canvas) else {
            return;
        };
        let window = *window;
        if surface.design_size().is_some() && !implicit {
            return;
        }
        let Some(requested) = self.requested_size(window) else {
            return;
        };
        if self.refit(canvas).is_err() {
            return;
        }
        let Ok(surface) = self.surface(canvas) else {
            return;
        };
        let (w, h) = surface.dimensions();
        let differs = (w as f32 - requested.0).abs() > 1.0 || (h as f32 - requested.1).abs() > 1.0;
        if differs && !implicit {
            surface.set_design_size(requested.0, requested.1);
            self.canvas.implicit.insert(canvas);
        } else if !differs && implicit {
            surface.set_design_size(0.0, 0.0);
            self.canvas.implicit.remove(&canvas);
        }
    }

    /// A pointer or wheel position in the window, in the design space of the
    /// window's first canvas when it has one -- what the app draws against.
    pub(crate) fn to_design(&self, x: f32, y: f32) -> Option<(f32, f32, f32)> {
        let (_, _, surface) = self.canvas.surfaces.values().next()?;
        let (dw, dh) = surface.design_size()?;
        let (cw, ch) = surface.dimensions();
        let k = (cw as f32 / dw).min(ch as f32 / dh);
        if k <= 0.0 {
            return None;
        }
        let (ox, oy) = ((cw as f32 - dw * k) / 2.0, (ch as f32 - dh * k) / 2.0);
        Some(((x - ox) / k, (y - oy) / k, k))
    }
}

/// Every canvas one app holds.
#[derive(Default)]
pub struct Canvases {
    pub(crate) surfaces: std::collections::BTreeMap<u64, (WindowId, WidgetId, CanvasSurface)>,
    pub(crate) next: u64,
    pub(crate) asked: std::collections::BTreeSet<u64>,
    pub(crate) implicit: std::collections::BTreeSet<u64>,
    pub(crate) kept: std::collections::BTreeMap<(u64, u64), Arc<ImagePixels>>,
    pub(crate) next_image: u64,
}

impl Canvases {
    pub fn new() -> Self {
        Self {
            next: 1,
            next_image: 1,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas_app(ui: &mut WebUi, asked: (u32, u32), room: (u32, u32)) -> (u64, u64) {
        let win = ui.create_window("Canvas", room.0, room.1, asked).unwrap();
        let node = |id: u64, parent: Option<u64>, kind: &str| {
            let mut v = json!({ "id": id.to_string(), "kind": kind, "style": { "grow": 1.0, "padding": 0.0 } });
            if let Some(p) = parent {
                v["parent"] = Value::String(p.to_string());
            }
            v.to_string()
        };
        ui.set_root(win, &node(1, None, "stack")).unwrap();
        ui.upsert(win, &node(2, Some(1), "canvas")).unwrap();
        while ui.next_event().is_some() {}
        let canvas = ui.canvas_bind(win, 2).unwrap();
        (win, canvas)
    }

    /// What an app draws reaches the window's frame on present.
    #[test]
    fn a_filled_canvas_reaches_the_frame() {
        let mut ui = WebUi::new(1.0, false);
        let (win, canvas) = canvas_app(&mut ui, (100, 80), (100, 80));
        assert_eq!(ui.canvas_size(canvas).unwrap(), (100.0, 80.0));
        ui.canvas_fill_rect(canvas, (0.0, 0.0, 100.0, 80.0), Rgba(1.0, 0.0, 0.0, 1.0))
            .unwrap();
        ui.canvas_present(canvas).unwrap();
        assert!(ui.render(win));
        let frame = ui.frame(win).unwrap();
        let mid = (40 * 100 + 50) * 4;
        assert_eq!(&frame[mid..mid + 4], &[255, 0, 0, 255]);
        assert!(ui.canvas_bind(win, 1).is_err(), "a stack is not a canvas");
    }

    /// An app that never asked its size, in a tab smaller than it asked for,
    /// draws in the size it asked for and hears the pointer in that size.
    #[test]
    fn an_unasked_canvas_in_a_smaller_tab_keeps_its_coordinates() {
        let mut ui = WebUi::new(1.0, false);
        let (win, canvas) = canvas_app(&mut ui, (400, 300), (200, 150));
        ui.canvas_present(canvas).unwrap();
        ui.pointer(win, 100.0, 75.0, 0, true, 0);
        let event: Value = serde_json::from_str(&ui.next_event().unwrap()).unwrap();
        assert_eq!(event["tag"], "pointer");
        assert!(
            (event["val"]["x"].as_f64().unwrap() - 200.0).abs() < 0.5,
            "{event}"
        );
        assert!(
            (event["val"]["y"].as_f64().unwrap() - 150.0).abs() < 0.5,
            "{event}"
        );
        // An app that asks is responsive: the host's design goes.
        assert_eq!(ui.canvas_size(canvas).unwrap(), (200.0, 150.0));
    }

    /// Text measures with the bundled font the surface will draw with.
    #[test]
    fn canvas_text_measures_with_the_bundled_font() {
        assert!(crate::use_bundled_font());
        let mut ui = WebUi::new(1.0, false);
        let (_, canvas) = canvas_app(&mut ui, (100, 80), (100, 80));
        let m = ui.canvas_measure(canvas, "Hello", 16.0, None).unwrap();
        assert!(m[0] > 25.0 && m[0] < 70.0, "{m:?}");
        assert!(ui.canvas_measure(99, "x", 16.0, None).is_err());
    }
}
