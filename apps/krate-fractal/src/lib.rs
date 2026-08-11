//! Krate fractal — the limitation probe for computed images.
//!
//! The wall it tests: an app that builds a picture pixel by pixel and hands it
//! to an image widget. This is the `ui.image` set-pixels path -- distinct from
//! the canvas draw path -- and the way a photo viewer, a chart image, or a
//! generated texture reaches the screen. If a guest cannot allocate and fill a
//! few hundred thousand pixels and get them drawn, a whole class of app is out.
//!
//! It renders the Mandelbrot set: for each pixel, iterate z = z^2 + c and color
//! by how fast it escapes. The math is plain f64 with no panic paths, the
//! buffer is one `alloc` Vec, and the component imports only `krate:*`.
//!
//! This app is `#![no_std]`, and that is the point. `ui.image` set-pixels sends
//! the pixel buffer as a growable `list<u8>`, whose lowering reaches the
//! allocation path. In a std-linked guest that path routes through std's
//! allocation-error handler and its panic runtime, dragging the whole `wasi:*`
//! import set into an otherwise pure component -- so the app fails to
//! instantiate against the Krate linker. Building `#![no_std]` lets the SDK own
//! the allocator and a trapping panic handler, so the same path traps instead
//! of leaking. This is the shape every real Krate app uses.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

// Linked purely for its `no_std` runtime lang items -- the global allocator,
// the panic handler, and the memory intrinsics a wasm guest needs when std is
// not linked. Not called directly; the underscore keeps the import.
extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::vec::Vec;

use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, image, tree, types, window};

const ROOT_ID: u64 = 1;
const IMAGE_ID: u64 = 2;

const WIDTH: u32 = 480;
const HEIGHT: u32 = 360;
/// Escape-iteration ceiling: higher is more detail and more work.
const MAX_ITER: u32 = 80;

const QUICK_ROUNDS: u32 = 20;
const MAX_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH,
            height: HEIGHT,
        };
        let Ok(win) = window::create("Mandelbrot", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &image_node()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }

        // Build the picture: width * height RGBA pixels, computed here.
        let pixels = render_mandelbrot();
        let image_data = image::ImagePixels {
            width: WIDTH,
            height: HEIGHT,
            rgba: pixels,
        };
        if image::set_pixels(win, IMAGE_ID, &image_data).is_err() {
            let _ = window::close(win);
            return 33;
        }

        let out = stdio::stdout();
        let _ = out.write(b"pixels:");
        let _ = out.write(u32_slice(WIDTH * HEIGHT, &mut [0u8; 12]));
        let _ = out.write(b"\n");

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");
        // A real session ends when the person closes the window, never
        // on a round count: 600 rounds x 50 ms quietly shut the window
        // after thirty seconds of use (K-092). `quick` keeps its bound
        // so a headless check can never hang.
        let rounds = if quick { QUICK_ROUNDS } else { u32::MAX };
        for _ in 0..rounds {
            match events::wait(Some(ROUND_MILLIS)) {
                Some(types::Event::CloseRequested(id)) if id == win => break,
                _ => {}
            }
        }

        let _ = window::close(win);
        0
    }
}

/// Render the Mandelbrot set into a fresh RGBA buffer.
fn render_mandelbrot() -> Vec<u8> {
    let w = WIDTH as usize;
    let h = HEIGHT as usize;
    let mut rgba = Vec::with_capacity(w * h * 4);

    // The classic view: real in [-2.4, 1.0], imaginary centered on zero.
    let min_re = -2.4_f64;
    let max_re = 1.0_f64;
    let min_im = -1.2_f64;
    let max_im = 1.2_f64;

    let mut py = 0usize;
    while py < h {
        let c_im = min_im + (max_im - min_im) * (py as f64) / (h as f64);
        let mut px = 0usize;
        while px < w {
            let c_re = min_re + (max_re - min_re) * (px as f64) / (w as f64);

            // Iterate z = z^2 + c until it escapes or hits the ceiling.
            let mut z_re = 0.0_f64;
            let mut z_im = 0.0_f64;
            let mut iter = 0u32;
            while iter < MAX_ITER {
                let re2 = z_re * z_re;
                let im2 = z_im * z_im;
                if re2 + im2 > 4.0 {
                    break;
                }
                z_im = 2.0 * z_re * z_im + c_im;
                z_re = re2 - im2 + c_re;
                iter += 1;
            }

            let (r, g, b) = color_for(iter);
            rgba.push(r);
            rgba.push(g);
            rgba.push(b);
            rgba.push(255);
            px += 1;
        }
        py += 1;
    }
    rgba
}

/// Map an escape count to a color: inside the set is near-black, and the
/// escape bands run through blue and gold so the boundary structure shows.
fn color_for(iter: u32) -> (u8, u8, u8) {
    if iter >= MAX_ITER {
        return (8, 8, 16);
    }
    let t = iter as f32 / MAX_ITER as f32;
    // A simple two-stop gradient: deep blue -> warm gold as t rises.
    let r = (40.0 + t * 215.0) as u8;
    let g = (60.0 + t * 160.0) as u8;
    let b = (120.0 + (1.0 - t) * 120.0) as u8;
    (r, g, b)
}

// ----- widget builders -----

fn stack_root() -> types::WidgetNode {
    node(ROOT_ID, None, types::WidgetKind::Stack)
}

fn image_node() -> types::WidgetNode {
    // No size and no grow: the layout fills a lone image to the window now, so
    // the fractal covers the whole frame.
    node(IMAGE_ID, Some(ROOT_ID), types::WidgetKind::Image)
}

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

// ----- number formatting -----

fn u32_slice(value: u32, buf: &mut [u8; 12]) -> &[u8] {
    if value == 0 {
        if let Some(slot) = buf.get_mut(0) {
            *slot = b'0';
        }
        return buf.get(..1).unwrap_or(b"0");
    }
    let mut scratch = [0u8; 12];
    let mut n = value;
    let mut count = 0usize;
    while n > 0 && count < scratch.len() {
        if let Some(slot) = scratch.get_mut(count) {
            *slot = b'0' + (n % 10) as u8;
        }
        n /= 10;
        count += 1;
    }
    let mut pos = 0usize;
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let (Some(src), Some(dst)) = (scratch.get(i), buf.get_mut(pos)) {
            *dst = *src;
            pos += 1;
        }
    }
    buf.get(..pos).unwrap_or(b"0")
}

bindings::export!(Component with_types_in bindings);
