//! Krate calc — a calculator, the limitation probe for dense button grids.
//!
//! The wall it tests: a real app is not one column of widgets, it is a grid of
//! them. Four rows of four buttons plus a wide display, each button a live
//! click target that routes back by widget id, arithmetic done in a no_std
//! guest that must never panic. If the layout, the hit routing, or the spacing
//! break under twenty widgets in a grid, this is where it shows.
//!
//! Everything is integer-and-fraction arithmetic on f64 kept off the panic
//! paths: no indexing that can trap, no `format!`, no allocation-error handler.
//! The display string is built by hand from digits so the whole component
//! imports only `krate:*` and stays portable.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const DISPLAY_ID: u64 = 2;
const GRID_ID: u64 = 3;
/// Button ids start here; there are sixteen of them in a 4x4 grid.
const KEY_BASE_ID: u64 = 100;
/// Row container ids, one per grid row.
const ROW_BASE_ID: u64 = 10;

/// The sixteen keys, left to right, top to bottom. A calculator layout every
/// person already knows, which is the point: it should look like one.
const KEYS: [&str; 16] = [
    "C", "+/-", "%", "/", //
    "7", "8", "9", "x", //
    "4", "5", "6", "-", //
    "1", "2", "3", "+",
];
/// The bottom row: a wide zero, a dot, and equals.
const BOTTOM_KEYS: [&str; 3] = ["0", ".", "="];
const BOTTOM_BASE_ID: u64 = 200;
const BOTTOM_ROW_ID: u64 = 20;

const WIDTH: u32 = 300;
const HEIGHT: u32 = 440;

/// Automation budget: a `quick` run exits fast for the screenshot path.
const QUICK_ROUNDS: u32 = 30;
const MAX_ROUNDS: u32 = 1200;
const ROUND_MILLIS: u32 = 40;

struct Component;

/// Calculator state: the whole machine is a running total, a pending operator,
/// and the number currently being typed. No heap, no growth, no panic paths.
struct Calc {
    /// The value already accumulated, applied when the next operator arrives.
    acc: f64,
    /// The operator waiting for its right-hand side: b'+', b'-', b'x', b'/'.
    pending: u8,
    /// The number being entered right now, as a value and its decimal state.
    entry: f64,
    /// Digits after the decimal point, so "1.05" reads back exactly.
    frac_digits: u32,
    /// True once a dot was pressed, so more digits go after the point.
    in_fraction: bool,
    /// True right after an operator or equals: the next digit starts fresh.
    fresh: bool,
    /// True when the last action was equals, so an operator chains from acc.
    just_evaluated: bool,
}

impl Calc {
    const NEW: Self = Self {
        acc: 0.0,
        pending: 0,
        entry: 0.0,
        frac_digits: 0,
        in_fraction: false,
        fresh: true,
        just_evaluated: false,
    };

    fn press_digit(&mut self, digit: u8) {
        if self.fresh {
            self.entry = 0.0;
            self.frac_digits = 0;
            self.in_fraction = false;
            self.fresh = false;
            self.just_evaluated = false;
        }
        let value = f64::from(digit);
        if self.in_fraction {
            self.frac_digits += 1;
            self.entry += value * pow10_neg(self.frac_digits);
        } else {
            self.entry = self.entry * 10.0 + value;
        }
    }

    fn press_dot(&mut self) {
        if self.fresh {
            self.entry = 0.0;
            self.frac_digits = 0;
            self.fresh = false;
            self.just_evaluated = false;
        }
        self.in_fraction = true;
    }

    fn press_operator(&mut self, op: u8) {
        // Chain: if an operator is already pending, evaluate it first so
        // "2 + 3 + 4" folds left the way a person expects.
        if self.pending != 0 && !self.fresh {
            self.evaluate();
        } else {
            self.acc = self.entry;
        }
        self.pending = op;
        self.fresh = true;
        self.just_evaluated = false;
    }

    fn evaluate(&mut self) {
        let result = match self.pending {
            b'+' => self.acc + self.entry,
            b'-' => self.acc - self.entry,
            b'x' => self.acc * self.entry,
            b'/' => {
                // No trap on divide by zero: a calculator shows a value, not a
                // crash. Zero-divide yields zero here, which reads as "cleared".
                if self.entry == 0.0 {
                    0.0
                } else {
                    self.acc / self.entry
                }
            }
            _ => self.entry,
        };
        self.acc = result;
        self.entry = result;
        self.pending = 0;
        self.fresh = true;
        self.in_fraction = false;
        self.frac_digits = 0;
        self.just_evaluated = true;
    }

    fn clear(&mut self) {
        *self = Self::NEW;
    }

