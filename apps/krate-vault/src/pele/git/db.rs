//! Ports diagrams/git/db.ts: the git graph the statements build.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::parser::{GitGraphAst, Statement};
use crate::json::Value;
use crate::pele::errors::{PeleError, PeleErrorCode};
use crate::pele::js::trim_start;
use crate::pele::quadrant::db::tighten_lines;

/// Ports `commitType`.
pub const NORMAL: u8 = 0;
pub const REVERSE: u8 = 1;
pub const HIGHLIGHT: u8 = 2;
pub const MERGE: u8 = 3;
pub const CHERRY_PICK: u8 = 4;

/// Ports `interface GitCommit`.
#[derive(Debug, Clone, PartialEq)]
pub struct GitCommit {
    pub id: String,
    pub message: String,
    pub seq: usize,
    pub kind: u8,
    pub tags: Vec<String>,
    pub parents: Vec<String>,
    pub branch: String,
    pub custom_type: Option<u8>,
    pub custom_id: Option<bool>,
}

/// Ports `interface GitModel`.
#[derive(Debug, Clone, PartialEq)]
pub struct GitModel {
    pub title: Option<String>,
    pub acc_title: Option<String>,
    pub acc_descr: Option<String>,
    pub direction: &'static str,
    /// Keyed by commit id, in the order the ids first appeared.
    pub commits: Vec<(String, GitCommit)>,
    /// The id of each branch's latest commit, or None while the branch has none.
    pub branches: Vec<(String, Option<String>)>,
    /// Branch names in drawing order: by `order`, then by creation.
    pub lanes: Vec<String>,
    pub current_branch: String,
    pub warnings: Vec<String>,
}

fn fail(message: String) -> PeleError {
    PeleError::of(message, PeleErrorCode::Semantic, "gitGraph")
}

/// A Map's `set`: a key keeps its first place.
fn set<V>(map: &mut Vec<(String, V)>, key: &str, value: V) {
    match map.iter_mut().find(|(k, _)| k == key) {
        Some(e) => e.1 = value,
        None => map.push((key.into(), value)),
    }
}

fn get<'a, V>(map: &'a [(String, V)], key: &str) -> Option<&'a V> {
    map.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// `(n >>> 0).toString(16)`.
fn hex(n: u32) -> String {
    format!("{:x}", n)
}

