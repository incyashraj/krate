//! Krate Founder OS -- the visual, dependency-aware front end for the founder
//! task ledger.
//!
//! The source-of-truth seed is generated from `Plan/founder-os/tasks.json` at
//! build time. In-app task status changes and newly added tasks persist through
//! `store.kv`, so this packaged `.krate` remains useful when moved away from the
//! repository. The current Krate pointer contract reports presses and releases,
//! not pointer movement, so the app uses click-to-inspect and explicit actions
//! instead of pretending that hover and drag exist.

#![no_std]
#![allow(clippy::needless_range_loop)]

extern crate alloc;
extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::store::kv as store_kv;
use bindings::krate::ui::{events, tree, types, window};
use serde::{Deserialize, Serialize};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const STATE_KEY: &str = "founder-os-state-v1";

const WIDTH: f32 = 1180.0;
const HEIGHT: f32 = 760.0;
const MARGIN: f32 = 24.0;
const GAP: f32 = 14.0;
const COL_W: f32 = (WIDTH - MARGIN * 2.0 - GAP * 2.0) / 3.0;
const BOARD_TOP: f32 = 154.0;
const BOARD_BOTTOM: f32 = 574.0;
const CARD_H: f32 = 84.0;
const CARD_GAP: f32 = 10.0;
const VISIBLE_CARDS: usize = 4;
const DETAIL_TOP: f32 = 590.0;
const DETAIL_H: f32 = 104.0;
const INPUT_TOP: f32 = 708.0;
const INPUT_H: f32 = 40.0;

const MAX_WAIT_ROUNDS: u32 = 600_000;
const WAIT_ROUND_MILLIS: u32 = 33;

const PARKED: u8 = 0;
const BLOCKED: u8 = 1;
const READY: u8 = 2;
const LOCKED: u8 = 3;
const WAITING: u8 = 4;
const DONE: u8 = 5;
const DROPPED: u8 = 6;

const BUILD: u8 = 0;
const COMPANY: u8 = 1;

const BG_TOP: gfx::Color = color(0.039, 0.047, 0.067, 1.0);
const BG_BOTTOM: gfx::Color = color(0.055, 0.067, 0.094, 1.0);
const SURFACE: gfx::Color = color(0.075, 0.090, 0.122, 1.0);
const SURFACE_2: gfx::Color = color(0.094, 0.110, 0.149, 1.0);
const LINE: gfx::Color = color(0.153, 0.176, 0.224, 1.0);
const INK: gfx::Color = color(0.945, 0.957, 0.980, 1.0);
const INK_DIM: gfx::Color = color(0.635, 0.675, 0.741, 1.0);
const INK_QUIET: gfx::Color = color(0.400, 0.447, 0.525, 1.0);
const ACCENT: gfx::Color = color(0.337, 0.565, 1.0, 1.0);
const ACCENT_SOFT: gfx::Color = color(0.104, 0.157, 0.263, 1.0);
const GREEN: gfx::Color = color(0.306, 0.824, 0.565, 1.0);
const GREEN_SOFT: gfx::Color = color(0.082, 0.180, 0.145, 1.0);
const AMBER: gfx::Color = color(0.929, 0.690, 0.278, 1.0);
const AMBER_SOFT: gfx::Color = color(0.196, 0.145, 0.071, 1.0);
const MUTED_BUTTON: gfx::Color = color(0.118, 0.137, 0.180, 1.0);

const fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

struct SeedTask {
    id: &'static str,
    title: &'static str,
    outcome: &'static str,
    category: &'static str,
    lane: u8,
    status: u8,
    deps: &'static [&'static str],
}

include!(concat!(env!("OUT_DIR"), "/tasks_seed.rs"));

