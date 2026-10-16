//! Incidents: what went wrong, how bad it was, what is being done about it,
//! and whether it is closed.
//!
//! One of the starters an organisation releases in its first minute on
//! Krate, on the second edition of the starter kit (read Team Notes first:
//! the store, the sync and the states are the same):
//!
//! - **One record per incident** in `incidents`: `what`, `detail`,
//!   `severity` (Low, Medium, High, Critical), `place`, `day`, `owner`,
//!   `status` (Open, Closed) and `reported_by`.
//! - **One record per action** in `actions` (`of` is the incident): `what`,
//!   `owner`, `due` and `done`. Two people ticking two actions at once both
//!   keep their tick.
//! - **Closing it is a line in the `log`** like every other step, so the
//!   incident reads from report to close with who did each part.
//!
//! `#![no_std]`, so it imports only `krate:*`.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Look, Rect, Said, Screen};
use krate::math::F32Ext as _;
use krate::math::F64Ext as _;
use krate::org::{self, Entry, OrgError, Value};
use krate::ui::{events, tree, types, window};

const APP_NAME: &str = "Incidents";
const THINGS: &str = "incidents";
const ABOUT: &str = "It keeps your team's incident log, and what was done about each one, \
                     encrypted so only your team can read it.";
const TOO_LONG: &str = "That is too long to save. Shorten what happened.";
const LOCK: &str = "Encrypted end to end: not even Krate can read your team's incidents.";
const MAIN: &str = "incidents";
const COLLECTIONS: &[&str] = &[MAIN, "actions", LOG];
const PEOPLE: &[&str] = &["owner", "reported_by"];
const WINDOW: (u32, u32) = (1080, 700);
const NEW_LABEL: &str = "Report an incident";
const NEW_SHORT: &str = "Report";
const NOTE_HINT: &str = "Add an update";

// ===== The starter kit, second edition: the same in all twelve record apps =====
//
// Team Notes, Expenses and Stock List share the first edition. These twelve
// (Requests, Leave, Timesheets, Assets, Inspections, Visitors, Onboarding,
// Help Desk, Contacts, Purchase Orders, Incidents, Shift Rota) share this
// one, which keeps the same store habits and adds what an app of records
// needs: a list with views and search, a record's page with its history,
// a form, and a run on this computer for someone who opens the app outside
// an organisation. Everything below this line up to "End of the starter
// kit" is the same text in all twelve; what makes each app itself comes
// after it, through the functions the kit calls (`rows`, `form_for`,
// `check`, `facts`, `actions`, `act`, `extra`, `summary`, `board` ...).

/// How often the app asks the store whether anything changed (local).
const LOOK_EVERY_MS: u64 = 1_000;
/// How often the app asks for a sync of its own, to know online/pending.
const SYNC_EVERY_MS: u64 = 20_000;
/// How long Undo is offered after a delete.
const UNDO_FOR_MS: u64 = 8_000;
/// Every change is also one record here, so "who changed what" is a list a
/// person can read, not only the last name on a record.
const LOG: &str = "log";

// ---------------------------------------------------------------------------
// The team's store: what it said last, in words a person reads
// ---------------------------------------------------------------------------

/// Whether this run can use the team's data at all.
#[derive(Clone, PartialEq)]
enum Access {
    /// The organisation's release, opened by a member (or a run here).
    Open,
    /// Run some other way: a build being made, a copy not released.
    NotAnOrgApp,
    /// The organisation said no: the stable reason and its own words.
    Refused(String, String),
    /// The person did not give the app the team's data.
    Denied,
    /// This host cannot reach an organisation (a browser tab, for now).
    Unsupported,
}

/// The records of a run on this computer only: what someone sees when they
/// open a copy that is not an organisation's release and choose to try it.
/// Nothing in it is written anywhere; it ends with the window.
struct Mem {
    recs: Vec<(String, Entry)>,
    version: u64,
}

struct Team {
    access: Access,
    /// The signed-in person as the organisation names them; empty until a
    /// sync has said, then learnt from `who`.
    me: String,
    /// The organisation's name, for "Acme's tickets".
    org: String,
    online: bool,
    pending: u32,
    reached_at: Option<u64>,
    answered: bool,
    /// The organisation has let this computer in (it holds the team's key).
    admitted: bool,
    saved_since: bool,
    problem: Option<String>,
    /// A run on this computer only, when the person chose to try the app.
    mem: Option<Mem>,
    /// The line saying a run here keeps nothing was dismissed.
    told: bool,
}

impl Team {
    fn new() -> Self {
        Team {
            access: Access::Open,
            me: String::new(),
            org: String::new(),
            online: false,
            pending: 0,
            reached_at: None,
            answered: false,
            admitted: false,
            saved_since: false,
            problem: None,
            mem: None,
            told: false,
        }
    }

    fn org_name(&self) -> &str {
        if self.org.is_empty() {
            "your team"
        } else {
            &self.org
        }
    }

    fn trying(&self) -> bool {
        self.mem.is_some()
    }

    /// Run on this computer only: the app works, and keeps nothing.
    fn try_here(&mut self) {
        self.mem = Some(Mem {
            recs: Vec::new(),
            version: 1,
        });
        self.access = Access::Open;
        self.me = String::from("you");
        self.answered = true;
        self.admitted = true;
        self.online = true;
    }

    /// Take in an error from any call. Refusals end the run's access to the
    /// data; limits are a line the person can act on.
    fn fail(&mut self, error: OrgError) {
        match error {
            OrgError::NotAnOrgApp => self.access = Access::NotAnOrgApp,
            OrgError::Refused(r) => self.access = Access::Refused(r.reason, r.message),
            OrgError::Denied => self.access = Access::Denied,
            OrgError::Unsupported => self.access = Access::Unsupported,
            OrgError::TooLarge => self.problem = Some(String::from(TOO_LONG)),
            OrgError::OverQuota => {
                self.problem = Some(format!(
                    "{}'s {THINGS} are full. Delete what nobody needs, then try again.",
                    self.org_name()
                ))
            }
            OrgError::Invalid(words) => self.problem = Some(words),
            OrgError::Io(words) => {
                self.problem = Some(format!("This computer could not save that: {words}"))
            }
        }
    }

    /// Ask for a sync now. It can wait on the network: call it when nobody
    /// is typing. True when the team changed something.
    fn sync(&mut self) -> bool {
        if self.trying() {
            return false;
        }
        match org::sync() {
            Ok(state) => {
                self.online = state.online;
                self.pending = state.pending;
                self.answered = true;
                self.saved_since = false;
                if state.online {
                    self.reached_at = Some(krate::time::now_millis());
                }
                self.learn_who();
                state.changed
            }
            Err(error) => {
                self.fail(error);
                false
            }
        }
    }

    /// Who this is, once the organisation has said.
    fn learn_who(&mut self) {
        if self.trying() {
            return;
        }
        if let Ok(Some(member)) = org::who() {
            self.me = member.name;
            self.org = member.org;
            self.admitted = true;
        }
    }

    fn waiting_to_be_let_in(&self) -> bool {
        self.answered && self.online && !self.admitted
    }

    /// One short line for the corner, and the colour of its dot.
    fn status(&self, now: u64, ink: &Ink, clock: &Clock) -> (String, gfx::Color) {
        if self.trying() {
            return (String::from("Trying it here"), ink.warn);
        }
        if !self.answered {
            return (format!("Opening {}...", self.org_name()), ink.muted);
        }
        if !self.online {
            return (String::from("Offline, saved here"), ink.warn);
        }
        if !self.admitted {
            return (String::from("Waiting to be let in"), ink.warn);
        }
        if self.saved_since {
            return (String::from("Saved"), ink.good);
        }
        if self.pending > 0 {
            return (String::from("Sending changes"), ink.muted);
        }
        match self.reached_at {
            Some(at) => (format!("Synced {}", clock.when(at, now)), ink.good),
            None => (String::from("Up to date"), ink.good),
        }
    }

    /// A longer line under the bar, for the states that need explaining.
    fn explain(&self) -> Option<String> {
        if let Some(problem) = &self.problem {
            return Some(problem.clone());
        }
        if self.trying() && !self.told {
            return Some(format!(
                "You are trying {APP_NAME} on this computer: nothing you add is kept or shared. \
                 Release it to your organisation on Krate to keep your team's {THINGS}."
            ));
        }
        if self.waiting_to_be_let_in() {
            return Some(format!(
                "This computer is new to your team. The team's {THINGS} appear here once a \
                 teammate's computer is online and lets this one in, which happens by itself. \
                 Anything you add now is kept and shared then."
            ));
        }
        None
    }

    // ----- the store: the organisation's, or this run's -----

    /// Every record in a collection, and the cursor to ask "what changed
    /// since?" with. Local: it never waits for the network.
    fn read(&mut self, collection: &str) -> Option<(Vec<Entry>, String)> {
        if let Some(mem) = &self.mem {
            let entries = mem
                .recs
                .iter()
                .filter(|(c, _)| c == collection)
                .map(|(_, e)| e.clone())
                .collect();
            return Some((entries, format!("{}", mem.version)));
        }
        let mut entries = Vec::new();
        let mut since: Option<String> = None;
        loop {
            match org::records(collection, since.as_deref()) {
                Ok(page) => {
                    entries.extend(page.entries);
                    since = Some(page.next);
                    if !page.more {
                        break;
                    }
                }
                Err(error) => {
                    self.fail(error);
                    return None;
                }
            }
        }
        Some((entries, since.unwrap_or_default()))
    }

    /// Whether anything in a collection changed after `cursor`: local, cheap,
    /// fine to ask every second.
    fn changed(&mut self, collection: &str, cursor: &str) -> bool {
        if let Some(mem) = &self.mem {
            return format!("{}", mem.version) != cursor;
        }
        match org::records(collection, Some(cursor)) {
            Ok(page) => !page.entries.is_empty() || !page.removed.is_empty(),
            Err(error) => {
                self.fail(error);
                true
            }
        }
    }

    /// Write some fields of one record; the others keep their values.
    fn put(&mut self, collection: &str, id: &str, fields: &[(&str, Value)]) -> bool {
        let me = self.me.clone();
        if let Some(mem) = self.mem.as_mut() {
            let now = krate::time::now_millis();
            let at = mem
                .recs
                .iter()
                .position(|(c, e)| c == collection && e.id == id);
            let i = match at {
                Some(i) => i,
                None => {
                    mem.recs.push((
                        String::from(collection),
                        Entry {
                            id: String::from(id),
                            fields: Vec::new(),
                            changed_by: me.clone(),
                            changed_at: now,
                        },
                    ));
                    mem.recs.len() - 1
                }
            };
            let entry = &mut mem.recs[i].1;
            for (name, value) in fields {
                entry.fields.retain(|(n, _)| n != name);
                if !matches!(value, Value::Null) {
                    entry.fields.push((String::from(*name), value.clone()));
                }
            }
            entry.fields.sort_by(|a, b| a.0.cmp(&b.0));
            entry.changed_by = me;
            entry.changed_at = now;
            mem.version += 1;
            self.problem = None;
            return true;
        }
        match org::put(collection, id, fields) {
            Ok(()) => {
                self.saved_since = true;
                self.problem = None;
                true
            }
            Err(error) => {
                self.fail(error);
                false
            }
        }
    }

    fn delete(&mut self, collection: &str, id: &str) -> bool {
        if let Some(mem) = self.mem.as_mut() {
            mem.recs.retain(|(c, e)| !(c == collection && e.id == id));
            mem.version += 1;
            return true;
        }
        match org::delete(collection, id) {
            Ok(()) => {
                self.saved_since = true;
                true
            }
            Err(error) => {
                self.fail(error);
                false
            }
        }
    }
}

/// A record id nobody else will make: the time, a counter, and a hash of
/// who is making it.
fn new_id(prefix: &str, me: &str, counter: u32) -> String {
    let now = krate::time::now_millis();
    let nanos = krate::time::monotonic_nanos();
    format!(
        "{prefix}{now:x}{:04x}{:03x}",
        (nanos as u32 ^ counter) & 0xffff,
        fnv(me) & 0xfff
    )
}

