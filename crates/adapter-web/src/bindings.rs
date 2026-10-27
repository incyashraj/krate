//! The JavaScript face of the Player module (wasm32 only).
//!
//! Every fallible call answers a JSON string: `{"ok": ...}` or
//! `{"err": <the WIT error value>}`, so the worker's host turns it into the
//! jco shape without a second copy of any rule. u64s cross as strings,
//! because a JSON number cannot carry all of them.

use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

use crate::web_canvas::{text_style, Rgba};
use crate::web_kv::WebKv;
use crate::web_policy::{consent_rows, WebGrants};
use crate::web_ui::WebUi;

fn answer<T: Into<Value>>(result: Result<T, Value>) -> String {
    match result {
        Ok(value) => json!({ "ok": value.into() }).to_string(),
        Err(err) => json!({ "err": err }).to_string(),
    }
}

fn id(text: &str) -> u64 {
    text.parse().unwrap_or(0)
}

/// Runs once when the module loads: a panic reaches the console in words,
/// and the text engine gets the bundled font.
#[wasm_bindgen(start)]
pub fn start() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&format!("krate player panic: {info}").into());
    }));
    crate::use_bundled_font();
}

/// The `krate:ui` host for one app.
#[wasm_bindgen]
pub struct Ui {
    inner: WebUi,
}

#[wasm_bindgen]
impl Ui {
    #[wasm_bindgen(constructor)]
    pub fn new(scale: f32, dark: bool) -> Ui {
        Ui {
            inner: WebUi::new(scale, dark),
        }
    }

    pub fn create_window(
        &mut self,
        title: &str,
        width: u32,
        height: u32,
        asked_width: u32,
        asked_height: u32,
    ) -> String {
        answer(
            self.inner
                .create_window(title, width, height, (asked_width, asked_height))
                .map(|w| Value::String(w.to_string())),
        )
    }

    pub fn show_window(&mut self, window: &str) -> String {
        answer(self.inner.show_window(id(window)).map(|_| Value::Null))
    }

    pub fn close_window(&mut self, window: &str) -> String {
        answer(self.inner.close_window(id(window)).map(|_| Value::Null))
    }

    pub fn set_title(&mut self, window: &str, title: &str) -> String {
        answer(self.inner.set_title(id(window), title).map(|_| Value::Null))
    }

    pub fn set_size(&mut self, window: &str, width: u32, height: u32, host: bool) -> String {
        answer(
            self.inner
                .set_size(id(window), width, height, host)
                .map(|_| Value::Null),
        )
    }

    pub fn request_redraw(&mut self, window: &str) -> String {
        answer(self.inner.request_redraw(id(window)).map(|_| Value::Null))
    }

    pub fn set_root(&mut self, window: &str, node: &str) -> String {
        answer(self.inner.set_root(id(window), node).map(|_| Value::Null))
    }

    pub fn upsert(&mut self, window: &str, node: &str) -> String {
        answer(self.inner.upsert(id(window), node).map(|_| Value::Null))
    }

    pub fn remove(&mut self, window: &str, widget: &str) -> String {
        answer(
            self.inner
                .remove(id(window), id(widget))
                .map(|_| Value::Null),
        )
    }

    pub fn focus(&mut self, window: &str, widget: &str) -> String {
        answer(
            self.inner
                .focus(id(window), id(widget))
                .map(|_| Value::Null),
        )
    }