#[derive(Clone, Serialize, Deserialize)]
struct Task {
    id: String,
    title: String,
    outcome: String,
    category: String,
    lane: u8,
    status: u8,
    deps: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct SeedStatus {
    id: String,
    status: u8,
}

#[derive(Serialize, Deserialize)]
struct SavedState {
    version: u8,
    seed_statuses: Vec<SeedStatus>,
    custom_tasks: Vec<Task>,
}

/// The first internal build stored statuses by seed-array position. Keep this
/// reader long enough to migrate that state once; version 2 stores stable ids.
#[derive(Deserialize)]
struct SavedStateV1 {
    version: u8,
    seed_statuses: Vec<u8>,
    custom_tasks: Vec<Task>,
}

#[derive(Clone, Copy, PartialEq)]
enum View {
    Board,
    Flow,
}

struct AppState {
    tasks: Vec<Task>,
    seed_count: usize,
    selected: usize,
    scroll: [usize; 3],
    view: View,
    draft: String,
    field_focus: bool,
    new_lane: u8,
    message: String,
}

fn seed_tasks() -> Vec<Task> {
    let mut tasks = Vec::with_capacity(SEED_TASKS.len() + 16);
    for seed in SEED_TASKS {
        tasks.push(Task {
            id: seed.id.to_string(),
            title: seed.title.to_string(),
            outcome: seed.outcome.to_string(),
            category: seed.category.to_string(),
            lane: seed.lane,
            status: seed.status,
            deps: seed.deps.iter().map(|dep| (*dep).to_string()).collect(),
        });
    }
    tasks
}

fn load_state() -> AppState {
    let mut tasks = seed_tasks();
    let seed_count = tasks.len();
    let mut custom_tasks = Vec::new();
    let mut migrated_v1 = false;
    if let Ok(Some(bytes)) = store_kv::get(STATE_KEY) {
        if let Ok(saved) = serde_json::from_slice::<SavedState>(&bytes) {
            if saved.version == 2 {
                for saved_status in saved.seed_statuses {
                    if saved_status.status > DROPPED {
                        continue;
                    }
                    if let Some(task) = tasks.iter_mut().find(|task| task.id == saved_status.id) {
                        task.status = saved_status.status;
                    }
                }
                custom_tasks = saved.custom_tasks;
            }
        } else if let Ok(saved) = serde_json::from_slice::<SavedStateV1>(&bytes) {
            if saved.version == 1 {
                // BLD-055 is the first seed added after v1 shipped. A v1 vector
                // with one fewer entry follows the previous order, so skip the
                // new id rather than applying every later status to its neighbor.
                if saved.seed_statuses.len() + 1 == seed_count {
                    let mut old_index = 0usize;
                    for task in tasks.iter_mut() {
                        if task.id == "BLD-055" {
                            continue;
                        }
                        if let Some(status) = saved.seed_statuses.get(old_index) {
                            if *status <= DROPPED {
                                task.status = *status;
                            }
                        }
                        old_index += 1;
                    }
                } else {
                    for (index, status) in saved.seed_statuses.iter().enumerate() {
                        if let Some(task) = tasks.get_mut(index) {
                            if *status <= DROPPED {
                                task.status = *status;
                            }
                        }
                    }
                }
                custom_tasks = saved.custom_tasks;
                migrated_v1 = true;
            }
        }
    }
    for task in custom_tasks {
        if tasks.len() < 96 {
            tasks.push(task);
        }
    }
    recompute_readiness(&mut tasks);
    let state = AppState {
        tasks,
        seed_count,
        selected: 0,
        scroll: [0, 0, 0],
        view: View::Board,
        draft: String::new(),
        field_focus: false,
        new_lane: COMPANY,
        message: "Select a card to inspect it. Wheel inside a column to see more.".to_string(),
    };
    if migrated_v1 {
        let _ = save_state(&state);
    }
    state
}

fn save_state(state: &AppState) -> bool {
    let mut seed_statuses = Vec::with_capacity(state.seed_count);
    for index in 0..state.seed_count {
        if let Some(task) = state.tasks.get(index) {
            seed_statuses.push(SeedStatus {
                id: task.id.clone(),
                status: task.status,
            });
        }
    }
    let mut custom_tasks = Vec::new();
    for index in state.seed_count..state.tasks.len() {
        if let Some(task) = state.tasks.get(index) {
            custom_tasks.push(task.clone());
        }
    }
    let saved = SavedState {
        version: 2,
        seed_statuses,
        custom_tasks,
    };
    match serde_json::to_vec(&saved) {
        Ok(bytes) => store_kv::set(STATE_KEY, &bytes).is_ok(),
        Err(_) => false,
    }
}

fn task_done(tasks: &[Task], id: &str) -> bool {
    tasks
        .iter()
        .any(|task| task.id == id && task.status == DONE)
}

fn deps_done(tasks: &[Task], index: usize) -> bool {
    let Some(task) = tasks.get(index) else {
        return false;
    };
    task.deps.iter().all(|dep| task_done(tasks, dep))
}

fn recompute_readiness(tasks: &mut [Task]) {
    let mut changes = Vec::with_capacity(tasks.len());
    for index in 0..tasks.len() {
        let Some(task) = tasks.get(index) else {
            continue;
        };
        if task.status == BLOCKED || task.status == READY || task.status == WAITING {
            changes.push((
                index,
                if deps_done(tasks, index) {
                    READY
                } else {
                    BLOCKED
                },
            ));
        }
    }
    for (index, status) in changes {
        if let Some(task) = tasks.get_mut(index) {
            task.status = status;
        }
    }
}

fn status_name(status: u8) -> &'static str {
    match status {
        PARKED => "PARKED",
        BLOCKED => "BLOCKED",
        READY => "READY",
        LOCKED => "LOCKED",
        WAITING => "WAITING",
        DONE => "DONE",
        DROPPED => "DROPPED",
        _ => "UNKNOWN",
    }
}

fn lane_name(lane: u8) -> &'static str {
    if lane == BUILD {
        "build"
    } else {
        "company"
    }
}

fn display_column(status: u8) -> Option<usize> {
    match status {
        LOCKED => Some(0),
        READY => Some(1),
        BLOCKED | WAITING => Some(2),
        _ => None,
    }
}

fn column_name(column: usize) -> &'static str {
    match column {
        0 => "Locked now",
        1 => "Ready queue",
        _ => "Waiting",
    }
}

fn column_count(tasks: &[Task], column: usize) -> usize {
    tasks
        .iter()
        .filter(|task| display_column(task.status) == Some(column))
        .count()
}

