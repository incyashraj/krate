//! Sound Check -- the browser Player's sound proof (K-1134,
//! crates/adapter-web/player/audio-proof.mjs).
//!
//! Every path of krate:audio playback a game uses, in one small window:
//!
//! - a stream kept fed with a quiet tone through `write`, never more than a
//!   tenth of a second ahead, by `queued` (K-1011) -- so before the person
//!   taps, the queue still drains and the app is not left waiting;
//! - a click loaded once with `load-sound` and, on every press of a key or a
//!   pointer, played twice at once with `play-sound`: two copies of one
//!   sound, mixed, which is what makes rapid fire sound like rapid fire.
//!
//! What it did is drawn on the canvas: the queue, the presses, the frames.

#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use krate::audio::playback;
use krate::audio::playback_queue;
use krate::audio::types::{SampleFormat, StreamConfig};
use krate::gfx::{canvas2d, types as gfx};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const WIDTH: f32 = 320.0;
const HEIGHT: f32 = 200.0;
const RATE: u32 = 44_100;
/// The most the stream runs ahead of the speaker: a tenth of a second.
const AHEAD: u32 = RATE / 10;
/// One write: a sixtieth of a second.
const CHUNK: usize = (RATE / 60) as usize;

struct Component;

fn color(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
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

fn push_s16(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
}

/// A triangle wave at `hz`, `level` loud, continuing from `phase`.
fn tone(phase: &mut f32, hz: f32, level: f32, frames: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(frames * 2);
    let step = hz / RATE as f32;
    for _ in 0..frames {
        let t = *phase;
        let tri = if t < 0.5 { 4.0 * t - 1.0 } else { 3.0 - 4.0 * t };
        push_s16(&mut out, tri * level);
        *phase += step;
        if *phase >= 1.0 {
            *phase -= 1.0;
        }
    }
    out
}

/// A short square click that dies away over a tenth of a second.
fn click() -> Vec<u8> {
    let frames = (RATE / 10) as usize;
    let mut out = Vec::with_capacity(frames * 2);
    let period = (RATE / 880) as usize;
    for i in 0..frames {
        let decay = 1.0 - i as f32 / frames as f32;
        let sq = if (i / (period / 2)) % 2 == 0 { 1.0 } else { -1.0 };
        push_s16(&mut out, sq * 0.3 * decay);
    }
    out
}

fn digits(mut n: u64) -> Vec<u8> {
    let mut d = Vec::new();
    loop {
        d.push(b'0' + (n % 10) as u8);
        n /= 10;
        if n == 0 {
            break;
        }
    }
    d.reverse();
    d
}

fn line(label: &str, n: u64) -> alloc::string::String {
    let mut s = alloc::string::String::from(label);
    for b in digits(n) {
        s.push(b as char);
    }
    s
}

impl krate::Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create(
            "Sound Check",
            types::WindowSize {
                width: WIDTH as u32,
                height: HEIGHT as u32,
            },
        ) else {
            return 1;
        };
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            return 2;
        }
        let _ = tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        );
        let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
            return 3;
        };
        let _ = canvas2d::set_design_size(
            canvas,
            gfx::Size {
                width: WIDTH,
                height: HEIGHT,
            },
        );

        // A host with no speakers is a normal outcome: the app runs silent.
        let stream = playback::open(StreamConfig {
            sample_rate: RATE,
            channels: 1,
            format: SampleFormat::PcmS16,
            buffer_frames: 1024,
        })
        .ok();
        if let Some(s) = stream {
            let _ = playback::start(s);
        }
        let snd = stream.and_then(|s| playback::load_sound(s, &click()).ok());

        let mut phase = 0.0_f32;
        let mut pending: Vec<u8> = Vec::new();
        let mut written: u64 = 0;
        let mut presses: u64 = 0;
        let mut frames: u64 = 0;
        let mut queued: u32 = 0;

        loop {
            // Keep the stream fed, a tenth of a second ahead at most.
            if let Some(s) = stream {
                queued = playback_queue::queued(s).unwrap_or(0);
                while queued < AHEAD {
                    if pending.is_empty() {
                        pending = tone(&mut phase, 220.0, 0.08, CHUNK);
                    }
                    let took = playback::write(s, &pending).unwrap_or(0) as usize;
                    if took == 0 {
                        break;
                    }
                    written += (took / 2) as u64;
                    pending.drain(..took);
                    queued = playback_queue::queued(s).unwrap_or(AHEAD);
                }
            }

            let _ = canvas2d::clear(canvas, color(0.07, 0.09, 0.14));
            let fill = (queued as f32 / AHEAD as f32).min(1.0) * (WIDTH - 40.0);
            let _ = canvas2d::fill_rect(
                canvas,
                gfx::Rect {
                    x: 20.0,
                    y: 150.0,
                    width: fill.max(1.0),
                    height: 14.0,
                },
                color(0.35, 0.75, 0.45),
            );
            let ink = color(0.92, 0.94, 0.98);
            let _ = canvas2d::draw_text(canvas, "Sound Check", gfx::Point { x: 20.0, y: 36.0 }, 22.0, ink);
            let _ = canvas2d::draw_text(canvas, &line("presses ", presses), gfx::Point { x: 20.0, y: 72.0 }, 14.0, ink);
            let _ = canvas2d::draw_text(canvas, &line("frames written ", written), gfx::Point { x: 20.0, y: 96.0 }, 14.0, ink);
            let _ = canvas2d::draw_text(canvas, &line("frame ", frames), gfx::Point { x: 20.0, y: 120.0 }, 14.0, ink);
            let _ = canvas2d::present(canvas);
            frames += 1;

            // Every event that came in this frame, not one: a press must
            // not wait behind others to be heard.
            let mut next = events::wait(Some(16));
            let mut closed = false;
            while let Some(event) = next {
                let pressed = match event {
                    types::Event::CloseRequested(_) => {
                        closed = true;
                        false
                    }
                    types::Event::Pointer(p) => p.pressed,
                    types::Event::Key(k) => k.pressed,
                    _ => false,
                };
                if pressed {
                    presses += 1;
                    // The same sound twice at once: two copies, mixed.
                    if let (Some(s), Some(id)) = (stream, snd) {
                        let _ = playback::play_sound(s, id, 1.0);
                        let _ = playback::play_sound(s, id, 1.0);
                    }
                }
                next = events::poll();
            }
            if closed {
                break;
            }
        }
        let _ = window::close(win);
        0
    }
}

krate::export!(Component);
