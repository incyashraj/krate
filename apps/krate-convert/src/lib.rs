//! Krate convert — the limitation probe for a form of live values.
//!
//! The wall it tests: a form with more than one field and a computed result.
//! Every app that converts, calculates, or previews as you type has this
//! shape: a couple of input fields, a live output, controls to change the
//! mode. This probe is a temperature converter -- a Celsius field, a
//! Fahrenheit field, the conversion between them shown large, and a row of
//! preset buttons. It puts several TextFields and a result in one layout and
//! proves they lay out together without stepping on each other.
//!
//! In the automated run it fills a preset so the screenshot shows a real
//! conversion. No panic paths, only `krate:*`.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const TITLE_ID: u64 = 2;
const C_FIELD_ID: u64 = 3;
const RESULT_ID: u64 = 4;
const F_FIELD_ID: u64 = 5;
const PRESETS_ID: u64 = 6;
const PRESET_BASE_ID: u64 = 100;

/// Celsius presets the buttons set.
const PRESETS: [i32; 4] = [0, 37, 100, -40];

const WIDTH: u32 = 380;
const HEIGHT: u32 = 360;

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
        let Ok(win) = window::create("Temperature", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }

        // Start at 100 C so the first frame shows a real conversion.
        let mut celsius: i32 = 100;
        if build(win, celsius).is_err() {
            let _ = window::close(win);
            return 32;
        }

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        let rounds = if quick { QUICK_ROUNDS } else { MAX_ROUNDS };
        for _ in 0..rounds {
            match events::wait(Some(ROUND_MILLIS)) {
                // A preset button sets the Celsius value and re-renders every
                // field, so the whole form updates from one press.
                Some(types::Event::Pointer(pointer)) if pointer.pressed => {
                    if let Some(id) = pointer.widget {
                        if id >= PRESET_BASE_ID && id < PRESET_BASE_ID + PRESETS.len() as u64 {
                            let index = (id - PRESET_BASE_ID) as usize;
                            if let Some(&value) = PRESETS.get(index) {
                                celsius = value;
                                let _ = tree::upsert_node(win, &celsius_field(celsius));
                                let _ = tree::upsert_node(win, &result_line(celsius));
                                let _ = tree::upsert_node(win, &fahrenheit_field(celsius));
                            }
                        }
                    }
                }
                Some(types::Event::CloseRequested(id)) if id == win => break,
                _ => {}
            }
        }

        let out = stdio::stdout();
        let _ = out.write(b"celsius:");
        let _ = out.write(i32_slice(celsius, &mut [0u8; 16]));
        let _ = out.write(b"\n");

        let _ = window::close(win);
        0
    }
}

fn build(win: u64, celsius: i32) -> Result<(), ()> {
    ok(tree::set_root(win, &stack_root()))?;
    ok(tree::upsert_node(win, &title()))?;
    ok(tree::upsert_node(win, &celsius_field(celsius)))?;
    ok(tree::upsert_node(win, &result_line(celsius)))?;
    ok(tree::upsert_node(win, &fahrenheit_field(celsius)))?;
    ok(tree::upsert_node(win, &presets_row()))?;
    let mut i = 0usize;
    while i < PRESETS.len() {
        ok(tree::upsert_node(win, &preset_button(i)))?;
        i += 1;
    }
    Ok(())
}

fn ok<T, E>(r: Result<T, E>) -> Result<(), ()> {
    r.map(|_| ()).map_err(|_| ())
}

/// Celsius to Fahrenheit: F = C * 9/5 + 32, in integers (presets are whole).
fn to_fahrenheit(celsius: i32) -> i32 {
    (celsius * 9) / 5 + 32
}

// ----- widget builders -----

fn stack_root() -> types::WidgetNode {
    node(ROOT_ID, None, types::WidgetKind::Stack, None, 0.0)
}

fn title() -> types::WidgetNode {
    let mut n = node(
        TITLE_ID,
        Some(ROOT_ID),
        types::WidgetKind::Text,
        Some("Celsius to Fahrenheit"),
        0.0,
    );
    n.role = Some(pure_string("heading"));
    n
}

fn celsius_field(celsius: i32) -> types::WidgetNode {
    let mut buf = [0u8; 24];
    let text = labeled_value(b"", celsius, b" C", &mut buf);
    let mut n = node(C_FIELD_ID, Some(ROOT_ID), types::WidgetKind::TextField, None, 0.0);
    n.label = Some(pure_string_from_bytes(text));
    n.style.height = Some(40.0);
    n.role = Some(pure_string("textbox"));
    n
}