fn status_count(tasks: &[Task], status: u8) -> usize {
    tasks.iter().filter(|task| task.status == status).count()
}

fn locked_in_lane(tasks: &[Task], lane: u8) -> bool {
    tasks
        .iter()
        .any(|task| task.status == LOCKED && task.lane == lane)
}

fn lock_selected(state: &mut AppState) {
    let Some(task) = state.tasks.get(state.selected) else {
        return;
    };
    if task.status != READY {
        state.message = "Only a READY task can be locked.".to_string();
        return;
    }
    if locked_in_lane(&state.tasks, task.lane) {
        state.message = if task.lane == BUILD {
            "The Build focus slot is already occupied. Complete or park it first.".to_string()
        } else {
            "The Company focus slot is already occupied. Complete or park it first.".to_string()
        };
        return;
    }
    if let Some(task) = state.tasks.get_mut(state.selected) {
        task.status = LOCKED;
        state.message = "Task locked into the current focus.".to_string();
    }
    let _ = save_state(state);
}

fn complete_selected(state: &mut AppState) {
    let Some(task) = state.tasks.get(state.selected) else {
        return;
    };
    if task.status != LOCKED && task.status != READY {
        state.message =
            "Blocked, parked, and dropped tasks cannot be completed from here.".to_string();
        return;
    }
    if let Some(task) = state.tasks.get_mut(state.selected) {
        task.status = DONE;
    }
    recompute_readiness(&mut state.tasks);
    state.message = "Task completed. Dependent tasks were recalculated.".to_string();
    let _ = save_state(state);
}

fn park_selected(state: &mut AppState) {
    let Some(task) = state.tasks.get(state.selected) else {
        return;
    };
    if task.status == DONE || task.status == DROPPED {
        state.message = "Completed or dropped work stays in its recorded state.".to_string();
        return;
    }
    if let Some(task) = state.tasks.get_mut(state.selected) {
        task.status = PARKED;
    }
    recompute_readiness(&mut state.tasks);
    state.message =
        "Task parked. It is hidden from the active board but remains saved.".to_string();
    let _ = save_state(state);
}

fn add_task(state: &mut AppState, after_selected: bool) {
    let title = state.draft.trim();
    if title.is_empty() {
        state.message = "Type the outcome before adding a task.".to_string();
        state.field_focus = true;
        return;
    }
    if state.tasks.len() >= 96 {
        state.message = "This build has reached its 96-task safety ceiling.".to_string();
        return;
    }
    let custom_number = state.tasks.len().saturating_sub(state.seed_count) + 1;
    let mut id = String::from("NEW-");
    push_padded_number(&mut id, custom_number as u32, 3);
    let mut deps = Vec::new();
    if after_selected {
        if let Some(task) = state.tasks.get(state.selected) {
            deps.push(task.id.clone());
        }
    }
    let status = if deps.iter().all(|dep| task_done(&state.tasks, dep)) {
        READY
    } else {
        BLOCKED
    };
    state.tasks.push(Task {
        id,
        title: title.to_string(),
        outcome: title.to_string(),
        category: "custom".to_string(),
        lane: state.new_lane,
        status,
        deps,
    });
    state.selected = state.tasks.len().saturating_sub(1);
    state.draft.clear();
    state.field_focus = false;
    state.message = if after_selected {
        "Task added after the selected dependency.".to_string()
    } else {
        "Independent task added to the ready queue.".to_string()
    };
    let _ = save_state(state);
}

fn push_padded_number(out: &mut String, value: u32, width: usize) {
    let mut digits = [0u8; 12];
    let slice = number_bytes(value, &mut digits);
    for _ in slice.len()..width {
        out.push('0');
    }
    for byte in slice {
        out.push(*byte as char);
    }
}

fn rounded(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::fill_round_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        gfx::CornerRadii {
            top_left: r,
            top_right: r,
            bottom_right: r,
            bottom_left: r,
        },
        c,
    )
}

fn stroke(
    canvas: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    c: gfx::Color,
) -> Result<(), gfx::GfxError> {
    canvas2d::stroke_round_rect(
        canvas,
        gfx::Rect {
            x,
            y,
            width: w,
            height: h,
        },
        gfx::CornerRadii {
            top_left: r,
            top_right: r,
            bottom_right: r,
            bottom_left: r,
        },
        1.3,
        c,
    )
}

fn text(canvas: u64, value: &str, x: f32, y: f32, size: f32, c: gfx::Color) {
    let _ = canvas2d::draw_text(canvas, value, gfx::Point { x, y }, size, c);
}

fn text_width(canvas: u64, value: &str, size: f32) -> f32 {
    canvas2d::measure_text(canvas, value, size)
        .map(|m| m.width)
        .unwrap_or(0.0)
}

fn draw_button(
    canvas: u64,
    label: &str,
    x: f32,
    y: f32,
    w: f32,
    active: bool,
    primary: bool,
) -> Result<(), gfx::GfxError> {
    let bg = if !active {
        MUTED_BUTTON
    } else if primary {
        ACCENT
    } else {
        SURFACE_2
    };
    rounded(canvas, x, y, w, 34.0, 9.0, bg)?;
    if active && !primary {
        stroke(canvas, x, y, w, 34.0, 9.0, LINE)?;
    }
    let ink = if !active {
        INK_QUIET
    } else if primary {
        BG_TOP
    } else {
        INK
    };
    let tw = text_width(canvas, label, 13.0);
    text(canvas, label, x + (w - tw) * 0.5, y + 22.0, 13.0, ink);
    Ok(())
}

