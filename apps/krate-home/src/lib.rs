//! Krate's own screens on a phone: Library (your apps), Discover (the apps
//! on the hub), You (your account), Settings, and a page for each app.
//!
//! A trusted guest the phone player runs. The player does what a sandboxed
//! screen should not -- the network, picture decoding, the account -- and
//! writes what to show into this app's folder. Every list is UTF-8 text,
//! one record a line, fields split by a TAB; blank lines are skipped, a
//! missing field or `-` is "not known", and fields past the ones named
//! here are ignored, so a newer player can add some.
//!
//! - `library.txt`: one app per line,
//!   `name<TAB>line<TAB>art[<TAB>data<TAB>version]`. `line` is the muted
//!   line under the name ("Received · 471 KB"); `data` is what the app
//!   keeps on this phone as a person reads it ("2.1 MB", "640 KB"); `version`
//!   is the app's version ("1.4.0"). `data` and `version` may be left off.
//!   Settings lists the apps by `data`, largest first.
//! - `discover.txt`: one app per line,
//!   `hub id<TAB>name<TAB>author<TAB>category<TAB>description<TAB>art<TAB>size`.
//!   The first app with a picture is featured at the top.
//! - `hub-state.txt` (optional): how the last look at the hub went, one
//!   line: `loading` (Discover shows placeholders and this screen reads the
//!   files again every 0.6 s until it changes), `ready`, `offline`, or
//!   `error<TAB>words a person can read`. Missing means `ready` when
//!   discover.txt lists apps and `offline` when it does not. A player that
//!   writes `loading` writes discover.txt first, then `ready`.
//! - `progress.txt` (optional): downloads under way, one a line,
//!   `get<TAB>hub id<TAB>percent` (0 to 100). That app's Get shows how far
//!   it has got. Read again every 0.6 s while it lists any.
//! - `account.txt` (optional; missing means signed out). The first line is
//!   `signed-in<TAB>name<TAB>handle<TAB>avatar art` or `signed-out`. Then,
//!   in any order and any number:
//!   - `org<TAB>name<TAB>role` -- an organisation and the person's role in
//!     it ("Owner", "Member").
//!   - `published<TAB>hub id<TAB>name<TAB>line` -- an app the person
//!     published; `line` is muted text under it ("12 opens").
//!   - `device<TAB>name` -- this phone's name ("Galaxy S22 Ultra").
//!   - `storage<TAB>size` -- what Krate keeps on this phone ("48 MB").
//!   - `version<TAB>version` -- the Krate player's version ("0.5.6").
//!
//!   `device`, `storage` and `version` are read when signed out too.
//! - `perms/<n>.txt` (optional): what the n-th Library app may use, one
//!   capability a line, `plain words<TAB>needed` (always given) or
//!   `plain words<TAB>asked` (asked each time it opens). Missing means not
//!   known, and the app's page leaves the section out; an empty file means
//!   it uses nothing beyond its own window.
//! - `art/*.rgba`: pictures, an 8-byte header (width, height, little-endian
//!   u32) then straight RGBA. `-` names no picture: the app wears Krate's
//!   one app icon. Names are plain file names, no folders.
//! - `sync.txt` (optional; missing hides sync, for an older player): the
//!   same app data on every device the person signs in on (IC-981). One
//!   record a line:
//!   - `state<TAB>signed-out|off|checking|on|waiting|offline|error<TAB>words`
//!     first: how this phone stands, and `words` saying what that means
//!     and what to do. `waiting` is a phone that needs a yes from another
//!     device (or the recovery code); `offline` keeps working and syncs
//!     later.
//!   - `checking`: a look at the hub is under way; the screen reads the
//!     files again every 0.6 s until it goes.
//!   - `this<TAB>device name<TAB>last sync` ("2 minutes ago").
//!   - `device<TAB>id<TAB>name<TAB>reads|waiting|ask`: another of the
//!     person's devices. `reads` reads the apps' data; `waiting` waits for
//!     a yes; `ask` waits and this phone can give it (Approve).
//!   - `app<TAB>n<TAB>on|off|none<TAB>last sync`: the n-th Library app's own
//!     switch; `none` is an app that keeps no data to sync.
//!   - `conflict<TAB>n<TAB>key<TAB>this value<TAB>other value<TAB>other
//!     device<TAB>more<TAB>flags`: something the n-th app keeps changed on
//!     two devices. Values are in words; `more` counts further devices
//!     that changed it; `flags` is a comma list of `merge` (both are lists
//!     or records), `this-deleted`, `other-deleted`.
//!
//!   The arguments may carry `sync-code` and the recovery code: shown once,
//!   until the person says they wrote it down. Never in a file.
//! - `start.txt` (optional): where to open. `library` (the default),
//!   `discover`, `you`, `settings` (or one of its pages: `storage`,
//!   `safety`, `links`, `sync`, `recover`), `app:<n>` (the n-th Library app's page) or
//!   `hub:<hub id>` (a hub app's page). A player that comes back after
//!   an answer about one app (clear, update, share) writes `app:<n>` so
//!   the person lands where they were.
//!
//! The answer is one stdout line, after which the screen closes:
//!
//! - `home:open:<n>` -- open the n-th Library app.
//! - `home:get:<hub id>` -- get this app from the hub.
//! - `home:files` -- open a .krate from the phone's files.
//! - `home:signin` / `home:signout` -- start signing in, or sign out.
//! - `home:clear:<n>` -- delete what the n-th Library app saved (the
//!   person has already confirmed it here).
//! - `home:update:<n>` -- look for a newer version of the n-th app.
//! - `home:share:<n>` -- share the n-th app.
//! - `home:remove:<n>` -- remove the n-th app from this phone (confirmed
//!   here first).
//! - `home:report` -- the person wants to report a problem with Krate.
//! - `home:refresh` -- try the hub again (Discover's offline and error
//!   states offer it).
//! - `home:sync:on` / `home:sync:off` -- sync for every app on this phone.
//! - `home:sync:app-on:<n>` / `home:sync:app-off:<n>` -- the n-th app's own.
//! - `home:sync:approve:<device id>` -- yes to a waiting device (confirmed
//!   here first: "Let Galaxy S22 Ultra read your apps' data?").
//! - `home:sync:recover:<code>` -- the recovery code, as typed here.
//! - `home:sync:code-kept` -- the person wrote the recovery code down.
//! - `home:sync:resolve:<c>:<this|other|both|merge>` -- the c-th
//!   `conflict` line, answered: keep this phone's, the other device's,
//!   both, or put the two together.
//! - `home:sync:check` -- ask the hub again.
//! - `home:closed` -- the person left Krate.
//!
//! Going into a page and back, switching places and confirming are drawn
//! here and answer nothing. A close request (the system back gesture) on a
//! page goes back one step; only on a list does it answer `home:closed`.
//!
//! Every picture is kept once (krate:gfx/canvas-images) and drawn by handle,
//! and only what is on screen is drawn: a scrolling list of screenshots
//! sent as pixels every frame lagged on an iPhone and broke its pictures.
//! Every frame is painted on the CPU on a phone, so the look is built from
//! cheap pieces: flat fills, hairlines, and pictures kept at the size they
//! are shown -- the one soft shadow too, made once. Text is measured once per
//! width; the screen moves only while something is moving and otherwise
//! waits for the next touch.

