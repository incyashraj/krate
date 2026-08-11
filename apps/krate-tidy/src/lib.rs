//! Krate tidy — the folder app that used to be impossible.
//!
//! Pick a folder; Tidy sorts what is inside into Images, Documents, Audio,
//! Archives and Other, by extension. That is the whole app — and it is the
//! worked example of the pattern that makes folder apps buildable at all:
//! **the pick is the grant**. The manifest declares no fs capability. The
//! person chooses a folder in the native dialog, the app receives a token,
//! and every ordinary fs call works under `picked/<token>/...` for this run,
//! inside that folder and nowhere else.
//!
//! An outside reviewer once asked for exactly this app and found it could
//! not exist: no path could be named, no dialog existed, and the generator
//! reached for `fs.*:**` because nothing else even looked like it might
//! work. This file is the after picture.
//!
//! `quick` proves the sorting brain without any dialog (headless runs
//! auto-cancel pickers): it classifies a fixed set of names, prints the
//! plan, draws a frame, and exits.

#![no_std]

#[allow(warnings)]
mod bindings;

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use bindings::krate::fs::files as fsapi;
use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::time::clock;
use bindings::krate::ui::{dialog, events, tree, types, window};
use krate::motion::Spring;

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 380.0;
const HEIGHT: f32 = 560.0;

/// The pill's hit box, shared by drawing and click handling so they cannot
/// drift apart.
const PILL: (f32, f32, f32, f32) = (40.0, HEIGHT - 96.0, WIDTH - 80.0, 52.0);

fn color(r: f32, g: f32, b: f32, a: f32) -> gfx::Color {
    gfx::Color { r, g, b, a }
}

fn rect(x: f32, y: f32, width: f32, height: f32) -> gfx::Rect {
    gfx::Rect {
        x,
        y,
        width,
        height,
    }
}

fn radii(all: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: all,
        top_right: all,
        bottom_right: all,
        bottom_left: all,
    }
}

fn style(weight: u16, spacing: f32) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: spacing,
        family: gfx::FontFamily::Sans,
    }
}

/// Where one file belongs, by its extension. The categories a person would
/// actually make by hand.
fn category(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.').map(|(_, e)| e)?;
    let ext = ext.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "bmp" | "svg" => "Images",
        "pdf" | "doc" | "docx" | "txt" | "md" | "rtf" | "odt" | "csv" | "xlsx" => "Documents",
        "mp3" | "wav" | "flac" | "m4a" | "ogg" | "aac" => "Audio",
        "zip" | "tar" | "gz" | "7z" | "rar" | "dmg" => "Archives",
        _ => "Other",
    })
}

/// One planned or completed move.
struct Move {
    name: String,
    dest: &'static str,
}

/// Sort the folder behind `token`. Lists it, makes the category folders,
/// moves each file. Everything runs under `picked/<token>/...`, which the
/// open-folder grant authorizes -- note there is not one fs capability in
/// the manifest.
fn tidy_picked(token: &str) -> Result<Vec<Move>, String> {
    let root = format!("picked/{token}");
    let names = fsapi::list(&root).map_err(|e| format!("could not list the folder: {e:?}"))?;

    let mut moves = Vec::new();
    for name in names {
        // Never descend into or move the folders we make ourselves, and skip
        // anything without an extension or that is a directory.
        let Some(dest) = category(&name) else {
            continue;
        };
        let stat = fsapi::stat(&format!("{root}/{name}"));
        if matches!(stat, Ok(s) if s.is_dir) {
            continue;
        }
        let _ = fsapi::mkdir(&format!("{root}/{dest}"));
        fsapi::rename(&format!("{root}/{name}"), &format!("{root}/{dest}/{name}"))
            .map_err(|e| format!("could not move {name}: {e:?}"))?;
        moves.push(Move {
            name,
            dest,
        });
    }
    Ok(moves)
}

/// What the app is showing right now.
enum Screen {
    Idle,
    Done {
        folder: String,
        moves: Vec<Move>,
    },
    Failed(String),
}

