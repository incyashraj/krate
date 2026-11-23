//! Ports diagrams/gantt/axis.ts: the time axis, following d3-time and
//! d3-time-format (English locale, local time), which Mermaid uses: the
//! `axisFormat` specifiers, tick intervals with d3's `every` rule, and its
//! choice of automatic ticks.

use alloc::string::String;
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::date::Date;
use super::dates::{MONTHS, WEEKDAYS};
use crate::pele::js::{self, jmax, jmin, num_str};

const SECOND: f64 = 1000.0;
const MINUTE: f64 = 60.0 * SECOND;
const HOUR: f64 = 60.0 * MINUTE;
const DAY: f64 = 24.0 * HOUR;
const WEEK: f64 = 7.0 * DAY;
const MONTH: f64 = 30.0 * DAY;
const YEAR: f64 = 365.0 * DAY;

/// Ports `type TickUnit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickUnit {
    Millisecond,
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

/// How `every` changed a calendar.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Every {
    Base,
    Millisecond(f64),
    Year(f64),
    /// The step and the floored epoch.
    Week(f64, f64),
    Field(f64),
}

/// Ports `interface Calendar`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Calendar {
    unit: TickUnit,
    week_start: f64,
    every: Every,
    cycle: f64,
    ms: f64,
}

fn start_of_day(d: &mut Date) {
    d.set_hours(0.0, 0.0, 0.0, 0.0);
}

impl Calendar {
    /// Ports `weekCalendar` and `CALENDARS[unit]`.
    fn base(unit: TickUnit, week_start: f64) -> Calendar {
        let (cycle, ms) = match unit {
            TickUnit::Millisecond => (f64::INFINITY, 1.0),
            TickUnit::Second => (60.0, SECOND),
            TickUnit::Minute => (60.0, MINUTE),
            TickUnit::Hour => (24.0, HOUR),
            TickUnit::Day => (31.0, DAY),
            TickUnit::Week => (f64::INFINITY, WEEK),
            TickUnit::Month => (12.0, MONTH),
            TickUnit::Year => (f64::INFINITY, YEAR),
        };
        Calendar {
            unit,
            week_start,
            every: Every::Base,
            cycle,
            ms,
        }
    }

    fn base_floor(&self, d: &mut Date) {
        match self.unit {
            TickUnit::Millisecond => {}
            TickUnit::Second => d.set_time(d.time() - d.milliseconds()),
            TickUnit::Minute => d.set_time(d.time() - d.milliseconds() - d.seconds() * SECOND),
            TickUnit::Hour => d.set_time(
                d.time() - d.milliseconds() - d.seconds() * SECOND - d.minutes() * MINUTE,
            ),
            TickUnit::Day => start_of_day(d),
            TickUnit::Week => {
                let back = (d.weekday() + 7.0 - self.week_start) % 7.0;
                d.set_date(d.date() - back);
                start_of_day(d);
            }
            TickUnit::Month => {
                d.set_date(1.0);
                start_of_day(d);
            }
            TickUnit::Year => {
                d.set_month(0.0, Some(1.0));
                start_of_day(d);
            }
        }
    }

    fn base_offset(&self, d: &mut Date, step: f64) {
        match self.unit {
            TickUnit::Millisecond => d.set_time(d.time() + step),
            TickUnit::Second => d.set_time(d.time() + step * SECOND),
            TickUnit::Minute => d.set_time(d.time() + step * MINUTE),
            TickUnit::Hour => d.set_time(d.time() + step * HOUR),
            TickUnit::Day => d.set_date(d.date() + step),
            TickUnit::Week => d.set_date(d.date() + step * 7.0),
            TickUnit::Month => d.set_month(d.month() + step, None),
            TickUnit::Year => d.set_full_year(d.full_year() + step),
        }
    }

    /// `field`: the value d3 tests against a step.
    fn field(&self, d: &Date) -> f64 {
        match self.unit {
            TickUnit::Second => d.seconds(),
            TickUnit::Minute => d.minutes(),
            TickUnit::Hour => d.hours(),
            TickUnit::Day => d.date() - 1.0,
            TickUnit::Month => d.month(),
            _ => f64::NAN,
        }
    }