fn fnv(text: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in text.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

// ---------------------------------------------------------------------------
// Records, as the app reads them
// ---------------------------------------------------------------------------

/// One record: its fields, and who changed it last and when.
#[derive(Clone)]
struct Rec {
    id: String,
    fields: Vec<(String, Value)>,
    by: String,
    at: u64,
}

impl Rec {
    fn of(entry: &Entry) -> Rec {
        Rec {
            id: entry.id.clone(),
            fields: entry.fields.clone(),
            by: entry.changed_by.clone(),
            at: entry.changed_at,
        }
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// A text field, or "".
    fn t(&self, key: &str) -> &str {
        match self.get(key) {
            Some(Value::Text(s)) => s.as_str(),
            _ => "",
        }
    }

    #[allow(dead_code)] // the kit is shared; not every app uses all of it
    fn n(&self, key: &str) -> Option<f64> {
        match self.get(key) {
            Some(Value::Number(n)) => Some(*n),
            _ => None,
        }
    }

    fn b(&self, key: &str) -> bool {
        matches!(self.get(key), Some(Value::Boolean(true)))
    }

    /// The text of a field as a form shows it.
    fn shown(&self, key: &str) -> String {
        match self.get(key) {
            Some(Value::Text(s)) => s.clone(),
            Some(Value::Number(n)) => plain_number(*n),
            Some(Value::Boolean(b)) => String::from(if *b { "yes" } else { "no" }),
            _ => String::new(),
        }
    }

    /// Whether any of `keys` holds `wanted`, ignoring case.
    fn matches(&self, keys: &[&str], wanted: &str) -> bool {
        let wanted = wanted.trim();
        wanted.is_empty() || keys.iter().any(|k| contains_folded(self.t(k), wanted))
    }
}

fn same_value(old: Option<&Value>, new: &Value) -> bool {
    match (old, new) {
        (None, Value::Null) => true,
        (Some(Value::Text(a)), Value::Text(b)) => a == b,
        (Some(Value::Number(a)), Value::Number(b)) => (a - b).abs() < 1e-9,
        (Some(Value::Boolean(a)), Value::Boolean(b)) => a == b,
        (None, Value::Text(b)) => b.is_empty(),
        _ => false,
    }
}

/// One collection as this run last read it.
struct Coll {
    name: &'static str,
    recs: Vec<Rec>,
    cursor: Option<String>,
}

fn contains_folded(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}

/// 12 as "12", 7.5 as "7.5", 1234.25 as "1234.25".
fn plain_number(n: f64) -> String {
    let hundredths = (n * 100.0).round() as i64;
    let sign = if hundredths < 0 { "-" } else { "" };
    let h = hundredths.unsigned_abs();
    if h.is_multiple_of(100) {
        format!("{sign}{}", h / 100)
    } else if h.is_multiple_of(10) {
        format!("{sign}{}.{}", h / 100, (h % 100) / 10)
    } else {
        format!("{sign}{}.{:02}", h / 100, h % 100)
    }
}

/// "12", "7.5", "7,5", "1,200": the number in it, or None.
fn parse_number(text: &str) -> Option<f64> {
    let kept: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',' || *c == '-')
        .collect();
    if kept.is_empty() {
        return None;
    }
    let normal = match (kept.rfind(','), kept.contains('.')) {
        (Some(at), false) if kept.len() - at - 1 <= 2 => {
            kept.replacen(',', ".", 1).replace(',', "")
        }
        _ => kept.replace(',', ""),
    };
    let value: f64 = normal.parse().ok()?;
    value.is_finite().then_some(value)
}

// ---------------------------------------------------------------------------
// Dates, in the person's own time zone
// ---------------------------------------------------------------------------

struct Clock {
    tz: String,
    loc: krate::locale::LocaleId,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
#[allow(dead_code)] // the kit is shared; not every app uses all of it
const FULL_MONTHS: [&str; 12] = [
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
#[allow(dead_code)] // the kit is shared; not every app uses all of it
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

impl Clock {
    fn new() -> Self {
        Clock {
            tz: krate::locale::timezone(),
            loc: krate::locale::current(),
        }
    }

    /// (year, month, day, "HH:MM") for an instant, here.
    fn parts(&self, ms: u64) -> (u32, usize, u32, String) {
        let s =
            krate::locale::format_date(ms, &self.tz, krate::locale::DateStyle::Medium, &self.loc);
        let num = |a: usize, b: usize| s.get(a..b).and_then(|t| t.parse::<u32>().ok()).unwrap_or(0);
        let hm = String::from(s.get(11..16).unwrap_or(""));
        (num(0, 4), (num(5, 7) as usize).clamp(1, 12), num(8, 10), hm)
    }

    /// Today here as `YYYY-MM-DD`, moved by `days`.
    fn today(&self, days: i64) -> String {
        let (y, m, d, _) = self.parts(krate::time::now_millis());
        let base = days_from_civil(y as i64, m as i64, d as i64);
        date_of(base + days)
    }

    /// "HH:MM" of an instant, here.
    #[allow(dead_code)] // the kit is shared; not every app uses all of it
    fn hm(&self, ms: u64) -> String {
        self.parts(ms).3
    }

    /// "just now", "4 min ago", "today 14:02", "yesterday 09:15", "12 Oct".
    fn when(&self, ms: u64, now: u64) -> String {
        if ms <= now + 60_000 && now.saturating_sub(ms) < 60_000 {
            return String::from("just now");
        }
        if now > ms && now - ms < 3_600_000 {
            return format!("{} min ago", (now - ms) / 60_000);
        }
        let (y, m, d, hm) = self.parts(ms);
        let (ty, tm, td, _) = self.parts(now);
        if (y, m, d) == (ty, tm, td) {
            return format!("today {hm}");
        }
        let (yy, ym, yd, _) = self.parts(now.saturating_sub(86_400_000));
        if (y, m, d) == (yy, ym, yd) {
            return format!("yesterday {hm}");
        }
        let month = MONTHS.get(m - 1).copied().unwrap_or("");
        if y == ty {
            format!("{d} {month}")
        } else {
            format!("{d} {month} {y}")
        }
    }
}

/// Days since 1970-01-01 for a calendar day.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The calendar day `days` after 1970-01-01, as `YYYY-MM-DD`.
fn date_of(days: i64) -> String {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 of a `YYYY-MM-DD`, or None.
fn day_number(date: &str) -> Option<i64> {
    if !valid_date(date) {
        return None;
    }
    let num = |a: usize, z: usize| date.get(a..z).and_then(|t| t.parse::<i64>().ok());
    Some(days_from_civil(num(0, 4)?, num(5, 7)?, num(8, 10)?))
}

/// Monday is 0.
fn weekday(date: &str) -> usize {
    day_number(date)
        .map(|n| (n + 3).rem_euclid(7) as usize)
        .unwrap_or(0)
}

/// `date` moved by `days`.
fn add_days(date: &str, days: i64) -> String {
    match day_number(date) {
        Some(n) => date_of(n + days),
        None => String::from(date),
    }
}

/// The Monday of the week holding `date`.
fn week_start(date: &str) -> String {
    add_days(date, -(weekday(date) as i64))
}

/// A real calendar day, written `YYYY-MM-DD`.
fn valid_date(date: &str) -> bool {
    let b = date.as_bytes();
    if b.len() != 10 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return false;
    }
    let num = |a: usize, z: usize| date.get(a..z).and_then(|t| t.parse::<u32>().ok());
    let (Some(y), Some(m), Some(d)) = (num(0, 4), num(5, 7), num(8, 10)) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1900..=2200).contains(&y) && d >= 1 && d <= days
}

/// "2026-10-14" as "14 Oct".
#[allow(dead_code)] // the kit is shared; not every app uses all of it
fn day_name(date: &str) -> String {
    if !valid_date(date) {
        return String::from(date);
    }
    let m: usize = date.get(5..7).and_then(|t| t.parse().ok()).unwrap_or(1);
    let d: u32 = date.get(8..10).and_then(|t| t.parse().ok()).unwrap_or(1);
    format!(
        "{d} {}",
        MONTHS.get(m.clamp(1, 12) - 1).copied().unwrap_or("")
    )
}

/// "2026-10-14" as "Wed 14 Oct".
#[allow(dead_code)] // the kit is shared; not every app uses all of it
fn long_day(date: &str) -> String {
    if !valid_date(date) {
        return String::from(date);
    }
    format!("{} {}", WEEKDAYS[weekday(date)], day_name(date))
}

/// "2026-10" as "October 2026".
#[allow(dead_code)] // the kit is shared; not every app uses all of it
fn month_name(month: &str) -> String {
    let m: usize = month.get(5..7).and_then(|t| t.parse().ok()).unwrap_or(1);
    format!(
        "{} {}",
        FULL_MONTHS.get(m.clamp(1, 12) - 1).copied().unwrap_or(""),
        month.get(..4).unwrap_or("")
    )
}

/// "9", "9:30", "0930", "09.30" as "09:30", or None.
fn parse_time(text: &str) -> Option<String> {
    let t = text.trim();
    let digits: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
    let (h, m) = if t.contains(':') || t.contains('.') {
        let mut parts = t.split([':', '.']);
        let h: u32 = parts.next()?.trim().parse().ok()?;
        let m: u32 = parts.next().unwrap_or("0").trim().parse().ok()?;
        (h, m)
    } else if digits.len() <= 2 && !digits.is_empty() {
        (digits.parse().ok()?, 0)
    } else if digits.len() == 3 || digits.len() == 4 {
        let (a, b) = digits.split_at(digits.len() - 2);
        (a.parse().ok()?, b.parse().ok()?)
    } else {
        return None;
    };
    (h < 24 && m < 60).then(|| format!("{h:02}:{m:02}"))
}

// ---------------------------------------------------------------------------
// A text field the app edits itself
// ---------------------------------------------------------------------------

struct Field {
    text: String,
    /// Caret, a byte offset on a char boundary.
    caret: usize,
    multiline: bool,
}

impl Field {
    fn new(multiline: bool) -> Self {
        Field {
            text: String::new(),
            caret: 0,
            multiline,
        }
    }

    fn with(text: &str, multiline: bool) -> Self {
        let mut f = Field::new(multiline);
        f.set(text);
        f
    }

    fn set(&mut self, text: &str) {
        self.text = String::from(text);
        self.caret = self.text.len();
    }

    fn insert(&mut self, typed: &str) -> bool {
        let mut clean = String::new();
        for ch in typed.chars() {
            if ch == '\n' || ch == '\r' {
                if self.multiline {
                    clean.push('\n');
                }
            } else if ch == '\t' {
                clean.push(' ');
            } else if !ch.is_control() {
                clean.push(ch);
            }
        }
        if clean.is_empty() {
            return false;
        }
        self.text.insert_str(self.caret, &clean);
        self.caret += clean.len();
        true
    }

    fn prev(&self, at: usize) -> usize {
        self.text[..at]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map(|c| at + c.len_utf8())
            .unwrap_or(at)
    }

    /// The editing keys every field shares: (text changed, key handled).
    fn key(&mut self, key: &str) -> (bool, bool) {
        match key {
            "Backspace" if self.caret > 0 => {
                let from = self.prev(self.caret);
                self.text.replace_range(from..self.caret, "");
                self.caret = from;
                (true, true)
            }
            "Delete" if self.caret < self.text.len() => {
                let to = self.next(self.caret);
                self.text.replace_range(self.caret..to, "");
                (true, true)
            }
            "ArrowLeft" => {
                self.caret = self.prev(self.caret);
                (false, true)
            }
            "ArrowRight" => {
                self.caret = self.next(self.caret);
                (false, true)
            }
            "Home" => {
                self.caret = self.text[..self.caret]
                    .rfind('\n')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                (false, true)
            }
            "End" => {
                self.caret = self.text[self.caret..]
                    .find('\n')
                    .map(|i| self.caret + i)
                    .unwrap_or(self.text.len());
                (false, true)
            }
            _ => (false, false),
        }
    }
}

// ---------------------------------------------------------------------------
// Drawing: colours from the person's look, measured text
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Ink {
    bg: gfx::Color,
    surface: gfx::Color,
    ink: gfx::Color,
    muted: gfx::Color,
    line: gfx::Color,
    accent: gfx::Color,
    on_accent: gfx::Color,
    tint: gfx::Color,
    good: gfx::Color,
    warn: gfx::Color,
    danger: gfx::Color,
}

const fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

fn ink_for(look: &Look) -> Ink {
    let p = look.palette();
    if look.dark {
        Ink {
            bg: p.background,
            surface: p.surface,
            ink: p.ink,
            muted: p.muted,
            line: p.line,
            accent: p.accent,
            on_accent: p.on_accent,
            tint: rgb(0.20, 0.20, 0.23),
            good: rgb(0.42, 0.83, 0.56),
            warn: rgb(0.98, 0.72, 0.33),
            danger: rgb(1.0, 0.50, 0.47),
        }
    } else {
        Ink {
            bg: p.background,
            surface: p.surface,
            ink: p.ink,
            muted: p.muted,
            line: p.line,
            accent: p.accent,
            on_accent: p.on_accent,
            tint: rgb(0.91, 0.91, 0.93),
            good: rgb(0.09, 0.52, 0.29),
            warn: rgb(0.70, 0.40, 0.0),
            danger: rgb(0.76, 0.15, 0.15),
        }
    }
}

fn style(weight: u16) -> gfx::TextStyle {
    gfx::TextStyle {
        weight,
        italic: false,
        letter_spacing: 0.0,
        family: gfx::FontFamily::Sans,
    }
}

fn gr(r: Rect) -> gfx::Rect {
    gfx::Rect {
        x: r.x,
        y: r.y,
        width: r.w,
        height: r.h,
    }
}

fn radii(all: f32) -> gfx::CornerRadii {
    gfx::CornerRadii {
        top_left: all,
        top_right: all,
        bottom_right: all,
        bottom_left: all,
    }
}

fn fill(c: u64, r: Rect, color: gfx::Color) {
    let _ = canvas2d::fill_rect(c, gr(r), color);
}

fn round(c: u64, r: Rect, radius: f32, color: gfx::Color) {
    let _ = canvas2d::fill_round_rect(c, gr(r), radii(radius), color);
}

fn outline(c: u64, r: Rect, radius: f32, color: gfx::Color) {
    let _ = canvas2d::stroke_round_rect(c, gr(r), radii(radius), 1.0, color);
}

fn shade(a: f32) -> gfx::Color {
    gfx::Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a,
    }
}

fn width_of(c: u64, s: &str, size: f32, weight: u16) -> f32 {
    canvas2d::measure_text_styled(c, s, size, style(weight))
        .map(|m| m.width)
        .unwrap_or(0.0)
}

fn text(c: u64, s: &str, x: f32, baseline: f32, size: f32, ink: gfx::Color, weight: u16) {
    if s.is_empty() {
        return;
    }
    let _ = canvas2d::draw_text_styled(
        c,
        s,
        gfx::Point { x, y: baseline },
        size,
        ink,
        style(weight),
    );
}

/// Where text of `size` sits to look centred in a band `h` tall at `y`.
fn baseline(y: f32, h: f32, size: f32) -> f32 {
    y + h * 0.5 + size * 0.36
}

/// `s` cut with an ellipsis to fit `max` points.
fn fit(c: u64, s: &str, size: f32, weight: u16, max: f32) -> String {
    if max <= 0.0 {
        return String::new();
    }
    if width_of(c, s, size, weight) <= max {
        return String::from(s);
    }
    let chars: Vec<char> = s.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let mut t: String = chars[..mid].iter().collect();
        t = String::from(t.trim_end());
        t.push('\u{2026}');
        if width_of(c, &t, size, weight) <= max {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 {
        return String::new();
    }
    let mut t: String = chars[..lo].iter().collect();
    t = String::from(t.trim_end());
    t.push('\u{2026}');
    t
}

/// Lines of `s` wrapped to `max` points, as byte ranges. A hard line break
/// starts a new line; a word too long for a line is broken inside it.
fn wrap(c: u64, s: &str, size: f32, weight: u16, max: f32) -> Vec<(usize, usize)> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for para in s.split('\n') {
        let end = start + para.len();
        let mut line = start;
        let mut i = start;
        while i < end {
            let rest = &s[i..end];
            let word_len = rest.find(' ').map(|j| j + 1).unwrap_or(rest.len());
            let word_len =
                word_len + rest[word_len..].len() - rest[word_len..].trim_start_matches(' ').len();
            let next = i + word_len;
            if width_of(c, s[line..next].trim_end(), size, weight) <= max {
                i = next;
                continue;
            }
            if line < i {
                lines.push((line, i));
                line = i;
                continue;
            }
            let mut j = i;
            for (k, ch) in s[i..next].char_indices() {
                let at = i + k + ch.len_utf8();
                if width_of(c, &s[line..at], size, weight) > max && at > line + ch.len_utf8() {
                    break;
                }
                j = at;
            }
            lines.push((line, j));
            line = j;
            i = j;
        }
        lines.push((line, end));
        start = end + 1;
    }
    lines
}

/// The byte in `s[from..to]` nearest to `x` points from its start.
fn nearest(c: u64, s: &str, from: usize, to: usize, x: f32, size: f32, weight: u16) -> usize {
    let mut best = from;
    let mut best_d = x.abs();
    for (k, ch) in s[from..to].char_indices() {
        let at = from + k + ch.len_utf8();
        let d = (width_of(c, &s[from..at], size, weight) - x).abs();
        if d < best_d {
            best_d = d;
            best = at;
        }
    }
    best
}

#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Primary,
    Plain,
    Quiet,
    Danger,
}

/// What a badge says about a record, as a colour.
#[allow(dead_code)] // the kit is shared; not every app uses all of it
#[derive(Clone, Copy, PartialEq)]
enum Mood {
    Good,
    Warn,
    Bad,
    Calm,
    Info,
}

fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style {
            width: None,
            height: None,
            grow: 1.0,
            padding: 0.0,
            text: None,
            place: None,
            box_: None,
        },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

// ---------------------------------------------------------------------------
// The pen: one frame's drawing, what it says, and where its controls are
// ---------------------------------------------------------------------------

#[allow(dead_code)] // the kit is shared; not every app uses all of it
#[derive(Clone, Copy, PartialEq)]
enum Hit {
    New,
    Row(usize),
    Quick(usize),
    Back,
    Close,
    View(usize),
    Search,
    ClearSearch,
    Field(usize),
    Pick(usize, usize),
    Suggest(usize, usize),
    Day(usize, i64),
    Save,
    Cancel,
    Delete,
    Edit,
    Undo,
    Dismiss,
    Backdrop,
    Card,
    Try,
    Note,
    AddNote,
    Act(u16),
    Cell(u16, u32),
    Start(usize),
    /// Move the week shown: -1 back, 1 on, 0 to this week.
    Week(i64),
    /// A row of a record's own list: tick it, or open it.
    Tick(u16, usize),
    Open(u16, usize),
}

struct Pen {
    c: u64,
    k: Ink,
    look: Look,
    said: Said,
    hits: Vec<(Rect, Hit)>,
    tap: f32,
    compact: bool,
}

impl Pen {
    fn s(&self, points: f32) -> f32 {
        self.look.text(points)
    }

    fn w(&self, s: &str, size: f32, weight: u16) -> f32 {
        width_of(self.c, s, size, weight)
    }

    fn fit(&self, s: &str, size: f32, weight: u16, max: f32) -> String {
        fit(self.c, s, size, weight, max)
    }

    fn put(&self, s: &str, x: f32, base: f32, size: f32, ink: gfx::Color, weight: u16) {
        text(self.c, s, x, base, size, ink, weight);
    }

    /// `s` fitted into `max` points at `x`.
    #[allow(clippy::too_many_arguments)]
    fn line(&self, s: &str, x: f32, base: f32, max: f32, size: f32, ink: gfx::Color, weight: u16) {
        let shown = self.fit(s, size, weight, max);
        text(self.c, &shown, x, base, size, ink, weight);
    }