#![no_std]

extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use krate::bindings::krate::io::{args, stdio};
use krate::bindings::krate::time::clock;
use krate::gfx::{canvas2d, canvas_images, types as gfx};
use krate::layout::{Look, Said};
use krate::ui::{events, tree, types, window};

mod kit;
mod screens;

use kit::*;

const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

/// The Krate mark, 120 px square, straight RGBA.
const MARK: &[u8] = include_bytes!("../assets/mark-120.rgba");
const MARK_SIZE: u32 = 120;

const MS: u64 = 1_000_000;
/// How long a tapped row shows as pressed before the player takes over.
const FLASH_NS: u64 = 90 * MS;
/// A page sliding in or out.
const PAGE_NS: u64 = 300 * MS;
/// The selected tab's pill moving to its new place.
const TAB_NS: u64 = 260 * MS;
/// The sheet rising, or sinking away.
const SHEET_NS: u64 = 280 * MS;
/// A list's rows fading in the first time it shows, one after another.
const ROWS_NS: u64 = 260 * MS;
const ROW_STEP_NS: u64 = 35 * MS;
/// How often the files are read again while the hub is loading or an app
/// is downloading.
const POLL_NS: u64 = 600 * MS;

fn out(line: &str) {
    let handle = stdio::stdout();
    let _ = handle.write(line.as_bytes());
    let _ = handle.write(b"\n");
}

struct LibraryApp {
    name: String,
    art: String,
    line: String,
    data: String,
    version: String,
    face: Face,
}

struct HubApp {
    id: String,
    name: String,
    author: String,
    /// The muted line in a list: author and size.
    by: String,
    category: String,
    description: String,
    size: String,
    art: String,
    face: Face,
}

struct Published {
    id: String,
    name: String,
    line: String,
    /// The same app in Discover, when the hub lists it.
    hub: Option<usize>,
}

struct Org {
    name: String,
    role: String,
    face: Face,
}

#[derive(Default)]
struct Account {
    signed_in: bool,
    name: String,
    handle: String,
    avatar: Option<Face>,
    avatar_art: String,
    orgs: Vec<Org>,
    published: Vec<Published>,
    device: String,
    storage: String,
    version: String,
}

/// The featured app's picture, kept in the shape its card has now.
struct Featured {
    index: usize,
    raw: Raw,
    tint: (f32, f32, f32),
    /// Cropped to fill the card, and the card shape it was cut for.
    fill: Option<(u64, f32)>,
    /// The whole picture, for one too tall to crop (a phone app).
    whole: Option<u64>,
    backdrop: Option<u64>,
}

#[derive(Clone, PartialEq)]
enum HubState {
    Ready,
    Loading,
    Offline,
    Error(String),
}

/// How this phone stands with sync (`sync.txt`'s first line).
#[derive(Clone, Copy, PartialEq, Default)]
enum SyncState {
    #[default]
    SignedOut,
    Off,
    Checking,
    On,
    Waiting,
    Offline,
    Error,
}

/// Another of the person's devices.
#[derive(Clone, PartialEq)]
struct SyncDevice {
    id: String,
    name: String,
    /// It reads the apps' data.
    reads: bool,
    /// It waits, and this phone can say yes.
    ask: bool,
}

/// One Library app's own switch.
#[derive(Clone, PartialEq)]
enum AppSync {
    /// On, with when it last synced ("" for never).
    On(String),
    Off,
    /// It keeps no data to sync.
    Nothing,
}

/// Something one app keeps that changed on two devices.
#[derive(Clone, PartialEq)]
struct Conflict {
    app: usize,
    key: String,
    /// This phone's value in words; `None` when deleted here.
    mine: Option<String>,
    theirs: Option<String>,
    device: String,
    more: usize,
    merge: bool,
}

/// Everything `sync.txt` says.
#[derive(Clone, PartialEq, Default)]
struct Sync {
    /// The player wrote one: sync shows at all.
    known: bool,
    state: SyncState,
    words: String,
    checking: bool,
    device: String,
    last: String,
    devices: Vec<SyncDevice>,
    apps: Vec<(usize, AppSync)>,
    conflicts: Vec<Conflict>,
}

impl Sync {
    fn app(&self, n: usize) -> Option<&AppSync> {
        self.apps.iter().find(|(k, _)| *k == n).map(|(_, a)| a)
    }

    /// Things waiting for the person: changes to choose between and
    /// devices to say yes to.
    fn waiting(&self) -> usize {
        if matches!(self.state, SyncState::Off | SyncState::SignedOut) {
            return 0;
        }
        self.conflicts.len() + self.devices.iter().filter(|d| d.ask).count()
    }
}

