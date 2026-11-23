//! Ports diagrams/gantt/dates.ts: the parts of dayjs 1.11 that Mermaid's
//! gantt code relies on, with the plugins it loads (custom parse format,
//! advanced format, ISO week) and the English locale: strict parsing
//! against a format, formatting, and calendar arithmetic, all in local
//! time (UTC here, see [`super::date`]).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

use super::date::{utc, Date};
use crate::pele::js::{self, jround, num_str};

pub const MONTHS: [&str; 12] = [
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
pub const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

const DAY: f64 = 86_400_000.0;
const WEEK: f64 = 7.0 * DAY;

/// Ports `type Unit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Ms,
    S,
    M,
    H,
    D,
    W,
    Month,
    Y,
}

/// Ports `isValid`.
pub fn is_valid(date: &Date) -> bool {
    date.is_valid()
}

/// Ports `pad`.
pub fn pad(value: f64, length: usize) -> String {
    let s = num_str(value);
    let n = js::len16(&s);
    if n >= length {
        s
    } else {
        "0".repeat(length - n) + &s
    }
}

/// Ports `ordinal`.
fn ordinal(n: f64) -> String {
    let s = ["th", "st", "nd", "rd"];
    let v = n % 100.0;
    let a = (v - 20.0) % 10.0;
    let suffix = if (0.0..4.0).contains(&a) && a.fract() == 0.0 {
        Some(s[a as usize])
    } else {
        None
    }
    .or(if (0.0..4.0).contains(&v) && v.fract() == 0.0 {
        Some(s[v as usize])
    } else {
        None
    })
    .unwrap_or(s[0]);
    format!("{}{}", num_str(n), suffix)
}

/// Ports `utcOffset`.
fn utc_offset(date: &Date) -> f64 {
    let offset = -jround(date.timezone_offset() / 15.0) * 15.0;
    if offset == 0.0 {
        0.0
    } else {
        offset
    }
}

/// Ports `zone`.
fn zone(date: &Date) -> String {
    let offset = utc_offset(date);
    let minutes = offset.abs();
    format!(
        "{}{}:{}",
        if offset >= 0.0 { "+" } else { "-" },
        pad((minutes / 60.0).floor(), 2),
        pad(minutes % 60.0, 2)
    )
}

/// Ports `isoWeekday`.
pub fn iso_weekday(date: &Date) -> f64 {
    js::or(date.weekday(), 7.0)
}

/// Ports `addDays`.
pub fn add_days(date: &Date, days: f64) -> Date {
    let mut next = *date;
    next.set_date(date.date() + days);
    next
}

/// Ports `daysInMonth`.
fn days_in_month(date: &Date) -> f64 {
    let mut last = Date::local(
        date.full_year(),
        date.month() + 1.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
    );
    last.set_hours(23.0, 59.0, 59.0, 999.0);
    last.date()
}

/// Ports `setCalendar`: sets the month or the year, keeping the day of
/// the month unless the new month is shorter.
fn set_calendar(date: &Date, apply: impl Fn(&mut Date)) -> Date {
    let mut next = *date;
    next.set_date(1.0);
    apply(&mut next);
    let d = js::jmin(date.date(), days_in_month(&next));
    next.set_date(d);
    next
}

/// Ports `add`.
pub fn add(date: &Date, amount: f64, unit: Unit) -> Date {
    match unit {
        Unit::Month => set_calendar(date, |d| d.set_month(date.month() + amount, None)),
        Unit::Y => set_calendar(date, |d| d.set_full_year(date.full_year() + amount)),
        Unit::D => add_days(date, jround(amount)),
        Unit::W => add_days(date, jround(7.0 * amount)),
        Unit::H => Date::new(date.time() + amount * 3_600_000.0),
        Unit::M => Date::new(date.time() + amount * 60000.0),
        Unit::S => Date::new(date.time() + amount * 1000.0),
        Unit::Ms => Date::new(date.time() + amount),
    }
}

/// Ports `weekThursday`.
fn week_thursday(date: &Date) -> Date {
    add_days(date, 4.0 - iso_weekday(date))
}

