//! Krate Weather — a good-looking weather card, one shareable file.
//!
//! A polished, static weather card: a city, a hero temperature, the current
//! condition, a five-day forecast, and a couple of stat lines. No network,
//! no files, no store — the data is mock and hardcoded. It exists as a
//! screenshot-worthy sample for the product gallery: a real-world layout that
//! shows what a Krate GUI app looks like when it is dressed up rather than
//! probing a capability.
//!
//! Held to the same discipline as the other samples: a Krate component may
//! import only `krate:*`, so every access is non-panicking and every owned
//! `String` is built through the raw-allocation helper. A stray `.unwrap()`,
//! `[i]` index, or `format!` would reach std's allocation-error handler and
//! drag the whole `wasi:*` import set into an otherwise pure component.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

// Widget ids. The tree is static, so each node gets a fixed id.
const ROOT_ID: u64 = 1;
const HEADER_CARD_ID: u64 = 2;
const CITY_ID: u64 = 3;
const TEMP_ID: u64 = 4;
const CONDITION_ID: u64 = 5;
const STATS_ROW_ID: u64 = 6;
const STAT_HUMIDITY_ID: u64 = 7;
const STAT_WIND_ID: u64 = 8;
const STAT_FEELS_ID: u64 = 9;
const FORECAST_TITLE_ID: u64 = 10;
const FORECAST_LIST_ID: u64 = 11;
const FORECAST_ROW_BASE_ID: u64 = 20;

const WINDOW_W: u32 = 380;
const WINDOW_H: u32 = 480;

/// Interactive runs stay open until the window is closed; automated runs pass
/// `quick` and exit promptly.
const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 50;

/// Consecutive quiet rounds before an unwatched run stops waiting, about ten
/// seconds. A person clicking or typing resets it, so it is only ever reached
/// when there is no window to come back to at all.
const MAX_IDLE_ROUNDS: u32 = 200;

/// One forecast entry: a day, its condition, and its high, all static.
struct Forecast {
    day: &'static str,
    condition: &'static str,
    high: &'static str,
}

/// The five-day forecast, hardcoded. Kept short so the card reads at a glance.
const FORECAST: [Forecast; 5] = [
    Forecast { day: "Mon", condition: "Sunny", high: "66°" },
    Forecast { day: "Tue", condition: "Cloudy", high: "61°" },
    Forecast { day: "Wed", condition: "Sunny", high: "68°" },
    Forecast { day: "Thu", condition: "Clear", high: "70°" },
    Forecast { day: "Fri", condition: "Rain", high: "63°" },
];

struct Component;

/// Build an owned `String` without touching std's allocation-error handler,
/// which would drag the `wasi:*` import set into the component. Mirrors the
/// raw-allocation path the generated bindings use. Copied verbatim from the
/// checklist sample.
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
            #[cfg(target_arch = "wasm32")]
            core::arch::wasm32::unreachable();
            #[cfg(not(target_arch = "wasm32"))]
            std::process::abort();
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

/// Concatenate several static strings into one owned `String` without
/// `format!`, which would reach the allocation-error handler. Used to build
/// stat lines and forecast rows into a single label.
fn joined(parts: &[&str]) -> String {
    let mut buf = [0u8; 64];
    let mut len = 0usize;
    for part in parts {
        for byte in part.as_bytes() {
            if let Some(slot) = buf.get_mut(len) {
                *slot = *byte;
                len += 1;
            }
        }
    }
    pure_string(core::str::from_utf8(buf.get(..len).unwrap_or(&[])).unwrap_or(""))
}

// ---- widget tree ----------------------------------------------------------

