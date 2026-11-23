//! JavaScript's `Date`, as gantt charts use it: a time value in
//! milliseconds (NaN for an invalid date) with the ECMAScript calendar
//! algorithms, and `new Date(string)` as V8 parses it
//! (src/date/dateparser*.{h,cc} in V8).
//!
//! Pele works in the local time of the page. The port has no time zone
//! database and takes local time to be UTC; the oracle is made with
//! `TZ=UTC`.

use alloc::vec::Vec;
#[cfg(target_arch = "wasm32")]
#[allow(unused_imports)]
use krate::prelude::*;

const MS_PER_DAY: f64 = 86_400_000.0;

/// A `Date`: its time value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Date(pub f64);

/// `ToIntegerOrInfinity`.
fn integer(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x.trunc() + 0.0
    }
}

/// `Day(t)`.
fn day(t: f64) -> f64 {
    (t / MS_PER_DAY).floor()
}

/// `TimeWithinDay(t)`.
fn time_within_day(t: f64) -> f64 {
    t.rem_euclid(MS_PER_DAY)
}

/// Days from 1970-01-01 to the first of a month (`month` 1-12) of a year.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Year, month (0-11) and date (1-31) of a day number.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m - 1, d)
}

/// `MakeTime`.
pub fn make_time(h: f64, m: f64, s: f64, ms: f64) -> f64 {
    if !(h.is_finite() && m.is_finite() && s.is_finite() && ms.is_finite()) {
        return f64::NAN;
    }
    integer(h) * 3_600_000.0 + integer(m) * 60_000.0 + integer(s) * 1000.0 + integer(ms)
}

/// `MakeDay`.
pub fn make_day(year: f64, month: f64, date: f64) -> f64 {
    if !(year.is_finite() && month.is_finite() && date.is_finite()) {
        return f64::NAN;
    }
    let y = integer(year);
    let m = integer(month);
    let dt = integer(date);
    let ym = y + (m / 12.0).floor();
    if ym.abs() > 1_000_000.0 {
        return f64::NAN;
    }
    let mn = m.rem_euclid(12.0);
    days_from_civil(ym as i64, mn as i64 + 1, 1) as f64 + dt - 1.0
}

/// `MakeDate`.
pub fn make_date(day: f64, time: f64) -> f64 {
    if !(day.is_finite() && time.is_finite()) {
        return f64::NAN;
    }
    let tv = day * MS_PER_DAY + time;
    if !tv.is_finite() {
        f64::NAN
    } else {
        tv
    }
}

/// `TimeClip`.
pub fn time_clip(t: f64) -> f64 {
    if !t.is_finite() || t.abs() > 8.64e15 {
        return f64::NAN;
    }
    integer(t)
}

/// The year a `Date(y, ...)` or `Date.UTC(y, ...)` call means: 0 to 99 is 1900 to 1999.
fn full_year(y: f64) -> f64 {
    if y.is_nan() {
        return y;
    }
    let yi = integer(y);
    if (0.0..=99.0).contains(&yi) {
        1900.0 + yi
    } else {
        y
    }
}

/// `Date.UTC(y, m, d, h, mi, s, ms)`.
pub fn utc(y: f64, m: f64, d: f64, h: f64, mi: f64, s: f64, ms: f64) -> f64 {
    time_clip(make_date(
        make_day(full_year(y), m, d),
        make_time(h, mi, s, ms),
    ))
}

impl Date {
    /// `new Date(t)` for a number.
    pub fn new(t: f64) -> Date {
        Date(time_clip(t))
    }

    /// `new Date(y, m, d, h, mi, s, ms)`, in local time (UTC here).
    pub fn local(y: f64, m: f64, d: f64, h: f64, mi: f64, s: f64, ms: f64) -> Date {
        Date(utc(y, m, d, h, mi, s, ms))
    }

    /// `+date`.
    pub fn time(&self) -> f64 {
        self.0
    }

