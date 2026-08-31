//! The browser adapter: a Krate app painting into a canvas in a tab.
//!
//! # Why this can work at all
//!
//! Two properties of the existing runtime, neither obvious until measured:
//!
//! 1. **The painter is a CPU framebuffer.** `paint_placements` writes
//!    `0xAARRGGBB` into a row-major `&mut [u32]`. That is byte-compatible
//!    with what `ImageData` wants, so a frame reaches the screen through
//!    `putImageData` with no WebGPU, no WebGL, and no shader path.
//! 2. **The JIT problem is already solved.** wasmtime selects pulley, its
//!    portable interpreter, automatically on wasm32 -- the same answer iOS
//!    needed because it forbids executable pages. Measured on the rate card,
//!    the interpreter costs nothing worth naming: the guest is a few
//!    kilobytes of decisions and the painting is host code either way.
//!
//! # What this file is today
//!
//! The spike, and only the spike: proof that the shared painter's pixels
//! reach a canvas. It deliberately does NOT implement `UiAdapter` yet,
//! because the honest blocker is the event loop, not the drawing -- the
//! guest's `run()` never returns, and the host blocks inside it on
//! `thread::sleep`, which traps on a browser's main thread. That needs a
//! Web Worker and `Atomics.wait`, and it is worth settling on top of a
//! proven pixel path rather than underneath an unproven one.
//!
//! Everything here is the real shared code. Nothing about the frame is
//! mocked: same painter, same placements, same layout crate the native
//! hosts use, so this cannot drift from what a Mac draws.

use krate_adapter_common::painter::{self, PaintInteraction};
use krate_adapter_common::ui::WidgetPlacement;

/// A frame of pixels, painted by the shared painter.
///
/// Kept separate from anything browser-specific so the same call can be
/// exercised by a native test -- which is how the spike is verified
/// without a browser in the loop.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// 0xAARRGGBB, row-major from the top.
    pub buffer: Vec<u32>,
}

