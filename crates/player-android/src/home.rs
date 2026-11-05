//! What the player writes for Krate's home screen (apps/krate-home) and how
//! it reads the answer: the library's index, the Library and Discover rows,
//! and the one line the screen prints. Kept apart from the phone-only code
//! so every rule here is tested on any machine; the iPhone player follows
//! the same contract.

use crate::run_config::sanitize;
use std::collections::HashMap;

/// A value for one tab-separated field: no tabs or line breaks inside, so
/// a name cannot add columns or rows to the lists the home screen reads.
pub fn field(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

/// A size as a person reads it.
pub fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024).max(1))
    }
}

/// The file an app is kept in, in the library: named for its app, so a
/// newer copy of an app replaces the older one.
pub fn library_file(app_id: &str) -> String {
    format!("{}.krate", sanitize(app_id))
}

/// One kept app, as the library's index remembers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indexed {
    pub name: String,
    pub sha: String,
    pub size: u64,
}

/// The library's index (`index.tsv`): one line per kept app -- file name,
/// app name, SHA-256, size -- so the home screen lists the library without
/// unpacking every app each time it is drawn. Lines that are not four
/// fields are not read.
pub fn read_index(text: &str) -> HashMap<String, Indexed> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            (f.len() == 4).then(|| {
                (
                    f[0].to_string(),
                    Indexed {
                        name: f[1].to_string(),
                        sha: f[2].to_string(),
                        size: f[3].parse().unwrap_or(0),
                    },
                )
            })
        })
        .collect()
}

/// The index with `file`'s line replaced, or added at the end.
pub fn index_with(text: &str, file: &str, kept: &Indexed) -> String {
    let mut lines: Vec<String> = text
        .lines()
        .filter(|line| !line.trim().is_empty() && line.split('\t').next() != Some(file))
        .map(str::to_string)
        .collect();
    lines.push(format!(
        "{}\t{}\t{}\t{}",
        field(file),
        field(&kept.name),
        field(&kept.sha),
        kept.size
    ));
    lines.join("\n") + "\n"
}

/// A Library row with what the app page shows too:
/// `name<TAB>line<TAB>art<TAB>data<TAB>version` -- `data` what the app keeps
/// on this phone, `version` its own.
pub fn library_row_full(
    name: &str,
    size: u64,
    art: Option<&str>,
    from_hub: bool,
    data: u64,
    version: Option<&str>,
) -> String {
    format!(
        "{}\t{} \u{00b7} {}\t{}\t{}\t{}",
        field(name),
        if from_hub { "From the hub" } else { "Received" },
        human_size(size),
        art.map(field).unwrap_or_else(|| "-".to_string()),
        if data == 0 {
            "None".to_string()
        } else {
            human_size(data)
        },
        version.map(field).unwrap_or_default()
    )
}

/// A Discover row:
/// `hub id<TAB>name<TAB>author<TAB>category<TAB>description<TAB>art<TAB>size`.
/// An app with no category is filed under "apps".
#[allow(clippy::too_many_arguments)]
pub fn discover_row(
    id: &str,
    name: &str,
    author: &str,
    category: &str,
    description: &str,
    art: Option<&str>,
    size: u64,
) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        hub_id(id),
        field(name),
        field(author),
        field(if category.is_empty() {
            "apps"
        } else {
            category
        }),
        field(description),
        art.map(field).unwrap_or_else(|| "-".to_string()),
        human_size(size),
    )
}

/// A hub id as the player uses it in a link: hex digits only, so an answer
/// cannot make the download address anything but `/a/<hex>`.
pub fn hub_id(text: &str) -> String {
    text.trim()
        .chars()
        .filter(char::is_ascii_hexdigit)
        .collect()
}