fn read_sync() -> Sync {
    let records = lines("sync.txt");
    let mut sync = Sync {
        known: !records.is_empty(),
        ..Sync::default()
    };
    for f in records {
        match f[0].as_str() {
            "state" => {
                sync.state = match field(&f, 1).as_str() {
                    "off" => SyncState::Off,
                    "checking" => SyncState::Checking,
                    "on" => SyncState::On,
                    "waiting" => SyncState::Waiting,
                    "offline" => SyncState::Offline,
                    "error" => SyncState::Error,
                    _ => SyncState::SignedOut,
                };
                sync.words = field(&f, 2);
            }
            "checking" => sync.checking = true,
            "this" => {
                sync.device = field(&f, 1);
                sync.last = field(&f, 2);
            }
            "device" if f.len() >= 4 => {
                let how = field(&f, 3);
                sync.devices.push(SyncDevice {
                    id: f[1].chars().filter(char::is_ascii_hexdigit).collect(),
                    name: field(&f, 2),
                    reads: how == "reads",
                    ask: how == "ask",
                });
            }
            "app" if f.len() >= 3 => {
                let Ok(n) = f[1].parse::<usize>() else {
                    continue;
                };
                let app = match field(&f, 2).as_str() {
                    "on" => AppSync::On(field(&f, 3)),
                    "off" => AppSync::Off,
                    _ => AppSync::Nothing,
                };
                sync.apps.push((n, app));
            }
            "conflict" if f.len() >= 7 => {
                let Ok(app) = f[1].parse::<usize>() else {
                    continue;
                };
                let flags = field(&f, 7);
                let has = |flag: &str| flags.split(',').any(|x| x.trim() == flag);
                sync.conflicts.push(Conflict {
                    app,
                    key: field(&f, 2),
                    mine: (!has("this-deleted")).then(|| field(&f, 3)),
                    theirs: (!has("other-deleted")).then(|| field(&f, 4)),
                    device: {
                        let d = field(&f, 5);
                        if d.is_empty() {
                            "another device".to_string()
                        } else {
                            d
                        }
                    },
                    more: f[6].parse().unwrap_or(0),
                    merge: has("merge"),
                });
            }
            _ => {}
        }
    }
    sync
}

/// An answer to a change made on two devices.
#[derive(Clone, Copy, PartialEq)]
enum Choice {
    This,
    Other,
    Both,
    Merge,
}

impl Choice {
    fn word(self) -> &'static str {
        match self {
            Choice::This => "this",
            Choice::Other => "other",
            Choice::Both => "both",
            Choice::Merge => "merge",
        }
    }
}

/// A recovery code is 26 letters and digits.
const CODE_LEN: usize = 26;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Library,
    Discover,
    You,
}

impl Tab {
    fn name(self) -> &'static str {
        match self {
            Tab::Library => "Library",
            Tab::Discover => "Discover",
            Tab::You => "You",
        }
    }

    fn slot(self) -> usize {
        match self {
            Tab::Library => 0,
            Tab::Discover => 1,
            Tab::You => 2,
        }
    }
}

const TABS: [Tab; 3] = [Tab::Library, Tab::Discover, Tab::You];

/// A page over a list.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    /// The n-th Library app.
    Lib(usize),
    /// A hub app, by its place in Discover.
    Hub(usize),
    /// One of the person's published apps the hub list does not carry.
    Pub(usize),
    Settings,
    /// What each app keeps on this phone.
    Storage,
    /// How Krate keeps apps safe.
    Safety,
    /// How apps open from links and files.
    Links,
    /// The same app data on every device.
    Sync,
    /// Typing the recovery code.
    Recover,
}

impl Target {
    fn name(self, home: &Home) -> String {
        match self {
            Target::Lib(n) => home.library[n].name.clone(),
            Target::Hub(i) => home.hub[i].name.clone(),
            Target::Pub(p) => home.account.published[p].name.clone(),
            Target::Settings => "Settings".to_string(),
            Target::Storage => "Storage".to_string(),
            Target::Safety => "Safety".to_string(),
            Target::Links => "Links".to_string(),
            Target::Sync => "Sync".to_string(),
            Target::Recover => "Recovery code".to_string(),
        }
    }
}

/// An open page and how far down it is scrolled.
#[derive(Clone, Copy)]
struct Page {
    target: Target,
    scroll: f32,
}

/// Something asked before it is done.
#[derive(Clone, Copy, PartialEq)]
enum Ask {
    Remove(usize),
    Clear(usize),
    /// Yes to the i-th device of `sync.devices`.
    Approve(usize),
}

/// Something tappable drawn this frame.
#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Open(usize),
    Get(usize),
    GetPub(usize),
    Tab(Tab),
    Files,
    Show(Target),
    Back,
    SignIn,
    SignOut,
    Clear(usize),
    Update(usize),
    Share(usize),
    Remove(usize),
    Report,
    Refresh,
    /// Sync for every app: turn it to this.
    SyncAll(bool),
    /// The n-th app's own switch: turn it to this.
    SyncApp(usize, bool),
    /// Ask before saying yes to the i-th device.
    Approve(usize),
    /// The c-th conflict, answered.
    Resolve(usize, Choice),
    /// The recovery code is written down.
    CodeKept,
    /// Ask the hub again.
    SyncCheck,
    /// The recovery code's field: type into it.
    CodeField,
    /// Use the code typed.
    CodeSubmit,
    /// Confirm what was asked.
    Yes,
    /// Do not.
    No,
    /// Inside a sheet, on nothing: swallows the tap.
    Stay,
}