fn draw_pill(canvas: u64, label: &str, x: f32, y: f32, status: u8) -> Result<(), gfx::GfxError> {
    let (bg, ink) = match status {
        LOCKED => (ACCENT_SOFT, ACCENT),
        READY => (GREEN_SOFT, GREEN),
        BLOCKED | WAITING => (AMBER_SOFT, AMBER),
        DONE => (GREEN_SOFT, GREEN),
        _ => (SURFACE_2, INK_DIM),
    };
    let w = text_width(canvas, label, 10.5) + 16.0;
    rounded(canvas, x, y, w, 20.0, 10.0, bg)?;
    text(canvas, label, x + 8.0, y + 14.0, 10.5, ink);
    Ok(())
}

fn truncate<'a>(value: &'a str, max: usize) -> &'a str {
    if value.len() <= max {
        return value;
    }
    let mut end = max;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.get(..end).unwrap_or("")
}

fn draw_wrapped(
    canvas: u64,
    value: &str,
    x: f32,
    y: f32,
    max_chars: usize,
    max_lines: usize,
    size: f32,
    c: gfx::Color,
) {
    let bytes = value.as_bytes();
    let mut start = 0usize;
    let mut line = 0usize;
    while start < bytes.len() && line < max_lines {
        let mut end = (start + max_chars).min(bytes.len());
        while end > start && !value.is_char_boundary(end) {
            end -= 1;
        }
        if end < bytes.len() {
            let slice = value.get(start..end).unwrap_or("");
            if let Some(space) = slice.rfind(' ') {
                if space > max_chars / 2 {
                    end = start + space;
                }
            }
        }
        let slice = value.get(start..end).unwrap_or("").trim();
        text(canvas, slice, x, y + line as f32 * (size + 4.0), size, c);
        start = end;
        while value.as_bytes().get(start).copied() == Some(b' ') {
            start += 1;
        }
        line += 1;
    }
}

fn draw_header(canvas: u64, state: &AppState) -> Result<(), gfx::GfxError> {
    text(canvas, "Krate Founder OS", MARGIN, 42.0, 26.0, INK);
    draw_wrapped(canvas, ROOT_GOAL, MARGIN, 68.0, 106, 2, 12.5, INK_DIM);

    let locked = status_count(&state.tasks, LOCKED);
    let ready = status_count(&state.tasks, READY);
    let blocked = status_count(&state.tasks, BLOCKED) + status_count(&state.tasks, WAITING);
    let done = status_count(&state.tasks, DONE);
    let mut label = [0u8; 48];
    let items = [
        ("locked", locked, ACCENT_SOFT, ACCENT),
        ("ready", ready, GREEN_SOFT, GREEN),
        ("waiting", blocked, AMBER_SOFT, AMBER),
        ("done", done, SURFACE_2, INK_DIM),
    ];
    let mut right = WIDTH - MARGIN;
    for (name, count, bg, ink) in items.iter().rev() {
        let content = count_label(*count as u32, name, &mut label);
        let value = core::str::from_utf8(content).unwrap_or("");
        let w = text_width(canvas, value, 11.5) + 18.0;
        right -= w;
        rounded(canvas, right, 24.0, w, 25.0, 12.5, *bg)?;
        text(canvas, value, right + 9.0, 41.0, 11.5, *ink);
        right -= 7.0;
    }

    draw_button(
        canvas,
        "Board",
        MARGIN,
        112.0,
        76.0,
        true,
        state.view == View::Board,
    )?;
    draw_button(
        canvas,
        "Flow",
        MARGIN + 84.0,
        112.0,
        70.0,
        true,
        state.view == View::Flow,
    )?;
    text(
        canvas,
        "Click a card for details. Status comes from dependencies, not where a card is drawn.",
        MARGIN + 174.0,
        134.0,
        12.0,
        INK_QUIET,
    );
    Ok(())
}

