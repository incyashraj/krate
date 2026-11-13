//! Live Check -- the browser Player's live-connection proof (IC-974,
//! crates/adapter-web/player/ws-proof.mjs).
//!
//! Every path of krate:net/ws a multiplayer game uses, in one small window:
//!
//! - a connection to the granted echo server, a text and a binary message
//!   sent before the handshake finishes (queued, as on desktop), both
//!   echoed back in order, then a polite close that reports `closed`;
//! - a connection to a host the app was never granted, refused at `open`
//!   with `permission-denied` before anything leaves the tab;
//! - a connection to the granted port with nothing listening, which opens
//!   a handle and then reports `failed`.
//!
//! Each outcome is a line on stdout (the proof reads it) and on the canvas.

#![no_std]
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use krate::gfx::{canvas2d, types as gfx};
use krate::io::stdio;
use krate::net::ws::{self, WsEvent, WsMessage};
use krate::net::NetError;
use krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;
const WIDTH: f32 = 320.0;
const HEIGHT: f32 = 200.0;
const ECHO: &str = "ws://127.0.0.1:8935/echo";
/// The same port, a path the proof's server hangs up on before the handshake.
const DEAD: &str = "ws://127.0.0.1:8935/refuse";
const ELSEWHERE: &str = "wss://example.com/";
const BYTES: [u8; 5] = [1, 2, 3, 250, 0];

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

fn digits(mut n: u64) -> String {
    let mut d = Vec::new();
    loop {
        d.push(b'0' + (n % 10) as u8);
        n /= 10;
        if n == 0 {
            break;
        }
    }
    d.reverse();
    String::from_utf8(d).unwrap_or_default()
}

fn error_name(e: &NetError) -> &'static str {
    match e {
        NetError::InvalidUrl => "invalid-url",
        NetError::DnsFailure(_) => "dns-failure",
        NetError::ConnectFailure(_) => "connect-failure",
        NetError::TlsFailure(_) => "tls-failure",
        NetError::Timeout => "timeout",
        NetError::BodyTooLarge => "body-too-large",
        NetError::PermissionDenied => "permission-denied",
        NetError::Protocol(_) => "protocol",
        NetError::Other(_) => "other",
    }
}

/// One outcome: printed for the proof, kept for the canvas.
fn say(lines: &mut Vec<String>, text: String) {
    let _ = stdio::println(&text);
    lines.push(text);
}

impl krate::Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create(
            "Live Check",
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

        let mut lines: Vec<String> = Vec::new();

        // A host the app was never granted: refused before anything leaves.
        match ws::open(ELSEWHERE) {
            Ok(_) => say(&mut lines, String::from("elsewhere: opened")),
            Err(e) => say(&mut lines, String::from("elsewhere: refused ") + error_name(&e)),
        }

        // The echo server: two messages sent before the handshake is done.
        let mut echo = match ws::open(ECHO) {
            Ok(h) => Some(h),
            Err(e) => {
                say(&mut lines, String::from("echo: refused ") + error_name(&e));
                None
            }
        };
        if let Some(h) = echo {
            if let Err(e) = ws::send_text(h, "hello from a tab") {
                say(&mut lines, String::from("echo: send text failed ") + error_name(&e));
            }
            if let Err(e) = ws::send_binary(h, &BYTES) {
                say(&mut lines, String::from("echo: send binary failed ") + error_name(&e));
            }
        }
        let mut echoed = 0;
        let mut dead = match ws::open(DEAD) {
            Ok(h) => Some(h),
            Err(e) => {
                say(&mut lines, String::from("dead: refused ") + error_name(&e));
                None
            }
        };

        let mut frames: u64 = 0;
        loop {
            if let Some(h) = echo {
                loop {
                    match ws::poll(h) {
                        WsEvent::Pending => break,
                        WsEvent::Opened => say(&mut lines, String::from("echo: opened")),
                        WsEvent::Message(WsMessage::Text(t)) => {
                            say(&mut lines, String::from("echo: text ") + &t);
                            echoed += 1;
                        }
                        WsEvent::Message(WsMessage::Binary(b)) => {
                            let mut s = String::from("echo: binary");
                            for v in b {
                                s.push(' ');
                                s.push_str(&digits(v as u64));
                            }
                            say(&mut lines, s);
                            echoed += 1;
                        }
                        WsEvent::Closed => {
                            say(&mut lines, String::from("echo: closed"));
                            echo = None;
                            break;
                        }
                        WsEvent::Failed(why) => {
                            say(&mut lines, String::from("echo: failed ") + &why);
                            echo = None;
                            break;
                        }
                        WsEvent::UnknownHandle => {
                            say(&mut lines, String::from("echo: unknown handle"));
                            echo = None;
                            break;
                        }
                    }
                    if echoed == 2 {
                        echoed += 1;
                        ws::close(h);
                    }
                }
            }
            if let Some(h) = dead {
                match ws::poll(h) {
                    WsEvent::Pending => {}
                    WsEvent::Opened => say(&mut lines, String::from("dead: opened")),
                    WsEvent::Failed(_) => {
                        say(&mut lines, String::from("dead: failed"));
                        dead = None;
                    }
                    WsEvent::Closed => {
                        say(&mut lines, String::from("dead: closed"));
                        dead = None;
                    }
                    _ => {
                        say(&mut lines, String::from("dead: other"));
                        dead = None;
                    }
                }
            }

            let _ = canvas2d::clear(canvas, color(0.07, 0.09, 0.14));
            let ink = color(0.92, 0.94, 0.98);
            let _ = canvas2d::draw_text(canvas, "Live Check", gfx::Point { x: 16.0, y: 30.0 }, 20.0, ink);
            for (i, l) in lines.iter().enumerate() {
                let _ = canvas2d::draw_text(canvas, l, gfx::Point { x: 16.0, y: 56.0 + 18.0 * i as f32 }, 12.0, ink);
            }
            let _ = canvas2d::present(canvas);
            frames += 1;
            let _ = frames;

            let mut next = events::wait(Some(16));
            let mut closed = false;
            while let Some(event) = next {
                if let types::Event::CloseRequested(_) = event {
                    closed = true;
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
