//! Krate hello-gui — the first GUI component vertical slice.
//!
//! One portable component builds a tiny widget tree (a button and a text
//! field), the host lowers it to real native controls when the native window
//! mode is selected, and a click on the native button comes back to this
//! component as a portable pointer event, which updates the text field.
//!
//! In the default headless mode the same code runs everywhere without a
//! window: the tree is accepted, events stay empty, and the app exits after
//! its bounded loop. That keeps this sample runnable in CI on all hosts.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::args;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const BUTTON_ID: u64 = 2;
const FIELD_ID: u64 = 3;

/// Interactive wait budget: 600 rounds of 50ms is a 30-second demo window.
const MAX_WAIT_ROUNDS: u32 = 600;
/// Automation wait budget used when the app is launched with a `quick` arg.
const QUICK_WAIT_ROUNDS: u32 = 40;
/// How long one wait round blocks, in milliseconds.
const WAIT_ROUND_MILLIS: u32 = 50;
/// How many rounds the window stays open after a click, so the text change
/// is visible before the app exits.
const LINGER_ROUNDS_AFTER_CLICK: u32 = 20;

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
        {
            let _ = window::close(win);
            return 32;
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
        for _ in 0..rounds {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::Pointer(pointer))
                    if pointer.widget == Some(BUTTON_ID) && pointer.pressed =>
                {
                    clicked = true;
                    linger = LINGER_ROUNDS_AFTER_CLICK;
                    let _ = tree::upsert_node(win, &text_field("clicked!"));
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
