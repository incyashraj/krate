//! File recovery: a copy of each note as it was, taken while it is being
//! edited (at most one every few minutes), kept for some days, to look back
//! through and bring back. Like the notes app, the copies live outside the
//! vault (in the app's own store), never as files among the notes.

use crate::json::{self, Value};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Milliseconds since 1970.
    pub at: u64,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Oldest first, per note path.
    pub files: BTreeMap<String, Vec<Snapshot>>,
    /// Paths changed since the store last saw them.
    pub dirty: Vec<String>,
}

const DAY_MS: u64 = 86_400_000;

impl Recovery {
    /// Keep a copy of `text` if the last copy is older than `interval_ms`
    /// and says something else. True when a copy was taken.
    pub fn note(&mut self, path: &str, text: &str, now: u64, interval_ms: u64) -> bool {
        let list = self.files.entry(String::from(path)).or_default();
        if let Some(last) = list.last() {
            if last.text == text || now.saturating_sub(last.at) < interval_ms {
                return false;
            }
        }
        list.push(Snapshot {
            at: now,
            text: String::from(text),
        });
        if !self.dirty.iter().any(|p| p == path) {
            self.dirty.push(String::from(path));
        }
        true
    }

    /// Drop copies older than `keep_days`.
    pub fn prune(&mut self, now: u64, keep_days: u64) {
        let cutoff = now.saturating_sub(keep_days * DAY_MS);
        let mut changed = Vec::new();
        for (p, list) in self.files.iter_mut() {
            let before = list.len();
            list.retain(|s| s.at >= cutoff);
            if list.len() != before {
                changed.push(p.clone());
            }
        }
        self.files.retain(|_, l| !l.is_empty());
        for p in changed {
            if !self.dirty.contains(&p) {
                self.dirty.push(p);
            }
        }
    }

    pub fn snapshots(&self, path: &str) -> &[Snapshot] {
        self.files.get(path).map_or(&[], Vec::as_slice)
    }

    /// A note moved: its copies follow it.
    pub fn moved(&mut self, from: &str, to: &str) {
        if let Some(list) = self.files.remove(from) {
            self.files.insert(String::from(to), list);
            self.dirty.push(String::from(from));
            self.dirty.push(String::from(to));
        }
    }

    /// One note's copies as stored text.
    pub fn to_text(&self, path: &str) -> String {
        let list = self
            .snapshots(path)
            .iter()
            .map(|s| {
                let mut o = Value::obj();
                o.set("ts", Value::Num(s.at as f64));
                o.set("data", Value::s(&s.text));
                o
            })
            .collect();
        json::compact(&Value::Arr(list))
    }

    /// Take in one note's stored copies.
    pub fn load(&mut self, path: &str, text: &str) {
        let Some(v) = json::parse(text) else {
            return;
        };
        let list: Vec<Snapshot> = v
            .arr()
            .iter()
            .filter_map(|o| {
                Some(Snapshot {
                    at: o.get_num("ts")? as u64,
                    text: String::from(o.get_str("data")?),
                })
            })
            .collect();
        if !list.is_empty() {
            self.files.insert(String::from(path), list);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_at_an_interval_kept_for_days() {
        let mut r = Recovery::default();
        let min = 60_000;
        assert!(r.note("a.md", "one", 0, 5 * min));
        assert!(!r.note("a.md", "two", 2 * min, 5 * min), "too soon");
        assert!(r.note("a.md", "two", 6 * min, 5 * min));
        assert!(!r.note("a.md", "two", 20 * min, 5 * min), "nothing new");
        assert_eq!(r.snapshots("a.md").len(), 2);
        let stored = r.to_text("a.md");
        let mut back = Recovery::default();
        back.load("a.md", &stored);
        assert_eq!(back.snapshots("a.md"), r.snapshots("a.md"));
        r.moved("a.md", "b.md");
        assert_eq!(r.snapshots("b.md").len(), 2);
        r.prune(7 * DAY_MS + 3 * min, 7);
        assert_eq!(
            r.snapshots("b.md").len(),
            1,
            "the first copy is over a week old"
        );
        assert_eq!(r.snapshots("b.md")[0].text, "two");
    }
}