fn draw_board(canvas: u64, state: &AppState) -> Result<(), gfx::GfxError> {
    for column in 0..3usize {
        let x = MARGIN + column as f32 * (COL_W + GAP);
        rounded(
            canvas,
            x,
            BOARD_TOP,
            COL_W,
            BOARD_BOTTOM - BOARD_TOP,
            16.0,
            SURFACE,
        )?;
        text(
            canvas,
            column_name(column),
            x + 16.0,
            BOARD_TOP + 30.0,
            16.0,
            INK,
        );
        let count = column_count(&state.tasks, column);
        let offset = state.scroll[column].min(count.saturating_sub(VISIBLE_CARDS));
        let mut header_buf = [0u8; 48];
        let header_bytes = if count > VISIBLE_CARDS {
            range_label(
                offset + 1,
                (offset + VISIBLE_CARDS).min(count),
                count,
                &mut header_buf,
            )
        } else {
            number_bytes(count as u32, &mut header_buf)
        };
        let header_text = core::str::from_utf8(header_bytes).unwrap_or("0");
        let header_width = text_width(canvas, header_text, 11.5);
        text(
            canvas,
            header_text,
            x + COL_W - 16.0 - header_width,
            BOARD_TOP + 29.0,
            11.5,
            INK_DIM,
        );

        let mut ordinal = 0usize;
        let mut slot = 0usize;
        for (index, task) in state.tasks.iter().enumerate() {
            if display_column(task.status) != Some(column) {
                continue;
            }
            if ordinal < offset {
                ordinal += 1;
                continue;
            }
            if slot >= VISIBLE_CARDS {
                break;
            }
            let y = BOARD_TOP + 48.0 + slot as f32 * (CARD_H + CARD_GAP);
            draw_task_card(
                canvas,
                task,
                index == state.selected,
                x + 10.0,
                y,
                COL_W - 20.0,
            )?;
            ordinal += 1;
            slot += 1;
        }
    }
    Ok(())
}

fn draw_task_card(
    canvas: u64,
    task: &Task,
    selected: bool,
    x: f32,
    y: f32,
    w: f32,
) -> Result<(), gfx::GfxError> {
    let bg = if selected { ACCENT_SOFT } else { SURFACE_2 };
    rounded(canvas, x, y, w, CARD_H, 12.0, bg)?;
    stroke(
        canvas,
        x,
        y,
        w,
        CARD_H,
        12.0,
        if selected { ACCENT } else { LINE },
    )?;
    text(canvas, &task.id, x + 12.0, y + 18.0, 10.5, INK_DIM);
    let pill_w = text_width(canvas, status_name(task.status), 10.5) + 16.0;
    draw_pill(
        canvas,
        status_name(task.status),
        x + w - pill_w - 10.0,
        y + 8.0,
        task.status,
    )?;
    draw_wrapped(canvas, &task.title, x + 12.0, y + 40.0, 40, 2, 13.5, INK);
    let mut footer = String::new();
    footer.push_str(lane_name(task.lane));
    footer.push_str(" · ");
    footer.push_str(&task.category);
    if !task.deps.is_empty() {
        footer.push_str(" · needs ");
        footer.push_str(&task.deps[0]);
        if task.deps.len() > 1 {
            footer.push_str(" +");
            let mut b = [0u8; 12];
            let digits = number_bytes((task.deps.len() - 1) as u32, &mut b);
            footer.push_str(core::str::from_utf8(digits).unwrap_or(""));
        }
    }
    text(
        canvas,
        truncate(&footer, 48),
        x + 12.0,
        y + CARD_H - 10.0,
        10.5,
        INK_QUIET,
    );
    Ok(())
}

fn draw_flow(canvas: u64, state: &AppState) -> Result<(), gfx::GfxError> {
    rounded(
        canvas,
        MARGIN,
        BOARD_TOP,
        WIDTH - MARGIN * 2.0,
        BOARD_BOTTOM - BOARD_TOP,
        16.0,
        SURFACE,
    )?;
    let Some(task) = state.tasks.get(state.selected) else {
        return Ok(());
    };
    text(
        canvas,
        "Dependency view",
        MARGIN + 18.0,
        BOARD_TOP + 30.0,
        16.0,
        INK,
    );
    text(
        canvas,
        "Direct prerequisites",
        MARGIN + 24.0,
        BOARD_TOP + 70.0,
        12.0,
        INK_DIM,
    );
    text(
        canvas,
        "Selected task",
        432.0,
        BOARD_TOP + 70.0,
        12.0,
        INK_DIM,
    );
    text(
        canvas,
        "Work this unlocks",
        812.0,
        BOARD_TOP + 70.0,
        12.0,
        INK_DIM,
    );

    if task.deps.is_empty() {
        draw_flow_node(
            canvas,
            "No prerequisite",
            "This task can start from the ledger itself.",
            MARGIN + 24.0,
            BOARD_TOP + 94.0,
            320.0,
            false,
        )?;
    } else {
        for (slot, dep) in task.deps.iter().take(3).enumerate() {
            let title = state
                .tasks
                .iter()
                .find(|candidate| candidate.id == *dep)
                .map(|t| t.title.as_str())
                .unwrap_or("Dependency not found");
            draw_flow_node(
                canvas,
                dep,
                title,
                MARGIN + 24.0,
                BOARD_TOP + 94.0 + slot as f32 * 92.0,
                320.0,
                false,
            )?;
        }
    }
    draw_flow_node(
        canvas,
        &task.id,
        &task.title,
        432.0,
        BOARD_TOP + 152.0,
        320.0,
        true,
    )?;

    let mut unlock_slot = 0usize;
    for candidate in &state.tasks {
        if candidate.deps.iter().any(|dep| dep == &task.id) {
            draw_flow_node(
                canvas,
                &candidate.id,
                &candidate.title,
                812.0,
                BOARD_TOP + 94.0 + unlock_slot as f32 * 92.0,
                320.0,
                false,
            )?;
            unlock_slot += 1;
            if unlock_slot >= 3 {
                break;
            }
        }
    }
    if unlock_slot == 0 {
        draw_flow_node(
            canvas,
            "No direct child",
            "Nothing currently names this task as a prerequisite.",
            812.0,
            BOARD_TOP + 94.0,
            320.0,
            false,
        )?;
    }
    Ok(())
}

