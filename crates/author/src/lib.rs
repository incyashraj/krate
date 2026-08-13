//! The authoring harness: a request in, a complete Krate guest crate out.
//!
//! This is the deterministic core of the AI authoring loop. It stands in for
//! the "a coding agent writes the app" step: given a plain request, it emits
//! every file a buildable Krate app needs — `Cargo.toml`, `src/lib.rs`, and a
//! `manifest.toml` that declares only the capabilities the app actually uses.
//!
//! Keeping this a pure function of the request (no filesystem, no toolchain)
//! makes the "author" step unit-testable and reproducible, and lets a real LLM
//! substitute for `generate` without touching the build/pack/verify steps that
//! follow in `scripts/author-krate.sh`.
//!
//! Built-in kinds include a word-frequency reporter, a persistent checklist,
//! and a voice-activated teleprompter. Each declares only the capabilities it uses, and each
//! has a required capability whose grant gates it — `fs.read` for the reporter,
//! `fs.write` for the checklist — so the packaged `.krate` has a real
//! permission wall to prove: run it with the grant and it works; withhold the
//! grant and it refuses before doing anything, like every other Krate app.

use serde::{Deserialize, Serialize};

pub mod feasibility;

/// What kind of app the agent was asked to build. The enum is the seam where
/// more request types slot in without reshaping the pipeline around them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppKind {
    /// A CLI app: read a file and print its most frequent words.
    WordFrequency,
    /// A GUI app: a checklist with checkboxes that saves locally.
    Checklist,
    /// A GUI app: a microphone-driven teleprompter.
    VoicePrompter,
}

impl AppKind {
    /// Infer the app kind from a free-text request, so `krate create "make a
    /// grocery list…"` picks the right template without an explicit flag.
    ///
    /// The checklist is a GUI app that opens a window and manages a list of
    /// items; the word-frequency app is a CLI file reporter. Most plain-English
    /// "make me an app" requests are list/tracker style, so the checklist GUI is
    /// matched broadly (any "list", "checklist", "todo", or tracking of items)
    /// and the CLI reporter is reserved for requests that clearly want to read
    /// or analyze a file. Matching the GUI broadly also gives an `--author-cmd`
    /// agent the right starter to adapt, which is where authoring succeeds.
    /// Whether a request is best served by a windowed app or a command-line
    /// one. Used only to pick the skeleton world for AI authoring, where the
    /// choice sets the WIT wiring the agent should not have to redo.
    ///
    /// Leans GUI: a windowed app is the more useful and demo-friendly default,
    /// and most consumer requests ("a tip calculator", "a maze you can walk")
    /// are visual. Only a clear command-line signal -- printing to stdout,
    /// reading a file to a report, a pipe -- picks CLI.
    pub fn wants_gui(request: &str) -> Skeleton {
        let lower = request.to_lowercase();
        let cli_signals = [
            "command line",
            "command-line",
            "cli ",
            " cli",
            "stdout",
            "terminal",
            "print to the console",
            "prints to the console",
            "read a file",
            "reads a file",
            "from a file",
            "pipe",
            "stdin",
            "no window",
            "headless",
        ];
        if cli_signals.iter().any(|s| lower.contains(s)) {
            return Skeleton::Cli;
        }
        Skeleton::Gui
    }

    pub fn infer(request: &str) -> AppKind {
        Self::infer_matched(request).unwrap_or(AppKind::Checklist)
    }

    /// Infer the kind only when the request clearly names one of the built-in
    /// templates. Returns `None` when nothing matched.
    ///
    /// The built-in maker has three templates and no ability to write arbitrary
    /// apps. `infer` above falls back to the checklist for anything it does not
    /// recognise, which is fine as a default but silent -- someone who asked for
    /// "a PDF merger" would get a checklist named "pdf-merger" and no hint that
    /// the AI did not run. Callers use this to say so, and to point at
    /// `--agent`. Keep the two in lockstep: every branch here is a branch there.
    pub fn infer_matched(request: &str) -> Option<AppKind> {
        let lower = request.to_lowercase();

        let wants_voice_prompter = lower.contains("voice prompter")
            || lower.contains("voice-prompter")
            || lower.contains("teleprompter")
            || (lower.contains("microphone")
                && (lower.contains("prompt") || lower.contains("script")));
        if wants_voice_prompter {
            return Some(AppKind::VoicePrompter);
        }

        // Clear signals for the CLI file-analysis app.
        let wants_file_report = lower.contains("word frequency")
            || lower.contains("word count")
            || lower.contains("most common words")
            || lower.contains("count the words")
            || lower.contains("analyze a file")
            || lower.contains("read a text file")
            || lower.contains("read a file");
        if wants_file_report {
            return Some(AppKind::WordFrequency);
        }

        // Everything list/tracker/item shaped is the checklist GUI. This covers
        // "grocery list", "shopping list", "reading list", "packing list",
        // "to-do", "tasks", "track my …", and the bare "checklist".
        let wants_list = lower.contains("checklist")
            || lower.contains("todo")
            || lower.contains("to-do")
            || lower.contains("to do")
            || lower.contains("task")
            || lower.contains("list")
            || lower.contains("track")
            || lower.contains("items")
            || lower.contains("groceries")
            || lower.contains("shopping");
        if wants_list {
            return Some(AppKind::Checklist);
        }

        // Nothing matched. The caller decides what to do -- `infer` falls back
        // to the checklist, the CLI warns that the built-in maker did not
        // recognise the request and points at --agent.
        None
    }
}

/// A request handed to the authoring harness — the plain description of the app
/// to build, plus the few knobs the generated code and manifest need.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppRequest {
    /// Kebab-case crate/app name, e.g. `word-count`. Used for the package name
    /// and the wasm artifact path.
    pub name: String,
    /// One-line human description of what was asked for. Carried into the
    /// manifest and the transcript so the request is legible in the evidence.
    pub description: String,
    /// The app kind to generate.
    pub kind: AppKind,
    /// Directory glob the app is allowed to read, relative to the run
    /// directory, e.g. `./input/**`. This is the one capability whose grant
    /// gates the app, so the permission wall is meaningful.
    pub read_glob: String,
    /// How many top words the reporter prints.
    pub top_n: u32,
}

