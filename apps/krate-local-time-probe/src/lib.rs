// The smallest app that reads the person's local time (krate:locale/info
// utc-offset, IC-991), so "today" is proved through the binary a person runs.
//
//   krate run local-time-probe.krate
//
// It prints one line, so a test sees exactly what an app is told:
//
//   tz=Europe/Zurich offset=120 local=2026-03-29 03:00:00 weekday=7 day=20541
//
// `day` is the local day number an app stores for "done today" and streaks.
#![no_std]
extern crate alloc;

use alloc::format;
use krate::{
    io::{stdio, streams::OutputStreamExt},
    locale, Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let now = locale::local_now();
        let line = format!(
            "tz={} offset={} local={:04}-{:02}-{:02} {:02}:{:02}:{:02} weekday={} day={}",
            locale::timezone(),
            now.offset_minutes,
            now.year,
            now.month,
            now.day,
            now.hour,
            now.minute,
            now.second,
            now.weekday,
            now.day_number,
        );
        let out = stdio::stdout();
        if out.write_line(&line).is_ok() && out.flush().is_ok() {
            0
        } else {
            20
        }
    }
}

krate::export!(Component);