fn draw_flow_node(
    canvas: u64,
    id: &str,
    title: &str,
    x: f32,
    y: f32,
    w: f32,
    selected: bool,
) -> Result<(), gfx::GfxError> {
    rounded(
        canvas,
        x,
        y,
        w,
        74.0,
        12.0,
        if selected { ACCENT_SOFT } else { SURFACE_2 },
    )?;
    stroke(
        canvas,
        x,
        y,
        w,
        74.0,
        12.0,
        if selected { ACCENT } else { LINE },
    )?;
    text(
        canvas,
        id,
        x + 12.0,
        y + 20.0,
        11.0,
        if selected { ACCENT } else { INK_DIM },
    );
    draw_wrapped(canvas, title, x + 12.0, y + 42.0, 42, 2, 12.5, INK);
    Ok(())
}

fn draw_detail(canvas: u64, state: &AppState) -> Result<(), gfx::GfxError> {
    rounded(
        canvas,
        MARGIN,
        DETAIL_TOP,
        WIDTH - MARGIN * 2.0,
        DETAIL_H,
        14.0,
        SURFACE,
    )?;
    let Some(task) = state.tasks.get(state.selected) else {
        return Ok(());
    };
    text(
        canvas,
        &task.id,
        MARGIN + 14.0,
        DETAIL_TOP + 24.0,
        11.0,
        ACCENT,
    );
    draw_wrapped(
        canvas,
        &task.outcome,
        MARGIN + 14.0,
        DETAIL_TOP + 48.0,
        92,
        3,
        12.0,
        INK_DIM,
    );
    text(
        canvas,
        truncate(&state.message, 84),
        MARGIN + 14.0,
        DETAIL_TOP + DETAIL_H - 10.0,
        10.5,
        INK_QUIET,
    );

    let can_lock = task.status == READY;
    let can_complete = task.status == READY || task.status == LOCKED;
    let can_park = task.status != DONE && task.status != DROPPED;
    let bx = WIDTH - MARGIN - 286.0;
    draw_button(canvas, "Lock", bx, DETAIL_TOP + 14.0, 82.0, can_lock, true)?;
    draw_button(
        canvas,
        "Complete",
        bx + 92.0,
        DETAIL_TOP + 14.0,
        92.0,
        can_complete,
        false,
    )?;
    draw_button(
        canvas,
        "Park",
        bx + 194.0,
        DETAIL_TOP + 14.0,
        82.0,
        can_park,
        false,
    )?;
    text(
        canvas,
        lane_name(task.lane),
        bx,
        DETAIL_TOP + 72.0,
        11.0,
        INK_DIM,
    );
    text(
        canvas,
        status_name(task.status),
        bx + 72.0,
        DETAIL_TOP + 72.0,
        11.0,
        INK_DIM,
    );
    Ok(())
}

fn draw_input(canvas: u64, state: &AppState) -> Result<(), gfx::GfxError> {
    let field_w = 690.0;
    rounded(
        canvas,
        MARGIN,
        INPUT_TOP,
        field_w,
        INPUT_H,
        10.0,
        if state.field_focus {
            ACCENT_SOFT
        } else {
            SURFACE
        },
    )?;
    stroke(
        canvas,
        MARGIN,
        INPUT_TOP,
        field_w,
        INPUT_H,
        10.0,
        if state.field_focus { ACCENT } else { LINE },
    )?;
    if state.draft.is_empty() {
        text(
            canvas,
            "Add a task outcome...",
            MARGIN + 12.0,
            INPUT_TOP + 25.0,
            13.0,
            INK_QUIET,
        );
    } else {
        text(
            canvas,
            truncate(&state.draft, 82),
            MARGIN + 12.0,
            INPUT_TOP + 25.0,
            13.0,
            INK,
        );
    }
    draw_button(
        canvas,
        if state.new_lane == BUILD {
            "Build"
        } else {
            "Company"
        },
        MARGIN + field_w + 10.0,
        INPUT_TOP + 3.0,
        90.0,
        true,
        false,
    )?;
    draw_button(
        canvas,
        "Add free",
        MARGIN + field_w + 110.0,
        INPUT_TOP + 3.0,
        92.0,
        !state.draft.trim().is_empty(),
        true,
    )?;
    draw_button(
        canvas,
        "Add after selected",
        MARGIN + field_w + 212.0,
        INPUT_TOP + 3.0,
        172.0,
        !state.draft.trim().is_empty(),
        false,
    )?;
    Ok(())
}

fn draw(canvas: u64, state: &AppState) -> Result<(), gfx::GfxError> {
    canvas2d::linear_gradient(
        canvas,
        gfx::Rect {
            x: 0.0,
            y: 0.0,
            width: WIDTH,
            height: HEIGHT,
        },
        BG_TOP,
        BG_BOTTOM,
    )?;
    draw_header(canvas, state)?;
    if state.view == View::Board {
        draw_board(canvas, state)?;
    } else {
        draw_flow(canvas, state)?;
    }
    draw_detail(canvas, state)?;
    draw_input(canvas, state)?;
    canvas2d::present(canvas)
}

