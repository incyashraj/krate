//! Noticing when the files in a folder change (`krate:fs/watch`, the
//! capability roadmap's `fs.watch`).
//!
//! A watch is a snapshot of what listing the folder showed -- every entry's
//! path, size and modified time -- and `changes` is the difference between
//! that snapshot and a fresh look. Looking, not an OS notification feed:
//! the same on every system, nothing to fall behind or overflow, no thread,
//! and the fresh look goes through the same path rules and the same grant
//! check as `files.list`, so a watch can never see more than listing could.
//!
//! The cost of looking is bounded two ways: a tree over `MAX_ENTRIES` is
//! refused, and a look sooner than `MIN_INTERVAL` after the last returns
//! nothing, so an app that asks every frame walks its folder four times a
//! second, not sixty.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::uapi_dispatch::ScanEntry;

/// Watches one app may hold at once.
pub const MAX_WATCHES: usize = 8;
/// Entries one watch may cover, folders included.
pub const MAX_ENTRIES: usize = 20_000;
/// The shortest time between two looks at one folder.
pub const MIN_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Created,
    Modified,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
    pub is_dir: bool,
}

/// What one entry looked like: folder or not, size, modified time.
type Seen = (bool, u64, u64);

struct Watch {
    path: String,
    seen: BTreeMap<String, Seen>,
    looked: Instant,
}

#[derive(Default)]
pub struct Watches {
    next: u64,
    live: BTreeMap<u64, Watch>,
}

pub enum Due<'a> {
    /// Look now, at this path.
    Now(&'a str),
    /// Looked too recently; nothing new is reported.
    Later,
    /// No such watch.
    Unknown,
}

impl Watches {
    pub fn is_full(&self) -> bool {
        self.live.len() >= MAX_WATCHES
    }

    pub fn start(&mut self, path: &str, entries: Vec<ScanEntry>) -> u64 {
        self.next += 1;
        self.live.insert(
            self.next,
            Watch {
                path: path.trim_end_matches('/').to_string(),
                seen: snapshot(entries),
                looked: Instant::now(),
            },
        );
        self.next
    }

    pub fn due(&self, id: u64) -> Due<'_> {
        match self.live.get(&id) {
            None => Due::Unknown,
            Some(watch) if watch.looked.elapsed() < MIN_INTERVAL => Due::Later,
            Some(watch) => Due::Now(&watch.path),
        }
    }

    /// Take a fresh look's entries, report what differs, and keep the look.
    pub fn apply(&mut self, id: u64, entries: Vec<ScanEntry>) -> Vec<Change> {
        let Some(watch) = self.live.get_mut(&id) else {
            return Vec::new();
        };
        let now = snapshot(entries);
        let changes = diff(&watch.path, &watch.seen, &now);
        watch.seen = now;
        watch.looked = Instant::now();
        changes
    }

    pub fn stop(&mut self, id: u64) {
        self.live.remove(&id);
    }
}

fn snapshot(entries: Vec<ScanEntry>) -> BTreeMap<String, Seen> {
    entries
        .into_iter()
        .map(|e| (e.path, (e.is_dir, e.size, e.modified_millis)))
        .collect()
}

/// Every path that appeared, went away, or changed size or time, sorted by
/// path. A folder's own size is whatever the system says and moves when
/// its contents do, so a folder is only ever created or removed -- the
/// files inside it carry the modifications.
fn diff(
    root: &str,
    before: &BTreeMap<String, Seen>,
    after: &BTreeMap<String, Seen>,
) -> Vec<Change> {
    let join = |rel: &str| {
        if root.is_empty() || root == "." {
            rel.to_string()
        } else {
            format!("{root}/{rel}")
        }
    };
    let mut changes = Vec::new();
    for (rel, now) in after {
        let kind = match before.get(rel) {
            None => Some(ChangeKind::Created),
            Some(was) if was.0 != now.0 => Some(ChangeKind::Created),
            Some(was) if !now.0 && was != now => Some(ChangeKind::Modified),
            Some(_) => None,
        };
        if let Some(kind) = kind {
            changes.push(Change {
                path: join(rel),
                kind,
                is_dir: now.0,
            });
        }
    }
    for (rel, was) in before {
        let gone = match after.get(rel) {
            None => true,
            // A file replaced by a folder of the same name, or the reverse:
            // the old one went away and the new one was created above.
            Some(now) => now.0 != was.0,
        };
        if gone {
            changes.push(Change {
                path: join(rel),
                kind: ChangeKind::Removed,
                is_dir: was.0,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool, size: u64, modified: u64) -> ScanEntry {
        ScanEntry {
            path: path.to_string(),
            is_dir,
            size,
            modified_millis: modified,
        }
    }

    #[test]
    fn a_look_reports_what_appeared_changed_and_went_away() {
        let mut watches = Watches::default();
        let id = watches.start(
            "picked/t1/photos/",
            vec![
                entry("a.jpg", false, 10, 1),
                entry("b.jpg", false, 20, 1),
                entry("old", true, 0, 1),
                entry("old/c.jpg", false, 5, 1),
            ],
        );
        let changes = watches.apply(
            id,
            vec![
                entry("a.jpg", false, 10, 1),
                entry("b.jpg", false, 21, 2),
                entry("new.jpg", false, 7, 3),
                entry("old", true, 99, 4),
            ],
        );
        let seen: Vec<(&str, ChangeKind, bool)> = changes
            .iter()
            .map(|c| (c.path.as_str(), c.kind, c.is_dir))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("picked/t1/photos/b.jpg", ChangeKind::Modified, false),
                ("picked/t1/photos/new.jpg", ChangeKind::Created, false),
                ("picked/t1/photos/old/c.jpg", ChangeKind::Removed, false),
            ],
            "a folder whose own size moved is not reported; its contents are"
        );
        assert!(
            watches
                .apply(
                    id,
                    vec![
                        entry("a.jpg", false, 10, 1),
                        entry("b.jpg", false, 21, 2),
                        entry("new.jpg", false, 7, 3),
                        entry("old", true, 99, 4),
                    ]
                )
                .is_empty(),
            "nothing changed, nothing reported"
        );
    }

    #[test]
    fn a_file_replaced_by_a_folder_is_a_removal_and_a_creation() {
        let mut watches = Watches::default();
        let id = watches.start("notes", vec![entry("x", false, 1, 1)]);
        let changes = watches.apply(id, vec![entry("x", true, 0, 2)]);
        assert_eq!(changes.len(), 2);
        assert!(changes
            .iter()
            .any(|c| c.kind == ChangeKind::Created && c.is_dir));
        assert!(changes
            .iter()
            .any(|c| c.kind == ChangeKind::Removed && !c.is_dir));
    }

    #[test]
    fn looking_again_too_soon_is_not_a_look_and_the_ninth_watch_is_refused() {
        let mut watches = Watches::default();
        let id = watches.start("a", Vec::new());
        assert!(matches!(watches.due(id), Due::Later));
        assert!(matches!(watches.due(id + 100), Due::Unknown));
        for _ in 1..MAX_WATCHES {
            assert!(!watches.is_full());
            watches.start("a", Vec::new());
        }
        assert!(watches.is_full());
        watches.stop(id);
        assert!(!watches.is_full(), "stopping one frees its place");
    }
}