impl Hit {
    /// What the player is told, for a hit that leaves the screen.
    fn answer(&self, home: &Home) -> Option<String> {
        Some(match self {
            Hit::Open(n) => format!("home:open:{n}"),
            Hit::Get(i) => format!("home:get:{}", home.hub[*i].id),
            Hit::GetPub(p) => format!("home:get:{}", home.account.published[*p].id),
            Hit::Files => "home:files".to_string(),
            Hit::SignIn => "home:signin".to_string(),
            Hit::SignOut => "home:signout".to_string(),
            Hit::Update(n) => format!("home:update:{n}"),
            Hit::Share(n) => format!("home:share:{n}"),
            Hit::Report => "home:report".to_string(),
            Hit::Refresh => "home:refresh".to_string(),
            Hit::SyncAll(on) => format!("home:sync:{}", if *on { "on" } else { "off" }),
            Hit::SyncApp(n, on) => {
                format!("home:sync:app-{}:{n}", if *on { "on" } else { "off" })
            }
            Hit::Resolve(c, choice) => format!("home:sync:resolve:{c}:{}", choice.word()),
            Hit::CodeKept => "home:sync:code-kept".to_string(),
            Hit::SyncCheck => "home:sync:check".to_string(),
            Hit::CodeSubmit => {
                let code = home.typed_code();
                if code.len() < CODE_LEN {
                    return None;
                }
                format!("home:sync:recover:{code}")
            }
            Hit::Yes => match home.ask? {
                Ask::Remove(n) => format!("home:remove:{n}"),
                Ask::Clear(n) => format!("home:clear:{n}"),
                Ask::Approve(i) => format!("home:sync:approve:{}", home.sync.devices.get(i)?.id),
            },
            _ => return None,
        })
    }
}

/// Sizes for this frame, from the screen and the person's text size.
#[derive(Clone, Copy)]
struct Metrics {
    w: f32,
    h: f32,
    /// The content column.
    x: f32,
    col: f32,
    /// The band the lists scroll in.
    top: f32,
    bottom: f32,
    /// The header's band.
    head_y: f32,
    head_h: f32,
    /// Where the floating bar's top edge is: content ends above it.
    nav_top: f32,
    /// Covered at the bottom (the home bar).
    safe_bottom: f32,
    /// Text sizes.
    title: f32,
    name: f32,
    body: f32,
    small: f32,
    button: f32,
    /// The person's text size against the default, held to what fits.
    scale: f32,
}

struct Home {
    canvas: u64,
    win: u64,
    library: Vec<LibraryApp>,
    hub: Vec<HubApp>,
    account: Account,
    featured: Option<Featured>,
    /// Discover's sections: a category and the hub apps in it, in order.
    sections: Vec<(String, Vec<usize>)>,
    hub_state: HubState,
    /// Downloads under way: hub id and percent.
    progress: Vec<(String, u32)>,
    /// What sync.txt says.
    sync: Sync,
    /// The recovery code to show once, from the player's arguments.
    code: Option<String>,
    /// The recovery code typed so far (letters and digits).
    typed: String,
    /// The keyboard is up for it.
    typing: bool,
    /// Use was tapped with too few letters.
    code_short: bool,
    next_poll: u64,
    mark: Option<u64>,
    fades: Vec<((u32, u32), u64)>,
    /// Kept icons, by shape, colour and size.
    icons: Vec<(u64, u64)>,
    /// The floating bar's shadow and the size it was made for.
    nav_shadow: Option<((u32, u32), u64)>,
    /// Faces already kept, by picture file: a hub app in the Library too
    /// is kept once.
    kept_faces: Vec<(String, Face)>,
    /// Some face may still be waiting for its picture.
    faces_left: bool,
    text: Text,
    look: Look,
    /// This frame's time, for everything that moves.
    now: u64,
    tab: Tab,
    /// The tab the pill is moving from, and since when.
    tab_from: Tab,
    tab_since: u64,
    /// When each list first showed, for its rows fading in; 0 for never.
    seen: [u64; 3],
    /// The pages open over the list, the top one last.
    stack: Vec<Page>,
    /// A page sliding in (true) or out (false), and since when.
    page_anim: Option<(bool, u64)>,
    /// Asked and waiting for an answer.
    ask: Option<Ask>,
    /// The sheet rising (true) or sinking (false), and since when.
    sheet_anim: Option<(bool, u64)>,
    /// A Library app's permissions: words, and needed (or asked).
    perms: Option<(usize, Vec<(String, bool)>)>,
    /// A hub app's screenshot: its place, handle, width, height.
    shot: Option<(usize, u64, u32, u32)>,
    scroll: [f32; 3],
    /// The content's height and the band it scrolls in, as last drawn.
    content: f32,
    span: f32,
    hits: Vec<(gfx::Rect, Hit)>,
    /// Held down now (a mouse), or tapped and about to leave (a finger).
    pressed: Option<Hit>,
    /// The press was let go over what it pressed.
    tapped: bool,
    said: Said,
}

/// Eased progress from `since` over `span` at `now`, 0 to 1: fast out of
/// the gate and settling softly, the way a spring with no bounce lands.
fn progress(now: u64, since: u64, span: u64) -> f32 {
    let t = (now.saturating_sub(since) as f32 / span as f32).clamp(0.0, 1.0);
    let inv = 1.0 - t;
    1.0 - inv * inv * inv * inv
}

impl Home {
    fn scroll_mut(&mut self) -> &mut f32 {
        if let Some(page) = self.stack.last_mut() {
            return &mut page.scroll;
        }
        &mut self.scroll[self.tab.slot()]
    }

    fn max_scroll(&self) -> f32 {
        (self.content - self.span).max(0.0)
    }

    /// The face for a picture file, kept once.
    fn face_for(&mut self, art: &str) -> Face {
        if let Some((_, face)) = self.kept_faces.iter().find(|(a, _)| a == art) {
            return *face;
        }
        let face = keep_face(self.canvas, read_raw(art).as_ref());
        self.kept_faces.push((art.to_string(), face));
        face
    }

    /// Keep one picture still waiting, the open place's first. False when
    /// none is left.
    fn keep_next_face(&mut self) -> bool {
        let hub_first = self.tab == Tab::Discover;
        for pass in 0..2 {
            if (pass == 0) != hub_first {
                if let Some(n) = self.library.iter().position(|a| a.face.pending) {
                    let art = self.library[n].art.clone();
                    self.library[n].face = self.face_for(&art);
                    return true;
                }
            } else if let Some(i) = self.hub.iter().position(|a| a.face.pending) {
                let art = self.hub[i].art.clone();
                self.hub[i].face = self.face_for(&art);
                return true;
            }
        }
        if self.account.avatar.is_some_and(|f| f.pending) {
            let art = self.account.avatar_art.clone();
            let face = self.face_for(&art);
            self.account.avatar = Some(if face.tile.is_some() {
                face
            } else {
                Face::hue(self.canvas, &self.account.name)
            });
            return true;
        }
        self.faces_left = false;
        false
    }

