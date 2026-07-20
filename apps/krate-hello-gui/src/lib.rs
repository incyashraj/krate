//! Krate hello-gui — the first GUI component vertical slice.
//!
//! One portable component builds a tiny widget tree (a button and a text
//! field), the host lowers it to real native controls when the native window
//! mode is selected, and a click on the native button comes back to this
//! component as a portable pointer event, which updates the text field.
//!
//! Typing works too: clicking the field focuses it, portable text-input
//! events append to a fixed buffer (Backspace deletes), the field re-renders
//! on every change, and the final text is written to stdout as
//! `typed:<text>` so automation can assert the full keyboard round trip.
//!
//! In the default headless mode the same code runs everywhere without a
//! window: the tree is accepted, events stay empty, and the app exits after
//! its bounded loop. That keeps this sample runnable in CI on all hosts.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const BUTTON_ID: u64 = 2;
const FIELD_ID: u64 = 3;
const CHECKBOX_ID: u64 = 4;
const PROGRESS_ID: u64 = 5;
const SCROLL_ID: u64 = 6;
const SCROLL_LINE_BASE_ID: u64 = 10;
const SCROLL_LINE_LABELS: [&str; 8] = [
    "scroll line one",
    "scroll line two",
    "scroll line three",
    "scroll line four",
    "scroll line five",
    "scroll line six",
    "scroll line seven",
    "scroll line eight",
];

/// Interactive wait budget: 600 rounds of 50ms is a 30-second demo window.
const MAX_WAIT_ROUNDS: u32 = 600;
/// Automation wait budget used when the app is launched with a `quick` arg.
const QUICK_WAIT_ROUNDS: u32 = 40;
/// How long one wait round blocks, in milliseconds.
const WAIT_ROUND_MILLIS: u32 = 50;
/// How many rounds the window stays open after a click, so the text change
/// is visible before the app exits.
const LINGER_ROUNDS_AFTER_CLICK: u32 = 20;
/// Maximum number of typed bytes the demo buffers and displays.
const TYPED_CAPACITY: usize = 64;

struct Component;

fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
        style: types::Style {
            width: Some(640.0),
            height: Some(480.0),
            grow: 0.0,
            padding: 16.0,
        },
        checked: None,
        value: None,
    }
}

