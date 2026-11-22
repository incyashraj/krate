//! Dates in the formats people write in their settings (`YYYY-MM-DD`,
//! `dddd, MMMM Do YYYY`, `HH:mm`): the moment.js tokens daily notes and
//! templates use, so a vault's own format makes the same file names here.

use alloc::string::String;

/// A local date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct When {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
pub fn days_from_civil(y: i32, m: u8, d: u8) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day number.
pub fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    ((y + i64::from(m <= 2)) as i32, m, d)
}

impl When {
    pub fn date(year: i32, month: u8, day: u8) -> When {
        When {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }

    pub fn day_number(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    /// 0 is Sunday.
    pub fn weekday(&self) -> usize {
        (self.day_number() + 4).rem_euclid(7) as usize
    }

    pub fn plus_days(&self, n: i64) -> When {
        let (y, m, d) = civil_from_days(self.day_number() + n);
        When {
            year: y,
            month: m,
            day: d,
            ..*self
        }
    }

    /// Format with moment.js tokens. Text in `[brackets]` is kept as is.
    pub fn format(&self, fmt: &str) -> String {
        let mut out = String::new();
        let b = fmt.as_bytes();
        let mut i = 0;
        let two = |out: &mut String, n: u32| {
            if n < 10 {
                out.push('0');
            }
            crate::text::push_num(out, n as usize);
        };
        while i < b.len() {
            if b[i] == b'[' {
                if let Some(end) = fmt[i + 1..].find(']') {
                    out.push_str(&fmt[i + 1..i + 1 + end]);
                    i += end + 2;
                    continue;
                }
            }
            let run = b[i..].iter().take_while(|&&c| c == b[i]).count();
            let c = b[i];
            let used = match (c, run) {
                (b'Y', n) if n >= 4 => {
                    crate::text::push_num(&mut out, self.year.max(0) as usize);
                    4
                }
                (b'Y', 2 | 3) => {
                    two(&mut out, (self.year.rem_euclid(100)) as u32);
                    2
                }
                (b'M', n) if n >= 4 => {
                    out.push_str(MONTHS[usize::from(self.month.clamp(1, 12)) - 1]);
                    4
                }
                (b'M', 3) => {
                    out.push_str(&MONTHS[usize::from(self.month.clamp(1, 12)) - 1][..3]);
                    3
                }
                (b'M', 2) => {
                    two(&mut out, u32::from(self.month));
                    2
                }
                (b'M', 1) => {
                    crate::text::push_num(&mut out, usize::from(self.month));
                    1
                }
                (b'D', n) if n >= 2 => {
                    two(&mut out, u32::from(self.day));
                    2
                }
                (b'D', 1) => {
                    if b.get(i + 1) == Some(&b'o') {
                        crate::text::push_num(&mut out, usize::from(self.day));
                        out.push_str(match (self.day % 100, self.day % 10) {
                            (11..=13, _) => "th",
                            (_, 1) => "st",
                            (_, 2) => "nd",
                            (_, 3) => "rd",
                            _ => "th",
                        });
                        2
                    } else {
                        crate::text::push_num(&mut out, usize::from(self.day));
                        1
                    }
                }
                (b'd', n) if n >= 4 => {
                    out.push_str(DAYS[self.weekday()]);
                    4
                }
                (b'd', 3) => {
                    out.push_str(&DAYS[self.weekday()][..3]);
                    3
                }
                (b'd', 2) => {
                    out.push_str(&DAYS[self.weekday()][..2]);
                    2
                }
                (b'd', 1) => {
                    crate::text::push_num(&mut out, self.weekday());
                    1
                }
                (b'H', n) if n >= 2 => {
                    two(&mut out, u32::from(self.hour));
                    2
                }
                (b'H', 1) => {
                    crate::text::push_num(&mut out, usize::from(self.hour));
                    1
                }
                (b'h', n) => {
                    let h = match self.hour % 12 {
                        0 => 12,
                        h => h,
                    };
                    if n >= 2 {
                        two(&mut out, u32::from(h));
                        2
                    } else {
                        crate::text::push_num(&mut out, usize::from(h));
                        1
                    }
                }
                (b'm', n) if n >= 2 => {
                    two(&mut out, u32::from(self.minute));
                    2
                }
                (b'm', 1) => {
                    crate::text::push_num(&mut out, usize::from(self.minute));
                    1
                }
                (b's', n) if n >= 2 => {
                    two(&mut out, u32::from(self.second));
                    2
                }
                (b'A', _) => {
                    out.push_str(if self.hour < 12 { "AM" } else { "PM" });
                    1
                }
                (b'a', _) => {
                    out.push_str(if self.hour < 12 { "am" } else { "pm" });
                    1
                }
                _ => {
                    let ch = fmt[i..].chars().next().unwrap_or(' ');
                    out.push(ch);
                    ch.len_utf8()
                }
            };
            i += used;
        }
        out
    }
}

/// Fill a template: `{{title}}`, `{{date}}`, `{{time}}`, and
/// `{{date:FORMAT}}` / `{{time:FORMAT}}` with a format of their own.
pub fn fill_template(
    text: &str,
    title: &str,
    now: &When,
    date_fmt: &str,
    time_fmt: &str,
) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = after[..close].trim();
        let (name, fmt) = match inner.split_once(':') {
            Some((n, f)) => (n.trim(), Some(f.trim())),
            None => (inner, None),
        };
        match name.to_ascii_lowercase().as_str() {
            "title" => out.push_str(title),
            "date" => out.push_str(&now.format(fmt.unwrap_or(date_fmt))),
            "time" => out.push_str(&now.format(fmt.unwrap_or(time_fmt))),
            _ => {
                out.push_str("{{");
                out.push_str(&after[..close]);
                out.push_str("}}");
            }
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_numbers_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2026, 11, 22), 20_779);
        for z in [-800_000i64, -1, 0, 59, 11_016, 20_779, 400_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        assert_eq!(When::date(2026, 11, 22).weekday(), 0, "a Sunday");
        assert_eq!(When::date(2024, 2, 28).plus_days(1).day, 29);
    }

    #[test]
    fn moment_tokens() {
        let w = When {
            year: 2026,
            month: 3,
            day: 2,
            hour: 14,
            minute: 5,
            second: 9,
        };
        assert_eq!(w.format("YYYY-MM-DD"), "2026-03-02");
        assert_eq!(w.format("dddd, MMMM Do YYYY"), "Monday, March 2nd 2026");
        assert_eq!(w.format("DD.MM.YY ddd MMM"), "02.03.26 Mon Mar");
        assert_eq!(w.format("HH:mm:ss h A"), "14:05:09 2 PM");
        assert_eq!(w.format("[Week of] YYYY/M/D"), "Week of 2026/3/2");
    }

    #[test]
    fn templates_fill_their_fields() {
        let w = When {
            year: 2026,
            month: 11,
            day: 22,
            hour: 9,
            minute: 30,
            second: 0,
        };
        assert_eq!(
            fill_template(
                "# {{title}}\n{{date}} {{time}} {{date:dddd}} {{other}}",
                "Standup",
                &w,
                "YYYY-MM-DD",
                "HH:mm"
            ),
            "# Standup\n2026-11-22 09:30 Sunday {{other}}"
        );
    }
}
