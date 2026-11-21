// The smallest app that says where it is reached and announces itself on the
// network (krate:net/sockets, IC-990), so the calls are proved through the
// binary a person runs.
//
//   krate run announce-probe.krate -- <check> <port> [this-computer|local-network] [type]
//
// Each check prints one line, so a test sees exactly what an app is told:
//
//   where     listen, print every address the app is reached at
//   announce  listen, announce as <type> (default _http._tcp), print what
//             was announced, then close the listener
//   hold      listen and announce, then leave without closing, so a test
//             can see the run ending withdraw the announcement
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::{
    io::{args, stdio, streams::OutputStreamExt},
    net::listen::{self, ListenError, Listener, Scope},
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw_args = args::raw();
        let words: Vec<&str> = raw_args.split('\n').filter(|w| !w.is_empty()).collect();
        let check = words.first().copied().unwrap_or("where");
        let port: u16 = words.get(1).and_then(|p| p.parse().ok()).unwrap_or(8080);
        let scope = match words.get(2).copied() {
            Some("this-computer") => Scope::ThisComputer,
            _ => Scope::LocalNetwork,
        };
        let service_type = words.get(3).copied().unwrap_or("_http._tcp");
        let line = match Listener::open(port, scope) {
            Err(err) => format!("{check}: {}", refusal(&err)),
            Ok(listener) => match check {
                "where" => match listener.addresses() {
                    Ok(found) => {
                        let shown: Vec<String> = found
                            .iter()
                            .map(|a| format!("{}:{} {}", a.ip, a.port, a.url))
                            .collect();
                        format!("where: [{}]", shown.join(", "))
                    }
                    Err(err) => format!("where: {}", refusal(&err)),
                },
                "announce" | "hold" => match listener.announce(service_type) {
                    Ok(named) => {
                        let line =
                            format!("{check}: {} at {} {}", named.name, named.host, named.url);
                        if check == "hold" {
                            // Never closed by the app: the run ending
                            // withdraws the announcement.
                            core::mem::forget(listener);
                        } else {
                            drop(listener);
                        }
                        line
                    }
                    Err(err) => format!("{check}: {}", refusal(&err)),
                },
                other => format!("unknown check {other}"),
            },
        };
        let out = stdio::stdout();
        if out.write_line(&line).is_ok() && out.flush().is_ok() {
            0
        } else {
            20
        }
    }
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
