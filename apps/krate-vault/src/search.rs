//! Searching the whole vault.
//!
//! A search is a job, not a call: it reads the notes a few at a time
//! between frames (`step`), so results appear as they are found and the
//! window never stops answering, however big the vault is. A note open in a
//! tab is searched as it is now, unsaved edits included.
//!
//! The query reads like the one people already know: words must all appear
//! in a note (any order, any case), `"quoted words"` must appear together,
//! `tag:#name` matches a tag, `path:` and `file:` match where a note lives
//! and what it is called, and a leading `-` excludes a word.

use crate::text::{excerpt, find_ci, lower, match_len};
use crate::vault::Vault;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub terms: Vec<String>,
    pub not: Vec<String>,
    pub tags: Vec<String>,
    pub paths: Vec<String>,
    pub files: Vec<String>,
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.tags.is_empty()
            && self.paths.is_empty()
            && self.files.is_empty()
    }
}

/// Read a query.
pub fn parse(q: &str) -> Query {
    let mut out = Query::default();
    let mut rest = q.trim();
    while !rest.is_empty() {
        let (word, after) = if let Some(r) = rest.strip_prefix('"') {
            match r.find('"') {
                Some(end) => (r.get(..end).unwrap_or(""), r.get(end + 1..).unwrap_or("")),
                None => (r, ""),
            }
        } else {
            match rest.find(char::is_whitespace) {
                Some(end) => (rest.get(..end).unwrap_or(""), rest.get(end..).unwrap_or("")),
                None => (rest, ""),
            }
        };
        rest = after.trim_start();
        if word.is_empty() {
            continue;
        }
        let w = lower(word);
        if let Some(t) = w.strip_prefix("tag:") {
            let t = t.trim_start_matches('#');
            if !t.is_empty() {
                out.tags.push(String::from(t));
            }
        } else if let Some(p) = w.strip_prefix("path:") {
            if !p.is_empty() {
                out.paths.push(String::from(p.trim_matches('"')));
            }
        } else if let Some(f) = w.strip_prefix("file:") {
            if !f.is_empty() {
                out.files.push(String::from(f.trim_matches('"')));
            }
        } else if let Some(n) = w.strip_prefix('-') {
            if !n.is_empty() {
                out.not.push(String::from(n));
            }
        } else {
            out.terms.push(w);
        }
    }
    out
}

/// One matching line, cut to fit a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineHit {
    pub line: u32,
    pub text: String,
    /// Where the match is inside `text`.
    pub at: (usize, usize),
    /// Where the match is in the real line (for jumping to it).
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHits {
    pub path: String,
    pub name: String,
    pub lines: Vec<LineHit>,
    /// Every match in the note, including the lines not kept.
    pub count: usize,
}

/// How many matching lines a result keeps to show.
pub const LINES_KEPT: usize = 12;
/// How many notes' results are kept to show (all are counted).
pub const FILES_KEPT: usize = 2_000;
const EXCERPT: usize = 90;

/// Whether `tags` holds the tag `want` or one below it (`work` matches
/// `work/alpha`).
fn has_tag(text_tags: &[String], want: &str) -> bool {
    text_tags.iter().any(|t| {
        let t = lower(t);
        t == want || t.strip_prefix(want).is_some_and(|r| r.starts_with('/'))
    })
}

/// Search one note's text. `None` when it does not match.
pub fn match_note(q: &Query, path: &str, name: &str, text: &str) -> Option<FileHits> {
    let lpath = lower(path);
    if !q.paths.iter().all(|p| lpath.contains(p.as_str())) {
        return None;
    }
    let lname = lower(name);
    if !q.files.iter().all(|f| lname.contains(f.as_str())) {
        return None;
    }
    if !q.tags.is_empty() {
        let meta = crate::vault::scan_text(text);
        if !q.tags.iter().all(|t| has_tag(&meta.tags, t)) {
            return None;
        }
    }
    // Every word somewhere in the note (or its name); no excluded word.
    for t in &q.terms {
        if find_ci(text, t, 0).is_none() && !lname.contains(t.as_str()) {
            return None;
        }
    }
    for n in &q.not {
        if find_ci(text, n, 0).is_some() {
            return None;
        }
    }
    let mut hits = FileHits {
        path: String::from(path),
        name: String::from(name),
        lines: Vec::new(),
        count: 0,
    };
    let looking: Vec<String> = if q.terms.is_empty() {
        q.tags
            .iter()
            .map(|t| {
                let mut s = String::from("#");
                s.push_str(t);
                s
            })
            .collect()
    } else {
        q.terms.clone()
    };
    for (n, line) in text.split('\n').enumerate() {
        let mut first: Option<(usize, usize)> = None;
        for t in &looking {
            let mut from = 0;
            while let Some(at) = find_ci(line, t, from) {
                let len = match_len(line, at, t).max(1);
                hits.count += 1;
                if first.is_none_or(|(a, _)| at < a) {
                    first = Some((at, at + len));
                }
                from = at + len;
            }
        }
        if let Some(hit) = first {
            if hits.lines.len() < LINES_KEPT {
                let (text, at) = excerpt(line, hit, EXCERPT);
                hits.lines.push(LineHit {
                    line: n as u32,
                    text,
                    at,
                    col: hit.0,
                });
            }
        }
    }
    Some(hits)
}

