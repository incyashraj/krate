//! Bookmarks: notes, headings, folders and searches kept in a list, in
//! groups, in `.obsidian/bookmarks.json` in the notes app's format.

use crate::json::Value;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    File { path: String },
    Heading { path: String, heading: String },
    Folder { path: String },
    Search { query: String },
    Group { items: Vec<Bookmark> },
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub kind: Kind,
    pub title: Option<String>,
    pub ctime: u64,
    raw: Value,
}

impl Bookmark {
    fn from_json(v: &Value) -> Bookmark {
        let s = |k: &str| String::from(v.get_str(k).unwrap_or(""));
        let kind = match v.get_str("type").unwrap_or("") {
            "file" => match v.get_str("subpath") {
                Some(sub) if sub.starts_with('#') && !sub.starts_with("#^") => Kind::Heading {
                    path: s("path"),
                    heading: String::from(sub.trim_start_matches('#')),
                },
                _ => Kind::File { path: s("path") },
            },
            "folder" => Kind::Folder { path: s("path") },
            "search" => Kind::Search { query: s("query") },
            "group" => Kind::Group {
                items: v
                    .get("items")
                    .map(|a| a.arr().iter().map(Bookmark::from_json).collect())
                    .unwrap_or_default(),
            },
            _ => Kind::Other,
        };
        Bookmark {
            kind,
            title: v.get_str("title").map(String::from),
            ctime: v.get_num("ctime").unwrap_or(0.0) as u64,
            raw: v.clone(),
        }
    }

    fn to_json(&self) -> Value {
        let mut v = self.raw.clone();
        let t = |v: &mut Value, ty: &str| v.set("type", Value::s(ty));
        v.set("ctime", Value::Num(self.ctime as f64));
        match &self.kind {
            Kind::File { path } => {
                t(&mut v, "file");
                v.set("path", Value::s(path));
            }
            Kind::Heading { path, heading } => {
                t(&mut v, "file");
                v.set("path", Value::s(path));
                let mut sub = String::from("#");
                sub.push_str(heading);
                v.set("subpath", Value::s(&sub));
            }
            Kind::Folder { path } => {
                t(&mut v, "folder");
                v.set("path", Value::s(path));
            }
            Kind::Search { query } => {
                t(&mut v, "search");
                v.set("query", Value::s(query));
            }
            Kind::Group { items } => {
                t(&mut v, "group");
                v.set(
                    "items",
                    Value::Arr(items.iter().map(Bookmark::to_json).collect()),
                );
            }
            Kind::Other => {}
        }
        match &self.title {
            Some(title) => v.set("title", Value::s(title)),
            None => v.remove("title"),
        }
        v
    }

    pub fn new(kind: Kind, now: u64) -> Bookmark {
        Bookmark {
            kind,
            title: None,
            ctime: now,
            raw: Value::obj(),
        }
    }

    /// What the list shows for it.
    pub fn label(&self) -> String {
        if let Some(t) = &self.title {
            return t.clone();
        }
        match &self.kind {
            Kind::File { path } | Kind::Folder { path } => {
                let n = crate::vfs::file_name(path);
                String::from(n.strip_suffix(".md").unwrap_or(n))
            }
            Kind::Heading { path, heading } => {
                let n = crate::vfs::file_name(path);
                let mut s = String::from(n.strip_suffix(".md").unwrap_or(n));
                s.push_str(" > ");
                s.push_str(heading);
                s
            }
            Kind::Search { query } => query.clone(),
            Kind::Group { .. } => String::from("Group"),
            Kind::Other => String::from("Bookmark"),
        }
    }
}

/// The whole list, as `bookmarks.json` holds it.
pub fn read(v: &Value) -> Vec<Bookmark> {
    v.get("items")
        .map(|a| a.arr().iter().map(Bookmark::from_json).collect())
        .unwrap_or_default()
}

pub fn write(list: &[Bookmark], into: &mut Value) {
    into.set(
        "items",
        Value::Arr(list.iter().map(Bookmark::to_json).collect()),
    );
}