    fn floor(&self, d: &mut Date) {
        match self.every {
            Every::Base => self.base_floor(d),
            Every::Millisecond(step) => d.set_time((d.time() / step).floor() * step),
            Every::Year(step) => {
                d.set_full_year((d.full_year() / step).floor() * step);
                self.base_floor(d);
            }
            Every::Week(step, epoch) => {
                self.base_floor(d);
                let back = -modulo(span(&Date(epoch), d, WEEK), step);
                self.base_offset(d, back);
            }
            Every::Field(step) => {
                self.base_floor(d);
                while d.time().is_finite() && self.field(d) % step != 0.0 {
                    d.set_time(d.time() - 1.0);
                    self.base_floor(d);
                }
            }
        }
    }

    fn offset(&self, d: &mut Date, n: f64) {
        match self.every {
            Every::Base => self.base_offset(d, n),
            Every::Millisecond(step) => d.set_time(d.time() + n * step),
            Every::Year(step) => d.set_full_year(d.full_year() + n * step),
            Every::Week(step, _) => self.base_offset(d, n * step),
            Every::Field(step) => {
                let mut i = 0.0;
                while i < n && d.time().is_finite() {
                    loop {
                        self.base_offset(d, 1.0);
                        if !(d.time().is_finite() && self.field(d) % step != 0.0) {
                            break;
                        }
                    }
                    i += 1.0;
                }
            }
        }
    }
}

/// Ports `span`: whole units between two floored dates, discounting a
/// change of UTC offset between them.
fn span(start: &Date, end: &Date, unit: f64) -> f64 {
    ((end.time() - start.time() - (end.timezone_offset() - start.timezone_offset()) * MINUTE)
        / unit)
        .floor()
}

/// Ports `mod`.
fn modulo(a: f64, b: f64) -> f64 {
    ((a % b) + b) % b
}

/// Ports `every`: the interval d3's `unit.every(step)` gives.
fn every(unit: TickUnit, step: f64, week_start: f64) -> Calendar {
    let mut base = Calendar::base(unit, week_start);
    if step <= 1.0 {
        return base;
    }
    base.every = match unit {
        TickUnit::Millisecond => Every::Millisecond(step),
        TickUnit::Year => Every::Year(step),
        TickUnit::Week => {
            let mut epoch = Date::new(0.0);
            base.base_floor(&mut epoch);
            Every::Week(step, epoch.time())
        }
        _ => Every::Field(step),
    };
    base
}

/// Ports `range`: ticks from start to stop inclusive, or None when there
/// would be more than `limit`.
fn range(calendar: &Calendar, start: f64, stop: f64, limit: f64) -> Option<Vec<f64>> {
    let mut d = Date::new(start - 1.0);
    calendar.floor(&mut d);
    calendar.offset(&mut d, 1.0);
    calendar.floor(&mut d);
    let mut out = Vec::new();
    let mut previous = f64::NEG_INFINITY;
    while d.time() <= stop && d.time() > previous {
        if out.len() as f64 >= limit {
            return None;
        }
        previous = d.time();
        out.push(previous);
        calendar.offset(&mut d, 1.0);
        calendar.floor(&mut d);
    }
    Some(out)
}

/// Ports `tickStep`.
fn tick_step(start: f64, stop: f64, count: f64) -> f64 {
    let step = (stop - start) / jmax(0.0, count);
    let power = step.log10().floor();
    let error = step / 10f64.powf(power);
    let factor = if error >= 50f64.sqrt() {
        10.0
    } else if error >= 10f64.sqrt() {
        5.0
    } else if error >= core::f64::consts::SQRT_2 {
        2.0
    } else {
        1.0
    };
    10f64.powf(power) * factor
}

const AUTO: [(TickUnit, f64, f64); 18] = [
    (TickUnit::Second, 1.0, SECOND),
    (TickUnit::Second, 5.0, 5.0 * SECOND),
    (TickUnit::Second, 15.0, 15.0 * SECOND),
    (TickUnit::Second, 30.0, 30.0 * SECOND),
    (TickUnit::Minute, 1.0, MINUTE),
    (TickUnit::Minute, 5.0, 5.0 * MINUTE),
    (TickUnit::Minute, 15.0, 15.0 * MINUTE),
    (TickUnit::Minute, 30.0, 30.0 * MINUTE),
    (TickUnit::Hour, 1.0, HOUR),
    (TickUnit::Hour, 3.0, 3.0 * HOUR),
    (TickUnit::Hour, 6.0, 6.0 * HOUR),
    (TickUnit::Hour, 12.0, 12.0 * HOUR),
    (TickUnit::Day, 1.0, DAY),
    (TickUnit::Day, 2.0, 2.0 * DAY),
    (TickUnit::Week, 1.0, WEEK),
    (TickUnit::Month, 1.0, MONTH),
    (TickUnit::Month, 3.0, 3.0 * MONTH),
    (TickUnit::Year, 1.0, YEAR),
];