    fn negate(&mut self) {
        self.entry = -self.entry;
    }

    fn percent(&mut self) {
        self.entry *= 0.01;
    }

    /// The number currently shown: the live entry, or the accumulator right
    /// after equals.
    fn shown(&self) -> f64 {
        if self.just_evaluated {
            self.acc
        } else {
            self.entry
        }
    }
}

/// 10^-n for small n, without pulling in `powi`'s machinery.
fn pow10_neg(n: u32) -> f64 {
    let mut value = 1.0_f64;
    let mut i = 0;
    while i < n {
        value *= 0.1;
        i += 1;
    }
    value
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH,
            height: HEIGHT,
        };
        let Ok(win) = window::create("Calculator", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }

        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &display("0")).is_err()
            || tree::upsert_node(win, &grid()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }
        // Four rows of four buttons.
        let mut row = 0usize;
        while row < 4 {
            if tree::upsert_node(win, &grid_row(row)).is_err() {
                let _ = window::close(win);
                return 32;
            }
            let mut col = 0usize;
            while col < 4 {
                let index = row * 4 + col;
                if tree::upsert_node(win, &key_button(index)).is_err() {
                    let _ = window::close(win);
                    return 32;
                }
                col += 1;
            }
            row += 1;
        }
        // The bottom row: 0, dot, equals.
        if tree::upsert_node(win, &bottom_row()).is_err() {
            let _ = window::close(win);
            return 32;
        }
        let mut b = 0usize;
        while b < BOTTOM_KEYS.len() {
            if tree::upsert_node(win, &bottom_button(b)).is_err() {
                let _ = window::close(win);
                return 32;
            }
            b += 1;
        }

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");
        let rounds = if quick { QUICK_ROUNDS } else { MAX_ROUNDS };

        let mut calc = Calc::NEW;
        // Seed a visible sum so a screenshot shows a real number, not "0":
        // 12 + 34 = 46. This runs only in quick mode so an interactive user
        // starts from a clean slate.
        if quick {
            calc.press_digit(1);
            calc.press_digit(2);
            calc.press_operator(b'+');
            calc.press_digit(3);
            calc.press_digit(4);
            calc.evaluate();
            let _ = tree::upsert_node(win, &display_number(calc.shown()));
        }

        for _ in 0..rounds {
            match events::wait(Some(ROUND_MILLIS)) {
                Some(types::Event::Pointer(pointer)) if pointer.pressed => {
                    if let Some(id) = pointer.widget {
                        apply_key(&mut calc, id);
                        let _ = tree::upsert_node(win, &display_number(calc.shown()));
                    }
                }
                Some(types::Event::CloseRequested(id)) if id == win => break,
                _ => {}
            }
        }

        // Report the final value so a script can assert the arithmetic, not
        // just photograph it.
        let out = stdio::stdout();
        let _ = out.write(b"result:");
        let mut buf = [0u8; 32];
        let text = format_number(calc.shown(), &mut buf);
        let _ = out.write(text);
        let _ = out.write(b"\n");

        let _ = window::close(win);
        0
    }
}

/// Map a pressed widget id to a calculator action.
fn apply_key(calc: &mut Calc, id: u64) {
    if id >= KEY_BASE_ID && id < KEY_BASE_ID + KEYS.len() as u64 {
        let index = (id - KEY_BASE_ID) as usize;
        let label = KEYS.get(index).copied().unwrap_or("");
        apply_label(calc, label);
    } else if id >= BOTTOM_BASE_ID && id < BOTTOM_BASE_ID + BOTTOM_KEYS.len() as u64 {
        let index = (id - BOTTOM_BASE_ID) as usize;
        let label = BOTTOM_KEYS.get(index).copied().unwrap_or("");
        apply_label(calc, label);
    }
}

fn apply_label(calc: &mut Calc, label: &str) {
    match label.as_bytes() {
        b"C" => calc.clear(),
        b"+/-" => calc.negate(),
        b"%" => calc.percent(),
        b"/" => calc.press_operator(b'/'),
        b"x" => calc.press_operator(b'x'),
        b"-" => calc.press_operator(b'-'),
        b"+" => calc.press_operator(b'+'),
        b"=" => calc.evaluate(),
        b"." => calc.press_dot(),
        [d] if d.is_ascii_digit() => calc.press_digit(d - b'0'),
        _ => {}
    }
}

// ----- widget builders -----

fn stack_root() -> types::WidgetNode {
    node(ROOT_ID, None, types::WidgetKind::Stack, None, 12.0)
}