impl AppRequest {
    /// A sensible default word-frequency request, so callers (and the CLI) can
    /// produce the canonical demo app with one line.
    pub fn word_frequency(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: "Read a text file and print its most common words.".to_string(),
            kind: AppKind::WordFrequency,
            read_glob: "./input/**".to_string(),
            top_n: 5,
        }
    }

    /// A checklist GUI request: a list of checkboxes that saves locally.
    pub fn checklist(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description: "A checklist with checkboxes that saves to a local file.".to_string(),
            kind: AppKind::Checklist,
            // The checklist reads and writes its own data directory, named for
            // the app; the glob is the read half of that grant.
            read_glob: format!("./{name}/**"),
            top_n: 0,
        }
    }

    /// A native teleprompter that advances after a spoken phrase.
    pub fn voice_prompter(name: &str) -> Self {
        Self {
            name: name.to_string(),
            description:
                "A voice-activated teleprompter that listens only with explicit permission."
                    .to_string(),
            kind: AppKind::VoicePrompter,
            read_glob: "./unused/**".to_string(),
            top_n: 0,
        }
    }

    /// The crate name with hyphens turned into the underscore the wasm artifact
    /// and Rust package identifier use.
    fn snake_name(&self) -> String {
        self.name.replace('-', "_")
    }

    /// Reject a request that would produce an invalid crate before any file is
    /// written. Names must be a safe kebab-case identifier and the glob must be
    /// a relative path, since the app is granted a relative read scope.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("app name must not be empty".to_string());
        }
        let name_ok = self
            .name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !name_ok || self.name.starts_with('-') || self.name.ends_with('-') {
            return Err(format!(
                "app name {:?} must be lowercase kebab-case (a-z, 0-9, -)",
                self.name
            ));
        }
        if !self.read_glob.starts_with("./") {
            return Err(format!(
                "read glob {:?} must be a relative path starting with ./",
                self.read_glob
            ));
        }
        if self.kind == AppKind::WordFrequency && self.top_n == 0 {
            return Err("top_n must be at least 1".to_string());
        }
        Ok(())
    }
}

/// One generated file: where it goes under the app directory, and its contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedFile {
    /// Path relative to the generated crate root, e.g. `src/lib.rs`.
    pub path: String,
    /// Full file contents.
    pub contents: String,
}

/// The complete set of files a buildable Krate app needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedApp {
    pub files: Vec<GeneratedFile>,
}

impl GeneratedApp {
    /// Look up one generated file's contents by path, for tests and callers
    /// that want to assert on what was produced.
    pub fn file(&self, path: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|file| file.path == path)
            .map(|file| file.contents.as_str())
    }
}

/// Generate a complete Krate guest crate from a request.
///
/// The path to the SDK, WIT, and workspace is expressed relative to where the
/// app is written (`sdk_prefix`, e.g. `../..` when the app lives two directories
/// deep), so the generated `Cargo.toml` resolves the same `krate` bindings and
/// WIT the in-tree samples use.
pub fn generate(request: &AppRequest, sdk_prefix: &str) -> Result<GeneratedApp, String> {
    request.validate()?;
    let files = match request.kind {
        AppKind::WordFrequency => vec![
            GeneratedFile {
                path: "Cargo.toml".to_string(),
                contents: cargo_toml(request, sdk_prefix),
            },
            GeneratedFile {
                path: "src/lib.rs".to_string(),
                contents: word_frequency_source(request),
            },
            GeneratedFile {
                path: "manifest.toml".to_string(),
                contents: manifest_toml(request),
            },
        ],
        AppKind::Checklist => vec![
            GeneratedFile {
                path: "Cargo.toml".to_string(),
                contents: checklist_cargo_toml(request, sdk_prefix),
            },
            GeneratedFile {
                path: "src/lib.rs".to_string(),
                contents: checklist_source(request),
            },
            GeneratedFile {
                path: "manifest.toml".to_string(),
                contents: checklist_manifest_toml(request),
            },
        ],
        AppKind::VoicePrompter => vec![
            GeneratedFile {
                path: "Cargo.toml".to_string(),
                contents: checklist_cargo_toml(request, sdk_prefix),
            },
            GeneratedFile {
                path: "src/lib.rs".to_string(),
                contents: voice_prompter_source(request),
            },
            GeneratedFile {
                path: "manifest.toml".to_string(),
                contents: voice_prompter_manifest_toml(request),
            },
        ],
    };
    Ok(GeneratedApp { files })
}

/// Which world a skeleton is wired for. A GUI skeleton opens a window; a CLI
/// skeleton reads args and prints. The choice sets the WIT world in Cargo.toml,
/// which is the one thing an agent cannot easily fix after the fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skeleton {
    /// A windowed app (phase-3 gui world).
    Gui,
    /// A command-line app (phase-2 cli world).
    Cli,
}

/// A minimal compiling skeleton for the AI author to fill in.
///
/// Unlike the built-in templates, this carries no app behavior to "adapt" -- it
/// is a blank that already builds, imports only `krate:*`, and passes
/// `check-app`. A GUI skeleton opens a window, honors the `quick` argument, and
/// exits; a CLI skeleton prints one line and honors `quick`. The agent writes
/// the real app over the stub, keeping the WIT-wired Cargo.toml and adjusting
/// the manifest to the capabilities it actually uses.
///
/// The Cargo.toml is the existing, proven wiring (GUI or CLI world), so the
/// hardest-to-get-right file is correct by construction.
pub fn skeleton(name: &str, sdk_prefix: &str, world: Skeleton) -> Result<GeneratedApp, String> {
    // Reuse a request only as the carrier for the name/wiring the cargo/manifest
    // helpers expect. The kind here is immaterial: skeleton sources and
    // manifests are written below, not taken from a template.
    let request = AppRequest {
        name: name.to_string(),
        description: "a Krate app".to_string(),
        kind: AppKind::Checklist,
        read_glob: "./data/**".to_string(),
        top_n: 10,
    };
    request.validate()?;
    let files = match world {
        Skeleton::Gui => vec![
            GeneratedFile {
                path: "Cargo.toml".to_string(),
                // The skeleton's src/lib.rs is a `std` app, unlike the no_std
                // checklist template, so it needs the SDK's `std` feature on.
                // Linking the default no_std SDK into a std guest fails at
                // link time with "failed to load bitcode of module std".
                contents: skeleton_cargo_toml(&request, sdk_prefix),
            },
            GeneratedFile {
                path: "src/lib.rs".to_string(),
                contents: gui_skeleton_source(),
            },
            GeneratedFile {
                path: "manifest.toml".to_string(),
                contents: gui_skeleton_manifest(&request),
            },
        ],
        Skeleton::Cli => vec![
            GeneratedFile {
                path: "Cargo.toml".to_string(),
                contents: cargo_toml(&request, sdk_prefix),
            },
            GeneratedFile {
                path: "src/lib.rs".to_string(),
                contents: cli_skeleton_source(),
            },
            GeneratedFile {
                path: "manifest.toml".to_string(),
                contents: cli_skeleton_manifest(&request),
            },
        ],
    };
    Ok(GeneratedApp { files })
}

