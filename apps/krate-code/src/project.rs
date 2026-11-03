//! A project: a folder, its file tree, and the file system seam.
//!
//! The editor never names a real path. A folder the person picked is reached
//! through `picked/<token>/` (the guest's disk backend adds that prefix), and
//! every path the editor holds is relative to the project root: `src/main.rs`.
//! [`Vfs`] is the seam: the guest's disk backend for a picked folder, and
//! [`MemFs`] for the tour (and for tests), which keeps files in memory.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// What the editor needs from a folder. Paths are relative to the project
/// root, `/`-separated, with no leading slash; `""` is the root itself.
pub trait Vfs {
    /// Whether writes reach the person's disk (false for the tour).
    fn is_real(&self) -> bool;
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, String>;
    fn read(&mut self, path: &str) -> Result<String, String>;
    fn write(&mut self, path: &str, text: &str) -> Result<(), String>;
    fn mkdir(&mut self, path: &str) -> Result<(), String>;
    fn rename(&mut self, from: &str, to: &str) -> Result<(), String>;
    fn remove_file(&mut self, path: &str) -> Result<(), String>;
    fn remove_dir(&mut self, path: &str) -> Result<(), String>;
    fn exists(&mut self, path: &str) -> bool;
    /// Whether something outside the editor changed the folder since the
    /// last call (a build writing `target/`).
    fn changed(&mut self) -> bool {
        false
    }
}

pub fn join(dir: &str, name: &str) -> String {
    let mut s = String::from(dir.trim_end_matches('/'));
    if !s.is_empty() {
        s.push('/');
    }
    s.push_str(name.trim_start_matches('/'));
    s
}

pub fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => path.get(..i).unwrap_or(""),
        None => "",
    }
}

pub fn file_name(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => path.get(i + 1..).unwrap_or(path),
        None => path,
    }
}

/// A name the person typed for a new file or folder, made safe: relative,
/// no `..`, no empty parts. `Err` says why not in plain words.
pub fn clean_relative(name: &str) -> Result<String, String> {
    let name = name.trim().trim_start_matches("./");
    if name.is_empty() {
        return Err(String::from("Type a name first"));
    }
    if name.starts_with('/') || name.contains('\\') || name.contains(':') {
        return Err(String::from(
            "Use a name inside the project, like src/util.rs",
        ));
    }
    let mut out = String::new();
    for part in name.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(String::from("A name cannot leave the project folder (..)"));
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    if out.is_empty() {
        return Err(String::from("Type a name first"));
    }
    Ok(out)
}

/// A crate name cargo accepts, made from a folder name: lowercase letters,
/// digits, `-` and `_`, starting with a letter.
pub fn crate_name_from(folder: &str) -> String {
    let mut s = String::new();
    for c in folder.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            s.push(c);
        } else if (c == ' ' || c == '.') && !s.ends_with('-') {
            s.push('-');
        }
    }
    let s = String::from(s.trim_matches('-'));
    if s.is_empty() || !s.as_bytes().first().is_some_and(u8::is_ascii_alphabetic) {
        let mut named = String::from("my-");
        named.push_str(if s.is_empty() { "project" } else { &s });
        return named;
    }
    s
}

pub fn valid_crate_name(name: &str) -> bool {
    !name.is_empty()
        && name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// The files `cargo new` would make for a binary crate.
pub fn new_project_files(name: &str) -> Vec<(&'static str, String)> {
    let mut cargo = String::from("[package]\nname = \"");
    cargo.push_str(name);
    cargo.push_str("\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n");
    let main = String::from("fn main() {\n    println!(\"Hello, world!\");\n}\n");
    alloc::vec![
        ("Cargo.toml", cargo),
        ("src/main.rs", main),
        (".gitignore", String::from("/target\n")),
    ]
}

/// Folders the tree shows folded and quick-open skips.
pub fn is_heavy_dir(name: &str) -> bool {
    matches!(name, "target" | ".git" | "node_modules")
}

fn sort_entries(v: &mut [Entry]) {
    v.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        })
    });
}

/// One row of the tree as drawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub expanded: BTreeSet<String>,
    cache: BTreeMap<String, Vec<Entry>>,
    pub rows: Vec<Row>,
    pub selected: usize,
    /// Rows scrolled off the top (fractional, for smooth wheels).
    pub scroll: f32,
    pub error: Option<String>,
}

impl Tree {
    pub fn new() -> Self {
        let mut t = Tree::default();
        t.expanded.insert(String::new());
        t
    }

    fn listing(&mut self, fs: &mut dyn Vfs, dir: &str) -> Vec<Entry> {
        if let Some(v) = self.cache.get(dir) {
            return v.clone();
        }
        match fs.list(dir) {
            Ok(mut v) => {
                v.retain(|e| e.name != ".git" && e.name != ".DS_Store");
                sort_entries(&mut v);
                self.cache.insert(String::from(dir), v.clone());
                v
            }
            Err(e) => {
                self.error = Some(e);
                Vec::new()
            }
        }
    }