    pub fn set_image(
        &mut self,
        window: &str,
        widget: &str,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> String {
        answer(
            self.inner
                .set_image(id(window), id(widget), width, height, rgba)
                .map(|_| Value::Null),
        )
    }

    pub fn clear_image(&mut self, window: &str, widget: &str) -> String {
        answer(
            self.inner
                .clear_image(id(window), id(widget))
                .map(|_| Value::Null),
        )
    }

    pub fn pointer(&mut self, window: &str, x: f32, y: f32, button: u8, pressed: bool, mods: u8) {
        self.inner.pointer(id(window), x, y, button, pressed, mods);
    }

    pub fn pointer_moved(&mut self, window: &str, x: f32, y: f32) {
        self.inner.pointer_moved(id(window), x, y);
    }

    pub fn pointer_left(&mut self, window: &str) {
        self.inner.pointer_left(id(window));
    }

    pub fn wheel(&mut self, window: &str, x: f32, y: f32, dx: f32, dy: f32, mods: u8) {
        self.inner.wheel(id(window), x, y, dx, dy, mods);
    }

    /// A finger on a touch screen: start, move, end or cancel.
    pub fn touch(&mut self, window: &str, phase: &str, finger: u32, x: f32, y: f32) {
        self.inner.touch(id(window), phase, u64::from(finger), x, y);
    }

    /// The text fields' rectangles on screen, four numbers each.
    pub fn text_rects(&self, window: &str) -> Vec<f32> {
        self.inner.text_rects(id(window))
    }

    pub fn key(&mut self, window: &str, key: &str, pressed: bool, mods: u8, text: Option<String>) {
        self.inner
            .key(id(window), key, pressed, mods, text.as_deref());
    }

    pub fn text(&mut self, window: &str, text: &str) {
        self.inner.text(id(window), text);
    }

    pub fn blurred(&mut self) {
        self.inner.blurred();
    }

    pub fn close_requested(&mut self, window: &str) {
        self.inner.close_requested(id(window));
    }

    pub fn theme(&mut self, dark: bool) {
        self.inner.theme(dark);
    }

    pub fn dark(&self) -> bool {
        self.inner.dark()
    }

    pub fn key_held(&self, name: &str) -> bool {
        self.inner.key_held(name)
    }

    pub fn button_held(&self, button: u8) -> bool {
        self.inner.button_held(button)
    }

    /// `[x, y]` or nothing.
    pub fn position(&self, window: &str) -> Option<Vec<f32>> {
        self.inner.position(id(window)).map(|(x, y)| vec![x, y])
    }

    pub fn focused(&self, window: &str) -> Option<String> {
        self.inner.focused(id(window)).map(|w| w.to_string())
    }

    pub fn text_focused(&self, window: &str) -> bool {
        self.inner.text_focused(id(window))
    }

    pub fn next_event(&mut self) -> Option<String> {
        self.inner.next_event()
    }

    pub fn render(&mut self, window: &str) -> bool {
        self.inner.render(id(window))
    }

    /// Where the window's RGBA frame sits in this module's memory.
    pub fn frame_ptr(&self, window: &str) -> usize {
        self.inner
            .frame(id(window))
            .map(|f| f.as_ptr() as usize)
            .unwrap_or(0)
    }

    pub fn frame_len(&self, window: &str) -> usize {
        self.inner.frame(id(window)).map(|f| f.len()).unwrap_or(0)
    }

    /// `[physical width, physical height, logical width, logical height]`.
    pub fn frame_size(&self, window: &str) -> Vec<u32> {
        let w = id(window);
        match (self.inner.frame_size(w), self.inner.logical_size(w)) {
            (Some((pw, ph)), Some((lw, lh))) => vec![pw, ph, lw, lh],
            _ => Vec::new(),
        }
    }

    pub fn scale(&self) -> f32 {
        self.inner.scale()
    }

    /// What is on screen, as JSON (see `WebUi::on_screen`).
    pub fn on_screen(&self, window: &str) -> String {
        self.inner.on_screen(id(window)).to_string()
    }

    /// `[x, y, width, height]` in logical pixels, or empty.
    pub fn widget_rect(&self, window: &str, widget: &str) -> Vec<f32> {
        self.inner
            .widget_rect(id(window), id(widget))
            .map(|(x, y, w, h)| vec![x, y, w, h])
            .unwrap_or_default()
    }
}

/// The consent screen's rows for the judgement's `manifest.capabilities`.
#[wasm_bindgen]
pub fn consent(capabilities_json: &str) -> String {
    consent_rows(capabilities_json).to_string()
}

/// The session's grants.
#[wasm_bindgen]
pub struct Grants {
    inner: WebGrants,
}

#[wasm_bindgen]
impl Grants {
    /// `granted` is a JSON list of capability strings.
    #[wasm_bindgen(constructor)]
    pub fn new(granted: &str) -> Grants {
        let list: Vec<String> = serde_json::from_str(granted).unwrap_or_default();
        Grants {
            inner: WebGrants::new(&list),
        }
    }