    /// Read the hub's list (again), keeping what can be kept.
    fn load_hub(&mut self) {
        let canvas = self.canvas;
        if let Some(feat) = self.featured.take() {
            for handle in [feat.fill.map(|f| f.0), feat.whole, feat.backdrop]
                .into_iter()
                .flatten()
            {
                canvas_images::forget(canvas, handle);
            }
        }
        if let Some((_, handle, _, _)) = self.shot.take() {
            canvas_images::forget(canvas, handle);
        }
        let mut hub: Vec<HubApp> = Vec::new();
        let mut featured = None;
        for f in lines("discover.txt").into_iter().filter(|f| f.len() >= 7) {
            // The featured app is the first with a picture that reads.
            let raw = if featured.is_none() && names_art(&f[5]) {
                read_raw(&f[5])
            } else {
                None
            };
            let face = match &raw {
                Some(raw) => match self.kept_faces.iter().find(|(a, _)| *a == f[5]) {
                    Some((_, face)) => *face,
                    None => {
                        let face = keep_face(canvas, Some(raw));
                        self.kept_faces.push((f[5].clone(), face));
                        face
                    }
                },
                None if names_art(&f[5]) => Face::WAITING,
                None => Face::KRATE,
            };
            if featured.is_none() {
                if let Some(raw) = raw {
                    featured = Some(Featured {
                        index: hub.len(),
                        tint: tint(&raw),
                        raw,
                        fill: None,
                        whole: None,
                        backdrop: None,
                    });
                }
            }
            let author = field(&f, 2);
            let size = field(&f, 6);
            hub.push(HubApp {
                id: f[0].clone(),
                name: f[1].clone(),
                by: match (author.is_empty(), size.is_empty()) {
                    (false, false) => format!("{author} \u{00b7} {size}"),
                    (false, true) => author.clone(),
                    _ => size.clone(),
                },
                author,
                category: if f[3].is_empty() {
                    "apps".to_string()
                } else {
                    f[3].to_lowercase()
                },
                description: field(&f, 4),
                size,
                art: f[5].clone(),
                face,
            });
        }
        // Sections in the order the hub lists them, the featured app shown
        // once, at the top.
        let mut sections: Vec<(String, Vec<usize>)> = Vec::new();
        for (index, app) in hub.iter().enumerate() {
            if featured
                .as_ref()
                .is_some_and(|f: &Featured| f.index == index)
            {
                continue;
            }
            match sections.iter_mut().find(|(c, _)| *c == app.category) {
                Some((_, apps)) => apps.push(index),
                None => sections.push((app.category.clone(), alloc::vec![index])),
            }
        }
        for p in self.account.published.iter_mut() {
            p.hub = hub.iter().position(|a| a.id == p.id && !p.id.is_empty());
        }
        // A page about a hub app no longer listed closes.
        self.stack.retain(|p| !matches!(p.target, Target::Hub(_)));
        self.hub = hub;
        self.featured = featured;
        self.sections = sections;
        self.faces_left = true;
    }

    /// Read how the hub is doing and what is downloading. True when the
    /// screen should change.
    fn read_state(&mut self) -> bool {
        let state = match lines("hub-state.txt").first() {
            Some(f) => match f[0].as_str() {
                "loading" => HubState::Loading,
                "offline" => HubState::Offline,
                "error" => HubState::Error(field(f, 1)),
                _ => HubState::Ready,
            },
            None if self.hub.is_empty() => HubState::Offline,
            None => HubState::Ready,
        };
        let progress: Vec<(String, u32)> = lines("progress.txt")
            .into_iter()
            .filter(|f| f.len() >= 3 && f[0] == "get")
            .map(|f| {
                let id: String = f[1].chars().filter(char::is_ascii_hexdigit).collect();
                (id, f[2].parse::<u32>().unwrap_or(0).min(100))
            })
            .collect();
        let mut changed = progress != self.progress;
        self.progress = progress;
        let sync = read_sync();
        if sync != self.sync {
            self.sync = sync;
            changed = true;
        }
        if state != self.hub_state {
            if self.hub_state == HubState::Loading {
                self.load_hub();
            }
            self.hub_state = state;
            changed = true;
        }
        changed
    }

    fn polling(&self) -> bool {
        self.hub_state == HubState::Loading || !self.progress.is_empty() || self.sync.checking
    }

    /// The recovery code typed so far, letters and digits only.
    fn typed_code(&self) -> String {
        self.typed.clone()
    }

    /// Put the keyboard away, if it is up.
    fn stop_typing(&mut self) {
        if self.typing {
            krate::ui::text_entry::stop(self.win);
            self.typing = false;
        }
    }

    /// Open a page over what is showing.
    fn push(&mut self, target: Target) {
        if self.page_anim.is_some() || self.stack.len() >= 6 {
            return;
        }
        match target {
            Target::Lib(n) => {
                self.perms = read_all(&format!("perms/{n}.txt")).map(|bytes| {
                    let list = core::str::from_utf8(&bytes)
                        .unwrap_or("")
                        .lines()
                        .filter_map(|l| {
                            let (words, how) = l.split_once('\t')?;
                            let words = words.trim();
                            (!words.is_empty()).then(|| (words.to_string(), how.trim() != "asked"))
                        })
                        .collect();
                    (n, list)
                });
            }
            Target::Hub(i) => {
                if self.shot.is_some_and(|s| s.0 != i) {
                    if let Some((_, handle, _, _)) = self.shot.take() {
                        canvas_images::forget(self.canvas, handle);
                    }
                }
                if self.shot.is_none() {
                    if let Some(raw) = read_raw(&self.hub[i].art) {
                        if let Ok(handle) =
                            canvas_images::keep(self.canvas, raw.width, raw.height, &raw.rgba)
                        {
                            self.shot = Some((i, handle, raw.width, raw.height));
                        }
                    }
                }
            }
            _ => {}
        }
        self.stack.push(Page {
            target,
            scroll: 0.0,
        });
        self.page_anim = Some((true, self.now));
    }

