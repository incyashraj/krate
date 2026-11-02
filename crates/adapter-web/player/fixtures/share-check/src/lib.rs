//! Share Check -- the browser Player's proof of krate:store/shared and
//! krate:store/secret (IC-974, crates/adapter-web/player/share-proof.mjs).
//!
//! What it does is chosen by its first argument, and every outcome is a
//! line on stdout (the proof reads it) and on the canvas:
//!
//! - `create`: sync before a share is not-joined; makes a share, writes
//!   milk and eggs, and is refused a value over the limit and bad keys;
//!   keeps a sign-in token as a secret and is refused a secret over the
//!   limit and a bad name;
//! - `join:<code>`: a made-up code is no-such-share; joins the real one,
//!   reads what the other wrote, deletes eggs and adds bread;
//! - `reopen`: the copy and the token outlived the tab; leaving keeps the
//!   copy and forgets the code;
//! - anything else: only the first lines, which an app without the grants
//!   hears as denied.
//!
//! In `create` and `join` it then syncs every few frames, printing the
//! whole list whenever a sync says the copy changed, and writes `jam` when
//! a key is pressed.

#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use krate::gfx::{canvas2d, types as gfx};
use krate::io::stdio;
use krate::shared::{self, SharedError};
use krate::secret::{self, SecretError};
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const WIDTH: f32 = 360.0;
const HEIGHT: f32 = 260.0;
/// What the proof looks for, in plain text, in the browser's storage.
const TOKEN: &str = "tok-share-check-PLAINTEXT-7f3a9c";

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

fn shared_name(e: &SharedError) -> String {
    match e {
        SharedError::Denied => "denied".into(),
        SharedError::NotJoined => "not-joined".into(),
        SharedError::NoSuchShare => "no-such-share".into(),
        SharedError::InvalidName => "invalid-name".into(),
        SharedError::TooLarge => "too-large".into(),
        SharedError::Io(words) => format!("io ({words})"),
    }
}

fn secret_name(e: &SecretError) -> String {
    match e {
        SecretError::Denied => "denied".into(),
        SecretError::InvalidName => "invalid-name".into(),
        SecretError::TooLarge => "too-large".into(),
        SecretError::Io(words) => format!("io ({words})"),
    }
}

fn unit<E>(r: Result<(), E>, name: fn(&E) -> String) -> String {
    match r {
        Ok(()) => "ok".into(),
        Err(e) => name(&e),
    }
}

fn say(lines: &mut Vec<String>, text: String) {
    let _ = stdio::println(&text);
    lines.push(text);
}

fn text_of(key: &str) -> String {
    match shared::get_text(key) {
        Ok(Some(v)) => v,
        Ok(None) => "none".into(),
        Err(e) => shared_name(&e),
    }
}

/// The whole list as it stands here: `bread=1 milk=2`.
fn list() -> String {
    match shared::keys() {
        Ok(keys) => keys
            .iter()
            .map(|k| format!("{k}={}", text_of(k)))
            .collect::<Vec<_>>()
            .join(" "),
        Err(e) => shared_name(&e),
    }
}

fn start(lines: &mut Vec<String>) {
    let code = match shared::code() {
        Ok(Some(c)) => c,
        Ok(None) => "none".into(),
        Err(e) => shared_name(&e),
    };
    say(lines, format!("code at start: {code}"));
    let token = match secret::get("token") {
        Ok(Some(v)) if v == TOKEN.as_bytes() => "kept".into(),
        Ok(Some(_)) => "changed".into(),
        Ok(None) => "none".into(),
        Err(e) => secret_name(&e),
    };
    say(lines, format!("token: {token}"));
    let names = match secret::names() {
        Ok(n) => n.join(","),
        Err(e) => secret_name(&e),
    };
    say(lines, format!("names: {names}"));
}

fn secrets(lines: &mut Vec<String>) {
    say(lines, format!("token set: {}", unit(secret::set_text("token", TOKEN), secret_name)));
    let big = vec![7u8; 64 * 1024 + 1];
    say(lines, format!("secret too big: {}", unit(secret::set("big", &big), secret_name)));
    say(lines, format!("secret at the limit: {}", unit(secret::set("limit", &big[..64 * 1024]), secret_name)));
    say(lines, format!("secret bad name: {}", unit(secret::set("a/b", b"x"), secret_name)));
    say(lines, format!("secret delete absent: {}", unit(secret::delete("nope"), secret_name)));
    say(lines, format!("secret delete limit: {}", unit(secret::delete("limit"), secret_name)));
    let names = match secret::names() {
        Ok(n) => n.join(","),
        Err(e) => secret_name(&e),
    };
    say(lines, format!("names now: {names}"));
}

