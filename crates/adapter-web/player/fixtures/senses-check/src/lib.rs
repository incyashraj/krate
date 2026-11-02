//! Senses Check -- the browser Player's proof of the device's senses
//! (IC-974, crates/adapter-web/player/senses-proof.mjs).
//!
//! Notifications, where the device is, the share sheet, speaking aloud,
//! the camera and the microphone, each the way an app uses it. Each outcome
//! is a line on stdout (the proof reads it) and on the canvas:
//!
//! - on start, what the desktop refuses before any device is touched (an
//!   empty title, a 0-pixel camera, an 1 kHz microphone, an empty line, a
//!   file name with a folder in it);
//! - the location (the first ask answers finding), speech (a line said and
//!   stopped), the camera (a frame of the size it says, then stop and
//!   close) and the microphone (two seconds heard, converted to 16 kHz
//!   mono 16-bit);
//! - seven seconds after the last tap, so nobody's tap is behind them, a
//!   notification and a share;
//! - on the first tap a notification, on the second a share.
//!
//! Without the capabilities (manifest-ungranted.toml) every call answers
//! the desktop's refusal.

#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::bindings::krate::audio::types::SampleFormat;
use krate::bindings::krate::camera::types::FrameFormat;
use krate::audio::capture as mic;
use krate::camera::capture as camera;
use krate::gfx::{canvas2d, types as gfx};
use krate::io::stdio;
use krate::speech::synthesis as speech;
use krate::ui::location;
use krate::ui::notify;
use krate::ui::share::{self, SharedFile};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const WIDTH: f32 = 360.0;
const HEIGHT: f32 = 340.0;

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

struct Screen {
    canvas: u64,
    lines: Vec<String>,
}

impl Screen {
    fn say(&mut self, text: String) {
        let _ = stdio::println(&text);
        self.lines.push(text);
        if self.lines.len() > 18 {
            self.lines.remove(0);
        }
        self.draw();
    }

    fn draw(&self) {
        let _ = canvas2d::clear(self.canvas, color(0.07, 0.09, 0.14));
        let ink = color(0.92, 0.94, 0.98);
        let _ = canvas2d::draw_text(
            self.canvas,
            "Senses Check",
            gfx::Point { x: 16.0, y: 30.0 },
            20.0,
            ink,
        );
        for (i, l) in self.lines.iter().enumerate() {
            let _ = canvas2d::draw_text(
                self.canvas,
                l,
                gfx::Point {
                    x: 16.0,
                    y: 56.0 + 15.0 * i as f32,
                },
                9.0,
                ink,
            );
        }
        let _ = canvas2d::present(self.canvas);
    }
}

/// Wait `n` tenths of a second with nothing happening.
fn quiet(n: u32) {
    let mut q = 0;
    while q < n {
        if events::wait(Some(100)).is_none() {
            q += 1;
        }
    }
}

fn cam_config(width: u32, height: u32) -> camera::StreamConfig {
    camera::StreamConfig {
        width,
        height,
        fps: 30,
        format: FrameFormat::Rgba8,
    }
}

fn mic_config(sample_rate: u32) -> mic::StreamConfig {
    mic::StreamConfig {
        sample_rate,
        channels: 1,
        format: SampleFormat::PcmS16,
        buffer_frames: 1600,
    }
}

fn note() -> Vec<SharedFile> {
    alloc::vec![SharedFile {
        name: String::from("note.txt"),
        data: Vec::from(&b"Shared from a tab\n"[..]),
    }]
}

fn location_line() -> (String, String) {
    let first = match location::current() {
        Ok(p) => format!("{:.4},{:.4}", p.latitude, p.longitude),
        Err(e) => format!("{e:?}"),
    };
    let mut last = String::new();
    for _ in 0..100 {
        match location::current() {
            Ok(p) => {
                return (
                    first,
                    format!(
                        "{:.4},{:.4} accuracy={} age-ms<60000={}",
                        p.latitude,
                        p.longitude,
                        p.accuracy,
                        p.age_ms < 60_000
                    ),
                )
            }
            Err(location::LocationError::Finding) => last = String::from("Finding"),
            Err(e) => return (first, format!("{e:?}")),
        }
        quiet(1);
    }
    (first, last)
}

fn camera_lines(s: &mut Screen) {
    match camera::devices() {
        Ok(list) => s.say(format!("camera-devices: {}", list.len())),
        Err(e) => s.say(format!("camera-devices: {e:?}")),
    }
    let id = match camera::open("", cam_config(320, 240)) {
        Ok(id) => id,
        Err(e) => return s.say(format!("camera: {e:?}")),
    };
    if let Ok(i) = camera::info(id) {
        s.say(format!("camera-info-before: {}x{}", i.width, i.height));
    }
    s.say(format!("camera-read-before-start: {:?}", camera::read(id).map(|f| f.is_some())));
    if let Err(e) = camera::start(id) {
        return s.say(format!("camera: start {e:?}"));
    }
    let mut line = String::from("camera: no frame in 15 s");
    for _ in 0..150 {
        match camera::read(id) {
            Ok(Some(f)) => {
                let fits = f.bytes.len() == (f.width * f.height * 4) as usize;
                let opaque = f.bytes.chunks_exact(4).all(|p| p[3] == 255);
                let lit = f.bytes.chunks_exact(4).any(|p| p[0] > 16 || p[1] > 16 || p[2] > 16);
                line = format!(
                    "camera: frame {}x{} fits={fits} opaque={opaque} lit={lit}",
                    f.width, f.height
                );
                break;
            }
            Ok(None) => {}
            Err(e) => {
                line = format!("camera: {e:?}");
                break;
            }
        }
        quiet(1);
    }
    let failed = !line.starts_with("camera: frame");
    s.say(line);
    if !failed {
        if let Ok(i) = camera::info(id) {
            s.say(format!("camera-info: {}x{}", i.width, i.height));
        }
    }
    s.say(format!("camera-stop: {:?}", camera::stop(id)));
    s.say(format!("camera-read-after-stop: {:?}", camera::read(id).map(|f| f.is_some())));
    s.say(format!("camera-close: {:?}", camera::close(id)));
    s.say(format!("camera-read-after-close: {:?}", camera::read(id).map(|f| f.is_some())));
}

