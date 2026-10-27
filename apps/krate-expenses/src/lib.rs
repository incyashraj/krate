//! Expenses: what the team spent, who is owed, what each month cost.
//!
//! One of the three starters an organisation releases in its first minute on
//! Krate (IC-958). It is built like Team Notes (read that one first: the
//! store, the sync, the states and the kit are the same), and adds what an
//! app of numbers needs:
//!
//! - **One record per expense** in the `expenses` collection: `amount` (a
//!   number), `currency`, `what`, `date` (`YYYY-MM-DD`, the day it was
//!   spent), an optional `note`, `spent_by` and `paid_back`.
//! - **Who spent it is a field, not `changed_by`.** Marking an expense paid
//!   back changes the record, so `changed_by` becomes whoever marked it;
//!   the person owed the money must not change with it.
//! - **An edit writes only the fields that changed.** If Dana fixes the
//!   amount while Sam marks it paid back, both changes stand, because they
//!   are different fields and neither wrote the other.
//! - **Totals are worked out on this computer** from the records, per
//!   currency: money in two currencies is never added together.
//! - **A wrong entry is easy to undo**: Delete keeps the expense for eight
//!   seconds behind an Undo, and paid back is a toggle.
//!
//! `#![no_std]`, so it imports only `krate:*`.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{Field, Look, Rect, Said, Screen};
use krate::math::F64Ext as _;
use krate::org::{self, Entry, OrgError, Value};
use krate::ui::{events, tree, types, window};

const APP_NAME: &str = "Expenses";
const EXPENSES: &str = "expenses";
/// What the team keeps here, for the kit's sentences ("Acme's expenses are full").
const THINGS: &str = "expenses";
const TOO_LONG: &str = "That receipt note is too long to save. Shorten it.";

/// How often the app asks the store whether anything changed (local).
const LOOK_EVERY_MS: u64 = 1_000;
/// How often the app asks for a sync of its own, to know online/pending.
const SYNC_EVERY_MS: u64 = 20_000;
/// How long Undo is offered after a delete.
const UNDO_FOR_MS: u64 = 8_000;

// ===== The starter kit: the same in Team Notes, Expenses and Stock List =====

// ---------------------------------------------------------------------------
// The team's store: what it said last, in words a person reads
// ---------------------------------------------------------------------------