/// A minimal GUI skeleton: opens a window with one label, waits a bounded number
/// of rounds (short when the first argument is `quick`), then exits. No_std, no
/// behavior, imports only `krate:*`. The agent replaces the body.
fn gui_skeleton_source() -> String {
    // Kept deliberately small and heavily commented: the comments are the
    // agent's in-file guide to the shape it must keep (bindings, quick, the
    // pure_string helper, export!). `#[allow(warnings)]` on bindings matches
    // every shipped GUI app.
    r####"//! A minimal Krate GUI skeleton. Replace this with the real app.
//!
//! It opens a window with one label, honors the `quick` argument (exit
//! promptly for automated checks), and imports only `krate:*`. Keep the
//! shape -- `#![no_std]`, `mod bindings`, the `quick` check, `pure_string`,
//! `export!` -- and build the requested app inside `run`. Read
//! KRATE_AUTHORING.md first, then the closest example under apps/, and run
//! `krate check-app .` until it prints OK.

// A Krate guest is no_std: the SDK owns the allocator, the panic handler, and
// the mem intrinsics, so nothing here can pull std's latent `wasi:*` imports.
// Linking std instead would fail the import check the moment this app does
// anything real, which is far too late to find out.
#![no_std]

extern crate alloc;

// Pulled in for its allocator and panic handler even though this file calls no
// `krate::*` function directly. Without it the link fails with "no global
// memory allocator found" and "`#[panic_handler]` function required".
extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use alloc::string::String;
use bindings::krate::io::args;
use bindings::krate::ui::{events, tree, types, window};

const ROOT_ID: u64 = 1;
const LABEL_ID: u64 = 2;

/// How many 50ms rounds to stay open: a short window for `quick`, a longer
/// one for a real session.
const QUICK_ROUNDS: u32 = 20;
const SESSION_ROUNDS: u32 = 600;
const ROUND_MILLIS: u32 = 50;

struct Component;

fn root() -> types::WidgetNode {
    types::WidgetNode {
        id: ROOT_ID,
        parent: None,
        kind: types::WidgetKind::Stack,
        label: None,
        role: None,
        style: types::Style { width: Some(480.0), height: Some(320.0), grow: 0.0, padding: 16.0 },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

fn label(text: &str) -> types::WidgetNode {
    types::WidgetNode {
        id: LABEL_ID,
        parent: Some(ROOT_ID),
        kind: types::WidgetKind::Text,
        label: Some(pure_string(text)),
        role: Some(pure_string("text")),
        style: types::Style { width: Some(440.0), height: Some(28.0), grow: 0.0, padding: 0.0 },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

impl bindings::Guest for Component {
    fn run() -> i32 {
        let size = types::WindowSize { width: 480, height: 320 };
        let Ok(win) = window::create("Krate App", size) else { return 30; };
        if window::show(win).is_err() { return 31; }
        if tree::set_root(win, &root()).is_err()
            || tree::upsert_node(win, &label("Replace me")).is_err()
        {
            let _ = window::close(win);
            return 32;
        }

        // `quick` is a bare first argument (not a flag). Compare bytes, not
        // with str methods, so no panic path pulls wasi in.
        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|b| *b == b'\n')
            .next()
            .is_some_and(|first| first == b"quick");
        let rounds = if quick { QUICK_ROUNDS } else { SESSION_ROUNDS };

        for _ in 0..rounds {
            if let Some(types::Event::CloseRequested(id)) = events::wait(Some(ROUND_MILLIS)) {
                if id == win {
                    break;
                }
            }
        }

        let _ = window::close(win);
        0
    }
}

/// Build an owned `String` without touching std's allocation-error handler,
/// whose panic path drags the whole `wasi:*` import set in. Keep this helper.
fn pure_string(text: &str) -> String {
    let len = text.len();
    if len == 0 {
        return String::new();
    }
    unsafe {
        let layout = core::alloc::Layout::from_size_align_unchecked(len, 1);
        let ptr = alloc::alloc::alloc(layout);
        if ptr.is_null() {
            core::arch::wasm32::unreachable()
        }
        core::ptr::copy_nonoverlapping(text.as_ptr(), ptr, len);
        String::from_raw_parts(ptr, len, len)
    }
}

bindings::export!(Component with_types_in bindings);
"####
        .to_string()
}

/// The GUI skeleton's manifest: a window and the automation defaults. The agent
/// adds the capabilities the real app uses.
fn gui_skeleton_manifest(request: &AppRequest) -> String {
    let snake = request.snake_name();
    let title = title_case(&request.name);
    format!(
        r#"[app]
id = "dev.krate.{snake}"
name = "{title}"
version = "0.1.0-dev"
entry = "target/wasm32-wasip1/release/{snake}.wasm"
world = "krate:app/gui@0.2.0"

# Declare only the capabilities the app uses. A window is the one every GUI
# app needs. Add fs.read/fs.write, store.kv, random.bytes, and so on as the
# app requires -- see KRATE_AUTHORING.md section 2.
[[capabilities]]
cap = "ui.window:create"
rationale = "Open the app window"
required = true

[[capabilities]]
cap = "io.args"
rationale = "Read the quick-run flag used by automated checks"
required = false
"#
    )
}

/// A minimal CLI skeleton: prints one line and exits, honoring `quick`. No_std,
/// imports only `krate:*`. The agent replaces the body.
fn cli_skeleton_source() -> String {
    r####"// A minimal Krate CLI skeleton. Replace this with the real app.
//
// It prints one line and exits, and honors the `quick` argument. It imports
// only `krate:*` and is no_std. Keep the shape -- the `quick` check and the
// krate::io calls -- and build the requested app inside `run`. Read
// KRATE_AUTHORING.md first, then the closest example under apps/, and run
// `krate check-app .` until it prints OK.
#![no_std]
extern crate alloc;

use krate::{
    io::{args, stdio, streams::OutputStreamExt},
    Guest,
};

struct Component;

impl Guest for Component {
    fn run() -> i32 {
        // `quick` is a bare first argument used by automated checks. A CLI app
        // must do its work once and exit 0 whether it gets `quick` or a real
        // argument. Handle it before any other parsing.
        let stdout = stdio::stdout();
        let _ = stdout.write_line("replace me");
        let _ = stdout.flush();
        0
    }
}

krate::export!(Component);
"####
        .to_string()
}

/// The CLI skeleton's manifest: stdout and args, no gating capability yet. The
/// agent adds fs.read (and marks it required) for a file-reading app, etc.
fn cli_skeleton_manifest(request: &AppRequest) -> String {
    let snake = request.snake_name();
    let name = &request.name;
    format!(
        r#"[app]
id = "dev.krate.{snake}"
name = "{name}"
version = "0.1.0-dev"
entry = "target/wasm32-wasip1/release/{snake}.wasm"
world = "krate:app/cli@0.1.0"

# Declare only the capabilities the app uses. stdout and args are shown here;
# add fs.read (mark it required for a file-reading app), net.connect,
# random.bytes, and so on as needed -- see KRATE_AUTHORING.md section 2.
[[capabilities]]
cap = "io.stdout"
rationale = "Print the app's output"
required = true

[[capabilities]]
cap = "io.args"
rationale = "Read command-line arguments"
required = false
"#
    )
}

fn cargo_toml(request: &AppRequest, sdk_prefix: &str) -> String {
    let name = &request.name;
    // Mirrors apps/krate-cat/Cargo.toml exactly, only the names and the path
    // prefix change. cargo-component reads [package.metadata.component] to know
    // the WIT world and its dependency packages.
    format!(
        r#"# An empty [workspace] table makes the generated app its own workspace
# root, so it builds standalone even when it happens to sit inside another
# cargo workspace's directory tree — a real shared app has no parent workspace.
[workspace]

[package]
name = "{name}"
version = "0.1.0-dev"
edition = "2021"
license = "MIT OR Apache-2.0"
repository = "https://github.com/incyashraj/krate"
rust-version = "1.91"

[dependencies]
krate = {{ path = "{sdk_prefix}/crates/bindings-rust" }}
wit-bindgen-rt = {{ version = "0.44.0", features = ["bitflags"] }}

[lib]
crate-type = ["cdylib"]

[package.metadata.component]
package = "krate:{name}"

[package.metadata.component.target]
path = "{sdk_prefix}/wit/krate/phase2"
world = "cli"

[package.metadata.component.target.dependencies]
"krate:io" = {{ path = "{sdk_prefix}/wit/krate/phase2/deps/io" }}
"krate:fs" = {{ path = "{sdk_prefix}/wit/krate/phase2/deps/fs" }}
"krate:net" = {{ path = "{sdk_prefix}/wit/krate/phase2/deps/net" }}
"krate:time" = {{ path = "{sdk_prefix}/wit/krate/phase2/deps/time" }}
"krate:locale" = {{ path = "{sdk_prefix}/wit/krate/phase2/deps/locale" }}
"krate:resources" = {{ path = "{sdk_prefix}/wit/krate/phase2/deps/resources" }}
"krate:store" = {{ path = "{sdk_prefix}/wit/krate/phase2/deps/store" }}
"krate:random" = {{ path = "{sdk_prefix}/wit/krate/phase2/deps/random" }}

# abort on panic and let LTO strip the unreachable std/panic paths that would
# otherwise leave dangling wasi:* import declarations in the component. A Krate
# app may import only krate:*, so this profile is load-bearing, not a tuning.
# opt-level = "s" is part of that: without it the dead-code elimination is not
# aggressive enough to drop std's latent wasi:* imports, so the component leaks
# them. The GUI template and the in-repo samples carry it for the same reason.
[profile.release]
panic = "abort"
lto = true
codegen-units = 1
opt-level = "s"
strip = true
"#
    )
}

fn manifest_toml(request: &AppRequest) -> String {
    let name = &request.name;
    let snake = request.snake_name();
    let description = &request.description;
    let read_glob = &request.read_glob;
    // Only the capabilities the generated code uses are declared, and the one
    // that gates the app — fs.read on its input — is required, so withholding
    // it produces the standard exit-5 refusal.
    format!(
        r#"[app]
id = "dev.krate.{snake}"
name = "{name}"
version = "0.1.0-dev"
entry = "target/wasm32-wasip1/release/{snake}.wasm"
world = "krate:app/cli@0.1.0"

[[capabilities]]
cap = "io.args"
rationale = "Read the path of the file to analyze"
required = true

[[capabilities]]
cap = "io.stdout"
rationale = "Print the word-frequency report"
required = true

[[capabilities]]
cap = "io.stderr"
rationale = "Print usage and errors"
required = true

# {description}
[[capabilities]]
cap = "fs.read:{read_glob}"
rationale = "Read the text file to analyze"
required = true
"#
    )
}

/// The generated app source: a word-frequency reporter over the `krate` SDK.
///
/// It reads the file named by its first argument, lowercases and splits it into
/// words, counts them, and prints the `top_n` most frequent. A permission
/// denial on the read is reported and returns exit 5 — the wall the packaged
/// `.krate` exists to prove.
///
/// The generated code deliberately avoids `HashMap`, `format!`, and other std
/// facilities that pull `wasi:*` imports into the component (HashMap's hasher
/// needs `wasi:random`, for one). A Krate component may only import `krate:*`,
/// so the counter is a plain `Vec` scanned linearly and the output is built by
/// hand — the same discipline the in-tree samples follow.
fn word_frequency_source(request: &AppRequest) -> String {
    let top_n = request.top_n;
    format!(
        r#"//! Generated by the Krate authoring harness (krate-author).
//!
//! {description}
//!
//! Reads the file named by the first argument, counts word frequencies, and
//! prints the {top_n} most common words. Requires `fs.read` on its input; a
//! denied read returns exit 5, the standard Krate permission refusal.

// A Krate guest links only `krate:*`. Building `no_std` means std's runtime —
// whose latent `wasi:*` imports a component may not carry — is never linked, so
// the app is `krate:*`-only by construction. The SDK owns the allocator, panic
// handler, and mem intrinsics; an app just declares `no_std` + `alloc`.
#![no_std]
extern crate alloc;

use krate::{{
    fs::{{self, FsError, OpenMode}},
    io::{{args, stdio}},
    Guest,
}};

/// Fixed capacities keep the app simple and its memory bounded. `alloc` is
/// available (the SDK provides an allocator), so this is a readability choice,
/// not a constraint imposed by the import wall.
const INPUT_CAP: usize = 65536;
const MAX_WORDS: usize = 4096;
const MAX_WORD_LEN: usize = 32;
const LINE_CAP: usize = MAX_WORD_LEN + 16;

/// One counted word: its bytes (truncated to MAX_WORD_LEN) and how often it
/// appeared. Copyable so the table is a plain fixed array.
#[derive(Clone, Copy)]
struct Word {{
    bytes: [u8; MAX_WORD_LEN],
    len: usize,
    count: u32,
    used: bool,
}}

impl Word {{
    const EMPTY: Word = Word {{
        bytes: [0; MAX_WORD_LEN],
        len: 0,
        count: 0,
        used: false,
    }};

    fn slice(&self) -> &[u8] {{
        self.bytes.get(..self.len).unwrap_or(&[])
    }}
}}

struct Component;

impl Guest for Component {{
    fn run() -> i32 {{
        // Read the first argument the sample way: `args::first()` pulls wasi
        // imports a Krate component may not have, so split the raw string here.
        let raw = args::raw();
        let Some(path) = raw.split('\n').find(|arg| !arg.is_empty()) else {{
            let _ = stdio::eprintln("usage: {name} <file>");
            return 2;
        }};

        let file = match fs::open(path, OpenMode::Read) {{
            Ok(file) => file,
            Err(FsError::PermissionDenied) => {{
                let _ = stdio::eprintln("{name}: permission denied reading the input file");
                return 5;
            }}
            Err(FsError::NotFound) => {{
                let _ = stdio::eprintln("{name}: file not found");
                return 20;
            }}
            Err(_) => {{
                let _ = stdio::eprintln("{name}: could not read the input file");
                return 21;
            }}
        }};

        // Read into a fixed buffer; content past the cap is ignored rather than
        // growing an allocation.
        let mut input = [0u8; INPUT_CAP];
        let mut input_len = 0usize;
        loop {{
            let chunk = match file.read(8192) {{
                Ok(chunk) => chunk,
                Err(_) => {{
                    let _ = stdio::eprintln("{name}: could not read the input file");
                    return 22;
                }}
            }};
            if chunk.is_empty() {{
                break;
            }}
            for byte in &chunk {{
                if let Some(slot) = input.get_mut(input_len) {{
                    *slot = *byte;
                    input_len += 1;
                }}
            }}
        }}

        // Tokenize on non-alphanumeric bytes and tally into a fixed word table.
        // Words are ASCII-lowercased so "The" and "the" count together.
        let mut words = [Word::EMPTY; MAX_WORDS];
        let mut word_count = 0usize;
        let mut current = [0u8; MAX_WORD_LEN];
        let mut current_len = 0usize;

        let tally = |current: &[u8],
                     words: &mut [Word; MAX_WORDS],
                     word_count: &mut usize| {{
            if current.is_empty() {{
                return;
            }}
            // Bump an existing entry if we have seen this word before.
            for slot in words.iter_mut() {{
                if slot.used && slot.slice() == current {{
                    slot.count += 1;
                    return;
                }}
            }}
            // Otherwise claim the next free slot, if the table has room.
            if let Some(slot) = words.get_mut(*word_count) {{
                for (dst, src) in slot.bytes.iter_mut().zip(current.iter()) {{
                    *dst = *src;
                }}
                slot.len = current.len();
                slot.count = 1;
                slot.used = true;
                *word_count += 1;
            }}
        }};

        for index in 0..input_len {{
            let byte = input.get(index).copied().unwrap_or(0);
            if byte.is_ascii_alphanumeric() {{
                if let Some(slot) = current.get_mut(current_len) {{
                    *slot = byte.to_ascii_lowercase();
                    current_len += 1;
                }}
            }} else {{
                tally(current.get(..current_len).unwrap_or(&[]), &mut words, &mut word_count);
                current_len = 0;
            }}
        }}
        tally(current.get(..current_len).unwrap_or(&[]), &mut words, &mut word_count);

        // Selection-sort the top words out of the table by count descending,
        // then word ascending, so the report is deterministic for an input.
        let _ = stdio::println("word,count");
        let mut printed = 0u32;
        while printed < {top_n} {{
            let mut best: Option<usize> = None;
            for i in 0..word_count {{
                let Some(word_i) = words.get(i) else {{ continue }};
                if !word_i.used {{
                    continue;
                }}
                match best {{
                    None => best = Some(i),
                    Some(b) => {{
                        if let Some(word_b) = words.get(b) {{
                            if word_i.count > word_b.count
                                || (word_i.count == word_b.count
                                    && word_i.slice() < word_b.slice())
                            {{
                                best = Some(i);
                            }}
                        }}
                    }}
                }}
            }}
            let Some(idx) = best else {{ break }};
            let (line_bytes, line_len) = match words.get_mut(idx) {{
                Some(word) => {{
                    word.used = false;
                    format_line(word)
                }}
                None => break,
            }};
            if let Ok(line) = core::str::from_utf8(line_bytes.get(..line_len).unwrap_or(&[])) {{
                let _ = stdio::println(line);
            }}
            printed += 1;
        }}
        0
    }}
}}

/// Build a `word,count` CSV line into a fixed buffer, no `format!`. Returns the
/// buffer and how many bytes are valid.
fn format_line(word: &Word) -> ([u8; LINE_CAP], usize) {{
    let mut line = [0u8; LINE_CAP];
    let mut len = 0usize;
    for byte in word.slice() {{
        if let Some(slot) = line.get_mut(len) {{
            *slot = *byte;
            len += 1;
        }}
    }}
    if let Some(slot) = line.get_mut(len) {{
        *slot = b',';
        len += 1;
    }}
    // Decimal digits, most significant first, without std formatting.
    let mut digits = [0u8; 10];
    let mut digit_len = 0usize;
    let mut value = word.count;
    if value == 0 {{
        digits[0] = b'0';
        digit_len = 1;
    }} else {{
        while value > 0 {{
            if let Some(slot) = digits.get_mut(digit_len) {{
                *slot = b'0' + (value % 10) as u8;
                digit_len += 1;
            }}
            value /= 10;
        }}
    }}
    for i in (0..digit_len).rev() {{
        if let (Some(src), Some(dst)) = (digits.get(i), line.get_mut(len)) {{
            *dst = *src;
            len += 1;
        }}
    }}
    (line, len)
}}

krate::export!(Component);
"#,
        description = request.description,
        name = request.name,
        top_n = top_n,
    )
}

// ---- checklist (GUI) generation -------------------------------------------

/// The canonical checklist app source is the in-tree `krate-checklist` sample,
/// included verbatim so the generated app and the maintained sample can never
/// drift apart: the sample *is* the template. Only the window title is
/// substituted per request.
const CHECKLIST_SOURCE: &str = include_str!("../../../apps/krate-checklist/src/lib.rs");
const VOICE_PROMPTER_SOURCE: &str = include_str!("voice_prompter_template.rs");

fn checklist_source(request: &AppRequest) -> String {
    // A human-facing window title from the kebab-case name (e.g. `my-list` ->
    // `My list`). The sample hardcodes "Krate Checklist"; swap it out.
    let title = title_case(&request.name);
    // Only the title needs rewriting now. The app keeps its items in its own
    // store under a key, so there is no longer a folder name to keep in step
    // with the manifest -- which is the point of the store: an app names a key,
    // never a location.
    CHECKLIST_SOURCE.replace("Krate Checklist", &title)
}

fn voice_prompter_source(request: &AppRequest) -> String {
    VOICE_PROMPTER_SOURCE.replace("Voice Prompter", &title_case(&request.name))
}

/// Turn a kebab-case name into a capitalized, spaced title.
fn title_case(name: &str) -> String {
    let spaced = name.replace('-', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => spaced,
    }
}

/// Cargo.toml for the AI-authoring skeleton.
///
/// Same as the checklist template except the `krate` dependency turns on the
/// SDK's `std` feature, because `gui_skeleton_source` is a `std` app. The SDK
/// is `no_std` by default so guests link no std and cannot leak its latent
/// `wasi:*` imports; linking that default into a `std` guest fails with
/// "failed to load bitcode of module std". An agent converting the app to
/// `#![no_std]` should drop `features = ["std"]` at the same time.
fn skeleton_cargo_toml(request: &AppRequest, sdk_prefix: &str) -> String {
    checklist_cargo_toml(request, sdk_prefix).replace(
        &format!(r#"krate = {{ path = "{sdk_prefix}/crates/bindings-rust" }}"#),
        &format!(r#"krate = {{ path = "{sdk_prefix}/crates/bindings-rust", features = ["std"] }}"#),
    )
}

fn checklist_cargo_toml(request: &AppRequest, sdk_prefix: &str) -> String {
    let name = &request.name;
    // The GUI world (phase3) needs the ui/gfx/audio WIT packages the CLI world
    // does not. Mirrors apps/krate-checklist/Cargo.toml, only names and the
    // path prefix change.
    format!(
        r#"# An empty [workspace] table makes the generated app its own workspace
# root, so it builds standalone inside another workspace's directory tree.
[workspace]

[package]
name = "{name}"
version = "0.1.0-dev"
edition = "2021"
license = "MIT OR Apache-2.0"
repository = "https://github.com/incyashraj/krate"
rust-version = "1.91"

[dependencies]
# The Krate SDK. Your app is `#![no_std]`, so this supplies the pieces Rust
# would normally take from the standard library: the allocator, the panic
# handler, and the memory intrinsics. Keep it even if you never call
# `krate::` yourself, because without it the app will not link.
krate = {{ path = "{sdk_prefix}/crates/bindings-rust" }}
wit-bindgen-rt = {{ version = "0.44.0", features = ["bitflags"] }}

[lib]
crate-type = ["cdylib"]

# Keeps the generated bindings free of the standard library, so your app can
# stay `#![no_std]`. Turning this off links std, which pulls in operating
# system calls that Krate refuses, and the build fails at the import check.
[package.metadata.component.bindings]
std_feature = true

[package.metadata.component]
package = "krate:{name}"

[package.metadata.component.target]
path = "{sdk_prefix}/wit/krate/phase3"
world = "gui"

[package.metadata.component.target.dependencies]
"krate:io" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/io" }}
"krate:fs" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/fs" }}
"krate:net" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/net" }}
"krate:time" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/time" }}
"krate:locale" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/locale" }}
"krate:resources" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/resources" }}
"krate:store" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/store" }}
"krate:random" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/random" }}
"krate:ui" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/ui" }}
"krate:gfx" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/gfx" }}
"krate:audio" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/audio" }}
"krate:speech" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/speech" }}

[profile.release]
panic = "abort"
lto = true
codegen-units = 1
opt-level = "s"
"#
    )
}

fn checklist_manifest_toml(request: &AppRequest) -> String {
    let name = &request.name;
    let snake = request.snake_name();
    let title = title_case(name);
    // Must match the directory baked into the source by `checklist_source`,
    // or the app would ask for one folder and write to another.
    // The checklist needs a window and read+write on its own data directory.
    // The write grant is the one that gates saving, so withholding it produces
    // the standard exit-5 refusal.
    format!(
        r#"[app]
id = "dev.krate.{snake}"
name = "{title}"
version = "0.1.0-dev"
entry = "target/wasm32-wasip1/release/{snake}.wasm"
world = "krate:app/gui@0.2.0"

[[capabilities]]
cap = "ui.window:create"
rationale = "Open the checklist window"
required = true

[[capabilities]]
cap = "io.stdout"
rationale = "Report state on exit for automated runs"
required = true

[[capabilities]]
cap = "io.args"
rationale = "Read the quick-run flag used by automated tests"
required = true

[[capabilities]]
cap = "store.kv"
rationale = "Save your {title} items"
required = true
"#
    )
}

fn voice_prompter_manifest_toml(request: &AppRequest) -> String {
    let name = &request.name;
    let snake = request.snake_name();
    let title = title_case(name);
    format!(
        r#"[app]
id = "dev.krate.{snake}"
name = "{title}"
version = "0.1.0-dev"
entry = "target/wasm32-wasip1/release/{snake}.wasm"
world = "krate:app/gui@0.2.0"

[[capabilities]]
cap = "ui.window:create"
rationale = "Open the teleprompter window"
required = true

[[capabilities]]
cap = "io.stdout"
rationale = "Report readiness during automated verification"
required = true

[[capabilities]]
cap = "io.args"
rationale = "Read the quick-run flag used by automated verification"
required = true

[[capabilities]]
cap = "audio.capture"
rationale = "Listen for your voice to advance the teleprompter"
required = true
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> AppRequest {
        AppRequest::word_frequency("word-count")
    }

    /// Every WIT package a world imports must also be listed as a component
    /// target dependency, or `cargo-component` cannot resolve it.
    ///
    /// Both lists are written by hand, and the second one is easy to forget:
    /// adding `krate:random` to the worlds without adding it here broke every
    /// port with "package 'krate:random@0.1.0' not found", which points at the
    /// world file rather than at the list that is actually missing an entry.
    /// This compares the two directly so the next addition cannot rot the same
    /// way.
    #[test]
    fn every_imported_wit_package_is_a_declared_dependency() {
        let cargo_toml = generate(&req(), "../..")
            .expect("generate")
            .file("Cargo.toml")
            .expect("Cargo.toml")
            .to_string();

        for phase in ["phase2", "phase3"] {
            let world = std::fs::read_to_string(format!("../../wit/krate/{phase}/world.wit"))
                .unwrap_or_else(|e| panic!("read {phase} world: {e}"));

            for line in world.lines() {
                let line = line.trim();
                let Some(rest) = line.strip_prefix("import krate:") else {
                    continue;
                };
                let package: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '-')
                    .collect();

                // The CLI template targets phase2; the GUI template phase3.
                // Only the phase this generated app uses has to be present.
                if phase != "phase2" {
                    continue;
                }
                assert!(
                    cargo_toml.contains(&format!("\"krate:{package}\"")),
                    "{phase}/world.wit imports krate:{package}, but the generated \
                     Cargo.toml does not list it under \
                     [package.metadata.component.target.dependencies]. Add it, or \
                     cargo-component cannot resolve the world."
                );
            }
        }
    }

    #[test]
    fn generates_the_three_files_an_app_needs() {
        let app = generate(&req(), "../..").expect("generate");
        assert!(app.file("Cargo.toml").is_some());
        assert!(app.file("src/lib.rs").is_some());
        assert!(app.file("manifest.toml").is_some());
    }

    #[test]
    fn manifest_declares_the_read_capability_as_required() {
        let app = generate(&req(), "../..").expect("generate");
        let manifest = app.file("manifest.toml").expect("manifest");
        assert!(manifest.contains(r#"cap = "fs.read:./input/**""#));
        // The read wall must be required, or there is nothing to refuse.
        assert!(manifest.contains("cap = \"fs.read:./input/**\"\nrationale"));
        assert!(manifest.contains("required = true"));
    }

    #[test]
    fn generated_source_refuses_with_exit_5_on_permission_denied() {
        let app = generate(&req(), "../..").expect("generate");
        let source = app.file("src/lib.rs").expect("source");
        assert!(source.contains("FsError::PermissionDenied"));
        assert!(source.contains("return 5;"));
    }

    #[test]
    fn cargo_toml_points_at_the_sdk_and_wit_via_the_prefix() {
        let app = generate(&req(), "../..").expect("generate");
        let cargo = app.file("Cargo.toml").expect("cargo");
        assert!(cargo.contains(r#"krate = { path = "../../crates/bindings-rust" }"#));
        assert!(cargo.contains(r#"path = "../../wit/krate/phase2""#));
        assert!(cargo.contains(r#"package = "krate:word-count""#));
    }

    /// The GUI world needs the same `krate` dependency the CLI world has. It
    /// was missing here once, and every windowed app the generator or the agent
    /// skeleton produced died on three errors that all meant this one line:
    /// "can't find crate for `krate`", "no global memory allocator found", and
    /// "`#[panic_handler]` function required". Nothing else in the pipeline
    /// caught it, because the CLI world was fine and only it was tested.
    #[test]
    fn the_gui_world_depends_on_the_sdk_too() {
        let gui = generate(&AppRequest::checklist("todo"), "../..").expect("generate");
        let cargo = gui.file("Cargo.toml").expect("cargo");
        assert!(
            cargo.contains(r#"krate = { path = "../../crates/bindings-rust" }"#),
            "the GUI Cargo.toml must depend on the SDK, got:\n{cargo}"
        );

        // The skeleton is what an AI author is handed to start from, so it has
        // to build before the agent writes a line.
        let skel = skeleton("todo", "../..", Skeleton::Gui).expect("skeleton");
        let skel_cargo = skel.file("Cargo.toml").expect("cargo");
        // Match the dependency, not its exact spelling. The skeleton is a `std`
        // app so it carries `features = ["std"]`; the SDK is no_std by default
        // and linking that into a std guest fails with "failed to load bitcode
        // of module std". An exact-string check fails on a correct Cargo.toml.
        assert!(
            skel_cargo.lines().any(|line| {
                let line = line.trim_start();
                (line.starts_with("krate ") || line.starts_with("krate="))
                    && line.contains("../../crates/bindings-rust")
            }),
            "the GUI skeleton must depend on the SDK, got:\n{skel_cargo}"
        );
    }

    #[test]
    fn rejects_a_bad_name() {
        let mut bad = req();
        bad.name = "Word Count".to_string();
        assert!(generate(&bad, "../..").is_err());
    }

    #[test]
    fn rejects_a_non_relative_read_glob() {
        let mut bad = req();
        bad.read_glob = "/etc/**".to_string();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn top_n_flows_into_the_generated_source() {
        let mut r = req();
        r.top_n = 9;
        let app = generate(&r, "../..").expect("generate");
        let source = app.file("src/lib.rs").expect("source");
        assert!(source.contains("while printed < 9"));
    }

    // ---- checklist (GUI) ----

    #[test]
    fn infers_the_checklist_kind_from_the_request_text() {
        assert_eq!(
            AppKind::infer("Make a checklist app that saves locally"),
            AppKind::Checklist
        );
        assert_eq!(AppKind::infer("build me a todo list"), AppKind::Checklist);
        assert_eq!(
            AppKind::infer("count the words in a file"),
            AppKind::WordFrequency
        );
    }

    #[test]
    fn infer_matched_reports_when_nothing_matched() {
        // A request the templates cannot serve returns None from the matcher,
        // even though `infer` still falls back to a checklist. This is the
        // signal the CLI uses to warn that the AI did not run.
        assert_eq!(AppKind::infer_matched("a pdf merger"), None);
        assert_eq!(AppKind::infer_matched("shrink these photos"), None);
        // A real match still reports Some, so the warning stays quiet.
        assert_eq!(
            AppKind::infer_matched("a grocery list"),
            Some(AppKind::Checklist)
        );
        // And the total function keeps its checklist default.
        assert_eq!(AppKind::infer("a pdf merger"), AppKind::Checklist);
    }

    #[test]
    fn wants_gui_leans_windowed_but_hears_a_clear_cli_signal() {
        // Visual/consumer requests -> a window.
        assert_eq!(AppKind::wants_gui("a tip calculator"), Skeleton::Gui);
        assert_eq!(AppKind::wants_gui("a maze you can walk"), Skeleton::Gui);
        assert_eq!(
            AppKind::wants_gui("a pomodoro timer with a ring"),
            Skeleton::Gui
        );
        // Explicit command-line shapes -> CLI.
        assert_eq!(
            AppKind::wants_gui("a command-line JSON pretty-printer"),
            Skeleton::Cli
        );
        assert_eq!(
            AppKind::wants_gui("read a file and print a word count to stdout"),
            Skeleton::Cli
        );
        assert_eq!(
            AppKind::wants_gui("a CLI that formats a markdown table"),
            Skeleton::Cli
        );
    }

    #[test]
    fn both_skeletons_depend_on_the_sdk() {
        // Measured: the GUI template shipped without the `krate` dependency
        // while the CLI one had it, so every windowed app an AI authored had to
        // find the missing dep through a failed link and add it back. The SDK
        // owns the allocator, panic handler, and mem* intrinsics a no_std guest
        // needs, and every shipped GUI app under apps/ depends on it.
        for world in [Skeleton::Gui, Skeleton::Cli] {
            let app = skeleton("my-app", "/sdk", world).expect("skeleton");
            let cargo = &app
                .files
                .iter()
                .find(|f| f.path == "Cargo.toml")
                .expect("Cargo.toml")
                .contents;
            // Match the dependency, not its exact spelling. The GUI skeleton
            // is a `std` app so it carries `features = ["std"]`; the SDK is
            // no_std by default and linking that into a std guest fails with
            // "failed to load bitcode of module std". An exact-string check
            // would fail on a correct Cargo.toml.
            assert!(
                cargo.lines().any(|line| {
                    let line = line.trim_start();
                    (line.starts_with("krate ") || line.starts_with("krate="))
                        && line.contains("/sdk/crates/bindings-rust")
                }),
                "{world:?} skeleton must depend on the SDK:\n{cargo}"
            );
        }
    }

    #[test]
    fn skeletons_produce_the_three_files_wired_to_the_right_world() {
        let gui = skeleton("my-app", "/sdk", Skeleton::Gui).expect("gui skeleton");
        let names: Vec<&str> = gui.files.iter().map(|f| f.path.as_str()).collect();
        assert!(names.contains(&"Cargo.toml"));
        assert!(names.contains(&"src/lib.rs"));
        assert!(names.contains(&"manifest.toml"));
        let gui_manifest = &gui
            .files
            .iter()
            .find(|f| f.path == "manifest.toml")
            .unwrap()
            .contents;
        assert!(gui_manifest.contains("gui@0.2.0"), "GUI world");
        assert!(gui_manifest.contains("ui.window:create"), "a window");
        let gui_lib = &gui
            .files
            .iter()
            .find(|f| f.path == "src/lib.rs")
            .unwrap()
            .contents;
        assert!(gui_lib.contains("window::create"), "opens a window");
        assert!(gui_lib.contains("quick"), "honors the quick argument");
        // The GUI skeleton must be no_std for the same reason the CLI one is:
        // it depends on the SDK, which supplies the allocator and the panic
        // handler, and linking std as well makes those collide
        // ("rust_begin_unwind: symbol multiply defined"). It also means the
        // agent starts from a shape that passes the import check rather than
        // one that fails it the moment it does anything real.
        assert!(gui_lib.contains("#![no_std]"), "no_std GUI");
        assert!(
            !gui_lib.contains("std::alloc::"),
            "a no_std guest must allocate through `alloc`, not `std`"
        );

        let cli = skeleton("my-app", "/sdk", Skeleton::Cli).expect("cli skeleton");
        let cli_manifest = &cli
            .files
            .iter()
            .find(|f| f.path == "manifest.toml")
            .unwrap()
            .contents;
        assert!(cli_manifest.contains("cli@0.1.0"), "CLI world");
        let cli_lib = &cli
            .files
            .iter()
            .find(|f| f.path == "src/lib.rs")
            .unwrap()
            .contents;
        assert!(cli_lib.contains("#![no_std]"), "no_std CLI");
        assert!(cli_lib.contains("write_line"), "prints something");
    }

    #[test]
    fn infers_and_generates_a_voice_prompter_with_microphone_permission() {
        assert_eq!(
            AppKind::infer("Make a voice prompter that listens to my microphone"),
            AppKind::VoicePrompter
        );
        let app =
            generate(&AppRequest::voice_prompter("voice-prompter"), "../..").expect("generate");
        let source = app.file("src/lib.rs").expect("source");
        let manifest = app.file("manifest.toml").expect("manifest");
        assert!(source.contains("capture::open"));
        assert!(source.contains("transcription::match_line_stream"));
        assert!(source.contains("Voice prompter"));
        assert!(manifest.contains(r#"cap = "audio.capture""#));
        assert!(manifest.contains("Listen for your voice"));
    }

    #[test]
    fn generates_a_gui_checklist_crate() {
        let app = generate(&AppRequest::checklist("my-list"), "../..").expect("generate");
        let cargo = app.file("Cargo.toml").expect("cargo");
        // The GUI world needs the phase3 target and the ui package.
        assert!(cargo.contains(r#"path = "../../wit/krate/phase3""#));
        // Without this, a GUI app cannot be `#![no_std]` -- the generated
        // bindings carry `impl std::error::Error` and so require std, and
        // linking std brings a dependency's panic path with it. An image
        // viewer's decoder was clean under no_std and pulled four wasi imports
        // the moment std was linked, which is a failure at the import check
        // long after the build succeeds.
        assert!(
            cargo.contains("std_feature = true"),
            "a GUI scaffold without std_feature cannot use no_std, and a windowed \
             app with any real dependency needs it"
        );
        assert!(cargo.contains("world = \"gui\""));
        assert!(cargo.contains(r#""krate:ui" = { path = "../../wit/krate/phase3/deps/ui" }"#));
        let manifest = app.file("manifest.toml").expect("manifest");
        assert!(manifest.contains(r#"cap = "ui.window:create""#));
        // The app keeps its items in its own store, so it asks to remember
        // things rather than for access to a folder. That is both a smaller
        // grant and a more honest sentence in the permission prompt.
        assert!(manifest.contains(r#"cap = "store.kv""#));
        assert!(
            !manifest.contains("fs.read:"),
            "no filesystem grant is needed"
        );
        assert!(
            !manifest.contains("fs.write:"),
            "no filesystem grant is needed"
        );
        assert!(manifest.contains(r#"world = "krate:app/gui@0.2.0""#));
    }

    #[test]
    fn a_generated_app_keeps_its_data_without_touching_the_filesystem() {
        // The mismatch this used to guard against -- a grant naming one folder
        // while the code wrote to another -- cannot happen now: the app names a
        // key, and the runtime decides where that lives. What is worth
        // asserting instead is that no filesystem authority is requested at all.
        let app = generate(&AppRequest::checklist("reading-list"), "../..").expect("generate");
        let manifest = app.file("manifest.toml").expect("manifest");
        let source = app.file("src/lib.rs").expect("source");
        assert!(manifest.contains(r#"cap = "store.kv""#));
        assert!(!manifest.contains("fs.read:"));
        assert!(!manifest.contains("fs.write:"));
        assert!(
            !source.contains("files::open"),
            "the generated app should not open files to keep its own data"
        );
    }

    #[test]
    fn checklist_source_is_the_maintained_sample_with_the_title_swapped() {
        let app = generate(&AppRequest::checklist("my-list"), "../..").expect("generate");
        let source = app.file("src/lib.rs").expect("source");
        // The window title is title-cased from the name.
        assert!(source.contains("My list"));
        // And it is the real checklist app: the persistence format is there.
        assert!(source.contains("[x] ") || source.contains("b\"[x] \""));
        assert!(source.contains("bindings::export!(Component"));
    }

    #[test]
    fn a_checklist_request_needs_no_top_n() {
        // top_n is 0 for a checklist and validate must not reject it.
        let req = AppRequest::checklist("my-list");
        assert_eq!(req.top_n, 0);
        assert!(req.validate().is_ok());
    }
}

#[cfg(test)]
mod krate_dependency_tests {
    use super::*;

    /// Every generated Cargo.toml must declare the `krate` dependency.
    ///
    /// The regression this guards: the GUI (checklist) template emitted
    /// `extern crate krate` in src/lib.rs but never wrote the matching
    /// dependency, so `krate create` without an agent could not build ANY app
    /// routed to it -- and most plain requests route there. It died with "can't
    /// find crate for `krate`", "no global memory allocator found", and
    /// "`#[panic_handler]` function required": three errors, one cause. The CLI
    /// template had the line, which is why this went unnoticed.
    #[test]
    fn every_template_declares_the_krate_dependency() {
        for kind in [
            AppKind::WordFrequency,
            AppKind::Checklist,
            AppKind::VoicePrompter,
        ] {
            let mut request = AppRequest::word_frequency("sample");
            request.kind = kind;
            let app = generate(&request, "../..").expect("generate");
            let cargo = app.file("Cargo.toml").expect("cargo");
            assert!(
                cargo
                    .lines()
                    .map(str::trim_start)
                    .any(|line| line.starts_with("krate ") || line.starts_with("krate=")),
                "the {kind:?} template has no `krate` dependency, so an app made \
                 from it cannot build:\n{cargo}"
            );
        }
    }
}