/// Ports `isoWeek`.
fn iso_week(date: &Date) -> f64 {
    let thursday = week_thursday(date);
    let jan1 = Date::local(thursday.full_year(), 0.0, 1.0, 0.0, 0.0, 0.0, 0.0);
    let weekday = iso_weekday(&jan1);
    let first = add_days(&jan1, 4.0 - weekday + if weekday > 4.0 { 7.0 } else { 0.0 });
    let zone_delta = (utc_offset(&first) - utc_offset(&thursday)) * 60000.0;
    let weeks = (thursday.time() - first.time() - zone_delta) / WEEK;
    (if weeks < 0.0 {
        js::or(weeks.ceil(), 0.0) + 0.0
    } else {
        weeks.floor()
    }) + 1.0
}

const DEFAULT_FORMAT: &str = "YYYY-MM-DDTHH:mm:ssZ";

/// Runs of the same character at `i` in `c`, at most `max` long.
fn run_of(c: &[char], i: usize, ch: char, max: usize) -> usize {
    let mut n = 0;
    while i + n < c.len() && c[i + n] == ch && n < max {
        n += 1;
    }
    n
}

/// `\[([^\]]+)]` at `i`: the end of the match.
fn bracket_literal(c: &[char], i: usize) -> Option<usize> {
    if c.get(i) != Some(&'[') {
        return None;
    }
    let mut j = i + 1;
    while j < c.len() && c[j] != ']' {
        j += 1;
    }
    if j < c.len() && j > i + 1 {
        Some(j + 1)
    } else {
        None
    }
}

/// `starts with word` at `i`.
fn word_at(c: &[char], i: usize, w: &str) -> bool {
    w.chars()
        .enumerate()
        .all(|(k, ch)| c.get(i + k) == Some(&ch))
}

/// The match of RE_ADVANCED at `i`: its length.
fn advanced_at(c: &[char], i: usize) -> Option<usize> {
    if let Some(e) = bracket_literal(c, i) {
        return Some(e - i);
    }
    for w in [
        "Q", "wo", "ww", "w", "WW", "W", "zzz", "z", "gggg", "GGGG", "Do", "X", "x",
    ] {
        if word_at(c, i, w) {
            return Some(w.chars().count());
        }
    }
    let k = run_of(c, i, 'k', 2);
    if k > 0 {
        return Some(k);
    }
    if c.get(i) == Some(&'S') {
        return Some(1);
    }
    None
}

/// The match of RE_FORMAT at `i`: its length, and whether it is a literal.
fn format_at(c: &[char], i: usize) -> Option<(usize, bool)> {
    if let Some(e) = bracket_literal(c, i) {
        return Some((e - i, true));
    }
    if word_at(c, i, "YYYY") {
        return Some((4, false));
    }
    if word_at(c, i, "YY") {
        return Some((2, false));
    }
    for (ch, max) in [('M', 4), ('D', 2), ('d', 4), ('H', 2), ('h', 2)] {
        let n = run_of(c, i, ch, max);
        if n > 0 {
            return Some((n, false));
        }
    }
    if matches!(c.get(i), Some('a') | Some('A')) {
        return Some((1, false));
    }
    for (ch, max) in [('m', 2), ('s', 2), ('Z', 2)] {
        let n = run_of(c, i, ch, max);
        if n > 0 {
            return Some((n, false));
        }
    }
    if word_at(c, i, "SSS") {
        return Some((3, false));
    }
    None
}

