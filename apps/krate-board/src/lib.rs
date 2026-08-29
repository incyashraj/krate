//! Board -- a kanban board drawn entirely on a canvas.
//!
//! The hero layout: three columns (To do, In progress, Done) of task cards
//! with colored label chips, assignee dots and comment counts, under a
//! header with the board name and the team's avatar stack. Clicking a card
//! in "To do" or "In progress" advances it one column; positions persist.
//! A seeded demo board; honest about being a demo, real about what it does.
//!
//! `#![no_std]` keeps it `krate:*`-only: the SDK owns the allocator and a
//! trapping panic handler. Fixed-size state, no `format!`, no panicking index.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;

extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::string::String;
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::kv as store_kv;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 1080.0;
const HEIGHT: f32 = 700.0;
const MARGIN: f32 = 32.0;
const COL_GAP: f32 = 20.0;
const COL_W: f32 = (WIDTH - 2.0 * MARGIN - 2.0 * COL_GAP) / 3.0;
const COLS_Y: f32 = 120.0;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;
const MAX_IDLE_ROUNDS: u32 = 300;

const STATE_KEY: &str = "card-columns";

// ---- palette (the shared canvas design system) ------------------------------

const BG_TOP: gfx::Color = rgb(0.043, 0.055, 0.082);
const BG_BOT: gfx::Color = rgb(0.063, 0.078, 0.114);
const WELL: gfx::Color = rgb(0.067, 0.084, 0.122);
const CARD: gfx::Color = rgb(0.098, 0.120, 0.169);
const CARD_EDGE: gfx::Color = rgb(0.137, 0.165, 0.220);
const INK: gfx::Color = rgb(0.949, 0.961, 0.980);
const INK_DIM: gfx::Color = rgb(0.604, 0.647, 0.710);
const INK_QUIET: gfx::Color = rgb(0.365, 0.408, 0.471);
const GREEN: gfx::Color = rgb(0.239, 0.839, 0.549);

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

// Label chips: text color + a pre-flattened dark tint behind it.
const LABELS: [(&str, gfx::Color, gfx::Color); 4] = [
    ("design", rgb(0.706, 0.549, 1.0), rgb(0.133, 0.116, 0.204)),
    ("build", rgb(0.361, 0.620, 1.0), rgb(0.090, 0.135, 0.224)),
    ("bug", rgb(1.0, 0.498, 0.582), rgb(0.196, 0.106, 0.145)),
    ("launch", rgb(1.0, 0.761, 0.294), rgb(0.204, 0.165, 0.098)),
];

// Assignee avatar colors.
const PEOPLE: [gfx::Color; 4] = [
    rgb(0.298, 0.553, 1.0),
    rgb(0.706, 0.549, 1.0),
    rgb(0.239, 0.839, 0.549),
    rgb(1.0, 0.624, 0.263),
];

/// One task card: title, label index, assignee index, comment count.
struct Task {
    title: &'static str,
    label: usize,
    who: usize,
    notes: u32,
}

const TASK_COUNT: usize = 9;
const TASKS: [Task; TASK_COUNT] = [
    Task { title: "Onboarding flow, pass 2", label: 0, who: 1, notes: 4 },
    Task { title: "Empty states for search", label: 0, who: 0, notes: 2 },
    Task { title: "Import from CSV", label: 1, who: 2, notes: 7 },
    Task { title: "Keyboard shortcuts", label: 1, who: 3, notes: 1 },
    Task { title: "Fix drag ghost offset", label: 2, who: 0, notes: 3 },
    Task { title: "Pricing page copy", label: 3, who: 1, notes: 5 },
    Task { title: "Dark mode audit", label: 0, who: 2, notes: 2 },
    Task { title: "Beta invite emails", label: 3, who: 3, notes: 6 },
    Task { title: "Launch checklist", label: 3, who: 0, notes: 9 },
];

/// Default column of each task: 0 to do, 1 in progress, 2 done.
const DEFAULT_COL: [u8; TASK_COUNT] = [0, 0, 0, 1, 1, 1, 2, 2, 2];

const COL_NAMES: [&str; 3] = ["To do", "In progress", "Done"];

// ---- tiny drawing + text helpers --------------------------------------------

fn fill(canvas: u64, x: f32, y: f32, w: f32, h: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_rect(canvas, gfx::Rect { x, y, width: w, height: h }, c)
}

fn disc(canvas: u64, x: f32, y: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_circle(canvas, gfx::Point { x, y }, r, c)
}

fn text(canvas: u64, s: &str, x: f32, y: f32, size: f32, c: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, s, gfx::Point { x, y }, size, c);
}

fn est_width(canvas: u64, s: &str, size: f32) -> f32 {
    match canvas2d::measure_text(canvas, s, size) {
        Ok(m) => m.width,
        Err(_) => 0.0,
    }
}