    pub fn allows(&self, cap: &str) -> bool {
        self.inner.allows(cap)
    }

    /// `headers` is a JSON list of `{name, value}`. Answers
    /// `{"ok":{"host","port"}}` or `{"err": <net-error>}`.
    pub fn check_http(&self, url: &str, headers: &str, body_len: usize) -> String {
        #[derive(serde::Deserialize)]
        struct Header {
            name: String,
            value: String,
        }
        let headers: Vec<Header> = match serde_json::from_str(headers) {
            Ok(list) => list,
            Err(_) => {
                return answer::<Value>(Err(
                    json!({ "tag": "protocol", "val": "invalid HTTP header" }),
                ))
            }
        };
        let pairs: Vec<(String, String)> = headers.into_iter().map(|h| (h.name, h.value)).collect();
        answer(
            self.inner
                .check_http(url, &pairs, body_len)
                .map(|(host, port)| json!({ "host": host, "port": port })),
        )
    }

    pub fn max_response_bytes() -> usize {
        crate::web_policy::MAX_RESPONSE_BYTES
    }
}

/// `krate:store/kv` for one app. Answers like the other fallible calls;
/// a value crosses as bytes.
#[wasm_bindgen]
pub struct Kv {
    inner: WebKv,
}

#[wasm_bindgen]
impl Kv {
    #[wasm_bindgen(constructor)]
    pub fn new(granted: bool) -> Kv {
        Kv {
            inner: WebKv::new(granted),
        }
    }

    pub fn load(&mut self, key: &str, value: Vec<u8>) {
        self.inner.load(key, value);
    }

    /// `{"ok": null}` when absent; a present value is fetched with
    /// `value` after `{"ok": true}`.
    pub fn get(&self, key: &str) -> String {
        answer(self.inner.get(key).map(|v| Value::Bool(v.is_some())))
    }

    pub fn value(&self, key: &str) -> Vec<u8> {
        self.inner.get(key).ok().flatten().unwrap_or_default()
    }

    pub fn check_set(&self, key: &str, len: usize) -> String {
        answer(self.inner.check_set(key, len).map(|_| Value::Null))
    }

    pub fn commit_set(&mut self, key: &str, value: Vec<u8>) {
        self.inner.commit_set(key, value);
    }

    pub fn check_delete(&self, key: &str) -> String {
        answer(self.inner.check_delete(key).map(Value::Bool))
    }

    pub fn commit_delete(&mut self, key: &str) {
        self.inner.commit_delete(key);
    }

    pub fn keys(&self) -> String {
        answer(self.inner.keys().map(|k| json!(k)))
    }

    pub fn check_clear(&self) -> String {
        answer(self.inner.check_clear().map(|_| Value::Null))
    }

    pub fn commit_clear(&mut self) {
        self.inner.commit_clear();
    }
}

/// `krate:gfx/canvas2d` and `canvas-images`, on the same host. Every call
/// answers nothing on success and the `gfx-error` JSON otherwise, except
/// those that return a value (`answer`'s shape). Colours are four floats;
/// rectangles x, y, width, height; radii top-left, top-right,
/// bottom-right, bottom-left.
#[wasm_bindgen]
impl Ui {
    pub fn c_bind(&mut self, window: &str, widget: &str) -> String {
        answer(
            self.inner
                .canvas_bind(id(window), id(widget))
                .map(|c| Value::String(c.to_string())),
        )
    }

    /// `{"ok":[width, height]}`.
    pub fn c_size(&mut self, canvas: u64) -> String {
        answer(self.inner.canvas_size(canvas).map(|(w, h)| json!([w, h])))
    }

    pub fn c_design(&mut self, canvas: u64, w: f32, h: f32) -> Option<String> {
        fail(self.inner.canvas_set_design(canvas, w, h))
    }

    pub fn c_clip(&mut self, canvas: u64, x: f32, y: f32, w: f32, h: f32) -> Option<String> {
        fail(self.inner.canvas_set_clip(canvas, Some((x, y, w, h))))
    }