    fn walk(&mut self, fs: &mut dyn Vfs, dir: &str, depth: usize, out: &mut Vec<Row>) {
        if depth > 24 || out.len() > 20_000 {
            return;
        }
        for e in self.listing(fs, dir) {
            let path = join(dir, &e.name);
            let expanded = e.is_dir && self.expanded.contains(&path);
            out.push(Row {
                path: path.clone(),
                name: e.name.clone(),
                depth,
                is_dir: e.is_dir,
                expanded,
            });
            if expanded {
                self.walk(fs, &path, depth + 1, out);
            }
        }
    }

    /// Rebuild the rows from the (cached) listings.
    pub fn refresh(&mut self, fs: &mut dyn Vfs) {
        self.error = None;
        let mut rows = Vec::new();
        self.walk(fs, "", 0, &mut rows);
        self.rows = rows;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
    }

    /// Forget every listing, so the next refresh reads the disk again.
    pub fn reload(&mut self, fs: &mut dyn Vfs) {
        self.cache.clear();
        self.refresh(fs);
    }

    /// Forget one folder's listing (after a change inside it).
    pub fn invalidate(&mut self, dir: &str) {
        self.cache.remove(dir);
    }

    pub fn toggle(&mut self, fs: &mut dyn Vfs, index: usize) {
        let Some(row) = self.rows.get(index).cloned() else {
            return;
        };
        if !row.is_dir {
            return;
        }
        if row.expanded {
            self.expanded.remove(&row.path);
        } else {
            self.expanded.insert(row.path.clone());
        }
        self.selected = index;
        self.refresh(fs);
    }

    /// Open every folder down to `path` and select it.
    pub fn reveal(&mut self, fs: &mut dyn Vfs, path: &str) {
        let mut dir = parent(path);
        loop {
            self.expanded.insert(String::from(dir));
            if dir.is_empty() {
                break;
            }
            dir = parent(dir);
        }
        self.refresh(fs);
        if let Some(i) = self.rows.iter().position(|r| r.path == path) {
            self.selected = i;
        }
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    /// The folder new files go in, from the selection.
    pub fn target_dir(&self) -> String {
        match self.selected_row() {
            Some(r) if r.is_dir => r.path.clone(),
            Some(r) => String::from(parent(&r.path)),
            None => String::new(),
        }
    }
}

pub struct Project {
    pub fs: Box<dyn Vfs>,
    pub tree: Tree,
    /// The folder's name, for the title and the tree header.
    pub name: String,
}

impl Project {
    pub fn new(name: &str, fs: Box<dyn Vfs>) -> Self {
        let mut p = Project {
            fs,
            tree: Tree::new(),
            name: String::from(name),
        };
        if p.fs.exists("src") {
            p.tree.expanded.insert(String::from("src"));
        }
        p.tree.refresh(&mut *p.fs);
        p
    }

    pub fn refresh(&mut self) {
        self.tree.refresh(&mut *self.fs);
    }

    pub fn reload(&mut self) {
        self.tree.reload(&mut *self.fs);
    }

