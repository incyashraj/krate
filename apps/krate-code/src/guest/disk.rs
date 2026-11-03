//! A folder the person picked, through `picked/<token>/`.
//!
//! The pick is the grant: every call below is an ordinary `krate::fs` call
//! under that prefix, so the manifest asks for no fs capability at all and
//! the editor can reach nothing the person did not choose, for this run only.

use crate::project::{Entry, Vfs};
use alloc::string::String;
use alloc::vec::Vec;
use krate::fs;

pub struct DiskFs {
    root: String,
    watch: Option<u64>,
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

impl DiskFs {
    pub fn new(token: &str) -> Self {
        let mut root = String::from("picked/");
        root.push_str(token);
        let watch = fs::watch::start(&root).ok();
        DiskFs { root, watch }
    }

    fn at(&self, path: &str) -> String {
        let mut s = self.root.clone();
        if !path.is_empty() {
            s.push('/');
            s.push_str(path);
        }
        s
    }
}

impl Vfs for DiskFs {
    fn is_real(&self) -> bool {
        true
    }

    fn list(&mut self, dir: &str) -> Result<Vec<Entry>, String> {
        let names = fs::list(&self.at(dir)).map_err(why)?;
        let mut out = Vec::new();
        for raw in names {
            let name = String::from(raw.trim_end_matches('/'));
            if name.is_empty() || name == "." || name == ".." {
                continue;
            }
            let mut p = String::from(dir);
            if !p.is_empty() {
                p.push('/');
            }
            p.push_str(&name);
            let is_dir =
                raw.ends_with('/') || fs::stat(&self.at(&p)).map(|s| s.is_dir).unwrap_or(false);
            out.push(Entry { name, is_dir });
        }
        Ok(out)
    }

    fn read(&mut self, path: &str) -> Result<String, String> {
        let bytes = fs::read(&self.at(path)).map_err(why)?;
        String::from_utf8(bytes).map_err(|_| String::from("it is not UTF-8 text"))
    }

    fn write(&mut self, path: &str, text: &str) -> Result<(), String> {
        fs::write(&self.at(path), text.as_bytes()).map_err(why)
    }

    fn mkdir(&mut self, path: &str) -> Result<(), String> {
        fs::mkdir(&self.at(path)).map_err(why)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        if self.exists(to) {
            return Err(String::from("Something with that name is already there"));
        }
        fs::rename(&self.at(from), &self.at(to)).map_err(why)
    }

    fn remove_file(&mut self, path: &str) -> Result<(), String> {
        fs::remove_file(&self.at(path)).map_err(why)
    }

    fn remove_dir(&mut self, path: &str) -> Result<(), String> {
        fs::remove_dir(&self.at(path)).map_err(why)
    }

    fn exists(&mut self, path: &str) -> bool {
        fs::stat(&self.at(path)).is_ok()
    }

    fn changed(&mut self) -> bool {
        match self.watch {
            Some(w) => fs::watch::changes(w)
                .map(|c| !c.is_empty())
                .unwrap_or(false),
            None => false,
        }
    }
}