/// Ports `buildGit`.
pub fn build_git(
    ast: GitGraphAst,
    config: &Value,
    title: Option<&str>,
) -> Result<GitModel, PeleError> {
    let git = config.get("gitGraph");
    let main_name = match git.and_then(|g| g.get("mainBranchName")) {
        Some(Value::Str(s)) => s.clone(),
        _ => String::from("main"),
    };
    let main_order = match git.and_then(|g| g.get("mainBranchOrder")) {
        Some(Value::Num(n)) => *n,
        _ => 0.0,
    };

    let mut commits: Vec<(String, GitCommit)> = Vec::new();
    let mut branches: Vec<(String, Option<String>)> = alloc::vec![(main_name.clone(), None)];
    let mut orders: Vec<(String, f64)> = alloc::vec![(main_name.clone(), main_order)];
    let mut warnings: Vec<String> = Vec::new();
    let mut head: Option<String> = None;
    let mut current = main_name;
    let mut direction: &'static str = "LR";
    let mut seq: usize = 0;

    // Mermaid appends seven random hex digits to the sequence number. These digits are derived
    // from the sequence number instead, so the same text always gives the same ids.
    let next_id = |commits: &[(String, GitCommit)], seq: usize| -> String {
        let mut salt: u64 = seq as u64 + 1;
        loop {
            let v = (salt as u32).wrapping_mul(0x9e37_79b1);
            let h = format!("{:0>8}", hex(v));
            let id = format!("{}-{}", seq, &h[..7]);
            if get(commits, &id).is_none() {
                return id;
            }
            salt += 0x9e37_79b1;
        }
    };

    if let Some(dir) = ast.dir {
        direction = dir;
    }
    for statement in ast.statements {
        match statement {
            Statement::Commit {
                id,
                message,
                tags,
                kind,
            } => {
                let id = match id.filter(|i| !i.is_empty()) {
                    Some(i) => i,
                    None => next_id(&commits, seq),
                };
                let commit = GitCommit {
                    id: id.clone(),
                    message: message.unwrap_or_default(),
                    seq,
                    kind: kind.unwrap_or(NORMAL),
                    tags,
                    parents: head.iter().cloned().collect(),
                    branch: current.clone(),
                    custom_type: None,
                    custom_id: None,
                };
                seq += 1;
                if get(&commits, &id).is_some() {
                    warnings.push(format!("Commit ID {} already exists", id));
                }
                head = Some(id.clone());
                set(&mut commits, &id, commit);
                set(&mut branches, &current, Some(id));
            }
            Statement::Branch { name, order } => {
                if get(&branches, &name).is_some() {
                    return Err(fail(format!(
                        "Trying to create an existing branch. (Help: Either use a new name if you want create a new branch or try using \"checkout {}\")",
                        name
                    )));
                }
                set(&mut branches, &name, head.clone());
                set(&mut orders, &name, order.unwrap_or(0.0));
                checkout(&branches, &commits, &name, &mut current, &mut head)?;
            }
            Statement::Merge {
                branch: other,
                id: custom_id,
                tags,
                kind,
            } => {
                let custom_id = custom_id.unwrap_or_default();
                let current_head = get(&branches, &current).cloned().flatten();
                let other_head = get(&branches, &other).cloned().flatten();
                let current_commit = current_head
                    .as_ref()
                    .filter(|h| !h.is_empty())
                    .and_then(|h| get(&commits, h));
                let other_commit = other_head
                    .as_ref()
                    .filter(|h| !h.is_empty())
                    .and_then(|h| get(&commits, h));
                if let (Some(c), Some(_)) = (current_commit, other_commit) {
                    if c.branch == other {
                        return Err(fail(format!(
                            "Cannot merge branch '{}' into itself.",
                            other
                        )));
                    }
                }
                if current == other {
                    return Err(fail(
                        "Incorrect usage of \"merge\". Cannot merge a branch to itself".into(),
                    ));
                }
                let Some(current_commit) = current_commit else {
                    return Err(fail(format!(
                        "Incorrect usage of \"merge\". Current branch ({})has no commits",
                        current
                    )));
                };
                if get(&branches, &other).is_none() {
                    return Err(fail(format!(
                        "Incorrect usage of \"merge\". Branch to be merged ({}) does not exist",
                        other
                    )));
                }
                let Some(other_commit) = other_commit else {
                    return Err(fail(format!(
                        "Incorrect usage of \"merge\". Branch to be merged ({}) has no commits",
                        other
                    )));
                };
                if core::ptr::eq(current_commit, other_commit) {
                    return Err(fail(
                        "Incorrect usage of \"merge\". Both branches have same head".into(),
                    ));
                }
                if !custom_id.is_empty() && get(&commits, &custom_id).is_some() {
                    return Err(fail(format!(
                        "Incorrect usage of \"merge\". Commit with id:{} already exists, use different custom id",
                        custom_id
                    )));
                }
                let id = if custom_id.is_empty() {
                    next_id(&commits, seq)
                } else {
                    custom_id.clone()
                };
                let commit = GitCommit {
                    id: id.clone(),
                    message: format!("merged branch {} into {}", other, current),
                    seq,
                    parents: match &head {
                        None => Vec::new(),
                        Some(h) => alloc::vec![h.clone(), other_head.clone().unwrap_or_default()],
                    },
                    branch: current.clone(),
                    kind: MERGE,
                    custom_type: kind,
                    custom_id: Some(!custom_id.is_empty()),
                    tags,
                };
                seq += 1;
                head = Some(id.clone());
                set(&mut commits, &id, commit);
                set(&mut branches, &current, Some(id));
            }
            Statement::CherryPicking { id, tags, parent } => {
                let tags = if tags.is_empty() { None } else { Some(tags) };
                let source = id
                    .as_ref()
                    .filter(|i| !i.is_empty())
                    .and_then(|i| get(&commits, i))
                    .cloned();
                let Some(source) = source else {
                    return Err(fail(
                        "Incorrect usage of \"cherryPick\". Source commit id should exist and provided"
                            .into(),
                    ));
                };
                let parent = parent.filter(|p| !p.is_empty());
                if let Some(p) = &parent {
                    if !source.parents.contains(p) {
                        return Err(fail("Invalid operation: The specified parent commit is not an immediate parent of the cherry-picked commit.".into()));
                    }
                }
                if source.kind == MERGE && parent.is_none() {
                    return Err(fail("Incorrect usage of cherry-pick: If the source commit is a merge commit, an immediate parent commit must be specified.".into()));
                }
                if source.branch == current {
                    return Err(fail(
                        "Incorrect usage of \"cherryPick\". Source commit is already on current branch"
                            .into(),
                    ));
                }
                let current_head = get(&branches, &current).cloned().flatten();
                if !current_head
                    .as_ref()
                    .is_some_and(|h| !h.is_empty() && get(&commits, h).is_some())
                {
                    return Err(fail(format!(
                        "Incorrect usage of \"cherry-pick\". Current branch ({})has no commits",
                        current
                    )));
                }
                let id = next_id(&commits, seq);
                let commit = GitCommit {
                    id: id.clone(),
                    message: format!("cherry-picked {} into {}", source.message, current),
                    seq,
                    parents: match &head {
                        None => Vec::new(),
                        Some(h) => alloc::vec![h.clone(), source.id.clone()],
                    },
                    branch: current.clone(),
                    kind: CHERRY_PICK,
                    custom_type: None,
                    custom_id: None,
                    tags: match tags {
                        Some(t) => t.into_iter().filter(|t| !t.is_empty()).collect(),
                        None => alloc::vec![format!(
                            "cherry-pick:{}{}",
                            source.id,
                            if source.kind == MERGE {
                                format!("|parent:{}", parent.clone().unwrap_or_default())
                            } else {
                                String::new()
                            }
                        )],
                    },
                };
                seq += 1;
                head = Some(id.clone());
                set(&mut commits, &id, commit);
                set(&mut branches, &current, Some(id));
            }
            Statement::Checkout { branch } => {
                checkout(&branches, &commits, &branch, &mut current, &mut head)?;
            }
        }
    }

    // A branch made without an order sorts by creation among those of order zero, as in Mermaid.
    let mut lanes: Vec<(String, f64)> = orders;
    lanes.sort_by(|a, b| {
        (a.1 - b.1)
            .partial_cmp(&0.0)
            .unwrap_or(core::cmp::Ordering::Equal)
    });

    Ok(GitModel {
        title: match ast.title.filter(|t| !t.is_empty()) {
            Some(t) => Some(t),
            None => title.map(String::from),
        },
        acc_title: ast
            .acc_title
            .filter(|t| !t.is_empty())
            .map(|t| String::from(trim_start(&t))),
        acc_descr: ast
            .acc_descr
            .filter(|t| !t.is_empty())
            .map(|t| tighten_lines(&t)),
        direction,
        commits,
        branches,
        lanes: lanes.into_iter().map(|(n, _)| n).collect(),
        current_branch: current,
        warnings,
    })
}

/// Ports `checkout`.
fn checkout(
    branches: &[(String, Option<String>)],
    commits: &[(String, GitCommit)],
    branch: &str,
    current: &mut String,
    head: &mut Option<String>,
) -> Result<(), PeleError> {
    let Some(id) = get(branches, branch) else {
        return Err(fail(format!(
            "Trying to checkout branch which is not yet created. (Help try using \"branch {}\")",
            branch
        )));
    };
    *current = branch.into();
    *head = id
        .as_ref()
        .filter(|i| !i.is_empty())
        .and_then(|i| get(commits, i))
        .map(|c| c.id.clone());
    Ok(())
}