/// Ports `format`: None for the week-of-year and time zone name tokens,
/// which need dayjs plugins Mermaid does not load and so fail there.
pub fn format(date: &Date, template: &str) -> Option<String> {
    if !date.is_valid() {
        return Some("Invalid Date".into());
    }
    let s = if template.is_empty() {
        DEFAULT_FORMAT
    } else {
        template
    };
    if s == "YYYY-MM-DD" {
        return Some(iso_date(date));
    }
    let mut unsupported = false;
    let c: Vec<char> = s.chars().collect();
    let mut advanced = String::new();
    let mut i = 0;
    while i < c.len() {
        let Some(n) = advanced_at(&c, i) else {
            advanced.push(c[i]);
            i += 1;
            continue;
        };
        let m: String = c[i..i + n].iter().collect();
        let out = match m.as_str() {
            "Q" => num_str(((date.month() + 1.0) / 3.0).ceil()),
            "Do" => format!("[{}]", ordinal(date.date())),
            "GGGG" => num_str(week_thursday(date).full_year()),
            "W" => num_str(iso_week(date)),
            "WW" => pad(iso_week(date), 2),
            "k" => num_str(js::or(date.hours(), 24.0)),
            "kk" => pad(js::or(date.hours(), 24.0), 2),
            "X" => num_str((date.time() / 1000.0).floor()),
            "x" => num_str(date.time()),
            "gggg" | "wo" | "w" | "ww" | "z" | "zzz" => {
                unsupported = true;
                m.clone()
            }
            _ => m.clone(),
        };
        advanced.push_str(&out);
        i += n;
    }
    if unsupported {
        return None;
    }
    let hours = date.hours();
    let c: Vec<char> = advanced.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        let Some((n, literal)) = format_at(&c, i) else {
            out.push(c[i]);
            i += 1;
            continue;
        };
        if literal {
            out.extend(c[i + 1..i + n - 1].iter());
            i += n;
            continue;
        }
        let m: String = c[i..i + n].iter().collect();
        let h12 = js::or(hours % 12.0, 12.0);
        let piece = match m.as_str() {
            "YY" => {
                let y = num_str(date.full_year());
                let k = y.chars().count();
                y.chars().skip(k.saturating_sub(2)).collect()
            }
            "YYYY" => pad(date.full_year(), 4),
            "M" => num_str(date.month() + 1.0),
            "MM" => pad(date.month() + 1.0, 2),
            "MMM" => MONTHS[date.month() as usize][..3].to_string(),
            "MMMM" => MONTHS[date.month() as usize].to_string(),
            "D" => num_str(date.date()),
            "DD" => pad(date.date(), 2),
            "d" => num_str(date.weekday()),
            "dd" => WEEKDAYS[date.weekday() as usize][..2].to_string(),
            "ddd" => WEEKDAYS[date.weekday() as usize][..3].to_string(),
            "dddd" => WEEKDAYS[date.weekday() as usize].to_string(),
            "H" => num_str(hours),
            "HH" => pad(hours, 2),
            "h" => num_str(h12),
            "hh" => pad(h12, 2),
            "a" => (if hours < 12.0 { "am" } else { "pm" }).to_string(),
            "A" => (if hours < 12.0 { "AM" } else { "PM" }).to_string(),
            "m" => num_str(date.minutes()),
            "mm" => pad(date.minutes(), 2),
            "s" => num_str(date.seconds()),
            "ss" => pad(date.seconds(), 2),
            "SSS" => pad(date.milliseconds(), 3),
            "Z" => zone(date),
            _ => zone(date).replacen(':', "", 1),
        };
        out.push_str(&piece);
        i += n;
    }
    Some(out)
}

/// Ports `isoDate`.
pub fn iso_date(date: &Date) -> String {
    let year = date.full_year();
    let month = date.month() + 1.0;
    let day = date.date();
    format!(
        "{}{}{}{}{}",
        if (1000.0..=9999.0).contains(&year) {
            num_str(year)
        } else {
            pad(year, 4)
        },
        if month < 10.0 { "-0" } else { "-" },
        num_str(month),
        if day < 10.0 { "-0" } else { "-" },
        num_str(day)
    )
}

/// Ports `interface Fields`.
#[derive(Debug, Clone, Default)]
struct Fields {
    year: Option<f64>,
    month: Option<f64>,
    day: Option<f64>,
    day_given: bool,
    hours: Option<f64>,
    minutes: Option<f64>,
    seconds: Option<f64>,
    milliseconds: Option<f64>,
    week: Option<f64>,
    offset: Option<f64>,
    afternoon: Option<bool>,
}

/// The patterns readers match with (`MATCH_*`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Re {
    One,
    Two,
    Three,
    Four,
    OneToTwo,
    Signed,
    Offset,
    Word,
}

fn is_digit(c: char) -> bool {
    c.is_ascii_digit()
}