    /// Start the top page sliding away.
    fn pop(&mut self) -> bool {
        if self.stack.is_empty() {
            return false;
        }
        self.stop_typing();
        if self.page_anim.is_none() {
            self.page_anim = Some((false, self.now));
        }
        true
    }

    fn close_sheet(&mut self) {
        if self.ask.is_some() && !matches!(self.sheet_anim, Some((false, _))) {
            self.sheet_anim = Some((false, self.now));
        }
    }

    fn switch(&mut self, tab: Tab) {
        self.stop_typing();
        if tab != self.tab {
            self.tab_from = self.tab;
            self.tab_since = self.now;
            self.tab = tab;
        }
        self.stack.clear();
        self.page_anim = None;
        self.ask = None;
        self.sheet_anim = None;
        if self.seen[tab.slot()] == 0 {
            self.seen[tab.slot()] = self.now.max(1);
        }
    }

    /// Do what a tap asks that stays on this screen.
    fn act(&mut self, hit: Hit) {
        match hit {
            Hit::Tab(tab) => self.switch(tab),
            Hit::Show(target) => self.push(target),
            Hit::Back => {
                self.pop();
            }
            Hit::Clear(n) => self.ask_for(Ask::Clear(n)),
            Hit::Remove(n) => self.ask_for(Ask::Remove(n)),
            Hit::Approve(i) => self.ask_for(Ask::Approve(i)),
            Hit::No => self.close_sheet(),
            Hit::CodeField => {
                // A phone shows its keyboard; a computer's is always there.
                let _ = krate::ui::text_entry::start(self.win);
                self.typing = true;
                self.code_short = false;
            }
            // Too few letters: say so under the field, and stay.
            Hit::CodeSubmit => self.code_short = true,
            _ => {}
        }
    }

    fn ask_for(&mut self, ask: Ask) {
        self.ask = Some(ask);
        self.sheet_anim = Some((true, self.now));
    }

    /// Back one step: a sheet, then a page. False when there is nowhere to
    /// go back to.
    fn back(&mut self) -> bool {
        if self.ask.is_some() {
            self.close_sheet();
            return true;
        }
        self.pop()
    }

    /// Finish what has finished moving. True while anything still moves.
    fn tick(&mut self, now: u64) -> bool {
        self.now = now;
        let mut moving = false;
        if let Some((pushing, since)) = self.page_anim {
            if now - since >= PAGE_NS {
                self.page_anim = None;
                if !pushing {
                    if let Some(page) = self.stack.pop() {
                        if let Target::Hub(i) = page.target {
                            if self.shot.is_some_and(|s| s.0 == i) {
                                if let Some((_, handle, _, _)) = self.shot.take() {
                                    canvas_images::forget(self.canvas, handle);
                                }
                            }
                        }
                    }
                }
            }
            moving = true;
        }
        if let Some((opening, since)) = self.sheet_anim {
            if now - since >= SHEET_NS {
                self.sheet_anim = None;
                if !opening {
                    self.ask = None;
                }
            }
            moving = true;
        }
        if now - self.tab_since < TAB_NS {
            moving = true;
        }
        let seen = self.seen[self.tab.slot()];
        let rows = match self.tab {
            Tab::Library => !self.library.is_empty(),
            Tab::Discover => !self.hub.is_empty(),
            Tab::You => false,
        };
        if rows && self.stack.is_empty() && seen > 0 && now - seen < ROWS_NS + ROW_STEP_NS * 14 {
            moving = true;
        }
        // Discover's placeholders breathe while the hub loads.
        if self.tab == Tab::Discover
            && self.stack.is_empty()
            && self.hub.is_empty()
            && self.hub_state == HubState::Loading
        {
            moving = true;
        }
        moving
    }
}