    pub fn is_valid(&self) -> bool {
        !self.0.is_nan()
    }

    fn civil(&self) -> (f64, f64, f64) {
        if self.0.is_nan() {
            return (f64::NAN, f64::NAN, f64::NAN);
        }
        let (y, m, d) = civil_from_days(day(self.0) as i64);
        (y as f64, m as f64, d as f64)
    }

    pub fn full_year(&self) -> f64 {
        self.civil().0
    }

    pub fn month(&self) -> f64 {
        self.civil().1
    }

    pub fn date(&self) -> f64 {
        self.civil().2
    }

    /// `getDay`.
    pub fn weekday(&self) -> f64 {
        if self.0.is_nan() {
            return f64::NAN;
        }
        (day(self.0) + 4.0).rem_euclid(7.0)
    }

    pub fn hours(&self) -> f64 {
        (time_within_day(self.0) / 3_600_000.0).floor()
    }

    pub fn minutes(&self) -> f64 {
        (time_within_day(self.0) / 60_000.0).floor() % 60.0
    }

    pub fn seconds(&self) -> f64 {
        (time_within_day(self.0) / 1000.0).floor() % 60.0
    }

    pub fn milliseconds(&self) -> f64 {
        time_within_day(self.0) % 1000.0
    }

    /// `getTimezoneOffset`: 0 in UTC, NaN for an invalid date.
    pub fn timezone_offset(&self) -> f64 {
        if self.0.is_nan() {
            f64::NAN
        } else {
            0.0
        }
    }

    /// `setTime`.
    pub fn set_time(&mut self, t: f64) {
        self.0 = time_clip(t);
    }

    /// `setDate`.
    pub fn set_date(&mut self, d: f64) {
        if self.0.is_nan() {
            return;
        }
        let t = self.0;
        let (y, m, _) = self.civil();
        self.0 = time_clip(make_date(make_day(y, m, d), time_within_day(t)));
    }

    /// `setMonth(m)`, or `setMonth(m, d)`.
    pub fn set_month(&mut self, m: f64, d: Option<f64>) {
        if self.0.is_nan() {
            return;
        }
        let t = self.0;
        let (y, _, dt) = self.civil();
        self.0 = time_clip(make_date(
            make_day(y, m, d.unwrap_or(dt)),
            time_within_day(t),
        ));
    }

    /// `setFullYear(y)`.
    pub fn set_full_year(&mut self, y: f64) {
        let t = if self.0.is_nan() { 0.0 } else { self.0 };
        let (_, m, dt) = Date(t).civil();
        self.0 = time_clip(make_date(make_day(y, m, dt), time_within_day(t)));
    }

    /// `setHours(h, m, s, ms)`.
    pub fn set_hours(&mut self, h: f64, m: f64, s: f64, ms: f64) {
        if self.0.is_nan() {
            return;
        }
        let t = self.0;
        self.0 = time_clip(make_date(day(t), make_time(h, m, s, ms)));
    }

    /// `new Date(string)`, as V8 parses it.
    pub fn parse(s: &str) -> Date {
        match parse_v8(s) {
            Some(t) => Date(t),
            None => Date(f64::NAN),
        }
    }
}

// --- V8's DateParser ----------------------------------------------------

const NONE: i64 = i32::MAX as i64;
const MAX_SIGNIFICANT_DIGITS: usize = 9;

