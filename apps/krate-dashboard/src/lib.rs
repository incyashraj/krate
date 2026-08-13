//! Krate dashboard — the limitation probe for mixing a canvas with widgets.
//!
//! The wall it tests: every canvas app so far filled the whole window with one
//! drawing. A real dashboard is different -- text stats, labels, and a chart in
//! the same window, laid out together. This probe puts a header and three stat
//! rows above a bar-chart canvas, all in one widget tree, and draws bars into
//! the canvas with gfx.canvas2d. If a canvas cannot coexist with regular
//! widgets, or the bind mis-sizes when it is not the only child, this is where
//! it shows.
//!
//! The bars are a week of mock values; the drawing is plain rectangle fills, no
//! panic paths, only `krate:*`.

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const TITLE_ID: u64 = 2;
const STAT1_ID: u64 = 3;
const STAT2_ID: u64 = 4;
const STAT3_ID: u64 = 5;
const CHART_LABEL_ID: u64 = 6;
const CANVAS_ID: u64 = 7;

const WIDTH: u32 = 460;
const HEIGHT: u32 = 460;

/// A week of mock daily values, 0..=100.
const BARS: [u32; 7] = [42, 65, 58, 80, 73, 91, 60];
const BAR_LABELS: [&str; 7] = ["M", "T", "W", "T", "F", "S", "S"];

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
        let Ok(win) = window::create("Dashboard", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &title()).is_err()
            || tree::upsert_node(win, &stat(STAT1_ID, "Visitors today       1,284")).is_err()
            || tree::upsert_node(win, &stat(STAT2_ID, "Signups              37")).is_err()
            || tree::upsert_node(win, &stat(STAT3_ID, "Revenue              $4,910")).is_err()
            || tree::upsert_node(win, &chart_label()).is_err()
            || tree::upsert_node(win, &canvas_node()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }

        // Bind the canvas and draw the bar chart. The bind sizes to the
        // canvas widget's layout rect, which here is the space left under the
        // stats -- the whole point of the probe.
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => {
                let _ = window::close(win);
                return 33;
            }
        };
        if draw_chart(canvas).is_err() {
            let _ = window::close(win);
            return 34;
        }

        let out = stdio::stdout();
        let _ = out.write(b"dashboard:ok\n");

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
                // Redraw on resize. `draw_chart` already lays out from
                // `canvas2d::canvas_size`, so it was right about the new
                // size the moment it ran -- it just never ran again. The
                // host refits the canvas and the old picture is gone, so
                // without this the window shows a stretched, blurry chart
                // (K-096, and the same shape as K-024).
                Some(types::Event::Resized(_)) => {
                    if draw_chart(canvas).is_err() {
                        break;
                    }
                }
                _ => {}
            }
        }

        let _ = window::close(win);
        0
    }
}

/// Draw the bar chart into the bound canvas.
fn draw_chart(canvas: u64) -> Result<(), gfx::GfxError> {
    let size = canvas2d::canvas_size(canvas)?;
    let w = size.width;
    let h = size.height;

    // A panel background so the chart reads as a card, not a hole.
    canvas2d::clear(canvas, color(0.10, 0.12, 0.16, 1.0))?;

    let count = BARS.len() as f32;
    let gap = 12.0_f32;
    let usable = w - gap * (count + 1.0);
    let bar_w = usable / count;
    let base_y = h - 28.0; // leave room for day labels
    let top_pad = 16.0;
    let chart_h = base_y - top_pad;

    let mut i = 0usize;
    while i < BARS.len() {
        let value = BARS.get(i).copied().unwrap_or(0) as f32 / 100.0;
        let bar_h = chart_h * value;
        let x = gap + (i as f32) * (bar_w + gap);
        let y = base_y - bar_h;
        // A blue bar, brighter toward the top for a bit of depth.
        canvas2d::fill_rect(
            canvas,
            gfx::Rect {
                x,
                y,
                width: bar_w,
                height: bar_h,
            },
            color(0.36, 0.55, 0.98, 1.0),
        )?;
        // The day label under each bar.
        let label = BAR_LABELS.get(i).copied().unwrap_or("");
        canvas2d::draw_text(
            canvas,
            label,
            gfx::Point {
                x: x + bar_w / 2.0 - 4.0,
                y: h - 10.0,
            },
            14.0,
            color(0.7, 0.75, 0.85, 1.0),
        )?;
        i += 1;
    }

    canvas2d::present(canvas)?;
    Ok(())
}

// ----- widget builders -----

fn stack_root() -> types::WidgetNode {
    node(ROOT_ID, None, types::WidgetKind::Stack, None)
}

fn title() -> types::WidgetNode {
    let mut n = node(TITLE_ID, Some(ROOT_ID), types::WidgetKind::Text, Some("This week"));
    n.role = Some(pure_string("heading"));
    n
}

fn stat(id: u64, text: &str) -> types::WidgetNode {
    let mut n = node(id, Some(ROOT_ID), types::WidgetKind::Text, Some(text));
    n.style.height = Some(26.0);
    n.role = Some(pure_string("status"));
    n
}

fn chart_label() -> types::WidgetNode {
    node(
        CHART_LABEL_ID,
        Some(ROOT_ID),
        types::WidgetKind::Text,
        Some("Daily visitors"),
    )
}

fn canvas_node() -> types::WidgetNode {
    // Grows to fill the space under the stats. Not the only child, so this is
    // the mixed-content case the probe exists for.
    let mut n = node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas, None);
    n.style.grow = 1.0;
    n
}

fn node(
    id: u64,
    parent: Option<u64>,
    kind: types::WidgetKind,
    label: Option<&str>,
) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: label.map(pure_string),
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

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

fn pure_string(text: &str) -> String {
    let bytes = text.as_bytes();
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

bindings::export!(Component with_types_in bindings);