fn read_account(canvas: u64, hub: &[HubApp]) -> Account {
    let mut account = Account::default();
    for (i, f) in lines("account.txt").into_iter().enumerate() {
        let tag = f[0].as_str();
        if i == 0 && tag == "signed-in" {
            account.signed_in = true;
            account.name = field(&f, 1);
            account.handle = field(&f, 2).trim_start_matches('@').to_string();
            if account.name.is_empty() {
                account.name = account.handle.clone();
            }
            account.avatar_art = field(&f, 3);
            account.avatar = Some(if names_art(&account.avatar_art) {
                Face::WAITING
            } else {
                Face::hue(canvas, &account.name)
            });
            continue;
        }
        match tag {
            "org" if f.len() >= 2 => {
                let name = field(&f, 1);
                account.orgs.push(Org {
                    face: Face::hue(canvas, &name),
                    role: field(&f, 2),
                    name,
                });
            }
            "published" if f.len() >= 3 => {
                let id: String = f[1].chars().filter(char::is_ascii_hexdigit).collect();
                account.published.push(Published {
                    hub: hub.iter().position(|a| a.id == id && !id.is_empty()),
                    line: field(&f, 3),
                    name: field(&f, 2),
                    id,
                });
            }
            "device" => account.device = field(&f, 1),
            "storage" => account.storage = field(&f, 1),
            "version" => account.version = field(&f, 1),
            _ => {}
        }
    }
    account
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

/// What an event asks of the loop.
enum Next {
    Stay,
    Leave(String),
}

struct Component;

impl krate::Guest for Component {
    fn run() -> i32 {
        let Ok(win) = window::create(
            "Krate",
            types::WindowSize {
                width: 390,
                height: 844,
            },
        ) else {
            out("home:closed");
            return 1;
        };
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err() {
            out("home:closed");
            return 1;
        }
        let _ = tree::upsert_node(
            win,
            &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
        );
        let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
            out("home:closed");
            return 1;
        };

        // Every picture crosses into the host once, at the size it is
        // shown: the first screen's before the first frame, the rest while
        // the screen is idle (see `keep_next_face`), so opening Krate never
        // waits on pictures nobody is looking at.
        let library: Vec<LibraryApp> = lines("library.txt")
            .into_iter()
            .filter(|f| f.len() >= 3)
            .map(|f| LibraryApp {
                face: if names_art(&f[2]) {
                    Face::WAITING
                } else {
                    Face::KRATE
                },
                art: f[2].clone(),
                name: f[0].clone(),
                line: f[1].clone(),
                data: field(&f, 3),
                version: field(&f, 4),
            })
            .collect();
        let now = clock::monotonic_nanos();
        let mut home = Home {
            canvas,
            win,
            library,
            hub: Vec::new(),
            account: Account::default(),
            featured: None,
            sections: Vec::new(),
            hub_state: HubState::Ready,
            progress: Vec::new(),
            sync: read_sync(),
            code: {
                // `sync-code` and the code: shown once, never in a file.
                let raw = args::raw();
                let mut parts = raw.split('\n');
                (parts.next() == Some("sync-code"))
                    .then(|| parts.next())
                    .flatten()
                    .map(|c| {
                        c.chars()
                            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                            .collect::<String>()
                    })
                    .filter(|c| !c.is_empty())
            },
            typed: String::new(),
            typing: false,
            code_short: false,
            next_poll: 0,
            mark: canvas_images::keep(canvas, MARK_SIZE, MARK_SIZE, MARK).ok(),
            fades: Vec::new(),
            icons: Vec::new(),
            nav_shadow: None,
            kept_faces: Vec::new(),
            faces_left: true,
            text: Text::new(canvas),
            look: Look::now(),
            now,
            tab: Tab::Library,
            tab_from: Tab::Library,
            tab_since: 0,
            seen: [0; 3],
            stack: Vec::new(),
            page_anim: None,
            ask: None,
            sheet_anim: None,
            perms: None,
            shot: None,
            scroll: [0.0; 3],
            content: 0.0,
            span: 1.0,
            hits: Vec::new(),
            pressed: None,
            tapped: false,
            said: Said::new(),
        };
        home.load_hub();
        home.account = read_account(canvas, &home.hub);
        home.read_state();

        let start = read_all("start.txt")
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
            .unwrap_or_default();
        let tab = match start.split_once(':').map(|(a, _)| a).unwrap_or(&start) {
            "discover" | "hub" => Tab::Discover,
            "you" | "settings" | "storage" | "safety" | "links" | "sync" | "recover" => Tab::You,
            _ => Tab::Library,
        };
        home.switch(tab);
        home.tab_since = 0;
        match start.split_once(':') {
            Some(("app", n)) => {
                if let Ok(n) = n.trim().parse::<usize>() {
                    if n < home.library.len() {
                        home.push(Target::Lib(n));
                    }
                }
            }
            Some(("hub", id)) => {
                if let Some(i) = home.hub.iter().position(|a| a.id == id.trim()) {
                    home.push(Target::Hub(i));
                }
            }
            _ => {
                let under = match start.as_str() {
                    "storage" => Some(Target::Storage),
                    "safety" => Some(Target::Safety),
                    "links" => Some(Target::Links),
                    _ => None,
                };
                if start == "settings" || under.is_some() {
                    home.push(Target::Settings);
                    home.page_anim = None;
                }
                if let Some(page) = under {
                    home.push(page);
                }
                // Sync opens from You, and the code page over it.
                if (start == "sync" || start == "recover") && home.sync.known {
                    home.push(Target::Sync);
                    home.page_anim = None;
                    if start == "recover" {
                        home.push(Target::Recover);
                    }
                }
            }
        }
        // Opening on a page shows it in place, not sliding in.
        home.page_anim = None;

        // The first screen's pictures before the first frame; the rest
        // while idle.
        for _ in 0..8 {
            if !home.keep_next_face() {
                break;
            }
        }

        // Draw only after a change. A flick keeps gliding after the finger
        // lifts and slows to a stop, the way a phone list moves: velocity is
        // in points per millisecond and every step uses the time that really
        // passed, so an uneven frame shows as a longer step, not a stutter.
        // Every event waiting is taken before the next frame, so a burst of
        // scroll steps costs one frame, not one each.
        let mut dirty = true;
        let mut velocity = 0.0f32;
        let mut last_wheel = 0u64;
        let mut last_step = 0u64;
        // A tap that leaves: the row shows pressed for a moment first.
        let mut leaving: Option<(String, u64)> = None;
        loop {
            let (w, h) = match canvas2d::canvas_size(canvas) {
                Ok(size) => (size.width.max(1.0), size.height.max(1.0)),
                Err(_) => (390.0, 844.0),
            };
            let now = clock::monotonic_nanos();
            let moving = home.tick(now);
            if dirty || moving {
                if home.draw(w, h).is_err() {
                    out("home:closed");
                    return 1;
                }
                let _ = window::request_redraw(win);
                dirty = false;
            }
            let now = clock::monotonic_nanos();
            if let Some((answer, at)) = &leaving {
                if now >= *at {
                    out(answer);
                    return 0;
                }
            }
            if home.polling() && now >= home.next_poll {
                home.next_poll = now + POLL_NS;
                dirty |= home.read_state();
            }
            let gliding = velocity.abs() > 0.02;
            let timeout = match &leaving {
                Some((_, at)) => Some((((*at - now) / MS) as u32).max(1)),
                None if gliding || moving => Some(8),
                // Pictures still to keep: look for input, then keep some.
                None if home.faces_left => Some(0),
                None if home.polling() => {
                    Some(((home.next_poll.saturating_sub(now)) / MS) as u32 + 1)
                }
                // Wake now and then to follow a change to dark mode or text
                // size the platform did not announce.
                None => Some(1000),
            };
            let mut event = events::wait(timeout);
            if event.is_none() && home.faces_left && leaving.is_none() && !gliding {
                for _ in 0..3 {
                    if !home.keep_next_face() {
                        break;
                    }
                }
                dirty = true;
            }
            let mut taken = 0;
            while let Some(e) = event.take() {
                let now = clock::monotonic_nanos();
                home.now = now;
                match handle(
                    &mut home,
                    e,
                    now,
                    &mut velocity,
                    &mut last_wheel,
                    &mut last_step,
                ) {
                    (Next::Leave(answer), _) => {
                        out(&answer);
                        return 0;
                    }
                    (Next::Stay, changed) => dirty |= changed,
                }
                taken += 1;
                if taken < 64 && leaving.is_none() && !home.tapped {
                    event = events::poll();
                }
            }
            // A tap: something that leaves shows pressed, then goes;
            // anything else happens now.
            if home.tapped && leaving.is_none() {
                home.tapped = false;
                if let Some(hit) = home.pressed {
                    match hit.answer(&home) {
                        Some(answer) => {
                            leaving = Some((answer, clock::monotonic_nanos() + FLASH_NS));
                        }
                        None => {
                            home.pressed = None;
                            home.now = clock::monotonic_nanos();
                            home.act(hit);
                            velocity = 0.0;
                        }
                    }
                    dirty = true;
                }
            }
            let now = clock::monotonic_nanos();
            if gliding && leaving.is_none() && now - last_wheel > 40 * MS {
                // The finger has lifted: keep going, slowing down at iOS's
                // own rate (0.998 per millisecond).
                let dt = ((now - last_step) as f32 / MS as f32).min(50.0);
                last_step = now;
                let max = home.max_scroll();
                let s = home.scroll_mut();
                let next = (*s + velocity * dt).clamp(0.0, max);
                if next == *s {
                    velocity = 0.0;
                } else {
                    *s = next;
                    // 0.998 to the power dt, without a float library.
                    for _ in 0..dt as u32 {
                        velocity *= 0.998;
                    }
                    velocity *= 1.0 - 0.002 * (dt - (dt as u32) as f32);
                    dirty = true;
                }
            }
            let look = Look::now();
            if look != home.look {
                dirty = true;
            }
        }
    }
}