/// Ports `autoTicks`: about `count` ticks at round times, chosen the way
/// d3's time scale chooses them.
pub fn auto_ticks(start: f64, stop: f64, count: f64) -> Vec<f64> {
    let target = (stop - start) / count;
    let mut i = 0;
    while i < AUTO.len() && AUTO[i].2 <= target {
        i += 1;
    }
    let calendar = if i == AUTO.len() {
        every(
            TickUnit::Year,
            jmax(1.0, tick_step(start / YEAR, stop / YEAR, count)),
            0.0,
        )
    } else if i == 0 {
        every(
            TickUnit::Millisecond,
            jmax(tick_step(start, stop, count), 1.0),
            0.0,
        )
    } else {
        let k = if target / AUTO[i - 1].2 < AUTO[i].2 / target {
            i - 1
        } else {
            i
        };
        every(AUTO[k].0, AUTO[k].1, 0.0)
    };
    range(&calendar, start, stop, 4.0 * count + 8.0).unwrap_or_default()
}

const WEEK_STARTS: [&str; 7] = [
    "sunday",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
];

/// Ports `intervalTicks`: ticks for a `tickInterval` such as `1week`, or
/// None when it is not one or asks for more than `limit`.
pub fn interval_ticks(
    interval: &str,
    weekday: &str,
    start: f64,
    stop: f64,
    limit: f64,
) -> Option<Vec<f64>> {
    // /^([1-9]\d*)(millisecond|second|minute|hour|day|week|month)$/
    let b = interval.as_bytes();
    if b.is_empty() || !(b'1'..=b'9').contains(&b[0]) {
        return None;
    }
    let mut k = 1;
    while k < b.len() && b[k].is_ascii_digit() {
        k += 1;
    }
    let unit = match &interval[k..] {
        "millisecond" => TickUnit::Millisecond,
        "second" => TickUnit::Second,
        "minute" => TickUnit::Minute,
        "hour" => TickUnit::Hour,
        "day" => TickUnit::Day,
        "week" => TickUnit::Week,
        "month" => TickUnit::Month,
        _ => return None,
    };
    let step = js::parse_int(&interval[..k], 10);
    let week_start = WEEK_STARTS
        .iter()
        .position(|w| *w == weekday)
        .map(|p| p as f64)
        .unwrap_or(0.0);
    let calendar = every(unit, step, week_start);
    if (stop - start) / (jmin(step, calendar.cycle) * calendar.ms) > limit {
        return None;
    }
    range(&calendar, start, stop, limit)
}

/// Ports `pad`.
fn pad(value: f64, fill: &str, width: usize) -> String {
    let sign = if value < 0.0 { "-" } else { "" };
    let digits = num_str(if sign.is_empty() { value } else { -value });
    let n = js::len16(&digits);
    let mut out = String::from(sign);
    if n < width {
        out.push_str(&fill.repeat(width - n));
    }
    out.push_str(&digits);
    out
}

/// Ports `floored`.
fn floored(date: &Date, calendar: &Calendar) -> Date {
    let mut d = *date;
    calendar.floor(&mut d);
    d
}

/// Ports `weekOfYear`: weeks since the last `weekStart` day before the
/// year began, which is d3's %U and %W.
fn week_of_year(date: &Date, week_start: f64) -> f64 {
    let week = Calendar::base(TickUnit::Week, week_start);
    let year_start = floored(date, &Calendar::base(TickUnit::Year, 0.0));
    span(
        &floored(&Date::new(year_start.time() - 1.0), &week),
        &floored(date, &week),
        WEEK,
    )
}

