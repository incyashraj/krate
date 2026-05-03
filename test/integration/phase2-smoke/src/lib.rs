#[allow(warnings)]
mod bindings;

use bindings::layer36::{
    fs::{files, types::OpenMode},
    io::stdio,
    locale::{format, info, types::NumberStyle},
    time::clock,
};
use bindings::Guest;

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let locale = info::current();
        let timezone = info::timezone();
        let formatted = format::format_number(12.5, NumberStyle::Decimal, &locale);
        let saw_wall_clock = clock::now_millis() > 0;
        let saw_monotonic_clock = clock::monotonic_nanos() > 0;

        let file = match files::open("phase2-smoke-input.txt", OpenMode::Read) {
            Ok(file) => file,
            Err(_) => return 20,
        };

        let bytes = match file.read(1024) {
            Ok(bytes) => bytes,
            Err(_) => return 21,
        };

        let input = match core::str::from_utf8(&bytes) {
            Ok(input) => input.trim(),
            Err(_) => return 22,
        };

        let stdout = stdio::stdout();
        if !write_line(&stdout, "phase2-smoke ok")
            || !write_pair(&stdout, "file", input)
            || !write_pair(&stdout, "locale", &locale.bcp47)
            || !write_pair(&stdout, "timezone", &timezone)
            || !write_pair(&stdout, "number", &formatted)
            || !write_pair(&stdout, "time-ok", bool_str(saw_wall_clock))
            || !write_pair(&stdout, "mono-ok", bool_str(saw_monotonic_clock))
            || stdout.flush().is_err()
        {
            return 23;
        }

        0
    }
}

fn write_line(stream: &bindings::layer36::io::streams::OutputStream, value: &str) -> bool {
    stream.write_all(value.as_bytes()).is_ok() && stream.write_all(b"\n").is_ok()
}

fn write_pair(
    stream: &bindings::layer36::io::streams::OutputStream,
    key: &str,
    value: &str,
) -> bool {
    stream.write_all(key.as_bytes()).is_ok()
        && stream.write_all(b"=").is_ok()
        && write_line(stream, value)
}

fn bool_str(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

bindings::export!(Component with_types_in bindings);