fn text_right(canvas: u64, s: &str, right: f32, y: f32, size: f32, c: gfx::Color) {
    text(canvas, s, right - est_width(canvas, s, size), y, size, c);
}

fn rounded(canvas: u64, x: f32, y: f32, w: f32, h: f32, r: f32, c: gfx::Color) -> Result<(), gfx::GfxError> {
    canvas2d::fill_round_rect(
        canvas,
        gfx::Rect { x, y, width: w, height: h },
        gfx::CornerRadii { top_left: r, top_right: r, bottom_right: r, bottom_left: r },
        c,
    )
}

fn pure_string(text: &str) -> String {
    let len = text.len();
    if len == 0 {
        return String::new();
    }
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(len, 1);
        let ptr = alloc::alloc::alloc(layout);
        if ptr.is_null() {
            core::arch::wasm32::unreachable()
        }
        core::ptr::copy_nonoverlapping(text.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

fn num<'b>(buf: &'b mut [u8; 12], mut n: u32) -> &'b str {
    let mut tmp = [0u8; 12];
    let mut i = 0usize;
    loop {
        if let Some(slot) = tmp.get_mut(i) {
            *slot = b'0' + (n % 10) as u8;
            i += 1;
        }
        n /= 10;
        if n == 0 {
            break;
        }
    }
    let mut out = 0usize;
    while i > 0 {
        i -= 1;
        if let Some(slot) = buf.get_mut(out) {
            *slot = tmp[i.min(11)];
            out += 1;
        }
    }
    core::str::from_utf8(buf.get(..out).unwrap_or(b"0")).unwrap_or("0")
}

// ---- drawing -----------------------------------------------------------------

const CARD_H: f32 = 96.0;
const CARD_STEP: f32 = CARD_H + 14.0;
const CARDS_TOP: f32 = COLS_Y + 58.0;

fn draw(canvas: u64, cols: [u8; TASK_COUNT]) -> Result<(), gfx::GfxError> {
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect { x: 0.0, y: 0.0, width: WIDTH, height: HEIGHT },
        BG_TOP,
        BG_BOT,
    )?;

    // Header: board name left, avatar stack right.
    text(canvas, "Launch board", MARGIN, 64.0, 30.0, INK);
    text(canvas, "Sprint 12 of 14", MARGIN, 88.0, 14.0, INK_QUIET);
    let ax = WIDTH - MARGIN - 96.0;
    for i in 0..4 {
        let x = ax + i as f32 * 26.0;
        disc(canvas, x, 56.0, 15.0, BG_TOP)?;
        disc(canvas, x, 56.0, 13.0, PEOPLE[i.min(3)])?;
    }

    for c in 0..3usize {
        draw_column(canvas, c, cols)?;
    }
    canvas2d::present(canvas)
}

fn draw_column(canvas: u64, col: usize, cols: [u8; TASK_COUNT]) -> Result<(), gfx::GfxError> {
    let x = MARGIN + col as f32 * (COL_W + COL_GAP);
    let count = cols.iter().filter(|c| **c as usize == col).count() as u32;

    // The column well.
    rounded(canvas, x, COLS_Y, COL_W, HEIGHT - COLS_Y - MARGIN, 18.0, WELL)?;

    // Column header: name + count pill; "Done" gets a green dot.
    let name = COL_NAMES[col.min(2)];
    if col == 2 {
        disc(canvas, x + 26.0, COLS_Y + 31.0, 5.0, GREEN)?;
        text(canvas, name, x + 40.0, COLS_Y + 38.0, 17.0, INK);
    } else {
        text(canvas, name, x + 20.0, COLS_Y + 38.0, 17.0, INK);
    }
    let mut buf = [0u8; 12];
    let n = num(&mut buf, count);
    let pw = est_width(canvas, n, 13.0) + 18.0;
    rounded(canvas, x + COL_W - 20.0 - pw, COLS_Y + 20.0, pw, 24.0, 12.0, CARD)?;
    text(canvas, n, x + COL_W - 20.0 - pw + 9.0, COLS_Y + 37.0, 13.0, INK_DIM);

    // Cards, in task order.
    let mut slot = 0u32;
    for i in 0..TASK_COUNT {
        if cols[i.min(TASK_COUNT - 1)] as usize != col {
            continue;
        }
        let t = &TASKS[i.min(TASK_COUNT - 1)];
        let y = CARDS_TOP + slot as f32 * CARD_STEP;
        slot += 1;
        canvas2d::drop_shadow_round_rect(
            canvas,
            gfx::Rect { x: x + 12.0, y: y + 3.0, width: COL_W - 24.0, height: CARD_H },
            gfx::CornerRadii { top_left: 12.0, top_right: 12.0, bottom_right: 12.0, bottom_left: 12.0 },
            8.0,
            gfx::Color { r: 0.0, g: 0.0, b: 0.0, a: 0.25 },
        )?;
        rounded(canvas, x + 12.0, y, COL_W - 24.0, CARD_H, 12.0, CARD)?;

        // Label chip.
        let (lname, lc, lbg) = LABELS[t.label.min(3)];
        let lw = est_width(canvas, lname, 12.0) + 20.0;
        rounded(canvas, x + 26.0, y + 14.0, lw, 22.0, 11.0, lbg)?;
        text(canvas, lname, x + 36.0, y + 30.0, 12.0, lc);

        // Title.
        text(canvas, t.title, x + 26.0, y + 58.0, 15.5, INK);

        // Meta row: assignee dot + comments right.
        disc(canvas, x + 33.0, y + 78.0, 8.0, PEOPLE[t.who.min(3)])?;
        let mut nbuf = [0u8; 12];
        let notes = num(&mut nbuf, t.notes);
        text_right(canvas, notes, x + COL_W - 30.0, y + 83.0, 13.0, INK_QUIET);
        disc(canvas, x + COL_W - 30.0 - est_width(canvas, notes, 13.0) - 12.0, y + 79.0, 4.0, INK_QUIET)?;

        // Done column: strike the mood green.
        if col == 2 {
            disc(canvas, x + COL_W - 30.0 - est_width(canvas, notes, 13.0) - 12.0, y + 79.0, 4.0, GREEN)?;
        }
    }
    Ok(())
}

