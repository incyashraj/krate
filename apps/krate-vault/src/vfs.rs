//! The seam between the vault and wherever its files live.
//!
//! On a computer that is the folder the person picked (the guest's
//! `DiskFs`); for the sample vault, the `quick` run and every test it is
//! [`MemFs`], a folder held in memory. The vault never knows which.
//! Paths are relative to the vault's root, `/`-separated, with no leading
//! slash; `""` is the root itself.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

pub trait Vfs {
    /// Whether this is a real folder on disk (the sample vault is not).
    fn real(&self) -> bool;
    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, String>;
    fn read(&mut self, path: &str) -> Result<String, String>;
    fn write(&mut self, path: &str, text: &str) -> Result<(), String>;
    fn mkdir(&mut self, path: &str) -> Result<(), String>;
    fn rename(&mut self, from: &str, to: &str) -> Result<(), String>;
    fn remove_file(&mut self, path: &str) -> Result<(), String>;
    fn exists(&mut self, path: &str) -> bool;
    /// A file's bytes, for the pictures a note embeds. A folder held in
    /// memory holds only text.
    fn read_bytes(&mut self, path: &str) -> Result<Vec<u8>, String> {
        self.read(path).map(String::into_bytes)
    }
    /// Where Krate's video player finds a file of the vault (for audio and
    /// video embeds), when the files are on disk.
    fn media_path(&self, _path: &str) -> Option<String> {
        None
    }
    /// Move a file or folder to the computer's trash. A folder in memory
    /// has none, so the vault uses its own `.trash` instead.
    fn trash(&mut self, _path: &str) -> Result<(), String> {
        Err(String::from("there is no system trash here"))
    }
    /// Remove an empty folder.
    fn remove_dir(&mut self, path: &str) -> Result<(), String> {
        let _ = path;
        Err(String::from("folders cannot be removed here"))
    }
    /// Write bytes (an attachment). A folder in memory holds text only.
    fn write_bytes(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        match core::str::from_utf8(bytes) {
            Ok(t) => self.write(path, t),
            Err(_) => Err(String::from("only text can be kept here")),
        }
    }
    /// What changed on disk since the last ask, outside the app, as paths
    /// in the vault. A folder in memory never changes behind the app's back.
    fn outside_changes(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// Join a folder and a name.
pub fn join(dir: &str, name: &str) -> String {
    let mut s = String::from(dir);
    if !s.is_empty() {
        s.push('/');
    }
    s.push_str(name);
    s
}

/// The folder part of a path (`""` for the root).
pub fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => path.get(..i).unwrap_or(""),
        None => "",
    }
}

/// The last part of a path.
pub fn file_name(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => path.get(i + 1..).unwrap_or(path),
        None => path,
    }
}

/// A folder held in memory.
#[derive(Debug, Clone, Default)]
pub struct MemFs {
    pub files: BTreeMap<String, String>,
    pub dirs: BTreeSet<String>,
}

impl MemFs {
    pub fn new() -> Self {
        MemFs::default()
    }

    /// Add a file, and every folder above it.
    pub fn put(&mut self, path: &str, text: &str) {
        let mut dir = parent(path);
        while !dir.is_empty() {
            self.dirs.insert(String::from(dir));
            dir = parent(dir);
        }
        self.files.insert(String::from(path), String::from(text));
    }
}

impl Vfs for MemFs {
    fn real(&self) -> bool {
        false
    }

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, String> {
        if !dir.is_empty() && !self.dirs.contains(dir) {
            return Err(String::from("not found"));
        }
        let mut out = Vec::new();
        let direct = |p: &str| -> Option<String> {
            let rest = if dir.is_empty() {
                p
            } else {
                p.strip_prefix(dir)?.strip_prefix('/')?
            };
            (!rest.is_empty() && !rest.contains('/')).then(|| String::from(rest))
        };
        for d in &self.dirs {
            if let Some(name) = direct(d) {
                out.push(Entry { name, is_dir: true });
            }
        }
        for f in self.files.keys() {
            if let Some(name) = direct(f) {
                out.push(Entry {
                    name,
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
            .ok_or_else(|| String::from("not found"))
    }

    fn write(&mut self, path: &str, text: &str) -> Result<(), String> {
        if self.dirs.contains(path) {
            return Err(String::from("that is a folder, not a file"));
        }
        self.put(path, text);
        Ok(())
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        if self.files.contains_key(path) {
            return Err(String::from("already exists"));
        }
        let mut dir = path;
        while !dir.is_empty() {
            self.dirs.insert(String::from(dir));
            dir = parent(dir);
        }
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        if self.exists(to) {
            return Err(String::from("Something with that name is already there"));
        }
        if let Some(text) = self.files.remove(from) {
            self.put(to, &text);
            return Ok(());
        }
        if !self.dirs.contains(from) {
            return Err(String::from("not found"));
        }
        let prefix = {
            let mut p = String::from(from);
            p.push('/');
            p
        };
        let moved_dirs: Vec<String> = self
            .dirs
            .iter()
            .filter(|d| d.as_str() == from || d.starts_with(&prefix))
            .cloned()
            .collect();
        for d in moved_dirs {
            self.dirs.remove(&d);
            let mut n = String::from(to);
            n.push_str(d.get(from.len()..).unwrap_or(""));
            self.dirs.insert(n);
        }
        let moved_files: Vec<String> = self
            .files
            .keys()
            .filter(|f| f.starts_with(&prefix))
            .cloned()
            .collect();
        for f in moved_files {
            if let Some(text) = self.files.remove(&f) {
                let mut n = String::from(to);
                n.push_str(f.get(from.len()..).unwrap_or(""));
                self.put(&n, &text);
            }
        }
        let mut dir = parent(to);
        while !dir.is_empty() {
            self.dirs.insert(String::from(dir));
            dir = parent(dir);
        }
        Ok(())
    }

    fn remove_file(&mut self, path: &str) -> Result<(), String> {
        self.files
            .remove(path)
            .map(|_| ())
            .ok_or_else(|| String::from("not found"))
    }

    fn exists(&mut self, path: &str) -> bool {
        path.is_empty() || self.files.contains_key(path) || self.dirs.contains(path)
    }

    fn remove_dir(&mut self, path: &str) -> Result<(), String> {
        if self.dirs.remove(path) {
            Ok(())
        } else {
            Err(String::from("not found"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_split_and_join() {
        assert_eq!(join("", "a.md"), "a.md");
        assert_eq!(join("x/y", "a.md"), "x/y/a.md");
        assert_eq!(parent("x/y/a.md"), "x/y");
        assert_eq!(parent("a.md"), "");
        assert_eq!(file_name("x/y/a.md"), "a.md");
    }

    #[test]
    fn memfs_lists_renames_folders_with_their_files() {
        let mut fs = MemFs::new();
        fs.put("a/b/one.md", "1");
        fs.put("a/two.md", "2");
        fs.put("root.md", "r");
        let root = fs.list("").unwrap();
        assert!(root.iter().any(|e| e.name == "a" && e.is_dir));
        assert!(root.iter().any(|e| e.name == "root.md" && !e.is_dir));
        fs.rename("a", "z/a").unwrap();
        assert_eq!(fs.read("z/a/b/one.md").unwrap(), "1");
        assert!(!fs.exists("a/two.md"));
        assert!(fs.list("z").unwrap().iter().any(|e| e.name == "a"));
        assert!(fs.rename("root.md", "z/a/two.md").is_err());
    }
}
