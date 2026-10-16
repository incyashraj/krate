//! Stock List: what the team has, how many, and what is running low.
//!
//! One of the three starters an organisation releases in its first minute on
//! Krate (IC-958). It is built like Team Notes (read that one first: the
//! store, the sync, the states and the kit are the same), and shows the one
//! thing a shared count needs that a shared note does not:
//!
//! - **A count two people change at once is one field per person.** If the
//!   count were one field, Dana taking a box out while Sam puts one in would
//!   keep only whichever write came last, and a box would vanish or appear.
//!   So each person's ins and outs are their own field, `n:<who>`, and the
//!   count is the sum of them all. Two people pressing + at the same moment
//!   on two computers both count.
//! - **Setting the count after a stocktake is a change too**: it moves this
//!   person's own field by the difference, so a teammate's + made at the
//!   same time still counts on top.
//! - **One record per item** in the `items` collection: `name`, `low` (the
//!   count at which it is running low) and the `n:` fields.
//! - **A record with no name is left out**: it is what remains when one
//!   person deletes an item while another, offline, counts it.
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

const APP_NAME: &str = "Stock List";
const ITEMS: &str = "items";
/// What the team keeps here, for the kit's sentences ("Acme's stock list is full").
const THINGS: &str = "stock list";
const TOO_LONG: &str = "That name is too long to save. Shorten it.";

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
// Items and their counts
// ---------------------------------------------------------------------------

/// The field that holds one person's ins and outs.
fn count_field(me: &str) -> String {
    format!("n:{me}")
}

#[derive(Clone)]
struct Item {
    id: String,
    name: String,
    low: i64,
    /// The sum of everyone's `n:` fields.
    count: i64,
    /// This person's own `n:` field.
    mine: f64,
    changed_by: String,
    changed_at: u64,
}

impl Item {
    fn from_entry(entry: &Entry, me: &str) -> Option<Item> {
        let name = org::text(entry, "name")?.trim();
        if name.is_empty() {
            return None;
        }
        let mut count = 0.0;
        let mut mine = 0.0;
        let own = count_field(me);
        for (field, value) in &entry.fields {
            if let (true, Value::Number(n)) = (field.starts_with("n:"), value) {
                count += *n;
                if *field == own {
                    mine = *n;
                }
            }
        }
        let low = match org::field(entry, "low") {
            Some(Value::Number(n)) => n.round() as i64,
            _ => 0,
        };
        Some(Item {
            id: entry.id.clone(),
            name: String::from(name),
            low,
            count: count.round() as i64,
            mine,
            changed_by: entry.changed_by.clone(),
            changed_at: entry.changed_at,
        })
    }

    fn running_low(&self) -> bool {
        self.count <= self.low
    }
}

/// Every item by name, and the cursor.
fn read_all(me: &str) -> Result<(Vec<Item>, String), OrgError> {
    let (entries, cursor) = read_records(ITEMS)?;
    let mut items: Vec<Item> = entries
        .iter()
        .filter_map(|e| Item::from_entry(e, me))
        .collect();
    items.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok((items, cursor))
}

/// A whole number from what was typed, or None.
fn parse_count(text: &str) -> Option<i64> {
    let t = text.trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit()) || t.len() > 9 {
        return None;
    }
    t.parse().ok()
}

// ---------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Name,
    Count,
    Low,
}

const ORDER: [Which; 3] = [Which::Name, Which::Count, Which::Low];

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Add,
    Suggest(usize),
    Edit(usize),
    Plus(usize),
    Minus(usize),
    View(bool),
    Field(Which),
    Save,
    Cancel,
    Delete,
    Undo,
    Dismiss,
    Card,
    Backdrop,
}

struct Form {
    editing: Option<Item>,
    name: Field,
    count: Field,
    low: Field,
    error: Option<(Which, String)>,
    rects: Vec<(Which, Rect)>,
}

impl Form {
    fn field(&mut self, which: Which) -> &mut Field {
        match which {
            Which::Name => &mut self.name,
            Which::Count => &mut self.count,
            Which::Low => &mut self.low,
        }
    }

    fn get(&self, which: Which) -> &Field {
        match which {
            Which::Name => &self.name,
            Which::Count => &self.count,
            Which::Low => &self.low,
        }
    }
}

struct Undo {
    item: Item,
    until: u64,
}

const SUGGESTIONS: [(&str, &str); 3] = [
    ("Printer paper", "3"),
    ("Coffee beans", "2"),
    ("First aid kits", "1"),
];