fn between(x: i64, lo: i64, hi: i64) -> bool {
    lo <= x && x <= hi
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kw {
    Invalid,
    MonthName,
    TimeZoneName,
    TimeSeparator,
    AmPm,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Token {
    Invalid,
    Unknown,
    WhiteSpace,
    Number(i64, usize),
    Symbol(u32),
    End,
    Keyword(Kw, i64, usize),
}

impl Token {
    fn is_number(&self) -> bool {
        matches!(self, Token::Number(..))
    }
    fn is_fixed(&self, n: usize) -> bool {
        matches!(self, Token::Number(_, l) if *l == n)
    }
    fn number(&self) -> i64 {
        match self {
            Token::Number(v, _) => *v,
            _ => 0,
        }
    }
    fn is_symbol(&self, c: char) -> bool {
        *self == Token::Symbol(c as u32)
    }
    fn is_sign(&self) -> bool {
        self.is_symbol('+') || self.is_symbol('-')
    }
    fn sign(&self) -> i64 {
        match self {
            Token::Symbol(c) => 44 - *c as i64,
            _ => 0,
        }
    }
    fn is_keyword_z(&self) -> bool {
        matches!(self, Token::Keyword(Kw::TimeZoneName, 0, 1))
    }
}

/// V8's `IsWhiteSpace` (not line terminators).
fn is_white_space(c: u32) -> bool {
    matches!(
        c,
        9 | 11 | 12 | 32 | 0xa0 | 0x1680 | 0x202f | 0x205f | 0x3000 | 0xfeff
    ) || (0x2000..=0x200a).contains(&c)
}

fn is_white_space_or_line_terminator(c: u32) -> bool {
    is_white_space(c) || matches!(c, 10 | 13 | 0x2028 | 0x2029)
}

const KEYWORDS: [([u32; 3], Kw, i64); 27] = [
    ([106, 97, 110], Kw::MonthName, 1),
    ([102, 101, 98], Kw::MonthName, 2),
    ([109, 97, 114], Kw::MonthName, 3),
    ([97, 112, 114], Kw::MonthName, 4),
    ([109, 97, 121], Kw::MonthName, 5),
    ([106, 117, 110], Kw::MonthName, 6),
    ([106, 117, 108], Kw::MonthName, 7),
    ([97, 117, 103], Kw::MonthName, 8),
    ([115, 101, 112], Kw::MonthName, 9),
    ([111, 99, 116], Kw::MonthName, 10),
    ([110, 111, 118], Kw::MonthName, 11),
    ([100, 101, 99], Kw::MonthName, 12),
    ([97, 109, 0], Kw::AmPm, 0),
    ([112, 109, 0], Kw::AmPm, 12),
    ([117, 116, 0], Kw::TimeZoneName, 0),
    ([117, 116, 99], Kw::TimeZoneName, 0),
    ([122, 0, 0], Kw::TimeZoneName, 0),
    ([103, 109, 116], Kw::TimeZoneName, 0),
    ([99, 100, 116], Kw::TimeZoneName, -5),
    ([99, 115, 116], Kw::TimeZoneName, -6),
    ([101, 100, 116], Kw::TimeZoneName, -4),
    ([101, 115, 116], Kw::TimeZoneName, -5),
    ([109, 100, 116], Kw::TimeZoneName, -6),
    ([109, 115, 116], Kw::TimeZoneName, -7),
    ([112, 100, 116], Kw::TimeZoneName, -7),
    ([112, 115, 116], Kw::TimeZoneName, -8),
    ([116, 0, 0], Kw::TimeSeparator, 0),
];

/// `KeywordTable::Lookup`.
fn lookup(pre: &[u32; 3], len: usize) -> (Kw, i64) {
    for (word, kind, value) in KEYWORDS {
        if word == *pre && (len <= 3 || kind == Kw::MonthName) {
            return (kind, value);
        }
    }
    (Kw::Invalid, 0)
}

/// `InputReader` and `DateStringTokenizer`.
struct Scanner {
    buf: Vec<u32>,
    index: usize,
    ch: u32,
    next: Token,
}

impl Scanner {
    fn new(s: &str) -> Scanner {
        let mut sc = Scanner {
            buf: s.encode_utf16().map(u32::from).collect(),
            index: 0,
            ch: 0,
            next: Token::End,
        };
        sc.advance();
        sc.next = sc.scan();
        sc
    }

    fn advance(&mut self) {
        self.ch = self.buf.get(self.index).copied().unwrap_or(0);
        self.index += 1;
    }

    fn position(&self) -> usize {
        self.index
    }

    fn is_digit(&self) -> bool {
        (48..=57).contains(&self.ch)
    }

    fn skip(&mut self, c: char) -> bool {
        if self.ch == c as u32 {
            self.advance();
            true
        } else {
            false
        }
    }

    fn read_unsigned_numeral(&mut self) -> i64 {
        let mut n: i64 = 0;
        let mut i = 0;
        while self.ch == 48 {
            self.advance();
        }
        while self.is_digit() {
            if i < MAX_SIGNIFICANT_DIGITS {
                n = n * 10 + (self.ch - 48) as i64;
            }
            i += 1;
            self.advance();
        }
        n
    }

    fn scan(&mut self) -> Token {
        let pre = self.position();
        if self.ch == 0 {
            return Token::End;
        }
        if self.is_digit() {
            let n = self.read_unsigned_numeral();
            return Token::Number(n, self.position() - pre);
        }
        for c in [':', '-', '+', '.', ')'] {
            if self.skip(c) {
                return Token::Symbol(c as u32);
            }
        }
        if self.ch >= 65 && !is_white_space(self.ch) {
            let mut prefix = [0u32; 3];
            let mut len = 0;
            while self.ch >= 65 && !is_white_space(self.ch) {
                if len < 3 {
                    prefix[len] = self.ch | 0x20;
                }
                len += 1;
                self.advance();
            }
            let (kind, value) = lookup(&prefix, len);
            return Token::Keyword(kind, value, len);
        }
        if is_white_space_or_line_terminator(self.ch) {
            self.advance();
            return Token::WhiteSpace;
        }
        if self.ch == 40 {
            let mut balance = 0;
            loop {
                if self.ch == 41 {
                    balance -= 1;
                } else if self.ch == 40 {
                    balance += 1;
                }
                self.advance();
                if !(balance > 0 && self.ch != 0) {
                    break;
                }
            }
            return Token::Unknown;
        }
        self.advance();
        Token::Unknown
    }

    fn next(&mut self) -> Token {
        let r = self.next;
        self.next = self.scan();
        r
    }

    fn peek(&self) -> Token {
        self.next
    }

    fn skip_symbol(&mut self, c: char) -> bool {
        if self.next.is_symbol(c) {
            self.next = self.scan();
            true
        } else {
            false
        }
    }
}

struct TimeZone {
    sign: i64,
    hour: i64,
    minute: i64,
}

impl TimeZone {
    fn set(&mut self, offset: i64) {
        self.sign = if offset < 0 { -1 } else { 1 };
        self.hour = offset * self.sign;
        self.minute = 0;
    }
    fn set_sign(&mut self, sign: i64) {
        self.sign = if sign < 0 { -1 } else { 1 };
    }
    fn is_expecting(&self, n: i64) -> bool {
        self.hour != NONE && self.minute == NONE && between(n, 0, 59)
    }
    fn is_utc(&self) -> bool {
        self.hour == 0 && self.minute == 0
    }
    fn is_empty(&self) -> bool {
        self.hour == NONE
    }
    fn write(&mut self) -> Option<f64> {
        if self.sign != NONE {
            if self.hour == NONE {
                self.hour = 0;
            }
            if self.minute == NONE {
                self.minute = 0;
            }
            let total = (self.hour as u32 as u64)
                .wrapping_mul(3600)
                .wrapping_add((self.minute as u32 as u64).wrapping_mul(60))
                & 0xffff_ffff;
            if total > (i32::MAX as u64 >> 1) {
                return None;
            }
            let total = total as i64;
            Some(if self.sign < 0 { -total } else { total } as f64)
        } else {
            Some(f64::NAN)
        }
    }
}

struct Time {
    comp: [i64; 4],
    index: usize,
    hour_offset: i64,
}

impl Time {
    fn is_empty(&self) -> bool {
        self.index == 0
    }
    fn is_expecting(&self, n: i64) -> bool {
        (self.index == 1 && between(n, 0, 59))
            || (self.index == 2 && between(n, 0, 59))
            || (self.index == 3 && between(n, 0, 999))
    }
    fn add(&mut self, n: i64) -> bool {
        if self.index < 4 {
            self.comp[self.index] = n;
            self.index += 1;
            true
        } else {
            false
        }
    }
    fn add_final(&mut self, n: i64) -> bool {
        if !self.add(n) {
            return false;
        }
        while self.index < 4 {
            self.comp[self.index] = 0;
            self.index += 1;
        }
        true
    }
    fn write(&mut self) -> Option<[i64; 4]> {
        while self.index < 4 {
            self.comp[self.index] = 0;
            self.index += 1;
        }
        let [mut hour, minute, second, ms] = self.comp;
        if self.hour_offset != NONE {
            if !between(hour, 0, 12) {
                return None;
            }
            hour %= 12;
            hour += self.hour_offset;
        }
        // A 24th hour is allowed if minutes, seconds, and milliseconds are 0.
        let valid = between(hour, 0, 23)
            && between(minute, 0, 59)
            && between(second, 0, 59)
            && between(ms, 0, 999);
        if !valid && (hour != 24 || minute != 0 || second != 0 || ms != 0) {
            return None;
        }
        Some([hour, minute, second, ms])
    }
}

struct Day {
    comp: [i64; 3],
    index: usize,
    named_month: i64,
    iso: bool,
}

impl Day {
    fn add(&mut self, n: i64) -> bool {
        if self.index < 3 {
            self.comp[self.index] = n;
            self.index += 1;
            true
        } else {
            false
        }
    }
    fn write(&mut self) -> Option<(i64, i64, i64)> {
        if self.index < 1 {
            return None;
        }
        while self.index < 3 {
            self.comp[self.index] = 1;
            self.index += 1;
        }
        let mut year = 0;
        let month;
        let day;
        let c = self.comp;
        if self.named_month == NONE {
            if self.iso || (self.index == 3 && !between(c[0], 1, 31)) {
                year = c[0];
                month = c[1];
                day = c[2];
            } else {
                month = c[0];
                day = c[1];
                if self.index == 3 {
                    year = c[2];
                }
            }
        } else {
            month = self.named_month;
            if self.index == 1 {
                day = c[0];
            } else if !between(c[0], 1, 31) {
                year = c[0];
                day = c[1];
            } else {
                day = c[0];
                year = c[1];
            }
        }
        if !self.iso {
            if between(year, 0, 49) {
                year += 2000;
            } else if between(year, 50, 99) {
                year += 1900;
            }
        }
        if !between(year, -(1 << 30), (1 << 30) - 1)
            || !between(month, 1, 12)
            || !between(day, 1, 31)
        {
            return None;
        }
        Some((year, month - 1, day))
    }
}

/// `ReadMilliseconds`.
fn read_milliseconds(token: Token) -> i64 {
    let (mut number, mut length) = match token {
        Token::Number(v, l) => (v, l),
        _ => (0, 0),
    };
    if length < 3 {
        if length == 1 {
            number *= 100;
        } else if length == 2 {
            number *= 10;
        }
    } else if length > 3 {
        if length > MAX_SIGNIFICANT_DIGITS {
            length = MAX_SIGNIFICANT_DIGITS;
        }
        let mut factor = 1;
        loop {
            factor *= 10;
            length -= 1;
            if length <= 3 {
                break;
            }
        }
        number /= factor;
    }
    number
}

/// `ParseES5DateTime`.
fn parse_es5(sc: &mut Scanner, day: &mut Day, time: &mut Time, tz: &mut TimeZone) -> Token {
    if sc.peek().is_sign() {
        let sign_token = sc.next();
        if !sc.peek().is_fixed(6) {
            return sign_token;
        }
        let sign = sign_token.sign();
        let year = sc.next().number();
        if sign < 0 && year == 0 {
            return sign_token;
        }
        day.add(sign * year);
    } else if sc.peek().is_fixed(4) {
        day.add(sc.next().number());
    } else {
        return sc.next();
    }
    if sc.skip_symbol('-') {
        if !sc.peek().is_fixed(2) || !between(sc.peek().number(), 1, 12) {
            return sc.next();
        }
        day.add(sc.next().number());
        if sc.skip_symbol('-') {
            if !sc.peek().is_fixed(2) || !between(sc.peek().number(), 1, 31) {
                return sc.next();
            }
            day.add(sc.next().number());
        }
    }
    if !matches!(sc.peek(), Token::Keyword(Kw::TimeSeparator, ..)) {
        if sc.peek() != Token::End {
            return sc.next();
        }
    } else {
        sc.next();
        if !sc.peek().is_fixed(2) || !between(sc.peek().number(), 0, 24) {
            return Token::Invalid;
        }
        let hour_is_24 = sc.peek().number() == 24;
        time.add(sc.next().number());
        if !sc.skip_symbol(':') {
            return Token::Invalid;
        }
        if !sc.peek().is_fixed(2)
            || !between(sc.peek().number(), 0, 59)
            || (hour_is_24 && sc.peek().number() > 0)
        {
            return Token::Invalid;
        }
        time.add(sc.next().number());
        if sc.skip_symbol(':') {
            if !sc.peek().is_fixed(2)
                || !between(sc.peek().number(), 0, 59)
                || (hour_is_24 && sc.peek().number() > 0)
            {
                return Token::Invalid;
            }
            time.add(sc.next().number());
            if sc.skip_symbol('.') {
                if !sc.peek().is_number() || (hour_is_24 && sc.peek().number() > 0) {
                    return Token::Invalid;
                }
                time.add(read_milliseconds(sc.next()));
            }
        }
        if sc.peek().is_keyword_z() {
            sc.next();
            tz.set(0);
        } else if sc.peek().is_symbol('+') || sc.peek().is_symbol('-') {
            tz.set_sign(if sc.next().is_symbol('+') { 1 } else { -1 });
            if sc.peek().is_fixed(4) {
                let hourmin = sc.next().number();
                let hour = hourmin / 100;
                let min = hourmin % 100;
                if !between(hour, 0, 23) || !between(min, 0, 59) {
                    return Token::Invalid;
                }
                tz.hour = hour;
                tz.minute = min;
            } else {
                if !sc.peek().is_fixed(2) || !between(sc.peek().number(), 0, 23) {
                    return Token::Invalid;
                }
                tz.hour = sc.next().number();
                if !sc.skip_symbol(':') {
                    return Token::Invalid;
                }
                if !sc.peek().is_fixed(2) || !between(sc.peek().number(), 0, 59) {
                    return Token::Invalid;
                }
                tz.minute = sc.next().number();
            }
        }
        if sc.peek() != Token::End {
            return Token::Invalid;
        }
    }
    if tz.is_empty() && time.is_empty() {
        tz.set(0);
    }
    day.iso = true;
    Token::End
}

/// `DateParser::Parse` and `ParseDateTimeString`: the time value, or None.
fn parse_v8(s: &str) -> Option<f64> {
    let mut sc = Scanner::new(s);
    let mut tz = TimeZone {
        sign: NONE,
        hour: NONE,
        minute: NONE,
    };
    let mut time = Time {
        comp: [0; 4],
        index: 0,
        hour_offset: NONE,
    };
    let mut day = Day {
        comp: [0; 3],
        index: 0,
        named_month: NONE,
        iso: false,
    };
    let next_unhandled = parse_es5(&mut sc, &mut day, &mut time, &mut tz);
    if next_unhandled == Token::Invalid {
        return None;
    }
    let mut has_read_number = day.index != 0;
    let mut token = next_unhandled;
    while token != Token::End {
        match token {
            Token::Number(n, _) => {
                has_read_number = true;
                if sc.skip_symbol(':') {
                    if sc.skip_symbol(':') {
                        if !time.is_empty() {
                            return None;
                        }
                        time.add(n);
                        time.add(0);
                    } else {
                        if !time.add(n) {
                            return None;
                        }
                        if sc.peek().is_symbol('.') {
                            sc.next();
                        }
                    }
                } else if sc.skip_symbol('.') && time.is_expecting(n) {
                    time.add(n);
                    if !sc.peek().is_number() {
                        return None;
                    }
                    let ms = read_milliseconds(sc.next());
                    if ms < 0 {
                        return None;
                    }
                    time.add_final(ms);
                } else if tz.is_expecting(n) {
                    tz.minute = n;
                } else if time.is_expecting(n) {
                    time.add_final(n);
                    let peek = sc.peek();
                    if !(peek == Token::End
                        || peek == Token::WhiteSpace
                        || peek.is_keyword_z()
                        || peek.is_sign())
                    {
                        return None;
                    }
                } else {
                    if !day.add(n) {
                        return None;
                    }
                    sc.skip_symbol('-');
                }
            }
            Token::Keyword(kind, value, _) => {
                if kind == Kw::AmPm && !time.is_empty() {
                    time.hour_offset = value;
                } else if kind == Kw::MonthName {
                    day.named_month = value;
                    sc.skip_symbol('-');
                } else if kind == Kw::TimeZoneName && has_read_number {
                    tz.set(value);
                } else {
                    if has_read_number {
                        return None;
                    }
                    if sc.peek().is_number() {
                        return None;
                    }
                }
            }
            t if t.is_sign() && (tz.is_utc() || !time.is_empty()) => {
                tz.set_sign(t.sign());
                let mut n = 0;
                let mut length = 0;
                if sc.peek().is_number() {
                    if let Token::Number(v, l) = sc.next() {
                        n = v;
                        length = l;
                    }
                }
                has_read_number = true;
                if sc.peek().is_symbol(':') {
                    tz.hour = n;
                    tz.minute = NONE;
                } else if length == 2 || length == 1 {
                    tz.hour = n;
                    tz.minute = 0;
                } else if length == 4 || length == 3 {
                    tz.hour = n / 100;
                    tz.minute = n % 100;
                } else {
                    return None;
                }
            }
            t if (t.is_sign() || t.is_symbol(')')) && has_read_number => return None,
            _ => {}
        }
        token = sc.next();
    }
    let (y, m, d) = day.write()?;
    let [h, mi, s, ms] = time.write()?;
    let offset = tz.write()?;
    let dv = make_day(y as f64, m as f64, d as f64);
    let tv = make_time(h as f64, mi as f64, s as f64, ms as f64);
    let mut date = make_date(dv, tv);
    if offset.is_nan() {
        // Local time, which is UTC here.
        if !(-8.64e15 - 864e6..=8.64e15 + 864e6).contains(&date) {
            return None;
        }
    } else {
        date -= offset * 1000.0;
        if !(-8.64e15..=8.64e15).contains(&date) {
            return None;
        }
    }
    let t = time_clip(date);
    if t.is_nan() {
        None
    } else {
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_as_v8() {
        let iso = |s: &str| Date::parse(s).0;
        assert_eq!(iso("2014-01-01"), 1388534400000.0);
        assert_eq!(iso("2014-006"), utc(2014.0, 5.0, 1.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(iso("217-07-20"), utc(217.0, 6.0, 20.0, 0.0, 0.0, 0.0, 0.0));
        assert_eq!(
            iso("accTitle: 2014-01-01"),
            utc(2014.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0)
        );
        assert!(iso("des1").is_nan());
        assert!(iso("20 14-02-10").is_nan());
        let mut d = Date::new(utc(2014.0, 0.0, 31.0, 5.0, 0.0, 0.0, 0.0));
        d.set_month(1.0, None);
        assert_eq!(d.month(), 2.0);
        assert_eq!(d.date(), 3.0);
    }
}