/// Small constructor so every node builder stays one readable block.
#[allow(clippy::too_many_arguments)]
fn node(
    id: u64,
    parent: Option<u64>,
    kind: types::WidgetKind,
    label: Option<String>,
    role: Option<String>,
    width: f32,
    height: f32,
    grow: f32,
    padding: f32,
) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label,
        role,
        style: types::Style {
            width: Some(width),
            height: Some(height),
            grow,
            padding,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// The outer column that holds every card.
fn root() -> types::WidgetNode {
    node(
        ROOT_ID,
        None,
        types::WidgetKind::Stack,
        None,
        None,
        WINDOW_W as f32,
        WINDOW_H as f32,
        0.0,
        16.0,
    )
}

/// The hero card: city, big temperature, and the current condition, grouped in
/// their own stack so they read as one block at the top.
fn header_card() -> types::WidgetNode {
    node(
        HEADER_CARD_ID,
        Some(ROOT_ID),
        types::WidgetKind::Stack,
        None,
        None,
        340.0,
        176.0,
        0.0,
        12.0,
    )
}

fn city() -> types::WidgetNode {
    let mut n = node(
        CITY_ID,
        Some(HEADER_CARD_ID),
        types::WidgetKind::Text,
        Some(pure_string("San Francisco")),
        Some(pure_string("heading")),
        316.0,
        26.0,
        0.0,
        0.0,
    );
    n.role = Some(pure_string("heading"));
    n
}

/// The hero number. Its tall explicit height is what makes the layout give it
/// room to read as the biggest thing on the card.
fn temp() -> types::WidgetNode {
    let mut n = node(
        TEMP_ID,
        Some(HEADER_CARD_ID),
        types::WidgetKind::Text,
        Some(pure_string("64°")),
        Some(pure_string("heading")),
        316.0,
        86.0,
        0.0,
        0.0,
    );
    n.role = Some(pure_string("heading"));
    n
}

fn condition() -> types::WidgetNode {
    let mut n = node(
        CONDITION_ID,
        Some(HEADER_CARD_ID),
        types::WidgetKind::Text,
        Some(pure_string("Partly cloudy")),
        Some(pure_string("status")),
        316.0,
        24.0,
        0.0,
        0.0,
    );
    n.role = Some(pure_string("status"));
    n
}

/// The stat strip: three short lines grouped in a wrapping row.
fn stats_row() -> types::WidgetNode {
    node(
        STATS_ROW_ID,
        Some(ROOT_ID),
        types::WidgetKind::Grid,
        None,
        None,
        340.0,
        34.0,
        0.0,
        4.0,
    )
}

fn stat(id: u64, label: &str, value: &str) -> types::WidgetNode {
    let mut n = node(
        id,
        Some(STATS_ROW_ID),
        types::WidgetKind::Text,
        Some(joined(&[label, "  ", value])),
        Some(pure_string("status")),
        104.0,
        26.0,
        0.0,
        0.0,
    );
    n.role = Some(pure_string("status"));
    n
}

fn forecast_title() -> types::WidgetNode {
    let mut n = node(
        FORECAST_TITLE_ID,
        Some(ROOT_ID),
        types::WidgetKind::Text,
        Some(pure_string("5-Day Forecast")),
        Some(pure_string("heading")),
        340.0,
        24.0,
        0.0,
        0.0,
    );
    n.role = Some(pure_string("heading"));
    n
}

/// The forecast card: a column of one row per day.
fn forecast_list() -> types::WidgetNode {
    node(
        FORECAST_LIST_ID,
        Some(ROOT_ID),
        types::WidgetKind::Stack,
        None,
        None,
        340.0,
        190.0,
        0.0,
        8.0,
    )
}

/// One forecast row: "Mon     Sunny     66°" as a single aligned line. Spaces
/// do the aligning; the label is one string.
fn forecast_row(index: usize, entry: &Forecast) -> types::WidgetNode {
    let text = joined(&[entry.day, "     ", entry.condition, "     ", entry.high]);
    let mut n = node(
        FORECAST_ROW_BASE_ID + index as u64,
        Some(FORECAST_LIST_ID),
        types::WidgetKind::Text,
        Some(text),
        Some(pure_string("status")),
        316.0,
        28.0,
        0.0,
        0.0,
    );
    n.role = Some(pure_string("status"));
    n
}

/// Build the whole tree once. Returns false if any upsert fails.
fn build(win: u64) -> bool {
    if tree::set_root(win, &root()).is_err() {
        return false;
    }
    let ok = tree::upsert_node(win, &header_card()).is_ok()
        && tree::upsert_node(win, &city()).is_ok()
        && tree::upsert_node(win, &temp()).is_ok()
        && tree::upsert_node(win, &condition()).is_ok()
        && tree::upsert_node(win, &stats_row()).is_ok()
        && tree::upsert_node(win, &stat(STAT_HUMIDITY_ID, "Humidity", "72%")).is_ok()
        && tree::upsert_node(win, &stat(STAT_WIND_ID, "Wind", "8 mph")).is_ok()
        && tree::upsert_node(win, &stat(STAT_FEELS_ID, "Feels", "61°")).is_ok()
        && tree::upsert_node(win, &forecast_title()).is_ok()
        && tree::upsert_node(win, &forecast_list()).is_ok();
    if !ok {
        return false;
    }
    for i in 0..FORECAST.len() {
        let Some(entry) = FORECAST.get(i) else {
            continue;
        };
        if tree::upsert_node(win, &forecast_row(i, entry)).is_err() {
            return false;
        }
    }
    true
}

// ---- the app --------------------------------------------------------------

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WINDOW_W,
            height: WINDOW_H,
        };
        let Ok(win) = window::create("Weather", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if !build(win) {
            let _ = window::close(win);
            return 32;
        }

        // A quick automated run builds the card and exits at once. It must NOT
        // enter the event-wait loop — waiting on window events during a
        // headless/verify run is what makes verification hang.
        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if quick {
            let _ = window::close(win);
            let out = stdio::stdout();
            let _ = out.write(b"weather:ok\n");
            return 0;
        }

        let mut close_requested = false;
        let mut idle_rounds = 0u32;
        for _ in 0..MAX_WAIT_ROUNDS {
            let event = events::wait(Some(WAIT_ROUND_MILLIS));
            if event.is_none() {
                idle_rounds += 1;
                if idle_rounds >= MAX_IDLE_ROUNDS {
                    break;
                }
                continue;
            }
            idle_rounds = 0;
            if let Some(types::Event::CloseRequested(_)) = event {
                close_requested = true;
                break;
            }
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"weather:ok\n");

        if close_requested {
            2
        } else {
            0
        }
    }
}

bindings::export!(Component with_types_in bindings);