/// What the person did on the home screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeAnswer {
    /// Open the n-th Library app.
    Open(usize),
    /// Get this app from the hub.
    Get(String),
    /// Open a .krate from the phone's files.
    Files,
    /// Sign in to Krate (the profile tab).
    SignIn,
    /// Sign out of Krate on this phone.
    SignOut,
    /// Remove the n-th Library app from this phone, its data with it.
    Remove(usize),
    /// Clear what the n-th Library app keeps on this phone.
    Clear(usize),
    /// Look for a newer version of the n-th Library app.
    Update(usize),
    /// Share the n-th Library app.
    Share(usize),
    /// Try the hub again (Discover's offline and error states).
    Refresh,
    /// Report a problem with Krate.
    Report,
    /// Something about keeping apps the same on every device (IC-981).
    Sync(SyncAnswer),
    /// An answer this Krate does not know (a newer screen): stay home. Only
    /// `home:closed`, or no answer at all, leaves.
    Stay,
    /// Leave Krate (or no answer at all: a crashed screen).
    Closed,
}

/// What the person did about sync, on the home screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAnswer {
    /// The switch for every app on this phone: on or off.
    All(bool),
    /// The n-th Library app's own switch.
    App(usize, bool),
    /// Yes to the waiting device with this id (hex only).
    Approve(String),
    /// The recovery code, as typed (letters, digits and dashes only).
    Recover(String),
    /// The person wrote the recovery code down: forget it here.
    CodeKept,
    /// The n-th `conflict` line of sync.txt, answered.
    Resolve(usize, Choice),
    /// Ask the hub again how this phone stands.
    Check,
}

/// An answer to a change made on two devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Keep this phone's.
    This,
    /// Keep the other device's.
    Other,
    /// Keep both: the other's beside this one, under its device's name.
    Both,
    /// Put two lists or records together.
    Merge,
}

fn sync_answer(rest: &str) -> Option<SyncAnswer> {
    match rest {
        "on" => return Some(SyncAnswer::All(true)),
        "off" => return Some(SyncAnswer::All(false)),
        "code-kept" => return Some(SyncAnswer::CodeKept),
        "check" => return Some(SyncAnswer::Check),
        _ => {}
    }
    if let Some(n) = rest.strip_prefix("app-on:") {
        return n.trim().parse().ok().map(|n| SyncAnswer::App(n, true));
    }
    if let Some(n) = rest.strip_prefix("app-off:") {
        return n.trim().parse().ok().map(|n| SyncAnswer::App(n, false));
    }
    if let Some(id) = rest.strip_prefix("approve:") {
        let id = hub_id(id);
        return (!id.is_empty() && id.len() <= 64).then_some(SyncAnswer::Approve(id));
    }
    if let Some(code) = rest.strip_prefix("recover:") {
        let code: String = code
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(64)
            .collect();
        return (!code.is_empty()).then_some(SyncAnswer::Recover(code));
    }
    let (n, choice) = rest.strip_prefix("resolve:")?.split_once(':')?;
    let choice = match choice.trim() {
        "this" => Choice::This,
        "other" => Choice::Other,
        "both" => Choice::Both,
        "merge" => Choice::Merge,
        _ => return None,
    };
    Some(SyncAnswer::Resolve(n.trim().parse().ok()?, choice))
}

/// The screen's answer: its first `home:` line. Anything else, or nothing,
/// is leaving.
pub fn home_answer(stdout: &str) -> HomeAnswer {
    let Some(line) = stdout
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("home:"))
    else {
        return HomeAnswer::Closed;
    };
    if let Some(n) = line.strip_prefix("home:open:") {
        return n.parse().map(HomeAnswer::Open).unwrap_or(HomeAnswer::Stay);
    }
    if let Some(id) = line.strip_prefix("home:get:") {
        let id = hub_id(id);
        return if id.is_empty() {
            HomeAnswer::Stay
        } else {
            HomeAnswer::Get(id)
        };
    }
    if let Some(rest) = line.strip_prefix("home:sync:") {
        return sync_answer(rest)
            .map(HomeAnswer::Sync)
            .unwrap_or(HomeAnswer::Stay);
    }
    match line {
        "home:files" => return HomeAnswer::Files,
        "home:signin" => return HomeAnswer::SignIn,
        "home:signout" => return HomeAnswer::SignOut,
        "home:refresh" => return HomeAnswer::Refresh,
        "home:report" => return HomeAnswer::Report,
        _ => {}
    }
    let numbered = |prefix: &str, make: fn(usize) -> HomeAnswer| {
        line.strip_prefix(prefix)
            .and_then(|n| n.trim().parse().ok())
            .map(make)
    };
    numbered("home:remove:", HomeAnswer::Remove)
        .or_else(|| numbered("home:clear:", HomeAnswer::Clear))
        .or_else(|| numbered("home:update:", HomeAnswer::Update))
        .or_else(|| numbered("home:share:", HomeAnswer::Share))
        .unwrap_or(if line == "home:closed" {
            HomeAnswer::Closed
        } else {
            HomeAnswer::Stay
        })
}

