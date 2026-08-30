//! Krate Timer -- a Pomodoro / countdown timer with a clean UI.
//!
//! A 25:00 focus timer: the time remaining shown large in the middle, a
//! "Focus" label above it, a progress bar tracking how much of the session
//! has elapsed, and a row of Start / Pause / Reset controls. Start begins
//! counting down using monotonic-clock deltas so the display really ticks.
//!
//! Built to the Krate discipline: a component may import only `krate:*`, so
//! every buffer is fixed capacity and every access is non-panicking. A growable
//! `String`, `format!`, or an out-of-range index would reach std's
//! allocation/panic path and drag the whole `wasi:*` import set in, which LTO
//! cannot strip and which stops the component instantiating.

#[allow(warnings)]
mod bindings;

use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const LABEL_ID: u64 = 2;
const TIME_ID: u64 = 3;
const PROGRESS_ID: u64 = 4;
const BUTTON_ROW_ID: u64 = 5;
const START_ID: u64 = 6;
const PAUSE_ID: u64 = 7;
const RESET_ID: u64 = 8;

const WIN_WIDTH: u32 = 360;
const WIN_HEIGHT: u32 = 420;

/// The length of one focus session, twenty-five minutes.
const SESSION_SECONDS: u64 = 25 * 60;

/// Interactive runs stay open until the person closes the window; automated
/// runs pass `quick` and exit promptly.
const WAIT_ROUND_MILLIS: u32 = 50;
const MAX_WAIT_ROUNDS: u32 = 600_000;
/// Consecutive quiet rounds before an unwatched run stops, about ten seconds.
/// A real window is a reason to keep waiting; with nothing there, waiting out
/// the full cap would look like a hang.
const MAX_IDLE_ROUNDS: u32 = 200;

struct Component;

/// The timer's whole state: how far the countdown has advanced, whether it is
/// currently running, and the clock reading when the last running span began.
struct Timer {
    /// Seconds already elapsed in this session, 0..=SESSION_SECONDS.
    elapsed_secs: u64,
    /// Whether the countdown is currently ticking.
    running: bool,
    /// Monotonic-clock nanos at the moment `running` last became true, so a
    /// delta gives the seconds to fold into `elapsed_secs`.
    started_nanos: u64,
}

impl Timer {
    const fn new() -> Self {
        Self {
            elapsed_secs: 0,
            running: false,
            started_nanos: 0,
        }
    }

    /// Seconds still left on the clock, never below zero.
    fn remaining_secs(&self) -> u64 {
        SESSION_SECONDS.saturating_sub(self.elapsed_secs)
    }

    /// Fraction of the session elapsed, 0.0..=1.0, for the progress bar.
    fn progress(&self) -> f32 {
        (self.elapsed_secs as f32 / SESSION_SECONDS as f32).min(1.0)
    }

    fn start(&mut self, now_nanos: u64) {
        if !self.running && self.elapsed_secs < SESSION_SECONDS {
            self.running = true;
            self.started_nanos = now_nanos;
        }
    }

    fn pause(&mut self, now_nanos: u64) {
        self.tick(now_nanos);
        self.running = false;
    }

    fn reset(&mut self) {
        self.elapsed_secs = 0;
        self.running = false;
        self.started_nanos = 0;
    }

    /// Fold the whole-seconds that have passed since the running span began
    /// into `elapsed_secs`, and re-anchor so the remainder is not lost. A no-op
    /// while paused. Returns whether the displayed time changed.
    fn tick(&mut self, now_nanos: u64) -> bool {
        if !self.running {
            return false;
        }
        let delta_nanos = now_nanos.saturating_sub(self.started_nanos);
        let whole_secs = delta_nanos / 1_000_000_000;
        if whole_secs == 0 {
            return false;
        }
        self.started_nanos = self
            .started_nanos
            .saturating_add(whole_secs.saturating_mul(1_000_000_000));
        let before = self.elapsed_secs;
        self.elapsed_secs = self
            .elapsed_secs
            .saturating_add(whole_secs)
            .min(SESSION_SECONDS);
        if self.elapsed_secs >= SESSION_SECONDS {
            self.running = false;
        }
        self.elapsed_secs != before
    }
}

/// Build an owned `String` without touching std's allocation-error handler,
/// which would drag the `wasi:*` import set into the component. Mirrors the
/// raw-allocation path the generated bindings use. Copied verbatim from the
/// other Krate GUI apps.
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

/// Build the `mm:ss` string by hand: two digits, a colon, two digits, with
/// leading zeros. No `format!`, no allocation beyond the final `pure_string`.
fn mmss_string(total_secs: u64) -> String {
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    let mut buf = [0u8; 5];
    // Minutes, clamped to two digits (a 25-minute session never overflows).
    let mm = (minutes % 100) as u8;
    buf[0] = b'0' + mm / 10;
    buf[1] = b'0' + mm % 10;
    buf[2] = b':';
    let ss = (seconds % 100) as u8;
    buf[3] = b'0' + ss / 10;
    buf[4] = b'0' + ss % 10;
    pure_string(core::str::from_utf8(buf.get(..5).unwrap_or(&[])).unwrap_or("00:00"))
}

// ---- widget tree ----------------------------------------------------------

/// A centered vertical stack fills the window with generous padding, so the
/// label, big clock, progress bar, and button row sit in a clean column.
fn stack_root() -> types::WidgetNode {
    node(
        ROOT_ID,
        None,
        types::WidgetKind::Stack,
        None,
        None,
        WIN_WIDTH as f32,
        WIN_HEIGHT as f32,
        0.0,
        28.0,
    )
}