/// `re.exec(text)`: the first match, as a char range.
fn search(re: Re, c: &[char]) -> Option<(usize, usize)> {
    let n = c.len();
    let digits = |i: usize| -> usize {
        let mut j = i;
        while j < n && is_digit(c[j]) {
            j += 1;
        }
        j
    };
    for i in 0..n {
        let end = match re {
            Re::One => (is_digit(c[i])).then_some(i + 1),
            Re::Two => (digits(i) >= i + 2).then_some(i + 2),
            Re::Three => (digits(i) >= i + 3).then_some(i + 3),
            Re::Four => (digits(i) >= i + 4).then_some(i + 4),
            Re::OneToTwo => {
                let d = digits(i);
                if d == i {
                    None
                } else {
                    Some(i + (d - i).min(2))
                }
            }
            Re::Signed => {
                let s = if c[i] == '+' || c[i] == '-' { i + 1 } else { i };
                let d = digits(s);
                if d > s {
                    Some(d)
                } else {
                    None
                }
            }
            Re::Offset => {
                if (c[i] == '+' || c[i] == '-') && digits(i + 1) >= i + 3 {
                    let mut e = i + 3;
                    if e < n && c[e] == ':' {
                        e += 1;
                    }
                    if digits(e) >= e + 2 {
                        e += 2;
                    }
                    Some(e)
                } else if c[i] == 'Z' {
                    Some(i + 1)
                } else {
                    None
                }
            }
            Re::Word => {
                let d = digits(i);
                let mut e = d;
                while e < n
                    && !matches!(c[e], '-' | '_' | ':' | '/' | ',' | '(' | ')')
                    && !js::is_space(c[e])
                    && !is_digit(c[e])
                {
                    e += 1;
                }
                (e > d).then_some(e)
            }
        };
        if let Some(e) = end {
            return Some((i, e));
        }
    }
    None
}

/// One step of a compiled format (`Step`): a literal, or a reader.
#[derive(Debug, Clone)]
enum Step {
    Lit(String),
    Read(Re, String),
}

/// The reader for a token (`READERS.get`).
fn reader(token: &str) -> Option<Re> {
    Some(match token {
        "A" | "a" | "Do" | "MMM" | "MMMM" => Re::Word,
        "Q" | "S" => Re::One,
        "SS" | "DD" | "ww" | "MM" | "YY" => Re::Two,
        "SSS" => Re::Three,
        "s" | "ss" | "m" | "mm" | "H" | "h" | "HH" | "hh" | "D" | "w" | "M" => Re::OneToTwo,
        "Y" => Re::Signed,
        "YYYY" => Re::Four,
        "Z" | "ZZ" => Re::Offset,
        _ => return None,
    })
}

fn is_sep(ch: char) -> bool {
    matches!(ch, '-' | '_' | ':' | '/' | '.' | ',' | '(' | ')') || js::is_space(ch)
}

/// `template.match(RE_TOKENS)`.
fn format_tokens(template: &str) -> Vec<String> {
    let c: Vec<char> = template.chars().collect();
    let n = c.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        // (\[[^[]*\])
        if c[i] == '[' {
            let mut j = i + 1;
            while j < n && c[j] != '[' {
                j += 1;
            }
            // Backtrack to the last ']' before j.
            let mut k = j;
            while k > i + 1 && c[k - 1] != ']' {
                k -= 1;
            }
            if k > i + 1 && c[k - 1] == ']' {
                out.push(c[i..k].iter().collect());
                i = k;
                continue;
            }
        }
        // ([-_:/.,()\s]+)
        if is_sep(c[i]) {
            let mut j = i;
            while j < n && is_sep(c[j]) {
                j += 1;
            }
            out.push(c[i..j].iter().collect());
            i = j;
            continue;
        }
        let at = |w: &str| word_at(&c, i, w);
        let len = if at("A") || at("a") || at("Q") {
            1
        } else if at("YYYY") {
            4
        } else if at("YY") {
            2
        } else if at("Y") {
            1
        } else if c[i] == 'w' {
            run_of(&c, i, 'w', 2)
        } else if c[i] == 'M' {
            run_of(&c, i, 'M', 4)
        } else if at("Do") {
            2
        } else if c[i] == 'D' {
            run_of(&c, i, 'D', 2)
        } else if c[i] == 'h' {
            run_of(&c, i, 'h', 2)
        } else if c[i] == 'H' {
            run_of(&c, i, 'H', 2)
        } else if c[i] == 'm' {
            run_of(&c, i, 'm', 2)
        } else if c[i] == 's' {
            run_of(&c, i, 's', 2)
        } else if c[i] == 'S' {
            run_of(&c, i, 'S', 3)
        } else if c[i] == 'z' {
            1
        } else if c[i] == 'Z' {
            run_of(&c, i, 'Z', 2)
        } else {
            0
        };
        if len > 0 {
            out.push(c[i..i + len].iter().collect());
            i += len;
        } else {
            i += 1;
        }
    }
    out
}