/// A signed-in person, as Krate keeps them on this phone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Account {
    pub login: String,
    pub name: String,
    /// The avatar's file under art/, once fetched.
    pub avatar: Option<String>,
    /// (name, role) for each organisation.
    pub orgs: Vec<(String, String)>,
    /// (hub id, name, line) for each app they published.
    pub published: Vec<(String, String, String)>,
}

/// The profile tab's `account.txt`: who is signed in (or `signed-out`), then
/// their organisations, published apps, this phone, storage and version.
pub fn account_file(
    account: Option<&Account>,
    device: &str,
    storage: u64,
    version: &str,
) -> String {
    let mut lines = Vec::new();
    match account {
        Some(a) => {
            let name = if a.name.trim().is_empty() {
                &a.login
            } else {
                &a.name
            };
            lines.push(format!(
                "signed-in\t{}\t{}\t{}",
                field(name),
                field(&a.login),
                a.avatar
                    .as_deref()
                    .map(field)
                    .unwrap_or_else(|| "-".to_string())
            ));
            for (org, role) in &a.orgs {
                lines.push(format!("org\t{}\t{}", field(org), field(role)));
            }
            for (id, name, line) in &a.published {
                lines.push(format!(
                    "published\t{}\t{}\t{}",
                    hub_id(id),
                    field(name),
                    field(line)
                ));
            }
        }
        None => lines.push("signed-out".to_string()),
    }
    lines.push(format!("device\t{}", field(device)));
    lines.push(format!("storage\t{}", human_size(storage)));
    lines.push(format!("version\t{}", field(version)));
    lines.join("\n") + "\n"
}

/// What a krate://signed-in hand-back carries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SignedIn {
    pub token: String,
    pub login: String,
    pub name: String,
    pub avatar_url: String,
    pub app_nonce: String,
}

/// Read `krate://signed-in?token=...&app_nonce=...` (query or fragment, the
/// way krate.tech/login/done writes it). None for any other address.
pub fn signed_in_fields(back: &str) -> Option<SignedIn> {
    let rest = back.strip_prefix("krate://signed-in")?;
    let query = rest.trim_start_matches('/').trim_start_matches(['?', '#']);
    let mut out = SignedIn::default();
    for pair in query.split(['&', '#']) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode(value);
        match key {
            "token" => out.token = value,
            "login" => out.login = value,
            "name" => out.name = value,
            "avatar_url" => out.avatar_url = value,
            "app_nonce" => out.app_nonce = value,
            _ => {}
        }
    }
    Some(out)
}

