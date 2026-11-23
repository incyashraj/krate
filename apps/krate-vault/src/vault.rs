//! A vault: one folder of Markdown notes, the folders inside it, and what
//! the notes say about each other.
//!
//! Opening a vault lists its folders and nothing more -- every note's name
//! and place, which the file tree, the quick switcher and link completion
//! need at once. What is inside the notes (their links and tags, for
//! backlinks and the tag pane) is read afterwards by [`Vault::index_some`],
//! a few files at a time between frames, so a 10,000-note vault shows its
//! tree at once and fills its backlinks in behind it.

use crate::md;
use crate::text::lower;
use crate::vfs::{file_name, join, parent, Vfs};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// One note.
#[derive(Debug, Clone)]
pub struct Note {
    /// From the vault's root, with `.md`.
    pub path: String,
    /// The file name without `.md`: what a `[[link]]` names.
    pub name: String,
    /// `name`, lower-cased, for matching links.
    pub key: String,
    pub folder: usize,
}

/// One folder; folder 0 is the vault's root.
#[derive(Debug, Clone)]
pub struct Folder {
    pub path: String,
    pub name: String,
    pub parent: Option<usize>,
    pub folders: Vec<usize>,
    pub notes: Vec<usize>,
}

/// What one note says, read by the indexer.
#[derive(Debug, Clone, Default)]
pub struct Meta {
    /// Each `[[link]]`: the target lower-cased (a name, or a path without
    /// `.md`), and the line it is on.
    pub links: Vec<(String, u32)>,
    /// Each `#tag`, as written (without the `#`), with the note's `tags`
    /// property.
    pub tags: Vec<String>,
    /// The note's properties (its `---` block), as a JSON object.
    pub props: Option<crate::json::Value>,
    /// The file's size in bytes and when it last changed (Unix ms), when
    /// the folder can tell.
    pub stat: Option<(u64, u64)>,
}

/// A note that links to another, and the lines it does it on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlink {
    pub path: String,
    pub lines: Vec<u32>,
    /// The text of those lines, when the app has read them.
    pub context: Vec<String>,
}

pub struct Vault {
    pub name: String,
    pub fs: Box<dyn Vfs>,
    pub notes: Vec<Note>,
    pub folders: Vec<Folder>,
    by_key: BTreeMap<String, Vec<usize>>,
    by_path: BTreeMap<String, usize>,
    folder_by_path: BTreeMap<String, usize>,
    pub meta: BTreeMap<String, Meta>,
    queue: Vec<String>,
    /// Bumped whenever the index or the set of notes changes, so a pane
    /// can tell its cached answer is stale.
    pub generation: u64,
    /// Folders that could not be read, and why.
    pub problems: Vec<String>,
    /// List every file, not only notes and attachments ("Detect all file
    /// extensions").
    pub show_all: bool,
}

/// File names a vault never shows: its own settings and the trash.
fn hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// A Markdown note.
fn is_md(name: &str) -> bool {
    name.len() > 3 && name.to_ascii_lowercase().ends_with(".md")
}

/// A file the vault shows and opens: notes, canvases and bases.
pub fn is_note(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    is_md(name) || (l.len() > 7 && l.ends_with(".canvas")) || (l.len() > 5 && l.ends_with(".base"))
}

/// The other files the notes app lists by default: pictures, sound,
/// video and PDF.
pub const ATTACHMENTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "svg", "webp", "avif", "mp3", "wav", "m4a", "ogg", "3gp",
    "flac", "webm", "mp4", "ogv", "mov", "mkv", "pdf",
];