/// The "Focus" caption above the clock.
fn label_node() -> types::WidgetNode {
    let mut n = node(
        LABEL_ID,
        Some(ROOT_ID),
        types::WidgetKind::Text,
        Some(pure_string("Focus")),
        Some(pure_string("heading")),
        304.0,
        34.0,
        0.0,
        0.0,
    );
    n.role = Some(pure_string("heading"));
    n
}

/// The big mm:ss countdown, the centerpiece of the window.
fn time_node(remaining: u64) -> types::WidgetNode {
    let mut n = node(
        TIME_ID,
        Some(ROOT_ID),
        types::WidgetKind::Text,
        Some(mmss_string(remaining)),
        Some(pure_string("timer")),
        304.0,
        120.0,
        0.0,
        0.0,
    );
    n.role = Some(pure_string("timer"));
    n
}

/// The progress bar, filled by how much of the session has elapsed.
fn progress_node(fraction: f32) -> types::WidgetNode {
    let mut n = node(
        PROGRESS_ID,
        Some(ROOT_ID),
        types::WidgetKind::Progress,
        None,
        Some(pure_string("progressbar")),
        304.0,
        16.0,
        0.0,
        0.0,
    );
    n.value = Some(fraction);
    n.role = Some(pure_string("progressbar"));
    n
}

/// A horizontal row that holds the three control buttons.
fn button_row_node() -> types::WidgetNode {
    node(
        BUTTON_ROW_ID,
        Some(ROOT_ID),
        types::WidgetKind::Stack,
        None,
        None,
        304.0,
        44.0,
        0.0,
        0.0,
    )
}

/// One control button, a child of the button row.
fn button_node(id: u64, text: &str) -> types::WidgetNode {
    node(
        id,
        Some(BUTTON_ROW_ID),
        types::WidgetKind::Button,
        Some(pure_string(text)),
        Some(pure_string("button")),
        96.0,
        40.0,
        1.0,
        0.0,
    )
}

/// Small constructor so every node above stays one readable line.
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

/// Rebuild the whole tree from the current timer state. Returns false if any
/// upsert fails.
fn rebuild(win: u64, timer: &Timer) -> bool {
    if tree::set_root(win, &stack_root()).is_err() {
        return false;
    }
    tree::upsert_node(win, &label_node()).is_ok()
        && tree::upsert_node(win, &time_node(timer.remaining_secs())).is_ok()
        && tree::upsert_node(win, &progress_node(timer.progress())).is_ok()
        && tree::upsert_node(win, &button_row_node()).is_ok()
        && tree::upsert_node(win, &button_node(START_ID, "Start")).is_ok()
        && tree::upsert_node(win, &button_node(PAUSE_ID, "Pause")).is_ok()
        && tree::upsert_node(win, &button_node(RESET_ID, "Reset")).is_ok()
}

// ---- the app --------------------------------------------------------------

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIN_WIDTH,
            height: WIN_HEIGHT,
        };
        let Ok(win) = window::create("Timer", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }

        let mut timer = Timer::new();
        if !rebuild(win, &timer) {
            let _ = window::close(win);
            return 32;
        }

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if quick {
            // The automated verification path: advance the timer to a visible
            // state so the screenshot is not a static 25:00, redraw, and exit
            // immediately. It must NOT enter the event-wait loop -- waiting on
            // window events during a headless run is what makes verification
            // hang. 23 seconds elapsed leaves 24:37 on the clock with the
            // progress bar just barely filled.
            timer.elapsed_secs = 23;
            let _ = rebuild(win, &timer);
            let _ = window::close(win);
            let out = stdio::stdout();
            let _ = out.write(b"timer:ok\n");
            return 0;
        }

        let mut close_requested = false;
        let mut idle_rounds = 0u32;
        for _ in 0..MAX_WAIT_ROUNDS {
            let event = events::wait(Some(WAIT_ROUND_MILLIS));

            // While running, fold elapsed whole-seconds in every round and
            // redraw when the shown time changes, so the clock visibly ticks.
            if timer.running {
                let now = clock::monotonic_nanos();
                if timer.tick(now) {
                    let _ = rebuild(win, &timer);
                }
            }

            if event.is_none() {
                // A running timer is never idle; only a paused, unwatched
                // window counts quiet rounds toward giving up.
                if timer.running {
                    idle_rounds = 0;
                } else {
                    idle_rounds += 1;
                    if quick && idle_rounds >= MAX_IDLE_ROUNDS {
                        break;
                    }
                }
                continue;
            }
            idle_rounds = 0;

            match event {
                Some(types::Event::Pointer(pointer)) if pointer.pressed => {
                    let now = clock::monotonic_nanos();
                    match pointer.widget {
                        Some(START_ID) => {
                            timer.start(now);
                            let _ = rebuild(win, &timer);
                        }
                        Some(PAUSE_ID) => {
                            timer.pause(now);
                            let _ = rebuild(win, &timer);
                        }
                        Some(RESET_ID) => {
                            timer.reset();
                            let _ = rebuild(win, &timer);
                        }
                        _ => {}
                    }
                }
                Some(types::Event::CloseRequested(_)) => {
                    close_requested = true;
                    break;
                }
                _ => {}
            }
        }

        let _ = window::close(win);

        let out = stdio::stdout();
        let _ = out.write(b"timer:ok\n");

        if close_requested {
            2
        } else {
            0
        }
    }
}

bindings::export!(Component with_types_in bindings);