/// `template.replace(RE_LITERAL, '')`: /\[[^\]]+]/g.
fn without_literals(template: &str) -> String {
    let c: Vec<char> = template.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if let Some(e) = bracket_literal(&c, i) {
            i = e;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// Ports `compile`.
fn compile(template: &str) -> Option<Vec<Step>> {
    // dayjs expands localized tokens through a locale table the English locale does not have, and fails.
    if without_literals(template).contains(['L', 'l']) {
        return None;
    }
    let tokens = format_tokens(template);
    if tokens.is_empty() {
        return None;
    }
    Some(
        tokens
            .into_iter()
            .map(|token| match reader(&token) {
                Some(re) => Step::Read(re, token),
                None => {
                    let mut t = token.as_str();
                    if let Some(r) = t.strip_prefix('[') {
                        t = r;
                    }
                    if let Some(r) = t.strip_suffix(']') {
                        t = r;
                    }
                    Step::Lit(t.into())
                }
            })
            .collect(),
    )
}

/// `+input`.
fn plus(input: &str) -> f64 {
    js::number(input)
}

/// Ports `readOffset`.
fn read_offset(f: &mut Fields, input: &str) {
    if input == "Z" {
        f.offset = Some(0.0);
        return;
    }
    // input.match(/([+-]|\d\d)/g)
    let c: Vec<char> = input.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '+' || c[i] == '-' {
            parts.push(c[i].to_string());
            i += 1;
        } else if i + 1 < c.len() && is_digit(c[i]) && is_digit(c[i + 1]) {
            parts.push(c[i..i + 2].iter().collect());
            i += 2;
        } else {
            i += 1;
        }
    }
    let p1 = parts.get(1).map(|s| plus(s)).unwrap_or(f64::NAN);
    let p2 = parts.get(2).map(|s| plus(s)).unwrap_or(f64::NAN);
    let minutes = p1 * 60.0 + js::or(p2, 0.0);
    f.offset = Some(if minutes == 0.0 {
        0.0
    } else if parts[0] == "+" {
        -minutes
    } else {
        minutes
    });
}

/// Runs a reader on its matched text (`step.read`).
fn read(token: &str, f: &mut Fields, input: &str) -> bool {
    match token {
        "A" => f.afternoon = Some(input == "PM"),
        "a" => f.afternoon = Some(input == "pm"),
        "Q" => f.month = Some((plus(input) - 1.0) * 3.0 + 1.0),
        "S" => f.milliseconds = Some(plus(input) * 100.0),
        "SS" => f.milliseconds = Some(plus(input) * 10.0),
        "SSS" => f.milliseconds = Some(plus(input)),
        "s" | "ss" => f.seconds = Some(plus(input)),
        "m" | "mm" => f.minutes = Some(plus(input)),
        "H" | "h" | "HH" | "hh" => f.hours = Some(plus(input)),
        "D" | "DD" => f.day = Some(plus(input)),
        "Do" => {
            let c: Vec<char> = input.chars().collect();
            let Some(s) = c.iter().position(|ch| is_digit(*ch)) else {
                return false;
            };
            let mut e = s;
            while e < c.len() && is_digit(c[e]) {
                e += 1;
            }
            let digits: String = c[s..e].iter().collect();
            // dayjs keeps the digits as a string here, so even "0" counts as a day having been given.
            f.day = Some(plus(&digits));
            f.day_given = true;
            for i in 1..=31 {
                if ordinal(i as f64) == input {
                    f.day = Some(i as f64);
                }
            }
        }
        "w" | "ww" => f.week = Some(plus(input)),
        "M" | "MM" => f.month = Some(plus(input)),
        "MMM" | "MMMM" => {
            let index = if token == "MMM" {
                MONTHS.iter().position(|m| &m[..3] == input)
            } else {
                MONTHS.iter().position(|m| *m == input)
            };
            let Some(index) = index else {
                return false;
            };
            let index = (index + 1) as f64;
            f.month = Some(js::or(index % 12.0, index));
        }
        "Y" | "YYYY" => f.year = Some(plus(input)),
        "YY" => {
            let v = plus(input);
            f.year = Some(v + if v > 68.0 { 1900.0 } else { 2000.0 });
        }
        "Z" | "ZZ" => read_offset(f, input),
        _ => {}
    }
    true
}

/// `x || y` for an optional number.
fn or_else(v: Option<f64>, y: f64) -> f64 {
    match v {
        Some(x) if x != 0.0 && !x.is_nan() => x,
        _ => y,
    }
}

fn truthy(v: Option<f64>) -> bool {
    matches!(v, Some(x) if x != 0.0 && !x.is_nan())
}

/// Ports `loose`.
fn loose(input: &str, template: &str, now: &Date) -> Option<Date> {
    if template == "x" || template == "X" {
        let k = if template == "X" { 1000.0 } else { 1.0 };
        return Some(Date::new(k * js::number(input)));
    }
    let steps = compile(template)?;
    let mut f = Fields::default();
    let mut rest: String = input.into();
    let mut start = 0usize;
    for step in &steps {
        match step {
            Step::Lit(s) => start += js::len16(s),
            Step::Read(re, token) => {
                // rest.slice(start), in UTF-16 units.
                let from = js::byte_at(&rest, start);
                let tail: Vec<char> = rest[from..].chars().collect();
                let (s, e) = search(*re, &tail)?;
                let matched: String = tail[s..e].iter().collect();
                if !read(token, &mut f, &matched) {
                    return None;
                }
                // dayjs drops the first occurrence of the matched text, wherever it is, and does not move `start`.
                rest = js::replace_first(&rest, &matched, "");
            }
        }
    }
    if let (Some(afternoon), Some(hours)) = (f.afternoon, f.hours) {
        if afternoon {
            if hours < 12.0 {
                f.hours = Some(hours + 12.0);
            }
        } else if hours == 12.0 {
            f.hours = Some(0.0);
        }
    }
    let day = if f.day_given || truthy(f.day) {
        f.day.unwrap_or(f64::NAN)
    } else if !truthy(f.year) && !truthy(f.month) {
        now.date()
    } else {
        1.0
    };
    let year = or_else(f.year, now.full_year());
    let mut month = 0.0;
    if !truthy(f.year) || truthy(f.month) {
        month = match f.month {
            Some(m) if m > 0.0 => m - 1.0,
            _ => now.month(),
        };
    }
    let hours = or_else(f.hours, 0.0);
    let minutes = or_else(f.minutes, 0.0);
    let seconds = or_else(f.seconds, 0.0);
    let ms = or_else(f.milliseconds, 0.0);
    if let Some(offset) = f.offset {
        return Some(Date::new(utc(
            year,
            month,
            day,
            hours,
            minutes,
            seconds,
            ms + offset * 60000.0,
        )));
    }
    // Applying a week number needs a dayjs plugin Mermaid does not load.
    if truthy(f.week) {
        return None;
    }
    Some(Date::local(year, month, day, hours, minutes, seconds, ms))
}

/// Ports `parseStrict`: parses the way `dayjs(input, format, true)` does.
/// `now` supplies the fields the format leaves out.
pub fn parse_strict(input: &str, template: &str, now: &Date) -> Option<Date> {
    let date = loose(input, template, now)?;
    if date.is_valid() && format(&date, template).as_deref() == Some(input) {
        Some(date)
    } else {
        None
    }
}