    /// Every file in the project (skipping build output), for quick open.
    pub fn all_files(&mut self, limit: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack: Vec<String> = alloc::vec![String::new()];
        while let Some(dir) = stack.pop() {
            let Ok(mut entries) = self.fs.list(&dir) else {
                continue;
            };
            sort_entries(&mut entries);
            for e in entries.iter().rev() {
                let path = join(&dir, &e.name);
                if e.is_dir {
                    if !is_heavy_dir(&e.name) && path.matches('/').count() < 16 {
                        stack.push(path);
                    }
                } else {
                    out.push(path);
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
        out.sort();
        out
    }

    /// Remove a file or a whole folder.
    pub fn remove(&mut self, path: &str, is_dir: bool) -> Result<(), String> {
        if is_dir {
            let entries = self.fs.list(path)?;
            for e in entries {
                self.remove(&join(path, &e.name), e.is_dir)?;
            }
            self.fs.remove_dir(path)?;
        } else {
            self.fs.remove_file(path)?;
        }
        self.tree.invalidate(parent(path));
        self.tree.invalidate(path);
        Ok(())
    }
}

/// Files held in memory: the tour, and tests.
#[derive(Debug, Clone, Default)]
pub struct MemFs {
    pub files: BTreeMap<String, String>,
    pub dirs: BTreeSet<String>,
    /// Writes reach nothing but memory.
    pub writes: usize,
}

impl MemFs {
    pub fn new() -> Self {
        MemFs::default()
    }

    pub fn with(files: &[(&str, &str)]) -> Self {
        let mut m = MemFs::new();
        for (p, t) in files {
            let _ = m.write(p, t);
        }
        m.writes = 0;
        m
    }

    fn add_parents(&mut self, path: &str) {
        let mut dir = parent(path);
        while !dir.is_empty() {
            self.dirs.insert(String::from(dir));
            dir = parent(dir);
        }
    }
}

impl Vfs for MemFs {
    fn is_real(&self) -> bool {
        false
    }

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, String> {
        if !dir.is_empty() && !self.dirs.contains(dir) {
            return Err(String::from("No such folder"));
        }
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for d in self.dirs.iter() {
            if parent(d) == dir && seen.insert(d.clone()) {
                out.push(Entry {
                    name: String::from(file_name(d)),
                    is_dir: true,
                });
            }
        }
        for f in self.files.keys() {
            if parent(f) == dir {
                out.push(Entry {
                    name: String::from(file_name(f)),
                    is_dir: false,
                });
            }
        }
        Ok(out)
    }

    fn read(&mut self, path: &str) -> Result<String, String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| String::from("No such file"))
    }

    fn write(&mut self, path: &str, text: &str) -> Result<(), String> {
        self.add_parents(path);
        self.files.insert(String::from(path), String::from(text));
        self.writes += 1;
        Ok(())
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        self.add_parents(path);
        self.dirs.insert(String::from(path));
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        if self.exists(to) {
            return Err(String::from("Something with that name is already there"));
        }
        if let Some(t) = self.files.remove(from) {
            self.add_parents(to);
            self.files.insert(String::from(to), t);
            return Ok(());
        }
        if self.dirs.contains(from) {
            let mut moved_files = Vec::new();
            for (k, v) in self.files.iter() {
                if let Some(rest) = k.strip_prefix(from).and_then(|r| r.strip_prefix('/')) {
                    moved_files.push((k.clone(), join(to, rest), v.clone()));
                }
            }
            for (old, new, v) in moved_files {
                self.files.remove(&old);
                self.files.insert(new, v);
            }
            let moved_dirs: Vec<String> = self
                .dirs
                .iter()
                .filter(|d| d.as_str() == from || d.starts_with(&(String::from(from) + "/")))
                .cloned()
                .collect();
            for d in moved_dirs {
                self.dirs.remove(&d);
                let rest = d.get(from.len()..).unwrap_or("");
                let mut n = String::from(to);
                n.push_str(rest);
                self.dirs.insert(n);
            }
            self.add_parents(to);
            return Ok(());
        }
        Err(String::from("No such file"))
    }

    fn remove_file(&mut self, path: &str) -> Result<(), String> {
        self.files
            .remove(path)
            .map(|_| ())
            .ok_or_else(|| String::from("No such file"))
    }

    fn remove_dir(&mut self, path: &str) -> Result<(), String> {
        let prefix = String::from(path) + "/";
        if self.files.keys().any(|k| k.starts_with(&prefix))
            || self.dirs.iter().any(|d| d.starts_with(&prefix))
        {
            return Err(String::from("The folder is not empty"));
        }
        if self.dirs.remove(path) {
            Ok(())
        } else {
            Err(String::from("No such folder"))
        }
    }

    fn exists(&mut self, path: &str) -> bool {
        self.files.contains_key(path) || self.dirs.contains(path)
    }
}

/// The tour: a small crate to try every mode on without opening a folder.
pub fn tour_files() -> Vec<(&'static str, &'static str)> {
    alloc::vec![
        ("Cargo.toml", TOUR_CARGO),
        ("README.md", TOUR_README),
        ("src/main.rs", TOUR_MAIN),
        ("src/shapes.rs", TOUR_SHAPES),
    ]
}

const TOUR_CARGO: &str =
    "[package]\nname = \"tour\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n";

const TOUR_README: &str = "# The tour\n\nA small crate to try Krate Code on. Nothing here is saved to your disk:\nopen a folder (Ctrl+O) or make a new project for that.\n\nTry: `w` `b` `e` to move by words, `dw` to delete one, `u` to undo it,\n`ci(` inside a call, `/area` then `n`, `:%s/Shape/Figure/g`, `Ctrl+W v`.\n";

const TOUR_MAIN: &str = r##"//! The tour: a little crate to try the editor on.
//!
//! Press `i` to type and `Esc` to stop. In normal mode the letters are
//! commands: `w` jumps a word, `dd` deletes a line, `u` undoes, `.` repeats.

mod shapes;

use shapes::{Circle, Rect, Shape};

/// How many shapes the demo draws.
const COUNT: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Scene<'a> {
    title: &'a str,
    scale: f64,
}

fn total_area(shapes: &[Box<dyn Shape>]) -> f64 {
    /* Block comments nest in Rust: /* inner */ and this is still a comment. */
    shapes.iter().map(|s| s.area()).sum()
}

fn main() {
    let scene = Scene { title: "shapes", scale: 1.5 };
    let shapes: Vec<Box<dyn Shape>> = vec![
        Box::new(Circle { r: 2.0 }),
        Box::new(Rect { w: 3.0, h: 4.5 }),
        Box::new(Circle { r: 0.75 }),
    ];
    assert_eq!(shapes.len(), COUNT);

    let label = r#"area of "everything""#;
    let tick = '\u{2713}';
    for (i, shape) in shapes.iter().enumerate() {
        println!("{tick} {} #{i}: {:.2}", shape.name(), shape.area() * scene.scale);
    }
    match total_area(&shapes) {
        a if a > 100.0 => println!("{label}: huge ({a:.1})"),
        a => println!("{label}: {a:.1} in {}", scene.title),
    }
}
"##;

const TOUR_SHAPES: &str = r#"/// Anything with an area.
pub trait Shape {
    fn area(&self) -> f64;
    fn name(&self) -> &'static str;
}

pub struct Circle {
    pub r: f64,
}

pub struct Rect {
    pub w: f64,
    pub h: f64,
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        std::f64::consts::PI * self.r * self.r
    }