fn display(text: &str) -> types::WidgetNode {
    let mut n = node(
        DISPLAY_ID,
        Some(ROOT_ID),
        types::WidgetKind::TextField,
        Some(text),
        0.0,
    );
    n.style.height = Some(64.0);
    n.role = Some(pure_string("status"));
    n
}

fn display_number(value: f64) -> types::WidgetNode {
    let mut buf = [0u8; 32];
    let text = format_number(value, &mut buf);
    let mut n = display("");
    n.label = Some(pure_string_from_bytes(text));
    n
}

fn grid() -> types::WidgetNode {
    let mut n = node(GRID_ID, Some(ROOT_ID), types::WidgetKind::Stack, None, 0.0);
    n.style.grow = 1.0;
    n
}

fn grid_row(row: usize) -> types::WidgetNode {
    // A row is a horizontal Stack. Krate lays a Stack as a column, so a row of
    // buttons needs Row direction, which the layout derives from a non-Stack
    // container. A Grid with four fixed children stays on one line, so use it.
    let mut n = node(
        ROW_BASE_ID + row as u64,
        Some(GRID_ID),
        types::WidgetKind::Grid,
        None,
        0.0,
    );
    n.style.grow = 1.0;
    n
}

fn key_button(index: usize) -> types::WidgetNode {
    let label = KEYS.get(index).copied().unwrap_or("");
    let row = index / 4;
    let mut n = node(
        KEY_BASE_ID + index as u64,
        Some(ROW_BASE_ID + row as u64),
        types::WidgetKind::Button,
        Some(label),
        0.0,
    );
    n.style.width = Some(60.0);
    n.style.height = Some(60.0);
    n.role = Some(pure_string("button"));
    n
}

fn bottom_row() -> types::WidgetNode {
    let mut n = node(
        BOTTOM_ROW_ID,
        Some(GRID_ID),
        types::WidgetKind::Grid,
        None,
        0.0,
    );
    n.style.grow = 1.0;
    n
}

fn bottom_button(index: usize) -> types::WidgetNode {
    let label = BOTTOM_KEYS.get(index).copied().unwrap_or("");
    let mut n = node(
        BOTTOM_BASE_ID + index as u64,
        Some(BOTTOM_ROW_ID),
        types::WidgetKind::Button,
        Some(label),
        0.0,
    );
    // The zero is a double-wide key, like every calculator.
    n.style.width = Some(if index == 0 { 128.0 } else { 60.0 });
    n.style.height = Some(60.0);
    n.role = Some(pure_string("button"));
    n
}

/// Shared node builder: the fields every widget sets the same way.
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

// ----- number formatting, panic-free -----

/// Format a calculator value into `buf`, returning the written slice.
///
/// Whole numbers show without a decimal; fractional ones show up to a few
/// places with trailing zeros trimmed. Hand-rolled so no `format!` and no
/// float-to-string machinery (and its panic paths) enter the component.
fn format_number(value: f64, buf: &mut [u8; 32]) -> &[u8] {
    let mut pos = 0usize;
    let mut v = value;
    if v.is_nan() {
        return b"error";
    }
    if v < 0.0 {
        push(buf, &mut pos, b'-');
        v = -v;
    }
    // Round to six decimal places to hide binary-float noise like 0.1+0.2.
    let scaled = (v * 1_000_000.0 + 0.5) as u64;
    let whole = scaled / 1_000_000;
    let frac = scaled % 1_000_000;

    push_u64(buf, &mut pos, whole);

    if frac != 0 {
        push(buf, &mut pos, b'.');
        // Six fixed digits, then trim trailing zeros.
        let mut divisor = 100_000u64;
        let mut f = frac;
        let frac_start = pos;
        while divisor > 0 {
            let digit = (f / divisor) as u8;
            push(buf, &mut pos, b'0' + digit);
            f %= divisor;
            divisor /= 10;
        }
        // Trim trailing zeros, but keep at least one fractional digit.
        while pos > frac_start + 1 && buf.get(pos - 1) == Some(&b'0') {
            pos -= 1;
        }
    }
    buf.get(..pos).unwrap_or(b"0")
}

fn push(buf: &mut [u8; 32], pos: &mut usize, byte: u8) {
    if let Some(slot) = buf.get_mut(*pos) {
        *slot = byte;
        *pos += 1;
    }
}

fn push_u64(buf: &mut [u8; 32], pos: &mut usize, value: u64) {
    // Write digits into a scratch, then copy reversed.
    let mut scratch = [0u8; 20];
    let mut n = value;
    let mut count = 0usize;
    if n == 0 {
        push(buf, pos, b'0');
        return;
    }
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
            push(buf, pos, *digit);
        }
    }
}

// ----- string helpers that avoid std's OOM handler (WASI-import free) -----

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
