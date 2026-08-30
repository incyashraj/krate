//! Krate settings -- the limitation probe for tabs and the full control set.
//!
//! The wall it tests: real apps are not one flat column. They have tab strips,
//! nested containers, and a mix of controls -- switches, sliders, radio groups,
//! progress bars -- all laid out together and all needing to look right. Every
//! earlier probe used one or two widget kinds; this one puts the whole palette
//! on screen inside a tab strip, where only the selected panel takes space.
//! If tabs, nesting, or any one control breaks, it shows here in one shot.
//!
//! Clicking a tab in the strip switches panels; the app owns the selection the
//! way a list does. Everything is fixed-size and panic-free, so the component
//! imports only `krate:*`.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const TITLE_ID: u64 = 2;
const TABS_ID: u64 = 3;
/// Panel container ids, one per tab.
const PANEL_BASE_ID: u64 = 10;
/// Controls inside the panels start here.
const CTRL_BASE_ID: u64 = 100;

const TAB_COUNT: u32 = 3;
const TAB_LABELS: [&str; 3] = ["General", "Display", "About"];

const WIDTH: u32 = 460;
const HEIGHT: u32 = 420;

const QUICK_ROUNDS: u32 = 20;
const MAX_ROUNDS: u32 = 800;
const ROUND_MILLIS: u32 = 50;

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH,
            height: HEIGHT,
        };
        let Ok(win) = window::create("Settings", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }

        let mut selected_tab: u32 = 0;
        if build(win, selected_tab).is_err() {
            let _ = window::close(win);
            return 32;
        }

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");
        // The automated run switches to the Display tab so a screenshot shows
        // the slider-and-switch panel, the busiest one, rather than the plain
        // first tab.
        if quick {
            selected_tab = 1;
            let _ = tree::upsert_node(win, &tab_strip(selected_tab));
        }

        // A real session ends when the person closes the window, never
        // on a round count: 600 rounds x 50 ms quietly shut the window
        // after thirty seconds of use (K-092). `quick` keeps its bound
        // so a headless check can never hang.
        let rounds = if quick { QUICK_ROUNDS } else { u32::MAX };
        for _ in 0..rounds {
            match events::wait(Some(ROUND_MILLIS)) {
                // A press on the tab strip: the hit widget id is the tab strip
                // itself; the app cannot tell which tab from a container press,
                // so it advances to the next tab, which is enough to prove the
                // strip switches panels.
                Some(types::Event::Pointer(pointer))
                    if pointer.pressed && pointer.widget == Some(TABS_ID) =>
                {
                    selected_tab = (selected_tab + 1) % TAB_COUNT;
                    let _ = tree::upsert_node(win, &tab_strip(selected_tab));
                }
                Some(types::Event::CloseRequested(id)) if id == win => break,
                _ => {}
            }
        }

        let out = stdio::stdout();
        let _ = out.write(b"tab:");
        let _ = out.write(u32_slice(selected_tab, &mut [0u8; 12]));
        let _ = out.write(b"\n");

        let _ = window::close(win);
        0
    }
}

/// Build the whole tree: title, tab strip, and the controls in each panel.
fn build(win: u64, selected: u32) -> Result<(), ()> {
    ok(tree::set_root(win, &stack_root()))?;
    ok(tree::upsert_node(win, &title()))?;
    ok(tree::upsert_node(win, &tab_strip(selected)))?;

    // Panel 0: General -- a couple of switches.
    ok(tree::upsert_node(win, &panel(0)))?;
    ok(tree::upsert_node(win, &switch_ctrl(0, "Launch at login", true)))?;
    ok(tree::upsert_node(win, &switch_ctrl(1, "Check for updates", false)))?;
    ok(tree::upsert_node(win, &label_ctrl(2, "Two toggles, both real controls.")))?;

    // Panel 1: Display -- a slider, a progress bar, a switch.
    ok(tree::upsert_node(win, &panel(1)))?;
    ok(tree::upsert_node(win, &slider_ctrl(3, "Brightness", 0.72)))?;
    ok(tree::upsert_node(win, &progress_ctrl(4, "Download", 0.4)))?;
    ok(tree::upsert_node(win, &switch_ctrl(5, "Dark mode", true)))?;

    // Panel 2: About -- radio group.
    ok(tree::upsert_node(win, &panel(2)))?;
    ok(tree::upsert_node(win, &radio_ctrl(6, "Stable channel", true)))?;
    ok(tree::upsert_node(win, &radio_ctrl(7, "Beta channel", false)))?;
    ok(tree::upsert_node(win, &label_ctrl(8, "Krate settings probe, version 1.")))?;
    Ok(())
}