fn hit_card(state: &AppState, x: f32, y: f32) -> Option<usize> {
    if y < BOARD_TOP + 48.0 || y > BOARD_BOTTOM - 12.0 {
        return None;
    }
    for column in 0..3usize {
        let cx = MARGIN + column as f32 * (COL_W + GAP);
        if x < cx + 10.0 || x > cx + COL_W - 10.0 {
            continue;
        }
        let count = column_count(&state.tasks, column);
        let offset = state.scroll[column].min(count.saturating_sub(VISIBLE_CARDS));
        let mut ordinal = 0usize;
        let mut slot = 0usize;
        for (index, task) in state.tasks.iter().enumerate() {
            if display_column(task.status) != Some(column) {
                continue;
            }
            if ordinal < offset {
                ordinal += 1;
                continue;
            }
            if slot >= VISIBLE_CARDS {
                break;
            }
            let cy = BOARD_TOP + 48.0 + slot as f32 * (CARD_H + CARD_GAP);
            if y >= cy && y <= cy + CARD_H {
                return Some(index);
            }
            ordinal += 1;
            slot += 1;
        }
    }
    None
}

fn in_rect(x: f32, y: f32, rx: f32, ry: f32, rw: f32, rh: f32) -> bool {
    x >= rx && x <= rx + rw && y >= ry && y <= ry + rh
}

fn handle_pointer(state: &mut AppState, x: f32, y: f32) {
    if in_rect(x, y, MARGIN, 112.0, 76.0, 34.0) {
        state.view = View::Board;
        state.field_focus = false;
        return;
    }
    if in_rect(x, y, MARGIN + 84.0, 112.0, 70.0, 34.0) {
        state.view = View::Flow;
        state.field_focus = false;
        return;
    }
    if state.view == View::Board {
        if let Some(index) = hit_card(state, x, y) {
            state.selected = index;
            state.field_focus = false;
            state.message =
                "Selected. Review the outcome and choose an explicit action.".to_string();
            return;
        }
    }

    let bx = WIDTH - MARGIN - 286.0;
    if in_rect(x, y, bx, DETAIL_TOP + 14.0, 82.0, 34.0) {
        lock_selected(state);
        return;
    }
    if in_rect(x, y, bx + 92.0, DETAIL_TOP + 14.0, 92.0, 34.0) {
        complete_selected(state);
        return;
    }
    if in_rect(x, y, bx + 194.0, DETAIL_TOP + 14.0, 82.0, 34.0) {
        park_selected(state);
        return;
    }

    let field_w = 690.0;
    if in_rect(x, y, MARGIN, INPUT_TOP, field_w, INPUT_H) {
        state.field_focus = true;
        return;
    }
    if in_rect(x, y, MARGIN + field_w + 10.0, INPUT_TOP + 3.0, 90.0, 34.0) {
        state.new_lane = if state.new_lane == BUILD {
            COMPANY
        } else {
            BUILD
        };
        state.message = "New-task focus lane changed.".to_string();
        return;
    }
    if in_rect(x, y, MARGIN + field_w + 110.0, INPUT_TOP + 3.0, 92.0, 34.0) {
        add_task(state, false);
        return;
    }
    if in_rect(x, y, MARGIN + field_w + 212.0, INPUT_TOP + 3.0, 172.0, 34.0) {
        add_task(state, true);
        return;
    }
    state.field_focus = false;
}

fn handle_wheel(state: &mut AppState, x: f32, y: f32, dy: f32) {
    if state.view != View::Board || y < BOARD_TOP || y > BOARD_BOTTOM {
        return;
    }
    for column in 0..3usize {
        let cx = MARGIN + column as f32 * (COL_W + GAP);
        if x < cx || x > cx + COL_W {
            continue;
        }
        let count = column_count(&state.tasks, column);
        let max = count.saturating_sub(VISIBLE_CARDS);
        if dy > 0.0 {
            state.scroll[column] = (state.scroll[column] + 1).min(max);
        } else if dy < 0.0 {
            state.scroll[column] = state.scroll[column].saturating_sub(1);
        }
    }
}

fn stack_root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 1.0,
            padding: 0.0,
            text: None,
            place: None,
            box_: None,
        },
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
        role: Some("Founder OS task board".to_string()),
        style: types::Style {
            width: None,
            height: None,
            grow: 1.0,
            padding: 0.0,
            text: None,
            place: None,
            box_: None,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn push_bytes(buf: &mut [u8], pos: &mut usize, bytes: &[u8]) {
    for byte in bytes {
        if let Some(slot) = buf.get_mut(*pos) {
            *slot = *byte;
            *pos += 1;
        }
    }
}

fn push_num(buf: &mut [u8], pos: &mut usize, value: u32) {
    let mut scratch = [0u8; 12];
    let mut n = value;
    let mut len = 0usize;
    if n == 0 {
        scratch[0] = b'0';
        len = 1;
    } else {
        while n > 0 && len < scratch.len() {
            scratch[len] = b'0' + (n % 10) as u8;
            n /= 10;
            len += 1;
        }
    }
    while len > 0 {
        len -= 1;
        if let Some(slot) = buf.get_mut(*pos) {
            *slot = scratch[len];
            *pos += 1;
        }
    }
}

fn number_bytes<'a>(value: u32, buf: &'a mut [u8]) -> &'a [u8] {
    let mut pos = 0usize;
    push_num(buf, &mut pos, value);
    buf.get(..pos).unwrap_or(b"0")
}

