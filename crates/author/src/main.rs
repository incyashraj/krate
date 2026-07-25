//! `krate-author` — write a generated Krate app to disk from a request.
//!
//! This is the entry the authoring loop calls for its first step. It builds a
//! request (from flags or a sensible default), generates the app with the
//! library, writes every file under the target directory, and prints a JSON
//! record of what it produced. The shell wrapper (`scripts/author-krate.sh`)
//! folds that record into the full transcript alongside the build, pack, and
//! run steps.
//!
//! Usage:
//!   krate-author --out <dir> --sdk-prefix <rel> [--name <kebab>]
//!                [--read-glob <./glob/**>] [--top-n <N>]
//!
//! It writes files and prints JSON; it does not build or run anything. Keeping
//! side effects to "write files, print JSON" is what lets a real LLM replace
//! this step by writing the same files itself.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use krate_author::{generate, AppRequest};
use serde_json::json;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("krate-author: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out: Option<PathBuf> = None;
    let mut sdk_prefix: Option<String> = None;
    let mut name = "word-count".to_string();
    let mut read_glob: Option<String> = None;
    let mut top_n: Option<u32> = None;

    let mut i = 0;
    while i < args.len() {
        let flag = &args[i];
        let mut take_value = |name: &str| -> Result<String, String> {
            i += 1;
            args.get(i)
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "--out" => out = Some(PathBuf::from(take_value("--out")?)),
            "--sdk-prefix" => sdk_prefix = Some(take_value("--sdk-prefix")?),
            "--name" => name = take_value("--name")?,
            "--read-glob" => read_glob = Some(take_value("--read-glob")?),
            "--top-n" => {
                let raw = take_value("--top-n")?;
                top_n = Some(
                    raw.parse()
                        .map_err(|_| format!("invalid --top-n {raw:?}"))?,
                );
            }
            other => return Err(format!("unknown flag {other}")),
        }
        i += 1;
    }

    let out = out.ok_or("--out <dir> is required")?;
    let sdk_prefix = sdk_prefix.ok_or("--sdk-prefix <rel> is required")?;

    let mut request = AppRequest::word_frequency(&name);
    if let Some(glob) = read_glob {
        request.read_glob = glob;
    }
    if let Some(n) = top_n {
        request.top_n = n;
    }

    let app = generate(&request, &sdk_prefix)?;

    // Write every generated file, creating parent directories as needed.
    let mut written = Vec::new();
    for file in &app.files {
        let dest = out.join(&file.path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("create {}: {err}", parent.display()))?;
        }
        std::fs::write(&dest, &file.contents)
            .map_err(|err| format!("write {}: {err}", dest.display()))?;
        written.push(dest);
    }

    // The author step's transcript record. The wrapper reads this on stdout.
    let record = json!({
        "schema": "krate.author.v1",
        "step": "author",
        "request": request,
        "out_dir": path_string(&out),
        "files": app
            .files
            .iter()
            .map(|f| json!({ "path": f.path, "bytes": f.contents.len() }))
            .collect::<Vec<_>>(),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&record).map_err(|err| err.to_string())?
    );
    Ok(())
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