fn mic_lines(s: &mut Screen) {
    let id = match mic::open(mic_config(16_000)) {
        Ok(id) => id,
        Err(e) => return s.say(format!("mic: {e:?}")),
    };
    s.say(format!("mic-before-start: {:?}", mic::read(id, 65_536).map(|b| b.len())));
    if let Err(e) = mic::start(id) {
        return s.say(format!("mic: start {e:?}"));
    }
    let mut heard: Vec<u8> = Vec::new();
    let mut failure = None;
    for _ in 0..20 {
        quiet(1);
        match mic::read(id, 65_536) {
            Ok(b) => heard.extend_from_slice(&b),
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    match failure {
        Some(e) => s.say(format!("mic: {e:?}")),
        None => {
            let peak = heard
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs())
                .max()
                .unwrap_or(0);
            s.say(format!("mic: bytes={} peak={peak}", heard.len()));
        }
    }
    s.say(format!("mic-stop: {:?}", mic::stop(id)));
    s.say(format!("mic-after-stop: {:?}", mic::read(id, 65_536).map(|b| b.len())));
    s.say(format!("mic-unknown: {:?}", mic::read(id + 99, 16).map(|b| b.len())));
}

fn speech_lines(s: &mut Screen) {
    match speech::voices() {
        Ok(v) => s.say(format!("voices: some={}", !v.is_empty())),
        Err(e) => s.say(format!("voices: {e:?}")),
    }
    s.say(format!("say-voice: {:?}", speech::say("hi", Some("no-such-voice"), None)));
    s.say(format!("say: {:?}", speech::say("Hello from Senses Check.", None, Some(1.0))));
    // Long enough for the browser to have begun the line before it is
    // stopped: a stop at once raced Chromium's start, one run in three.
    quiet(10);
    s.say(format!("speaking: {:?}", speech::speaking()));
    s.say(format!("stop: {:?}", speech::stop()));
    quiet(3);
    s.say(format!("speaking-after-stop: {:?}", speech::speaking()));
}

impl krate::Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create(
            "Senses Check",
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
        let mut s = Screen {
            canvas,
            lines: Vec::new(),
        };
        s.draw();

        // The grant is checked before the text, as on desktop: an empty
        // title is invalid-content with ui.notify, denied without it.
        let empty = notify::show("", "");
        let refused = matches!(empty, Err(notify::NotifyError::Denied));
        s.say(format!("notify-empty: {empty:?}"));
        s.say(format!("location-available: {}", location::available()));
        if refused {
            s.say(format!("location: {:?}", location::current().map(|_| ())));
            s.say(format!("camera-devices: {:?}", camera::devices().map(|d| d.len())));
            s.say(format!("camera-open: {:?}", camera::open("", cam_config(320, 240))));
            s.say(format!("mic-open: {:?}", mic::open(mic_config(16_000))));
            s.say(format!("notify-start: {:?}", notify::show("Timer done", "Tea is ready")));
        } else {
            let long = "x".repeat(300);
            s.say(format!("notify-long: {:?}", notify::show(&long, "")));
            s.say(format!("notify-control: {:?}", notify::show("bell\u{7}", "")));
            s.say(format!("camera-zero: {:?}", camera::open("", cam_config(0, 240))));
            s.say(format!("mic-rate: {:?}", mic::open(mic_config(1_000))));
            s.say(format!("say-empty: {:?}", speech::say("  ", None, None)));
            s.say(format!("say-rate: {:?}", speech::say("hi", None, Some(3.0))));
            s.say(format!("share-nothing: {:?}", share::share(win, None, None, &[])));
            let bad = alloc::vec![SharedFile {
                name: String::from("a/b.txt"),
                data: Vec::from(&b"x"[..]),
            }];
            s.say(format!("share-path: {:?}", share::share(win, Some("x"), None, &bad)));
            let (first, fix) = location_line();
            s.say(format!("location-first: {first}"));
            s.say(format!("location: {fix}"));
            speech_lines(&mut s);
            camera_lines(&mut s);
            mic_lines(&mut s);
            // Long enough after the last tap (Allow on the consent card)
            // that the browser no longer counts it.
            quiet(70);
            s.say(format!("notify-start: {:?}", notify::show("Timer done", "Tea is ready")));
            // And again, so the notification's tap is not behind the share.
            quiet(70);
            s.say(format!(
                "share-start: {:?}",
                share::share(win, Some("Shared from a tab"), Some("https://krate.tech/"), &note())
            ));
        }
        s.say(String::from("ready"));

        let mut taps = 0u32;
        loop {
            let mut next = events::wait(Some(100));
            let mut closed = false;
            while let Some(event) = next {
                match event {
                    types::Event::CloseRequested(_) => closed = true,
                    types::Event::Pointer(p) if p.pressed && !refused => {
                        taps += 1;
                        let line = match taps {
                            1 => format!("notify-tap: {:?}", notify::show("Second", "From a tap")),
                            2 => format!("share-tap: {:?}", share::share(win, Some("From a tap"), None, &[])),
                            _ => format!("tap: {taps}"),
                        };
                        s.say(line);
                    }
                    _ => {}
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