/// Act on one event. Returns what the loop does next and whether the
/// screen changed.
fn handle(
    home: &mut Home,
    event: types::Event,
    now: u64,
    velocity: &mut f32,
    last_wheel: &mut u64,
    last_step: &mut u64,
) -> (Next, bool) {
    let busy = home.page_anim.is_some();
    match event {
        // The system back gesture: back a step, and only out of Krate from
        // a list.
        types::Event::CloseRequested(_) => {
            if home.back() {
                *velocity = 0.0;
                (Next::Stay, true)
            } else {
                (Next::Leave("home:closed".to_string()), false)
            }
        }
        // The recovery code, typed: letters and digits only, at most 26.
        types::Event::TextInput(text) if home.typing => {
            for c in text.chars().filter(char::is_ascii_alphanumeric) {
                if home.typed.len() < CODE_LEN {
                    home.typed.push(c.to_ascii_uppercase());
                }
            }
            home.code_short = false;
            (Next::Stay, true)
        }
        types::Event::TextChanged(change) if home.typing => {
            home.typed = change
                .text
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .map(|c| c.to_ascii_uppercase())
                .take(CODE_LEN)
                .collect();
            (Next::Stay, true)
        }
        types::Event::Key(key) if key.pressed && home.typing && key.key == "Backspace" => {
            home.typed.pop();
            (Next::Stay, true)
        }
        types::Event::Key(key) if key.pressed && home.typing && key.key == "Enter" => {
            match Hit::CodeSubmit.answer(home) {
                Some(answer) => {
                    home.stop_typing();
                    (Next::Leave(answer), false)
                }
                None => {
                    home.code_short = true;
                    (Next::Stay, true)
                }
            }
        }
        types::Event::Key(key)
            if key.pressed && matches!(key.key.as_str(), "Escape" | "GoBack" | "BrowserBack") =>
        {
            let changed = home.back();
            (Next::Stay, changed)
        }
        types::Event::Resized(_)
        | types::Event::RedrawRequested(_)
        | types::Event::ThemeChanged(_) => (Next::Stay, true),
        types::Event::Wheel(wheel) => {
            if home.ask.is_some() || busy {
                return (Next::Stay, false);
            }
            // The finger's own speed, smoothed; a pause forgets the speed
            // before it.
            let gap = (now - *last_wheel) as f32 / MS as f32;
            let speed = wheel.dy / gap.clamp(4.0, 100.0);
            *velocity = if gap > 100.0 {
                speed
            } else {
                *velocity * 0.3 + speed * 0.7
            };
            *last_wheel = now;
            *last_step = now;
            let mut changed = false;
            // A scroll is not a tap: let go of what was held.
            if home.pressed.take().is_some() {
                changed = true;
            }
            let max = home.max_scroll();
            let s = home.scroll_mut();
            let next = (*s + wheel.dy).clamp(0.0, max);
            if next != *s {
                *s = next;
                changed = true;
            }
            (Next::Stay, changed)
        }
        // Act on release over what was pressed, as iOS does: a press that
        // turns into a scroll is let go elsewhere, and opens nothing.
        types::Event::Pointer(p) if p.button.is_some() => {
            if busy {
                return (Next::Stay, false);
            }
            let at = home
                .hits
                .iter()
                .rposition(|(r, _)| inside(r, p.x, p.y))
                .map(|i| home.hits[i].1);
            if p.pressed {
                *velocity = 0.0;
                // A place switches at the touch, the way a tab bar does.
                if let Some(Hit::Tab(tab)) = at {
                    let changed = home.tab != tab || !home.stack.is_empty();
                    home.act(Hit::Tab(tab));
                    home.pressed = None;
                    return (Next::Stay, changed);
                }
                let changed = home.pressed != at;
                home.pressed = at;
                return (Next::Stay, changed);
            }
            let held = home.pressed;
            if at.is_some() && at == held {
                home.tapped = true;
                (Next::Stay, true)
            } else {
                home.pressed = None;
                (Next::Stay, held.is_some())
            }
        }
        _ => (Next::Stay, false),
    }
}

krate::export!(Component);