    /// Wrapped text from `y`; returns where it ends.
    #[allow(clippy::too_many_arguments)]
    fn para(
        &self,
        s: &str,
        x: f32,
        y: f32,
        w: f32,
        size: f32,
        ink: gfx::Color,
        weight: u16,
    ) -> f32 {
        let lh = size * 1.5;
        let mut y = y;
        for (a, b) in wrap(self.c, s, size, weight, w.max(40.0)) {
            text(self.c, &s[a..b], x, y + size, size, ink, weight);
            y += lh;
        }
        y
    }

    fn mood(&self, m: Mood) -> gfx::Color {
        match m {
            Mood::Good => self.k.good,
            Mood::Warn => self.k.warn,
            Mood::Bad => self.k.danger,
            Mood::Calm => self.k.muted,
            Mood::Info => self.k.ink,
        }
    }

    /// A section label in small capitals; returns its height.
    fn caps(&mut self, x: f32, y: f32, w: f32, label: &str) -> f32 {
        let ls = self.s(11.0);
        let _ = canvas2d::draw_text_styled(
            self.c,
            &label.to_uppercase(),
            gfx::Point { x, y: y + ls },
            ls,
            self.k.muted,
            gfx::TextStyle {
                letter_spacing: 0.8,
                ..style(600)
            },
        );
        self.said.heading(Rect::new(x, y, w, ls * 1.6), label);
        ls * 1.6
    }

    /// A badge: a dot and a word in the mood's colour, drawn from `x`;
    /// returns its width.
    fn badge(&self, x: f32, base: f32, label: &str, mood: Mood) -> f32 {
        let s = self.s(13.0);
        let ink = self.mood(mood);
        let _ = canvas2d::fill_circle(
            self.c,
            gfx::Point {
                x: x + 4.0,
                y: base - s * 0.35,
            },
            3.5,
            ink,
        );
        text(self.c, label, x + 13.0, base, s, ink, 600);
        13.0 + self.w(label, s, 600)
    }

    fn badge_w(&self, label: &str) -> f32 {
        13.0 + self.w(label, self.s(13.0), 600)
    }

    fn button(&mut self, r: Rect, label: &str, tone: Tone, hit: Hit) {
        let c = self.c;
        let k = self.k;
        let s = self.s(14.0);
        let (bg, fg, edge, weight) = match tone {
            Tone::Primary => (Some(k.accent), k.on_accent, None, 600),
            Tone::Plain => (Some(k.surface), k.ink, Some(k.line), 500),
            Tone::Quiet => (None, k.ink, None, 500),
            Tone::Danger => (None, k.danger, None, 500),
        };
        if let Some(bg) = bg {
            round(c, r, 8.0, bg);
        }
        if let Some(edge) = edge {
            outline(c, r, 8.0, edge);
        }
        let shown = fit(c, label, s, weight, r.w - 12.0);
        let w = width_of(c, &shown, s, weight);
        text(
            c,
            &shown,
            r.x + (r.w - w) / 2.0,
            baseline(r.y, r.h, s),
            s,
            fg,
            weight,
        );
        self.said.button(r, label);
        self.hits.push((r, hit));
    }

    /// The width a button with `label` wants.
    fn button_w(&self, label: &str, tone: Tone) -> f32 {
        let weight = if tone == Tone::Primary { 600 } else { 500 };
        self.w(label, self.s(14.0), weight) + if tone == Tone::Primary { 28.0 } else { 24.0 }
    }

    /// A round choice: filled when on.
    fn chip(&mut self, r: Rect, label: &str, on: bool, hit: Hit) {
        let c = self.c;
        let s = self.s(13.0);
        if on {
            round(c, r, r.h / 2.0, self.k.accent);
        } else {
            round(c, r, r.h / 2.0, self.k.surface);
            outline(c, r, r.h / 2.0, self.k.line);
        }
        let shown = fit(c, label, s, 500, r.w - 16.0);
        let w = width_of(c, &shown, s, 500);
        text(
            c,
            &shown,
            r.x + (r.w - w) / 2.0,
            baseline(r.y, r.h, s),
            s,
            if on { self.k.on_accent } else { self.k.ink },
            500,
        );
        self.said.toggle(r, label, on);
        self.hits.push((r, hit));
    }

    fn chip_w(&self, label: &str) -> f32 {
        self.w(label, self.s(13.0), 500) + 24.0
    }

    /// A boxed one-line field.
    #[allow(clippy::too_many_arguments)]
    fn field_box(
        &mut self,
        r: Rect,
        field: &Field,
        placeholder: &str,
        size: f32,
        weight: u16,
        focused: bool,
        wrong: bool,
    ) {
        let c = self.c;
        let k = self.k;
        round(c, r, 8.0, k.surface);
        outline(
            c,
            r,
            8.0,
            if wrong {
                k.danger
            } else if focused {
                k.ink
            } else {
                k.line
            },
        );
        let inner = Rect::new(r.x + 12.0, r.y, r.w - 24.0, r.h);
        let base = baseline(inner.y, inner.h, size);
        let _ = canvas2d::set_clip(c, inner.x - 2.0, inner.y, inner.w + 4.0, inner.h);
        if field.text.is_empty() {
            text(c, placeholder, inner.x, base, size, k.muted, 400);
        }
        let caret_x = width_of(c, &field.text[..field.caret], size, weight);
        let shift = (caret_x - (inner.w - 4.0)).max(0.0);
        text(c, &field.text, inner.x - shift, base, size, k.ink, weight);
        if focused {
            fill(
                c,
                Rect::new(
                    inner.x - shift + caret_x,
                    base - size * 0.95,
                    2.0,
                    size * 1.2,
                ),
                k.ink,
            );
        }
        let _ = canvas2d::clear_clip(c);
    }

    /// A boxed field of several lines; the caret sits where the text says.
    #[allow(clippy::too_many_arguments)]
    fn area_box(
        &mut self,
        r: Rect,
        field: &Field,
        placeholder: &str,
        size: f32,
        focused: bool,
        wrong: bool,
    ) {
        let c = self.c;
        let k = self.k;
        round(c, r, 8.0, k.surface);
        outline(
            c,
            r,
            8.0,
            if wrong {
                k.danger
            } else if focused {
                k.ink
            } else {
                k.line
            },
        );
        let inner = r.inset(9.0, 12.0, 9.0, 12.0);
        let lh = size * 1.45;
        let _ = canvas2d::set_clip(c, inner.x - 2.0, inner.y, inner.w + 4.0, inner.h);
        if field.text.is_empty() {
            text(c, placeholder, inner.x, inner.y + size, size, k.muted, 400);
        }
        let lines = wrap(c, &field.text, size, 400, inner.w - 4.0);
        let caret_line = lines
            .iter()
            .rposition(|(s, _)| *s <= field.caret)
            .unwrap_or(0);
        let rows = ((inner.h / lh).floor() as usize).max(1);
        let first = (caret_line + 1).saturating_sub(rows);
        let mut y = inner.y;
        for (i, (a, b)) in lines.iter().enumerate().skip(first) {
            text(c, &field.text[*a..*b], inner.x, y + size, size, k.ink, 400);
            if focused && i == caret_line {
                let cx = inner.x + width_of(c, &field.text[*a..field.caret.max(*a)], size, 400);
                fill(c, Rect::new(cx, y + 2.0, 2.0, size * 1.2), k.ink);
            }
            y += lh;
        }
        if focused && lines.is_empty() {
            fill(c, Rect::new(inner.x, inner.y + 2.0, 2.0, size * 1.2), k.ink);
        }
        let _ = canvas2d::clear_clip(c);
    }
}

// ---------------------------------------------------------------------------
// Forms: what the app asks for, the kit draws and checks
// ---------------------------------------------------------------------------

#[allow(dead_code)] // the kit is shared; not every app uses all of it
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// One line of text.
    Line,
    /// Several lines.
    Long,
    /// A number: hours, an amount, a count.
    Number,
    /// A day to come, with Today / Tomorrow / Next week one click away.
    Day,
    /// A day gone, with Today / Yesterday one click away.
    Past,
    /// A time of day, like 09:30.
    Time,
    /// One of a few words, as chips.
    Pick(&'static [&'static str]),
    /// A person, with the people this team's records name one click away.
    Person,
    /// Text, with what the team wrote in this field before one click away.
    Known(&'static str),
}

#[derive(Clone, Copy)]
struct Spec {
    key: &'static str,
    label: &'static str,
    hint: &'static str,
    kind: Kind,
    need: bool,
}

const fn spec(key: &'static str, label: &'static str, hint: &'static str, kind: Kind) -> Spec {
    Spec {
        key,
        label,
        hint,
        kind,
        need: false,
    }
}

const fn need(key: &'static str, label: &'static str, hint: &'static str, kind: Kind) -> Spec {
    Spec {
        key,
        label,
        hint,
        kind,
        need: true,
    }
}

struct Form {
    title: String,
    save: String,
    /// The collection the record lives in.
    coll: &'static str,
    /// Which of the app's forms this is.
    purpose: u16,
    specs: Vec<Spec>,
    vals: Vec<Field>,
    /// The record being changed, or None for a new one.
    editing: Option<Rec>,
    /// The record whose history this change belongs in.
    log_of: Option<String>,
    /// The id a new record gets, when there is only ever one of it.
    fixed_id: Option<String>,
    /// A line under the fields: who it will be from, what happens next.
    foot: String,
    error: Option<(usize, String)>,
    rects: Vec<(usize, Rect)>,
    scroll: f32,
    max: f32,
    body: Rect,
}

impl Form {
    fn new(title: &str, save: &str, coll: &'static str, specs: &[Spec], rec: Option<&Rec>) -> Form {
        let vals = specs
            .iter()
            .map(|s| {
                let text = rec.map(|r| r.shown(s.key)).unwrap_or_default();
                Field::with(&text, s.kind == Kind::Long)
            })
            .collect();
        Form {
            title: String::from(title),
            save: String::from(save),
            coll,
            purpose: 0,
            specs: specs.to_vec(),
            vals,
            editing: rec.cloned(),
            log_of: rec.map(|r| r.id.clone()),
            fixed_id: None,
            foot: String::new(),
            error: None,
            rects: Vec::new(),
            scroll: 0.0,
            max: 0.0,
            body: Rect::default(),
        }
    }

    fn at(&self, key: &str) -> Option<usize> {
        self.specs.iter().position(|s| s.key == key)
    }

    /// The trimmed text of a field.
    fn v(&self, key: &str) -> &str {
        self.at(key)
            .and_then(|i| self.vals.get(i))
            .map(|f| f.text.trim())
            .unwrap_or("")
    }

    #[allow(dead_code)] // the kit is shared; not every app uses all of it
    fn set(&mut self, key: &str, text: &str) {
        if let Some(i) = self.at(key) {
            self.vals[i].set(text);
        }
    }

    /// Every field as the store keeps it, checked: what is needed is there,
    /// numbers are numbers, days are days, times are times.
    fn values(&self) -> Result<Vec<(&'static str, Value)>, (usize, String)> {
        let mut out = Vec::new();
        for (i, s) in self.specs.iter().enumerate() {
            let t = self.vals[i].text.trim();
            if t.is_empty() {
                if s.need {
                    let what = match s.kind {
                        Kind::Pick(_) => "Pick one",
                        Kind::Day | Kind::Past => "Pick a day",
                        Kind::Person => "Say who",
                        _ => "Fill this in",
                    };
                    return Err((i, format!("{what}: {}.", s.label.to_lowercase())));
                }
                out.push((s.key, Value::Null));
                continue;
            }
            let value = match s.kind {
                Kind::Number => match parse_number(t) {
                    Some(n) if n >= 0.0 => Value::Number(n),
                    _ => {
                        return Err((
                            i,
                            format!("Write {} as a number, like 7.5.", s.label.to_lowercase()),
                        ))
                    }
                },
                Kind::Day | Kind::Past => {
                    if !valid_date(t) {
                        return Err((
                            i,
                            String::from("Write the day as year-month-day, like 2026-10-14."),
                        ));
                    }
                    Value::Text(String::from(t))
                }
                Kind::Time => match parse_time(t) {
                    Some(hm) => Value::Text(hm),
                    None => return Err((i, String::from("Write the time like 09:30."))),
                },
                _ => Value::Text(String::from(t)),
            };
            out.push((s.key, value));
        }
        Ok(out)
    }
}

/// What the kit needs to draw one row of the list.
struct Row {
    id: String,
    /// The heading above this row when it starts a new group.
    section: String,
    title: String,
    sub: String,
    right: String,
    badge: Option<(String, Mood)>,
    dim: bool,
    /// One button on the row itself: its label and the app's action.
    quick: Option<(String, u16)>,
}

impl Row {
    fn new(rec: &Rec, title: String) -> Row {
        Row {
            id: rec.id.clone(),
            section: String::new(),
            title,
            sub: String::new(),
            right: String::new(),
            badge: None,
            dim: false,
            quick: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    None,
    Search,
    Form(usize),
    Note,
}

// ---------------------------------------------------------------------------
// The app's shell: the same screens in all twelve
// ---------------------------------------------------------------------------

struct App {
    win: u64,
    c: u64,
    screen: Screen,
    w: f32,
    h: f32,
    team: Team,
    clock: Clock,
    colls: Vec<Coll>,
    /// Which of the app's views is showing.
    view: usize,
    search: Field,
    search_rect: Rect,
    focus: Focus,
    /// The record whose page is open.
    sel: Option<String>,
    /// On a phone: the record's page fills the screen instead of the list.
    phone_detail: bool,
    form: Option<Form>,
    note: Field,
    note_rect: Rect,
    rows: Vec<Row>,
    list_scroll: f32,
    list_max: f32,
    list_rect: Rect,
    detail_scroll: f32,
    detail_max: f32,
    detail_rect: Rect,
    board_scroll: f32,
    board_max: f32,
    board_rect: Rect,
    summary_h: f32,
    /// The Monday of the week a board shows.
    week: String,
    hits: Vec<(Rect, Hit)>,
    undo: Option<(Rec, u64)>,
    made: u32,
    titled: bool,
    #[allow(dead_code)] // the kit is shared; not every app uses all of it
    /// What only this app keeps between frames.
    st: State,
}

impl App {
    fn now(&self) -> u64 {
        krate::time::now_millis()
    }

    fn open_access(&self) -> bool {
        self.team.access == Access::Open
    }

    fn recs(&self, name: &str) -> &[Rec] {
        self.colls
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.recs.as_slice())
            .unwrap_or(&[])
    }

    fn main(&self) -> &[Rec] {
        self.recs(MAIN)
    }

    fn find(&self, name: &str, id: &str) -> Option<&Rec> {
        self.recs(name).iter().find(|r| r.id == id)
    }

    fn selected(&self) -> Option<Rec> {
        let id = self.sel.as_ref()?;
        self.find(MAIN, id).cloned()
    }

    #[allow(dead_code)] // the kit is shared; not every app uses all of it
    fn me(&self) -> String {
        self.team.me.clone()
    }

    fn new_id(&mut self, prefix: &str) -> String {
        self.made += 1;
        new_id(prefix, &self.team.me, self.made)
    }

    /// "you" for this person, a login without its email domain otherwise.
    fn short_name(&self, name: &str) -> String {
        if name.is_empty() {
            return String::from("someone");
        }
        if !self.team.me.is_empty() && name == self.team.me {
            return String::from("you");
        }
        String::from(name.split('@').next().unwrap_or(name))
    }

    /// "Carol" at the start of a sentence, "You" for this person.
    fn lead_name(&self, name: &str) -> String {
        let s = self.short_name(name);
        let mut chars = s.chars();
        match chars.next() {
            Some(first) => {
                let mut out: String = first.to_uppercase().collect();
                out.push_str(chars.as_str());
                out
            }
            None => s,
        }
    }

    /// The people this team's records name, the most recent first, this
    /// person among them.
    fn people(&self) -> Vec<String> {
        let mut seen: Vec<(String, u64)> = Vec::new();
        let mut add = |name: &str, at: u64| {
            let name = name.trim();
            if name.is_empty() {
                return;
            }
            match seen.iter_mut().find(|(n, _)| n == name) {
                Some((_, t)) => *t = (*t).max(at),
                None => seen.push((String::from(name), at)),
            }
        };
        for coll in &self.colls {
            for r in &coll.recs {
                add(&r.by, r.at);
                for key in PEOPLE {
                    add(r.t(key), r.at);
                }
            }
        }
        if !self.team.me.is_empty() {
            add(&self.team.me.clone(), u64::MAX);
        }
        seen.sort_by(|a, b| b.1.cmp(&a.1));
        seen.into_iter().map(|(n, _)| n).collect()
    }

    /// What a field offers one click away: people, or what was written in
    /// it before, the most recent first.
    fn suggestions(&self, kind: Kind) -> Vec<String> {
        match kind {
            Kind::Person => self.people(),
            Kind::Known(key) => {
                let mut seen: Vec<(String, u64)> = Vec::new();
                for coll in &self.colls {
                    for r in &coll.recs {
                        let v = r.t(key).trim();
                        if v.is_empty() {
                            continue;
                        }
                        match seen.iter_mut().find(|(n, _)| n == v) {
                            Some((_, t)) => *t = (*t).max(r.at),
                            None => seen.push((String::from(v), r.at)),
                        }
                    }
                }
                seen.sort_by(|a, b| b.1.cmp(&a.1));
                seen.into_iter().map(|(n, _)| n).collect()
            }
            _ => Vec::new(),
        }
    }

    /// One record's history: every change and note, oldest first.
    fn history(&self, of: &str) -> Vec<Rec> {
        let mut out: Vec<Rec> = self
            .recs(LOG)
            .iter()
            .filter(|r| r.t("of") == of)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.at.cmp(&b.at).then(a.id.cmp(&b.id)));
        out
    }

