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

/// A Library row: `name<TAB>line<TAB>art`, `-` for no picture.
pub fn library_row(name: &str, size: u64, art: Option<&str>) -> String {
    format!(
        "{}\tReceived \u{00b7} {}\t{}",
        field(name),
        human_size(size),
        art.map(field).unwrap_or_else(|| "-".to_string())
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
    /// Leave Krate (or no answer at all: a crashed screen).
    Closed,
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
        return n
            .parse()
            .map(HomeAnswer::Open)
            .unwrap_or(HomeAnswer::Closed);
    }
    if let Some(id) = line.strip_prefix("home:get:") {
        let id = hub_id(id);
        return if id.is_empty() {
            HomeAnswer::Closed
        } else {
            HomeAnswer::Get(id)
        };
    }
    match line {
        "home:files" => return HomeAnswer::Files,
        "home:signin" => return HomeAnswer::SignIn,
        "home:signout" => return HomeAnswer::SignOut,
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
        .unwrap_or(HomeAnswer::Closed)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_cannot_add_columns_or_rows() {
        assert_eq!(field("a\tb\nc\rd"), "a b c d");
        let row = library_row("Evil\t-\nNext", 2048, None);
        assert_eq!(row.lines().count(), 1);
        assert_eq!(row.split('\t').count(), 3);
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
        assert_eq!(home_answer("home:get:zz"), HomeAnswer::Closed);
        assert_eq!(home_answer("home:files"), HomeAnswer::Files);
        assert_eq!(home_answer("home:closed"), HomeAnswer::Closed);
        assert_eq!(home_answer("home:open:x"), HomeAnswer::Closed);
        assert_eq!(home_answer(""), HomeAnswer::Closed);
    }

    #[test]
    fn the_profile_and_app_answers_are_read() {
        assert_eq!(home_answer("home:signin\n"), HomeAnswer::SignIn);
        assert_eq!(home_answer("home:signout"), HomeAnswer::SignOut);
        assert_eq!(home_answer("home:remove:2"), HomeAnswer::Remove(2));
        assert_eq!(home_answer("home:clear:0"), HomeAnswer::Clear(0));
        assert_eq!(home_answer("home:update:1"), HomeAnswer::Update(1));
        assert_eq!(home_answer("home:share:3"), HomeAnswer::Share(3));
        assert_eq!(home_answer("home:remove:x"), HomeAnswer::Closed);
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
}