fn ok<T, E>(result: Result<T, E>) -> Result<(), ()> {
    result.map(|_| ()).map_err(|_| ())
}

// ----- widget builders -----

fn stack_root() -> types::WidgetNode {
    node(ROOT_ID, None, types::WidgetKind::Stack, None)
}

fn title() -> types::WidgetNode {
    let mut n = node(TITLE_ID, Some(ROOT_ID), types::WidgetKind::Text, Some("Settings"));
    n.role = Some(pure_string("heading"));
    n
}

/// The tab strip: a Tabs container whose selected index picks the live panel.
/// Its label is the current tab name so the strip reads as a header.
fn tab_strip(selected: u32) -> types::WidgetNode {
    let name = TAB_LABELS.get(selected as usize).copied().unwrap_or("");
    let mut buf = [0u8; 48];
    let text = tab_header(name, &mut buf);
    let mut n = node(TABS_ID, Some(ROOT_ID), types::WidgetKind::Tabs, None);
    n.label = Some(pure_string_from_bytes(text));
    n.selected = Some(selected);
    n.style.grow = 1.0;
    n.role = Some(pure_string("tablist"));
    n
}

fn panel(index: u32) -> types::WidgetNode {
    let mut n = node(
        PANEL_BASE_ID + u64::from(index),
        Some(TABS_ID),
        types::WidgetKind::Stack,
        None,
    );
    n.style.padding = 4.0;
    n
}

fn ctrl_parent(index: u64) -> u64 {
    // Controls 0-2 in panel 0, 3-5 in panel 1, 6-8 in panel 2.
    PANEL_BASE_ID + index / 3
}

fn switch_ctrl(index: u64, label: &str, on: bool) -> types::WidgetNode {
    let mut n = node(
        CTRL_BASE_ID + index,
        Some(ctrl_parent(index)),
        types::WidgetKind::Switch,
        Some(label),
    );
    n.checked = Some(on);
    n.style.height = Some(28.0);
    n.role = Some(pure_string("switch"));
    n
}

fn slider_ctrl(index: u64, label: &str, value: f32) -> types::WidgetNode {
    let mut n = node(
        CTRL_BASE_ID + index,
        Some(ctrl_parent(index)),
        types::WidgetKind::Slider,
        Some(label),
    );
    n.value = Some(value);
    n.style.height = Some(28.0);
    n.role = Some(pure_string("slider"));
    n
}

fn progress_ctrl(index: u64, label: &str, value: f32) -> types::WidgetNode {
    let mut n = node(
        CTRL_BASE_ID + index,
        Some(ctrl_parent(index)),
        types::WidgetKind::Progress,
        Some(label),
    );
    n.value = Some(value);
    n.style.height = Some(24.0);
    n.role = Some(pure_string("progressbar"));
    n
}

fn radio_ctrl(index: u64, label: &str, on: bool) -> types::WidgetNode {
    let mut n = node(
        CTRL_BASE_ID + index,
        Some(ctrl_parent(index)),
        types::WidgetKind::Radio,
        Some(label),
    );
    n.checked = Some(on);
    n.style.height = Some(26.0);
    n.role = Some(pure_string("radio"));
    n
}

fn label_ctrl(index: u64, label: &str) -> types::WidgetNode {
    node(
        CTRL_BASE_ID + index,
        Some(ctrl_parent(index)),
        types::WidgetKind::Text,
        Some(label),
    )
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

// ----- byte helpers -----

/// "Display  (tap to switch)" so the strip explains itself in a screenshot.
fn tab_header<'a>(name: &str, buf: &'a mut [u8; 48]) -> &'a [u8] {
    let mut pos = 0usize;
    for byte in name.as_bytes() {
        push(buf, &mut pos, *byte);
    }
    for byte in b"   (tap to switch)" {
        push(buf, &mut pos, *byte);
    }
    buf.get(..pos).unwrap_or(b"")
}

fn push(buf: &mut [u8], pos: &mut usize, byte: u8) {
    if let Some(slot) = buf.get_mut(*pos) {
        *slot = byte;
        *pos += 1;
    }
}

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

fn pure_string(text: &str) -> String {
    pure_string_from_bytes(text.as_bytes())
}

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

bindings::export!(Component with_types_in bindings);