    pub fn c_unclip(&mut self, canvas: u64) -> Option<String> {
        fail(self.inner.canvas_set_clip(canvas, None))
    }

    pub fn c_clear(&mut self, canvas: u64, r: f32, g: f32, b: f32, a: f32) -> Option<String> {
        fail(self.inner.canvas_clear(canvas, Rgba(r, g, b, a)))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn c_fill_rect(
        &mut self,
        canvas: u64,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        g: f32,
        b: f32,
        a: f32,
    ) -> Option<String> {
        fail(
            self.inner
                .canvas_fill_rect(canvas, (x, y, w, h), Rgba(r, g, b, a)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn c_stroke_rect(
        &mut self,
        canvas: u64,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        g: f32,
        b: f32,
        a: f32,
        width: f32,
    ) -> Option<String> {
        fail(
            self.inner
                .canvas_stroke_rect(canvas, (x, y, w, h), Rgba(r, g, b, a), width),
        )
    }

    /// `geometry` is [x, y, w, h, tl, tr, br, bl]; `color` [r, g, b, a].
    pub fn c_fill_round_rect(
        &mut self,
        canvas: u64,
        geometry: Vec<f32>,
        color: Vec<f32>,
    ) -> Option<String> {
        let (a, r) = area_radii(&geometry);
        fail(
            self.inner
                .canvas_fill_round_rect(canvas, a, r, rgba(&color)),
        )
    }

    pub fn c_stroke_round_rect(
        &mut self,
        canvas: u64,
        geometry: Vec<f32>,
        width: f32,
        color: Vec<f32>,
    ) -> Option<String> {
        let (a, r) = area_radii(&geometry);
        fail(
            self.inner
                .canvas_stroke_round_rect(canvas, a, r, width, rgba(&color)),
        )
    }

    pub fn c_drop_shadow(
        &mut self,
        canvas: u64,
        geometry: Vec<f32>,
        blur: f32,
        color: Vec<f32>,
    ) -> Option<String> {
        let (a, r) = area_radii(&geometry);
        fail(
            self.inner
                .canvas_drop_shadow(canvas, a, r, blur, rgba(&color)),
        )
    }

    /// `stops` is [offset, r, g, b, a, offset, r, g, b, a, ...].
    pub fn c_gradient_stops(
        &mut self,
        canvas: u64,
        area: Vec<f32>,
        angle: f32,
        stops: Vec<f32>,
    ) -> Option<String> {
        let list: Vec<(f32, Rgba)> = stops
            .chunks_exact(5)
            .map(|s| (s[0], Rgba(s[1], s[2], s[3], s[4])))
            .collect();
        fail(
            self.inner
                .canvas_gradient_stops(canvas, area4(&area), angle, &list),
        )
    }

    pub fn c_linear_gradient(
        &mut self,
        canvas: u64,
        area: Vec<f32>,
        top: Vec<f32>,
        bottom: Vec<f32>,
    ) -> Option<String> {
        fail(
            self.inner
                .canvas_linear_gradient(canvas, area4(&area), rgba(&top), rgba(&bottom)),
        )
    }

    pub fn c_radial_gradient(
        &mut self,
        canvas: u64,
        cx: f32,
        cy: f32,
        radius: f32,
        inner: Vec<f32>,
        outer: Vec<f32>,
    ) -> Option<String> {
        fail(
            self.inner
                .canvas_radial_gradient(canvas, cx, cy, radius, rgba(&inner), rgba(&outer)),
        )
    }

    pub fn c_fill_circle(
        &mut self,
        canvas: u64,
        cx: f32,
        cy: f32,
        radius: f32,
        color: Vec<f32>,
    ) -> Option<String> {
        fail(
            self.inner
                .canvas_fill_circle(canvas, cx, cy, radius, rgba(&color)),
        )
    }

    pub fn c_stroke_circle(
        &mut self,
        canvas: u64,
        cx: f32,
        cy: f32,
        radius: f32,
        width: f32,
        color: Vec<f32>,
    ) -> Option<String> {
        fail(
            self.inner
                .canvas_stroke_circle(canvas, cx, cy, radius, width, rgba(&color)),
        )
    }

    /// `arc` is [cx, cy, radius, start, sweep, width].
    pub fn c_stroke_arc(&mut self, canvas: u64, arc: Vec<f32>, color: Vec<f32>) -> Option<String> {
        let v = |i: usize| arc.get(i).copied().unwrap_or(0.0);
        fail(
            self.inner
                .canvas_stroke_arc(canvas, v(0), v(1), v(2), v(3), v(4), v(5), rgba(&color)),
        )
    }

    /// `style` is [weight, italic (0/1), letter-spacing, family (0 sans,
    /// 1 serif, 2 mono)], or empty for the plain call.
    #[allow(clippy::too_many_arguments)]
    pub fn c_text(
        &mut self,
        canvas: u64,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Vec<f32>,
        style: Vec<f32>,
    ) -> Option<String> {
        fail(
            self.inner
                .canvas_text(canvas, text, x, y, size, rgba(&color), style_of(&style)),
        )
    }

    /// `{"ok":[width, height, ascent, descent]}`.
    pub fn c_measure(&mut self, canvas: u64, text: &str, size: f32, style: Vec<f32>) -> String {
        answer(
            self.inner
                .canvas_measure(canvas, text, size, style_of(&style))
                .map(|m| json!(m)),
        )
    }

    /// `geometry` is [x, y, w, h] or [x, y, w, h, tl, tr, br, bl].
    pub fn c_pixels(
        &mut self,
        canvas: u64,
        geometry: Vec<f32>,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Option<String> {
        let radii = (geometry.len() >= 8).then(|| area_radii(&geometry).1);
        fail(
            self.inner
                .canvas_pixels(canvas, area4(&geometry), radii, width, height, rgba),
        )
    }

    /// `place` is [cx, cy, dst width, dst height, angle].
    pub fn c_sprite(
        &mut self,
        canvas: u64,
        place: Vec<f32>,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Option<String> {
        let v = |i: usize| place.get(i).copied().unwrap_or(0.0);
        fail(
            self.inner
                .canvas_sprite(canvas, v(0), v(1), v(2), v(3), v(4), width, height, rgba),
        )
    }

    pub fn c_keep(&mut self, canvas: u64, width: u32, height: u32, rgba: Vec<u8>) -> String {
        answer(
            self.inner
                .canvas_keep(canvas, width, height, rgba)
                .map(|h| Value::String(h.to_string())),
        )
    }

    pub fn c_draw_kept(&mut self, canvas: u64, image: u64, geometry: Vec<f32>) -> Option<String> {
        let radii = (geometry.len() >= 8).then(|| area_radii(&geometry).1);
        fail(
            self.inner
                .canvas_draw_kept(canvas, image, area4(&geometry), radii),
        )
    }

    pub fn c_forget(&mut self, canvas: u64, image: u64) {
        self.inner.canvas_forget(canvas, image);
    }

    pub fn c_present(&mut self, canvas: u64) -> Option<String> {
        fail(self.inner.canvas_present(canvas))
    }
}

fn fail(result: Result<(), Value>) -> Option<String> {
    result.err().map(|err| err.to_string())
}

fn rgba(c: &[f32]) -> Rgba {
    let v = |i: usize| c.get(i).copied().unwrap_or(0.0);
    Rgba(v(0), v(1), v(2), v(3))
}

fn area4(g: &[f32]) -> (f32, f32, f32, f32) {
    let v = |i: usize| g.get(i).copied().unwrap_or(0.0);
    (v(0), v(1), v(2), v(3))
}

fn area_radii(g: &[f32]) -> (crate::web_canvas::Area, crate::web_canvas::Radii) {
    let v = |i: usize| g.get(i).copied().unwrap_or(0.0);
    ((v(0), v(1), v(2), v(3)), (v(4), v(5), v(6), v(7)))
}

fn style_of(s: &[f32]) -> Option<krate_adapter_common::vector_text::CanvasTextStyle> {
    if s.len() < 4 {
        return None;
    }
    Some(text_style(
        s[0].clamp(1.0, 1000.0) as u16,
        s[1] != 0.0,
        s[2],
        s[3] as u8,
    ))
}