    /// Add a line to a record's history: what changed, in words.
    fn log(&mut self, of: &str, says: &str) {
        if !COLLECTIONS.contains(&LOG) || says.is_empty() {
            return;
        }
        let id = self.new_id("l");
        let fields = [
            ("of", Value::Text(String::from(of))),
            ("says", Value::Text(String::from(says))),
        ];
        self.team.put(LOG, &id, &fields);
    }

    /// Write some fields of a record, log what changed, read again.
    #[allow(dead_code)] // the kit is shared; not every app uses all of it
    fn write(
        &mut self,
        coll: &'static str,
        id: &str,
        fields: &[(&str, Value)],
        says: &str,
    ) -> bool {
        if !self.team.put(coll, id, fields) {
            return false;
        }
        let of = if coll == MAIN {
            String::from(id)
        } else {
            self.find(coll, id)
                .map(|r| String::from(r.t("of")))
                .filter(|o| !o.is_empty())
                .unwrap_or_else(|| String::from(id))
        };
        self.log(&of, says);
        self.reload();
        true
    }

    // ----- reading -----

    fn reload(&mut self) {
        for i in 0..self.colls.len() {
            let name = self.colls[i].name;
            if let Some((entries, cursor)) = self.team.read(name) {
                let mut recs: Vec<Rec> = entries.iter().map(Rec::of).collect();
                recs.sort_by(|a, b| b.at.cmp(&a.at));
                self.colls[i].recs = recs;
                self.colls[i].cursor = Some(cursor);
            }
        }
        if let Some(id) = self.sel.clone() {
            if self.find(MAIN, &id).is_none() && self.form.is_none() {
                self.sel = None;
                self.phone_detail = false;
            }
        }
    }

    /// Something changed since the cursor? Read again. Local and cheap.
    fn look_for_changes(&mut self) -> bool {
        let mut any = false;
        for i in 0..self.colls.len() {
            let name = self.colls[i].name;
            match self.colls[i].cursor.clone() {
                Some(cursor) => any |= self.team.changed(name, &cursor),
                None => any = true,
            }
        }
        if any {
            self.reload();
        }
        any
    }

    // ----- writing -----

    fn open_form(&mut self, form: Form) {
        let first = form
            .specs
            .iter()
            .position(|s| !matches!(s.kind, Kind::Pick(_)))
            .unwrap_or(0);
        self.form = Some(form);
        self.focus = Focus::Form(first);
    }

    /// Check the form, then write it: a new record whole, a changed one
    /// only in the fields that changed, so a teammate changing another field
    /// at the same time keeps theirs.
    fn submit(&mut self) {
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let checked = check(self, form);
        let fields = match checked {
            Ok(fields) => fields,
            Err((i, words)) => {
                if let Some(form) = self.form.as_mut() {
                    form.error = Some((i, words));
                }
                self.focus = Focus::Form(i);
                return;
            }
        };
        let Some(form) = self.form.take() else {
            return;
        };
        let new = form.editing.is_none();
        let id = match &form.editing {
            Some(r) => r.id.clone(),
            None => match &form.fixed_id {
                Some(id) => id.clone(),
                None => {
                    let prefix = if form.coll == MAIN { "r" } else { "s" };
                    self.new_id(prefix)
                }
            },
        };
        let write: Vec<(&str, Value)> = match &form.editing {
            Some(old) => fields
                .into_iter()
                .filter(|(k, v)| !same_value(old.get(k), v))
                .collect(),
            None => fields
                .into_iter()
                .filter(|(_, v)| !matches!(v, Value::Null))
                .collect(),
        };
        self.focus = Focus::None;
        if write.is_empty() {
            return;
        }
        let says = if new {
            String::new()
        } else {
            let names: Vec<String> = write
                .iter()
                .map(|(k, _)| {
                    form.specs
                        .iter()
                        .find(|s| s.key == *k)
                        .map(|s| s.label.to_lowercase())
                        .unwrap_or_else(|| String::from(*k))
                })
                .collect();
            format!("Changed {}", names.join(", "))
        };
        if !self.team.put(form.coll, &id, &write) {
            // Keep what was typed: the line under the bar says why.
            let mut form = form;
            if let Some(problem) = self.team.problem.take() {
                form.error = Some((form.specs.len().saturating_sub(1), problem));
            }
            self.form = Some(form);
            return;
        }
        let of = form.log_of.clone().unwrap_or_else(|| id.clone());
        if new && form.coll == MAIN {
            // What was just made opens, on a phone too.
            self.sel = Some(id.clone());
            self.detail_scroll = 0.0;
            self.phone_detail = true;
        }
        let says = log_words(self, &form, &id, new, &says);
        self.log(&of, &says);
        self.reload();
        saved(self, &form, &id, new);
    }

    fn delete_selected(&mut self) {
        let Some(rec) = self.selected() else {
            return;
        };
        self.form = None;
        self.focus = Focus::None;
        if self.team.delete(MAIN, &rec.id) {
            self.undo = Some((rec, self.now() + UNDO_FOR_MS));
            self.sel = None;
            self.phone_detail = false;
            self.reload();
        }
    }

    /// Put a deleted record back, every field as it was.
    fn undo_delete(&mut self) {
        let Some((rec, _)) = self.undo.take() else {
            return;
        };
        let fields: Vec<(&str, Value)> = rec
            .fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        if self.team.put(MAIN, &rec.id, &fields) {
            self.sel = Some(rec.id.clone());
            self.reload();
            self.log(&rec.id, "Brought back after a delete");
            self.reload();
        }
    }

    fn add_note(&mut self) {
        let words = String::from(self.note.text.trim());
        let Some(id) = self.sel.clone() else {
            return;
        };
        if words.is_empty() {
            return;
        }
        let lid = self.new_id("l");
        let fields = [
            ("of", Value::Text(id)),
            ("says", Value::Text(words)),
            ("note", Value::Boolean(true)),
        ];
        if self.team.put(LOG, &lid, &fields) {
            self.note.set("");
            self.reload();
        }
    }

    fn select(&mut self, id: &str) {
        if self.sel.as_deref() != Some(id) {
            self.note.set("");
            self.detail_scroll = 0.0;
        }
        self.sel = Some(String::from(id));
        self.phone_detail = true;
        if self.focus == Focus::Note {
            self.focus = Focus::None;
        }
    }

    // ----- drawing -----

    fn draw(&mut self) {
        let c = self.c;
        if let Ok(size) = canvas2d::canvas_size(c) {
            self.w = size.width;
            self.h = size.height;
        }
        let look = self.screen.look;
        let k = ink_for(&look);
        let mut p = Pen {
            c,
            k,
            look,
            said: Said::new(),
            hits: Vec::new(),
            tap: self.screen.tap().max(32.0),
            compact: self.screen.compact(),
        };
        let _ = canvas2d::clear(c, k.surface);
        let safe = Rect::new(0.0, 0.0, self.w, self.h).inset(
            self.screen.safe.0,
            self.screen.safe.1,
            self.screen.safe.2,
            self.screen.safe.3,
        );
        match self.team.access.clone() {
            Access::Open => {
                self.draw_open(safe, &mut p);
                if self.form.is_some() {
                    // The form is all a screen reader should hear now.
                    p.said = Said::new();
                    p.hits.clear();
                    self.draw_form(safe, &mut p);
                }
            }
            other => self.draw_closed(safe, &mut p, &other),
        }
        let _ = canvas2d::present(c);
        p.said.send(self.win);
        self.hits = p.hits;
    }

    /// The bar every screen starts with; returns what is left below it and
    /// the room left in the bar between the name and the right side.
    fn draw_bar(&mut self, area: Rect, p: &mut Pen, closed: bool) -> Rect {
        let c = self.c;
        let compact = p.compact;
        let side = if compact { 16.0 } else { 24.0 };
        let tap = p.tap;
        let mut rest = area;
        let bar = rest.take_top(tap + 24.0);
        fill(
            c,
            Rect::new(bar.x, bar.bottom() - 1.0, bar.w, 1.0),
            p.k.line,
        );
        let hs = p.s(17.0);
        let ms = p.s(13.0);
        let mut x = bar.x + side;
        let on_page = compact && self.phone_detail && self.sel.is_some() && !closed;
        if on_page {
            let b = Rect::new(x - 8.0, bar.y + (bar.h - tap) / 2.0, 72.0, tap);
            p.button(b, "Back", Tone::Quiet, Hit::Back);
            x = b.right() + 4.0;
        } else {
            p.put(APP_NAME, x, baseline(bar.y, bar.h, hs), hs, p.k.ink, 650);
            p.said.heading(Rect::new(x, bar.y, 140.0, bar.h), APP_NAME);
            x += p.w(APP_NAME, hs, 650) + 10.0;
        }
        if closed {
            return rest;
        }
        let mut right = bar.right() - side;
        let empty = self.main().is_empty();
        if !empty && !on_page {
            let label = if compact { NEW_SHORT } else { NEW_LABEL };
            let bw = p.button_w(label, Tone::Primary);
            let b = Rect::new(right - bw, bar.y + (bar.h - tap) / 2.0, bw, tap);
            p.button(b, label, Tone::Primary, Hit::New);
            right = b.x - 16.0;
        }
        let (status, dot) = self.team.status(self.now(), &p.k, &self.clock);
        let room = right - x - 16.0;
        if room > 40.0 {
            let shown = p.fit(&status, ms, 400, room - 14.0);
            let shown_w = p.w(&shown, ms, 400);
            let sx = right - shown_w;
            let _ = canvas2d::fill_circle(
                c,
                gfx::Point {
                    x: sx - 9.0,
                    y: bar.y + bar.h / 2.0,
                },
                3.5,
                dot,
            );
            p.put(&shown, sx, baseline(bar.y, bar.h, ms), ms, p.k.muted, 400);
            p.said.text(Rect::new(sx, bar.y, shown_w, bar.h), &status);
            right = sx - 20.0;
        }
        let count = count_line(self);
        if !on_page && !count.is_empty() && right - x > 60.0 {
            p.line(
                &count,
                x,
                baseline(bar.y, bar.h, ms),
                right - x,
                ms,
                p.k.muted,
                400,
            );
        }
        // The long story, when there is one.
        let explain = if empty && self.team.problem.is_none() && !self.team.trying() {
            None
        } else {
            self.team.explain()
        };
        if let Some(line) = explain {
            let s = p.s(13.0);
            let lines = wrap(c, &line, s, 400, (rest.w - side * 2.0 - 100.0).max(120.0));
            let band = rest.take_top(lines.len() as f32 * s * 1.5 + 20.0);
            fill(c, band, p.k.tint);
            let mut y = band.y + 10.0;
            for (a, b) in &lines {
                p.put(&line[*a..*b], band.x + side, y + s, s, p.k.ink, 400);
                y += s * 1.5;
            }
            p.said.text(band, &line);
            if self.team.problem.is_some() || self.team.trying() {
                let b = Rect::new(
                    band.right() - side - 80.0,
                    band.y + (band.h - 28.0) / 2.0,
                    80.0,
                    28.0,
                );
                p.button(b, "Dismiss", Tone::Quiet, Hit::Dismiss);
            }
        }
        rest
    }

    /// The whole window, when the team's records cannot be shown at all.
    fn draw_closed(&mut self, area: Rect, p: &mut Pen, why: &Access) {
        let c = self.c;
        let org = String::from(self.team.org_name());
        let lead = match org.as_str() {
            "your team" => String::from("Your team"),
            _ => org.clone(),
        };
        let (title, body): (String, String) = match why {
            Access::NotAnOrgApp => (
                format!("{APP_NAME} is waiting for its team"),
                format!(
                    "{ABOUT} Release it to your organisation on Krate, then open it from there. \
                     This copy is not an organisation's release, so there is no team to share \
                     with yet. You can try it here first: nothing you add is kept."
                ),
            ),
            Access::Refused(reason, message) => {
                let title = match reason.as_str() {
                    "removed" => format!("You are no longer in {org}"),
                    "not-a-member" => format!("You are not in {org}"),
                    "not-in-audience" => format!("{lead} has not shared {APP_NAME} with you"),
                    "release-withdrawn" => format!("This version of {APP_NAME} was withdrawn"),
                    "not-released" => format!("This version of {APP_NAME} is not released yet"),
                    "not-signed-in" => format!("Sign in to Krate to see your team's {THINGS}"),
                    "org-closed" => format!("{lead} is closed"),
                    _ => format!("{APP_NAME} cannot open your team's {THINGS}"),
                };
                let body = if message.trim().is_empty() || message.contains(reason.as_str()) {
                    match reason.as_str() {
                        "not-in-audience" => {
                            format!(
                                "Ask an admin of {org} to add you to the group it is shared with."
                            )
                        }
                        "removed" | "not-a-member" => format!("Ask an admin of {org} to add you."),
                        "release-withdrawn" | "not-released" => {
                            format!("Open the version {org} has released now, from {org}'s apps.")
                        }
                        "not-signed-in" => String::from("Sign in to Krate, then open it again."),
                        _ => format!("Ask an admin of {org}."),
                    }
                } else {
                    message.clone()
                };
                (title, body)
            }
            Access::Denied => (
                format!("{APP_NAME} was not given your team's {THINGS}"),
                format!(
                    "It needs them to show anything. Open it again and allow it to keep the \
                     {THINGS} with your team."
                ),
            ),
            Access::Unsupported => (
                format!("Open {APP_NAME} on your computer"),
                format!(
                    "Your team's {THINGS} are not available in a browser tab yet. Open \
                     {APP_NAME} with Krate on your computer and they are all there."
                ),
            ),
            Access::Open => (String::new(), String::new()),
        };
        let area = self.draw_bar(area, p, true);
        let pad = if p.compact { 24.0 } else { 48.0 };
        let max_w = (area.w - pad * 2.0).min(520.0);
        let x = area.x + (area.w - max_w) / 2.0;
        let ts = p.s(22.0);
        let bs = p.s(15.0);
        let title_lines = wrap(c, &title, ts, 650, max_w);
        let body_lines = wrap(c, &body, bs, 400, max_w);
        let can_try = *why == Access::NotAnOrgApp;
        let tap = p.tap.max(36.0);
        let total = title_lines.len() as f32 * ts * 1.3
            + 14.0
            + body_lines.len() as f32 * bs * 1.55
            + if can_try { 24.0 + tap } else { 0.0 };
        let mut y = area.y + ((area.h - total) / 2.0).max(pad);
        let top = y;
        for (s, e) in &title_lines {
            p.put(&title[*s..*e], x, y + ts, ts, p.k.ink, 650);
            y += ts * 1.3;
        }
        p.said.heading(Rect::new(x, top, max_w, y - top), &title);
        y += 14.0;
        let body_top = y;
        for (s, e) in &body_lines {
            p.put(&body[*s..*e], x, y + bs, bs, p.k.muted, 400);
            y += bs * 1.55;
        }
        p.said
            .text(Rect::new(x, body_top, max_w, y - body_top), &body);
        if can_try {
            y += 24.0;
            let label = "Try it here";
            let bw = p.button_w(label, Tone::Plain) + 8.0;
            p.button(Rect::new(x, y, bw, tap), label, Tone::Plain, Hit::Try);
        }
    }

