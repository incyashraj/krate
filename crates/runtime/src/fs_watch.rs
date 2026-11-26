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
//!
//! And a look happens only when something may have changed: on a computer
//! the system's own change notice (FSEvents, inotify, ReadDirectoryChangesW)
//! sets a flag, and with the flag down a watch reports nothing without
//! walking the folder at all -- a quiet 10,000-note vault costs nothing to
//! watch. The notice decides WHEN to look, never WHAT changed: the look is
//! still the scan above, so nothing is lost to a notice that overflowed or
//! never came, and every `SAFETY_LOOK` a scan happens anyway.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::uapi_dispatch::ScanEntry;

/// Watches one app may hold at once.
pub const MAX_WATCHES: usize = 8;
/// Entries one watch may cover, folders included.
pub const MAX_ENTRIES: usize = 20_000;
/// The shortest time between two looks at one folder.
pub const MIN_INTERVAL: Duration = Duration::from_millis(250);
/// The longest a watch with a change notice goes without a look: a network
/// folder or a missed notice is still found, just later. A big folder
/// waits longer (`SAFETY_SHARE`).
pub const SAFETY_LOOK: Duration = Duration::from_secs(10);
/// How much of the time the safety look may spend looking, at most: one
/// part in this many. Walking a 10,000-note vault every ten seconds cost
/// about 150 ms a time -- 1.5% of a core spent idle on a look that almost
/// never finds anything (K-1373) -- so a look that took 150 ms waits 30 s
/// for the next, and a small folder still gets one every `SAFETY_LOOK`.
pub const SAFETY_SHARE: u32 = 200;
/// The longest any safety look waits, however big the folder.
pub const SAFETY_LOOK_MAX: Duration = Duration::from_secs(300);

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

/// The files this process changed lately (a write, a remove, a rename, a
/// new folder, a trash), numbered. The system's notice can arrive a beat
/// after the change -- FSEvents does -- so a look straight after the app's
/// own write found nothing and reported it one look late (K-1384). A watch
/// looks now when one of these landed inside its folder since its last look.
static HOST_WRITES: std::sync::Mutex<(u64, std::collections::VecDeque<(u64, std::path::PathBuf)>)> =
    std::sync::Mutex::new((0, std::collections::VecDeque::new()));

/// How many recent changes are kept; a watch further behind than this just
/// looks, which costs one scan and loses nothing.
const HOST_WRITES_KEPT: usize = 64;

/// Note that this process changed `path`, for the next look of any watch
/// over it.
pub(crate) fn host_wrote(path: &std::path::Path) {
    if let Ok(mut log) = HOST_WRITES.lock() {
        log.0 += 1;
        let n = log.0;
        if log.1.len() == HOST_WRITES_KEPT {
            log.1.pop_front();
        }
        log.1.push_back((n, path.to_path_buf()));
    }
}

/// The latest change's number, and whether one after `seen` landed in
/// `folder` (or the log no longer reaches back that far).
fn host_wrote_in(folder: Option<&std::path::Path>, seen: u64) -> (u64, bool) {
    let Ok(log) = HOST_WRITES.lock() else {
        return (seen, false);
    };
    let Some(folder) = folder else {
        return (log.0, false);
    };
    let lost = log.1.front().is_some_and(|(n, _)| *n > seen + 1);
    let inside = log
        .1
        .iter()
        .any(|(n, path)| *n > seen && path.starts_with(folder));
    (log.0, lost || inside)
}

/// What one entry looked like: folder or not, size, modified time.
type Seen = (bool, u64, u64);

struct Watch {
    path: String,
    seen: BTreeMap<String, Seen>,
    looked: Instant,
    /// Raised by the system's change notice; `None` where there is none,
    /// and then every look scans.
    changed: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The folder on this computer, for this process's own changes.
    host: Option<std::path::PathBuf>,
    /// The last of this process's changes the last look came after.
    writes_seen: u64,
    /// When the look being made now began, to time it.
    look_began: Option<Instant>,
    /// How long a quiet watch goes between safety looks: `SAFETY_LOOK`,
    /// or `SAFETY_SHARE` times what the last look took.
    safety: Duration,
    /// Kept so the notice keeps coming; dropped with the watch.
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    _notice: Option<notify::RecommendedWatcher>,
}

