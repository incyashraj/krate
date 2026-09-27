// The smallest app that watches a folder (krate:fs/watch), so the
// capability is proved through the binary a person runs.
//
//   krate run watch-probe.krate -- <folder>
//
// It starts a watch, then creates, changes and removes one file itself,
// asking `changes` after each, and prints one line per look -- so a test
// sees exactly what a watch reports, and that a look too soon is empty.
#![no_std]
extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use krate::{
    fs::{
        self,
        watch::{self, ChangeKind},
        FsError,
    },
    io::{
        args, stdio,
        streams::OutputStreamExt,
    },
    time::sleep::sleep_millis,
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        let raw = args::raw();
        let folder = raw.split('\n').find(|w| !w.is_empty()).unwrap_or("inbox");
        let out = stdio::stdout();
        let say = |line: &str| out.write_line(line).is_ok() && out.flush().is_ok();

        let id = match watch::start(folder) {
            Ok(id) => id,
            Err(err) => {
                say(&["start=", describe(&err)].concat());
                return 0;
            }
        };
        let file = [folder, "/note.txt"].concat();
        let steps: [(&str, &dyn Fn() -> Result<(), FsError>); 3] = [
            ("created", &|| fs::write(&file, b"one")),
            ("modified", &|| fs::write(&file, b"one and two")),
            ("removed", &|| fs::remove_file(&file)),
        ];
        for (label, act) in steps {
            if let Err(err) = act() {
                say(&[label, "=act-failed ", describe(&err)].concat());
                return 0;
            }
            // Too soon: a look within a quarter second is empty by design.
            let soon = watch::changes(id).map(|c| c.len()).unwrap_or(usize::MAX);
            sleep_millis(300);
            let line = match watch::changes(id) {
                Ok(changes) => {
                    let seen: Vec<String> = changes
                        .iter()
                        .map(|c| {
                            let kind = match c.kind {
                                ChangeKind::Created => "created",
                                ChangeKind::Modified => "modified",
                                ChangeKind::Removed => "removed",
                            };
                            [kind, " ", &c.path].concat()
                        })
                        .collect();
                    let soon = if soon == 0 { "soon=empty" } else { "soon=NOT-EMPTY" };
                    [label, ": ", soon, " then ", &seen.join(", ")].concat()
                }
                Err(err) => [label, "=error ", describe(&err)].concat(),
            };
            if !say(&line) {
                return 20;
            }
        }
        watch::stop(id);
        let after = match watch::changes(id) {
            Err(FsError::NotFound) => "stopped=gone",
            _ => "stopped=STILL-WATCHING",
        };
        say(after);
        0
    }
}

fn describe(err: &FsError) -> &'static str {
    match err {
        FsError::NotFound => "not-found",
        FsError::PermissionDenied => "permission-denied",
        FsError::AlreadyExists => "already-exists",
        FsError::InvalidPath => "invalid-path",
        FsError::NotADirectory => "not-a-directory",
        FsError::IsADirectory => "is-a-directory",
        FsError::Io(_) => "io",
    }
}

krate::export!(Component);