// ---- ui tree -----------------------------------------------------------------

fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
        style: types::Style { width: None, height: None, grow: 1.0, padding: 0.0 },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn canvas_node() -> types::WidgetNode {
    types::WidgetNode {
        id: CANVAS_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Canvas,
        label: None,
        role: Some(pure_string("canvas")),
        style: types::Style { width: None, height: None, grow: 1.0, padding: 0.0 },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

// ---- persistence -------------------------------------------------------------

fn load_cols() -> [u8; TASK_COUNT] {
    let mut cols = DEFAULT_COL;
    if let Ok(Some(bytes)) = store_kv::get(STATE_KEY) {
        for i in 0..TASK_COUNT {
            if let Some(b) = bytes.get(i) {
                if *b <= 2 {
                    cols[i] = *b;
                }
            }
        }
    }
    cols
}

fn save_cols(cols: [u8; TASK_COUNT]) {
    let _ = store_kv::set(STATE_KEY, &cols);
}

/// Which task card a click landed on, if any.
fn hit_card(x: f32, y: f32, cols: [u8; TASK_COUNT]) -> Option<usize> {
    for col in 0..3usize {
        let cx = MARGIN + col as f32 * (COL_W + COL_GAP);
        if x < cx + 12.0 || x > cx + COL_W - 12.0 {
            continue;
        }
        let mut slot = 0u32;
        for i in 0..TASK_COUNT {
            if cols[i.min(TASK_COUNT - 1)] as usize != col {
                continue;
            }
            let cy = CARDS_TOP + slot as f32 * CARD_STEP;
            slot += 1;
            if y >= cy && y < cy + CARD_H {
                return Some(i);
            }
        }
    }
    None
}

// ---- app ---------------------------------------------------------------------

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Board", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err() || tree::upsert_node(win, &canvas_node()).is_err() {
            let _ = window::close(win);
            return 32;
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(c) => c,
            Err(_) => {
                let _ = window::close(win);
                return 33;
            }
        };
        let _ = canvas2d::set_design_size(canvas, gfx::Size { width: WIDTH, height: HEIGHT });

        let mut cols = load_cols();

        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");

        if quick {
            let _ = draw(canvas, DEFAULT_COL);
            let out = stdio::stdout();
            let _ = out.write(b"board:ok\n");
            let _ = out.flush();
            let _ = window::close(win);
            return 0;
        }

        let _ = draw(canvas, cols);

        let mut idle = 0u32;
        for _ in 0..MAX_WAIT_ROUNDS {
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::Pointer(p)) if p.pressed => {
                    idle = 0;
                    if let Some(i) = hit_card(p.x, p.y, cols) {
                        // Advance one column; Done cards stay done.
                        if let Some(slot) = cols.get_mut(i) {
                            if *slot < 2 {
                                *slot += 1;
                                save_cols(cols);
                                let _ = draw(canvas, cols);
                            }
                        }
                    }
                }
                Some(types::Event::CloseRequested(id)) if id == win => break,
                Some(_) => idle = 0,
                None => {
                    idle += 1;
                    if quick && idle > MAX_IDLE_ROUNDS * 20 {
                        break;
                    }
                }
            }
        }

        let _ = window::close(win);
        let out = stdio::stdout();
        let _ = out.write(b"board:ok\n");
        let _ = out.flush();
        0
    }
}

bindings::export!(Component with_types_in bindings);