/// Ports `isoThursday`.
fn iso_thursday(date: &Date) -> Date {
    let thursday = Calendar::base(TickUnit::Week, 4.0);
    let day = date.weekday();
    let late = day >= 4.0 || day == 0.0;
    let mut d = floored(
        &if late {
            *date
        } else {
            Date::new(date.time() - 1.0)
        },
        &thursday,
    );
    if !late {
        thursday.offset(&mut d, 1.0);
        thursday.floor(&mut d);
    }
    d
}

/// Ports `timeFormat`: formats a date with a d3 time format specifier
/// such as `%Y-%m-%d`.
pub fn time_format(specifier: &str, date: &Date) -> String {
    let c: Vec<char> = specifier.chars().collect();
    let n = c.len();
    let mut out = String::new();
    let mut from = 0;
    let mut i = 0;
    while i < n {
        if c[i] != '%' {
            i += 1;
            continue;
        }
        out.extend(c[from..i].iter());
        i += 1;
        let mut ch = c.get(i).copied();
        let fill = match ch {
            Some('-') => Some(""),
            Some('_') => Some(" "),
            Some('0') => Some("0"),
            _ => None,
        };
        let fill = match fill {
            Some(f) => {
                i += 1;
                ch = c.get(i).copied();
                f
            }
            None => {
                if ch == Some('e') {
                    " "
                } else {
                    "0"
                }
            }
        };
        out.push_str(&directive(ch, date, fill));
        from = i + 1;
        i += 1;
    }
    if from < n {
        out.extend(c[from..].iter());
    }
    out
}

fn directive(c: Option<char>, d: &Date, p: &str) -> String {
    let Some(c) = c else {
        return String::new();
    };
    match c {
        'a' => WEEKDAYS[d.weekday() as usize][..3].into(),
        'A' => WEEKDAYS[d.weekday() as usize].into(),
        'b' => MONTHS[d.month() as usize][..3].into(),
        'B' => MONTHS[d.month() as usize].into(),
        'c' => time_format("%x, %X", d),
        'd' | 'e' => pad(d.date(), p, 2),
        'f' => pad(d.milliseconds(), p, 3) + "000",
        'g' => pad(iso_thursday(d).full_year() % 100.0, p, 2),
        'G' => pad(iso_thursday(d).full_year() % 10000.0, p, 4),
        'H' => pad(d.hours(), p, 2),
        'I' => pad(js::or(d.hours() % 12.0, 12.0), p, 2),
        'j' => pad(
            1.0 + span(
                &floored(d, &Calendar::base(TickUnit::Year, 0.0)),
                &floored(d, &Calendar::base(TickUnit::Day, 0.0)),
                DAY,
            ),
            p,
            3,
        ),
        'L' => pad(d.milliseconds(), p, 3),
        'm' => pad(d.month() + 1.0, p, 2),
        'M' => pad(d.minutes(), p, 2),
        'p' => (if d.hours() >= 12.0 { "PM" } else { "AM" }).into(),
        'q' => num_str(1.0 + (d.month() / 3.0).floor()),
        'Q' => num_str(d.time()),
        's' => num_str((d.time() / 1000.0).floor()),
        'S' => pad(d.seconds(), p, 2),
        'u' => num_str(js::or(d.weekday(), 7.0)),
        'U' => pad(week_of_year(d, 0.0), p, 2),
        'V' => {
            let thursday = iso_thursday(d);
            let year_start = floored(&thursday, &Calendar::base(TickUnit::Year, 0.0));
            let first = floored(&year_start, &Calendar::base(TickUnit::Week, 4.0));
            pad(
                span(&first, &thursday, WEEK)
                    + if year_start.weekday() == 4.0 {
                        1.0
                    } else {
                        0.0
                    },
                p,
                2,
            )
        }
        'w' => num_str(d.weekday()),
        'W' => pad(week_of_year(d, 1.0), p, 2),
        'x' => time_format("%-m/%-d/%Y", d),
        'X' => time_format("%-I:%M:%S %p", d),
        'y' => pad(d.full_year() % 100.0, p, 2),
        'Y' => pad(d.full_year() % 10000.0, p, 4),
        'Z' => {
            let z = d.timezone_offset();
            let abs = z.abs();
            String::from(if z > 0.0 { "-" } else { "+" })
                + &pad((abs / 60.0).floor(), "0", 2)
                + &pad(abs % 60.0, "0", 2)
        }
        '%' => "%".into(),
        other => other.into(),
    }
}
