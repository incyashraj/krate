//! A vault on disk, through Krate's fs calls.
//!
//! Two ways in. A folder the person picked is reached through
//! `picked/<token>/`: the pick is the grant, so the manifest asks for no
//! fs capability at all. On a computer Krate keeps the pick for this app,
//! and Vault reopens it on the next run from the saved token. An automated run names a
//! folder inside the app's own folder instead (`vault=./vault`), which
//! needs the optional `fs.*:./vault/**` grants a run gives by name.

use crate::vfs::{Entry, Vfs};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use krate::bindings::krate::fs::files::{self, OpenMode};
use krate::fs;

/// The most one read or write call carries. Krate 0.6.1 refused a single
/// request over 8 MiB (K-1335, fixed since), and a 200,000-line note is
/// bigger than that, so Vault sends pieces either way.
const CHUNK: usize = 4 << 20;
/// How often to look for changes made outside the app. The runtime's
/// watch is a scan of the folder, so this is a cost paid while idle.
pub const WATCH_EVERY_MS: u64 = 3_000;

pub struct DiskFs {
    root: String,
    watch: Option<u64>,
    /// Paths this app itself changed lately: their changes are not news.
    own: BTreeMap<String, u64>,
}

fn why(e: fs::FsError) -> String {
    use fs::FsError::*;
    match e {
        NotFound => String::from("not found"),
        PermissionDenied => String::from("Krate did not allow that here"),
        AlreadyExists => String::from("already exists"),
        InvalidPath => String::from("that name is not allowed"),
        NotADirectory => String::from("that is a file, not a folder"),
        IsADirectory => String::from("that is a folder, not a file"),
        Io(m) => m,
    }
}

/// Names that are certainly files, so listing a big folder does not have
/// to ask the host about each one.
fn surely_file(name: &str) -> bool {
    let Some(dot) = name.rfind('.') else {
        return false;
    };
    let ext = name.get(dot + 1..).unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "md" | "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "svg"
            | "pdf"
            | "canvas"
            | "json"
            | "txt"
            | "csv"
            | "mp3"
            | "mp4"
            | "m4a"
            | "wav"
            | "mov"
            | "webm"
            | "css"
            | "js"
            | "html"
            | "base"
    )
}

impl DiskFs {
    /// The folder the person picked.
    pub fn picked(token: &str) -> DiskFs {
        let mut root = String::from("picked/");
        root.push_str(token);
        DiskFs::rooted(root)
    }

    /// A folder inside the app's own folder (automated runs).
    pub fn at(path: &str) -> DiskFs {
        let p = path.trim_end_matches('/');
        DiskFs::rooted(String::from(if p.is_empty() { "." } else { p }))
    }

    fn rooted(root: String) -> DiskFs {
        let watch = fs::watch::start(&root).ok();
        DiskFs {
            root,
            watch,
            own: BTreeMap::new(),
        }
    }

    fn at_path(&self, path: &str) -> String {
        let mut s = self.root.clone();
        if !path.is_empty() {
            s.push('/');
            s.push_str(path);
        }
        s
    }

    fn mine(&mut self, path: &str) {
        self.own
            .insert(String::from(path), krate::time::now_millis());
    }
}

impl Vfs for DiskFs {
    fn real(&self) -> bool {
        true
    }

    /// The changes made outside the app since the last look, as paths in
    /// the vault. Empty when there are none (or no watch).
    fn outside_changes(&mut self) -> Vec<String> {
        let Some(w) = self.watch else {
            return Vec::new();
        };
        let Ok(changes) = fs::watch::changes(w) else {
            return Vec::new();
        };
        let now = krate::time::now_millis();
        self.own
            .retain(|_, at| now.saturating_sub(*at) < 3 * WATCH_EVERY_MS);
        let mut prefix = self.root.clone();
        prefix.push('/');
        let mut out = Vec::new();
        for c in changes {
            let p = c.path.as_str();
            let p = p.strip_prefix(prefix.as_str()).unwrap_or(p);
            let p = p.strip_prefix("./").unwrap_or(p);
            if p.starts_with('.') || p.contains("/.") {
                continue;
            }
            if self.own.contains_key(p) {
                continue;
            }
            out.push(String::from(p));
        }
        out
    }

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, String> {
        let names = fs::list(&self.at_path(dir)).map_err(why)?;
        let mut out = Vec::with_capacity(names.len());
        for raw in names {
            let name = String::from(raw.trim_end_matches('/'));
            if name.is_empty() || name == "." || name == ".." {
                continue;
            }
            let is_dir = if raw.ends_with('/') {
                true
            } else if name.starts_with('.') || surely_file(&name) {
                false
            } else {
                let mut p = String::from(dir);
                if !p.is_empty() {
                    p.push('/');
                }
                p.push_str(&name);
                fs::stat(&self.at_path(&p))
                    .map(|s| s.is_dir)
                    .unwrap_or(false)
            };
            out.push(Entry { name, is_dir });
        }
        Ok(out)
    }

    fn read(&mut self, path: &str) -> Result<String, String> {
        let file = files::open(&self.at_path(path), OpenMode::Read).map_err(why)?;
        let mut bytes: Vec<u8> = Vec::new();
        loop {
            let chunk = file.read(CHUNK as u32).map_err(why)?;
            if chunk.is_empty() {
                break;
            }
            if bytes.is_empty() {
                bytes = chunk;
            } else {
                bytes.extend_from_slice(&chunk);
            }
        }
        String::from_utf8(bytes).map_err(|_| String::from("it is not UTF-8 text"))
    }

    fn write(&mut self, path: &str, text: &str) -> Result<(), String> {
        self.mine(path);
        let file = files::open(&self.at_path(path), OpenMode::Write).map_err(why)?;
        let bytes = text.as_bytes();
        let mut done = 0usize;
        while done < bytes.len() {
            let end = (done + CHUNK).min(bytes.len());
            let piece = bytes.get(done..end).unwrap_or(&[]);
            let n = file.write(piece).map_err(why)? as usize;
            if n == 0 {
                return Err(String::from("the disk took none of the note"));
            }
            done += n;
        }
        Ok(())
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        self.mine(path);
        fs::mkdir(&self.at_path(path)).map_err(why)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        if fs::stat(&self.at_path(to)).is_ok() && !from.eq_ignore_ascii_case(to) {
            return Err(String::from("Something with that name is already there"));
        }
        self.mine(from);
        self.mine(to);
        fs::rename(&self.at_path(from), &self.at_path(to)).map_err(why)
    }

    fn remove_file(&mut self, path: &str) -> Result<(), String> {
        self.mine(path);
        fs::remove_file(&self.at_path(path)).map_err(why)
    }

    fn exists(&mut self, path: &str) -> bool {
        fs::stat(&self.at_path(path)).is_ok()
    }
}