/// `%XX` and `+` as a query writes them; a `%` not followed by two hex
/// digits stays as it is.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One app's `perms/<n>.txt`: each thing it can use in plain words, and
/// whether it is `needed` (always given) or `asked` (asked at every open).
pub fn perms_file(uses: &[(String, bool)]) -> String {
    uses.iter()
        .map(|(words, needed)| {
            format!(
                "{}\t{}",
                field(words),
                if *needed { "needed" } else { "asked" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// How this phone stands with sync, for the first line of `sync.txt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    /// Nobody signed in on this phone.
    SignedOut,
    /// The person turned it off for every app here.
    Off,
    /// Asked the hub and nothing came back yet, the first time.
    Checking,
    /// Syncing.
    On,
    /// Registered, and waiting for a yes from another device (or the code).
    Waiting,
    /// The hub is out of reach: what shows is what it last said.
    Offline,
    /// Something went wrong, said in words.
    Error,
}

impl SyncState {
    fn word(self) -> &'static str {
        match self {
            SyncState::SignedOut => "signed-out",
            SyncState::Off => "off",
            SyncState::Checking => "checking",
            SyncState::On => "on",
            SyncState::Waiting => "waiting",
            SyncState::Offline => "offline",
            SyncState::Error => "error",
        }
    }
}

/// One Library app's sync, for its page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppSync {
    /// Kept in sync; when it last synced, in words ("" for never).
    On(String),
    /// Its own switch is off.
    Off,
    /// It keeps no data that syncs (it never asks for store.kv).
    Nothing,
}

/// One change made on two devices, as its card shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictCard {
    /// The Library app's place.
    pub app: usize,
    /// The entry's name in the app's store.
    pub key: String,
    /// This phone's value in words; `None` when it was deleted here.
    pub mine: Option<String>,
    /// The other device's; `None` when it was deleted there.
    pub theirs: Option<String>,
    /// The other device's name.
    pub device: String,
    /// More devices that changed it too.
    pub more: usize,
    /// Both are lists, or both records: they can be put together.
    pub merge: bool,
}

/// Everything Krate's home shows about sync (`sync.txt`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncView {
    pub state: SyncState,
    /// What the state means and what to do, in words.
    pub words: String,
    /// A look at the hub is under way: the screen reads the file again.
    pub checking: bool,
    pub device: String,
    /// When this phone last synced, in words ("" for never).
    pub last: String,
    /// The account's other devices: id, name, and whether it reads the
    /// apps' data (else it waits for a yes).
    pub devices: Vec<(String, String, bool)>,
    /// This phone holds the account's key, so it can say yes to another.
    pub can_approve: bool,
    pub apps: Vec<(usize, AppSync)>,
    pub conflicts: Vec<ConflictCard>,
}

