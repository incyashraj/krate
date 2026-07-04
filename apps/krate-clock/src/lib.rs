use krate::{
    io::{
        stdio,
        streams::{OutputStream, OutputStreamExt},
    },
    locale::{self, DateStyle},
    time,
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let millis = time::now_millis();
        let locale = locale::current();
        let timezone = locale::timezone();
        let date = locale::format_date(millis, &timezone, DateStyle::Medium, &locale);

        let stdout = stdio::stdout();
        if !write_pair(&stdout, "app", "krate-clock")
            || !write_pair(&stdout, "timezone", &timezone)
            || !write_pair(&stdout, "locale", &locale.bcp47)
            || !write_pair(&stdout, "date", &date)
            || stdout.flush().is_err()
        {
            return 20;
        }

        0
    }
}

fn write_line(stream: &OutputStream, value: &str) -> bool {
    stream.write_line(value).is_ok()
}

fn write_pair(stream: &OutputStream, key: &str, value: &str) -> bool {
    stream.write_text(key).is_ok() && stream.write_text("=").is_ok() && write_line(stream, value)
}

krate::export!(Component);
