//! Team Notes: notes a whole team shares, with nothing to run but the app.
//!
//! One of the three starters an organisation releases in its first minute on
//! Krate (IC-958), and the reference for an organisation app. Copy these
//! habits; each one is here because the plain way gets it wrong:
//!
//! - **The team's data is `store.org`.** One record per note in the `notes`
//!   collection, one field per thing a person edits (`title`, `body`,
//!   `pinned`). Two people editing different notes, or the title and the
//!   body of one note, both keep their change. No server, no sign-in code,
//!   no encryption of our own: the organisation's store is end to end
//!   encrypted and the runtime decides who may read it.
//! - **Read locally, never wait.** `org::records` answers from this
//!   computer. The app keeps the cursor its last read gave and asks every
//!   second only for what changed after it; a teammate's edit shows within a
//!   second of the runtime's background sync bringing it down.
//! - **`org::status` says where this computer stands and never waits**:
//!   synced, syncing, offline, or waiting for the team's key, asked every
//!   second beside the records. `org::sync` only asks the runtime to go now
//!   (in a window it never waits), once after the first frame and then
//!   every twenty seconds. Writes are never followed by a sync: the runtime
//!   sends them in the background within seconds, and once more when the
//!   window closes.
//! - **Who changed it is the entry's own `changed_by`** and when is
//!   `changed_at`; nothing needs storing for "last changed by dana". Who
//!   *made* a note is a field (`made_by`), because `changed_by` moves on.
//! - **Every state the store can report is a screen with words**: not an
//!   organisation's app, removed, not shared with you, a version withdrawn,
//!   a computer still waiting for the team's key, offline. None of them is an error box.
//! - **Nothing typed is lost.** Edits are saved half a second after typing
//!   stops, when another note is opened, and when the window closes; a
//!   deleted note can be brought back with Undo.
//!
//! The interface is drawn on one canvas from `krate::layout`: the screen in
//! front of the app (`Screen::of`), the person's look (dark, contrast, text
//! size), and what a screen reader hears (`Said`). A phone gets one column.
//!
//! `#![no_std]`, so it imports only `krate:*`.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use krate::gfx::{canvas2d, types as gfx};
use krate::layout::{selected_on_line, Field, Look, Rect, Said, Screen};
use krate::math::F32Ext as _;
use krate::org::{self, Entry, OrgError, Value};
use krate::ui::{events, tree, types, window};

const APP_NAME: &str = "Team Notes";
const NOTES: &str = "notes";
/// What the team keeps here, for the kit's sentences ("Acme's notes are full").
const THINGS: &str = "notes";
const TOO_LONG: &str = "That note is too long to save. Split it into two notes.";

/// How long typing must pause before the note is saved.
const SAVE_AFTER_MS: u64 = 500;
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
// Notes
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Note {
    id: String,
    title: String,
    body: String,
    pinned: bool,
    made_by: String,
    changed_by: String,
    changed_at: u64,
}

impl Note {
    fn from_entry(entry: &Entry) -> Note {
        Note {
            id: entry.id.clone(),
            title: String::from(org::text(entry, "title").unwrap_or("")),
            body: String::from(org::text(entry, "body").unwrap_or("")),
            pinned: matches!(org::field(entry, "pinned"), Some(Value::Boolean(true))),
            made_by: String::from(org::text(entry, "made_by").unwrap_or("")),
            changed_by: entry.changed_by.clone(),
            changed_at: entry.changed_at,
        }
    }

    fn shown_title(&self) -> &str {
        let t = self.title.trim();
        if t.is_empty() {
            first_line(&self.body).unwrap_or("Untitled")
        } else {
            t
        }
    }

    fn snippet(&self) -> &str {
        if self.title.trim().is_empty() {
            // The first line is already the title.
            self.body
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .nth(1)
                .unwrap_or("")
        } else {
            first_line(&self.body).unwrap_or("")
        }
    }

    fn matches(&self, wanted: &str) -> bool {
        wanted.is_empty()
            || contains_folded(&self.title, wanted)
            || contains_folded(&self.body, wanted)
    }
}

fn first_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|l| !l.is_empty())
}

fn contains_folded(hay: &str, needle: &str) -> bool {
    let hay = hay.to_lowercase();
    hay.contains(&needle.to_lowercase())
}

/// Every note, and the cursor to ask "what changed since?" with.
fn read_all() -> Result<(Vec<Note>, String), OrgError> {
    let (entries, cursor) = read_records(NOTES)?;
    let mut notes: Vec<Note> = entries.iter().map(Note::from_entry).collect();
    notes.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then(b.changed_at.cmp(&a.changed_at))
    });
    Ok((notes, cursor))
}

// ---------------------------------------------------------------------------
// The app
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    NewNote,
    Suggest(usize),
    Open(usize),
    Search,
    ClearSearch,
    Title,
    Body,
    Pin,
    Delete,
    Back,
    Undo,
    Dismiss,
}

#[derive(Clone, Copy, PartialEq)]
enum Focus {
    None,
    Search,
    Title,
    Body,
}

/// The note being edited, and what was last saved of it.
struct Open {
    id: String,
    title: Field,
    body: Field,
    saved_title: String,
    saved_body: String,
    /// It has been written to the store at least once.
    exists: bool,
    /// When it was last typed in, for saving after a pause.
    edited_at: Option<u64>,
}

