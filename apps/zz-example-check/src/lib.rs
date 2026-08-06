#![no_std]

// The SDK owns the allocator, the panic handler, and the memory intrinsics
// this guest needs. Nothing calls it directly, so link it explicitly or the
// build fails with "`#[panic_handler]` function required".
extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

// Widget ids are yours to choose. Keep them as constants: the tree refers to
// them and so does `canvas2d::bind`.
const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 320.0;

struct Component;

/// The button rectangle, defined once. Drawing and hit-testing both call this,
/// so they cannot disagree about where the button is.
fn button_rect(width: f32, height: f32) -> gfx::Rect {
    gfx::Rect { x: width / 2.0 - 60.0, y: height - 80.0, width: 120.0, height: 40.0 }
}

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

impl bindings::Guest for Component {
    // The world exports `run: func() -> s32`. No arguments, no Result: read
    // arguments with `args::raw()` and return 0 for success.
    fn run() -> i32 {
        // check-app runs every app once with the bare word `quick`, and kills
        // it after 60 seconds. `args::raw()` is one string, arguments
        // separated by newlines.
        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .any(|arg| arg == b"quick");

        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Counter", size) else { return 30 };
        if window::show(win).is_err() {
            return 31;
        }
        // A window has no widgets until you build the tree.
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err()
            || tree::upsert_node(
                win,
                &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
            )
            .is_err()
        {
            let _ = window::close(win);
            return 32;
        }
        let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
            let _ = window::close(win);
            return 33;
        };

        let mut count: i32 = 0;
        let mut dirty = true;
        // A quick run still opens the window and draws -- check-app clicks it,
        // resizes it, and confirms it stays open. What it must not do is block
        // forever, so the quick path polls a bounded number of rounds and the
        // interactive path blocks until the person closes it.
        let mut rounds: u32 = 0;
        const QUICK_ROUNDS: u32 = 400;

        loop {
            if dirty {
                let Ok(size) = canvas2d::canvas_size(canvas) else { break };
                canvas2d::clear(canvas, rgb(0.11, 0.12, 0.15)).ok();

                let mut label = [0u8; 12];
                let text = format_int(count, &mut label);
                canvas2d::draw_text(
                    canvas,
                    text,
                    gfx::Point { x: size.width / 2.0 - 14.0, y: size.height / 2.0 },
                    48.0,
                    rgb(0.93, 0.94, 0.96),
                )
                .ok();

                let b = button_rect(size.width, size.height);
                canvas2d::fill_rect(canvas, b, rgb(0.22, 0.45, 0.85)).ok();
                canvas2d::draw_text(
                    canvas,
                    "Add one",
                    gfx::Point { x: b.x + 20.0, y: b.y + 26.0 },
                    16.0,
                    rgb(1.0, 1.0, 1.0),
                )
                .ok();

                canvas2d::present(canvas).ok();
                dirty = false;
            }

            // Interactive: block until something happens, so the app sits idle
            // instead of burning a core. Quick: never block, and stop after a
            // bounded number of rounds.
            let event = if quick {
                rounds += 1;
                if rounds > QUICK_ROUNDS {
                    break;
                }
                events::poll()
            } else {
                events::wait(None)
            };

            match event {
                // Always handle this. It is what the window's close button and
                // Ctrl-C both send; an app that ignores it cannot be closed.
                Some(types::Event::CloseRequested(_)) => break,
                // One Pointer event covers press and release; check `pressed`.
                Some(types::Event::Pointer(p)) if p.pressed => {
                    if let Ok(size) = canvas2d::canvas_size(canvas) {
                        let b = button_rect(size.width, size.height);
                        if p.x >= b.x && p.x <= b.x + b.width
                            && p.y >= b.y && p.y <= b.y + b.height
                        {
                            count += 1;
                            dirty = true;
                        }
                    }
                }
                Some(types::Event::Resized(_)) => dirty = true,
                _ => {}
            }
        }

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"counter: window ran\n");
        }
        let _ = window::close(win);
        0
    }
}

/// Integers to text without `alloc` or `format!`.
fn format_int(mut value: i32, buffer: &mut [u8; 12]) -> &str {
    let negative = value < 0;
    let mut at = buffer.len();
    if value == 0 {
        at -= 1;
        buffer[at] = b'0';
    }
    while value != 0 {
        at -= 1;
        buffer[at] = b'0' + (value % 10).unsigned_abs() as u8;
        value /= 10;
    }
    if negative {
        at -= 1;
        buffer[at] = b'-';
    }
    core::str::from_utf8(&buffer[at..]).unwrap_or("0")
}

/// One widget node with the default style.
fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style { width: None, height: None, grow: 0.0, padding: 0.0 },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

bindings::export!(Component with_types_in bindings);