fn draw(canvas: u64, screen: &Screen, pill_scale: f32) -> Result<(), gfx::GfxError> {
    canvas2d::linear_gradient_stops(
        canvas,
        rect(0.0, 0.0, WIDTH, HEIGHT),
        115.0,
        &[
            gfx::GradientStop {
                offset: 0.0,
                color: color(0.05, 0.06, 0.11, 1.0),
            },
            gfx::GradientStop {
                offset: 1.0,
                color: color(0.10, 0.12, 0.22, 1.0),
            },
        ],
    )?;

    canvas2d::draw_text_styled(
        canvas,
        "Tidy",
        gfx::Point { x: 28.0, y: 66.0 },
        34.0,
        color(1.0, 1.0, 1.0, 1.0),
        style(700, -0.8),
    )?;
    canvas2d::draw_text(
        canvas,
        "Sort a folder into Images, Documents, Audio and more",
        gfx::Point { x: 28.0, y: 90.0 },
        13.0,
        color(1.0, 1.0, 1.0, 0.45),
    )?;

    // The result card.
    let card = rect(24.0, 116.0, WIDTH - 48.0, HEIGHT - 240.0);
    canvas2d::drop_shadow_round_rect(
        canvas,
        rect(card.x, card.y + 6.0, card.width, card.height),
        radii(18.0),
        16.0,
        color(0.0, 0.0, 0.0, 0.4),
    )?;
    canvas2d::fill_round_rect(canvas, card, radii(18.0), color(1.0, 1.0, 1.0, 0.05))?;
    canvas2d::stroke_round_rect(canvas, card, radii(18.0), 1.0, color(1.0, 1.0, 1.0, 0.12))?;

    let inner_x = card.x + 20.0;
    match screen {
        Screen::Idle => {
            canvas2d::draw_text(
                canvas,
                "Nothing tidied yet.",
                gfx::Point {
                    x: inner_x,
                    y: card.y + 40.0,
                },
                15.0,
                color(1.0, 1.0, 1.0, 0.85),
            )?;
            canvas2d::draw_text(
                canvas,
                "Pick a folder below. Krate only lets this app",
                gfx::Point {
                    x: inner_x,
                    y: card.y + 66.0,
                },
                13.0,
                color(1.0, 1.0, 1.0, 0.45),
            )?;
            canvas2d::draw_text(
                canvas,
                "touch the folder you choose, and only this run.",
                gfx::Point {
                    x: inner_x,
                    y: card.y + 84.0,
                },
                13.0,
                color(1.0, 1.0, 1.0, 0.45),
            )?;
        }
        Screen::Done { folder, moves } => {
            canvas2d::draw_text_styled(
                canvas,
                &format!("{} moved", moves.len()),
                gfx::Point {
                    x: inner_x,
                    y: card.y + 44.0,
                },
                26.0,
                color(1.0, 1.0, 1.0, 1.0),
                style(650, -0.5),
            )?;
            canvas2d::draw_text(
                canvas,
                &format!("in {folder}"),
                gfx::Point {
                    x: inner_x,
                    y: card.y + 66.0,
                },
                12.5,
                color(1.0, 1.0, 1.0, 0.45),
            )?;
            let mut y = card.y + 96.0;
            for one in moves.iter().take(10) {
                canvas2d::draw_text(
                    canvas,
                    &format!("{}  ->  {}/", one.name, one.dest),
                    gfx::Point { x: inner_x, y },
                    13.0,
                    color(1.0, 1.0, 1.0, 0.75),
                )?;
                y += 20.0;
            }
            if moves.len() > 10 {
                canvas2d::draw_text(
                    canvas,
                    &format!("and {} more", moves.len() - 10),
                    gfx::Point { x: inner_x, y },
                    12.5,
                    color(1.0, 1.0, 1.0, 0.4),
                )?;
            }
        }
        Screen::Failed(why) => {
            canvas2d::draw_text(
                canvas,
                "That did not work:",
                gfx::Point {
                    x: inner_x,
                    y: card.y + 40.0,
                },
                15.0,
                color(1.0, 0.6, 0.6, 0.95),
            )?;
            canvas2d::draw_text(
                canvas,
                why,
                gfx::Point {
                    x: inner_x,
                    y: card.y + 64.0,
                },
                12.5,
                color(1.0, 1.0, 1.0, 0.6),
            )?;
        }
    }

    // The pill, breathing on its spring while idle.
    let (px, py, pw, ph) = PILL;
    let grow_x = pw * (pill_scale - 1.0) / 2.0;
    let grow_y = ph * (pill_scale - 1.0) / 2.0;
    let pill = rect(px - grow_x, py - grow_y, pw + grow_x * 2.0, ph + grow_y * 2.0);
    canvas2d::drop_shadow_round_rect(
        canvas,
        rect(pill.x, pill.y + 5.0, pill.width, pill.height),
        radii(26.0),
        14.0,
        color(0.30, 0.42, 1.0, 0.45),
    )?;
    canvas2d::fill_round_rect(canvas, pill, radii(26.0), color(0.42, 0.55, 1.0, 1.0))?;
    canvas2d::draw_text_styled(
        canvas,
        "Pick a folder to tidy",
        gfx::Point {
            x: pill.x + pill.width / 2.0 - 76.0,
            y: pill.y + pill.height / 2.0 + 6.0,
        },
        16.0,
        color(1.0, 1.0, 1.0, 1.0),
        style(600, 0.2),
    )?;

    canvas2d::present(canvas)
}