    fn name(&self) -> &'static str {
        "circle"
    }
}

impl Shape for Rect {
    fn area(&self) -> f64 {
        self.w * self.h
    }

    fn name(&self) -> &'static str {
        "rect"
    }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn tour() -> Project {
        let mut m = MemFs::new();
        for (p, t) in tour_files() {
            m.write(p, t).unwrap();
        }
        Project::new("tour", Box::new(m))
    }

    #[test]
    fn the_tree_lists_folders_first_and_opens_src() {
        let p = tour();
        let names: Vec<(&str, usize)> = p
            .tree
            .rows
            .iter()
            .map(|r| (r.name.as_str(), r.depth))
            .collect();
        assert_eq!(
            names,
            [
                ("src", 0),
                ("main.rs", 1),
                ("shapes.rs", 1),
                ("Cargo.toml", 0),
                ("README.md", 0)
            ]
        );
    }

    #[test]
    fn toggle_and_reveal() {
        let mut p = tour();
        p.tree.toggle(&mut *p.fs, 0);
        assert_eq!(p.tree.rows.len(), 3);
        p.tree.reveal(&mut *p.fs, "src/shapes.rs");
        assert_eq!(p.tree.rows.len(), 5);
        assert_eq!(p.tree.selected_row().unwrap().path, "src/shapes.rs");
        assert_eq!(p.tree.target_dir(), "src");
    }

    #[test]
    fn new_rename_delete_follow_the_tree() {
        let mut p = tour();
        p.fs.write("src/util.rs", "").unwrap();
        p.tree.invalidate("src");
        p.refresh();
        assert!(p.tree.rows.iter().any(|r| r.path == "src/util.rs"));
        p.fs.rename("src/util.rs", "src/helpers.rs").unwrap();
        p.tree.invalidate("src");
        p.refresh();
        assert!(p.tree.rows.iter().any(|r| r.path == "src/helpers.rs"));
        assert!(!p.tree.rows.iter().any(|r| r.path == "src/util.rs"));
        p.remove("src", true).unwrap();
        p.refresh();
        assert!(!p.tree.rows.iter().any(|r| r.path.starts_with("src")));
        assert_eq!(p.all_files(100), ["Cargo.toml", "README.md"]);
    }

    #[test]
    fn names_stay_inside_the_project() {
        assert_eq!(clean_relative(" src/new.rs ").unwrap(), "src/new.rs");
        assert_eq!(clean_relative("./a//b").unwrap(), "a/b");
        assert!(clean_relative("../etc/passwd").is_err());
        assert!(clean_relative("/abs").is_err());
        assert!(clean_relative("C:\\x").is_err());
        assert!(clean_relative("").is_err());
    }

    #[test]
    fn crate_names_from_folder_names() {
        assert_eq!(crate_name_from("My Project"), "my-project");
        assert_eq!(crate_name_from("2048 game"), "my-2048-game");
        assert_eq!(crate_name_from("..."), "my-project");
        assert!(valid_crate_name("hello_world"));
        assert!(!valid_crate_name("Hello"));
        assert!(!valid_crate_name("1x"));
        let files = new_project_files("demo");
        assert!(files[0].1.contains("name = \"demo\""));
        assert!(files[1].1.contains("fn main()"));
    }

    #[test]
    fn all_files_skips_build_output() {
        let mut m = MemFs::with(&[("src/main.rs", ""), ("target/debug/x", ""), ("a.txt", "")]);
        m.mkdir("target").unwrap();
        let mut p = Project::new("p", Box::new(m));
        assert_eq!(p.all_files(100), ["a.txt", "src/main.rs"]);
    }
}