/// `sync.txt`, one record a line (the contract is apps/krate-home's):
///
/// - `state<TAB>signed-out|off|checking|on|waiting|offline|error<TAB>words`
/// - `checking` -- a look at the hub is under way
/// - `this<TAB>device name<TAB>last sync`
/// - `device<TAB>id<TAB>name<TAB>reads|waiting|ask` (`ask`: waiting, and
///   this phone can say yes)
/// - `app<TAB>n<TAB>on|off|none<TAB>last sync`
/// - `conflict<TAB>n<TAB>key<TAB>this value<TAB>other value<TAB>other
///   device<TAB>more<TAB>flags`, flags a comma list of `merge`,
///   `this-deleted`, `other-deleted`
pub fn sync_file(view: &SyncView) -> String {
    let dash = |text: &str| {
        if text.trim().is_empty() {
            "-".to_string()
        } else {
            field(text)
        }
    };
    let mut lines = vec![format!(
        "state\t{}\t{}",
        view.state.word(),
        dash(&view.words)
    )];
    if view.checking {
        lines.push("checking".to_string());
    }
    lines.push(format!(
        "this\t{}\t{}",
        dash(&view.device),
        dash(&view.last)
    ));
    for (id, name, reads) in &view.devices {
        let how = match (reads, view.can_approve) {
            (true, _) => "reads",
            (false, true) => "ask",
            (false, false) => "waiting",
        };
        lines.push(format!("device\t{}\t{}\t{how}", hub_id(id), dash(name)));
    }
    for (n, app) in &view.apps {
        let (state, last) = match app {
            AppSync::On(last) => ("on", dash(last)),
            AppSync::Off => ("off", "-".to_string()),
            AppSync::Nothing => ("none", "-".to_string()),
        };
        lines.push(format!("app\t{n}\t{state}\t{last}"));
    }
    for c in &view.conflicts {
        let mut flags = Vec::new();
        if c.merge {
            flags.push("merge");
        }
        if c.mine.is_none() {
            flags.push("this-deleted");
        }
        if c.theirs.is_none() {
            flags.push("other-deleted");
        }
        lines.push(format!(
            "conflict\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            c.app,
            dash(&c.key),
            c.mine
                .as_deref()
                .map(dash)
                .unwrap_or_else(|| "-".to_string()),
            c.theirs
                .as_deref()
                .map(dash)
                .unwrap_or_else(|| "-".to_string()),
            dash(&c.device),
            c.more,
            if flags.is_empty() {
                "-".to_string()
            } else {
                flags.join(",")
            }
        ));
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_cannot_add_columns_or_rows() {
        assert_eq!(field("a\tb\nc\rd"), "a b c d");
        let row = library_row_full("Evil\t-\nNext", 2048, None, false, 0, None);
        assert_eq!(row.lines().count(), 1);
        assert_eq!(row.split('\t').count(), 5);
    }

    #[test]
    fn sizes_read_as_a_person_reads_them() {
        assert_eq!(human_size(0), "1 KB");
        assert_eq!(human_size(1), "1 KB");
        assert_eq!(human_size(1025), "2 KB");
        assert_eq!(human_size(3 * 1024 * 1024 / 2), "1.5 MB");
    }

    #[test]
    fn an_app_is_kept_under_a_safe_name_of_its_id() {
        assert_eq!(library_file("dev.krate.notes"), "dev.krate.notes.krate");
        assert_eq!(library_file("../../etc"), ".._.._etc.krate");
    }

    #[test]
    fn the_index_keeps_one_line_per_file() {
        let one = Indexed {
            name: "Notes".into(),
            sha: "ab".into(),
            size: 10,
        };
        let text = index_with("", "notes.krate", &one);
        assert_eq!(text, "notes.krate\tNotes\tab\t10\n");
        let newer = Indexed {
            name: "Notes 2".into(),
            sha: "cd".into(),
            size: 20,
        };
        let other = Indexed {
            name: "Chess".into(),
            sha: "ef".into(),
            size: 30,
        };
        let text = index_with(&text, "chess.krate", &other);
        let text = index_with(&text, "notes.krate", &newer);
        let read = read_index(&text);
        assert_eq!(read.len(), 2);
        assert_eq!(read["notes.krate"], newer);
        assert_eq!(read["chess.krate"], other);
        // A broken line is skipped, not misread.
        let read = read_index("half\tline\n");
        assert!(read.is_empty());
    }

    #[test]
    fn a_name_cannot_forge_an_index_line() {
        let sneaky = Indexed {
            name: "A\nfake.krate\tB\tff\t1".into(),
            sha: "00".into(),
            size: 1,
        };
        let read = read_index(&index_with("", "a.krate", &sneaky));
        assert_eq!(read.len(), 1);
        assert!(read.contains_key("a.krate"));
    }

    #[test]
    fn discover_rows_carry_seven_fields() {
        let row = discover_row(
            "ab12",
            "Chess",
            "Ann",
            "",
            "Play\tchess",
            Some("ab.rgba"),
            4096,
        );
        let f: Vec<&str> = row.split('\t').collect();
        assert_eq!(
            f,
            [
                "ab12",
                "Chess",
                "Ann",
                "apps",
                "Play chess",
                "ab.rgba",
                "4 KB"
            ]
        );
        let row = discover_row("ab", "X", "", "games", "", None, 1);
        assert_eq!(row.split('\t').nth(5), Some("-"));
        assert_eq!(row.split('\t').nth(3), Some("games"));
    }

    #[test]
    fn the_home_screens_answer_is_read_strictly() {
        assert_eq!(home_answer("home:open:2\n"), HomeAnswer::Open(2));
        assert_eq!(
            home_answer("noise\nhome:get:ab12\n"),
            HomeAnswer::Get("ab12".into())
        );
        // An id is hex only: nothing else reaches the download address.
        assert_eq!(
            home_answer("home:get:ab/../x?y=1"),
            HomeAnswer::Get("ab1".into())
        );
        // A malformed answer stays home: leaving looked like a crash.
        assert_eq!(home_answer("home:get:zz"), HomeAnswer::Stay);
        assert_eq!(home_answer("home:files"), HomeAnswer::Files);
        assert_eq!(home_answer("home:closed"), HomeAnswer::Closed);
        assert_eq!(home_answer("home:open:x"), HomeAnswer::Stay);
        assert_eq!(home_answer(""), HomeAnswer::Closed);
    }

    #[test]
    fn the_profile_and_app_answers_are_read() {
        assert_eq!(home_answer("home:signin\n"), HomeAnswer::SignIn);
        assert_eq!(home_answer("home:signout"), HomeAnswer::SignOut);
        assert_eq!(home_answer("home:refresh"), HomeAnswer::Refresh);
        assert_eq!(home_answer("home:report"), HomeAnswer::Report);
        assert_eq!(home_answer("home:remove:2"), HomeAnswer::Remove(2));
        assert_eq!(home_answer("home:clear:0"), HomeAnswer::Clear(0));
        assert_eq!(home_answer("home:update:1"), HomeAnswer::Update(1));
        assert_eq!(home_answer("home:share:3"), HomeAnswer::Share(3));
        assert_eq!(home_answer("home:remove:x"), HomeAnswer::Stay);
        assert_eq!(
            home_answer("home:something-newer"),
            HomeAnswer::Stay,
            "a newer screen's answer keeps Krate open"
        );
        assert_eq!(home_answer("home:closed"), HomeAnswer::Closed);
        assert_eq!(
            home_answer(""),
            HomeAnswer::Closed,
            "a screen that crashed leaves"
        );
    }

    #[test]
    fn the_account_file_cannot_be_forged_by_a_name() {
        let a = Account {
            login: "yash".into(),
            name: "Yash\tPardeshi\nsigned-out".into(),
            avatar: None,
            orgs: vec![("Acme".into(), "owner".into())],
            published: vec![("ab12".into(), "Notes".into(), "12 opens".into())],
        };
        let text = account_file(Some(&a), "Galaxy S22", 2048, "0.5.5");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "signed-in\tYash Pardeshi signed-out\tyash\t-");
        assert_eq!(lines[1], "org\tAcme\towner");
        assert_eq!(lines[2], "published\tab12\tNotes\t12 opens");
        assert_eq!(lines.len(), 6);
        assert_eq!(
            account_file(None, "x", 0, "v").lines().next(),
            Some("signed-out")
        );
        assert_eq!(
            perms_file(&[("Use the camera".into(), false)]),
            "Use the camera\tasked\n"
        );
    }

    #[test]
    fn a_full_library_row_carries_data_and_version() {
        let row = library_row_full("Notes\tx", 2048, Some("a.rgba"), true, 0, Some("1.2.0"));
        assert_eq!(
            row,
            "Notes x\tFrom the hub \u{00b7} 2 KB\ta.rgba\tNone\t1.2.0"
        );
        assert_eq!(row.split('\t').count(), 5);
    }

    #[test]
    fn the_sync_answers_are_read_strictly() {
        let sync = |line: &str| home_answer(line);
        assert_eq!(
            sync("home:sync:on"),
            HomeAnswer::Sync(SyncAnswer::All(true))
        );
        assert_eq!(
            sync("home:sync:off"),
            HomeAnswer::Sync(SyncAnswer::All(false))
        );
        assert_eq!(
            sync("home:sync:app-off:2"),
            HomeAnswer::Sync(SyncAnswer::App(2, false))
        );
        assert_eq!(
            sync("home:sync:app-on:0"),
            HomeAnswer::Sync(SyncAnswer::App(0, true))
        );
        assert_eq!(
            sync("home:sync:approve:ab12cd34ef56ab78"),
            HomeAnswer::Sync(SyncAnswer::Approve("ab12cd34ef56ab78".into()))
        );
        assert_eq!(
            sync("home:sync:approve:../etc"),
            HomeAnswer::Sync(SyncAnswer::Approve("ec".into())),
            "an id is hex only"
        );
        assert_eq!(
            sync("home:sync:recover:ABCDE-FGHJK 12;rm"),
            HomeAnswer::Sync(SyncAnswer::Recover("ABCDE-FGHJK12rm".into())),
            "a code is letters, digits and dashes only"
        );
        assert_eq!(
            sync("home:sync:code-kept"),
            HomeAnswer::Sync(SyncAnswer::CodeKept)
        );
        assert_eq!(sync("home:sync:check"), HomeAnswer::Sync(SyncAnswer::Check));
        for (word, choice) in [
            ("this", Choice::This),
            ("other", Choice::Other),
            ("both", Choice::Both),
            ("merge", Choice::Merge),
        ] {
            assert_eq!(
                sync(&format!("home:sync:resolve:3:{word}")),
                HomeAnswer::Sync(SyncAnswer::Resolve(3, choice))
            );
        }
        // Anything malformed, or newer, stays home.
        for line in [
            "home:sync:resolve:3:everything",
            "home:sync:resolve:x:this",
            "home:sync:resolve:3",
            "home:sync:approve:zz",
            "home:sync:recover:",
            "home:sync:app-on:x",
            "home:sync:rotate-keys",
        ] {
            assert_eq!(sync(line), HomeAnswer::Stay, "{line}");
        }
    }

    #[test]
    fn the_sync_file_carries_every_state_and_cannot_be_forged() {
        let view = SyncView {
            state: SyncState::On,
            words: "On".into(),
            checking: true,
            device: "Galaxy S22\tUltra".into(),
            last: "2 minutes ago".into(),
            devices: vec![
                ("bbbbbbbbbbbbbbbb".into(), "Yash's MacBook Pro".into(), true),
                ("cccccccccccccccc".into(), "iPad\nstate\toff".into(), false),
            ],
            can_approve: true,
            apps: vec![
                (0, AppSync::On("just now".into())),
                (1, AppSync::Off),
                (2, AppSync::Nothing),
            ],
            conflicts: vec![
                ConflictCard {
                    app: 0,
                    key: "r1".into(),
                    mine: Some("title: Milk".into()),
                    theirs: Some("title: Oat\tmilk".into()),
                    device: "Yash's MacBook Pro".into(),
                    more: 0,
                    merge: true,
                },
                ConflictCard {
                    app: 0,
                    key: "r2".into(),
                    mine: Some("Tea".into()),
                    theirs: None,
                    device: "iPad".into(),
                    more: 1,
                    merge: false,
                },
            ],
        };
        let text = sync_file(&view);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "state\ton\tOn");
        assert_eq!(lines[1], "checking");
        assert_eq!(lines[2], "this\tGalaxy S22 Ultra\t2 minutes ago");
        assert_eq!(
            lines[3],
            "device\tbbbbbbbbbbbbbbbb\tYash's MacBook Pro\treads"
        );
        assert_eq!(lines[4], "device\tcccccccccccccccc\tiPad state off\task");
        assert_eq!(lines[5], "app\t0\ton\tjust now");
        assert_eq!(lines[6], "app\t1\toff\t-");
        assert_eq!(lines[7], "app\t2\tnone\t-");
        assert_eq!(
            lines[8],
            "conflict\t0\tr1\ttitle: Milk\ttitle: Oat milk\tYash's MacBook Pro\t0\tmerge"
        );
        assert_eq!(lines[9], "conflict\t0\tr2\tTea\t-\tiPad\t1\tother-deleted");
        assert_eq!(lines.len(), 10, "a name cannot add a line");
        // Waiting devices only get a yes from a phone that holds the key.
        let mut waiting = view.clone();
        waiting.can_approve = false;
        waiting.state = SyncState::Waiting;
        waiting.checking = false;
        let text = sync_file(&waiting);
        assert!(text.contains("\twaiting\n") && !text.contains("\task\n"));
        assert!(text.starts_with("state\twaiting\t"));
        let out = SyncView {
            state: SyncState::SignedOut,
            words: String::new(),
            checking: false,
            device: String::new(),
            last: String::new(),
            devices: vec![],
            can_approve: false,
            apps: vec![],
            conflicts: vec![],
        };
        assert_eq!(sync_file(&out), "state\tsigned-out\t-\nthis\t-\t-\n");
    }

    #[test]
    fn a_sign_in_hand_back_is_read_and_only_with_its_fields() {
        let back = "krate://signed-in?token=abc%2B1&login=yash&name=Yash+Pardeshi&avatar_url=https%3A%2F%2Fa.b%2Fc.png&app_nonce=0123456789abcdef0123456789abcdef";
        let got = signed_in_fields(back).expect("a hand-back");
        assert_eq!(got.token, "abc+1");
        assert_eq!(got.name, "Yash Pardeshi");
        assert_eq!(got.avatar_url, "https://a.b/c.png");
        assert_eq!(got.app_nonce, "0123456789abcdef0123456789abcdef");
        assert!(signed_in_fields("https://evil.example/?token=x").is_none());
        assert!(signed_in_fields("krate://signed-in#token=t&app_nonce=n")
            .is_some_and(|f| f.token == "t"));
        assert_eq!(percent_decode("100%"), "100%");
    }
}