impl Open {
    fn dirty(&self) -> bool {
        self.title.text != self.saved_title || self.body.text != self.saved_body
    }
}

/// A deleted note, kept so Undo can put it back.
struct Undo {
    note: Note,
    until: u64,
}

const SUGGESTIONS: [(&str, &str); 2] = [
    (
        "Meeting notes",
        "Date:\nWho was there:\n\nWhat we decided:\n- \n\nWho does what next:\n- ",
    ),
    (
        "How we do things",
        "Opening up:\n- \n\nClosing:\n- \n\nWho to call when something breaks:\n- ",
    ),
];

struct App {
    win: u64,
    c: u64,
    screen: Screen,
    w: f32,
    h: f32,
    team: Team,
    clock: Clock,
    notes: Vec<Note>,
    cursor: Option<String>,
    open: Option<Open>,
    focus: Focus,
    search: Field,
    /// On a phone: the editor fills the screen instead of the list.
    phone_editor: bool,
    list_scroll: f32,
    list_max: f32,
    list_rect: Rect,
    body_scroll: f32,
    body_max: f32,
    body_rect: Rect,
    body_lines: Vec<(usize, usize)>,
    body_size: f32,
    body_lh: f32,
    title_rect: Rect,
    title_size: f32,
    search_rect: Rect,
    search_size: f32,
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

    // ----- reading -----

    fn reload(&mut self) {
        match read_all() {
            Ok((notes, cursor)) => {
                self.notes = notes;
                self.cursor = Some(cursor);
                self.follow_open_note();
            }
            Err(error) => self.team.fail(error),
        }
    }