/// A file's extension, lower-cased (`""` when it has none).
pub fn ext_of(name: &str) -> String {
    let n = file_name(name);
    match n.rfind('.') {
        Some(i) if i > 0 => n[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// An attachment the notes app lists without "Detect all file extensions".
pub fn is_attachment(name: &str) -> bool {
    let e = ext_of(name);
    ATTACHMENTS.contains(&e.as_str())
}

/// What kind of file a path is, by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Note,
    Canvas,
    Base,
    Image,
    Pdf,
    Media,
    /// Any other file (listed when "Detect all file extensions" is on).
    Other,
}

pub fn kind_of(path: &str) -> FileKind {
    let l = path.to_ascii_lowercase();
    if l.ends_with(".canvas") {
        FileKind::Canvas
    } else if l.ends_with(".base") {
        FileKind::Base
    } else if l.ends_with(".md") {
        FileKind::Note
    } else {
        match ext_of(&l).as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" | "avif" => FileKind::Image,
            "pdf" => FileKind::Pdf,
            "mp3" | "wav" | "m4a" | "ogg" | "3gp" | "flac" | "webm" | "mp4" | "ogv" | "mov"
            | "mkv" => FileKind::Media,
            _ => FileKind::Other,
        }
    }
}

/// Whether the indexer reads this file (notes and canvases).
fn indexed_kind(path: &str) -> bool {
    matches!(kind_of(path), FileKind::Note | FileKind::Canvas)
}

/// The name shown for a file: without its extension (the tree shows the
/// extension as a tag).
fn strip_md(name: &str) -> &str {
    if is_note(name) || name.rfind('.').is_some_and(|i| i > 0) {
        let dot = name.rfind('.').unwrap_or(name.len());
        name.get(..dot).unwrap_or(name)
    } else {
        name
    }
}

/// The key a link names a file by: a note's name, or another file's name
/// with its extension (`[[Board.canvas]]`).
fn link_key(file: &str) -> String {
    if is_md(file) {
        lower(strip_md(file))
    } else {
        lower(file)
    }
}

/// Sort the way a file tree does: case-insensitive, then exact.
fn name_order(a: &str, b: &str) -> core::cmp::Ordering {
    lower(a).cmp(&lower(b)).then_with(|| a.cmp(b))
}

impl Vault {
    /// Open the folder `fs` reaches: list every folder in it, once.
    pub fn open(name: &str, fs: Box<dyn Vfs>) -> Vault {
        let mut v = Vault {
            name: String::from(name),
            fs,
            notes: Vec::new(),
            folders: Vec::new(),
            by_key: BTreeMap::new(),
            by_path: BTreeMap::new(),
            folder_by_path: BTreeMap::new(),
            meta: BTreeMap::new(),
            queue: Vec::new(),
            generation: 1,
            problems: Vec::new(),
            show_all: false,
        };
        v.scan();
        v.queue = v
            .notes
            .iter()
            .rev()
            .filter(|n| indexed_kind(&n.path))
            .map(|n| n.path.clone())
            .collect();
        v
    }

    /// List every folder from the root down, and rebuild the tree.
    pub fn scan(&mut self) {
        let mut note_paths: Vec<String> = Vec::new();
        let mut folder_paths: Vec<String> = Vec::new();
        let mut pending: Vec<String> = alloc::vec![String::new()];
        self.problems.clear();
        while let Some(dir) = pending.pop() {
            match self.fs.list(&dir) {
                Ok(entries) => {
                    for e in entries {
                        if hidden(&e.name) {
                            continue;
                        }
                        let p = join(&dir, &e.name);
                        if e.is_dir {
                            folder_paths.push(p.clone());
                            pending.push(p);
                        } else if is_note(&e.name) || is_attachment(&e.name) || self.show_all {
                            note_paths.push(p);
                        }
                    }
                }
                Err(why) => {
                    let mut msg = if dir.is_empty() {
                        String::from("the vault folder")
                    } else {
                        dir.clone()
                    };
                    msg.push_str(": ");
                    msg.push_str(&why);
                    self.problems.push(msg);
                }
            }
        }
        self.build(folder_paths, note_paths);
    }

    /// Make the tree and the name maps from a set of paths.
    fn build(&mut self, mut folder_paths: Vec<String>, mut note_paths: Vec<String>) {
        folder_paths.sort_by(|a, b| name_order(a, b));
        note_paths.sort_by(|a, b| name_order(a, b));
        self.folders.clear();
        self.notes.clear();
        self.by_key.clear();
        self.by_path.clear();
        self.folder_by_path.clear();
        self.folders.push(Folder {
            path: String::new(),
            name: self.name.clone(),
            parent: None,
            folders: Vec::new(),
            notes: Vec::new(),
        });
        self.folder_by_path.insert(String::new(), 0);
        // Parents sort before their children, so each parent exists first.
        folder_paths.sort_by_key(|p| p.matches('/').count());
        for p in folder_paths {
            let up = self.folder_by_path.get(parent(&p)).copied().unwrap_or(0);
            let id = self.folders.len();
            self.folders.push(Folder {
                name: String::from(file_name(&p)),
                path: p.clone(),
                parent: Some(up),
                folders: Vec::new(),
                notes: Vec::new(),
            });
            self.folder_by_path.insert(p, id);
        }
        for p in note_paths {
            let folder = self.folder_by_path.get(parent(&p)).copied().unwrap_or(0);
            let name = String::from(strip_md(file_name(&p)));
            let key = link_key(file_name(&p));
            let id = self.notes.len();
            self.by_key.entry(key.clone()).or_default().push(id);
            self.by_path.insert(p.clone(), id);
            self.notes.push(Note {
                path: p,
                name,
                key,
                folder,
            });
        }
        // Children in tree order: folders by name, then notes by name.
        for id in 1..self.folders.len() {
            if let Some(up) = self.folders.get(id).and_then(|f| f.parent) {
                if let Some(f) = self.folders.get_mut(up) {
                    f.folders.push(id);
                }
            }
        }
        for (id, n) in self.notes.iter().enumerate() {
            if let Some(f) = self.folders.get_mut(n.folder) {
                f.notes.push(id);
            }
        }
        let names: Vec<String> = self.folders.iter().map(|f| f.name.clone()).collect();
        let note_names: Vec<String> = self.notes.iter().map(|n| n.name.clone()).collect();
        for f in self.folders.iter_mut() {
            f.folders.sort_by(|a, b| {
                name_order(
                    names.get(*a).map_or("", String::as_str),
                    names.get(*b).map_or("", String::as_str),
                )
            });
            f.notes.sort_by(|a, b| {
                name_order(
                    note_names.get(*a).map_or("", String::as_str),
                    note_names.get(*b).map_or("", String::as_str),
                )
            });
        }
        // Index entries for notes that are gone go too.
        let live: BTreeMap<String, ()> = self.notes.iter().map(|n| (n.path.clone(), ())).collect();
        self.meta.retain(|p, _| live.contains_key(p));
        self.generation += 1;
    }

    fn rebuild(&mut self) {
        let folders: Vec<String> = self
            .folders
            .iter()
            .skip(1)
            .map(|f| f.path.clone())
            .collect();
        let notes: Vec<String> = self.notes.iter().map(|n| n.path.clone()).collect();
        self.build(folders, notes);
    }

    pub fn note_id(&self, path: &str) -> Option<usize> {
        self.by_path.get(path).copied()
    }

    pub fn note(&self, path: &str) -> Option<&Note> {
        self.note_id(path).and_then(|i| self.notes.get(i))
    }

    pub fn folder_id(&self, path: &str) -> Option<usize> {
        self.folder_by_path.get(path).copied()
    }

    /// The note a `[[link]]` names: by name, case-insensitive, or by a path
    /// from the root (`folder/name`). Of two notes with one name, the one
    /// nearer the root wins, the way a shortest-path link means it.
    pub fn resolve(&self, link: &str) -> Option<usize> {
        let link = link.trim();
        let link = link.strip_suffix(".md").unwrap_or(link);
        if link.is_empty() {
            return None;
        }
        let key = lower(link);
        if key.contains('/') {
            let mut with = key.clone();
            with.push_str(".md");
            return self
                .notes
                .iter()
                .position(|n| lower(&n.path) == with || lower(&n.path).ends_with(&with));
        }
        let ids = self.by_key.get(&key)?;
        ids.iter()
            .copied()
            .min_by_key(|i| self.notes.get(*i).map_or(usize::MAX, |n| n.path.len()))
    }

    /// How many notes are still waiting to be indexed.
    pub fn index_pending(&self) -> usize {
        self.queue.len()
    }

    pub fn indexed(&self) -> usize {
        self.meta.len()
    }

    /// Read and index notes until `more` says to stop. True if any were
    /// indexed.
    pub fn index_some(&mut self, more: &mut dyn FnMut() -> bool) -> bool {
        let mut any = false;
        while more() {
            let Some(path) = self.queue.pop() else {
                break;
            };
            if self.by_path.contains_key(&path) {
                if let Ok(text) = self.fs.read(&path) {
                    let mut m = scan_file(&path, &text);
                    m.stat = self.fs.stat(&path);
                    self.meta.insert(path, m);
                    any = true;
                }
            }
        }
        if any {
            self.generation += 1;
        }
        any
    }

    /// Index one note from text the app already holds (a note just saved).
    pub fn index_text(&mut self, path: &str, text: &str) {
        if self.by_path.contains_key(path) {
            let mut m = scan_file(path, text);
            m.stat = self.fs.stat(path);
            self.meta.insert(String::from(path), m);
            self.generation += 1;
        }
    }

    /// Read this note again soon (it changed on disk).
    pub fn queue_index(&mut self, path: &str) {
        if !self.queue.iter().any(|q| q == path) {
            self.queue.push(String::from(path));
        }
    }

    /// Put every note back in the queue (after the folder changed on disk).
    pub fn reindex_all(&mut self) {
        self.queue = self
            .notes
            .iter()
            .rev()
            .filter(|n| indexed_kind(&n.path))
            .map(|n| n.path.clone())
            .collect();
    }

    /// The notes that link to `path`, with the lines they do it on, in
    /// tree order.
    pub fn backlinks(&self, path: &str) -> Vec<Backlink> {
        let Some(note) = self.note(path) else {
            return Vec::new();
        };
        let key = note.key.clone();
        let path_key = lower(strip_md(&note.path));
        let mut out = Vec::new();
        for n in &self.notes {
            let Some(meta) = self.meta.get(&n.path) else {
                continue;
            };
            let mut lines = Vec::new();
            for (target, line) in &meta.links {
                let hit = *target == key
                    || *target == path_key
                    || (target.contains('/') && path_key.ends_with(target.as_str()));
                // A note with the same name nearer the root takes plain links.
                if hit
                    && (target.contains('/')
                        || self.resolve(target).is_some_and(|i| {
                            self.notes.get(i).is_some_and(|m| m.path == note.path)
                        }))
                {
                    lines.push(*line);
                }
            }
            if !lines.is_empty() && n.path != note.path {
                out.push(Backlink {
                    path: n.path.clone(),
                    lines,
                    context: Vec::new(),
                });
            }
        }
        out
    }

    /// Every tag in the vault and how many times it is used, most used
    /// first.
    pub fn tags(&self) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<String, (String, usize)> = BTreeMap::new();
        for m in self.meta.values() {
            for t in &m.tags {
                let e = counts.entry(lower(t)).or_insert_with(|| (t.clone(), 0));
                e.1 += 1;
            }
        }
        let mut out: Vec<(String, usize)> = counts.into_values().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| name_order(&a.0, &b.0)));
        out
    }

    /// A free name in `folder` starting from `base`: `Untitled`,
    /// `Untitled 1`, `Untitled 2`...
    pub fn free_name(&mut self, folder: &str, base: &str, ext: &str) -> String {
        let mut n = 0usize;
        loop {
            let mut name = String::from(base);
            if n > 0 {
                name.push(' ');
                crate::text::push_num(&mut name, n);
            }
            name.push_str(ext);
            let p = join(folder, &name);
            if !self.fs.exists(&p) {
                return p;
            }
            n += 1;
        }
    }

    /// Make an empty note in `folder`, named from `base`. Its path.
    pub fn create_note(&mut self, folder: &str, base: &str, text: &str) -> Result<String, String> {
        self.create_file(folder, base, ".md", text)
    }

    /// Make a file (`ext` is `.md`, `.canvas` or `.base`) in `folder`, named
    /// from `base`. Its path.
    pub fn create_file(
        &mut self,
        folder: &str,
        base: &str,
        ext: &str,
        text: &str,
    ) -> Result<String, String> {
        if !folder.is_empty() && !self.fs.exists(folder) {
            self.fs.mkdir(folder)?;
        }
        let path = self.free_name(folder, base, ext);
        self.fs.write(&path, text)?;
        let mut folders: Vec<String> = self
            .folders
            .iter()
            .skip(1)
            .map(|f| f.path.clone())
            .collect();
        let mut d = folder;
        while !d.is_empty() {
            if !folders.iter().any(|f| f == d) {
                folders.push(String::from(d));
            }
            d = parent(d);
        }
        let mut notes: Vec<String> = self.notes.iter().map(|n| n.path.clone()).collect();
        notes.push(path.clone());
        self.build(folders, notes);
        self.meta.insert(path.clone(), scan_file(&path, text));
        Ok(path)
    }

    /// Make a note at exactly `path` (a link to a note that is not there
    /// yet, followed). Folders on the way are made too.
    pub fn create_at(&mut self, path: &str) -> Result<String, String> {
        if self.by_path.contains_key(path) {
            return Ok(String::from(path));
        }
        let dir = parent(path);
        if !dir.is_empty() && !self.fs.exists(dir) {
            self.fs.mkdir(dir)?;
        }
        self.fs.write(path, "")?;
        let mut folders: Vec<String> = self
            .folders
            .iter()
            .skip(1)
            .map(|f| f.path.clone())
            .collect();
        let mut d = dir;
        while !d.is_empty() {
            if !folders.iter().any(|f| f == d) {
                folders.push(String::from(d));
            }
            d = parent(d);
        }
        let mut notes: Vec<String> = self.notes.iter().map(|n| n.path.clone()).collect();
        notes.push(String::from(path));
        self.build(folders, notes);
        self.meta.insert(String::from(path), Meta::default());
        Ok(String::from(path))
    }

    /// Make a folder in `parent_dir`, named from `base`. Its path.
    pub fn create_folder(&mut self, parent_dir: &str, base: &str) -> Result<String, String> {
        let path = self.free_name(parent_dir, base, "");
        self.fs.mkdir(&path)?;
        let mut folders: Vec<String> = self
            .folders
            .iter()
            .skip(1)
            .map(|f| f.path.clone())
            .collect();
        folders.push(path.clone());
        let notes: Vec<String> = self.notes.iter().map(|n| n.path.clone()).collect();
        self.build(folders, notes);
        Ok(path)
    }

    /// Move or rename a note or folder from `from` to `to`, on disk and in
    /// the tree. Index entries follow their notes.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        if from == to {
            return Ok(());
        }
        if to.is_empty() || file_name(to).is_empty() {
            return Err(String::from("A name cannot be empty"));
        }
        if file_name(to).contains(['\\', ':', '*', '?', '"', '<', '>', '|']) {
            return Err(String::from("A name cannot contain \\ : * ? \" < > |"));
        }
        if self.fs.exists(to) && lower(from) != lower(to) {
            return Err(String::from("Something with that name is already there"));
        }
        let is_folder = self.folder_by_path.contains_key(from);
        let dir = parent(to);
        if !dir.is_empty() && !self.fs.exists(dir) {
            self.fs.mkdir(dir)?;
        }
        self.fs.rename(from, to)?;
        let mut prefix = String::from(from);
        prefix.push('/');
        let moved = |p: &str| -> Option<String> {
            if p == from {
                Some(String::from(to))
            } else {
                p.strip_prefix(prefix.as_str()).map(|rest| {
                    let mut n = String::from(to);
                    n.push('/');
                    n.push_str(rest);
                    n
                })
            }
        };
        let mut folders: Vec<String> = Vec::new();
        for f in self.folders.iter().skip(1) {
            folders.push(moved(&f.path).unwrap_or_else(|| f.path.clone()));
        }
        let mut d = dir;
        while !d.is_empty() {
            if !folders.iter().any(|f| f == d) {
                folders.push(String::from(d));
            }
            d = parent(d);
        }
        let mut notes: Vec<String> = Vec::new();
        let mut meta_moves: Vec<(String, String)> = Vec::new();
        for n in &self.notes {
            match moved(&n.path) {
                Some(np) => {
                    meta_moves.push((n.path.clone(), np.clone()));
                    notes.push(np);
                }
                None => notes.push(n.path.clone()),
            }
        }
        for (a, b) in meta_moves {
            if let Some(m) = self.meta.remove(&a) {
                self.meta.insert(b, m);
            }
        }
        let _ = is_folder;
        self.build(folders, notes);
        Ok(())
    }

    /// Delete a note or folder by moving it into the vault's `.trash`
    /// folder, where it can still be found and brought back. Returns where
    /// it went.
    pub fn trash(&mut self, path: &str) -> Result<String, String> {
        if !self.fs.exists(".trash") {
            self.fs.mkdir(".trash")?;
        }
        let name = file_name(path);
        let (base, ext) = match name.rfind('.') {
            Some(i) if !self.folder_by_path.contains_key(path) => {
                (name.get(..i).unwrap_or(name), name.get(i..).unwrap_or(""))
            }
            _ => (name, ""),
        };
        let base = String::from(base);
        let ext = String::from(ext);
        let to = self.free_name(".trash", &base, &ext);
        self.fs.rename(path, &to)?;
        let mut prefix = String::from(path);
        prefix.push('/');
        let folders: Vec<String> = self
            .folders
            .iter()
            .skip(1)
            .filter(|f| f.path != path && !f.path.starts_with(&prefix))
            .map(|f| f.path.clone())
            .collect();
        let notes: Vec<String> = self
            .notes
            .iter()
            .filter(|n| n.path != path && !n.path.starts_with(&prefix))
            .map(|n| n.path.clone())
            .collect();
        self.build(folders, notes);
        Ok(to)
    }

    /// Delete as the vault's "Deleted files" setting says: `system` (the
    /// computer's trash, falling back to `.trash` where there is none),
    /// `local` (the vault's `.trash`) or `none` (gone for good). Says where
    /// it went: the trash path, `"system"`, or `""` when it is gone.
    pub fn delete_file(&mut self, path: &str, how: &str) -> Result<String, String> {
        match how {
            "none" => {
                self.remove_all(path)?;
                self.forget(path);
                Ok(String::new())
            }
            "local" => self.trash(path),
            _ => match self.fs.trash(path) {
                Ok(()) => {
                    self.forget(path);
                    Ok(String::from("system"))
                }
                Err(_) => self.trash(path),
            },
        }
    }

    /// Remove a file, or a folder and everything in it.
    fn remove_all(&mut self, path: &str) -> Result<(), String> {
        if self.folder_by_path.contains_key(path) {
            for e in self.fs.list(path)? {
                let p = join(path, &e.name);
                if e.is_dir {
                    self.fs_remove_dir_all(&p)?;
                } else {
                    self.fs.remove_file(&p)?;
                }
            }
            self.fs.remove_dir(path)
        } else {
            self.fs.remove_file(path)
        }
    }

    fn fs_remove_dir_all(&mut self, dir: &str) -> Result<(), String> {
        for e in self.fs.list(dir)? {
            let p = join(dir, &e.name);
            if e.is_dir {
                self.fs_remove_dir_all(&p)?;
            } else {
                self.fs.remove_file(&p)?;
            }
        }
        self.fs.remove_dir(dir)
    }

    /// Take a path (and what is under it) out of the tree.
    fn forget(&mut self, path: &str) {
        let mut prefix = String::from(path);
        prefix.push('/');
        let folders: Vec<String> = self
            .folders
            .iter()
            .skip(1)
            .filter(|f| f.path != path && !f.path.starts_with(&prefix))
            .map(|f| f.path.clone())
            .collect();
        let notes: Vec<String> = self
            .notes
            .iter()
            .filter(|n| n.path != path && !n.path.starts_with(&prefix))
            .map(|n| n.path.clone())
            .collect();
        self.build(folders, notes);
    }

    /// Every folder path, root first, for the "move to" chooser.
    pub fn folder_paths(&self) -> Vec<String> {
        self.folders.iter().map(|f| f.path.clone()).collect()
    }

    /// Pull a change that happened on disk into the tree.
    pub fn refresh(&mut self) {
        self.scan();
        self.reindex_all();
    }

    /// Notes in tree order (for a stable report).
    pub fn note_count(&self) -> usize {
        self.notes.len()
    }

    pub fn folder_count(&self) -> usize {
        self.folders.len().saturating_sub(1)
    }

    #[allow(dead_code)]
    fn touch(&mut self) {
        self.rebuild();
    }
}