/// Every bookmark in order with its depth (groups open), flattened for a
/// list on screen; `open` says whether a group (by its place) is open.
pub fn flatten<'a>(
    list: &'a [Bookmark],
    depth: u16,
    prefix: &mut Vec<usize>,
    closed: &dyn Fn(&[usize]) -> bool,
    out: &mut Vec<(Vec<usize>, u16, &'a Bookmark)>,
) {
    for (i, b) in list.iter().enumerate() {
        prefix.push(i);
        out.push((prefix.clone(), depth, b));
        if let Kind::Group { items } = &b.kind {
            if !closed(prefix) {
                flatten(items, depth + 1, prefix, closed, out);
            }
        }
        prefix.pop();
    }
}

/// The bookmark at a place in the tree.
pub fn at<'a>(list: &'a [Bookmark], place: &[usize]) -> Option<&'a Bookmark> {
    let (first, rest) = place.split_first()?;
    let b = list.get(*first)?;
    if rest.is_empty() {
        return Some(b);
    }
    match &b.kind {
        Kind::Group { items } => at(items, rest),
        _ => None,
    }
}

/// Take a bookmark out by its place.
pub fn remove(list: &mut Vec<Bookmark>, place: &[usize]) -> Option<Bookmark> {
    let (first, rest) = place.split_first()?;
    if rest.is_empty() {
        return (*first < list.len()).then(|| list.remove(*first));
    }
    match &mut list.get_mut(*first)?.kind {
        Kind::Group { items } => remove(items, rest),
        _ => None,
    }
}

/// Whether a note (or one of its headings) is bookmarked anywhere.
pub fn has_file(list: &[Bookmark], path: &str) -> bool {
    list.iter().any(|b| match &b.kind {
        Kind::File { path: p } => p == path,
        Kind::Group { items } => has_file(items, path),
        _ => false,
    })
}

/// Paths follow a rename.
pub fn moved(list: &mut [Bookmark], from: &str, to: &str) -> bool {
    let mut any = false;
    for b in list.iter_mut() {
        match &mut b.kind {
            Kind::File { path } | Kind::Folder { path } | Kind::Heading { path, .. }
                if path == from =>
            {
                *path = String::from(to);
                any = true;
            }
            Kind::Group { items } => any |= moved(items, from, to),
            _ => {}
        }
    }
    any
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_apps_bookmarks_file_round_trips() {
        let text = "{\"items\":[{\"type\":\"file\",\"ctime\":1,\"path\":\"Home.md\"},{\"type\":\"group\",\"ctime\":2,\"items\":[{\"type\":\"search\",\"ctime\":3,\"query\":\"tag:#task\"},{\"type\":\"file\",\"ctime\":4,\"path\":\"Plan.md\",\"subpath\":\"#Goals\"}],\"title\":\"Work\"},{\"type\":\"folder\",\"ctime\":5,\"path\":\"Projects\"}]}";
        let v = crate::json::parse(text).unwrap();
        let mut list = read(&v);
        assert_eq!(list.len(), 3);
        assert_eq!(list[1].label(), "Work");
        let mut out = Vec::new();
        flatten(&list, 0, &mut Vec::new(), &|_| false, &mut out);
        let labels: Vec<String> = out.iter().map(|(_, _, b)| b.label()).collect();
        assert_eq!(
            labels,
            ["Home", "Work", "tag:#task", "Plan > Goals", "Projects"]
        );
        assert_eq!(out[3].1, 1);
        let mut again = Value::obj();
        write(&list, &mut again);
        assert_eq!(crate::json::compact(&again), text);
        assert!(moved(&mut list, "Plan.md", "Roadmap.md"));
        assert_eq!(at(&list, &[1, 1]).unwrap().label(), "Roadmap > Goals");
        list.push(Bookmark::new(
            Kind::File {
                path: String::from("New.md"),
            },
            9,
        ));
        assert!(has_file(&list, "New.md"));
        remove(&mut list, &[1, 0]);
        assert_eq!(at(&list, &[1, 0]).unwrap().label(), "Roadmap > Goals");
    }
}