fn count_label<'a>(count: u32, name: &str, buf: &'a mut [u8; 48]) -> &'a [u8] {
    let mut pos = 0usize;
    push_num(buf, &mut pos, count);
    push_bytes(buf, &mut pos, b" ");
    push_bytes(buf, &mut pos, name.as_bytes());
    buf.get(..pos).unwrap_or(&[])
}

fn range_label<'a>(from: usize, to: usize, total: usize, buf: &'a mut [u8; 48]) -> &'a [u8] {
    let mut pos = 0usize;
    push_num(buf, &mut pos, from as u32);
    push_bytes(buf, &mut pos, b"-");
    push_num(buf, &mut pos, to as u32);
    push_bytes(buf, &mut pos, b" of ");
    push_num(buf, &mut pos, total as u32);
    buf.get(..pos).unwrap_or(&[])
}

fn report(state: &AppState) {
    let out = stdio::stdout();
    let _ = out.write(b"founder-os:ok\n");
    let mut buf = [0u8; 12];
    let _ = out.write(b"tasks:");
    let _ = out.write(number_bytes(state.tasks.len() as u32, &mut buf));
    let _ = out.write(b" locked:");
    let _ = out.write(number_bytes(
        status_count(&state.tasks, LOCKED) as u32,
        &mut buf,
    ));
    let _ = out.write(b" ready:");
    let _ = out.write(number_bytes(
        status_count(&state.tasks, READY) as u32,
        &mut buf,
    ));
    let _ = out.write(b" blocked:");
    let _ = out.write(number_bytes(
        status_count(&state.tasks, BLOCKED) as u32,
        &mut buf,
    ));
    let _ = out.write(b"\n");
    let _ = out.flush();
}

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
        };
        let Ok(win) = window::create("Krate Founder OS", size) else {
            return 30;
        };
        if window::show(win).is_err() {
            return 31;
        }
        if tree::set_root(win, &stack_root()).is_err()
            || tree::upsert_node(win, &canvas_node()).is_err()
        {
            let _ = window::close(win);
            return 32;
        }
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => {
                let _ = window::close(win);
                return 33;
            }
        };
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );
        let mut state = load_state();
        let raw = args::raw();
        let mut quick = false;
        let mut shoot_flow = false;
        for argument in raw.as_bytes().split(|byte| *byte == b'\n') {
            if argument == b"quick" {
                quick = true;
            }
            if argument == b"flow" {
                shoot_flow = true;
            }
        }
        if shoot_flow {
            state.view = View::Flow;
        }

        if draw(canvas, &state).is_err() {
            let _ = window::close(win);
            return 34;
        }
        if quick {
            report(&state);
            let _ = window::close(win);
            return 0;
        }

        for _ in 0..MAX_WAIT_ROUNDS {
            let mut dirty = false;
            let mut close = false;
            match events::wait(Some(WAIT_ROUND_MILLIS)) {
                Some(types::Event::Pointer(pointer)) if pointer.pressed => {
                    handle_pointer(&mut state, pointer.x, pointer.y);
                    dirty = true;
                }
                Some(types::Event::Wheel(wheel)) => {
                    handle_wheel(&mut state, wheel.x, wheel.y, wheel.dy);
                    dirty = true;
                }
                Some(types::Event::TextInput(value)) if state.field_focus => {
                    for ch in value.chars() {
                        if !ch.is_control() && state.draft.len() < 180 {
                            state.draft.push(ch);
                        }
                    }
                    dirty = true;
                }
                Some(types::Event::TextChanged(changed)) if state.field_focus => {
                    state.draft.clear();
                    for ch in changed.text.chars() {
                        if !ch.is_control() && state.draft.len() < 180 {
                            state.draft.push(ch);
                        }
                    }
                    dirty = true;
                }
                Some(types::Event::Key(key)) if key.pressed && state.field_focus => {
                    if key.key.as_bytes() == b"Backspace" {
                        state.draft.pop();
                        dirty = true;
                    } else if key.key.as_bytes() == b"Enter" || key.key.as_bytes() == b"Return" {
                        add_task(&mut state, false);
                        dirty = true;
                    } else if key.key.as_bytes() == b"Escape" {
                        state.field_focus = false;
                        dirty = true;
                    }
                }
                Some(types::Event::Resized(_))
                | Some(types::Event::RedrawRequested(_))
                | Some(types::Event::ThemeChanged(_)) => {
                    dirty = true;
                }
                Some(types::Event::CloseRequested(id)) if id == win => close = true,
                _ => {}
            }
            if dirty {
                let _ = draw(canvas, &state);
            }
            if close {
                break;
            }
        }
        let _ = window::close(win);
        report(&state);
        0
    }
}

bindings::export!(Component with_types_in bindings);