impl Frame {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            buffer: vec![painter::COLOR_BACKGROUND; (width as usize) * (height as usize)],
        }
    }

    /// Paint one frame of widgets through the shared CPU painter.
    pub fn paint(&mut self, placements: &[WidgetPlacement], scale: f32) {
        painter::paint_placements(
            &mut self.buffer,
            self.width,
            self.height,
            scale,
            placements,
            PaintInteraction::default(),
        );
    }

    /// The frame as RGBA bytes, which is what `ImageData` takes.
    ///
    /// The painter stores `0xAARRGGBB` in a `u32`; a canvas wants
    /// `[R, G, B, A]` bytes in memory order. This is the one conversion
    /// between the two worlds, and it is a byte shuffle, not a re-render.
    pub fn to_rgba(&self) -> Vec<u8> {
        let mut rgba = vec![0u8; self.buffer.len() * 4];
        for (chunk, word) in rgba.chunks_exact_mut(4).zip(self.buffer.iter()) {
            chunk[0] = (word >> 16) as u8;
            chunk[1] = (word >> 8) as u8;
            chunk[2] = *word as u8;
            chunk[3] = (word >> 24) as u8;
        }
        rgba
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::Frame;
    use krate_adapter_common::ui::{WidgetId, WidgetKind, WidgetPlacement};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::Clamped;

    /// Paint a demonstration frame into the canvas with the given id.
    ///
    /// This is the spike's whole public surface: it proves that pixels
    /// produced by the shared painter land on a canvas. Real placements
    /// will come from the runtime's layout pass; here they are built by
    /// hand so the path can be tested before the runtime compiles for
    /// this target.
    #[wasm_bindgen]
    pub fn paint_demo(canvas_id: &str, width: u32, height: u32) -> Result<(), JsValue> {
        let window = web_sys::window().ok_or("no window")?;
        let document = window.document().ok_or("no document")?;
        let canvas = document
            .get_element_by_id(canvas_id)
            .ok_or("no canvas with that id")?
            .dyn_into::<web_sys::HtmlCanvasElement>()?;
        canvas.set_width(width);
        canvas.set_height(height);
        let ctx = canvas
            .get_context("2d")?
            .ok_or("no 2d context")?
            .dyn_into::<web_sys::CanvasRenderingContext2d>()?;

        let mut frame = Frame::new(width, height);
        frame.paint(&demo_placements(width as f32, height as f32), 1.0);

        let mut rgba = frame.to_rgba();
        let data = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
            Clamped(&mut rgba),
            width,
            height,
        )?;
        ctx.put_image_data(&data, 0.0, 0.0)?;
        Ok(())
    }

    /// A handful of widgets, laid out by hand, to exercise the painter.
    fn demo_placements(width: f32, _height: f32) -> Vec<WidgetPlacement> {
        let pad = 24.0;
        let w = width - pad * 2.0;
        vec![
            placement(1, WidgetKind::Text, "Krate, running in a browser tab", pad, pad, w, 28.0),
            placement(2, WidgetKind::Text, "Painted by the same shared painter a Mac uses.", pad, pad + 36.0, w, 22.0),
            placement(3, WidgetKind::Button, "A button", pad, pad + 76.0, 160.0, 36.0),
            placement(4, WidgetKind::TextField, "A text field", pad, pad + 124.0, w, 34.0),
            placement(5, WidgetKind::Progress, "", pad, pad + 174.0, w, 14.0),
        ]
    }

    fn placement(
        id: u32,
        kind: WidgetKind,
        label: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> WidgetPlacement {
        WidgetPlacement {
            widget: WidgetId::new(id.into()).expect("demo ids are non-zero"),
            kind,
            label: if label.is_empty() { None } else { Some(label.to_string()) },
            checked: None,
            value: if matches!(kind, WidgetKind::Progress) { Some(0.6) } else { None },
            selection: None,
            text_cursor: None,
            clip: None,
            x,
            y,
            width,
            height,
            clickable: matches!(kind, WidgetKind::Button),
            role: None,
            pixels: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use krate_adapter_common::ui::{WidgetId, WidgetKind};

    fn text(id: u32, label: &str, x: f32, y: f32, w: f32, h: f32) -> WidgetPlacement {
        WidgetPlacement {
            widget: WidgetId::new(id.into()).expect("demo ids are non-zero"),
            kind: WidgetKind::Button,
            label: Some(label.to_string()),
            checked: None,
            value: None,
            selection: None,
            text_cursor: None,
            clip: None,
            x,
            y,
            width: w,
            height: h,
            clickable: true,
            role: None,
            pixels: None,
        }
    }

    /// The spike's real assertion: the shared painter puts pixels in the
    /// buffer, and they survive the conversion a canvas needs.
    ///
    /// Run natively, so the pixel path is proven without a browser. If
    /// this passes and `putImageData` shows nothing, the bug is in the
    /// glue, not in the rendering -- which is exactly the split worth
    /// having before the adapter work starts.
    #[test]
    fn the_shared_painter_fills_a_frame_a_canvas_could_show() {
        let mut frame = Frame::new(200, 80);
        let blank = frame.buffer.clone();
        frame.paint(&[text(1, "Hello", 10.0, 10.0, 120.0, 30.0)], 1.0);

        assert_ne!(frame.buffer, blank, "painting should change the frame");

        let rgba = frame.to_rgba();
        assert_eq!(rgba.len(), 200 * 80 * 4, "RGBA is four bytes a pixel");
        assert!(
            rgba.chunks_exact(4).all(|px| px[3] == 0xFF),
            "every pixel must be opaque, or the canvas shows the page through it",
        );

        // The button is drawn somewhere inside its own rect, and the frame
        // outside it is untouched background.
        let at = |x: usize, y: usize| frame.buffer[y * 200 + x];
        assert_ne!(
            at(60, 25),
            painter::COLOR_BACKGROUND,
            "the button's middle should be painted",
        );
        assert_eq!(
            at(190, 75),
            painter::COLOR_BACKGROUND,
            "the far corner should be untouched",
        );
    }

    /// The byte order the canvas expects, pinned. A red pixel must come
    /// out as R=255, and not as B=255 -- the classic way this path fails
    /// silently, with everything on screen looking almost right.
    #[test]
    fn pixels_convert_to_canvas_byte_order() {
        let mut frame = Frame::new(1, 1);
        frame.buffer[0] = 0xFF_FF_00_00; // opaque red
        assert_eq!(frame.to_rgba(), vec![0xFF, 0x00, 0x00, 0xFF]);
    }
}
