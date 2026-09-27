// Asks for a buffer of N MiB with `try_reserve_exact` and says what happened
// (K-416), so memory.budget is proved through the binary a person runs:
//
//   krate run krate-memory-probe.krate -- 400
//
// prints `held=400` or `refused=400`. Never `crashed`: since K-395 the host
// answers an over-budget growth with "could not grow", which try_reserve
// turns into an error the app can report.
#![no_std]
extern crate alloc;

use alloc::vec::Vec;
use krate::{
    io::{
        args, stdio,
        streams::{OutputStream, OutputStreamExt},
    },
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let stdout = stdio::stdout();
        let Some(mib) = raw
            .split('\n')
            .find(|word| !word.is_empty())
            .and_then(|word| word.parse::<usize>().ok())
        else {
            return if say(&stdout, "usage: <MiB>") { 2 } else { 20 };
        };
        let mut buffer: Vec<u8> = Vec::new();
        let outcome = match buffer.try_reserve_exact(mib * 1024 * 1024) {
            Ok(()) => "held=",
            Err(_) => "refused=",
        };
        let mut line = alloc::string::String::from(outcome);
        line.push_str(raw.split('\n').find(|word| !word.is_empty()).unwrap_or(""));
        if say(&stdout, &line) {
            0
        } else {
            20
        }
    }
}

fn say(stream: &OutputStream, line: &str) -> bool {
    stream.write_line(line).is_ok() && stream.flush().is_ok()
}

krate::export!(Component);
