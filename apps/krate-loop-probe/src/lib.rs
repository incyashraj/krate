// The smallest command-line app that never ends, so Ctrl-C on a running
// app is proved through the binary a person runs (K-1181).
//
//   krate run loop-probe.krate -- spin    a loop that never calls the host
//   krate run loop-probe.krate -- sleep   a loop that sleeps in the host
//
// Either way it first prints "started" (flushed) and then "still here"
// with no newline and no flush, so a test sees whether the output written
// before the interrupt reaches the terminal.
#![no_std]
extern crate alloc;

use krate::{
    io::{args, stdio, streams::OutputStreamExt},
    time::sleep::sleep_millis,
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let mode = raw.split('\n').find(|w| !w.is_empty()).unwrap_or("spin");
        let out = stdio::stdout();
        let _ = out.write_line("started");
        let _ = out.flush();
        let _ = out.write_text("still here");
        if mode == "sleep" {
            loop {
                sleep_millis(50);
            }
        }
        let mut n: u64 = 0;
        loop {
            n = core::hint::black_box(n.wrapping_add(1));
        }
    }
}

krate::export!(Component);