/// The conversion, shown large in the middle: "100 C = 212 F".
fn result_line(celsius: i32) -> types::WidgetNode {
    let fahrenheit = to_fahrenheit(celsius);
    let mut buf = [0u8; 48];
    let text = conversion_text(celsius, fahrenheit, &mut buf);
    let mut n = node(RESULT_ID, Some(ROOT_ID), types::WidgetKind::Text, None, 0.0);
    n.label = Some(pure_string_from_bytes(text));
    n.style.height = Some(52.0);
    n.role = Some(pure_string("status"));
    n
}

fn fahrenheit_field(celsius: i32) -> types::WidgetNode {
    let mut buf = [0u8; 24];
    let text = labeled_value(b"", to_fahrenheit(celsius), b" F", &mut buf);
    let mut n = node(F_FIELD_ID, Some(ROOT_ID), types::WidgetKind::TextField, None, 0.0);
    n.label = Some(pure_string_from_bytes(text));
    n.style.height = Some(40.0);
    n.role = Some(pure_string("textbox"));
    n
}

fn presets_row() -> types::WidgetNode {
    let mut n = node(PRESETS_ID, Some(ROOT_ID), types::WidgetKind::Grid, None, 0.0);
    n.style.height = Some(48.0);
    n
}

fn preset_button(index: usize) -> types::WidgetNode {
    let value = PRESETS.get(index).copied().unwrap_or(0);
    let mut buf = [0u8; 24];
    let text = labeled_value(b"", value, b" C", &mut buf);
    let mut n = node(
        PRESET_BASE_ID + index as u64,
        Some(PRESETS_ID),
        types::WidgetKind::Button,
        None,
        0.0,
    );
    n.label = Some(pure_string_from_bytes(text));
    n.style.width = Some(76.0);
    n.style.height = Some(40.0);
    n.role = Some(pure_string("button"));
    n
}

fn node(
    id: u64,
    parent: Option<u64>,
    kind: types::WidgetKind,
    label: Option<&str>,
    padding: f32,
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
            padding,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

// ----- byte helpers -----

/// "<prefix><value><suffix>", e.g. "212 F".
fn labeled_value<'a>(
    prefix: &[u8],
    value: i32,
    suffix: &[u8],
    buf: &'a mut [u8; 24],
) -> &'a [u8] {
    let mut pos = 0usize;
    for byte in prefix {
        push(buf, &mut pos, *byte);
    }
    let mut num = [0u8; 16];
    for byte in i32_slice(value, &mut num) {
        push(buf, &mut pos, *byte);
    }
    for byte in suffix {
        push(buf, &mut pos, *byte);
    }
    buf.get(..pos).unwrap_or(b"")
}

/// "100 C = 212 F".
fn conversion_text<'a>(celsius: i32, fahrenheit: i32, buf: &'a mut [u8; 48]) -> &'a [u8] {
    let mut pos = 0usize;
    let mut num = [0u8; 16];
    for byte in i32_slice(celsius, &mut num) {
        push(buf, &mut pos, *byte);
    }
    for byte in b" C  =  " {
        push(buf, &mut pos, *byte);
    }
    for byte in i32_slice(fahrenheit, &mut num) {
        push(buf, &mut pos, *byte);
    }
    for byte in b" F" {
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

fn i32_slice(value: i32, buf: &mut [u8; 16]) -> &[u8] {
    let mut pos = 0usize;
    let mut v = value;
    if v < 0 {
        push(buf, &mut pos, b'-');
        // Negate into i64 to avoid i32::MIN overflow on negation.
        let mag = (i64::from(v)).unsigned_abs();
        return finish_uint(buf, pos, mag);
    }
    let mag = v as u64;
    v = 0;
    let _ = v;
    finish_uint(buf, pos, mag)
}

fn finish_uint(buf: &mut [u8; 16], mut pos: usize, value: u64) -> &[u8] {
    if value == 0 {
        push(buf, &mut pos, b'0');
        return buf.get(..pos).unwrap_or(b"0");
    }
    let mut scratch = [0u8; 20];
    let mut n = value;
    let mut count = 0usize;
    while n > 0 && count < scratch.len() {
        if let Some(slot) = scratch.get_mut(count) {
            *slot = b'0' + (n % 10) as u8;
        }
        n /= 10;
        count += 1;
    }
    let mut i = count;
    while i > 0 {
        i -= 1;
        if let Some(digit) = scratch.get(i) {
            push(buf, &mut pos, *digit);
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