/// The system's change notice for `folder`, raising the returned flag on
/// any change below it; `None` where the system will not watch it.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn notice_for(
    folder: &std::path::Path,
) -> Option<(
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    notify::RecommendedWatcher,
)> {
    use notify::Watcher;
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let raise = flag.clone();
    let mut watcher = notify::recommended_watcher(move |_event: notify::Result<notify::Event>| {
        // Any event -- or an error, which may mean events were lost -- is a
        // reason to look.
        raise.store(true, std::sync::atomic::Ordering::SeqCst);
    })
    .ok()?;
    watcher
        .watch(folder, notify::RecursiveMode::Recursive)
        .ok()?;
    Some((flag, watcher))
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

    /// Start a watch on `path`, whose first look found `entries`.
    /// `host` is the folder on this computer behind it, for the system's
    /// change notice; `None` scans on every look as before.
    pub fn start(
        &mut self,
        path: &str,
        entries: Vec<ScanEntry>,
        host: Option<&std::path::Path>,
    ) -> u64 {
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        let (changed, notice) = match host.and_then(notice_for) {
            Some((flag, watcher)) => (Some(flag), Some(watcher)),
            None => (None, None),
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        let changed = {
            let _ = host;
            None
        };
        self.next += 1;
        self.live.insert(
            self.next,
            Watch {
                path: path.trim_end_matches('/').to_string(),
                seen: snapshot(entries),
                looked: Instant::now(),
                changed,
                writes_seen: host_wrote_in(None, 0).0,
                host: host.map(std::path::Path::to_path_buf),
                look_began: None,
                safety: SAFETY_LOOK,
                #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
                _notice: notice,
            },
        );
        self.next
    }

    /// Whether to look now. With a change notice and nothing raised, not
    /// until `SAFETY_LOOK`; a look that is due lowers the flag first, so a
    /// change during the scan raises it again for the next.
    pub fn due(&mut self, id: u64) -> Due<'_> {
        match self.live.get_mut(&id) {
            None => Due::Unknown,
            Some(watch) if watch.looked.elapsed() < MIN_INTERVAL => Due::Later,
            Some(watch) => {
                let (writes, wrote) = host_wrote_in(watch.host.as_deref(), watch.writes_seen);
                if let Some(flag) = &watch.changed {
                    let raised = flag.swap(false, std::sync::atomic::Ordering::SeqCst);
                    if !raised && !wrote && watch.looked.elapsed() < watch.safety {
                        return Due::Later;
                    }
                }
                watch.writes_seen = writes;
                watch.look_began = Some(Instant::now());
                Due::Now(&watch.path)
            }
        }
    }

    /// Whether this watch has the system's change notice (for tests).
    #[cfg(test)]
    pub fn has_notice(&self, id: u64) -> bool {
        self.live.get(&id).is_some_and(|w| w.changed.is_some())
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
        if let Some(began) = watch.look_began.take() {
            watch.safety = safety_after(began.elapsed());
        }
        changes
    }

    pub fn stop(&mut self, id: u64) {
        self.live.remove(&id);
    }
}

/// The wait before the next safety look, after a look that took `cost`.
fn safety_after(cost: Duration) -> Duration {
    cost.saturating_mul(SAFETY_SHARE)
        .clamp(SAFETY_LOOK, SAFETY_LOOK_MAX)
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
            None,
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
        let id = watches.start("notes", vec![entry("x", false, 1, 1)], None);
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
        let id = watches.start("a", Vec::new(), None);
        assert!(matches!(watches.due(id), Due::Later));
        assert!(matches!(watches.due(id + 100), Due::Unknown));
        for _ in 1..MAX_WATCHES {
            assert!(!watches.is_full());
            watches.start("a", Vec::new(), None);
        }
        assert!(watches.is_full());
        watches.stop(id);
        assert!(!watches.is_full(), "stopping one frees its place");
    }

    /// With the system's change notice, a quiet folder is not walked at
    /// all, and a file written into it makes the next look due.
    /// K-1373: a look that is slow spaces the safety looks out, so they
    /// cost at most one part in `SAFETY_SHARE`; a quick one keeps them at
    /// `SAFETY_LOOK`, and none waits past `SAFETY_LOOK_MAX`.
    #[test]
    fn a_slow_look_spaces_the_safety_looks_out() {
        assert_eq!(safety_after(Duration::from_millis(1)), SAFETY_LOOK);
        assert_eq!(
            safety_after(Duration::from_millis(150)),
            Duration::from_secs(30)
        );
        assert_eq!(safety_after(Duration::from_secs(10)), SAFETY_LOOK_MAX);
    }

    /// K-1384: this process's own change inside the folder makes the next
    /// look due before any system notice comes; one elsewhere does not.
    #[test]
    fn the_apps_own_change_makes_a_look_due_at_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let mut watches = Watches::default();
        let id = watches.start("vault", Vec::new(), Some(dir.path()));
        // A watch with no system notice scans every look anyway; this is
        // about the one that has it, waiting on a notice not yet come.
        if !watches.has_notice(id) {
            return;
        }
        std::thread::sleep(Duration::from_millis(400));
        let _ = matches!(watches.due(id), Due::Now(_));
        std::thread::sleep(MIN_INTERVAL + Duration::from_millis(50));
        let _ = matches!(watches.due(id), Due::Now(_));
        std::thread::sleep(MIN_INTERVAL + Duration::from_millis(50));
        // No file is touched, so no system notice can come: only the note.
        host_wrote(&elsewhere.path().join("other.md"));
        assert!(
            matches!(watches.due(id), Due::Later),
            "a change outside the folder is not a reason to look"
        );
        std::thread::sleep(MIN_INTERVAL + Duration::from_millis(50));
        host_wrote(&dir.path().join("note.md"));
        assert!(
            matches!(watches.due(id), Due::Now(_)),
            "the app's own change inside the folder is"
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    #[test]
    fn a_quiet_folder_is_not_walked_and_a_change_makes_a_look_due() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut watches = Watches::default();
        let id = watches.start("vault", Vec::new(), Some(dir.path()));
        assert!(watches.has_notice(id), "the system watches the folder");
        std::thread::sleep(MIN_INTERVAL + Duration::from_millis(50));
        // Notices from creating the folder may still arrive: drain them.
        std::thread::sleep(Duration::from_millis(400));
        let _ = matches!(watches.due(id), Due::Now(_));
        std::thread::sleep(MIN_INTERVAL + Duration::from_millis(50));
        assert!(
            matches!(watches.due(id), Due::Later),
            "nothing changed: no look"
        );
        std::fs::write(dir.path().join("new.md"), b"hi").unwrap();
        let mut due = false;
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(100));
            if matches!(watches.due(id), Due::Now(_)) {
                due = true;
                break;
            }
        }
        assert!(due, "a written file makes a look due");
    }
}