fn create(lines: &mut Vec<String>) {
    say(lines, format!("sync before a share: {}", match shared::sync() {
        Ok(b) => format!("ok {b}"),
        Err(e) => shared_name(&e),
    }));
    match shared::create() {
        Ok(code) => say(lines, format!("created: {code}")),
        Err(e) => return say(lines, format!("create: {}", shared_name(&e))),
    }
    say(lines, format!("milk: {}", unit(shared::set_text("milk", "2"), shared_name)));
    say(lines, format!("eggs: {}", unit(shared::set_text("eggs", "12"), shared_name)));
    let big = vec![1u8; 64 * 1024 + 1];
    say(lines, format!("too big: {}", unit(shared::set("big", &big), shared_name)));
    say(lines, format!("empty key: {}", unit(shared::set_text("", "x"), shared_name)));
    let long: String = core::iter::repeat('k').take(129).collect();
    say(lines, format!("long key: {}", unit(shared::set_text(&long, "x"), shared_name)));
    say(lines, format!("reads back: {}", list()));
    secrets(lines);
}

fn join(lines: &mut Vec<String>, code: &str) {
    say(lines, format!("made-up code: {}", unit(shared::join("nothere234"), shared_name)));
    say(lines, format!("short code: {}", unit(shared::join("abc"), shared_name)));
    match shared::join(code) {
        Ok(()) => say(lines, String::from("joined")),
        Err(e) => return say(lines, format!("join: {}", shared_name(&e))),
    }
    say(lines, format!("after join: {}", list()));
    say(lines, format!("delete eggs: {}", unit(shared::delete("eggs"), shared_name)));
    say(lines, format!("bread: {}", unit(shared::set_text("bread", "1"), shared_name)));
    say(lines, format!("edited: {}", list()));
}

fn reopen(lines: &mut Vec<String>) {
    say(lines, format!("kept here: {}", list()));
    say(lines, format!("leave: {}", unit(shared::leave(), shared_name)));
    let code = match shared::code() {
        Ok(Some(c)) => c,
        Ok(None) => "none".into(),
        Err(e) => shared_name(&e),
    };
    say(lines, format!("code after leave: {code}"));
    say(lines, format!("sync after leave: {}", match shared::sync() {
        Ok(b) => format!("ok {b}"),
        Err(e) => shared_name(&e),
    }));
    say(lines, format!("after leave: {}", list()));
}

impl krate::Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create(
            "Share Check",
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
        let _ = tree::upsert_node(win, &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas));
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

        let mode = krate::io::args::first().unwrap_or_default();
        let mut lines: Vec<String> = Vec::new();
        start(&mut lines);
        let syncing = if mode == "create" {
            create(&mut lines);
            true
        } else if let Some(code) = mode.strip_prefix("join:") {
            join(&mut lines, code);
            true
        } else if mode == "reopen" {
            reopen(&mut lines);
            false
        } else {
            false
        };
        say(&mut lines, String::from("ready"));

        let mut frame: u64 = 0;
        loop {
            if syncing && frame % 20 == 0 {
                match shared::sync() {
                    Ok(true) => say(&mut lines, format!("changed: {}", list())),
                    Ok(false) => {}
                    Err(e) => say(&mut lines, format!("sync error: {}", shared_name(&e))),
                }
            }
            frame += 1;

            let _ = canvas2d::clear(canvas, color(0.07, 0.09, 0.14));
            let ink = color(0.92, 0.94, 0.98);
            let _ = canvas2d::draw_text(canvas, "Share Check", gfx::Point { x: 16.0, y: 30.0 }, 20.0, ink);
            let from = lines.len().saturating_sub(11);
            for (i, l) in lines[from..].iter().enumerate() {
                let _ = canvas2d::draw_text(canvas, l, gfx::Point { x: 16.0, y: 56.0 + 18.0 * i as f32 }, 12.0, ink);
            }
            let _ = canvas2d::present(canvas);

            let mut next = events::wait(Some(16));
            let mut closed = false;
            while let Some(event) = next {
                match event {
                    types::Event::CloseRequested(_) => closed = true,
                    types::Event::Key(k) if k.pressed && syncing => {
                        say(&mut lines, format!("wrote jam: {}", unit(shared::set_text("jam", "1"), shared_name)));
                        say(&mut lines, format!("sync after jam: {}", match shared::sync() {
                            Ok(b) => format!("ok {b}"),
                            Err(e) => shared_name(&e),
                        }));
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