    fn draw_open(&mut self, area: Rect, p: &mut Pen) {
        let c = self.c;
        let compact = p.compact;
        let rest = self.draw_bar(area, p, false);
        let now = self.now();
        if self.main().is_empty() && self.sel.is_none() {
            self.draw_empty(rest, p);
            self.draw_undo(area, p, now);
            return;
        }
        if compact {
            if self.phone_detail && self.selected().is_some() {
                self.draw_detail(rest, p, 16.0);
            } else {
                self.draw_main(rest, p, 16.0, true);
            }
        } else {
            let panel_w = (rest.w * 0.40).clamp(320.0, 460.0);
            let mut main = rest;
            let panel = main.take_right(panel_w);
            fill(c, panel, p.k.bg);
            fill(c, Rect::new(panel.x, panel.y, 1.0, panel.h), p.k.line);
            let pad = if rest.w > 1100.0 { 32.0 } else { 24.0 };
            self.draw_main(main, p, pad, false);
            if self.selected().is_some() {
                self.draw_detail(panel, p, 24.0);
            } else {
                self.detail_rect = Rect::default();
                let inner = panel.inset(22.0, 22.0, 22.0, 22.0);
                let _ = canvas2d::set_clip(c, panel.x, panel.y, panel.w, panel.h);
                summary(self, p, inner);
                let _ = canvas2d::clear_clip(c);
            }
        }
        self.draw_undo(area, p, now);
    }

    fn draw_empty(&mut self, area: Rect, p: &mut Pen) {
        let org = String::from(self.team.org_name());
        let max_w = (area.w - 48.0).min(500.0);
        let x = area.x + (area.w - max_w) / 2.0;
        let ts = p.s(22.0);
        let bs = p.s(15.0);
        let ss = p.s(13.0);
        let waiting = self.team.waiting_to_be_let_in();
        let (title, body, label) = if waiting {
            (
                format!("Your team's {THINGS} are on their way"),
                format!(
                    "This computer is new to your team. The {THINGS} appear here once a \
                     teammate's computer is online and lets this one in, which happens by \
                     itself. You can add one now; it is shared then."
                ),
                NEW_LABEL,
            )
        } else {
            let (t, b, l) = empty_words(&org);
            (t, b, l)
        };
        let lines = wrap(self.c, &body, bs, 400, max_w);
        let tap = p.tap.max(36.0);
        let starts = starts();
        let block = ts * 1.3
            + 12.0
            + lines.len() as f32 * bs * 1.55
            + 24.0
            + tap
            + if starts.is_empty() { 0.0 } else { 20.0 + tap }
            + 28.0
            + ss * 3.0;
        let mut y = area.y + ((area.h - block) / 2.0).max(24.0);
        let title_lines = wrap(self.c, &title, ts, 650, max_w);
        let top = y;
        for (a, b) in &title_lines {
            p.put(&title[*a..*b], x, y + ts, ts, p.k.ink, 650);
            y += ts * 1.3;
        }
        p.said.heading(Rect::new(x, top, max_w, y - top), &title);
        y += 12.0;
        let top = y;
        for (a, b) in &lines {
            p.put(&body[*a..*b], x, y + bs, bs, p.k.muted, 400);
            y += bs * 1.55;
        }
        p.said.text(Rect::new(x, top, max_w, y - top), &body);
        y += 24.0;
        let bw = p.button_w(label, Tone::Primary) + 4.0;
        p.button(Rect::new(x, y, bw, tap), label, Tone::Primary, Hit::New);
        y += tap + 20.0;
        if !starts.is_empty() {
            let small = p.s(13.0);
            let lead = "Or start from";
            let mut sx = x;
            let lw = p.w(lead, small, 400);
            if lw + 140.0 < max_w {
                p.put(lead, sx, baseline(y, tap, small), small, p.k.muted, 400);
                sx += lw + 10.0;
            }
            for (i, name) in starts.iter().enumerate() {
                let w = p.button_w(name, Tone::Plain);
                if sx + w > x + max_w + 8.0 {
                    break;
                }
                p.button(Rect::new(sx, y, w, tap), name, Tone::Plain, Hit::Start(i));
                sx += w + 8.0;
            }
            y += tap + 20.0;
        }
        y += 8.0;
        let end = p.para(LOCK, x, y, max_w, ss, p.k.muted, 400);
        p.said.text(Rect::new(x, y, max_w, end - y), LOCK);
    }

    /// The views, the search, and the list (or the app's own board).
    fn draw_main(&mut self, area: Rect, p: &mut Pen, pad: f32, with_summary: bool) {
        let c = self.c;
        let mut rest = area;
        let tap = p.tap.max(34.0);
        // Views and search, in one row when there is room.
        let views = views(self);
        let head = rest.take_top(tap + 28.0);
        let row = Rect::new(head.x + pad, head.y + 14.0, head.w - pad * 2.0, tap);
        let s = p.s(14.0);
        let seg_ws: Vec<f32> = views.iter().map(|v| p.w(v, s, 500) + 24.0).collect();
        let seg_total: f32 = seg_ws.iter().sum::<f32>() + 8.0;
        let search_min = 180.0;
        let same_row = views.len() <= 1 || seg_total + 16.0 + search_min <= row.w;
        let mut search_r = row;
        if views.len() > 1 {
            let track_w = seg_total.min(row.w);
            let track = Rect::new(row.x, row.y, track_w, row.h);
            round(c, track, 10.0, p.k.bg);
            let _ = canvas2d::set_clip(c, track.x, track.y, track.w, track.h);
            let mut x = row.x + 4.0;
            for (i, label) in views.iter().enumerate() {
                let w = seg_ws[i];
                let seg = Rect::new(x, row.y + 4.0, w, row.h - 8.0);
                let on = self.view == i;
                if on {
                    round(c, seg, 7.0, p.k.surface);
                    outline(c, seg, 7.0, p.k.line);
                }
                p.put(
                    label,
                    seg.x + 12.0,
                    baseline(seg.y, seg.h, s),
                    s,
                    if on { p.k.ink } else { p.k.muted },
                    if on { 600 } else { 500 },
                );
                p.said.toggle(seg, label, on);
                p.hits.push((seg, Hit::View(i)));
                x += w;
            }
            let _ = canvas2d::clear_clip(c);
            if same_row {
                search_r = Rect::new(
                    track.right() + 16.0,
                    row.y,
                    row.right() - track.right() - 16.0,
                    row.h,
                );
            } else if !is_board(self) {
                let next = rest.take_top(tap + 10.0);
                search_r = Rect::new(row.x, next.y, row.w, tap);
            }
        }
        let board_view = is_board(self);
        if !board_view {
            let ss = p.s(14.0);
            let clear_w = if self.search.text.is_empty() {
                0.0
            } else {
                60.0
            };
            let field = search_r;
            self.search_rect = field;
            let hint = search_hint();
            p.field_box(
                field,
                &self.search,
                hint,
                ss,
                400,
                self.focus == Focus::Search,
                false,
            );
            p.hits.push((field, Hit::Search));
            p.said.field(field, hint, &self.search.text);
            if clear_w > 0.0 {
                let b = Rect::new(
                    field.right() - clear_w - 4.0,
                    field.y + 4.0,
                    clear_w,
                    field.h - 8.0,
                );
                p.button(b, "Clear", Tone::Quiet, Hit::ClearSearch);
            }
        } else {
            self.search_rect = Rect::default();
        }

        // The app's own board (a week, a grid), or the list.
        if board_view {
            self.list_rect = Rect::default();
            self.board_rect = rest;
            let _ = canvas2d::set_clip(c, rest.x, rest.y, rest.w, rest.h);
            board(
                self,
                p,
                Rect::new(rest.x + pad, rest.y, rest.w - pad * 2.0, rest.h),
            );
            let _ = canvas2d::clear_clip(c);
            return;
        }
        self.board_rect = Rect::default();
        self.rows = rows(self);
        self.list_rect = rest;
        let s1 = p.s(15.0);
        let s2 = p.s(13.0);
        let ls = p.s(11.0);
        let row_h = (s1 * 1.4 + s2 * 1.5 + 24.0).max(tap + 22.0);
        let head_h = ls * 1.6 + 26.0;
        let mut content = 8.0;
        let mut last = String::new();
        for r in &self.rows {
            if !r.section.is_empty() && r.section != last {
                content += head_h;
                last = r.section.clone();
            }
            content += row_h;
        }
        if with_summary {
            content += self.summary_h + 24.0;
        }
        self.list_max = (content + 24.0 - rest.h).max(0.0);
        self.list_scroll = self.list_scroll.clamp(0.0, self.list_max);
        let _ = canvas2d::set_clip(c, rest.x, rest.y, rest.w, rest.h);
        let mut y = rest.y - self.list_scroll + 4.0;
        let x = rest.x + pad;
        let w = rest.w - pad * 2.0;
        if with_summary {
            let top = y + 8.0;
            let used = summary(self, p, Rect::new(x, top, w, 2000.0));
            self.summary_h = used;
            y = top + used + 16.0;
            if used > 0.0 {
                fill(c, Rect::new(rest.x, y - 8.0, rest.w, 1.0), p.k.line);
            }
        }
        if self.rows.is_empty() {
            let line = if self.search.text.trim().is_empty() {
                nothing_here(self)
            } else {
                format!("Nothing matches \"{}\".", self.search.text.trim())
            };
            p.line(&line, x, y + 30.0, w, p.s(15.0), p.k.muted, 400);
            p.said.text(Rect::new(x, y, w, 40.0), &line);
        }
        let sel = self.sel.clone();
        let mut last = String::new();
        let count = self.rows.len();
        for i in 0..count {
            let (section, row_id) = (self.rows[i].section.clone(), self.rows[i].id.clone());
            if !section.is_empty() && section != last {
                last = section.clone();
                if y + head_h >= rest.y && y <= rest.bottom() {
                    p.caps(x, y + 18.0, w, &section);
                }
                y += head_h;
            }
            if y + row_h >= rest.y && y <= rest.bottom() {
                let r = &self.rows[i];
                let row = Rect::new(x - 8.0, y, w + 16.0, row_h);
                let on = sel.as_deref() == Some(row_id.as_str()) && !compact_page(p);
                if on {
                    round(c, row, 8.0, p.k.tint);
                } else {
                    fill(c, Rect::new(x, y + row_h - 1.0, w, 1.0), p.k.line);
                }
                let mut right_edge = x + w;
                if let Some((label, _)) = &r.quick {
                    let bw = p.button_w(label, Tone::Plain).min(w * 0.4);
                    let b = Rect::new(right_edge - bw, y + (row_h - tap) / 2.0, bw, tap);
                    right_edge = b.x - 14.0;
                    p.button(b, label, Tone::Plain, Hit::Quick(i));
                }
                let ty = y + 12.0 + s1;
                let sy = ty + s2 * 1.6;
                let ink = if r.dim { p.k.muted } else { p.k.ink };
                let mut title_max = right_edge - x;
                if !r.right.is_empty() {
                    let rw = p.w(&r.right, s1, 600).min(w * 0.45);
                    p.line(&r.right, right_edge - rw, ty, rw, s1, ink, 600);
                    title_max = right_edge - rw - 16.0 - x;
                }
                p.line(&r.title, x, ty, title_max, s1, ink, 600);
                let mut sub_max = right_edge - x;
                if let Some((label, mood)) = &r.badge {
                    let bw = p.badge_w(label);
                    if bw < (right_edge - x) * 0.6 {
                        p.badge(right_edge - bw, sy, label, *mood);
                        sub_max = right_edge - bw - 16.0 - x;
                    }
                }
                p.line(&r.sub, x, sy, sub_max, s2, p.k.muted, 400);
                let mut words = format!("{}, {}", r.title, r.sub);
                if let Some((label, _)) = &r.badge {
                    words.push_str(", ");
                    words.push_str(label);
                }
                if !r.right.is_empty() {
                    words.push_str(", ");
                    words.push_str(&r.right);
                }
                let hit_w = right_edge - row.x;
                let visible = Rect::new(
                    row.x,
                    row.y.max(rest.y),
                    hit_w,
                    (row.bottom().min(rest.bottom()) - row.y.max(rest.y)).max(0.0),
                );
                p.said.button(visible, &words);
                p.hits.push((visible, Hit::Row(i)));
            }
            y += row_h;
        }
        let _ = canvas2d::clear_clip(c);
    }

    /// One record's page: what it is, what can be done to it, its history.
    fn draw_detail(&mut self, area: Rect, p: &mut Pen, pad: f32) {
        let c = self.c;
        let Some(rec) = self.selected() else {
            return;
        };
        let now = self.now();
        let inner = area.inset(20.0, pad, 0.0, pad);
        self.detail_rect = area;
        let _ = canvas2d::set_clip(c, area.x, area.y, area.w, area.h);
        let mut y = inner.y - self.detail_scroll;
        let top = y;
        let x = inner.x;
        let w = inner.w;
        let tap = p.tap.max(32.0);

        // The title, its badge, and closing it.
        let ts = p.s(if p.compact { 20.0 } else { 22.0 });
        let title = title_of(&rec);
        let close_w = if p.compact { 0.0 } else { 64.0 };
        if close_w > 0.0 {
            let b = Rect::new(x + w - close_w + 8.0, y - 4.0, close_w, tap);
            p.button(b, "Close", Tone::Quiet, Hit::Close);
        }
        let tl = wrap(c, &title, ts, 650, w - close_w - 8.0);
        let ty0 = y;
        for (a, b) in tl.iter().take(3) {
            p.put(&title[*a..*b], x, y + ts, ts, p.k.ink, 650);
            y += ts * 1.3;
        }
        p.said.heading(Rect::new(x, ty0, w, y - ty0), &title);
        y += 6.0;
        let s2 = p.s(13.0);
        let mut mx = x;
        if let Some((label, mood)) = badge_of(self, &rec) {
            let bw = p.badge(x, y + s2, &label, mood);
            p.said.text(Rect::new(x, y, bw, s2 * 1.5), &label);
            mx += bw + 14.0;
        }
        let meta = format!(
            "{} changed it {}",
            self.lead_name(&rec.by),
            self.clock.when(rec.at, now)
        );
        p.line(&meta, mx, y + s2, x + w - mx, s2, p.k.muted, 400);
        p.said.text(Rect::new(mx, y, x + w - mx, s2 * 1.5), &meta);
        y += s2 * 1.5 + 18.0;

        // What can be done to it: the app's actions, then Edit and Delete.
        let mut acts = actions(self, &rec);
        acts.push((String::from("Edit"), Tone::Plain, u16::MAX));
        let mut bx = x;
        for (label, tone, code) in &acts {
            let bw = p.button_w(label, *tone);
            if bx + bw > x + w && bx > x {
                bx = x;
                y += tap + 8.0;
            }
            let hit = if *code == u16::MAX {
                Hit::Edit
            } else {
                Hit::Act(*code)
            };
            p.button(Rect::new(bx, y, bw, tap), label, *tone, hit);
            bx += bw + 8.0;
        }
        y += tap + 22.0;

        // The facts, label above value.
        let ls = p.s(12.0);
        let vs = p.s(15.0);
        for (label, value) in facts(self, &rec) {
            if value.trim().is_empty() {
                continue;
            }
            p.put(label, x, y + ls, ls, p.k.muted, 500);
            y += ls * 1.5;
            let end = p.para(&value, x, y, w, vs, p.k.ink, 400);
            p.said.text(
                Rect::new(x, y - ls * 1.5, w, end - y + ls * 1.5),
                &format!("{label}: {value}"),
            );
            y = end + 10.0;
        }

        // What only this app shows here (a checklist, tasks, results).
        y += 4.0;
        let used = extra(self, p, Rect::new(x, y, w, 0.0), &rec);
        if used > 0.0 {
            y += used + 18.0;
        }

        // A note anyone can add, then everything that happened to it.
        y += 6.0;
        p.caps(x, y, w, "History");
        y += p.s(11.0) * 1.6 + 10.0;
        let field_h = tap.max(36.0);
        let add_w = p.button_w("Add", Tone::Plain);
        let nr = Rect::new(x, y, w - add_w - 8.0, field_h);
        self.note_rect = nr;
        p.field_box(
            nr,
            &self.note,
            NOTE_HINT,
            p.s(14.0),
            400,
            self.focus == Focus::Note,
            false,
        );
        p.hits.push((nr, Hit::Note));
        p.said.field(nr, NOTE_HINT, &self.note.text);
        p.button(
            Rect::new(nr.right() + 8.0, y, add_w, field_h),
            "Add",
            Tone::Plain,
            Hit::AddNote,
        );
        y += field_h + 16.0;
        let history = self.history(&rec.id);
        if history.is_empty() {
            let line = format!(
                "{} added it {}.",
                self.lead_name(&rec.by),
                self.clock.when(rec.at, now)
            );
            p.line(&line, x, y + s2, w, s2, p.k.muted, 400);
            p.said.text(Rect::new(x, y, w, s2 * 1.5), &line);
            y += s2 * 1.6;
        }
        for h in history.iter().rev() {
            let who = self.lead_name(&h.by);
            let when = self.clock.when(h.at, now);
            let head = format!("{who} \u{b7} {when}");
            let note = h.b("note");
            let hy = y;
            p.line(&head, x, y + s2, w, s2, p.k.muted, 500);
            y += s2 * 1.5;
            let says = h.t("says");
            let end = p.para(
                says,
                x,
                y,
                w,
                p.s(14.0),
                if note { p.k.ink } else { p.k.muted },
                400,
            );
            p.said
                .text(Rect::new(x, hy, w, end - hy), &format!("{head}: {says}"));
            y = end + 10.0;
        }
        y += 8.0;
        if self.team.trying() {
            y += 4.0;
        }
        let used = y - top;
        self.detail_max = (used + 24.0 - area.h + 20.0).max(0.0);
        self.detail_scroll = self.detail_scroll.clamp(0.0, self.detail_max);
        let _ = canvas2d::clear_clip(c);
    }

