// The smallest app that serves the people on its network (krate:net/sockets
// listening, IC-987), so the capability is proved through the binary a
// person runs.
//
//   krate run serve-probe.krate -- <check> <port> [this-computer|local-network]
//
// Each check prints one line, so a test sees exactly what an app is told:
//
//   serve   listen, wait (up to 20 s) for one device, read its HTTP request,
//           answer with a small page, close, and say what was asked for
//   listen  listen, say so, close: what a refusal looks like
//   hold    listen and leave without closing, so a test can see the run
//           ending closed the port
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::{
    io::{args, stdio, streams::OutputStreamExt},
    net::listen::{self, Connection, ListenError, Listener, ReadResult, Scope},
    time, Guest,
};

struct Component;

const PAGE: &str = "<!doctype html><title>Krate</title><h1>Served by a Krate app</h1>";

impl Guest for Component {
    fn run() -> i32 {
        let raw_args = args::raw();
        let words: Vec<&str> = raw_args.split('\n').filter(|w| !w.is_empty()).collect();
        let check = words.first().copied().unwrap_or("serve");
        let port: u16 = words.get(1).and_then(|p| p.parse().ok()).unwrap_or(8080);
        let scope = match words.get(2).copied() {
            Some("local-network") => Scope::LocalNetwork,
            _ => Scope::ThisComputer,
        };
        let line = match check {
            "serve" => format!("serve: {}", serve(port, scope)),
            "listen" => match Listener::open(port, scope) {
                Ok(listener) => match listener.port() {
                    Ok(at) => format!("listen: listening on {at}"),
                    Err(err) => format!("listen: port {}", refusal(&err)),
                },
                Err(err) => format!("listen: {}", refusal(&err)),
            },
            "hold" => match Listener::open(port, scope) {
                Ok(listener) => {
                    // Never closed by the app: the run ending closes it.
                    core::mem::forget(listener);
                    String::from("hold: left open")
                }
                Err(err) => format!("hold: {}", refusal(&err)),
            },
            other => format!("unknown check {other}"),
        };
        let out = stdio::stdout();
        if out.write_line(&line).is_ok() && out.flush().is_ok() {
            0
        } else {
            20
        }
    }
}

fn serve(port: u16, scope: Scope) -> String {
    let listener = match Listener::open(port, scope) {
        Ok(listener) => listener,
        Err(err) => return refusal(&err),
    };
    // Twenty seconds for a device to arrive, asked from the loop.
    let mut conn: Option<Connection> = None;
    for _ in 0..2000 {
        match listener.accept() {
            Ok(Some(c)) => {
                conn = Some(c);
                break;
            }
            Ok(None) => time::sleep_millis(10),
            Err(err) => return format!("accept {}", refusal(&err)),
        }
    }
    let Some(conn) = conn else {
        return String::from("nobody came");
    };
    let peer = conn.peer().unwrap_or_default();
    let from = if peer.starts_with("127.0.0.1:") {
        "this computer"
    } else {
        "another device"
    };
    // The request: up to 8 KiB, until the blank line ends its head.
    let mut request: Vec<u8> = Vec::new();
    for _ in 0..1000 {
        match conn.read(4096) {
            ReadResult::Data(bytes) => request.extend_from_slice(&bytes),
            ReadResult::Pending => time::sleep_millis(5),
            ReadResult::Closed => break,
            ReadResult::Failed(_) => return String::from("the device went away"),
        }
        if request.windows(4).any(|w| w == b"\r\n\r\n") || request.len() > 8192 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&request);
    let asked = head.lines().next().unwrap_or("").trim();
    let mut parts = asked.split(' ');
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let answer = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{PAGE}",
        PAGE.len()
    );
    let mut rest = answer.as_bytes();
    while !rest.is_empty() {
        match conn.write(rest) {
            Ok(taken) => rest = &rest[taken as usize..],
            Err(_) => return String::from("the device went away"),
        }
        if !rest.is_empty() {
            time::sleep_millis(5);
        }
    }
    // Written and closed at once: the page still arrives.
    drop(conn);
    drop(listener);
    format!("served {method} {path} to {from}")
}

fn kind(err: &ListenError) -> &'static str {
    match err {
        ListenError::NotAllowed(_) => "not-allowed",
        ListenError::InUse(_) => "in-use",
        ListenError::BadPort(_) => "bad-port",
        ListenError::TooMany(_) => "too-many",
        ListenError::Unavailable(_) => "unavailable",
        ListenError::UnknownHandle => "unknown-handle",
        ListenError::Failed(_) => "failed",
    }
}

fn refusal(err: &ListenError) -> String {
    format!("{}: {}", kind(err), listen::words(err))
}

krate::export!(Component);
