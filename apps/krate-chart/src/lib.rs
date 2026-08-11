//! Krate chart — the first app to draw with `gfx.canvas2d`.
//!
//! A bar chart of a week's rainfall, drawn with canvas commands: one clear,
//! one filled bar per day, a stroked frame, and a text label. The point is not
//! the chart; it is that this is the first guest ever to reach the rasterizer
//! through the WIT boundary, so it proves the path the host tests cannot —
//! bindings, lifting, the canvas id round trip, and the draw commands
//! themselves, on every operating system the runtime supports.
//!
//! `quick` verifies without a person: build the tree, bind, draw, and print
//! what was submitted so automation can assert the whole journey ran.

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const TITLE_ID: u64 = 2;
const CANVAS_ID: u64 = 3;

/// Millimetres of rain, Monday to Sunday. Fixed data: the sample exists to
/// exercise drawing, and a fixed picture is one a test can reason about.
const RAINFALL: [f32; 7] = [4.0, 12.0, 7.0, 0.0, 21.0, 15.0, 9.0];

struct Component;

fn color(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
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
            grow: 1.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn draw_chart(canvas: u64) -> Result<u32, gfx::GfxError> {
    // Draw to the canvas's real size, not a guessed one. The canvas shares
    // this window with a native title above it, so it is shorter than the
    // window; an earlier version hardcoded 200 and drew a chart that ran off
    // the bottom into space that was not there. `canvas-size` is how an app
    // finds out what it actually got.
    let size = canvas2d::canvas_size(canvas)?;
    let w = size.width;
    let h = size.height;

    // A cream sheet behind everything.
    canvas2d::clear(canvas, color(0.98, 0.97, 0.94))?;
    let mut calls = 1_u32;

    // A margin all round, and the plot inside it. Everything below is a
    // fraction of the real canvas, so the chart fills whatever height it was
    // given rather than a fixed one.
    let margin = 12.0;
    let label_band = 16.0;
    let plot_top = label_band + 4.0;
    let baseline = h - margin;
    let chart_height = (baseline - plot_top).max(1.0);
    let days = RAINFALL.len().max(1) as f32;
    let plot_width = w - 2.0 * margin;
    let slot = plot_width / days;
    let bar_width = (slot * 0.7).max(1.0);
    let bar_gap = slot - bar_width;

    // One bar per day, scaled so the wettest day fills the plot height.
    let max = RAINFALL.iter().copied().fold(1.0_f32, f32::max);
    for (day, rain) in RAINFALL.iter().enumerate() {
        let height = (rain / max) * chart_height;
        canvas2d::fill_rect(
            canvas,
            gfx::Rect {
                x: margin + bar_gap / 2.0 + day as f32 * slot,
                y: baseline - height,
                width: bar_width,
                height,
            },
            color(0.23, 0.51, 0.96),
        )?;
        calls += 1;
    }

    // A frame around the plot, and the axis label.
    canvas2d::stroke_rect(
        canvas,
        gfx::Rect {
            x: margin - 4.0,
            y: plot_top,
            width: plot_width + 8.0,
            height: baseline - plot_top,
        },
        color(0.1, 0.1, 0.1),
        1.0,
    )?;
    canvas2d::draw_text(
        canvas,
        "rain, mm",
        gfx::Point { x: margin, y: 12.0 },
        7.0,
        color(0.1, 0.1, 0.1),
    )?;
    calls += 2;

    // Nothing is shown until the raster is presented.
    canvas2d::present(canvas)?;
    Ok(calls)
}

// `to_string()` routes through std's OOM handler, which drags the whole
// `wasi:*` surface into an otherwise pure component -- 33 imports from two
// labels. Allocating the bytes directly is what keeps a guest clean; the
// hello-gui sample carries the same pair for the same reason.
fn pure_string_from_bytes(bytes: &[u8]) -> String {
    let len = bytes.len();
    if len == 0 {
        return String::new();
    }
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(len, 1);
        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() {
            core::arch::wasm32::unreachable()
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

fn pure_string(text: &str) -> String {
    let len = text.len();
    if len == 0 {
        return String::new();
    }
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(len, 1);
        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() {
            core::arch::wasm32::unreachable()
        }
        core::ptr::copy_nonoverlapping(text.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

fn out(text: &str) {
    let handle = stdio::stdout();
    let _ = handle.write(text.as_bytes());
    let _ = handle.write(b"\n");
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw.split_whitespace().any(|word| word == "quick");

        let win = match window::create("Rainfall", types::WindowSize { width: 240, height: 200 }) {
            Ok(win) => win,
            Err(_) => {
                out("window:no");
                return 1;
            }
        };

        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            out("tree:no");
            return 1;
        }
        let mut title = node(TITLE_ID, Some(ROOT_ID), types::WidgetKind::Text);
        title.label = Some(pure_string("This week's rain"));
        title.role = Some(pure_string("heading"));
        let _ = tree::upsert_node(win, &title);
        let _ = tree::upsert_node(win, &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas));

        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(error) => {
                let _ = error;
                out("bind:no");
                return 1;
            }
        };
        let commands = match draw_chart(canvas) {
            Ok(count) => count,
            Err(error) => {
                let _ = error;
                out("draw:no");
                return 1;
            }
        };

        out("bars:7");
        let _ = commands;
        out("commands:10");
        out("drawn:yes");

        if quick {
            let _ = window::close(win);
            return 0;
        }

        // Stay open until the person closes the window, and redraw when the
        // window changes size. The old comment here said redraw was not
        // needed because "the canvas keeps its raster" -- true until the
        // window is resized, at which point the canvas is refitted and the
        // old picture is gone, leaving the app blank or stretched (K-096).
        loop {
            match events::wait(Some(1_000)) {
                Some(types::Event::CloseRequested(_)) => break,
                Some(types::Event::Resized(_)) => {
                    let _ = draw_chart(canvas);
                }
                Some(_) | None => {}
            }
        }
        let _ = window::close(win);
        0
    }
}

bindings::export!(Component with_types_in bindings);