fn click_button() -> types::WidgetNode {
    types::WidgetNode {
        id: BUTTON_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Button,
        label: Some(pure_string("Click me")),
        role: Some(pure_string("button")),
        style: types::Style {
            width: Some(160.0),
            height: Some(32.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
    }
}

fn text_field(label: &str) -> types::WidgetNode {
    types::WidgetNode {
        id: FIELD_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::TextField,
        label: Some(pure_string(label)),
        role: Some(pure_string("textfield")),
        style: types::Style {
            width: Some(320.0),
            height: Some(28.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
    }
}

fn scroll_area() -> types::WidgetNode {
    types::WidgetNode {
        id: SCROLL_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Scroll,
        label: None,
        role: Some(pure_string("scrollarea")),
        style: types::Style {
            width: Some(320.0),
            height: Some(120.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
    }
}

fn scroll_line(index: usize) -> types::WidgetNode {
    types::WidgetNode {
        id: SCROLL_LINE_BASE_ID + index as u64,
        parent: Some(SCROLL_ID),
        kind: types::WidgetKind::Text,
        label: Some(pure_string(SCROLL_LINE_LABELS[index & 7])),
        role: Some(pure_string("text")),
        style: types::Style {
            width: Some(300.0),
            height: Some(24.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
    }
}

fn robot_checkbox(checked: bool) -> types::WidgetNode {
    types::WidgetNode {
        id: CHECKBOX_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Checkbox,
        label: Some(pure_string("robot was here")),
        role: Some(pure_string("checkbox")),
        style: types::Style {
            width: Some(200.0),
            height: Some(20.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: Some(checked),
        value: None,
    }
}

fn typing_progress(fraction: f32) -> types::WidgetNode {
    types::WidgetNode {
        id: PROGRESS_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Progress,
        label: None,
        role: Some(pure_string("progressbar")),
        style: types::Style {
            width: Some(320.0),
            height: Some(12.0),
            grow: 0.0,
            padding: 0.0,
        },
        checked: None,
        value: Some(if fraction > 1.0 { 1.0 } else { fraction }),
    }
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: 640,
            height: 480,
        };
        let Ok(win) = window::create("Krate Hello GUI", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }

        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &click_button()).is_err()
            || tree::upsert_node(win, &text_field("waiting for click")).is_err()
            || tree::upsert_node(win, &robot_checkbox(false)).is_err()
            || tree::upsert_node(win, &typing_progress(0.0)).is_err()
            || tree::upsert_node(win, &scroll_area()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }

        // Eight lines of 24px content in a 120px scroll area: the host
        // clips and scrolls them; this component never handles a wheel.
        let mut line_index = 0usize;
        while line_index < SCROLL_LINE_LABELS.len() {
            if tree::upsert_node(win, &scroll_line(line_index)).is_err() {
                let _ = window::close(win);
                return 32;
            }
            line_index += 1;
        }

        // Byte equality, not str::contains: pattern-search machinery pulls
        // std panic paths (and with them WASI imports) into the component.
        let rounds = if args::raw().as_bytes() == b"quick" {
            QUICK_WAIT_ROUNDS
        } else {
            MAX_WAIT_ROUNDS
        };

        let mut clicked = false;
        let mut close_requested = false;
        let mut linger = 0u32;
        let mut checkbox_on = false;
        // Fixed-capacity typing buffer: growth-free so no allocation-error
        // machinery (and its WASI imports) enters the component.
        let mut typed = [0u8; TYPED_CAPACITY];
        let mut typed_len = 0usize;
        for _ in 0..rounds {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::Pointer(pointer))
                    if pointer.widget == Some(BUTTON_ID) && pointer.pressed =>
                {
                    clicked = true;
                    linger = LINGER_ROUNDS_AFTER_CLICK;
                    let _ = tree::upsert_node(win, &text_field("clicked!"));
                }
                Some(types::Event::Pointer(pointer))
                    if pointer.widget == Some(CHECKBOX_ID) && pointer.pressed =>
                {
                    checkbox_on = !checkbox_on;
                    let _ = tree::upsert_node(win, &robot_checkbox(checkbox_on));
                }
                Some(types::Event::TextInput(text)) => {
                    for byte in text.as_bytes() {
                        let printable = byte.is_ascii_graphic() || *byte == b' ';
                        if !printable {
                            continue;
                        }
                        // Non-panicking write: indexed stores emit bounds
                        // checks whose panic path drags WASI into the
                        // component (same lesson as str::contains).
                        if let Some(slot) = typed.get_mut(typed_len) {
                            *slot = *byte;
                            typed_len += 1;
                        }
                    }
                    let shown = typed.get(..typed_len).unwrap_or(&[]);
                    let _ = tree::upsert_node(win, &text_field_bytes(shown));
                    let _ =
                        tree::upsert_node(win, &typing_progress(typed_len as f32 / 16.0));
                }
                Some(types::Event::Key(key))
                    if key.pressed && key.key.as_bytes() == b"Backspace" =>
                {
                    if typed_len > 0 {
                        typed_len -= 1;
                        let shown = typed.get(..typed_len).unwrap_or(&[]);
                        let _ = tree::upsert_node(win, &text_field_bytes(shown));
                    }
                }
                Some(types::Event::CloseRequested(id)) if id == win => {
                    close_requested = true;
                    break;
                }
                _ => {}
            }
            if clicked {
                if linger == 0 {
                    break;
                }
                linger -= 1;
            }
        }

        let _ = window::close(win);

        // Report the typed text so scripts can assert the keyboard round
        // trip: `typed:<text>` on stdout, written as raw bytes.
        if typed_len > 0 {
            let out = stdio::stdout();
            let _ = out.write(b"typed:");
            let _ = out.write(typed.get(..typed_len).unwrap_or(&[]));
            let _ = out.write(b"\n");
        }

        // The exit code reports what the run observed, so scripts and tests
        // can assert behavior in both headless and native modes:
        //   0 = native click round trip observed
        //   1 = clean bounded run without a click (normal headless outcome)
        //   2 = user closed the window before clicking
        if clicked {
            0
        } else if close_requested {
            2
        } else {
            1
        }
    }
}

/// Build an owned `String` without touching std's allocation-error handler.
///
/// `String::from` and friends reference std's OOM handler, which drags the
/// whole `wasi:cli`/`wasi:io` import set into an otherwise pure component.
/// This mirrors the raw-allocation path the generated bindings use for
/// lifting, trapping on allocation failure instead. Keeps the component
/// importing only `krate:*` interfaces.
/// Text-field node whose label comes from raw typed bytes (ASCII only).
fn text_field_bytes(bytes: &[u8]) -> types::WidgetNode {
    let mut node = text_field("");
    node.label = Some(pure_string_from_bytes(bytes));
    node
}

/// `pure_string` for a raw ASCII byte slice.
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

bindings::export!(Component with_types_in bindings);