/// Whether this run can use the team's data at all.
#[derive(Clone, PartialEq)]
enum Access {
    /// The organisation's release, opened by a member.
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

struct Team {
    access: Access,
    /// The signed-in person as the organisation names them; empty until a
    /// sync has said, then learnt from `who`.
    me: String,
    /// The organisation's name, for "Acme's notes".
    org: String,
    online: bool,
    pending: u32,
    /// When a sync last reached the organisation.
    reached_at: Option<u64>,
    /// A sync has answered at least once.
    answered: bool,
    /// The organisation has named this person: `who` answered once this
    /// computer held the team's key and exchanged data.
    admitted: bool,
    /// This computer is new to the team and its key has not arrived yet
    /// (`org::status` says `WaitingForKey`); a teammate's computer passes it
    /// on by itself.
    waiting_for_key: bool,
    /// Something was saved since the last sync answered.
    saved_since: bool,
    /// A problem worth a line that is not the end of the run: full, too
    /// long, this computer's copy unreadable.
    problem: Option<String>,
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
            waiting_for_key: false,
            saved_since: false,
            problem: None,
        }
    }

    fn org_name(&self) -> &str {
        if self.org.is_empty() {
            "your team"
        } else {
            &self.org
        }
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

    /// Ask the runtime for a sync now. It never waits: the sync runs in the
    /// background, and a later call says so when the team changed
    /// something. True when it did.
    fn sync(&mut self) -> bool {
        match org::sync() {
            Ok(state) => {
                self.saved_since = false;
                self.look();
                state.changed
            }
            Err(error) => {
                self.fail(error);
                false
            }
        }
    }

    /// Where this computer stands, from `org::status`: local and instant, so
    /// it is asked every second. True when the line in the corner changed.
    fn look(&mut self) -> bool {
        let before = (
            self.online,
            self.pending,
            self.waiting_for_key,
            self.admitted,
            self.answered,
        );
        match org::status() {
            Ok(standing) => {
                self.online = standing.link != org::Link::Offline;
                self.pending = standing.pending;
                self.waiting_for_key = standing.link == org::Link::WaitingForKey;
                self.answered |=
                    standing.reached_at.is_some() || standing.link == org::Link::Offline;
                if standing.reached_at.is_some() {
                    self.reached_at = standing.reached_at;
                }
                if self.org.is_empty() && !standing.org.is_empty() {
                    self.org = standing.org;
                }
                if !self.admitted {
                    self.learn_who();
                }
            }
            Err(error) => self.fail(error),
        }
        before
            != (
                self.online,
                self.pending,
                self.waiting_for_key,
                self.admitted,
                self.answered,
            )
    }

    /// Who this is, once the organisation has said.
    fn learn_who(&mut self) {
        if let Ok(Some(member)) = org::who() {
            self.me = member.name;
            self.org = member.org;
            self.admitted = true;
        }
    }

    /// Waiting for the team's key, which a teammate's computer passes on.
    fn waiting_to_be_let_in(&self) -> bool {
        self.waiting_for_key
    }

    /// One short line for the corner, and the colour of its dot.
    fn status(&self, now: u64, ink: &Ink, clock: &Clock) -> (String, gfx::Color) {
        if !self.answered {
            return (format!("Opening {}...", self.org_name()), ink.muted);
        }
        if !self.online {
            return (String::from("Offline, saved here"), ink.warn);
        }
        if self.waiting_for_key {
            return (String::from("Waiting for your team's key"), ink.warn);
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
        if self.waiting_to_be_let_in() {
            return Some(format!(
                "Waiting for your team's key: someone in {} opens Krate and it arrives. \
                 Anything you add now is kept and shared then.",
                self.org_name()
            ));
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Records: read here, ask only what changed
// ---------------------------------------------------------------------------

/// Every record in a collection, and the cursor to ask "what changed
/// since?" with. Local: it never waits for the network.
fn read_records(collection: &str) -> Result<(Vec<Entry>, String), OrgError> {
    let mut entries = Vec::new();
    let mut since: Option<String> = None;
    loop {
        let page = org::records(collection, since.as_deref())?;
        entries.extend(page.entries);
        since = Some(page.next);
        if !page.more {
            break;
        }
    }
    Ok((entries, since.unwrap_or_default()))
}

/// Whether anything in a collection changed after `cursor`: local, cheap,
/// fine to ask every second.
fn changed_since(collection: &str, cursor: &str) -> Result<bool, OrgError> {
    let page = org::records(collection, Some(cursor))?;
    Ok(!page.entries.is_empty() || !page.removed.is_empty())
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
// Dates, in the person's own time zone
// ---------------------------------------------------------------------------

struct Clock {
    tz: String,
    loc: krate::locale::LocaleId,
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

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

// ---------------------------------------------------------------------------
// A text field the app edits itself: krate::layout::Field, which selects
// with Shift and Cmd/Ctrl+A and takes the person's paste as typed text
// ---------------------------------------------------------------------------

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

/// Behind selected text: the accent, faint enough to read through.
fn selection_ink(k: &Ink) -> gfx::Color {
    gfx::Color {
        a: 0.28,
        ..k.accent
    }
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
            // The next word, with the spaces after it.
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
            // One word wider than the line: break it by characters.
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

/// Which line of `lines` holds byte `at`.
#[allow(dead_code)] // Team Notes edits wrapped text; the others edit one line.
fn line_of(lines: &[(usize, usize)], at: usize) -> usize {
    lines.iter().rposition(|(s, _)| *s <= at).unwrap_or(0)
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

#[derive(Clone, Copy)]
enum Tone {
    Primary,
    Plain,
    Quiet,
    Danger,
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

// ===== End of the starter kit =====

// ---------------------------------------------------------------------------
// Expenses and money
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Expense {
    id: String,
    amount: f64,
    currency: String,
    what: String,
    /// `YYYY-MM-DD`, the day it was spent.
    date: String,
    note: String,
    spent_by: String,
    paid_back: bool,
    changed_at: u64,
}

impl Expense {
    /// None for a record with no amount: what is left when one person
    /// deleted an expense as another changed one field of it.
    fn from_entry(entry: &Entry) -> Option<Expense> {
        let amount = match org::field(entry, "amount") {
            Some(Value::Number(n)) => *n,
            _ => return None,
        };
        Some(Expense {
            id: entry.id.clone(),
            amount,
            currency: String::from(org::text(entry, "currency").unwrap_or("EUR")),
            what: String::from(org::text(entry, "what").unwrap_or("")),
            date: String::from(org::text(entry, "date").unwrap_or("")),
            note: String::from(org::text(entry, "note").unwrap_or("")),
            spent_by: String::from(org::text(entry, "spent_by").unwrap_or(&entry.changed_by)),
            paid_back: matches!(org::field(entry, "paid_back"), Some(Value::Boolean(true))),
            changed_at: entry.changed_at,
        })
    }

    /// `YYYY-MM`.
    fn month(&self) -> &str {
        self.date.get(..7).unwrap_or("")
    }
}

/// Every expense, newest day first, and the cursor.
fn read_all() -> Result<(Vec<Expense>, String), OrgError> {
    let (entries, cursor) = read_records(EXPENSES)?;
    let mut all: Vec<Expense> = entries.iter().filter_map(Expense::from_entry).collect();
    all.sort_by(|a, b| b.date.cmp(&a.date).then(b.changed_at.cmp(&a.changed_at)));
    Ok((all, cursor))
}

fn symbol(currency: &str) -> &'static str {
    match currency {
        "EUR" => "\u{20ac}",
        "USD" => "$",
        "GBP" => "\u{a3}",
        "INR" => "\u{20b9}",
        "JPY" => "\u{a5}",
        "CNY" => "\u{a5}",
        "KRW" => "\u{20a9}",
        "AUD" => "A$",
        "CAD" => "C$",
        "NZD" => "NZ$",
        "SGD" => "S$",
        "HKD" => "HK$",
        "BRL" => "R$",
        "MXN" => "MX$",
        "ZAR" => "R",
        _ => "",
    }
}

fn decimals(currency: &str) -> u32 {
    match currency {
        "JPY" | "KRW" => 0,
        _ => 2,
    }
}

/// "€1,234.50", "CHF 80.00".
fn money(amount: f64, currency: &str) -> String {
    let places = decimals(currency);
    let scale = if places == 0 { 1.0 } else { 100.0 };
    let minor = (amount.abs() * scale).round() as u64;
    let whole = minor / scale as u64;
    let mut digits = format!("{whole}");
    let mut grouped = String::new();
    while digits.len() > 3 {
        let tail = digits.split_off(digits.len() - 3);
        grouped = format!(",{tail}{grouped}");
    }
    grouped = format!("{digits}{grouped}");
    let sign = if amount < 0.0 { "-" } else { "" };
    let body = if places == 0 {
        grouped
    } else {
        format!("{grouped}.{:02}", minor % 100)
    };
    match symbol(currency) {
        "" => format!("{sign}{currency} {body}"),
        s => format!("{sign}{s}{body}"),
    }
}

/// "12", "12.5", "12,50", "€ 1,200.00": the amount in it, or None.
fn parse_amount(text: &str) -> Option<f64> {
    let kept: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    if kept.is_empty() {
        return None;
    }
    // A comma with one or two digits after it and no dot is a decimal comma
    // ("12,50"); any other comma groups thousands ("1,200").
    let normal = match (kept.rfind(','), kept.contains('.')) {
        (Some(at), false) if kept.len() - at - 1 <= 2 => {
            kept.replacen(',', ".", 1).replace(',', "")
        }
        _ => kept.replace(',', ""),
    };
    let value: f64 = normal.parse().ok()?;
    (value.is_finite() && value > 0.0).then(|| (value * 100.0).round() / 100.0)
}

/// Totals per currency, in a steady order.
#[derive(Default)]
struct Sum(Vec<(String, f64)>);

impl Sum {
    fn add(&mut self, currency: &str, amount: f64) {
        match self.0.iter_mut().find(|(c, _)| c == currency) {
            Some((_, total)) => *total += amount,
            None => {
                self.0.push((String::from(currency), amount));
                self.0.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }
    }

    fn text(&self) -> String {
        let parts: Vec<String> = self.0.iter().map(|(c, a)| money(*a, c)).collect();
        parts.join(" + ")
    }
}

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

/// "2026-10" as "October 2026".
fn month_name(month: &str) -> String {
    let m: usize = month.get(5..7).and_then(|t| t.parse().ok()).unwrap_or(1);
    format!(
        "{} {}",
        FULL_MONTHS.get(m.clamp(1, 12) - 1).copied().unwrap_or(""),
        month.get(..4).unwrap_or("")
    )
}

/// "2026-10-14" as "14 Oct".
fn day_name(date: &str) -> String {
    let m: usize = date.get(5..7).and_then(|t| t.parse().ok()).unwrap_or(1);
    let d: u32 = date.get(8..10).and_then(|t| t.parse().ok()).unwrap_or(1);
    format!(
        "{d} {}",
        MONTHS.get(m.clamp(1, 12) - 1).copied().unwrap_or("")
    )
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

/// The currency people here most likely pay in, from where they are.
fn local_currency(loc: &str) -> &'static str {
    let region = loc.rsplit(['-', '_']).next().unwrap_or("").to_uppercase();
    match region.as_str() {
        "US" => "USD",
        "GB" => "GBP",
        "IN" => "INR",
        "JP" => "JPY",
        "AU" => "AUD",
        "CA" => "CAD",
        "NZ" => "NZD",
        "SG" => "SGD",
        "CH" => "CHF",
        "HK" => "HKD",
        "BR" => "BRL",
        "MX" => "MXN",
        "ZA" => "ZAR",
        "SE" => "SEK",
        "NO" => "NOK",
        "DK" => "DKK",
        "PL" => "PLN",
        "DE" | "FR" | "ES" | "IT" | "NL" | "BE" | "AT" | "IE" | "PT" | "FI" | "GR" | "SK"
        | "SI" | "EE" | "LV" | "LT" | "LU" | "MT" | "CY" | "HR" => "EUR",
        _ => "USD",
    }
}

// ---------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Amount,
    Currency,
    What,
    Date,
    Note,
}

const ORDER: [Which; 5] = [
    Which::Amount,
    Which::Currency,
    Which::What,
    Which::Date,
    Which::Note,
];

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Log,
    Edit(usize),
    Paid(usize),
    Field(Which),
    Currency(usize),
    Day(u64),
    Save,
    Cancel,
    Delete,
    Undo,
    Dismiss,
    Backdrop,
}

/// The expense being logged or changed.
struct Form {
    /// The record being changed, or None for a new one.
    editing: Option<Expense>,
    amount: Field,
    currency: Field,
    what: Field,
    date: Field,
    note: Field,
    spent_by: String,
    error: Option<(Which, String)>,
    rects: Vec<(Which, Rect)>,
}

impl Form {
    fn field(&mut self, which: Which) -> &mut Field {
        match which {
            Which::Amount => &mut self.amount,
            Which::Currency => &mut self.currency,
            Which::What => &mut self.what,
            Which::Date => &mut self.date,
            Which::Note => &mut self.note,
        }
    }

    fn get(&self, which: Which) -> &Field {
        match which {
            Which::Amount => &self.amount,
            Which::Currency => &self.currency,
            Which::What => &self.what,
            Which::Date => &self.date,
            Which::Note => &self.note,
        }
    }
}

struct Undo {
    expense: Expense,
    until: u64,
}

struct App {
    win: u64,
    c: u64,
    screen: Screen,
    w: f32,
    h: f32,
    team: Team,
    clock: Clock,
    all: Vec<Expense>,
    cursor: Option<String>,
    form: Option<Form>,
    focus: Option<Which>,
    list_scroll: f32,
    list_max: f32,
    list_rect: Rect,
    hits: Vec<(Rect, Hit)>,
    undo: Option<Undo>,
    made: u32,
    titled: bool,
    /// The currency this person used last, to start the next one in.
    last_currency: String,
}

impl App {
    fn now(&self) -> u64 {
        krate::time::now_millis()
    }

    fn open_access(&self) -> bool {
        self.team.access == Access::Open
    }

    fn reload(&mut self) {
        match read_all() {
            Ok((all, cursor)) => {
                self.all = all;
                self.cursor = Some(cursor);
                if self.last_currency.is_empty() {
                    let me = self.team.me.clone();
                    if let Some(e) = self.all.iter().find(|e| !me.is_empty() && e.spent_by == me) {
                        self.last_currency = e.currency.clone();
                    }
                }
            }
            Err(error) => self.team.fail(error),
        }
    }

    fn look_for_changes(&mut self) -> bool {
        let Some(cursor) = self.cursor.clone() else {
            self.reload();
            return true;
        };
        match changed_since(EXPENSES, &cursor) {
            Ok(true) => {
                self.reload();
                true
            }
            Ok(false) => false,
            Err(error) => {
                self.team.fail(error);
                true
            }
        }
    }

    fn today(&self, back_days: u64) -> String {
        let (y, m, d, _) = self
            .clock
            .parts(self.now().saturating_sub(back_days * 86_400_000));
        format!("{y:04}-{m:02}-{d:02}")
    }

    /// The currencies to offer as one click: the one to start in, then the
    /// ones the team has used, then the common ones.
    fn currencies(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let start = self.start_currency();
        out.push(start);
        for e in &self.all {
            if !out.contains(&e.currency) {
                out.push(e.currency.clone());
            }
        }
        for c in ["EUR", "USD", "GBP", "INR"] {
            if !out.iter().any(|o| o == c) {
                out.push(String::from(c));
            }
        }
        out.truncate(4);
        out
    }

    fn start_currency(&self) -> String {
        if !self.last_currency.is_empty() {
            return self.last_currency.clone();
        }
        if let Some(e) = self.all.first() {
            return e.currency.clone();
        }
        String::from(local_currency(&self.clock.loc.bcp47))
    }

    fn open_form(&mut self, editing: Option<Expense>) {
        let f = |multi: bool, text: &str| {
            let mut field = Field::new(multi);
            field.set(text);
            field
        };
        let form = match &editing {
            Some(e) => Form {
                amount: f(false, &trim_money(e.amount, &e.currency)),
                currency: f(false, &e.currency),
                what: f(false, &e.what),
                date: f(false, &e.date),
                note: f(false, &e.note),
                spent_by: e.spent_by.clone(),
                editing: editing.clone(),
                error: None,
                rects: Vec::new(),
            },
            None => Form {
                amount: f(false, ""),
                currency: f(false, &self.start_currency()),
                what: f(false, ""),
                date: f(false, &self.today(0)),
                note: f(false, ""),
                spent_by: self.team.me.clone(),
                editing: None,
                error: None,
                rects: Vec::new(),
            },
        };
        self.form = Some(form);
        self.focus = Some(Which::Amount);
    }

    /// Check the form, then write it: a new expense whole, a changed one
    /// only in the fields that changed.
    fn save(&mut self) {
        let me = self.team.me.clone();
        self.made += 1;
        let id_new = new_id("e", &me, self.made);
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let amount = parse_amount(&form.amount.text);
        let currency = form.currency.text.trim().to_uppercase();
        let what = String::from(form.what.text.trim());
        let date = String::from(form.date.text.trim());
        let note = String::from(form.note.text.trim());
        let problem = if amount.is_none() {
            Some((Which::Amount, "Enter how much it was, like 12.50."))
        } else if currency.len() != 3 || !currency.chars().all(|c| c.is_ascii_uppercase()) {
            Some((
                Which::Currency,
                "Use a three-letter currency code, like EUR or USD.",
            ))
        } else if what.is_empty() {
            Some((Which::What, "Say what it was for, like \"Train to Leeds\"."))
        } else if !valid_date(&date) {
            Some((
                Which::Date,
                "Write the day as year-month-day, like 2026-10-14.",
            ))
        } else {
            None
        };
        if let Some((which, words)) = problem {
            form.error = Some((which, String::from(words)));
            self.focus = Some(which);
            return;
        }
        let amount = amount.unwrap_or(0.0);
        let mut fields: Vec<(&str, Value)> = Vec::new();
        let note_value = if note.is_empty() {
            Value::Null
        } else {
            Value::Text(note.clone())
        };
        let id = match &form.editing {
            Some(old) => {
                if (old.amount - amount).abs() > 0.001 {
                    fields.push(("amount", Value::Number(amount)));
                }
                if old.currency != currency {
                    fields.push(("currency", Value::Text(currency.clone())));
                }
                if old.what != what {
                    fields.push(("what", Value::Text(what)));
                }
                if old.date != date {
                    fields.push(("date", Value::Text(date)));
                }
                if old.note != note {
                    fields.push(("note", note_value));
                }
                old.id.clone()
            }
            None => {
                fields.push(("amount", Value::Number(amount)));
                fields.push(("currency", Value::Text(currency.clone())));
                fields.push(("what", Value::Text(what)));
                fields.push(("date", Value::Text(date)));
                if !note.is_empty() {
                    fields.push(("note", note_value));
                }
                if !me.is_empty() {
                    fields.push(("spent_by", Value::Text(me)));
                }
                fields.push(("paid_back", Value::Boolean(false)));
                id_new
            }
        };
        if fields.is_empty() {
            self.form = None;
            self.focus = None;
            return;
        }
        let new = form.editing.is_none();
        match org::put(EXPENSES, &id, &fields) {
            Ok(()) => {
                // Before the first sync this computer does not know its
                // person's name, but the store does: the record it just
                // wrote says who changed it. Keep that as who spent it, or
                // whoever marks it paid back would become the one owed.
                if new && self.team.me.is_empty() {
                    if let Ok(Some(entry)) = org::get(EXPENSES, &id) {
                        let _ = org::put_text(EXPENSES, &id, "spent_by", &entry.changed_by);
                    }
                }
                self.last_currency = currency;
                self.form = None;
                self.focus = None;
                self.team.saved_since = true;
                self.team.problem = None;
                self.reload();
            }
            Err(error) => {
                self.team.fail(error);
                if let (Some(form), Some(problem)) = (self.form.as_mut(), self.team.problem.take())
                {
                    form.error = Some((Which::Note, problem));
                }
            }
        }
    }

    fn toggle_paid(&mut self, index: usize) {
        let Some(e) = self.all.get(index) else {
            return;
        };
        match org::put(
            EXPENSES,
            &e.id,
            &[("paid_back", Value::Boolean(!e.paid_back))],
        ) {
            Ok(()) => {
                self.team.saved_since = true;
                self.reload();
            }
            Err(error) => self.team.fail(error),
        }
    }

    fn delete(&mut self) {
        let Some(form) = self.form.take() else {
            return;
        };
        self.focus = None;
        let Some(old) = form.editing else {
            return;
        };
        match org::delete(EXPENSES, &old.id) {
            Ok(()) => {
                self.team.saved_since = true;
                self.undo = Some(Undo {
                    expense: old,
                    until: self.now() + UNDO_FOR_MS,
                });
                self.reload();
            }
            Err(error) => self.team.fail(error),
        }
    }

    fn undo_delete(&mut self) {
        let Some(undo) = self.undo.take() else {
            return;
        };
        let e = undo.expense;
        let mut fields = Vec::new();
        fields.push(("amount", Value::Number(e.amount)));
        fields.push(("currency", Value::Text(e.currency.clone())));
        fields.push(("what", Value::Text(e.what.clone())));
        fields.push(("date", Value::Text(e.date.clone())));
        if !e.note.is_empty() {
            fields.push(("note", Value::Text(e.note.clone())));
        }
        fields.push(("spent_by", Value::Text(e.spent_by.clone())));
        fields.push(("paid_back", Value::Boolean(e.paid_back)));
        match org::put(EXPENSES, &e.id, &fields) {
            Ok(()) => {
                self.team.saved_since = true;
                self.reload();
            }
            Err(error) => self.team.fail(error),
        }
    }

    fn short_name(&self, name: &str) -> String {
        if !self.team.me.is_empty() && name == self.team.me {
            return String::from("you");
        }
        String::from(name.split('@').next().unwrap_or(name))
    }

    /// What each person is still owed, per currency.
    fn owed(&self) -> Vec<(String, Sum, usize)> {
        let mut out: Vec<(String, Sum, usize)> = Vec::new();
        for e in self.all.iter().filter(|e| !e.paid_back) {
            match out.iter_mut().find(|(who, _, _)| *who == e.spent_by) {
                Some((_, sum, n)) => {
                    sum.add(&e.currency, e.amount);
                    *n += 1;
                }
                None => {
                    let mut sum = Sum::default();
                    sum.add(&e.currency, e.amount);
                    out.push((e.spent_by.clone(), sum, 1));
                }
            }
        }
        out
    }

    /// Each month's total, newest first.
    fn months(&self) -> Vec<(String, Sum)> {
        let mut out: Vec<(String, Sum)> = Vec::new();
        for e in &self.all {
            let month = String::from(e.month());
            match out.iter_mut().find(|(m, _)| *m == month) {
                Some((_, sum)) => sum.add(&e.currency, e.amount),
                None => {
                    let mut sum = Sum::default();
                    sum.add(&e.currency, e.amount);
                    out.push((month, sum));
                }
            }
        }
        out
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
        self.hits.clear();
        let mut said = Said::new();
        let _ = canvas2d::clear(c, k.surface);
        let safe = Rect::new(0.0, 0.0, self.w, self.h).inset(
            self.screen.safe.0,
            self.screen.safe.1,
            self.screen.safe.2,
            self.screen.safe.3,
        );
        match self.team.access.clone() {
            Access::Open => {
                self.draw_expenses(safe, &k, &look, &mut said);
                if self.form.is_some() {
                    // The form is all a screen reader should hear now.
                    said = Said::new();
                    self.hits.clear();
                    self.draw_form(safe, &k, &look, &mut said);
                }
            }
            other => self.draw_closed(safe, &k, &look, &mut said, &other),
        }
        let _ = canvas2d::present(c);
        said.send(self.win);
    }

    fn draw_closed(&mut self, area: Rect, k: &Ink, look: &Look, said: &mut Said, why: &Access) {
        let c = self.c;
        let org = String::from(self.team.org_name());
        // At the start of a sentence: "Acme", or "Your team".
        let lead = match org.as_str() {
            "your team" => String::from("Your team"),
            _ => org.clone(),
        };
        let (title, body): (String, String) = match why {
            Access::NotAnOrgApp => (
                String::from("Expenses is waiting for its team"),
                String::from(
                    "It keeps what your whole organisation spends in one place, encrypted so \
                     only your team can see it. Release it to your organisation on Krate, then \
                     open it from there. This copy is not an organisation's release, so there \
                     is no team to share with yet.",
                ),
            ),
            Access::Refused(reason, message) => {
                let title = match reason.as_str() {
                    "removed" => format!("You are no longer in {org}"),
                    "not-a-member" => format!("You are not in {org}"),
                    "not-in-audience" => format!("{lead} has not shared Expenses with you"),
                    "release-withdrawn" => String::from("This version of Expenses was withdrawn"),
                    "not-released" => String::from("This version of Expenses is not released yet"),
                    "not-signed-in" => String::from("Sign in to Krate to see your team's expenses"),
                    "org-closed" => format!("{lead} is closed"),
                    _ => String::from("Expenses cannot open your team's expenses"),
                };
                // The organisation's own words say what to do next; a bare
                // reason code is not words, so say it ourselves then.
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
                String::from("Expenses was not given your team's expenses"),
                String::from(
                    "It needs them to show anything. Open it again and allow it to keep the \
                     expenses with your team.",
                ),
            ),
            Access::Unsupported => (
                String::from("Open Expenses on your computer"),
                String::from(
                    "Your team's expenses are not available in a browser tab yet. Open \
                     Expenses with Krate on your computer and they are all there.",
                ),
            ),
            Access::Open => (String::new(), String::new()),
        };
        // The same bar as always, so the app is recognisably itself.
        let side = if self.screen.compact() { 16.0 } else { 24.0 };
        let mut area = area;
        let bar = area.take_top(self.screen.tap().max(32.0) + 24.0);
        fill(c, Rect::new(bar.x, bar.bottom() - 1.0, bar.w, 1.0), k.line);
        let hs = look.text(17.0);
        text(
            c,
            APP_NAME,
            bar.x + side,
            baseline(bar.y, bar.h, hs),
            hs,
            k.ink,
            650,
        );
        said.heading(Rect::new(bar.x + side, bar.y, 120.0, bar.h), APP_NAME);
        let pad = if self.screen.compact() { 24.0 } else { 48.0 };
        let max_w = (area.w - pad * 2.0).min(520.0);
        let x = area.x + (area.w - max_w) / 2.0;
        let ts = look.text(22.0);
        let bs = look.text(15.0);
        let title_lines = wrap(c, &title, ts, 650, max_w);
        let body_lines = wrap(c, &body, bs, 400, max_w);
        let total =
            title_lines.len() as f32 * ts * 1.3 + 14.0 + body_lines.len() as f32 * bs * 1.55;
        let mut y = area.y + ((area.h - total) / 2.0).max(pad);
        for (s, e) in &title_lines {
            text(c, &title[*s..*e], x, y + ts, ts, k.ink, 650);
            y += ts * 1.3;
        }
        said.heading(Rect::new(x, area.y, max_w, ts * 1.3), &title);
        y += 14.0;
        let top = y;
        for (s, e) in &body_lines {
            text(c, &body[*s..*e], x, y + bs, bs, k.muted, 400);
            y += bs * 1.55;
        }
        said.text(Rect::new(x, top, max_w, y - top), &body);
    }

    fn draw_expenses(&mut self, area: Rect, k: &Ink, look: &Look, said: &mut Said) {
        let c = self.c;
        let compact = self.screen.compact();
        let now = self.now();
        let side = if compact { 16.0 } else { 24.0 };
        let tap = self.screen.tap().max(32.0);
        let empty = self.all.is_empty();

        // ---- the bar ----
        let mut rest = area;
        let bar = rest.take_top(tap + 24.0);
        fill(c, Rect::new(bar.x, bar.bottom() - 1.0, bar.w, 1.0), k.line);
        let hs = look.text(17.0);
        let ms = look.text(13.0);
        let x = bar.x + side;
        text(c, APP_NAME, x, baseline(bar.y, bar.h, hs), hs, k.ink, 650);
        said.heading(Rect::new(x, bar.y, 120.0, bar.h), APP_NAME);
        let x = x + width_of(c, APP_NAME, hs, 650) + 10.0;
        let mut right = bar.right() - side;
        if !empty {
            let label = if compact { "Log" } else { "Log an expense" };
            let bw = width_of(c, label, look.text(14.0), 600) + 28.0;
            let b = Rect::new(right - bw, bar.y + (bar.h - tap) / 2.0, bw, tap);
            self.button(b, label, Tone::Primary, Hit::Log, k, look, said);
            right = b.x - 16.0;
        }
        let (status, dot) = self.team.status(now, k, &self.clock);
        let room = right - x - 16.0;
        if room > 40.0 {
            let shown = fit(c, &status, ms, 400, room - 14.0);
            let shown_w = width_of(c, &shown, ms, 400);
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
            text(c, &shown, sx, baseline(bar.y, bar.h, ms), ms, k.muted, 400);
            said.text(Rect::new(sx, bar.y, shown_w, bar.h), &status);
            right = sx - 20.0;
        }
        let count = match self.all.len() {
            0 => String::new(),
            1 => String::from("1 expense"),
            n => format!("{n} expenses"),
        };
        if !count.is_empty() && right - x > 60.0 {
            text(
                c,
                &fit(c, &count, ms, 400, right - x),
                x,
                baseline(bar.y, bar.h, ms),
                ms,
                k.muted,
                400,
            );
        }

        // ---- the long story, when there is one ----
        let explain = if empty && self.team.problem.is_none() {
            None
        } else {
            self.team.explain()
        };
        if let Some(line) = explain {
            let s = look.text(13.0);
            let lines = wrap(c, &line, s, 400, (rest.w - side * 2.0 - 90.0).max(120.0));
            let band = rest.take_top(lines.len() as f32 * s * 1.5 + 20.0);
            fill(c, band, k.tint);
            let mut y = band.y + 10.0;
            for (a, b) in &lines {
                text(c, &line[*a..*b], band.x + side, y + s, s, k.ink, 400);
                y += s * 1.5;
            }
            said.text(band, &line);
            if self.team.problem.is_some() {
                let b = Rect::new(
                    band.right() - side - 80.0,
                    band.y + (band.h - 28.0) / 2.0,
                    80.0,
                    28.0,
                );
                self.button(b, "Dismiss", Tone::Quiet, Hit::Dismiss, k, look, said);
            }
        }

        if empty {
            self.draw_empty(rest, k, look, said);
        } else if compact {
            self.draw_list(rest, k, look, said, side, true);
        } else {
            let panel_w = (rest.w * 0.32).clamp(260.0, 340.0);
            let mut main = rest;
            let panel = main.take_right(panel_w);
            fill(c, panel, k.bg);
            fill(c, Rect::new(panel.x, panel.y, 1.0, panel.h), k.line);
            self.draw_summary(panel.inset(20.0, 22.0, 20.0, 22.0), k, look, said);
            self.draw_list(
                main,
                k,
                look,
                said,
                if rest.w > 1100.0 { 40.0 } else { 28.0 },
                false,
            );
        }
        self.draw_undo(area, k, look, said, now);
    }

    fn draw_empty(&mut self, area: Rect, k: &Ink, look: &Look, said: &mut Said) {
        let c = self.c;
        let org = String::from(self.team.org_name());
        let max_w = (area.w - 48.0).min(480.0);
        let x = area.x + (area.w - max_w) / 2.0;
        let ts = look.text(22.0);
        let bs = look.text(15.0);
        let ss = look.text(13.0);
        let waiting = self.team.waiting_to_be_let_in();
        let (title, body, label) = if waiting {
            (
                "Your team's expenses are on their way",
                format!(
                    "Waiting for your team's key: someone in {org} opens Krate and it \
                     arrives. You can log an expense now; it is shared then."
                ),
                "Log an expense",
            )
        } else {
            (
                "Log what the team spends",
                format!(
                    "Everyone in {org} logs what they paid for here. It adds up who is owed \
                     what and what each month cost, and anyone can mark an expense paid back."
                ),
                "Log the first expense",
            )
        };
        let lines = wrap(c, &body, bs, 400, max_w);
        let tap = self.screen.tap().max(36.0);
        let block = ts * 1.3 + 12.0 + lines.len() as f32 * bs * 1.55 + 24.0 + tap + 28.0 + ss;
        let mut y = area.y + ((area.h - block) / 2.0).max(24.0);
        text(c, title, x, y + ts, ts, k.ink, 650);
        said.heading(Rect::new(x, y, max_w, ts * 1.3), title);
        y += ts * 1.3 + 12.0;
        let top = y;
        for (a, b) in &lines {
            text(c, &body[*a..*b], x, y + bs, bs, k.muted, 400);
            y += bs * 1.55;
        }
        said.text(Rect::new(x, top, max_w, y - top), &body);
        y += 24.0;
        let bw = width_of(c, label, look.text(14.0), 600) + 32.0;
        self.button(
            Rect::new(x, y, bw, tap),
            label,
            Tone::Primary,
            Hit::Log,
            k,
            look,
            said,
        );
        y += tap + 28.0;
        let lock = "Encrypted end to end: not even Krate can see your team's spending.";
        text(
            c,
            &fit(c, lock, ss, 400, max_w),
            x,
            y + ss,
            ss,
            k.muted,
            400,
        );
        said.text(Rect::new(x, y, max_w, ss * 1.5), lock);
    }

    /// Who is owed what, and each month's total.
    fn draw_summary(&mut self, area: Rect, k: &Ink, look: &Look, said: &mut Said) -> f32 {
        let c = self.c;
        let mut y = area.y;
        let ls = look.text(11.0);
        let s1 = look.text(15.0);
        let s2 = look.text(12.0);
        let heading = |c: u64, said: &mut Said, label: &str, y: f32| {
            let _ = canvas2d::draw_text_styled(
                c,
                label,
                gfx::Point {
                    x: area.x,
                    y: y + ls,
                },
                ls,
                k.muted,
                gfx::TextStyle {
                    letter_spacing: 0.8,
                    ..style(600)
                },
            );
            said.heading(Rect::new(area.x, y, area.w, ls * 1.6), label);
        };
        heading(c, said, "TO PAY BACK", y);
        y += ls * 1.6 + 12.0;
        let owed = self.owed();
        if owed.is_empty() {
            let line = "Everything is paid back.";
            text(c, line, area.x, y + s1, s1, k.muted, 400);
            said.text(Rect::new(area.x, y, area.w, s1 * 1.5), line);
            y += s1 * 1.6 + 8.0;
        }
        for (who, sum, n) in &owed {
            let you = !self.team.me.is_empty() && *who == self.team.me;
            let name = if you {
                String::from("You")
            } else {
                self.short_name(who)
            };
            let total = sum.text();
            let tw = width_of(c, &total, s1, 600).min(area.w * 0.6);
            text(
                c,
                &fit(c, &total, s1, 600, area.w * 0.6),
                area.right() - tw,
                y + s1,
                s1,
                k.ink,
                600,
            );
            text(
                c,
                &fit(c, &name, s1, 500, area.w - tw - 12.0),
                area.x,
                y + s1,
                s1,
                k.ink,
                500,
            );
            let detail = if *n == 1 {
                String::from("1 expense")
            } else {
                format!("{n} expenses")
            };
            text(c, &detail, area.x, y + s1 * 1.35 + s2, s2, k.muted, 400);
            let verb = if you { "are" } else { "is" };
            said.text(
                Rect::new(area.x, y, area.w, s1 * 2.6),
                &format!("{name} {verb} owed {total} for {detail}"),
            );
            y += s1 * 1.35 + s2 * 1.5 + 14.0;
        }
        y += 18.0;
        heading(c, said, "BY MONTH", y);
        y += ls * 1.6 + 12.0;
        for (month, sum) in self.months().iter().take(6) {
            let name = month_name(month);
            let total = sum.text();
            let tw = width_of(c, &total, s1, 600).min(area.w * 0.6);
            text(
                c,
                &fit(c, &total, s1, 600, area.w * 0.6),
                area.right() - tw,
                y + s1,
                s1,
                k.ink,
                600,
            );
            text(
                c,
                &fit(c, &name, s1, 400, area.w - tw - 12.0),
                area.x,
                y + s1,
                s1,
                k.ink,
                400,
            );
            said.text(
                Rect::new(area.x, y, area.w, s1 * 1.6),
                &format!("{name}: {total}"),
            );
            y += s1 * 1.6 + 8.0;
        }
        y += 18.0;
        let lock = "Encrypted end to end: not even Krate can see your team's spending.";
        let lines = wrap(c, lock, s2, 400, area.w);
        for (a, b) in &lines {
            text(c, &lock[*a..*b], area.x, y + s2, s2, k.muted, 400);
            y += s2 * 1.5;
        }
        said.text(
            Rect::new(
                area.x,
                y - lines.len() as f32 * s2 * 1.5,
                area.w,
                lines.len() as f32 * s2 * 1.5,
            ),
            lock,
        );
        y - area.y
    }

    fn draw_list(
        &mut self,
        area: Rect,
        k: &Ink,
        look: &Look,
        said: &mut Said,
        pad: f32,
        with_summary: bool,
    ) {
        let c = self.c;
        self.list_rect = area;
        let s1 = look.text(15.0);
        let s2 = look.text(13.0);
        let ls = look.text(11.0);
        let row_h = s1 * 1.4 + s2 * 1.5 + 22.0;
        let head_h = ls + 34.0;
        let mut content = 0.0;
        let mut last = "";
        for e in &self.all {
            if e.month() != last {
                content += head_h;
                last = e.month();
            }
            content += row_h;
        }
        let summary_h = if with_summary {
            260.0 + self.owed().len() as f32 * 48.0
        } else {
            0.0
        };
        self.list_max = (content + summary_h + 24.0 - area.h).max(0.0);
        self.list_scroll = self.list_scroll.clamp(0.0, self.list_max);
        let _ = canvas2d::set_clip(c, area.x, area.y, area.w, area.h);
        let mut y = area.y - self.list_scroll;
        if with_summary {
            let used = self.draw_summary(
                Rect::new(area.x + pad, y + 20.0, area.w - pad * 2.0, 400.0),
                k,
                look,
                said,
            );
            y += used + 36.0;
            fill(c, Rect::new(area.x, y - 12.0, area.w, 1.0), k.line);
        }
        let months = self.months();
        let all = self.all.clone();
        let mut last = String::new();
        let x = area.x + pad;
        let w = area.w - pad * 2.0;
        let tap = self.screen.tap().max(30.0);
        for (i, e) in all.iter().enumerate() {
            if e.month() != last {
                last = String::from(e.month());
                let label = month_name(&last).to_uppercase();
                let _ = canvas2d::draw_text_styled(
                    c,
                    &label,
                    gfx::Point {
                        x,
                        y: y + 22.0 + ls,
                    },
                    ls,
                    k.muted,
                    gfx::TextStyle {
                        letter_spacing: 0.8,
                        ..style(600)
                    },
                );
                if let Some((_, sum)) = months.iter().find(|(m, _)| *m == last) {
                    let t = sum.text();
                    let tw = width_of(c, &t, look.text(12.0), 500);
                    text(
                        c,
                        &t,
                        x + w - tw,
                        y + 22.0 + ls,
                        look.text(12.0),
                        k.muted,
                        500,
                    );
                    said.heading(
                        Rect::new(x, y + 12.0, w, head_h - 12.0),
                        &format!("{}, {t}", month_name(&last)),
                    );
                }
                y += head_h;
            }
            if y + row_h >= area.y && y <= area.bottom() {
                let row = Rect::new(x, y, w, row_h);
                fill(c, Rect::new(x, y, w, 1.0), k.line);
                // The day, the what and who, the amount, and paid back.
                let day_w = look.text(56.0);
                let ty = y + 11.0 + s1;
                text(c, &day_name(&e.date), x, ty, s2, k.muted, 400);
                let status_w = (look.text(118.0)).min(w * 0.3);
                let amount = money(e.amount, &e.currency);
                let aw = width_of(c, &amount, s1, 600);
                let ax = x + w - status_w - 16.0 - aw;
                let ink = if e.paid_back { k.muted } else { k.ink };
                text(c, &amount, ax, ty, s1, ink, 600);
                let tx = x + day_w;
                let tw = (ax - 16.0 - tx).max(20.0);
                text(c, &fit(c, &e.what, s1, 600, tw), tx, ty, s1, ink, 600);
                let by = if e.note.is_empty() {
                    format!("Paid by {}", self.short_name(&e.spent_by))
                } else {
                    format!("Paid by {} \u{b7} {}", self.short_name(&e.spent_by), e.note)
                };
                text(
                    c,
                    &fit(c, &by, s2, 400, tw),
                    tx,
                    ty + s2 * 1.55,
                    s2,
                    k.muted,
                    400,
                );
                self.hits.push((row, Hit::Edit(i)));
                said.button(
                    Rect::new(row.x, row.y, row.w - status_w, row.h),
                    &format!("{}, {amount}, {}, {}", e.what, day_name(&e.date), by),
                );
                // Paid back: a toggle anyone can flip.
                let st = Rect::new(x + w - status_w, y + (row_h - tap) / 2.0, status_w, tap);
                if e.paid_back {
                    let label = "Paid back";
                    let lw = width_of(c, label, s2, 600);
                    let cx = st.right() - lw;
                    let _ = canvas2d::fill_circle(
                        c,
                        gfx::Point {
                            x: cx - 10.0,
                            y: st.y + st.h / 2.0,
                        },
                        3.5,
                        k.good,
                    );
                    text(c, label, cx, baseline(st.y, st.h, s2), s2, k.good, 600);
                    said.toggle(st, &format!("Paid back: {}", e.what), true);
                } else {
                    round(c, st, 8.0, k.surface);
                    outline(c, st, 8.0, k.line);
                    let label = "Mark paid back";
                    let shown = fit(c, label, s2, 500, st.w - 12.0);
                    let lw = width_of(c, &shown, s2, 500);
                    text(
                        c,
                        &shown,
                        st.x + (st.w - lw) / 2.0,
                        baseline(st.y, st.h, s2),
                        s2,
                        k.ink,
                        500,
                    );
                    said.toggle(st, &format!("Mark paid back: {}", e.what), false);
                }
                self.hits.push((st, Hit::Paid(i)));
            }
            y += row_h;
        }
        let _ = canvas2d::clear_clip(c);
    }

    fn draw_form(&mut self, area: Rect, k: &Ink, look: &Look, said: &mut Said) {
        let c = self.c;
        let compact = self.screen.compact();
        let _ = canvas2d::fill_rect(
            c,
            gr(Rect::new(0.0, 0.0, self.w, self.h)),
            gfx::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.38,
            },
        );
        self.hits
            .push((Rect::new(0.0, 0.0, self.w, self.h), Hit::Backdrop));
        let tap = self.screen.tap().max(40.0);
        let ls = look.text(12.0);
        let fs = look.text(15.0);
        let gap = if compact { 10.0 } else { 14.0 };
        let field_block = ls * 1.6 + tap + gap;
        let editing = self.form.as_ref().is_some_and(|f| f.editing.is_some());
        let card_w = (area.w - 32.0).min(500.0);
        let card_h = (look.text(20.0) * 1.6
            + 16.0
            + field_block * 4.0
            + 34.0
            + ls * 2.0
            + 30.0
            + tap
            + 48.0)
            .min(area.h - 24.0);
        let card = Rect::new(
            area.x + (area.w - card_w) / 2.0,
            area.y + ((area.h - card_h) / 2.0).max(12.0),
            card_w,
            card_h,
        );
        let _ = canvas2d::drop_shadow_round_rect(
            c,
            gr(card),
            radii(14.0),
            24.0,
            gfx::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.25,
            },
        );
        round(c, card, 14.0, k.surface);
        self.hits.push((card, Hit::Dismiss));
        let inner = card.pad(24.0);
        let mut y = inner.y;
        let ts = look.text(20.0);
        let title = if editing {
            "Change this expense"
        } else {
            "Log an expense"
        };
        text(c, title, inner.x, y + ts, ts, k.ink, 650);
        said.heading(Rect::new(inner.x, y, inner.w, ts * 1.4), title);
        y += ts * 1.6 + 12.0;

        let mut rects = Vec::new();
        // Amount and currency, side by side.
        let cur_w = 92.0f32.min(inner.w * 0.3);
        let amount_r = Rect::new(inner.x, y + ls * 1.6, inner.w - cur_w - 10.0, tap);
        let cur_r = Rect::new(amount_r.right() + 10.0, amount_r.y, cur_w, tap);
        self.label(inner.x, y, "Amount", ls, k);
        self.label(cur_r.x, y, "Currency", ls, k);
        self.form_field(
            amount_r,
            Which::Amount,
            "0.00",
            look.text(18.0),
            600,
            k,
            said,
            "Amount",
        );
        self.form_field(cur_r, Which::Currency, "EUR", fs, 600, k, said, "Currency");
        rects.push((Which::Amount, amount_r));
        rects.push((Which::Currency, cur_r));
        y = amount_r.bottom() + 8.0;
        // One click for the currencies this team uses.
        let chips = self.currencies();
        let mut cx = inner.x;
        let chip_h = (tap - 10.0).max(28.0);
        let current = self
            .form
            .as_ref()
            .map(|f| f.currency.text.trim().to_uppercase())
            .unwrap_or_default();
        for (i, code) in chips.iter().enumerate() {
            let cw = width_of(c, code, look.text(13.0), 500) + 20.0;
            if cx + cw > inner.right() {
                break;
            }
            let r = Rect::new(cx, y, cw, chip_h);
            let on = *code == current;
            if on {
                round(c, r, chip_h / 2.0, k.accent);
            } else {
                outline(c, r, chip_h / 2.0, k.line);
            }
            let s = look.text(13.0);
            text(
                c,
                code,
                r.x + 10.0,
                baseline(r.y, r.h, s),
                s,
                if on { k.on_accent } else { k.ink },
                500,
            );
            said.button(r, code);
            self.hits.push((r, Hit::Currency(i)));
            cx += cw + 8.0;
        }
        y += chip_h + gap;

        let what_r = Rect::new(inner.x, y + ls * 1.6, inner.w, tap);
        self.label(inner.x, y, "What was it for", ls, k);
        self.form_field(
            what_r,
            Which::What,
            "Train to Leeds",
            fs,
            400,
            k,
            said,
            "What was it for",
        );
        rects.push((Which::What, what_r));
        y = what_r.bottom() + gap;

        let date_w = (look.text(140.0)).min(inner.w * 0.45);
        let date_r = Rect::new(inner.x, y + ls * 1.6, date_w, tap);
        self.label(inner.x, y, "Day", ls, k);
        self.form_field(date_r, Which::Date, "2026-10-14", fs, 400, k, said, "Day");
        rects.push((Which::Date, date_r));
        let mut dx = date_r.right() + 10.0;
        for (back, name) in [(0u64, "Today"), (1, "Yesterday")] {
            let s = look.text(13.0);
            let dw = width_of(c, name, s, 500) + 22.0;
            if dx + dw > inner.right() {
                break;
            }
            let r = Rect::new(dx, date_r.y + (tap - chip_h) / 2.0, dw, chip_h);
            let on = self
                .form
                .as_ref()
                .is_some_and(|f| f.date.text.trim() == self.today(back));
            if on {
                round(c, r, chip_h / 2.0, k.accent);
            } else {
                outline(c, r, chip_h / 2.0, k.line);
            }
            text(
                c,
                name,
                r.x + 11.0,
                baseline(r.y, r.h, s),
                s,
                if on { k.on_accent } else { k.ink },
                500,
            );
            said.button(r, name);
            self.hits.push((r, Hit::Day(back)));
            dx += dw + 8.0;
        }
        y = date_r.bottom() + gap;

        let note_r = Rect::new(inner.x, y + ls * 1.6, inner.w, tap);
        self.label(inner.x, y, "Receipt note (if you want one)", ls, k);
        self.form_field(
            note_r,
            Which::Note,
            "Booking ref, who it was for",
            fs,
            400,
            k,
            said,
            "Receipt note",
        );
        rects.push((Which::Note, note_r));
        y = note_r.bottom() + 12.0;

        let spent_by = self
            .form
            .as_ref()
            .map(|f| f.spent_by.clone())
            .unwrap_or_default();
        let who = if spent_by.is_empty() || spent_by == self.team.me {
            String::from("Paid by you, so you are the one owed it.")
        } else {
            format!(
                "Paid by {}, so {} is the one owed it.",
                self.short_name(&spent_by),
                self.short_name(&spent_by)
            )
        };
        text(
            c,
            &fit(c, &who, ls, 400, inner.w),
            inner.x,
            y + ls,
            ls,
            k.muted,
            400,
        );
        said.text(Rect::new(inner.x, y, inner.w, ls * 1.5), &who);
        y += ls * 1.5 + 6.0;
        if let Some((_, words)) = self.form.as_ref().and_then(|f| f.error.clone()) {
            let s = look.text(13.0);
            text(
                c,
                &fit(c, &words, s, 500, inner.w),
                inner.x,
                y + s,
                s,
                k.danger,
                500,
            );
            said.text(Rect::new(inner.x, y, inner.w, s * 1.5), &words);
        }

        // The one thing to do, and the ways out.
        let by = card.bottom() - 24.0 - tap;
        let save = if editing {
            "Save changes"
        } else {
            "Save expense"
        };
        let sw = width_of(c, save, look.text(14.0), 600) + 32.0;
        let save_r = Rect::new(inner.right() - sw, by, sw, tap);
        self.button(save_r, save, Tone::Primary, Hit::Save, k, look, said);
        let cw = width_of(c, "Cancel", look.text(14.0), 500) + 28.0;
        let cancel_r = Rect::new(save_r.x - cw - 10.0, by, cw, tap);
        self.button(cancel_r, "Cancel", Tone::Plain, Hit::Cancel, k, look, said);
        if editing {
            let dw = width_of(c, "Delete", look.text(14.0), 500) + 24.0;
            self.button(
                Rect::new(inner.x - 10.0, by, dw, tap),
                "Delete",
                Tone::Danger,
                Hit::Delete,
                k,
                look,
                said,
            );
        }
        if let Some(f) = self.form.as_mut() {
            f.rects = rects;
        }
    }

    fn label(&self, x: f32, y: f32, words: &str, size: f32, k: &Ink) {
        text(self.c, words, x, y + size, size, k.muted, 500);
    }

    /// A boxed one-line field of the form.
    #[allow(clippy::too_many_arguments)]
    fn form_field(
        &mut self,
        r: Rect,
        which: Which,
        placeholder: &str,
        size: f32,
        weight: u16,
        k: &Ink,
        said: &mut Said,
        name: &str,
    ) {
        let c = self.c;
        let focused = self.focus == Some(which);
        let wrong = self
            .form
            .as_ref()
            .and_then(|f| f.error.as_ref())
            .is_some_and(|(w, _)| *w == which);
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
        let Some(form) = self.form.as_ref() else {
            return;
        };
        let field = form.get(which);
        let inner = Rect::new(r.x + 12.0, r.y, r.w - 24.0, r.h);
        let base = baseline(inner.y, inner.h, size);
        let _ = canvas2d::set_clip(c, inner.x - 2.0, inner.y, inner.w + 4.0, inner.h);
        if field.text.is_empty() {
            text(c, placeholder, inner.x, base, size, k.muted, 400);
        }
        let caret_x = width_of(c, &field.text[..field.caret], size, weight);
        let shift = (caret_x - (inner.w - 4.0)).max(0.0);
        if let Some((a, b)) = field.selection().filter(|_| focused) {
            let x0 = width_of(c, &field.text[..a], size, weight);
            let x1 = width_of(c, &field.text[..b], size, weight);
            fill(
                c,
                Rect::new(
                    inner.x - shift + x0,
                    base - size * 0.95,
                    x1 - x0,
                    size * 1.2,
                ),
                selection_ink(k),
            );
        }
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
        said.field(r, name, &field.text);
        self.hits.push((r, Hit::Field(which)));
    }

    fn draw_undo(&mut self, area: Rect, k: &Ink, look: &Look, said: &mut Said, now: u64) {
        let Some(undo) = self.undo.as_ref() else {
            return;
        };
        if now > undo.until {
            self.undo = None;
            return;
        }
        let c = self.c;
        let s = look.text(14.0);
        let line = format!("Deleted \"{}\"", undo.expense.what);
        let tap = self.screen.tap().max(36.0);
        let shown = fit(c, &line, s, 400, (area.w - 140.0).min(380.0));
        let w = width_of(c, &shown, s, 400) + 110.0;
        let r = Rect::new(
            area.x + (area.w - w) / 2.0,
            area.bottom() - tap - 28.0,
            w,
            tap + 8.0,
        );
        let _ = canvas2d::drop_shadow_round_rect(
            c,
            gr(r),
            radii(10.0),
            14.0,
            gfx::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.22,
            },
        );
        round(c, r, 10.0, k.ink);
        text(
            c,
            &shown,
            r.x + 16.0,
            baseline(r.y, r.h, s),
            s,
            k.surface,
            400,
        );
        said.text(r, &line);
        let b = Rect::new(r.right() - 80.0, r.y + 4.0, 72.0, r.h - 8.0);
        let lw = width_of(c, "Undo", s, 650);
        text(
            c,
            "Undo",
            b.x + (b.w - lw) / 2.0,
            baseline(b.y, b.h, s),
            s,
            k.surface,
            650,
        );
        said.button(b, "Undo");
        self.hits.push((b, Hit::Undo));
    }

    #[allow(clippy::too_many_arguments)]
    fn button(
        &mut self,
        r: Rect,
        label: &str,
        tone: Tone,
        hit: Hit,
        k: &Ink,
        look: &Look,
        said: &mut Said,
    ) {
        let c = self.c;
        let s = look.text(14.0);
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
        said.button(r, label);
        self.hits.push((r, hit));
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
            return false;
        };
        match hit {
            Hit::Log => self.open_form(None),
            Hit::Edit(i) => {
                if let Some(e) = self.all.get(i).cloned() {
                    self.open_form(Some(e));
                }
            }
            Hit::Paid(i) => self.toggle_paid(i),
            Hit::Field(which) => {
                self.focus = Some(which);
                let c = self.c;
                let size = if which == Which::Amount {
                    self.screen.look.text(18.0)
                } else {
                    self.screen.look.text(15.0)
                };
                let weight = if matches!(which, Which::Amount | Which::Currency) {
                    600
                } else {
                    400
                };
                if let Some(form) = self.form.as_mut() {
                    let r = form
                        .rects
                        .iter()
                        .find(|(w, _)| *w == which)
                        .map(|(_, r)| *r)
                        .unwrap_or_default();
                    let field = form.field(which);
                    let t = field.text.clone();
                    field.move_to(
                        nearest(c, &t, 0, t.len(), x - r.x - 12.0, size, weight),
                        false,
                    );
                }
            }
            Hit::Currency(i) => {
                let codes = self.currencies();
                if let (Some(code), Some(form)) = (codes.get(i), self.form.as_mut()) {
                    form.currency.set(code);
                }
            }
            Hit::Day(back) => {
                let day = self.today(back);
                if let Some(form) = self.form.as_mut() {
                    form.date.set(&day);
                }
            }
            Hit::Save => self.save(),
            Hit::Cancel | Hit::Backdrop => {
                self.form = None;
                self.focus = None;
            }
            Hit::Delete => self.delete(),
            Hit::Undo => self.undo_delete(),
            Hit::Dismiss => {
                if self.form.is_none() {
                    self.team.problem = None;
                }
            }
        }
        true
    }

    fn typed(&mut self, s: &str) -> bool {
        let Some(which) = self.focus else {
            return false;
        };
        let Some(form) = self.form.as_mut() else {
            return false;
        };
        let field = form.field(which);
        let changed = if which == Which::Currency {
            let up: String = s
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .map(|c| c.to_ascii_uppercase())
                .collect();
            // What is selected is typed over, so it does not count.
            field.text.len() - field.selected().len() + up.len() <= 3 && field.insert(&up)
        } else {
            field.insert(s)
        };
        if changed {
            form.error = None;
        }
        changed
    }

    /// A key press. `shift` selects; `primary` is Cmd on a Mac and Ctrl
    /// elsewhere (Cmd+A selects all).
    fn key(&mut self, key: &str, shift: bool, primary: bool) -> bool {
        if self.form.is_none() {
            return false;
        }
        match key {
            "Escape" => {
                self.form = None;
                self.focus = None;
                true
            }
            "Enter" => {
                self.save();
                true
            }
            "Tab" => {
                let at = self
                    .focus
                    .and_then(|f| ORDER.iter().position(|o| *o == f))
                    .unwrap_or(ORDER.len() - 1);
                self.focus = ORDER.get((at + 1) % ORDER.len()).copied();
                true
            }
            _ => {
                let Some(which) = self.focus else {
                    return false;
                };
                let Some(form) = self.form.as_mut() else {
                    return false;
                };
                let (changed, handled) = form.field(which).key_with(key, shift, primary);
                if changed {
                    form.error = None;
                }
                handled
            }
        }
    }

    fn wheel(&mut self, x: f32, y: f32, dy: f32) -> bool {
        if self.form.is_none() && self.list_rect.contains(x, y) {
            let next = (self.list_scroll + dy).clamp(0.0, self.list_max);
            let moved = next != self.list_scroll;
            self.list_scroll = next;
            return moved;
        }
        false
    }

    fn name_window(&mut self) {
        if !self.titled && !self.team.org.is_empty() {
            self.titled = true;
            let _ = window::set_title(self.win, &format!("{APP_NAME} \u{b7} {}", self.team.org));
        }
    }
}

/// An amount as it reads in the form: "12.5" rather than "12.50".
fn trim_money(amount: f64, currency: &str) -> String {
    if decimals(currency) == 0 {
        return format!("{}", amount.round() as i64);
    }
    let cents = (amount * 100.0).round() as i64;
    if cents % 100 == 0 {
        format!("{}", cents / 100)
    } else {
        format!("{}.{:02}", cents / 100, cents % 100)
    }
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create(
            APP_NAME,
            types::WindowSize {
                width: 1040,
                height: 680,
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
            w: 1040.0,
            h: 680.0,
            team: Team::new(),
            clock: Clock::new(),
            all: Vec::new(),
            cursor: None,
            form: None,
            focus: None,
            list_scroll: 0.0,
            list_max: 0.0,
            list_rect: Rect::default(),
            hits: Vec::new(),
            undo: None,
            made: 0,
            titled: false,
            last_currency: String::new(),
        };

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
                        if (p.x - x0).abs() < 10.0 && (p.y - y0).abs() < 10.0 && app.open_access() {
                            dirty = app.tap(p.x, p.y);
                        }
                    }
                }
                Some(types::Event::Wheel(w)) => dirty = app.wheel(w.x, w.y, w.dy),
                Some(types::Event::TextInput(s)) => dirty = app.typed(&s),
                Some(types::Event::Key(k)) if k.pressed => {
                    // Cmd on a Mac, Ctrl elsewhere: the app cannot tell
                    // which system it is on, and needs not.
                    let m = k.modifiers;
                    dirty = app.key(&k.key, m.shift, m.meta || m.control);
                }
                _ => {}
            }
            let now = app.now();
            if app.open_access() {
                if now >= looked + LOOK_EVERY_MS {
                    looked = now;
                    dirty |= app.look_for_changes();
                    // Where this computer stands: local, never waits.
                    dirty |= app.team.look();
                    dirty |= app.undo.is_some();
                }
                // Not while a form is open: a sync can wait on the network.
                if now >= synced + SYNC_EVERY_MS && app.form.is_none() {
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
