// The smallest app that talks to a serial device the person picks
// (krate:serial, IC-985), so the capability is proved through the binary a
// person runs.
//
//   krate run serial-probe.krate -- <check>
//
// Each check prints one line, so a test sees exactly what an app is told:
//
//   pick    ask for a device and say what came back
//   talk    pick, open at 115200 8N1, write PING, read until PONG, then a
//           read with nothing waiting (empty when its wait runs out), then
//           close and see the handle is spent
//   forge   pick, then try handles the person never gave: the next number
//           and 0 reach nothing, and the pick itself is not open yet
//   bad     open with a baud rate of 0: refused in words
//   hold    pick, open, write HELD, and leave without closing, so a test can
//           see the run ending closed it
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::{
    bindings::krate::serial::ports as raw,
    io::{args, stdio, streams::OutputStreamExt},
    serial::{self, Port, SerialError, Settings},
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw_args = args::raw();
        let check = raw_args
            .split('\n')
            .find(|w| !w.is_empty())
            .unwrap_or("pick");
        let line = match check {
            "pick" => match serial::pick("Choose the test device") {
                Ok(Some(port)) => format!("pick: picked {}", port.name()),
                Ok(None) => String::from("pick: none"),
                Err(err) => format!("pick: {}", refusal(&err)),
            },
            "talk" => with_port("talk", talk),
            "forge" => with_port("forge", forge),
            "bad" => with_port("bad", |port| {
                match port.open(&Settings::baud(0)) {
                    Ok(()) => String::from("OPENED"),
                    Err(err) => refusal(&err),
                }
            }),
            "hold" => with_port("hold", |port| {
                if let Err(err) = port.open(&Settings::baud(115_200)) {
                    return refusal(&err);
                }
                if let Err(err) = port.write(b"HELD\n") {
                    return refusal(&err);
                }
                // Never closed by the app: the run ending closes it.
                core::mem::forget(port);
                String::from("left open")
            }),
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

/// Pick, then hand the port to `then`; `label: ...` either way.
fn with_port(label: &str, then: impl FnOnce(Port) -> String) -> String {
    match serial::pick("Choose the test device") {
        Ok(Some(port)) => format!("{label}: {}", then(port)),
        Ok(None) => format!("{label}: none"),
        Err(err) => format!("{label}: {}", refusal(&err)),
    }
}

fn talk(port: Port) -> String {
    let settings = Settings {
        read_timeout_ms: 200,
        ..Settings::baud(115_200)
    };
    if let Err(err) = port.open(&settings) {
        return format!("open {}", refusal(&err));
    }
    if let Err(err) = port.write(b"PING\n") {
        return format!("write {}", refusal(&err));
    }
    let mut heard: Vec<u8> = Vec::new();
    // Up to 25 reads of 200 ms: five seconds for the answer.
    for _ in 0..25 {
        match port.read(64, None) {
            Ok(bytes) => heard.extend_from_slice(&bytes),
            Err(err) => return format!("read {}", refusal(&err)),
        }
        if heard.ends_with(b"PONG\n") {
            break;
        }
    }
    if !heard.ends_with(b"PONG\n") {
        return format!("heard {:?}", String::from_utf8_lossy(&heard));
    }
    let quiet = match port.read(64, Some(100)) {
        Ok(bytes) if bytes.is_empty() => "nothing for 100 ms",
        Ok(_) => "UNEXPECTED BYTES",
        Err(_) => "A QUIET READ FAILED",
    };
    let handle = port.handle();
    drop(port);
    let after = match raw::write(handle, b"x") {
        Ok(()) => String::from("STILL OPEN"),
        Err(err) => kind(&err).into(),
    };
    format!("heard PONG, then {quiet}, closed: {after}")
}

fn forge(port: Port) -> String {
    let h = port.handle();
    let unopened = match raw::read(h, 1, Some(0)) {
        Ok(_) => "READ",
        Err(err) => kind(&err),
    };
    let next = match raw::open(h + 1, Settings::baud(9_600)) {
        Ok(()) => "OPENED",
        Err(err) => kind(&err),
    };
    let zero = match raw::write(0, b"x") {
        Ok(()) => "WROTE",
        Err(err) => kind(&err),
    };
    format!("unopened={unopened} next={next} zero={zero}")
}

fn kind(err: &SerialError) -> &'static str {
    match err {
        SerialError::NotAllowed(_) => "not-allowed",
        SerialError::Unavailable(_) => "unavailable",
        SerialError::UnknownHandle => "unknown-handle",
        SerialError::NotOpen => "not-open",
        SerialError::BadSettings(_) => "bad-settings",
        SerialError::Busy(_) => "busy",
        SerialError::Disconnected(_) => "disconnected",
        SerialError::TooMany(_) => "too-many",
        SerialError::Failed(_) => "failed",
    }
}

fn refusal(err: &SerialError) -> String {
    format!("{}: {}", kind(err), serial::words(err))
}

krate::export!(Component);