struct App {
    win: u64,
    c: u64,
    screen: Screen,
    w: f32,
    h: f32,
    team: Team,
    clock: Clock,
    items: Vec<Item>,
    cursor: Option<String>,
    /// Showing only what is running low.
    low_only: bool,
    form: Option<Form>,
    focus: Option<Which>,
    list_scroll: f32,
    list_max: f32,
    list_rect: Rect,
    hits: Vec<(Rect, Hit)>,
    undo: Option<Undo>,
    made: u32,
    titled: bool,
}

impl App {
    fn now(&self) -> u64 {
        krate::time::now_millis()
    }

    fn open_access(&self) -> bool {
        self.team.access == Access::Open
    }

    fn reload(&mut self) {
        match read_all(&self.team.me) {
            Ok((items, cursor)) => {
                self.items = items;
                self.cursor = Some(cursor);
            }
            Err(error) => self.team.fail(error),
        }
    }

    fn look_for_changes(&mut self) -> bool {
        let Some(cursor) = self.cursor.clone() else {
            self.reload();
            return true;
        };
        match changed_since(ITEMS, &cursor) {
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

    fn shown(&self) -> Vec<Item> {
        self.items
            .iter()
            .filter(|i| !self.low_only || i.running_low())
            .cloned()
            .collect()
    }

    fn low_count(&self) -> usize {
        self.items.iter().filter(|i| i.running_low()).count()
    }

    /// Count one in or out: this person's own field moves by `delta`.
    fn adjust(&mut self, index: usize, delta: i64) {
        let Some(item) = self.shown().get(index).cloned() else {
            return;
        };
        if item.count + delta < 0 {
            return;
        }
        let field = count_field(&self.team.me);
        match org::put(
            ITEMS,
            &item.id,
            &[(field.as_str(), Value::Number(item.mine + delta as f64))],
        ) {
            Ok(()) => {
                self.team.saved_since = true;
                self.reload();
            }
            Err(error) => self.team.fail(error),
        }
    }

    fn open_form(&mut self, editing: Option<Item>, name: &str, count: &str) {
        let f = |text: &str| {
            let mut field = Field::new(false);
            field.set(text);
            field
        };
        let form = match &editing {
            Some(i) => Form {
                name: f(&i.name),
                count: f(&format!("{}", i.count)),
                low: f(&format!("{}", i.low)),
                editing: editing.clone(),
                error: None,
                rects: Vec::new(),
            },
            None => Form {
                name: f(name),
                count: f(count),
                low: f(""),
                editing: None,
                error: None,
                rects: Vec::new(),
            },
        };
        self.focus = Some(if name.is_empty() || editing.is_some() {
            Which::Name
        } else {
            Which::Low
        });
        self.form = Some(form);
    }

    fn save(&mut self) {
        let me = self.team.me.clone();
        self.made += 1;
        let id_new = new_id("i", &me, self.made);
        let Some(form) = self.form.as_mut() else {
            return;
        };
        let name = String::from(form.name.text.trim());
        let count = parse_count(&form.count.text);
        let low = if form.low.text.trim().is_empty() {
            Some(0)
        } else {
            parse_count(&form.low.text)
        };
        let problem = if name.is_empty() {
            Some((Which::Name, "Name the item, like \"Printer paper\"."))
        } else if count.is_none() {
            Some((
                Which::Count,
                "Say how many there are now, as a whole number.",
            ))
        } else if low.is_none() {
            Some((
                Which::Low,
                "Say at what count it is running low, as a whole number.",
            ))
        } else {
            None
        };
        if let Some((which, words)) = problem {
            form.error = Some((which, String::from(words)));
            self.focus = Some(which);
            return;
        }
        let (count, low) = (count.unwrap_or(0), low.unwrap_or(0));
        let mine_field = count_field(&me);
        let mut fields: Vec<(&str, Value)> = Vec::new();
        let id = match &form.editing {
            Some(old) => {
                if old.name != name {
                    fields.push(("name", Value::Text(name)));
                }
                if old.low != low {
                    fields.push(("low", Value::Number(low as f64)));
                }
                if old.count != count {
                    // A stocktake: move this person's own share by the
                    // difference, so a teammate's count made meanwhile
                    // still stands on top of it.
                    fields.push((
                        mine_field.as_str(),
                        Value::Number(old.mine + (count - old.count) as f64),
                    ));
                }
                old.id.clone()
            }
            None => {
                fields.push(("name", Value::Text(name)));
                fields.push(("low", Value::Number(low as f64)));
                fields.push((mine_field.as_str(), Value::Number(count as f64)));
                if !me.is_empty() {
                    fields.push(("made_by", Value::Text(me.clone())));
                }
                id_new
            }
        };
        if fields.is_empty() {
            self.form = None;
            self.focus = None;
            return;
        }
        match org::put(ITEMS, &id, &fields) {
            Ok(()) => {
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
                    form.error = Some((Which::Name, problem));
                }
            }
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
        match org::delete(ITEMS, &old.id) {
            Ok(()) => {
                self.team.saved_since = true;
                self.undo = Some(Undo {
                    item: old,
                    until: self.now() + UNDO_FOR_MS,
                });
                self.reload();
            }
            Err(error) => self.team.fail(error),
        }
    }

    /// Put a deleted item back with the count it had, as this person's
    /// share: the other shares went with the delete.
    fn undo_delete(&mut self) {
        let Some(undo) = self.undo.take() else {
            return;
        };
        let i = undo.item;
        let field = count_field(&self.team.me);
        let fields = [
            ("name", Value::Text(i.name.clone())),
            ("low", Value::Number(i.low as f64)),
            (field.as_str(), Value::Number(i.count as f64)),
        ];
        match org::put(ITEMS, &i.id, &fields) {
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
                self.draw_stock(safe, &k, &look, &mut said);
                if self.form.is_some() {
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
                String::from("Stock List is waiting for its team"),
                String::from(
                    "It keeps count of what your whole organisation has, encrypted so only \
                     your team can see it. Release it to your organisation on Krate, then open \
                     it from there. This copy is not an organisation's release, so there is no \
                     team to share with yet.",
                ),
            ),
            Access::Refused(reason, message) => {
                let title = match reason.as_str() {
                    "removed" => format!("You are no longer in {org}"),
                    "not-a-member" => format!("You are not in {org}"),
                    "not-in-audience" => format!("{lead} has not shared Stock List with you"),
                    "release-withdrawn" => String::from("This version of Stock List was withdrawn"),
                    "not-released" => String::from("This version of Stock List is not released yet"),
                    "not-signed-in" => String::from("Sign in to Krate to see your team's stock"),
                    "org-closed" => format!("{lead} is closed"),
                    _ => String::from("Stock List cannot open your team's stock list"),
                };
                // The organisation's own words say what to do next; a bare
                // reason code is not words, so say it ourselves then.
                let body = if message.trim().is_empty() || message.contains(reason.as_str()) {
                    match reason.as_str() {
                        "not-in-audience" => {
                            format!("Ask an admin of {org} to add you to the group it is shared with.")
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
                String::from("Stock List was not given your team's stock list"),
                String::from(
                    "It needs it to show anything. Open it again and allow it to keep the \
                     stock list with your team.",
                ),
            ),
            Access::Unsupported => (
                String::from("Open Stock List on your computer"),
                String::from(
                    "Your team's stock list is not available in a browser tab yet. Open Stock List \
                     with Krate on your computer and it is all there.",
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

    fn draw_stock(&mut self, area: Rect, k: &Ink, look: &Look, said: &mut Said) {
        let c = self.c;
        let compact = self.screen.compact();
        let now = self.now();
        let side = if compact { 16.0 } else { 24.0 };
        let tap = self.screen.tap().max(32.0);
        let empty = self.items.is_empty();

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
            let label = if compact { "Add" } else { "Add an item" };
            let bw = width_of(c, label, look.text(14.0), 600) + 28.0;
            let b = Rect::new(right - bw, bar.y + (bar.h - tap) / 2.0, bw, tap);
            self.button(b, label, Tone::Primary, Hit::Add, k, look, said);
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
        let count = match self.items.len() {
            0 => String::new(),
            1 => String::from("1 item"),
            n => format!("{n} items"),
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
        } else {
            // The list, never wider than reads well; centred on a wide window.
            let max_w = 860.0f32;
            let pad = if compact { 16.0 } else { 32.0 };
            let w = (rest.w - pad * 2.0).min(max_w);
            let x = rest.x + (rest.w - w) / 2.0;
            let mut col = Rect::new(x, rest.y, w, rest.h);
            col.take_top(16.0);
            let tabs = col.take_top(tap);
            self.draw_tabs(tabs, k, look, said);
            col.take_top(8.0);
            self.list_rect = Rect::new(rest.x, col.y, rest.w, col.h);
            self.draw_list(col, k, look, said, now);
        }
        self.draw_undo(area, k, look, said, now);
    }

    /// Everything, or only what is running low.
    fn draw_tabs(&mut self, r: Rect, k: &Ink, look: &Look, said: &mut Said) {
        let c = self.c;
        let s = look.text(14.0);
        let low = self.low_count();
        let labels = [
            (false, format!("Everything ({})", self.items.len())),
            (true, format!("Running low ({low})")),
        ];
        let mut x = r.x;
        let total_w: f32 = labels
            .iter()
            .map(|(_, l)| width_of(c, l, s, 500) + 28.0)
            .sum::<f32>()
            + 8.0;
        let track = Rect::new(r.x, r.y, total_w.min(r.w), r.h);
        round(c, track, 10.0, k.bg);
        x += 4.0;
        for (low_only, label) in &labels {
            let w = width_of(c, label, s, 500) + 28.0;
            let seg = Rect::new(x, r.y + 4.0, w, r.h - 8.0);
            let on = self.low_only == *low_only;
            if on {
                round(c, seg, 7.0, k.surface);
                outline(c, seg, 7.0, k.line);
            }
            let ink = if *low_only && low > 0 {
                k.warn
            } else if on {
                k.ink
            } else {
                k.muted
            };
            text(
                c,
                label,
                seg.x + 14.0,
                baseline(seg.y, seg.h, s),
                s,
                ink,
                if on { 600 } else { 500 },
            );
            said.toggle(seg, label, on);
            self.hits.push((seg, Hit::View(*low_only)));
            x += w;
        }
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
                "Your team's stock list is on its way",
                format!(
                    "Waiting for your team's key: someone in {org} opens Krate and it \
                     arrives. You can add an item now; it is shared then."
                ),
                "Add an item",
            )
        } else {
            (
                "Keep count of what the team has",
                format!(
                    "Add what {org} keeps in stock, how many there are, and when it counts as \
                     running low. Anyone in the team counts things in and out with + and -, \
                     and everyone sees the same numbers."
                ),
                "Add the first item",
            )
        };
        let lines = wrap(c, &body, bs, 400, max_w);
        let tap = self.screen.tap().max(36.0);
        let block = ts * 1.3 + 12.0 + lines.len() as f32 * bs * 1.55 + 24.0 + tap * 2.0 + 48.0 + ss;
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
            Hit::Add,
            k,
            look,
            said,
        );
        y += tap + 20.0;
        let small = look.text(13.0);
        let mut sx = x;
        text(
            c,
            "Or start with",
            sx,
            baseline(y, tap, small),
            small,
            k.muted,
            400,
        );
        sx += width_of(c, "Or start with", small, 400) + 10.0;
        for (i, (name, _)) in SUGGESTIONS.iter().enumerate() {
            let w = width_of(c, name, look.text(14.0), 500) + 24.0;
            if sx + w > x + max_w + 40.0 {
                break;
            }
            self.button(
                Rect::new(sx, y, w, tap),
                name,
                Tone::Plain,
                Hit::Suggest(i),
                k,
                look,
                said,
            );
            sx += w + 8.0;
        }
        y += tap + 28.0;
        let lock = "Encrypted end to end: not even Krate can see what your team keeps.";
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

    fn draw_list(&mut self, area: Rect, k: &Ink, look: &Look, said: &mut Said, now: u64) {
        let c = self.c;
        let items = self.shown();
        let s1 = look.text(16.0);
        let s2 = look.text(13.0);
        let big = look.text(22.0);
        let tap = self.screen.tap().max(36.0);
        let row_h = (s1 * 1.4 + s2 * 1.5 + 26.0).max(tap + 20.0);
        self.list_max = (items.len() as f32 * row_h + 24.0 - area.h).max(0.0);
        self.list_scroll = self.list_scroll.clamp(0.0, self.list_max);
        let _ = canvas2d::set_clip(c, area.x - 8.0, area.y, area.w + 16.0, area.h);
        let mut y = area.y - self.list_scroll;
        if items.is_empty() {
            let line = "Nothing is running low.";
            text(c, line, area.x, y + 30.0, look.text(15.0), k.muted, 400);
            said.text(Rect::new(area.x, y, area.w, 40.0), line);
        }
        for (i, item) in items.iter().enumerate() {
            if y + row_h >= area.y && y <= area.bottom() {
                let row = Rect::new(area.x, y, area.w, row_h);
                if i > 0 {
                    fill(c, Rect::new(row.x, row.y, row.w, 1.0), k.line);
                }
                // + and -, with the count between them, on the right.
                let bw = tap;
                let plus = Rect::new(row.right() - bw, y + (row_h - bw) / 2.0, bw, bw);
                let count_w = look.text(64.0);
                let minus = Rect::new(plus.x - count_w - bw, plus.y, bw, bw);
                let low = item.running_low();
                for (r, sign, hit, can) in [
                    (minus, "\u{2212}", Hit::Minus(i), item.count > 0),
                    (plus, "+", Hit::Plus(i), true),
                ] {
                    round(c, r, 10.0, k.surface);
                    outline(c, r, 10.0, k.line);
                    let gs = look.text(20.0);
                    let gw = width_of(c, sign, gs, 500);
                    text(
                        c,
                        sign,
                        r.x + (r.w - gw) / 2.0,
                        baseline(r.y, r.h, gs) - 1.0,
                        gs,
                        if can { k.ink } else { k.line },
                        500,
                    );
                    let what = if sign == "+" { "One more" } else { "One fewer" };
                    said.button(r, &format!("{what} {}", item.name));
                    if !can {
                        said.disabled();
                    }
                    self.hits.push((r, hit));
                }
                let n = format!("{}", item.count);
                let nw = width_of(c, &n, big, 650);
                text(
                    c,
                    &n,
                    minus.right() + (count_w - nw) / 2.0,
                    baseline(minus.y, minus.h, big),
                    big,
                    if low { k.warn } else { k.ink },
                    650,
                );
                // The name, and whether it is running low.
                let tw = minus.x - 20.0 - row.x;
                let ty = y + 13.0 + s1;
                text(
                    c,
                    &fit(c, &item.name, s1, 600, tw),
                    row.x,
                    ty,
                    s1,
                    k.ink,
                    600,
                );
                let detail = if low {
                    format!(
                        "Running low \u{b7} {} counted {}",
                        self.short_name(&item.changed_by),
                        self.clock.when(item.changed_at, now)
                    )
                } else {
                    format!(
                        "Low at {} \u{b7} {} counted {}",
                        item.low,
                        self.short_name(&item.changed_by),
                        self.clock.when(item.changed_at, now)
                    )
                };
                let dy = ty + s2 * 1.6;
                if low {
                    let _ = canvas2d::fill_circle(
                        c,
                        gfx::Point {
                            x: row.x + 4.0,
                            y: dy - s2 * 0.35,
                        },
                        3.5,
                        k.warn,
                    );
                    text(
                        c,
                        &fit(c, &detail, s2, 500, tw - 14.0),
                        row.x + 14.0,
                        dy,
                        s2,
                        k.warn,
                        500,
                    );
                } else {
                    text(
                        c,
                        &fit(c, &detail, s2, 400, tw),
                        row.x,
                        dy,
                        s2,
                        k.muted,
                        400,
                    );
                }
                let name_r = Rect::new(row.x, row.y, tw, row.h);
                said.button(
                    name_r,
                    &format!("{}, {} left, {}", item.name, item.count, detail),
                );
                self.hits.push((name_r, Hit::Edit(i)));
            }
            y += row_h;
        }
        let _ = canvas2d::clear_clip(c);
    }

    fn draw_form(&mut self, area: Rect, k: &Ink, look: &Look, said: &mut Said) {
        let c = self.c;
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
        let gap = 14.0;
        let editing = self.form.as_ref().is_some_and(|f| f.editing.is_some());
        let card_w = (area.w - 32.0).min(460.0);
        let card_h = (look.text(20.0) * 1.6
            + 12.0
            + (ls * 1.6 + tap + gap) * 2.0
            + ls * 1.5
            + 12.0
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
        self.hits.push((card, Hit::Card));
        let inner = card.pad(24.0);
        let mut y = inner.y;
        let ts = look.text(20.0);
        let title = if editing {
            "Change this item"
        } else {
            "Add an item"
        };
        text(c, title, inner.x, y + ts, ts, k.ink, 650);
        said.heading(Rect::new(inner.x, y, inner.w, ts * 1.4), title);
        y += ts * 1.6 + 12.0;
        let mut rects = Vec::new();
        let name_r = Rect::new(inner.x, y + ls * 1.6, inner.w, tap);
        self.label(inner.x, y, "What is it", ls, k);
        self.form_field(
            name_r,
            Which::Name,
            "Printer paper",
            fs,
            400,
            k,
            said,
            "What is it",
        );
        rects.push((Which::Name, name_r));
        y = name_r.bottom() + gap;
        let half = (inner.w - 12.0) / 2.0;
        let count_r = Rect::new(inner.x, y + ls * 1.6, half, tap);
        let low_r = Rect::new(inner.x + half + 12.0, count_r.y, half, tap);
        self.label(
            count_r.x,
            y,
            if editing {
                "How many there are now"
            } else {
                "How many there are"
            },
            ls,
            k,
        );
        self.label(low_r.x, y, "Running low at", ls, k);
        self.form_field(
            count_r,
            Which::Count,
            "0",
            look.text(17.0),
            600,
            k,
            said,
            "How many there are",
        );
        self.form_field(
            low_r,
            Which::Low,
            "0",
            look.text(17.0),
            600,
            k,
            said,
            "Running low at",
        );
        rects.push((Which::Count, count_r));
        rects.push((Which::Low, low_r));
        y = count_r.bottom() + 10.0;
        let hint = "It shows as running low at that count or below.";
        text(
            c,
            &fit(c, hint, ls, 400, inner.w),
            inner.x,
            y + ls,
            ls,
            k.muted,
            400,
        );
        said.text(Rect::new(inner.x, y, inner.w, ls * 1.5), hint);
        y += ls * 1.5 + 8.0;
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
        let by = card.bottom() - 24.0 - tap;
        let save = if editing { "Save changes" } else { "Add item" };
        let sw = width_of(c, save, look.text(14.0), 600) + 32.0;
        let save_r = Rect::new(inner.right() - sw, by, sw, tap);
        self.button(save_r, save, Tone::Primary, Hit::Save, k, look, said);
        let cw = width_of(c, "Cancel", look.text(14.0), 500) + 28.0;
        self.button(
            Rect::new(save_r.x - cw - 10.0, by, cw, tap),
            "Cancel",
            Tone::Plain,
            Hit::Cancel,
            k,
            look,
            said,
        );
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
        let line = format!("Deleted \"{}\"", undo.item.name);
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
            Hit::Add => self.open_form(None, "", ""),
            Hit::Suggest(i) => {
                if let Some((name, count)) = SUGGESTIONS.get(i) {
                    self.open_form(None, name, count);
                }
            }
            Hit::Edit(i) => {
                if let Some(item) = self.shown().get(i).cloned() {
                    self.open_form(Some(item), "", "");
                }
            }
            Hit::Plus(i) => self.adjust(i, 1),
            Hit::Minus(i) => self.adjust(i, -1),
            Hit::View(low_only) => {
                self.low_only = low_only;
                self.list_scroll = 0.0;
            }
            Hit::Field(which) => {
                self.focus = Some(which);
                let c = self.c;
                let (size, weight) = if which == Which::Name {
                    (self.screen.look.text(15.0), 400)
                } else {
                    (self.screen.look.text(17.0), 600)
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
            Hit::Save => self.save(),
            Hit::Cancel | Hit::Backdrop => {
                self.form = None;
                self.focus = None;
            }
            Hit::Delete => self.delete(),
            Hit::Undo => self.undo_delete(),
            Hit::Dismiss => self.team.problem = None,
            Hit::Card => {}
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
        let changed = if which == Which::Name {
            field.insert(s)
        } else {
            let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
            // What is selected is typed over, so it does not count.
            field.text.len() - field.selected().len() + digits.len() <= 9 && field.insert(&digits)
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

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create(
            APP_NAME,
            types::WindowSize {
                width: 900,
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
            w: 900.0,
            h: 680.0,
            team: Team::new(),
            clock: Clock::new(),
            items: Vec::new(),
            cursor: None,
            low_only: false,
            form: None,
            focus: None,
            list_scroll: 0.0,
            list_max: 0.0,
            list_rect: Rect::default(),
            hits: Vec::new(),
            undo: None,
            made: 0,
            titled: false,
        };

        // Who this is decides which count is theirs, so ask before reading.
        app.team.learn_who();
        app.reload();
        app.draw();
        if app.open_access() {
            let had_name = !app.team.me.is_empty();
            if app.team.sync() || !had_name {
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
                if now >= synced + SYNC_EVERY_MS && app.form.is_none() {
                    synced = now;
                    let had_name = !app.team.me.is_empty();
                    if app.team.sync() || (!had_name && !app.team.me.is_empty()) {
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