    fn draw_form(&mut self, area: Rect, p: &mut Pen) {
        let c = self.c;
        let compact = p.compact;
        let _ = canvas2d::fill_rect(c, gr(Rect::new(0.0, 0.0, self.w, self.h)), shade(0.38));
        p.hits
            .push((Rect::new(0.0, 0.0, self.w, self.h), Hit::Backdrop));
        let tap = p.tap.max(40.0);
        let ls = p.s(12.0);
        let fs = p.s(15.0);
        let gap = if compact { 12.0 } else { 16.0 };
        let today = self.clock.today(0);
        let offers: Vec<Vec<String>> = match self.form.as_ref() {
            Some(f) => f.specs.iter().map(|s| self.suggestions(s.kind)).collect(),
            None => Vec::new(),
        };
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let card_w = (area.w - 24.0).min(540.0);
        let inner_w = card_w - 48.0;
        let chip_h = (tap - 10.0).max(28.0);
        // Measure first: how tall each field is with its chips.
        let mut heights = Vec::new();
        for (i, s) in form.specs.iter().enumerate() {
            let mut h = ls * 1.6;
            h += match s.kind {
                Kind::Long => tap * 2.2,
                Kind::Pick(opts) => {
                    let mut rows = 1.0;
                    let mut cx = 0.0;
                    for o in opts {
                        let cw = p.chip_w(o);
                        if cx + cw > inner_w && cx > 0.0 {
                            rows += 1.0;
                            cx = 0.0;
                        }
                        cx += cw + 8.0;
                    }
                    rows * (chip_h + 8.0) - 8.0
                }
                Kind::Person | Kind::Known(_) => {
                    if offers.get(i).is_none_or(|o| o.is_empty()) {
                        tap
                    } else {
                        tap + 8.0 + chip_h
                    }
                }
                _ => tap,
            };
            heights.push(h + gap);
        }
        let ts = p.s(20.0);
        let head_h = ts * 1.6 + 12.0;
        let foot_lines = if form.foot.is_empty() {
            0.0
        } else {
            wrap(c, &form.foot, ls, 400, inner_w).len() as f32
        };
        let error_h = if form.error.is_some() {
            p.s(13.0) * 3.2
        } else {
            0.0
        };
        let fields_h: f32 = heights.iter().sum();
        let want = 24.0 + head_h + fields_h + foot_lines * ls * 1.5 + error_h + 12.0 + tap + 24.0;
        let card_h = want.min(area.h - 24.0);
        let card = Rect::new(
            area.x + (area.w - card_w) / 2.0,
            area.y + ((area.h - card_h) / 2.0).max(12.0),
            card_w,
            card_h,
        );
        let _ = canvas2d::drop_shadow_round_rect(c, gr(card), radii(14.0), 24.0, shade(0.25));
        round(c, card, 14.0, p.k.surface);
        p.hits.push((card, Hit::Card));
        let inner = card.inset(24.0, 24.0, 24.0, 24.0);
        let title = form.title.clone();
        p.put(
            &p.fit(&title, ts, 650, inner.w),
            inner.x,
            inner.y + ts,
            ts,
            p.k.ink,
            650,
        );
        p.said
            .heading(Rect::new(inner.x, inner.y, inner.w, ts * 1.4), &title);
        // The fields scroll between the title and the buttons.
        let body = Rect::new(
            card.x,
            inner.y + head_h,
            card.w,
            (inner.bottom() - tap - 12.0) - (inner.y + head_h),
        );
        let content = fields_h + foot_lines * ls * 1.5 + error_h;
        let max = (content - body.h).max(0.0);
        let scroll = form.scroll.clamp(0.0, max);
        let _ = canvas2d::set_clip(c, body.x, body.y, body.w, body.h);
        let mut y = body.y - scroll;
        let mut rects = Vec::new();
        let focus = self.focus;
        let specs = form.specs.clone();
        for (i, s) in specs.iter().enumerate() {
            let Some(form) = self.form.as_ref() else {
                break;
            };
            let field = &form.vals[i];
            let wrong = form.error.as_ref().is_some_and(|(w, _)| *w == i);
            let label = if s.need {
                String::from(s.label)
            } else {
                format!("{} (if you want)", s.label)
            };
            p.put(
                &p.fit(&label, ls, 500, inner.w),
                inner.x,
                y + ls,
                ls,
                if wrong { p.k.danger } else { p.k.muted },
                500,
            );
            y += ls * 1.6;
            match s.kind {
                Kind::Pick(opts) => {
                    let mut cx = inner.x;
                    let current = field.text.trim();
                    for (j, o) in opts.iter().enumerate() {
                        let cw = p.chip_w(o);
                        if cx + cw > inner.right() && cx > inner.x {
                            cx = inner.x;
                            y += chip_h + 8.0;
                        }
                        let r = Rect::new(cx, y, cw, chip_h);
                        p.chip(r, o, current == *o, Hit::Pick(i, j));
                        cx += cw + 8.0;
                    }
                    if wrong {
                        let r = Rect::new(inner.x - 4.0, y - 4.0, inner.w + 8.0, chip_h + 8.0);
                        outline(c, r, 10.0, p.k.danger);
                    }
                    y += chip_h + gap;
                }
                Kind::Long => {
                    let r = Rect::new(inner.x, y, inner.w, tap * 2.2);
                    p.area_box(r, field, s.hint, fs, focus == Focus::Form(i), wrong);
                    p.said.field(r, s.label, &field.text);
                    p.hits.push((r, Hit::Field(i)));
                    rects.push((i, r));
                    y += r.h + gap;
                }
                _ => {
                    let chips: Vec<(String, Hit)> = match s.kind {
                        Kind::Day => vec![
                            (String::from("Today"), Hit::Day(i, 0)),
                            (String::from("Tomorrow"), Hit::Day(i, 1)),
                            (String::from("In a week"), Hit::Day(i, 7)),
                        ],
                        Kind::Past => vec![
                            (String::from("Today"), Hit::Day(i, 0)),
                            (String::from("Yesterday"), Hit::Day(i, -1)),
                        ],
                        _ => Vec::new(),
                    };
                    let mut fw = inner.w;
                    if !chips.is_empty() {
                        fw = (p.s(150.0)).min(inner.w * 0.42);
                    }
                    let r = Rect::new(inner.x, y, fw, tap);
                    let weight = if s.kind == Kind::Number { 600 } else { 400 };
                    p.field_box(r, field, s.hint, fs, weight, focus == Focus::Form(i), wrong);
                    p.said.field(r, s.label, &field.text);
                    p.hits.push((r, Hit::Field(i)));
                    rects.push((i, r));
                    let mut cx = r.right() + 10.0;
                    for (label, hit) in &chips {
                        let cw = p.chip_w(label);
                        if cx + cw > inner.right() {
                            break;
                        }
                        let on = match hit {
                            Hit::Day(_, d) => field.text.trim() == add_days(&today, *d),
                            _ => false,
                        };
                        p.chip(
                            Rect::new(cx, y + (tap - chip_h) / 2.0, cw, chip_h),
                            label,
                            on,
                            *hit,
                        );
                        cx += cw + 8.0;
                    }
                    y += tap;
                    let offer = offers.get(i).cloned().unwrap_or_default();
                    if matches!(s.kind, Kind::Person | Kind::Known(_)) && !offer.is_empty() {
                        y += 8.0;
                        let mut cx = inner.x;
                        let current = field.text.trim();
                        for (j, name) in offer.iter().enumerate().take(6) {
                            let shown = if s.kind == Kind::Person {
                                self.short_name(name)
                            } else {
                                name.clone()
                            };
                            let cw = p.chip_w(&shown);
                            if cx + cw > inner.right() {
                                break;
                            }
                            p.chip(
                                Rect::new(cx, y, cw, chip_h),
                                &shown,
                                current == name,
                                Hit::Suggest(i, j),
                            );
                            cx += cw + 8.0;
                        }
                        y += chip_h;
                    }
                    y += gap;
                }
            }
        }
        let Some(form) = self.form.as_ref() else {
            return;
        };
        if !form.foot.is_empty() {
            let foot = form.foot.clone();
            let end = p.para(&foot, inner.x, y - 4.0, inner.w, ls, p.k.muted, 400);
            p.said.text(Rect::new(inner.x, y, inner.w, end - y), &foot);
            y = end + 4.0;
        }
        if let Some((_, words)) = form.error.clone() {
            let s = p.s(13.0);
            let end = p.para(&words, inner.x, y, inner.w, s, p.k.danger, 500);
            p.said.text(Rect::new(inner.x, y, inner.w, end - y), &words);
        }
        let _ = canvas2d::clear_clip(c);
        let editing = form.editing.is_some();
        let save = form.save.clone();
        let by = inner.bottom() - tap;
        let sw = p.button_w(&save, Tone::Primary) + 6.0;
        let save_r = Rect::new(inner.right() - sw, by, sw, tap);
        p.button(save_r, &save, Tone::Primary, Hit::Save);
        let cw = p.button_w("Cancel", Tone::Plain);
        let cancel_r = Rect::new(save_r.x - cw - 10.0, by, cw, tap);
        p.button(cancel_r, "Cancel", Tone::Plain, Hit::Cancel);
        if editing && (form.coll != MAIN || form.purpose == 0) {
            let dw = p.button_w("Delete", Tone::Danger);
            if cancel_r.x - dw - 10.0 > inner.x - 12.0 {
                p.button(
                    Rect::new(inner.x - 10.0, by, dw, tap),
                    "Delete",
                    Tone::Danger,
                    Hit::Delete,
                );
            }
        }
        if let Some(form) = self.form.as_mut() {
            form.rects = rects;
            form.max = max;
            form.scroll = scroll;
            form.body = body;
        }
    }

    fn draw_undo(&mut self, area: Rect, p: &mut Pen, now: u64) {
        let Some((rec, until)) = self.undo.as_ref() else {
            return;
        };
        if now > *until {
            self.undo = None;
            return;
        }
        let c = self.c;
        let s = p.s(14.0);
        let line = format!("Deleted \"{}\"", title_of(rec));
        let tap = p.tap.max(36.0);
        let shown = p.fit(&line, s, 400, (area.w - 140.0).min(380.0));
        let w = p.w(&shown, s, 400) + 110.0;
        let r = Rect::new(
            area.x + (area.w - w) / 2.0,
            area.bottom() - tap - 28.0,
            w,
            tap + 8.0,
        );
        let _ = canvas2d::drop_shadow_round_rect(c, gr(r), radii(10.0), 14.0, shade(0.22));
        round(c, r, 10.0, p.k.ink);
        p.put(
            &shown,
            r.x + 16.0,
            baseline(r.y, r.h, s),
            s,
            p.k.surface,
            400,
        );
        p.said.text(r, &line);
        let b = Rect::new(r.right() - 80.0, r.y + 4.0, 72.0, r.h - 8.0);
        let lw = p.w("Undo", s, 650);
        p.put(
            "Undo",
            b.x + (b.w - lw) / 2.0,
            baseline(b.y, b.h, s),
            s,
            p.k.surface,
            650,
        );
        p.said.button(b, "Undo");
        p.hits.push((b, Hit::Undo));
    }

    // ----- input -----