/// A search going on.
pub struct Job {
    pub query: String,
    q: Query,
    /// Notes left to look at, as paths, last first.
    todo: Vec<String>,
    pub total: usize,
    pub results: Vec<FileHits>,
    pub notes_matched: usize,
    pub matches: usize,
    pub done: bool,
    /// The vault changed under the search; it should be run again.
    pub stale_at: u64,
}

impl Job {
    pub fn new(query: &str, vault: &Vault) -> Job {
        let q = parse(query);
        let todo: Vec<String> = if q.is_empty() {
            Vec::new()
        } else {
            vault.notes.iter().rev().map(|n| n.path.clone()).collect()
        };
        Job {
            query: String::from(query),
            total: todo.len(),
            done: todo.is_empty(),
            q,
            todo,
            results: Vec::new(),
            notes_matched: 0,
            matches: 0,
            stale_at: vault.generation,
        }
    }

    pub fn scanned(&self) -> usize {
        self.total - self.todo.len()
    }

    /// Look at notes until `more` says to stop. `open` gives the current
    /// text of a note open in a tab. True if anything was found.
    pub fn step(
        &mut self,
        vault: &mut Vault,
        open: &dyn Fn(&str) -> Option<String>,
        more: &mut dyn FnMut() -> bool,
    ) -> bool {
        let mut found = false;
        while more() {
            let Some(path) = self.todo.pop() else {
                self.done = true;
                break;
            };
            let Some(name) = vault.note(&path).map(|n| n.name.clone()) else {
                continue;
            };
            let text = match open(&path) {
                Some(t) => t,
                None => match vault.fs.read(&path) {
                    Ok(t) => t,
                    Err(_) => continue,
                },
            };
            if let Some(h) = match_note(&self.q, &path, &name, &text) {
                self.notes_matched += 1;
                self.matches += h.count.max(1);
                if self.results.len() < FILES_KEPT {
                    self.results.push(h);
                }
                found = true;
            }
        }
        if self.todo.is_empty() {
            self.done = true;
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemFs;
    use alloc::boxed::Box;

    #[test]
    fn queries_are_read() {
        let q = parse(r#"plan "next week" tag:#work -draft path:Projects file:notes"#);
        assert_eq!(q.terms, ["plan", "next week"]);
        assert_eq!(q.tags, ["work"]);
        assert_eq!(q.not, ["draft"]);
        assert_eq!(q.paths, ["projects"]);
        assert_eq!(q.files, ["notes"]);
        assert!(parse("   ").is_empty());
    }

    #[test]
    fn a_note_matches_when_every_word_is_in_it() {
        let text = "# Garden\nPlant the tomatoes next week.\nWater daily.\nTomatoes need sun";
        let q = parse("tomatoes water");
        let hits = match_note(&q, "Garden.md", "Garden", text).unwrap();
        assert_eq!(hits.count, 3);
        let lines: Vec<u32> = hits.lines.iter().map(|l| l.line).collect();
        assert_eq!(lines, [1, 2, 3]);
        let l = &hits.lines[0];
        assert_eq!(l.text.get(l.at.0..l.at.1), Some("tomatoes"));
        assert!(match_note(&parse("tomatoes cucumber"), "Garden.md", "Garden", text).is_none());
        assert!(match_note(&parse("water -sun"), "Garden.md", "Garden", text).is_none());
        // The name counts as part of the note.
        assert!(match_note(&parse("garden"), "x/Garden.md", "Garden", "nothing").is_some());
    }

    #[test]
    fn tags_match_their_children() {
        let text = "Ship it #project/alpha";
        assert!(match_note(&parse("tag:project"), "a.md", "a", text).is_some());
        assert!(match_note(&parse("tag:#project/alpha"), "a.md", "a", text).is_some());
        assert!(match_note(&parse("tag:proj"), "a.md", "a", text).is_none());
    }

    #[test]
    fn a_job_runs_in_steps_and_sees_unsaved_text() {
        let mut fs = MemFs::new();
        for i in 0..30 {
            let mut p = String::from("n");
            crate::text::push_num(&mut p, i);
            p.push_str(".md");
            fs.put(
                &p,
                if i % 3 == 0 {
                    "has needle here"
                } else {
                    "plain"
                },
            );
        }
        let mut v = Vault::open("v", Box::new(fs));
        let mut job = Job::new("needle", &v);
        assert_eq!(job.total, 30);
        let open = |p: &str| (p == "n1.md").then(|| String::from("needle typed but not saved"));
        let mut budget = 5;
        job.step(&mut v, &open, &mut || {
            budget -= 1;
            budget >= 0
        });
        assert_eq!(job.scanned(), 5);
        assert!(!job.done);
        job.step(&mut v, &open, &mut || true);
        assert!(job.done);
        assert_eq!(job.notes_matched, 11);
        assert!(job.results.iter().any(|r| r.path == "n1.md"));
    }
}