/// Read what a file says: a note's links, tags and properties, a canvas's
/// cards that show notes and the links in its text cards.
pub fn scan_file(path: &str, text: &str) -> Meta {
    match kind_of(path) {
        FileKind::Note => scan_text(text),
        FileKind::Canvas => {
            let mut meta = Meta::default();
            if let Some(c) = crate::canvas::Canvas::parse(text) {
                for (i, n) in c.nodes.iter().enumerate() {
                    if let Some(f) = n.file.as_deref() {
                        let f = f.strip_suffix(".md").unwrap_or(f);
                        meta.links.push((lower(f), i as u32));
                    }
                    if let Some(t) = n.text.as_deref() {
                        let inner = scan_text(t);
                        meta.links
                            .extend(inner.links.into_iter().map(|(l, _)| (l, i as u32)));
                        meta.tags.extend(inner.tags);
                    }
                }
            }
            meta
        }
        _ => Meta::default(),
    }
}

/// Read a note's links and tags, skipping fenced code.
pub fn scan_text(text: &str) -> Meta {
    let mut meta = Meta::default();
    let mut in_code = false;
    let body = match crate::yaml::front_matter(text) {
        Some((props, body)) => {
            for key in ["tags", "tag"] {
                match props.get(key) {
                    Some(crate::json::Value::Arr(a)) => meta.tags.extend(
                        a.iter()
                            .filter_map(crate::json::Value::str)
                            .map(|t| String::from(t.trim_start_matches('#'))),
                    ),
                    Some(crate::json::Value::Str(s)) => meta.tags.extend(
                        s.split([',', ' '])
                            .filter(|t| !t.is_empty())
                            .map(|t| String::from(t.trim_start_matches('#'))),
                    ),
                    _ => {}
                }
            }
            meta.props = Some(props);
            body
        }
        None => 0,
    };
    for (n, line) in text.split('\n').enumerate() {
        if n < body {
            // Links written in properties count, the rest of the block not.
            if let Some(i) = line.find("[[") {
                md::refs(
                    line.get(i..).unwrap_or(""),
                    |l| {
                        let l = l.strip_suffix(".md").unwrap_or(l);
                        if !l.is_empty() {
                            meta.links.push((lower(l), n as u32));
                        }
                    },
                    |_| {},
                );
            }
            continue;
        }
        if md::is_fence(line) {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        md::refs(
            line,
            |l| {
                let l = l.strip_suffix(".md").unwrap_or(l);
                if !l.is_empty() {
                    meta.links.push((lower(l), n as u32));
                }
            },
            |t| meta.tags.push(String::from(t)),
        );
    }
    meta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemFs;

    fn sample() -> Vault {
        let mut fs = MemFs::new();
        fs.put(
            "Welcome.md",
            "# Welcome\nSee [[Plan]] and [[Ideas|my ideas]]. #start",
        );
        fs.put(
            "Projects/Plan.md",
            "# Plan\nBack to [[welcome]].\n```\n[[NotALink]]\n```\n#work",
        );
        fs.put(
            "Projects/Ideas.md",
            "Ideas link [[Projects/Plan]] #work #idea",
        );
        fs.put("Archive/Old/Plan.md", "older plan");
        fs.put(".settings/app.json", "{}");
        fs.put("image.png", "x");
        Vault::open("Sample", Box::new(fs))
    }

    fn index_all(v: &mut Vault) {
        v.index_some(&mut || true);
    }

    #[test]
    fn opening_lists_notes_and_folders_but_not_hidden_ones() {
        let mut v = sample();
        // Four notes and a picture (an attachment is listed, not indexed).
        assert_eq!(v.note_count(), 5);
        assert_eq!(v.note("image.png").map(|n| n.name.as_str()), Some("image"));
        assert_eq!(kind_of("image.png"), FileKind::Image);
        assert_eq!(v.folder_count(), 3);
        assert!(v.note("Projects/Plan.md").is_some());
        assert!(v.note(".settings/app.json").is_none());
        let root = &v.folders[0];
        // Folders first, by name; then notes.
        let names: Vec<&str> = root
            .folders
            .iter()
            .map(|i| v.folders[*i].name.as_str())
            .collect();
        assert_eq!(names, ["Archive", "Projects"]);
        assert_eq!(root.notes.len(), 2);
        assert_eq!(v.index_pending(), 4);
        // Deleting: the vault's trash, or for good; "system" falls back to
        // the vault's trash where the computer has none.
        assert_eq!(
            v.delete_file("image.png", "system").unwrap(),
            ".trash/image.png"
        );
        assert!(v.note("image.png").is_none());
        assert_eq!(v.delete_file("Welcome.md", "none").unwrap(), "");
        assert!(!v.fs.exists("Welcome.md"));
        assert!(v.note("Welcome.md").is_none());
    }

    #[test]
    fn links_resolve_by_name_path_and_shortest_path() {
        let v = sample();
        let plan = v.resolve("plan").map(|i| v.notes[i].path.clone());
        assert_eq!(plan.as_deref(), Some("Projects/Plan.md"));
        let old = v
            .resolve("Archive/Old/Plan")
            .map(|i| v.notes[i].path.clone());
        assert_eq!(old.as_deref(), Some("Archive/Old/Plan.md"));
        assert!(v.resolve("Welcome.md").is_some());
        assert!(v.resolve("Missing").is_none());
    }

    #[test]
    fn backlinks_and_tags_come_from_the_index() {
        let mut v = sample();
        assert!(v.backlinks("Projects/Plan.md").is_empty());
        index_all(&mut v);
        assert_eq!(v.index_pending(), 0);
        let back = v.backlinks("Projects/Plan.md");
        let from: Vec<&str> = back.iter().map(|b| b.path.as_str()).collect();
        assert_eq!(from, ["Projects/Ideas.md", "Welcome.md"]);
        assert_eq!(back[1].lines, [1]);
        // Code fences hide links.
        assert!(v.meta["Projects/Plan.md"]
            .links
            .iter()
            .all(|(t, _)| t != "notalink"));
        let tags = v.tags();
        assert_eq!(tags[0], (String::from("work"), 2));
        assert!(tags.iter().any(|(t, _)| t == "idea"));
        // The old plan with the same name is not what plain links reach.
        assert!(v.backlinks("Archive/Old/Plan.md").is_empty());
    }

    #[test]
    fn create_rename_move_and_trash() {
        let mut v = sample();
        index_all(&mut v);
        let p = v.create_note("", "Untitled", "").unwrap();
        assert_eq!(p, "Untitled.md");
        let p2 = v.create_note("", "Untitled", "").unwrap();
        assert_eq!(p2, "Untitled 1.md");
        v.rename("Untitled.md", "Projects/Fresh.md").unwrap();
        assert!(v.note("Projects/Fresh.md").is_some());
        assert!(v.note("Untitled.md").is_none());
        assert!(v.rename("Untitled 1.md", "Welcome.md").is_err());
        // A folder moves with everything in it, index included.
        v.rename("Projects", "Work/Projects").unwrap();
        assert!(v.note("Work/Projects/Plan.md").is_some());
        assert!(v.meta.contains_key("Work/Projects/Ideas.md"));
        assert!(v.folder_id("Work").is_some());
        let went = v.trash("Work/Projects/Ideas.md").unwrap();
        assert_eq!(went, ".trash/Ideas.md");
        assert!(v.note("Work/Projects/Ideas.md").is_none());
        assert!(v.fs.exists(".trash/Ideas.md"));
        let f = v.create_folder("", "Untitled").unwrap();
        assert_eq!(f, "Untitled");
        assert!(v.folder_id("Untitled").is_some());
        let made = v.create_at("New/Linked.md").unwrap();
        assert!(v.note(&made).is_some());
    }
}