fn out(line: &str) {
    let handle = stdio::stdout();
    let _ = handle.write(line.as_bytes());
    let _ = handle.write(b"\n");
}

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 1.0,
            padding: 0.0,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

/// The quick run proves the sorting brain with no dialog and no fs: it
/// classifies a fixed set of names and prints the plan the interactive run
/// would execute.
fn quick_proof() {
    let names = [
        "holiday.jpg",
        "invoice.pdf",
        "song.mp3",
        "backup.zip",
        "notes.txt",
        "mystery.bin",
        "no-extension",
    ];
    let mut planned = 0;
    for name in names {
        if let Some(dest) = category(name) {
            out(&format!("plan:{name}->{dest}"));
            planned += 1;
        }
    }
    out(&format!("planned:{planned}"));
}

struct Component;

impl bindings::Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let quick = raw.split_whitespace().any(|word| word == "quick");

        let win = match window::create(
            "Tidy",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) {
            Ok(win) => win,
            Err(_) => {
                out("window:no");
                return 1;
            }
        };
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            out("tree:no");
            return 1;
        }
        let _ = tree::upsert_node(win, &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas));
        let canvas = match canvas2d::bind(win, CANVAS_ID) {
            Ok(canvas) => canvas,
            Err(_) => {
                out("bind:no");
                return 1;
            }
        };
        // The app's own coordinate system: keep drawing in these numbers
        // and the host scales them to any window, centred, never stretched
        // out of proportion (K-096).
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: WIDTH,
            },
        );

        let mut screen = Screen::Idle;
        let mut pill = Spring::rest_at(1.0, 14.0);
        let mut pill_target = 1.0f32;
        let started = clock::monotonic_nanos();
        let mut last = started;
        let mut frames: u32 = 0;

        loop {
            let now = clock::monotonic_nanos();
            let dt = if quick {
                1.0 / 60.0
            } else {
                let dt = ((now.saturating_sub(last)) as f32 / 1_000_000_000.0).min(0.05);
                last = now;
                dt
            };
            pill.tick(pill_target, dt);

            if draw(canvas, &screen, pill.value).is_err() {
                out("draw:no");
                return 1;
            }
            frames += 1;

            if quick {
                if frames >= 30 {
                    break;
                }
                continue;
            }

            let _ = window::request_redraw(win);
            match events::wait(Some(16)) {
                Some(types::Event::CloseRequested(_)) => break,
                Some(types::Event::Pointer(p)) => {
                    let (px, py, pw, ph) = PILL;
                    let inside = p.x >= px && p.x <= px + pw && p.y >= py && p.y <= py + ph;
                    pill_target = if inside { 1.04 } else { 1.0 };
                    let pressed = p.pressed && p.button.is_some();
                    if inside && pressed {
                        match dialog::open_folder(win, "Pick a folder to tidy") {
                            Ok(Some(folder)) => {
                                screen = match tidy_picked(&folder.token) {
                                    Ok(moves) => Screen::Done {
                                        folder: folder.name,
                                        moves,
                                    },
                                    Err(why) => Screen::Failed(why),
                                };
                            }
                            // Cancelled: a normal answer, nothing changes.
                            Ok(None) => {}
                            Err(e) => {
                                screen = Screen::Failed(format!("{e:?}"));
                            }
                        }
                    }
                }
                Some(_) | None => {}
            }
        }

        if quick {
            quick_proof();
        }
        out("tidy:ready");
        0
    }
}

bindings::export!(Component with_types_in bindings);