    fn tap(&mut self, x: f32, y: f32) -> bool {
        let Some(hit) = self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| r.contains(x, y))
            .map(|(_, h)| *h)
        else {
            if self.focus != Focus::None {
                self.focus = Focus::None;
                return true;
            }
            return false;
        };
        if !matches!(hit, Hit::Search | Hit::Note | Hit::Field(_)) && self.form.is_none() {
            self.focus = Focus::None;
        }
        match hit {
            Hit::New => {
                let form = form_for(self, 0, None);
                self.open_form(form);
            }
            Hit::Start(i) => start(self, i),
            Hit::Row(i) => {
                if let Some(id) = self.rows.get(i).map(|r| r.id.clone()) {
                    self.select(&id);
                }
            }
            Hit::Quick(i) => {
                if let Some((id, code)) = self
                    .rows
                    .get(i)
                    .and_then(|r| r.quick.as_ref().map(|q| (r.id.clone(), q.1)))
                {
                    act(self, code, &id);
                }
            }
            Hit::Back => {
                self.phone_detail = false;
                self.focus = Focus::None;
            }
            Hit::Close => {
                self.sel = None;
                self.phone_detail = false;
            }
            Hit::View(i) => {
                self.view = i;
                self.list_scroll = 0.0;
                self.board_scroll = 0.0;
            }
            Hit::Search => {
                self.focus = Focus::Search;
                let size = self.screen.look.text(14.0);
                let t = self.search.text.clone();
                self.search.caret = nearest(
                    self.c,
                    &t,
                    0,
                    t.len(),
                    x - self.search_rect.x - 12.0,
                    size,
                    400,
                );
            }
            Hit::ClearSearch => {
                self.search.set("");
                self.list_scroll = 0.0;
            }
            Hit::Field(i) => {
                self.focus = Focus::Form(i);
                let c = self.c;
                let size = self.screen.look.text(15.0);
                if let Some(form) = self.form.as_mut() {
                    let r = form
                        .rects
                        .iter()
                        .find(|(w, _)| *w == i)
                        .map(|(_, r)| *r)
                        .unwrap_or_default();
                    let weight = if form.specs[i].kind == Kind::Number {
                        600
                    } else {
                        400
                    };
                    let field = &mut form.vals[i];
                    if field.multiline {
                        field.caret = field.text.len();
                    } else {
                        let t = field.text.clone();
                        let caret_x = width_of(c, &t[..field.caret], size, weight);
                        let shift = (caret_x - (r.w - 28.0)).max(0.0);
                        field.caret =
                            nearest(c, &t, 0, t.len(), x - r.x - 12.0 + shift, size, weight);
                    }
                }
            }
            Hit::Pick(i, j) => {
                if let Some(form) = self.form.as_mut() {
                    if let Kind::Pick(opts) = form.specs[i].kind {
                        if let Some(o) = opts.get(j) {
                            let now: &str = if form.vals[i].text.trim() == *o && !form.specs[i].need
                            {
                                ""
                            } else {
                                o
                            };
                            form.vals[i].set(now);
                            form.error = None;
                        }
                    }
                }
            }
            Hit::Suggest(i, j) => {
                let kind = self
                    .form
                    .as_ref()
                    .and_then(|f| f.specs.get(i))
                    .map(|s| s.kind);
                let offer = kind.map(|k| self.suggestions(k)).unwrap_or_default();
                if let (Some(name), Some(form)) = (offer.get(j), self.form.as_mut()) {
                    form.vals[i].set(name);
                    form.error = None;
                }
            }
            Hit::Day(i, d) => {
                let day = self.clock.today(d);
                if let Some(form) = self.form.as_mut() {
                    form.vals[i].set(&day);
                    form.error = None;
                }
            }
            Hit::Save => self.submit(),
            Hit::Cancel | Hit::Backdrop => {
                self.form = None;
                self.focus = Focus::None;
            }
            Hit::Card => {
                self.focus = Focus::None;
            }
            Hit::Delete => {
                let sub = self.form.as_ref().filter(|f| f.coll != MAIN).and_then(|f| {
                    let rec = f.editing.as_ref()?;
                    let what = f.vals.first().map(|v| String::from(v.text.trim()));
                    Some((f.coll, rec.id.clone(), f.log_of.clone(), what))
                });
                match sub {
                    Some((coll, id, of, what)) => {
                        self.form = None;
                        self.focus = Focus::None;
                        if self.team.delete(coll, &id) {
                            if let (Some(of), Some(what)) = (of, what) {
                                self.log(&of, &format!("Removed {what}"));
                            }
                            self.reload();
                        }
                    }
                    None => self.delete_selected(),
                }
            }
            Hit::Edit => {
                if let Some(rec) = self.selected() {
                    let form = form_for(self, 0, Some(&rec));
                    self.open_form(form);
                }
            }
            Hit::Undo => self.undo_delete(),
            Hit::Dismiss => {
                if self.team.problem.is_some() {
                    self.team.problem = None;
                } else {
                    self.team.told = true;
                }
            }
            Hit::Try => {
                self.team.try_here();
                self.reload();
            }
            Hit::Note => self.focus = Focus::Note,
            Hit::AddNote => self.add_note(),
            Hit::Act(code) => {
                if let Some(id) = self.sel.clone() {
                    act(self, code, &id);
                }
            }
            Hit::Cell(code, n) => cell(self, code, n),
            Hit::Week(d) => {
                self.week = if d == 0 {
                    week_start(&self.clock.today(0))
                } else {
                    add_days(&self.week, d * 7)
                };
            }
            Hit::Tick(code, i) | Hit::Open(code, i) => cell(self, code, i as u32),
        }
        true
    }

    fn typed(&mut self, s: &str) -> bool {
        match self.focus {
            Focus::Search => {
                self.list_scroll = 0.0;
                self.search.insert(s)
            }
            Focus::Note => self.note.insert(s),
            Focus::Form(i) => {
                let Some(form) = self.form.as_mut() else {
                    return false;
                };
                let changed = form.vals.get_mut(i).is_some_and(|f| f.insert(s));
                if changed {
                    form.error = None;
                }
                changed
            }
            Focus::None => false,
        }
    }

    fn key(&mut self, key: &str, shift: bool) -> bool {
        if self.form.is_some() {
            return self.form_key(key, shift);
        }
        match (self.focus, key) {
            (_, "Escape") if self.focus != Focus::None => {
                self.focus = Focus::None;
                true
            }
            (Focus::None, "Escape") => {
                if self.sel.is_some() {
                    self.sel = None;
                    self.phone_detail = false;
                    return true;
                }
                false
            }
            (Focus::Note, "Enter") => {
                self.add_note();
                true
            }
            (Focus::Search, "Enter") => {
                if let Some(id) = self.rows.first().map(|r| r.id.clone()) {
                    self.select(&id);
                }
                self.focus = Focus::None;
                true
            }
            (Focus::None, "ArrowDown") | (Focus::None, "ArrowUp") => {
                if self.rows.is_empty() {
                    return false;
                }
                let at = self
                    .sel
                    .as_ref()
                    .and_then(|s| self.rows.iter().position(|r| &r.id == s));
                let next = match (at, key == "ArrowDown") {
                    (None, _) => 0,
                    (Some(i), true) => (i + 1).min(self.rows.len() - 1),
                    (Some(i), false) => i.saturating_sub(1),
                };
                let id = self.rows[next].id.clone();
                self.select(&id);
                self.phone_detail = false;
                true
            }
            (Focus::None, "PageDown")
            | (Focus::None, "PageUp")
            | (Focus::None, "End")
            | (Focus::None, "Home") => {
                let page = (self.list_rect.h.max(self.board_rect.h) - 60.0).max(100.0);
                let (scroll, max) = if self.board_rect.h > 0.0 {
                    (&mut self.board_scroll, self.board_max)
                } else {
                    (&mut self.list_scroll, self.list_max)
                };
                let before = *scroll;
                *scroll = match key {
                    "PageDown" => (*scroll + page).min(max),
                    "PageUp" => (*scroll - page).max(0.0),
                    "End" => max,
                    _ => 0.0,
                };
                before != *scroll
            }
            (Focus::Search, k) => {
                let (changed, handled) = self.search.key(k);
                if changed {
                    self.list_scroll = 0.0;
                }
                handled
            }
            (Focus::Note, k) => self.note.key(k).1,
            _ => false,
        }
    }

    fn form_key(&mut self, key: &str, shift: bool) -> bool {
        let Focus::Form(i) = self.focus else {
            return match key {
                "Escape" => {
                    self.form = None;
                    true
                }
                "Tab" => {
                    self.focus = Focus::Form(0);
                    true
                }
                "Enter" => {
                    self.submit();
                    true
                }
                _ => false,
            };
        };
        let Some(form) = self.form.as_mut() else {
            return false;
        };
        let n = form.specs.len();
        match key {
            "Escape" => {
                self.form = None;
                self.focus = Focus::None;
                true
            }
            "Tab" => {
                // Chips are pressed, not typed into: Tab skips them.
                let mut j = i;
                for _ in 0..n {
                    j = if shift { (j + n - 1) % n } else { (j + 1) % n };
                    if !matches!(form.specs[j].kind, Kind::Pick(_)) {
                        break;
                    }
                }
                self.focus = Focus::Form(j);
                true
            }
            "Enter" if form.specs[i].kind == Kind::Long && !shift => form.vals[i].insert("\n"),
            "Enter" => {
                self.submit();
                true
            }
            "ArrowUp" | "ArrowDown" if form.specs[i].kind == Kind::Long => {
                let f = &mut form.vals[i];
                f.caret = if key == "ArrowUp" { 0 } else { f.text.len() };
                true
            }
            _ => {
                let (changed, handled) = form.vals[i].key(key);
                if changed {
                    form.error = None;
                }
                handled
            }
        }
    }

    fn wheel(&mut self, x: f32, y: f32, dy: f32) -> bool {
        let pick = |scroll: &mut f32, max: f32| {
            let next = (*scroll + dy).clamp(0.0, max);
            let moved = next != *scroll;
            *scroll = next;
            moved
        };
        if let Some(form) = self.form.as_mut() {
            if form.body.contains(x, y) {
                return pick(&mut form.scroll, form.max);
            }
            return false;
        }
        if self.detail_rect.contains(x, y) {
            return pick(&mut self.detail_scroll, self.detail_max);
        }
        if self.board_rect.contains(x, y) {
            return pick(&mut self.board_scroll, self.board_max);
        }
        if self.list_rect.contains(x, y) {
            return pick(&mut self.list_scroll, self.list_max);
        }
        false
    }

    fn typing(&self) -> bool {
        self.form.is_some() || matches!(self.focus, Focus::Note)
    }

    fn name_window(&mut self) {
        if !self.titled && !self.team.org.is_empty() {
            self.titled = true;
            let _ = window::set_title(self.win, &format!("{APP_NAME} \u{b7} {}", self.team.org));
        }
    }
}

#[allow(dead_code)] // the kit is shared; not every app uses all of it
impl App {
    /// "This week", or the days it covers, with buttons either side; returns
    /// its height.
    fn week_nav(&mut self, p: &mut Pen, x: f32, y: f32, w: f32) -> f32 {
        let tap = p.tap.max(32.0);
        let this = week_start(&self.clock.today(0));
        let label = if self.week == this {
            String::from("This week")
        } else if self.week == add_days(&this, 7) {
            String::from("Next week")
        } else if self.week == add_days(&this, -7) {
            String::from("Last week")
        } else {
            format!("Week of {}", day_name(&self.week))
        };
        let span = format!(
            "{} to {}",
            day_name(&self.week),
            day_name(&add_days(&self.week, 6))
        );
        let hs = p.s(17.0);
        let ms = p.s(13.0);
        let pw = p.button_w("Earlier", Tone::Plain);
        let nw = p.button_w("Later", Tone::Plain);
        let tw = p.button_w("This week", Tone::Plain);
        let right = x + w;
        p.button(
            Rect::new(right - nw, y, nw, tap),
            "Later",
            Tone::Plain,
            Hit::Week(1),
        );
        p.button(
            Rect::new(right - nw - 8.0 - pw, y, pw, tap),
            "Earlier",
            Tone::Plain,
            Hit::Week(-1),
        );
        let mut room = right - nw - pw - 24.0 - x;
        if self.week != this && room > tw + 120.0 {
            p.button(
                Rect::new(right - nw - pw - 16.0 - tw, y, tw, tap),
                "This week",
                Tone::Plain,
                Hit::Week(0),
            );
            room -= tw + 8.0;
        }
        p.line(&label, x, baseline(y, tap, hs), room, hs, p.k.ink, 650);
        let lw = p.w(&label, hs, 650);
        if lw + 16.0 + p.w(&span, ms, 400) < room {
            p.put(
                &span,
                x + lw + 12.0,
                baseline(y, tap, hs),
                ms,
                p.k.muted,
                400,
            );
        }
        p.said
            .heading(Rect::new(x, y, room, tap), &format!("{label}, {span}"));
        tap
    }

    /// A record's own list (tasks, actions, people): a box to tick on the
    /// left when an item has one, its words, and a button to add one.
    /// Returns its height.
    #[allow(clippy::too_many_arguments)]
    fn sub_list(
        &mut self,
        p: &mut Pen,
        area: Rect,
        heading: &str,
        items: &[(String, String, Option<bool>)],
        tick: u16,
        open: u16,
        add: Option<(&str, u16)>,
    ) -> f32 {
        let c = self.c;
        let x = area.x;
        let w = area.w;
        let mut y = area.y;
        y += p.caps(x, y, w, heading) + 6.0;
        let s1 = p.s(15.0);
        let s2 = p.s(13.0);
        let tap = p.tap.max(32.0);
        let row_h = (s1 * 1.4 + s2 * 1.5 + 14.0).max(tap + 8.0);
        for (i, (title, sub, done)) in items.iter().enumerate() {
            let mut tx = x;
            if let Some(done) = done {
                let b = Rect::new(x, y + (row_h - 22.0) / 2.0, 22.0, 22.0);
                let hit = Rect::new(x - 6.0, y, 34.0, row_h);
                if *done {
                    round(c, b, 6.0, p.k.accent);
                    draw_tick(c, b, p.k.on_accent);
                } else {
                    round(c, b, 6.0, p.k.surface);
                    outline(c, b, 6.0, p.k.muted);
                }
                p.said.toggle(hit, &format!("Done: {title}"), *done);
                p.hits.push((hit, Hit::Tick(tick, i)));
                tx = x + 34.0;
            }
            let ink = if done.unwrap_or(false) {
                p.k.muted
            } else {
                p.k.ink
            };
            p.line(title, tx, y + 7.0 + s1, x + w - tx, s1, ink, 500);
            p.line(
                sub,
                tx,
                y + 7.0 + s1 + s2 * 1.55,
                x + w - tx,
                s2,
                p.k.muted,
                400,
            );
            let r = Rect::new(tx, y, x + w - tx, row_h);
            p.said.button(r, &format!("{title}, {sub}"));
            p.hits.push((r, Hit::Open(open, i)));
            fill(c, Rect::new(x, y + row_h - 1.0, w, 1.0), p.k.line);
            y += row_h;
        }
        if let Some((label, code)) = add {
            y += 10.0;
            let bw = p.button_w(label, Tone::Plain);
            p.button(
                Rect::new(x, y, bw, tap),
                label,
                Tone::Plain,
                Hit::Cell(code, 0),
            );
            y += tap;
        }
        y - area.y
    }
}

/// A tick inside `r`, drawn as a run of dots (the canvas has no lines).
#[allow(dead_code)] // the kit is shared; not every app uses all of it
fn draw_tick(c: u64, r: Rect, color: gfx::Color) {
    let pts = [(0.24, 0.52), (0.42, 0.70), (0.76, 0.32)];
    for seg in pts.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let x = r.x + r.w * (a.0 + (b.0 - a.0) * t);
            let y = r.y + r.h * (a.1 + (b.1 - a.1) * t);
            let _ = canvas2d::fill_circle(c, gfx::Point { x, y }, (r.w * 0.075).max(1.1), color);
        }
    }
}

/// On a phone the selected row is a page of its own, so it is not marked.
fn compact_page(p: &Pen) -> bool {
    p.compact
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create(
            APP_NAME,
            types::WindowSize {
                width: WINDOW.0,
                height: WINDOW.1,
            },
        ) else {
            return 1;
        };
        if tree::set_root(win, &node(1, None, types::WidgetKind::Stack)).is_err()
            || tree::upsert_node(win, &node(2, Some(1), types::WidgetKind::Canvas)).is_err()
        {
            return 1;
        }
        let Ok(c) = canvas2d::bind(win, 2) else {
            return 1;
        };
        let mut app = App {
            win,
            c,
            screen: Screen::of(win),
            w: WINDOW.0 as f32,
            h: WINDOW.1 as f32,
            team: Team::new(),
            clock: Clock::new(),
            colls: COLLECTIONS
                .iter()
                .copied()
                .map(|name| Coll {
                    name,
                    recs: Vec::new(),
                    cursor: None,
                })
                .collect(),
            view: 0,
            search: Field::new(false),
            search_rect: Rect::default(),
            focus: Focus::None,
            sel: None,
            phone_detail: false,
            form: None,
            note: Field::new(false),
            note_rect: Rect::default(),
            rows: Vec::new(),
            list_scroll: 0.0,
            list_max: 0.0,
            list_rect: Rect::default(),
            detail_scroll: 0.0,
            detail_max: 0.0,
            detail_rect: Rect::default(),
            board_scroll: 0.0,
            board_max: 0.0,
            board_rect: Rect::default(),
            summary_h: 0.0,
            week: String::new(),
            hits: Vec::new(),
            undo: None,
            made: 0,
            titled: false,
            st: State::new(),
        };

        app.week = week_start(&app.clock.today(0));
        // This computer's copy first, so the window opens at once; then one
        // sync, to learn who this is and to bring down what changed.
        app.reload();
        app.team.learn_who();
        app.draw();
        if app.open_access() {
            if app.team.sync() {
                app.reload();
            }
            app.name_window();
            app.draw();
        }

        let mut pressed: Option<(f32, f32)> = None;
        let mut looked = app.now();
        let mut synced = app.now();
        loop {
            let event = events::wait(Some(500));
            let mut dirty = false;
            match event {
                Some(types::Event::CloseRequested(_)) => {
                    let _ = window::close(win);
                    return 0;
                }
                Some(types::Event::Resized(_)) | Some(types::Event::ThemeChanged(_)) => {
                    app.screen = Screen::of(win);
                    dirty = true;
                }
                Some(types::Event::Pointer(p)) if p.button.is_some() => {
                    if p.pressed {
                        pressed = Some((p.x, p.y));
                    } else if let Some((x0, y0)) = pressed.take() {
                        if (p.x - x0).abs() < 10.0 && (p.y - y0).abs() < 10.0 {
                            dirty = app.tap(p.x, p.y);
                        }
                    }
                }
                Some(types::Event::Wheel(w)) => dirty = app.wheel(w.x, w.y, w.dy),
                Some(types::Event::TextInput(s)) => dirty = app.typed(&s),
                Some(types::Event::Key(k)) if k.pressed => {
                    dirty = app.key(&k.key, k.modifiers.shift)
                }
                _ => {}
            }
            let now = app.now();
            if app.open_access() {
                if now >= looked + LOOK_EVERY_MS {
                    looked = now;
                    dirty |= app.look_for_changes();
                    // The status words ("synced 2 min ago") age too.
                    dirty |= app.undo.is_some();
                }
                // Not while someone is typing: a sync can wait on the network.
                if now >= synced + SYNC_EVERY_MS && !app.typing() {
                    synced = now;
                    if app.team.sync() {
                        app.reload();
                    }
                    app.name_window();
                    dirty = true;
                }
            }
            if dirty {
                app.screen.look = Look::now();
                app.draw();
            }
        }
    }
}

