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
//! The one app kind today is a word-frequency reporter: it reads a file and
//! prints its most common words. It needs exactly one interesting capability —
//! `fs.read` on its input — so the packaged `.krate` has a real permission wall
//! to prove: run it with the grant and it works; withhold the grant and it
//! refuses before doing anything, exactly like every other Krate app.

use serde::{Deserialize, Serialize};

/// What kind of app the agent was asked to build. The enum is the seam where
/// more request types slot in without reshaping the pipeline around them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppKind {
    /// A CLI app: read a file and print its most frequent words.
    WordFrequency,
    /// A GUI app: a checklist with checkboxes that saves locally.
    Checklist,
}

impl AppKind {
    /// Infer the app kind from a free-text request, so `krate create "make a
    /// checklist…"` picks the right template without an explicit flag. Falls
    /// back to the word-frequency CLI app.
    pub fn infer(request: &str) -> AppKind {
        let lower = request.to_lowercase();
        if lower.contains("checklist")
            || lower.contains("todo")
            || lower.contains("to-do")
            || lower.contains("task list")
        {
            AppKind::Checklist
        } else {
            AppKind::WordFrequency
        }
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
            // The checklist reads and writes its own data directory; the glob
            // is the read half of that grant.
            read_glob: "./checklist/**".to_string(),
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
    };
    Ok(GeneratedApp { files })
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

# abort on panic and let LTO strip the unreachable std/panic paths that would
# otherwise leave dangling wasi:* import declarations in the component. A Krate
# app may import only krate:*, so this profile is load-bearing, not a tuning.
[profile.release]
panic = "abort"
lto = true
codegen-units = 1
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
//!
//! No `HashMap`, `format!`, or other std facilities that would pull `wasi:*`
//! imports into the component: a Krate app may import only `krate:*`.

use krate::{{
    fs::{{self, FsError, OpenMode}},
    io::{{args, stdio}},
    Guest,
}};

/// Fixed capacities. A growable `Vec` reallocates, and realloc references std's
/// allocation-error handler, which drags the whole `wasi:*` import set into a
/// component that may import only `krate:*`. So, exactly like the in-tree
/// samples, every buffer here is fixed and every access non-panicking.
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

fn checklist_source(request: &AppRequest) -> String {
    // A human-facing window title from the kebab-case name (e.g. `my-list` ->
    // `My list`). The sample hardcodes "Krate Checklist"; swap it out.
    let title = title_case(&request.name);
    CHECKLIST_SOURCE.replace("Krate Checklist", &title)
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
wit-bindgen-rt = {{ version = "0.44.0", features = ["bitflags"] }}

[lib]
crate-type = ["cdylib"]

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
"krate:ui" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/ui" }}
"krate:gfx" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/gfx" }}
"krate:audio" = {{ path = "{sdk_prefix}/wit/krate/phase3/deps/audio" }}

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
cap = "fs.read:./checklist/**"
rationale = "Load your saved checklist"
required = true

[[capabilities]]
cap = "fs.write:./checklist/**"
rationale = "Save changes to your checklist"
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
    fn generates_a_gui_checklist_crate() {
        let app = generate(&AppRequest::checklist("my-list"), "../..").expect("generate");
        let cargo = app.file("Cargo.toml").expect("cargo");
        // The GUI world needs the phase3 target and the ui package.
        assert!(cargo.contains(r#"path = "../../wit/krate/phase3""#));
        assert!(cargo.contains("world = \"gui\""));
        assert!(cargo.contains(r#""krate:ui" = { path = "../../wit/krate/phase3/deps/ui" }"#));
        let manifest = app.file("manifest.toml").expect("manifest");
        assert!(manifest.contains(r#"cap = "ui.window:create""#));
        assert!(manifest.contains(r#"cap = "fs.write:./checklist/**""#));
        assert!(manifest.contains(r#"world = "krate:app/gui@0.2.0""#));
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
