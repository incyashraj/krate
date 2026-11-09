// The smallest app that runs programs on the computer (krate:process,
// IC-977), so the capability is proved through the binary a person runs.
//
//   krate run process-probe.krate -- <check>
//
// Each check prints one line, so a test sees exactly what an app is told:
//
//   version     cargo --version, run to the end
//   undeclared  rustc, which the manifest does not name
//   path        /bin/sh, a path rather than a bare name
//   raw-cwd     cargo in /tmp, a folder on the computer rather than the app's
//   escape      cargo in picked/folder-1/../.., out of a folder by `..`
//   terminal    a terminal: type `echo hi`, read `hi`, resize, kill
//   orphan      a program the app forgets and leaves running as it exits;
//               prints its pid so a test can see it ended with the app
//   hold        starts a program, prints its pid, and keeps running, so a
//               test can stop Krate itself (Ctrl-C, kill) and see the
//               program end with it
#![no_std]
extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use krate::{
    io::{args, stdio, streams::OutputStreamExt},
    process::{self, Process, ProcessError},
    time::sleep::sleep_millis,
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let check = raw.split('\n').find(|w| !w.is_empty()).unwrap_or("version");
        let out = stdio::stdout();
        let say = |line: &str| out.write_line(line).is_ok() && out.flush().is_ok();
        let line = match check {
            "version" => match process::run("cargo", &["--version"], ".") {
                Ok(done) => {
                    let text = String::from_utf8_lossy(&done.stdout);
                    let first = text.lines().next().unwrap_or("").trim();
                    let first = first.split(' ').take(2).collect::<Vec<_>>().join(" ");
                    format!("version: {first} code={}", code(done.status.code))
                }
                Err(err) => format!("version: {}", refusal(&err)),
            },
            "undeclared" => start_line("undeclared", "rustc", "."),
            "path" => start_line("path", "/bin/sh", "."),
            "raw-cwd" => start_line("raw-cwd", "cargo", "/tmp"),
            "escape" => start_line("escape", "cargo", "picked/folder-1/../.."),
            "terminal" => terminal(),
            "orphan" => orphan(),
            "hold" => {
                let line = orphan();
                if !say(&line) {
                    return 20;
                }
                // Until Krate is stopped from outside (a minute at most).
                for _ in 0..600 {
                    sleep_millis(100);
                }
                String::from("hold: never stopped")
            }
            other => format!("unknown check {other}"),
        };
        if say(&line) {
            0
        } else {
            20
        }
    }
}

fn code(code: Option<i32>) -> String {
    match code {
        Some(c) => format!("{c}"),
        None => String::from("none"),
    }
}

fn kind(err: &ProcessError) -> &'static str {
    match err {
        ProcessError::NotDeclared(_) => "not-declared",
        ProcessError::NotFound(_) => "not-found",
        ProcessError::RefusedByPerson(_) => "refused-by-person",
        ProcessError::BadFolder(_) => "bad-folder",
        ProcessError::TooMany(_) => "too-many",
        ProcessError::UnknownHandle => "unknown-handle",
        ProcessError::Platform(_) => "platform",
    }
}

fn refusal(err: &ProcessError) -> String {
    format!("{}: {}", kind(err), process::words(err))
}

fn start_line(label: &str, program: &str, cwd: &str) -> String {
    match Process::start(program, &["--version"], cwd) {
        Ok(_) => format!("{label}: STARTED"),
        Err(err) => format!("{label}: {}", refusal(&err)),
    }
}

/// What a terminal's output says as lines of text: ESC [ ... and ESC ] ...
/// sequences and carriage returns dropped.
///
/// ESC ] is an operating-system command, ended by BEL or ESC \. Newer Ubuntu
/// has bash mark every command with one (systemd's OSC 3008 context), on the
/// same line as the command's output, so `hi` read as `]3008;start=...\hi`
/// (CI, 2026-11-09).
fn plain_lines(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // Parameters, then one final letter or `~`.
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() || d == '~' {
                        break;
                    }
                }
            } else if chars.peek() == Some(&']') {
                chars.next();
                while let Some(d) = chars.next() {
                    if d == '\u{7}' {
                        break;
                    }
                    if d == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    out.split('\n').map(|l| String::from(l.trim_end())).collect()
}

fn terminal() -> String {
    let term = match Process::terminal(".", 80, 24) {
        Ok(term) => term,
        Err(err) => return format!("terminal: {}", refusal(&err)),
    };
    if let Err(err) = term.write(b"echo hi\n") {
        return format!("terminal: write {}", refusal(&err));
    }
    let mut seen: Vec<u8> = Vec::new();
    let mut saw = false;
    for _ in 0..2000 {
        match term.read_stdout(65536) {
            Ok(out) => seen.extend_from_slice(&out.bytes),
            Err(err) => return format!("terminal: read {}", refusal(&err)),
        }
        // The command echoed as typed, then its output on a line of its own.
        // Shells wrap lines in control sequences (bash on Linux turns
        // bracketed paste off and back on around each command), so read
        // the screen's text: escapes and carriage returns removed.
        if plain_lines(&seen).iter().any(|line| line == "hi") {
            saw = true;
            break;
        }
        sleep_millis(10);
    }
    if !saw {
        // What the terminal did say, so a run on another system shows why.
        let said = String::from_utf8_lossy(&seen);
        let tail: String = said.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect();
        let ended = matches!(term.wait(0), Ok(Some(_)));
        return format!("terminal: NEVER SAW hi (ended={ended}, {} bytes: {:?})", seen.len(), tail);
    }
    if let Err(err) = term.resize(100, 30) {
        return format!("terminal: resize {}", refusal(&err));
    }
    if let Err(err) = term.kill() {
        return format!("terminal: kill {}", refusal(&err));
    }
    match term.wait(5_000) {
        Ok(Some(_)) => String::from("terminal: saw hi, resized, ended"),
        Ok(None) => String::from("terminal: STILL RUNNING after kill"),
        Err(err) => format!("terminal: wait {}", refusal(&err)),
    }
}

fn orphan() -> String {
    let child = match Process::start("sh", &["-c", "echo $$; exec sleep 300"], ".") {
        Ok(child) => child,
        Err(err) => return format!("orphan: {}", refusal(&err)),
    };
    let mut seen: Vec<u8> = Vec::new();
    for _ in 0..1000 {
        match child.read_stdout(64) {
            Ok(out) => seen.extend_from_slice(&out.bytes),
            Err(err) => return format!("orphan: {}", refusal(&err)),
        }
        if seen.contains(&b'\n') {
            break;
        }
        sleep_millis(5);
    }
    let pid = String::from_utf8_lossy(&seen).trim().into();
    // Forgotten, not dropped: the app leaves it running and exits, and only
    // the runtime can end it.
    core::mem::forget(child);
    let pid: String = pid;
    format!("orphan pid={pid}")
}

krate::export!(Component);