    /// The open note changed elsewhere: take the change unless the person is
    /// mid-edit, in which case theirs is newer and wins when it is saved.
    fn follow_open_note(&mut self) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if !open.exists {
            return;
        }
        match self.notes.iter().find(|n| n.id == open.id) {
            Some(note) => {
                if open.title.text == open.saved_title && note.title != open.saved_title {
                    open.title.replace(&note.title);
                    open.saved_title = note.title.clone();
                }
                if open.body.text == open.saved_body && note.body != open.saved_body {
                    open.body.replace(&note.body);
                    open.saved_body = note.body.clone();
                }
            }
            None if !open.dirty() => {
                // Deleted by someone else while it was open.
                self.open = None;
                self.focus = Focus::None;
                self.phone_editor = false;
                self.team.problem = Some(String::from("The note you had open was deleted."));
            }
            None => {
                // Deleted elsewhere, but the person has typed since: saving
                // brings it back with their words.
                open.exists = false;
            }
        }
    }

    /// Something changed since the cursor? Reload. Local and cheap.
    fn look_for_changes(&mut self) -> bool {
        let Some(cursor) = self.cursor.clone() else {
            self.reload();
            return true;
        };
        match changed_since(NOTES, &cursor) {
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

    // ----- writing -----

    /// Save the open note's changed fields. Only what changed is written, so
    /// a teammate editing the other field keeps theirs.
    fn save(&mut self) {
        let me = self.team.me.clone();
        let Some(open) = self.open.as_mut() else {
            return;
        };
        open.edited_at = None;
        if !open.dirty() {
            return;
        }
        if !open.exists && open.title.text.trim().is_empty() && open.body.text.trim().is_empty() {
            return;
        }
        let mut fields: Vec<(&str, Value)> = Vec::new();
        if open.title.text != open.saved_title || !open.exists {
            fields.push(("title", Value::Text(open.title.text.clone())));
        }
        if open.body.text != open.saved_body || !open.exists {
            fields.push(("body", Value::Text(open.body.text.clone())));
        }
        if !open.exists && !me.is_empty() {
            fields.push(("made_by", Value::Text(me)));
        }
        match org::put(NOTES, &open.id, &fields) {
            Ok(()) => {
                open.exists = true;
                open.saved_title = open.title.text.clone();
                open.saved_body = open.body.text.clone();
                self.team.saved_since = true;
                self.team.problem = None;
                self.reload();
            }
            Err(error) => self.team.fail(error),
        }
    }

    fn new_note(&mut self, title: &str, body: &str) {
        self.save();
        self.made += 1;
        let mut t = Field::new(false);
        t.set(title);
        let mut b = Field::new(true);
        b.set(body);
        self.open = Some(Open {
            id: new_id("n", &self.team.me, self.made),
            title: t,
            body: b,
            saved_title: String::new(),
            saved_body: String::new(),
            exists: false,
            edited_at: None,
        });
        self.focus = if title.is_empty() {
            Focus::Title
        } else {
            Focus::Body
        };
        if !body.is_empty() {
            // Straight after "Date:", where the first thing goes.
            if let Some(o) = self.open.as_mut() {
                o.body.move_to(o.body.text.find('\n').unwrap_or(o.body.text.len()), false);
            }
        }
        self.body_scroll = 0.0;
        self.phone_editor = true;
        if !title.is_empty() {
            self.save();
        }
    }

    fn open_note(&mut self, index: usize) {
        self.save();
        let Some(note) = self.visible().get(index).map(|n| (*n).clone()) else {
            return;
        };
        let mut t = Field::new(false);
        t.set(&note.title);
        let mut b = Field::new(true);
        b.set(&note.body);
        self.open = Some(Open {
            id: note.id,
            title: t,
            body: b,
            saved_title: note.title,
            saved_body: note.body,
            exists: true,
            edited_at: None,
        });
        self.focus = Focus::None;
        self.body_scroll = 0.0;
        self.phone_editor = true;
    }

    fn toggle_pin(&mut self) {
        self.save();
        let Some(open) = self.open.as_ref() else {
            return;
        };
        if !open.exists {
            return;
        }
        let pinned = self.notes.iter().any(|n| n.id == open.id && n.pinned);
        match org::put(NOTES, &open.id, &[("pinned", Value::Boolean(!pinned))]) {
            Ok(()) => {
                self.team.saved_since = true;
                self.reload();
            }
            Err(error) => self.team.fail(error),
        }
    }

    fn delete(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        self.focus = Focus::None;
        self.phone_editor = false;
        if !open.exists {
            return;
        }
        let kept = self
            .notes
            .iter()
            .find(|n| n.id == open.id)
            .cloned()
            .map(|mut n| {
                n.title = open.title.text.clone();
                n.body = open.body.text.clone();
                n
            });
        match org::delete(NOTES, &open.id) {
            Ok(()) => {
                self.team.saved_since = true;
                if let Some(note) = kept {
                    self.undo = Some(Undo {
                        note,
                        until: self.now() + UNDO_FOR_MS,
                    });
                }
                self.reload();
            }
            Err(error) => self.team.fail(error),
        }
    }

    /// Put a deleted note back, every field as it was.
    fn undo_delete(&mut self) {
        let Some(undo) = self.undo.take() else {
            return;
        };
        let n = undo.note;
        let mut fields = Vec::new();
        fields.push(("title", Value::Text(n.title.clone())));
        fields.push(("body", Value::Text(n.body.clone())));
        if n.pinned {
            fields.push(("pinned", Value::Boolean(true)));
        }
        if !n.made_by.is_empty() {
            fields.push(("made_by", Value::Text(n.made_by.clone())));
        }
        match org::put(NOTES, &n.id, &fields) {
            Ok(()) => {
                self.team.saved_since = true;
                self.reload();
                if let Some(i) = self.visible().iter().position(|v| v.id == n.id) {
                    self.open_note(i);
                }
            }
            Err(error) => self.team.fail(error),
        }
    }

    fn visible(&self) -> Vec<&Note> {
        let wanted = self.search.text.trim();
        self.notes.iter().filter(|n| n.matches(wanted)).collect()
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
            Access::Open => self.draw_notes(safe, &k, &look, &mut said),
            other => self.draw_closed(safe, &k, &look, &mut said, &other),
        }
        let _ = canvas2d::present(c);
        said.send(self.win);
    }

    /// The whole window, when the team's notes cannot be shown at all.
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
                String::from("Team Notes is waiting for its team"),
                String::from(
                    "It keeps notes your whole organisation shares, encrypted so only your \
                     team can read them. Release it to your organisation on Krate, then open \
                     it from there. This copy is not an organisation's release, so there is \
                     no team to share with yet.",
                ),
            ),
            Access::Refused(reason, message) => {
                let title = match reason.as_str() {
                    "removed" => format!("You are no longer in {org}"),
                    "not-a-member" => format!("You are not in {org}"),
                    "not-in-audience" => format!("{lead} has not shared Team Notes with you"),
                    "release-withdrawn" => String::from("This version of Team Notes was withdrawn"),
                    "not-released" => {
                        String::from("This version of Team Notes is not released yet")
                    }
                    "not-signed-in" => String::from("Sign in to Krate to see your team's notes"),
                    "org-closed" => format!("{lead} is closed"),
                    _ => String::from("Team Notes cannot open your team's notes"),
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
                String::from("Team Notes was not given your team's notes"),
                String::from(
                    "It needs them to show anything. Open it again and allow \"keep the notes \
                     with your team\".",
                ),
            ),
            Access::Unsupported => (
                String::from("Open Team Notes on your computer"),
                String::from(
                    "Your team's notes are not available in a browser tab yet. Open Team Notes \
                     with Krate on your computer and they are all there.",
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
        let body_top = y;
        for (s, e) in &body_lines {
            text(c, &body[*s..*e], x, y + bs, bs, k.muted, 400);
            y += bs * 1.55;
        }
        said.text(Rect::new(x, body_top, max_w, y - body_top), &body);
    }

    fn draw_notes(&mut self, area: Rect, k: &Ink, look: &Look, said: &mut Said) {
        let c = self.c;
        let compact = self.screen.compact();
        let now = self.now();
        let side = if compact { 16.0 } else { 24.0 };
        let tap = self.screen.tap().max(32.0);

        // ---- the bar: name, count, status, the one primary action ----
        let mut rest = area;
        let bar = rest.take_top(tap + 24.0);
        fill(
            self.c,
            Rect::new(bar.x, bar.bottom() - 1.0, bar.w, 1.0),
            k.line,
        );
        let editing_on_phone = compact && self.phone_editor && self.open.is_some();
        let hs = look.text(17.0);
        let ms = look.text(13.0);
        let mut x = bar.x + side;
        if editing_on_phone {
            let b = Rect::new(x - 8.0, bar.y + (bar.h - tap) / 2.0, 72.0, tap);
            self.button(b, "Back", Tone::Quiet, Hit::Back, k, look, said);
            x = b.right() + 4.0;
        } else {
            text(c, APP_NAME, x, baseline(bar.y, bar.h, hs), hs, k.ink, 650);
            said.heading(Rect::new(x, bar.y, 120.0, bar.h), APP_NAME);
            x += width_of(c, APP_NAME, hs, 650) + 10.0;
        }
        let count = match self.notes.len() {
            0 => String::new(),
            1 => String::from("1 note"),
            n => format!("{n} notes"),
        };
        let mut right = bar.right() - side;
        let empty = self.notes.is_empty() && self.open.is_none();
        if !editing_on_phone && !empty {
            let label = "New note";
            let bw = width_of(c, label, look.text(14.0), 600) + 28.0;
            let b = Rect::new(right - bw, bar.y + (bar.h - tap) / 2.0, bw, tap);
            self.button(b, label, Tone::Primary, Hit::NewNote, k, look, said);
            right = b.x - 16.0;
        } else if let Some((exists, id)) = self.open.as_ref().map(|o| (o.exists, o.id.clone())) {
            if exists {
                let pinned = self.notes.iter().any(|n| n.id == id && n.pinned);
                let dw = width_of(c, "Delete", look.text(14.0), 500) + 24.0;
                let d = Rect::new(right - dw, bar.y + (bar.h - tap) / 2.0, dw, tap);
                self.button(d, "Delete", Tone::Danger, Hit::Delete, k, look, said);
                let label = if pinned { "Unpin" } else { "Pin" };
                let pw = width_of(c, label, look.text(14.0), 500) + 24.0;
                let p = Rect::new(d.x - pw - 8.0, d.y, pw, tap);
                self.button(p, label, Tone::Plain, Hit::Pin, k, look, said);
                right = p.x - 12.0;
            }
        }
        // Status: a dot and a few words; the long story goes in a line below.
        let (status, dot) = self.team.status(now, k, &self.clock);
        let sw = width_of(c, &status, ms, 400);
        let room = right - x - 16.0;
        if room > 40.0 {
            let shown = fit(c, &status, ms, 400, room - 14.0);
            let shown_w = width_of(c, &shown, ms, 400).min(sw);
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
        if !editing_on_phone && !count.is_empty() && right - x > 60.0 {
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
        // The empty screen says it itself, in its own words.
        let explain = if empty && self.team.problem.is_none() {
            None
        } else {
            self.team.explain()
        };
        if let Some(line) = explain {
            let s = look.text(13.0);
            let inner = rest.w - side * 2.0 - 90.0;
            let lines = wrap(c, &line, s, 400, inner.max(120.0));
            let h = lines.len() as f32 * s * 1.5 + 20.0;
            let band = rest.take_top(h);
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

        // ---- nothing yet: teach, with a real first action ----
        if self.notes.is_empty() && self.open.is_none() {
            self.draw_empty(rest, k, look, said);
            self.draw_undo(area, k, look, said, now);
            return;
        }

        if !compact && self.open.is_none() && !self.visible().is_empty() {
            self.open_note(0);
        }
        if compact {
            if editing_on_phone {
                self.draw_editor(rest, k, look, said, now, side, false);
            } else {
                self.draw_list(rest, k, look, said, now, side);
            }
        } else {
            let list_w = (rest.w * 0.38).clamp(260.0, 360.0);
            let mut right_part = rest;
            let list = right_part.take_left(list_w);
            fill(c, list, k.bg);
            fill(
                c,
                Rect::new(list.right() - 1.0, list.y, 1.0, list.h),
                k.line,
            );
            self.draw_list(list, k, look, said, now, 14.0);
            self.draw_editor(
                right_part,
                k,
                look,
                said,
                now,
                if rest.w > 1000.0 { 48.0 } else { 32.0 },
                true,
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
        let (title, body) = if self.team.waiting_to_be_let_in() {
            (
                "Your team's notes are on their way",
                format!(
                    "Waiting for your team's key: someone in {org} opens Krate and it \
                     arrives. You can write a note now; it is shared then."
                ),
            )
        } else {
            (
                "Start your team's first note",
                format!(
                    "Notes here are shared with everyone in {org}: what a meeting decided, how \
                     to open up, the door code. Anyone in the team can edit them, and each one \
                     shows who changed it last."
                ),
            )
        };
        let lines = wrap(c, &body, bs, 400, max_w);
        let tap = self.screen.tap().max(36.0);
        let block =
            ts * 1.3 + 12.0 + lines.len() as f32 * bs * 1.55 + 24.0 + tap + 20.0 + tap + 28.0 + ss;
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
        let label = if self.team.waiting_to_be_let_in() {
            "Write a note"
        } else {
            "Write the first note"
        };
        let bw = width_of(c, label, look.text(14.0), 600) + 32.0;
        self.button(
            Rect::new(x, y, bw, tap),
            label,
            Tone::Primary,
            Hit::NewNote,
            k,
            look,
            said,
        );
        y += tap + 20.0;
        // Two real notes to start from, one click each.
        let mut sx = x;
        let small = look.text(13.0);
        text(
            c,
            "Or start from",
            sx,
            baseline(y, tap, small),
            small,
            k.muted,
            400,
        );
        sx += width_of(c, "Or start from", small, 400) + 10.0;
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
        let lock = "Encrypted end to end: not even Krate can read your team's notes.";
        let shown = fit(c, lock, ss, 400, max_w);
        text(c, &shown, x, y + ss, ss, k.muted, 400);
        said.text(Rect::new(x, y, max_w, ss * 1.5), lock);
    }

    fn draw_list(&mut self, area: Rect, k: &Ink, look: &Look, said: &mut Said, now: u64, pad: f32) {
        let c = self.c;
        let mut rest = area;
        // Search, at the top of the list.
        let ss = look.text(14.0);
        let field_h = self.screen.tap().max(36.0);
        let top = rest.take_top(field_h + 24.0);
        let field = Rect::new(top.x + pad, top.y + 12.0, top.w - pad * 2.0, field_h);
        round(c, field, 8.0, k.surface);
        outline(
            c,
            field,
            8.0,
            if self.focus == Focus::Search {
                k.ink
            } else {
                k.line
            },
        );
        self.search_rect = field;
        self.search_size = ss;
        let clear_w = if self.search.text.is_empty() {
            0.0
        } else {
            56.0
        };
        let inner = Rect::new(field.x + 12.0, field.y, field.w - 24.0 - clear_w, field.h);
        self.draw_line_field(inner, Focus::Search, "Search notes", ss, 400, k.ink, k);
        self.hits.push((field, Hit::Search));
        said.field(field, "Search notes", &self.search.text);
        if clear_w > 0.0 {
            let b = Rect::new(
                field.right() - clear_w - 4.0,
                field.y + 4.0,
                clear_w,
                field.h - 8.0,
            );
            self.button(b, "Clear", Tone::Quiet, Hit::ClearSearch, k, look, said);
        }

        let notes: Vec<Note> = self.visible().into_iter().cloned().collect();
        self.list_rect = rest;
        let row_h = look.text(13.0) * 1.5 + look.text(15.0) * 1.4 + look.text(12.0) * 1.5 + 22.0;
        let label_h = look.text(11.0) + 22.0;
        let pinned = notes.iter().filter(|n| n.pinned).count();
        let mut content = notes.len() as f32 * row_h;
        if pinned > 0 {
            content += label_h * if pinned < notes.len() { 2.0 } else { 1.0 };
        }
        self.list_max = (content + 12.0 - rest.h).max(0.0);
        self.list_scroll = self.list_scroll.clamp(0.0, self.list_max);
        let _ = canvas2d::set_clip(c, rest.x, rest.y, rest.w, rest.h);
        let mut y = rest.y - self.list_scroll;
        if notes.is_empty() {
            let s = look.text(14.0);
            let line = format!("No notes match \"{}\".", self.search.text.trim());
            text(
                c,
                &fit(c, &line, s, 400, rest.w - pad * 2.0),
                rest.x + pad,
                y + 28.0,
                s,
                k.muted,
                400,
            );
            said.text(Rect::new(rest.x, y, rest.w, 40.0), &line);
        }
        let open_id = self.open.as_ref().map(|o| o.id.clone());
        for (i, note) in notes.iter().enumerate() {
            if pinned > 0 && (i == 0 || i == pinned) && !(i == pinned && pinned == notes.len()) {
                let label = if i == 0 { "PINNED" } else { "NOTES" };
                let ls = look.text(11.0);
                let _ = canvas2d::draw_text_styled(
                    c,
                    label,
                    gfx::Point {
                        x: rest.x + pad + 8.0,
                        y: y + 16.0 + ls,
                    },
                    ls,
                    k.muted,
                    gfx::TextStyle {
                        letter_spacing: 0.8,
                        ..style(600)
                    },
                );
                said.heading(
                    Rect::new(rest.x + pad, y, rest.w - pad * 2.0, label_h),
                    label,
                );
                y += label_h;
            }
            let row = Rect::new(rest.x + pad - 6.0, y, rest.w - pad * 2.0 + 12.0, row_h);
            if y + row_h >= rest.y && y <= rest.bottom() {
                let selected = open_id.as_deref() == Some(note.id.as_str());
                if selected {
                    round(c, row, 8.0, k.tint);
                }
                let tx = row.x + 14.0;
                let tw = row.w - 28.0;
                let s1 = look.text(15.0);
                let s2 = look.text(13.0);
                let s3 = look.text(12.0);
                let mut ty = y + 11.0 + s1;
                text(
                    c,
                    &fit(c, note.shown_title(), s1, 600, tw),
                    tx,
                    ty,
                    s1,
                    k.ink,
                    600,
                );
                ty += s2 * 1.5;
                let snippet = note.snippet();
                if snippet.is_empty() {
                    text(c, "No more text", tx, ty, s2, k.muted, 400);
                } else {
                    text(c, &fit(c, snippet, s2, 400, tw), tx, ty, s2, k.muted, 400);
                }
                ty += s3 * 1.6;
                let meta = format!(
                    "{} \u{b7} {}",
                    self.short_name(&note.changed_by),
                    self.clock.when(note.changed_at, now)
                );
                text(c, &fit(c, &meta, s3, 400, tw), tx, ty, s3, k.muted, 400);
                if !selected {
                    fill(c, Rect::new(tx, row.bottom() - 1.0, tw, 1.0), k.line);
                }
                said.button(
                    row,
                    &format!(
                        "{}, changed by {} {}",
                        note.shown_title(),
                        note.changed_by,
                        self.clock.when(note.changed_at, now)
                    ),
                );
                let visible = Rect::new(
                    row.x,
                    row.y.max(rest.y),
                    row.w,
                    row.bottom().min(rest.bottom()) - row.y.max(rest.y),
                );
                self.hits.push((visible, Hit::Open(i)));
            }
            y += row_h;
        }
        let _ = canvas2d::clear_clip(c);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_editor(
        &mut self,
        area: Rect,
        k: &Ink,
        look: &Look,
        said: &mut Said,
        now: u64,
        pad: f32,
        toolbar: bool,
    ) {
        let c = self.c;
        let Some(open) = self.open.as_ref() else {
            let s = look.text(15.0);
            let line = "Pick a note, or start a new one.";
            let w = width_of(c, line, s, 400);
            text(
                c,
                line,
                area.x + (area.w - w) / 2.0,
                area.y + area.h * 0.42,
                s,
                k.muted,
                400,
            );
            said.text(
                Rect::new(area.x, area.y + area.h * 0.38, area.w, 30.0),
                line,
            );
            self.body_rect = Rect::default();
            self.title_rect = Rect::default();
            return;
        };
        let exists = open.exists;
        let id = open.id.clone();
        let mut rest = area.inset(pad * 0.75, pad, pad * 0.5, pad);
        let tap = self.screen.tap().max(30.0);
        let ms = look.text(13.0);

        // Who changed it, and the two things to do to it.
        let note = self.notes.iter().find(|n| n.id == id).cloned();
        let meta = match &note {
            Some(n) => format!(
                "Changed by {} {}",
                self.short_name(&n.changed_by),
                self.clock.when(n.changed_at, now)
            ),
            None if exists => String::from("Saved"),
            None => String::from("New note, saved as you type"),
        };
        let head = rest.take_top(tap);
        let mut meta_right = head.right();
        if toolbar && exists {
            let pinned = note.as_ref().is_some_and(|n| n.pinned);
            let dw = width_of(c, "Delete", look.text(14.0), 500) + 24.0;
            let d = Rect::new(head.right() - dw, head.y, dw, tap);
            self.button(d, "Delete", Tone::Danger, Hit::Delete, k, look, said);
            let label = if pinned { "Unpin" } else { "Pin" };
            let pw = width_of(c, label, look.text(14.0), 500) + 24.0;
            let p = Rect::new(d.x - pw - 8.0, head.y, pw, tap);
            self.button(p, label, Tone::Plain, Hit::Pin, k, look, said);
            meta_right = p.x - 12.0;
        }
        let shown = fit(c, &meta, ms, 400, meta_right - head.x);
        text(
            c,
            &shown,
            head.x,
            baseline(head.y, head.h, ms),
            ms,
            k.muted,
            400,
        );
        said.text(
            Rect::new(head.x, head.y, meta_right - head.x, head.h),
            &meta,
        );
        rest.take_top(10.0);

        // The title.
        let ts = look.text(if self.screen.compact() { 22.0 } else { 26.0 });
        let title = rest.take_top(ts * 1.6);
        self.title_rect = title;
        self.title_size = ts;
        self.draw_line_field(title, Focus::Title, "Untitled", ts, 650, k.ink, k);
        self.hits.push((title, Hit::Title));
        said.field(
            title,
            "Title",
            self.open
                .as_ref()
                .map(|o| o.title.text.as_str())
                .unwrap_or(""),
        );
        rest.take_top(8.0);

        // The body, wrapped, scrolled, with its caret.
        let bs = look.text(15.0);
        let lh = (bs * 1.6).round();
        let body = rest;
        self.body_rect = body;
        self.body_size = bs;
        self.body_lh = lh;
        self.hits.push((body, Hit::Body));
        let (body_text, caret, selection) = match self.open.as_ref() {
            Some(o) => (o.body.text.clone(), o.body.caret, o.body.selection()),
            None => (String::new(), 0, None),
        };
        let selection = selection.filter(|_| self.focus == Focus::Body);
        self.body_lines = wrap(c, &body_text, bs, 400, body.w - 4.0);
        let content_h = self.body_lines.len() as f32 * lh;
        self.body_max = (content_h - body.h + lh).max(0.0);
        // Keep the caret in view while typing in a long note.
        if self.focus == Focus::Body {
            let cy = line_of(&self.body_lines, caret) as f32 * lh;
            if cy - self.body_scroll > body.h - lh * 1.5 {
                self.body_scroll = cy - body.h + lh * 1.5;
            }
            if cy < self.body_scroll {
                self.body_scroll = cy;
            }
        }
        self.body_scroll = self.body_scroll.clamp(0.0, self.body_max);
        let _ = canvas2d::set_clip(c, body.x - 2.0, body.y, body.w + 4.0, body.h);
        if body_text.is_empty() {
            text(
                c,
                "Write something your team should know",
                body.x,
                body.y + bs,
                bs,
                k.muted,
                400,
            );
        }
        let mut y = body.y - self.body_scroll;
        for (a, b) in &self.body_lines {
            if y + lh >= body.y && y <= body.bottom() {
                if let Some((s, e)) = selected_on_line((*a, *b), selection) {
                    let x0 = width_of(c, &body_text[*a..s], bs, 400);
                    let x1 = width_of(c, &body_text[*a..e], bs, 400);
                    // A selected line break shows as a sliver, as text
                    // fields do.
                    let w = if selection.is_some_and(|(_, end)| end > *b) {
                        (x1 - x0).max(4.0)
                    } else {
                        x1 - x0
                    };
                    fill(
                        c,
                        Rect::new(body.x + x0, y + 2.0, w, bs * 1.25),
                        selection_ink(k),
                    );
                }
                text(c, &body_text[*a..*b], body.x, y + bs, bs, k.ink, 400);
            }
            y += lh;
        }
        if self.focus == Focus::Body {
            let li = line_of(&self.body_lines, caret);
            let (a, _) = self.body_lines.get(li).copied().unwrap_or((0, 0));
            let cx = body.x + width_of(c, &body_text[a..caret.max(a)], bs, 400);
            let cy = body.y - self.body_scroll + li as f32 * lh;
            fill(c, Rect::new(cx, cy + 2.0, 2.0, bs * 1.25), k.ink);
        }
        let _ = canvas2d::clear_clip(c);
        said.field(body, "Note", &body_text);
    }

    /// One line of editable text, clipped, scrolled so the caret shows.
    #[allow(clippy::too_many_arguments)]
    fn draw_line_field(
        &self,
        r: Rect,
        which: Focus,
        placeholder: &str,
        size: f32,
        weight: u16,
        ink: gfx::Color,
        k: &Ink,
    ) {
        let c = self.c;
        let field = match which {
            Focus::Search => &self.search,
            Focus::Title => match self.open.as_ref() {
                Some(o) => &o.title,
                None => return,
            },
            _ => return,
        };
        let focused = self.focus == which;
        let base = baseline(r.y, r.h, size);
        let _ = canvas2d::set_clip(c, r.x - 2.0, r.y, r.w + 4.0, r.h);
        if field.text.is_empty() {
            text(c, placeholder, r.x, base, size, k.muted, weight);
        }
        let caret_x = width_of(c, &field.text[..field.caret], size, weight);
        let shift = (caret_x - (r.w - 6.0)).max(0.0);
        if let Some((a, b)) = field.selection().filter(|_| focused) {
            let x0 = width_of(c, &field.text[..a], size, weight);
            let x1 = width_of(c, &field.text[..b], size, weight);
            fill(
                c,
                Rect::new(r.x - shift + x0, base - size * 0.95, x1 - x0, size * 1.2),
                selection_ink(k),
            );
        }
        text(c, &field.text, r.x - shift, base, size, ink, weight);
        if focused {
            fill(
                c,
                Rect::new(r.x - shift + caret_x, base - size * 0.95, 2.0, size * 1.2),
                k.ink,
            );
        }
        let _ = canvas2d::clear_clip(c);
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
        let line = format!("Deleted \"{}\"", undo.note.shown_title());
        let tap = self.screen.tap().max(36.0);
        let max_line = (area.w - 140.0).min(380.0);
        let shown = fit(c, &line, s, 400, max_line);
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

    fn short_name(&self, name: &str) -> String {
        if !self.team.me.is_empty() && name == self.team.me {
            return String::from("you");
        }
        String::from(name.split('@').next().unwrap_or(name))
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
                self.save();
                return true;
            }
            return false;
        };
        match hit {
            Hit::NewNote => self.new_note("", ""),
            Hit::Suggest(i) => {
                if let Some((title, body)) = SUGGESTIONS.get(i) {
                    self.new_note(title, body);
                }
            }
            Hit::Open(i) => self.open_note(i),
            Hit::Search => {
                self.save();
                self.focus = Focus::Search;
                let size = self.search_size;
                let inner_x = x - self.search_rect.x - 12.0;
                let t = self.search.text.clone();
                self.search.move_to(nearest(self.c, &t, 0, t.len(), inner_x, size, 400), false);
            }
            Hit::ClearSearch => {
                self.search.set("");
                self.list_scroll = 0.0;
            }
            Hit::Title => {
                self.focus = Focus::Title;
                let (r, size, c) = (self.title_rect, self.title_size, self.c);
                if let Some(o) = self.open.as_mut() {
                    let t = o.title.text.clone();
                    let caret_x = width_of(c, &t[..o.title.caret], size, 650);
                    let shift = (caret_x - (r.w - 6.0)).max(0.0);
                    o.title.move_to(nearest(c, &t, 0, t.len(), x - r.x + shift, size, 650), false);
                }
            }
            Hit::Body => {
                self.focus = Focus::Body;
                let (r, size, lh, c) = (self.body_rect, self.body_size, self.body_lh, self.c);
                let row = ((y - r.y + self.body_scroll) / lh).floor().max(0.0) as usize;
                let lines = self.body_lines.clone();
                if let Some(o) = self.open.as_mut() {
                    let at = match lines.get(row) {
                        Some((a, b)) => nearest(c, &o.body.text, *a, *b, x - r.x, size, 400),
                        None => o.body.text.len(),
                    };
                    o.body.move_to(at, false);
                }
            }
            Hit::Pin => self.toggle_pin(),
            Hit::Delete => self.delete(),
            Hit::Back => {
                self.save();
                self.phone_editor = false;
                self.focus = Focus::None;
            }
            Hit::Undo => self.undo_delete(),
            Hit::Dismiss => self.team.problem = None,
        }
        true
    }

    fn typed(&mut self, s: &str) -> bool {
        let now = self.now();
        match self.focus {
            Focus::Search => {
                self.list_scroll = 0.0;
                self.search.insert(s)
            }
            Focus::Title | Focus::Body => {
                let body = self.focus == Focus::Body;
                let Some(o) = self.open.as_mut() else {
                    return false;
                };
                let changed = if body {
                    o.body.insert(s)
                } else {
                    o.title.insert(s)
                };
                if changed {
                    o.edited_at = Some(now);
                }
                changed
            }
            Focus::None => false,
        }
    }

    /// A key press. `shift` selects; `primary` is Cmd on a Mac and Ctrl
    /// elsewhere (Cmd+A selects all).
    fn key(&mut self, key: &str, shift: bool, primary: bool) -> bool {
        let now = self.now();
        match (self.focus, key) {
            (Focus::None, _) => false,
            (_, "Escape") => {
                self.focus = Focus::None;
                self.save();
                true
            }
            (Focus::Title, "Enter") | (Focus::Title, "Tab") => {
                self.focus = Focus::Body;
                if let Some(o) = self.open.as_mut() {
                    o.body.move_to(0, false);
                }
                true
            }
            (Focus::Search, "Enter") => {
                if !self.visible().is_empty() {
                    self.open_note(0);
                }
                true
            }
            (Focus::Body, "Enter") => self.typed("\n"),
            (Focus::Body, "ArrowUp") | (Focus::Body, "ArrowDown") => {
                let down = key == "ArrowDown";
                let (c, size) = (self.c, self.body_size);
                let lines = self.body_lines.clone();
                if let Some(o) = self.open.as_mut() {
                    let li = line_of(&lines, o.body.caret);
                    let target = if down { li + 1 } else { li.wrapping_sub(1) };
                    if let (Some((a, _)), Some((ta, tb))) = (lines.get(li), lines.get(target)) {
                        let x = width_of(c, &o.body.text[*a..o.body.caret], size, 400);
                        let at = nearest(c, &o.body.text, *ta, *tb, x, size, 400);
                        o.body.move_to(at, shift);
                    } else if down {
                        o.body.move_to(o.body.text.len(), shift);
                    } else {
                        o.body.move_to(0, shift);
                    }
                }
                true
            }
            (focus, key) => {
                let field = match focus {
                    Focus::Search => Some(&mut self.search),
                    Focus::Title => self.open.as_mut().map(|o| &mut o.title),
                    Focus::Body => self.open.as_mut().map(|o| &mut o.body),
                    Focus::None => None,
                };
                let Some(field) = field else {
                    return false;
                };
                let (changed, handled) = field.key_with(key, shift, primary);
                if changed && focus != Focus::Search {
                    if let Some(o) = self.open.as_mut() {
                        o.edited_at = Some(now);
                    }
                }
                if changed && focus == Focus::Search {
                    self.list_scroll = 0.0;
                }
                handled
            }
        }
    }

    fn wheel(&mut self, x: f32, y: f32, dy: f32) -> bool {
        if self.body_rect.contains(x, y) {
            let next = (self.body_scroll + dy).clamp(0.0, self.body_max);
            let moved = next != self.body_scroll;
            self.body_scroll = next;
            return moved;
        }
        if self.list_rect.contains(x, y) {
            let next = (self.list_scroll + dy).clamp(0.0, self.list_max);
            let moved = next != self.list_scroll;
            self.list_scroll = next;
            return moved;
        }
        false
    }

    /// Save once typing has paused.
    fn save_if_paused(&mut self, now: u64) -> bool {
        let due = self
            .open
            .as_ref()
            .and_then(|o| o.edited_at)
            .is_some_and(|at| now >= at + SAVE_AFTER_MS);
        if due {
            self.save();
        }
        due
    }

    fn typing(&self, now: u64) -> bool {
        self.open
            .as_ref()
            .and_then(|o| o.edited_at)
            .is_some_and(|at| now < at + 2_000)
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
                width: 980,
                height: 660,
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
            w: 980.0,
            h: 660.0,
            team: Team::new(),
            clock: Clock::new(),
            notes: Vec::new(),
            cursor: None,
            open: None,
            focus: Focus::None,
            search: Field::new(false),
            phone_editor: false,
            list_scroll: 0.0,
            list_max: 0.0,
            list_rect: Rect::default(),
            body_scroll: 0.0,
            body_max: 0.0,
            body_rect: Rect::default(),
            body_lines: Vec::new(),
            body_size: 15.0,
            body_lh: 24.0,
            title_rect: Rect::default(),
            title_size: 26.0,
            search_rect: Rect::default(),
            search_size: 14.0,
            hits: Vec::new(),
            undo: None,
            made: 0,
            titled: false,
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
            let wait = if app.open.as_ref().is_some_and(|o| o.edited_at.is_some()) {
                120
            } else {
                500
            };
            let event = events::wait(Some(wait));
            let mut dirty = false;
            match event {
                Some(types::Event::CloseRequested(_)) => {
                    app.save();
                    let _ = window::close(win);
                    return 0;
                }
                Some(types::Event::Resized(_)) => {
                    app.screen = Screen::of(win);
                    dirty = true;
                }
                Some(types::Event::ThemeChanged(_)) => {
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
                dirty |= app.save_if_paused(now);
                if now >= looked + LOOK_EVERY_MS {
                    looked = now;
                    dirty |= app.look_for_changes();
                    // Where this computer stands: local, never waits.
                    dirty |= app.team.look();
                    // The status words ("synced 2 min ago") age too.
                    dirty |= app.undo.is_some();
                }
                if now >= synced + SYNC_EVERY_MS && !app.typing(now) {
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