krate::export!(Component);

// ===== End of the starter kit =====

// ---------------------------------------------------------------------------
// Incidents and their actions
// ---------------------------------------------------------------------------

const ACTIONS: &str = "actions";
const SEVERITIES: &[&str] = &["Low", "Medium", "High", "Critical"];

const FORM: &[Spec] = &[
    need(
        "what",
        "What happened",
        "Water leak in the server room",
        Kind::Line,
    ),
    need("severity", "How bad", "", Kind::Pick(SEVERITIES)),
    spec(
        "detail",
        "Details",
        "What you saw, who was there, what was done at once",
        Kind::Long,
    ),
    spec(
        "place",
        "Where",
        "Server room, floor 1",
        Kind::Known("place"),
    ),
    need("day", "When", "2026-10-14", Kind::Past),
    spec("owner", "Who follows it up", "A name", Kind::Person),
];

const ACTION: &[Spec] = &[
    need("what", "What needs doing", "Call the plumber", Kind::Line),
    spec("owner", "Who does it", "A name", Kind::Person),
    spec("due", "By when", "2026-10-20", Kind::Day),
];

const CLOSE: u16 = 1;
const REOPEN: u16 = 2;
const ADD_ACTION: u16 = 3;
const TICK: u16 = 4;
const OPEN_ACTION: u16 = 5;

struct State;

impl State {
    fn new() -> State {
        State
    }
}

fn open(r: &Rec) -> bool {
    r.t("status") != "Closed"
}

fn rank(s: &str) -> usize {
    SEVERITIES.iter().position(|x| *x == s).unwrap_or(1)
}

fn severity_mood(s: &str) -> Mood {
    match s {
        "Critical" => Mood::Bad,
        "High" => Mood::Warn,
        _ => Mood::Calm,
    }
}

fn actions_of<'a>(app: &'a App, id: &str) -> Vec<&'a Rec> {
    let mut out: Vec<&Rec> = app
        .recs(ACTIONS)
        .iter()
        .filter(|a| a.t("of") == id)
        .collect();
    out.sort_by(|a, b| {
        a.b("done").cmp(&b.b("done")).then(
            a.n("made")
                .unwrap_or(0.0)
                .total_cmp(&b.n("made").unwrap_or(0.0)),
        )
    });
    out
}

fn progress(app: &App, id: &str) -> (usize, usize) {
    let all = actions_of(app, id);
    (all.iter().filter(|a| a.b("done")).count(), all.len())
}

fn count_line(app: &App) -> String {
    match app.main().iter().filter(|r| open(r)).count() {
        0 => String::from("Nothing open"),
        1 => String::from("1 open"),
        n => format!("{n} open"),
    }
}

fn views(app: &App) -> Vec<String> {
    let all = app.main();
    vec![
        format!("Open ({})", all.iter().filter(|r| open(r)).count()),
        format!("Closed ({})", all.iter().filter(|r| !open(r)).count()),
        format!("All ({})", all.len()),
    ]
}

fn is_board(_app: &App) -> bool {
    false
}

fn board(_app: &mut App, _p: &mut Pen, _area: Rect) {}

fn search_hint() -> &'static str {
    "Search incidents"
}

fn rows(app: &App) -> Vec<Row> {
    let wanted = app.search.text.trim();
    let mut list: Vec<&Rec> = app
        .main()
        .iter()
        .filter(|r| match app.view {
            0 => open(r),
            1 => !open(r),
            _ => true,
        })
        .filter(|r| r.matches(&["what", "detail", "place", "owner", "reported_by"], wanted))
        .collect();
    list.sort_by(|a, b| {
        open(b)
            .cmp(&open(a))
            .then(rank(b.t("severity")).cmp(&rank(a.t("severity"))))
            .then(b.t("day").cmp(a.t("day")))
    });
    list.into_iter()
        .map(|r| {
            let mut row = Row::new(r, String::from(r.t("what")));
            let (done, all) = progress(app, &r.id);
            let mut parts: Vec<String> = vec![long_day(r.t("day"))];
            if !r.t("place").is_empty() {
                parts.push(String::from(r.t("place")));
            }
            parts.push(match all {
                0 => String::from("no actions yet"),
                _ => format!("{done} of {all} actions done"),
            });
            row.sub = parts.join(" \u{b7} ");
            let sev = match r.t("severity") {
                "" => "Medium",
                s => s,
            };
            row.badge = Some(if open(r) {
                (String::from(sev), severity_mood(sev))
            } else {
                (String::from("Closed"), Mood::Good)
            });
            row.dim = !open(r);
            if app.view == 0 {
                row.section = match sev {
                    "Critical" | "High" => String::from("High and critical"),
                    _ => String::from("Everything else"),
                };
            }
            row
        })
        .collect()
}

fn nothing_here(app: &App) -> String {
    match app.view {
        0 => String::from("Nothing is open."),
        1 => String::from("Nothing has been closed yet."),
        _ => String::from("No incidents yet."),
    }
}

fn summary(app: &mut App, p: &mut Pen, area: Rect) -> f32 {
    let x = area.x;
    let w = area.w;
    let mut y = area.y;
    let s1 = p.s(15.0);
    let all: Vec<Rec> = app.main().to_vec();
    y += p.caps(x, y, w, "Open by how bad") + 10.0;
    for sev in SEVERITIES.iter().rev() {
        let n = all
            .iter()
            .filter(|r| {
                open(r)
                    && (r.t("severity") == *sev || (r.t("severity").is_empty() && *sev == "Medium"))
            })
            .count();
        let value = format!("{n}");
        let vw = p.w(&value, s1, 600);
        let ink = if n > 0 {
            p.mood(severity_mood(sev))
        } else {
            p.k.ink
        };
        p.put(sev, x, y + s1, s1, p.k.ink, 400);
        p.put(
            &value,
            x + w - vw,
            y + s1,
            s1,
            if n > 0 && rank(sev) >= 2 {
                ink
            } else {
                p.k.ink
            },
            600,
        );
        p.said
            .text(Rect::new(x, y, w, s1 * 1.5), &format!("{sev}: {n} open"));
        y += s1 * 1.7;
    }
    y += 16.0;
    y += p.caps(x, y, w, "Actions not done") + 10.0;
    let today = app.clock.today(0);
    let open_ids: Vec<String> = all
        .iter()
        .filter(|r| open(r))
        .map(|r| r.id.clone())
        .collect();
    let mut todo: Vec<Rec> = app
        .recs(ACTIONS)
        .iter()
        .filter(|a| !a.b("done") && open_ids.iter().any(|i| i == a.t("of")))
        .cloned()
        .collect();
    todo.sort_by(|a, b| {
        let da = if valid_date(a.t("due")) {
            a.t("due")
        } else {
            "9999"
        };
        let db = if valid_date(b.t("due")) {
            b.t("due")
        } else {
            "9999"
        };
        da.cmp(db)
    });
    if todo.is_empty() {
        p.put("Nothing waiting.", x, y + s1, s1, p.k.muted, 400);
        y += s1 * 1.7;
    }
    for a in todo.iter().take(6) {
        let late = valid_date(a.t("due")) && a.t("due") < today.as_str();
        let when = if valid_date(a.t("due")) {
            day_name(a.t("due"))
        } else {
            String::new()
        };
        let ww = p.w(&when, s1, 500);
        p.line(a.t("what"), x, y + s1, w - ww - 12.0, s1, p.k.ink, 400);
        p.put(
            &when,
            x + w - ww,
            y + s1,
            s1,
            if late { p.k.danger } else { p.k.muted },
            500,
        );
        p.said.text(
            Rect::new(x, y, w, s1 * 1.5),
            &format!("{}, due {when}", a.t("what")),
        );
        y += s1 * 1.7;
    }
    y += 16.0;
    let end = p.para(LOCK, x, y, w, p.s(12.0), p.k.muted, 400);
    p.said.text(Rect::new(x, y, w, end - y), LOCK);
    end - area.y
}

fn empty_words(org: &str) -> (String, String, &'static str) {
    (
        String::from("When something goes wrong, write it down"),
        format!(
            "Anyone in {org} reports an incident: a leak, a fall, an outage, a near miss. Add \
             the actions that follow, tick them off, and close it when it is dealt with."
        ),
        "Report the first incident",
    )
}

fn starts() -> &'static [&'static str] {
    &[]
}

fn start(_app: &mut App, _i: usize) {}

fn form_for(app: &App, purpose: u16, rec: Option<&Rec>) -> Form {
    match purpose {
        ADD_ACTION | OPEN_ACTION => {
            let mut f = match (purpose, rec) {
                (OPEN_ACTION, Some(r)) => {
                    let mut f = Form::new(
                        "Change this action",
                        "Save changes",
                        ACTIONS,
                        ACTION,
                        Some(r),
                    );
                    f.log_of = Some(String::from(r.t("of")));
                    f
                }
                _ => {
                    let mut f = Form::new("Add an action", "Add action", ACTIONS, ACTION, None);
                    f.log_of = app.sel.clone();
                    f
                }
            };
            f.purpose = purpose;
            f
        }
        _ => {
            let mut f = match rec {
                Some(r) => Form::new("Change this incident", "Save changes", MAIN, FORM, Some(r)),
                None => Form::new("Report an incident", "Report it", MAIN, FORM, None),
            };
            if f.v("severity").is_empty() {
                f.set("severity", "Medium");
            }
            if f.v("day").is_empty() {
                f.set("day", &app.clock.today(0));
            }
            f
        }
    }
}

fn check(app: &App, form: &Form) -> Result<Vec<(&'static str, Value)>, (usize, String)> {
    let mut fields = form.values()?;
    match form.purpose {
        ADD_ACTION => {
            fields.push(("of", Value::Text(app.sel.clone().unwrap_or_default())));
            fields.push(("done", Value::Boolean(false)));
            fields.push(("made", Value::Number(app.now() as f64)));
        }
        OPEN_ACTION => {}
        _ if form.editing.is_none() => {
            fields.push(("status", Value::Text(String::from("Open"))));
            fields.push(("reported_by", Value::Text(app.me())));
        }
        _ => {}
    }
    Ok(fields)
}

fn log_words(app: &App, form: &Form, _id: &str, new: bool, changed: &str) -> String {
    match form.purpose {
        ADD_ACTION => match form.v("owner") {
            "" => format!("Added an action: {}", form.v("what")),
            o => format!(
                "Added an action for {}: {}",
                app.short_name(o),
                form.v("what")
            ),
        },
        OPEN_ACTION => format!("Changed the action \"{}\"", form.v("what")),
        _ if new => format!("Reported it, {}", form.v("severity").to_lowercase()),
        _ => String::from(changed),
    }
}

fn saved(_app: &mut App, _form: &Form, _id: &str, _new: bool) {}

fn title_of(r: &Rec) -> String {
    match r.t("what") {
        "" => String::from("An incident"),
        w => String::from(w),
    }
}

fn badge_of(app: &App, r: &Rec) -> Option<(String, Mood)> {
    let sev = match r.t("severity") {
        "" => "Medium",
        s => s,
    };
    if open(r) {
        let (done, all) = progress(app, &r.id);
        let tail = if all > 0 {
            format!(", {done} of {all} actions done")
        } else {
            String::new()
        };
        Some((
            format!("Open, {}{tail}", sev.to_lowercase()),
            severity_mood(sev),
        ))
    } else {
        Some((String::from("Closed"), Mood::Good))
    }
}

fn facts(app: &App, r: &Rec) -> Vec<(&'static str, String)> {
    vec![
        ("Details", String::from(r.t("detail"))),
        ("When", long_day(r.t("day"))),
        ("Where", String::from(r.t("place"))),
        ("Reported by", app.lead_name(r.t("reported_by"))),
        (
            "Who follows it up",
            match r.t("owner") {
                "" => String::from("Nobody yet"),
                o => app.lead_name(o),
            },
        ),
    ]
}

fn actions(_app: &App, r: &Rec) -> Vec<(String, Tone, u16)> {
    if open(r) {
        vec![(String::from("Close incident"), Tone::Primary, CLOSE)]
    } else {
        vec![(String::from("Reopen"), Tone::Plain, REOPEN)]
    }
}

fn act(app: &mut App, code: u16, id: &str) {
    let (done, all) = progress(app, id);
    match code {
        CLOSE => {
            let says = if done < all {
                format!("Closed it with {} actions not done", all - done)
            } else {
                String::from("Closed it")
            };
            app.write(
                MAIN,
                id,
                &[("status", Value::Text(String::from("Closed")))],
                &says,
            );
        }
        REOPEN => {
            app.write(
                MAIN,
                id,
                &[("status", Value::Text(String::from("Open")))],
                "Reopened it",
            );
        }
        _ => {}
    }
}

fn extra(app: &mut App, p: &mut Pen, area: Rect, r: &Rec) -> f32 {
    let today = app.clock.today(0);
    let items: Vec<(String, String, Option<bool>)> = actions_of(app, &r.id)
        .iter()
        .map(|a| {
            let mut parts: Vec<String> = Vec::new();
            if !a.t("owner").is_empty() {
                parts.push(app.lead_name(a.t("owner")));
            }
            if valid_date(a.t("due")) {
                let late = !a.b("done") && a.t("due") < today.as_str();
                parts.push(format!(
                    "{} {}",
                    if late { "late, was due" } else { "by" },
                    day_name(a.t("due"))
                ));
            }
            if a.b("done") {
                parts.push(format!("done by {}", app.short_name(&a.by)));
            }
            (
                String::from(a.t("what")),
                if parts.is_empty() {
                    String::from("Nobody named")
                } else {
                    parts.join(" \u{b7} ")
                },
                Some(a.b("done")),
            )
        })
        .collect();
    app.sub_list(
        p,
        area,
        "Actions",
        &items,
        TICK,
        OPEN_ACTION,
        Some(("Add an action", ADD_ACTION)),
    )
}

fn cell(app: &mut App, code: u16, n: u32) {
    let Some(id) = app.sel.clone() else {
        return;
    };
    let item = actions_of(app, &id).get(n as usize).map(|a| (*a).clone());
    match (code, item) {
        (ADD_ACTION, _) => {
            let form = form_for(app, ADD_ACTION, None);
            app.open_form(form);
        }
        (TICK, Some(a)) => {
            let done = !a.b("done");
            let says = if done {
                format!("Done: {}", a.t("what"))
            } else {
                format!("Not done after all: {}", a.t("what"))
            };
            app.write(ACTIONS, &a.id, &[("done", Value::Boolean(done))], &says);
        }
        (OPEN_ACTION, Some(a)) => {
            let form = form_for(app, OPEN_ACTION, Some(&a));
            app.open_form(form);
        }
        _ => {}
    }
}
