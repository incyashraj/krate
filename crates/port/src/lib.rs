//! Read-only source project analysis for `krate port --plan`.
//!
//! This crate intentionally does not run build scripts, package managers, or
//! source code. Its first job is to turn an unknown repository into a stable,
//! inspectable porting plan before an AI agent or compiler changes anything.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

const MAX_FILES: usize = 20_000;
const MAX_SCANNED_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TOTAL_SCANNED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SNAPSHOT_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TOTAL_SNAPSHOT_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum PortError {
    #[error("source project does not exist: {0}")]
    Missing(PathBuf),
    #[error("source project is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("project contains more than {MAX_FILES} files; narrow the source directory")]
    TooManyFiles,
    #[error("project scan exceeded {MAX_TOTAL_SCANNED_BYTES} bytes; narrow the source directory")]
    TooManyBytes,
    #[error("source snapshot destination already exists: {0}")]
    SnapshotExists(PathBuf),
    #[error(
        "source snapshot exceeded {MAX_TOTAL_SNAPSHOT_BYTES} bytes; narrow the source directory"
    )]
    SnapshotTooLarge,
}

pub type Result<T> = std::result::Result<T, PortError>;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Ready,
    NeedsChanges,
    Unsupported,
}

impl Verdict {
    fn label(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NeedsChanges => "needs changes",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Info,
    Change,
    Blocker,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Evidence {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    pub confidence: Confidence,
    pub title: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PortPlan {
    pub schema: String,
    pub source: String,
    pub verdict: Verdict,
    pub profile: String,
    pub languages: Vec<String>,
    pub frameworks: Vec<String>,
    pub entry_points: Vec<String>,
    pub suggested_capabilities: Vec<String>,
    pub findings: Vec<Finding>,
    pub next_steps: Vec<String>,
    pub scan: ScanSummary,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ScanSummary {
    pub files_seen: usize,
    pub files_scanned: usize,
    pub bytes_scanned: u64,
    pub skipped_large_files: usize,
    pub symlinks_skipped: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
pub struct SnapshotSummary {
    pub files_copied: usize,
    pub bytes_copied: u64,
    pub sensitive_files_excluded: Vec<String>,
    pub large_files_excluded: Vec<String>,
    pub symlinks_excluded: usize,
}

impl PortPlan {
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str("Krate port plan\n");
        out.push_str("================\n");
        out.push_str(&format!("Source: {}\n", self.source));
        out.push_str(&format!("Verdict: {}\n", self.verdict.label()));
        out.push_str(&format!("Profile: {}\n", self.profile));
        out.push_str(&format!(
            "Languages: {}\n",
            display_list(&self.languages, "not detected")
        ));
        out.push_str(&format!(
            "Frameworks: {}\n",
            display_list(&self.frameworks, "not detected")
        ));

        if !self.suggested_capabilities.is_empty() {
            out.push_str("\nLikely capabilities\n");
            for cap in &self.suggested_capabilities {
                out.push_str(&format!("  - {cap}\n"));
            }
        }

        if !self.findings.is_empty() {
            out.push_str("\nFindings\n");
            for finding in &self.findings {
                let level = match finding.severity {
                    Severity::Info => "INFO",
                    Severity::Change => "CHANGE",
                    Severity::Blocker => "BLOCKER",
                };
                let confidence = match finding.confidence {
                    Confidence::High => "high",
                    Confidence::Medium => "medium",
                    Confidence::Low => "low",
                };
                out.push_str(&format!(
                    "  [{level}, {confidence} confidence] {}\n",
                    finding.title
                ));
                out.push_str(&format!("    {}\n", finding.detail));
                for evidence in finding.evidence.iter().take(5) {
                    match evidence.line {
                        Some(line) => {
                            out.push_str(&format!("    at {}:{line}\n", evidence.path));
                        }
                        None => out.push_str(&format!("    at {}\n", evidence.path)),
                    }
                }
                if finding.evidence.len() > 5 {
                    out.push_str(&format!(
                        "    and {} more location(s)\n",
                        finding.evidence.len() - 5
                    ));
                }
            }
        }

        out.push_str("\nNext steps\n");
        for (index, step) in self.next_steps.iter().enumerate() {
            out.push_str(&format!("  {}. {step}\n", index + 1));
        }
        out.push_str(&format!(
            "\nRead-only scan: {} files seen, {} text files scanned, {} bytes read\n",
            self.scan.files_seen, self.scan.files_scanned, self.scan.bytes_scanned
        ));
        out
    }
}

fn display_list(values: &[String], empty: &str) -> String {
    if values.is_empty() {
        empty.to_string()
    } else {
        values.join(", ")
    }
}

#[derive(Default)]
struct Analysis {
    languages: BTreeSet<String>,
    /// Direct dependencies from every Cargo.toml in the project. The std
    /// question is decided per-dependency, and it is the wall most ports
    /// actually hit (K-079).
    direct_dependencies: BTreeSet<String>,
    frameworks: BTreeSet<String>,
    entry_points: BTreeSet<String>,
    capabilities: BTreeSet<String>,
    findings: BTreeMap<String, Finding>,
    has_krate_manifest: bool,
    source_files: usize,
    binary_files: usize,
    scan: ScanSummaryBuilder,
}

#[derive(Default)]
struct ScanSummaryBuilder {
    files_seen: usize,
    files_scanned: usize,
    bytes_scanned: u64,
    skipped_large_files: usize,
    symlinks_skipped: usize,
}

impl ScanSummaryBuilder {
    fn finish(self) -> ScanSummary {
        ScanSummary {
            files_seen: self.files_seen,
            files_scanned: self.files_scanned,
            bytes_scanned: self.bytes_scanned,
            skipped_large_files: self.skipped_large_files,
            symlinks_skipped: self.symlinks_skipped,
        }
    }
}

pub fn analyze(source: impl AsRef<Path>) -> Result<PortPlan> {
    let source = source.as_ref();
    if !source.exists() {
        return Err(PortError::Missing(source.to_path_buf()));
    }
    if !source.is_dir() {
        return Err(PortError::NotDirectory(source.to_path_buf()));
    }

    let canonical = fs::canonicalize(source).map_err(|source_error| PortError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    let mut analysis = Analysis::default();
    scan_dir(&canonical, &canonical, &mut analysis)?;
    scan_cargo_lock(&canonical, &mut analysis);
    scan_std_wall(&mut analysis);
    finish_plan(canonical, analysis)
}

/// Read `Cargo.lock` for crates that link a library the operating system
/// must already have installed.
///
/// The lockfile is skipped by the pattern scan for good reason: it is large,
/// generated, and every crate in the tree appears in it, so ordinary patterns
/// produce noise there rather than findings. It is also the only place a
/// transitive native binding is visible -- an image viewer's own `Cargo.toml`
/// says `image = { features = ["avif-native"] }`, which looks like a pure-Rust
/// dependency, and the `dav1d-sys` it pulls appears two levels down.
///
/// The test is not the `-sys` suffix. That convention marks a crate that binds
/// to something outside Rust, which includes plenty that build for wasm
/// perfectly well: `js-sys` and `web-sys` are WebAssembly's own bindings,
/// `windows-sys` and `linux-raw-sys` are generated syscall declarations. Every
/// proven port already carries several. Flagging the suffix marked all six as
/// unsupported.
///
/// What actually marks a native binding is a build dependency on `system-deps`
/// or `pkg-config`: those exist to locate a library already installed on the
/// machine, and the wasm target has none. `dav1d-sys` has one; none of the
/// harmless `-sys` crates do.
///
/// It is a change, not a blocker, and the six proven ports are why. Every one
/// of them carries a native binding: `openssl-sys` under a HTTP client,
/// `libsqlite3-sys` under a database layer, `x11-dl` and `wayland-sys` under
/// egui's windowing. All six ported anyway, because each binding sits beneath
/// a dependency the port replaces outright -- the HTTP client becomes Krate's
/// `net`, the database becomes `store.sql`, the windowing becomes Krate's own
/// UI. Calling that a blocker would have told six people not to try six ports
/// that work.
///
/// It stays a blocker only when the binding is the app's actual job, which the
/// caller decides: a JPEG compressor built on `mozjpeg-sys` has nothing left
/// once that crate is gone.
/// Direct dependencies out of a Cargo.toml, by walking its sections.
///
/// A line parser rather than a TOML parse on purpose: the crate has no toml
/// dependency, and section-plus-key is all that is needed. `[dependencies]`,
/// `[dev-dependencies]` and target-specific tables all count -- the port has
/// to survive whichever of them the build actually uses.
fn collect_direct_dependencies(text: &str, deps: &mut BTreeSet<String>) {
    let mut in_deps = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_deps = trimmed.contains("dependencies");
            // `[dependencies.serde]` names the dependency in the header.
            if let Some(rest) = trimmed.strip_prefix("[dependencies.") {
                if let Some(name) = rest.strip_suffix(']') {
                    deps.insert(name.to_string());
                }
            }
            continue;
        }
        if !in_deps || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((name, _)) = trimmed.split_once('=') {
            let name = name.trim().trim_matches('"');
            if !name.is_empty() && name != "features" && name != "workspace" {
                deps.insert(name.to_string());
            }
        }
    }
}

/// Crates known to build without std (with the right features), from ports
/// that actually shipped. Everything absent from both lists is "not
/// verified", which the report says out loud rather than guessing.
const KNOWN_NO_STD: &[&str] = &[
    "rand",
    "rand_core",
    "getrandom",
    "uuid",
    "serde",
    "serde_json",
    "heapless",
    "libm",
    "hashbrown",
    "itoa",
    "ryu",
    "bitflags",
    "arrayvec",
    "smallvec",
    "zune-png",
    "zune-jpeg",
    "zune-core",
    "nb",
    "embedded-hal",
    "micromath",
    "regex-lite",
    "krate",
];

/// Crates that require std, from documentation or from a port that hit the
/// wall. One of these in the tree means the port cannot proceed as-is.
const KNOWN_STD_ONLY: &[&str] = &[
    "tokio",
    "reqwest",
    "hyper",
    "image",
    "lopdf",
    "printpdf",
    "pdf",
    "pdfium-render",
    "clap",
    "regex",
    "chrono",
    "rusqlite",
    "notify",
    "walkdir",
    "rayon",
    "crossterm",
    "ratatui",
    "eframe",
    "egui",
    "iced",
    "druid",
    "gtk",
    "winit",
];

fn scan_std_wall(analysis: &mut Analysis) {
    if analysis.direct_dependencies.is_empty() {
        return;
    }
    let std_only: Vec<String> = analysis
        .direct_dependencies
        .iter()
        .filter(|d| KNOWN_STD_ONLY.contains(&d.as_str()))
        .cloned()
        .collect();
    let unverified: Vec<String> = analysis
        .direct_dependencies
        .iter()
        .filter(|d| !KNOWN_NO_STD.contains(&d.as_str()) && !KNOWN_STD_ONLY.contains(&d.as_str()))
        .cloned()
        .collect();

    // The report that earned this check said "needs changes, one finding,
    // map your file paths" about a project whose PDF crate needs std -- a
    // weekend-sized omission delivered in a confident voice. The std verdict
    // now leads whenever it is not clean.
    if !std_only.is_empty() {
        add_evidence_finding(
            analysis,
            FindingSpec {
                id: "std-dependency-wall".to_string(),
                severity: Severity::Blocker,
                confidence: Confidence::High,
                title: "A dependency needs std, and Krate guests build without it".to_string(),
                detail: format!(
                    "Krate apps are `#![no_std]`: reaching the OS through std pulls `wasi:*` imports and the app is rejected. These direct dependencies are known to require std: {}. The port cannot proceed until each is replaced with a no_std equivalent or dropped -- this is the wall, and everything else in this report comes after it.",
                    std_only.join(", ")
                ),
                capability: None,
            },
            Evidence { path: "Cargo.toml".to_string(), line: None },
        );
    }
    if !unverified.is_empty() {
        add_evidence_finding(
            analysis,
            FindingSpec {
                id: "std-dependency-unverified".to_string(),
                severity: Severity::Change,
                confidence: Confidence::Medium,
                title: "Every dependency must work without std -- these are unverified".to_string(),
                detail: format!(
                    "Krate guests build `#![no_std]`, and most of the crates people reach for do not (every common PDF, image and document crate needs std; `rand` works because it supports no_std, which makes it the exception rather than the rule). Not yet verified either way: {}. Check each crate's docs for no_std support before starting -- one std-only dependency is enough to stop the port.",
                    unverified.join(", ")
                ),
                capability: None,
            },
            Evidence { path: "Cargo.toml".to_string(), line: None },
        );
    }
}

fn scan_cargo_lock(root: &Path, analysis: &mut Analysis) {
    let lock = root.join("Cargo.lock");
    let Ok(text) = fs::read_to_string(&lock) else {
        return;
    };

    let mut found: Vec<String> = Vec::new();
    let mut name: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("name = \"") {
            name = rest.strip_suffix('"').map(str::to_string);
            continue;
        }
        // Inside a package's `dependencies = [...]`, entries are quoted and
        // indented. A package that pulls one of these is asking the build to
        // find a library on the host.
        let entry = line.trim().trim_matches(|c| c == '"' || c == ',');
        if matches!(entry, "system-deps" | "pkg-config") {
            if let Some(package) = name.clone() {
                if !found.contains(&package) {
                    found.push(package);
                }
            }
        }
    }
    if found.is_empty() {
        return;
    }

    found.sort();
    let named = found.join(", ");
    let line = text
        .lines()
        .position(|l| found.iter().any(|f| l == format!("name = \"{f}\"")))
        .map(|index| index + 1);
    add_evidence_finding(
        analysis,
        FindingSpec {
            id: "native-library-binding".to_string(),
            severity: Severity::Change,
            // High, not medium: this is not a keyword in prose. The crate is in
            // the resolved dependency graph, and it asks the build to go
            // looking for a native library that the wasm target does not have.
            confidence: Confidence::High,
            title: "Binds a native C library".to_string(),
            detail: format!(
                "The dependency tree pulls {named}, which locate and link a library already installed on the machine. Nothing here builds for Krate: the target has no C toolchain and no system libraries to find. Usually these sit under a dependency the port replaces anyway -- an HTTP client becomes `net`, a database becomes `store.sql`, a windowing crate becomes Krate's own UI -- and the binding leaves with it. Sometimes one feature flag is the whole cause: `image`'s `avif-native` pulls `dav1d-sys` while the same crate's pure-Rust decoders do not. Check which case this is before starting, because if the native library is what the app actually does, the port has nothing left to build on."
            ),
            capability: None,
        },
        Evidence {
            path: "Cargo.lock".to_string(),
            line,
        },
    );
}

/// Copy a bounded, credential-filtered, read-only source snapshot for an AI
/// porting task. Build outputs, dependency caches, VCS metadata, AI-tool
/// settings, symlinks, and common credential files are never copied.
pub fn snapshot(
    source: impl AsRef<Path>,
    destination: impl AsRef<Path>,
) -> Result<SnapshotSummary> {
    let source = source.as_ref();
    let destination = destination.as_ref();
    if !source.exists() {
        return Err(PortError::Missing(source.to_path_buf()));
    }
    if !source.is_dir() {
        return Err(PortError::NotDirectory(source.to_path_buf()));
    }
    if destination.exists() {
        return Err(PortError::SnapshotExists(destination.to_path_buf()));
    }

    let source = fs::canonicalize(source).map_err(|source_error| PortError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    fs::create_dir_all(destination).map_err(|source_error| PortError::Io {
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    let mut summary = SnapshotSummary::default();
    copy_snapshot_dir(&source, &source, destination, &mut summary)?;
    set_read_only(destination)?;
    Ok(summary)
}

fn copy_snapshot_dir(
    root: &Path,
    dir: &Path,
    destination: &Path,
    summary: &mut SnapshotSummary,
) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|source| PortError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut entries = entries
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|source| PortError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let source_path = entry.path();
        let file_type = entry.file_type().map_err(|source| PortError::Io {
            path: source_path.clone(),
            source,
        })?;
        let relative = source_path
            .strip_prefix(root)
            .unwrap_or(&source_path)
            .to_string_lossy()
            .replace('\\', "/");

        if file_type.is_symlink() {
            summary.symlinks_excluded += 1;
            continue;
        }
        if file_type.is_dir() {
            if should_skip_dir(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let target = destination.join(entry.file_name());
            fs::create_dir_all(&target).map_err(|source| PortError::Io {
                path: target.clone(),
                source,
            })?;
            copy_snapshot_dir(root, &source_path, &target, summary)?;
            set_read_only(&target)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if is_sensitive_snapshot_path(&source_path) {
            summary.sensitive_files_excluded.push(relative);
            continue;
        }

        let metadata = entry.metadata().map_err(|source| PortError::Io {
            path: source_path.clone(),
            source,
        })?;
        if metadata.len() > MAX_SNAPSHOT_FILE_BYTES {
            summary.large_files_excluded.push(relative);
            continue;
        }
        summary.bytes_copied = summary
            .bytes_copied
            .checked_add(metadata.len())
            .ok_or(PortError::SnapshotTooLarge)?;
        if summary.bytes_copied > MAX_TOTAL_SNAPSHOT_BYTES {
            return Err(PortError::SnapshotTooLarge);
        }
        summary.files_copied += 1;
        if summary.files_copied > MAX_FILES {
            return Err(PortError::TooManyFiles);
        }

        let target = destination.join(entry.file_name());
        fs::copy(&source_path, &target).map_err(|source| PortError::Io {
            path: source_path.clone(),
            source,
        })?;
        set_read_only(&target)?;
    }
    Ok(())
}

fn is_sensitive_snapshot_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    name == ".env"
        || name.starts_with(".env.")
        || matches!(name.as_str(), ".ds_store" | "thumbs.db")
        || matches!(
            name.as_str(),
            ".npmrc"
                | ".pypirc"
                | ".netrc"
                | "credentials"
                | "credentials.json"
                | "secrets.json"
                | "id_rsa"
                | "id_ed25519"
        )
        || matches!(
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "pem" | "key" | "p12" | "pfx"
        )
}

fn set_read_only(path: &Path) -> Result<()> {
    let mut permissions = fs::metadata(path)
        .map_err(|source| PortError::Io {
            path: path.to_path_buf(),
            source,
        })?
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).map_err(|source| PortError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn scan_dir(root: &Path, dir: &Path, analysis: &mut Analysis) -> Result<()> {
    let entries = fs::read_dir(dir).map_err(|source| PortError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    let mut entries = entries
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|source| PortError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| PortError::Io {
            path: path.clone(),
            source,
        })?;

        if file_type.is_symlink() {
            analysis.scan.symlinks_skipped += 1;
            continue;
        }
        if file_type.is_dir() {
            if should_skip_dir(&entry.file_name().to_string_lossy()) {
                continue;
            }
            scan_dir(root, &path, analysis)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        analysis.scan.files_seen += 1;
        if analysis.scan.files_seen > MAX_FILES {
            return Err(PortError::TooManyFiles);
        }

        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        inspect_filename(&relative, analysis);

        let metadata = entry.metadata().map_err(|source| PortError::Io {
            path: path.clone(),
            source,
        })?;
        if metadata.len() > MAX_SCANNED_FILE_BYTES {
            analysis.scan.skipped_large_files += 1;
            analysis.binary_files += 1;
            continue;
        }
        if !is_probably_text_path(&path) {
            analysis.binary_files += 1;
            continue;
        }

        let bytes = fs::read(&path).map_err(|source| PortError::Io {
            path: path.clone(),
            source,
        })?;
        if bytes.iter().take(8192).any(|byte| *byte == 0) {
            analysis.binary_files += 1;
            continue;
        }
        analysis.scan.bytes_scanned = analysis
            .scan
            .bytes_scanned
            .checked_add(bytes.len() as u64)
            .ok_or(PortError::TooManyBytes)?;
        if analysis.scan.bytes_scanned > MAX_TOTAL_SCANNED_BYTES {
            return Err(PortError::TooManyBytes);
        }
        analysis.scan.files_scanned += 1;
        analysis.source_files += 1;
        let text = String::from_utf8_lossy(&bytes);
        inspect_content(&relative, &text, analysis);
    }
    Ok(())
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".venv"
            | "venv"
            | "__pycache__"
            | "vendor"
            | "Pods"
            | "DerivedData"
            | "xcuserdata"
            | ".swiftpm"
            | ".idea"
            | ".vscode"
            | ".claude"
            | ".codex"
            | ".codex_work"
            | ".openai"
            | ".cache"
    )
}

fn is_probably_text_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if matches!(
        name,
        "Cargo.toml"
            | "Cargo.lock"
            | "package.json"
            | "pyproject.toml"
            | "requirements.txt"
            | "setup.py"
            | "go.mod"
            | "go.sum"
            | "CMakeLists.txt"
            | "Makefile"
            | "meson.build"
            | "manifest.toml"
    ) {
        return true;
    }
    matches!(
        path.extension().and_then(|ext| ext.to_str()).unwrap_or(""),
        "rs" | "toml"
            | "json"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "mjs"
            | "cjs"
            | "py"
            | "go"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "swift"
            | "m"
            | "mm"
            | "cs"
            | "csproj"
            | "fsproj"
            | "java"
            | "kt"
            | "kts"
            | "html"
            | "css"
            | "scss"
            | "xml"
            | "yaml"
            | "yml"
            | "md"
            | "wit"
            | "sh"
            | "ps1"
    )
}

fn inspect_filename(path: &str, analysis: &mut Analysis) {
    let name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let extension = Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");

    match name {
        "Cargo.toml" => {
            analysis.languages.insert("rust".to_string());
            analysis.entry_points.insert(path.to_string());
        }
        "package.json" => {
            analysis
                .languages
                .insert("javascript-or-typescript".to_string());
            analysis.entry_points.insert(path.to_string());
        }
        "pyproject.toml" | "requirements.txt" | "setup.py" => {
            analysis.languages.insert("python".to_string());
            analysis.entry_points.insert(path.to_string());
        }
        "go.mod" => {
            analysis.languages.insert("go".to_string());
            analysis.entry_points.insert(path.to_string());
        }
        "Package.swift" | "project.pbxproj" => {
            analysis.languages.insert("swift".to_string());
            analysis.entry_points.insert(path.to_string());
        }
        "CMakeLists.txt" | "meson.build" | "Makefile" => {
            analysis.languages.insert("c-or-cpp".to_string());
            analysis.entry_points.insert(path.to_string());
        }
        "manifest.toml" => {
            analysis.entry_points.insert(path.to_string());
        }
        _ => {}
    }

    match extension {
        "rs" => {
            analysis.languages.insert("rust".to_string());
        }
        "ts" | "tsx" => {
            analysis.languages.insert("typescript".to_string());
        }
        "js" | "jsx" | "mjs" | "cjs" => {
            analysis.languages.insert("javascript".to_string());
        }
        "py" => {
            analysis.languages.insert("python".to_string());
        }
        "go" => {
            analysis.languages.insert("go".to_string());
        }
        "c" | "h" | "cc" | "cpp" | "hpp" => {
            analysis.languages.insert("c-or-cpp".to_string());
        }
        "swift" => {
            analysis.languages.insert("swift".to_string());
        }
        "cs" | "csproj" | "fsproj" => {
            analysis.languages.insert("dotnet".to_string());
        }
        "java" | "kt" | "kts" => {
            analysis.languages.insert("jvm".to_string());
        }
        _ => {}
    }
}

fn inspect_content(path: &str, text: &str, analysis: &mut Analysis) {
    if !should_analyze_content(path) {
        return;
    }
    if path.ends_with("Cargo.toml") {
        collect_direct_dependencies(text, &mut analysis.direct_dependencies);
    }
    inspect_krate_manifest(path, text, analysis);
    let lower = text.to_ascii_lowercase();
    if path.ends_with(".swift")
        && (lower.contains("@main")
            || path.ends_with("AppDelegate.swift")
            || path.ends_with("SceneDelegate.swift"))
    {
        analysis.entry_points.insert(path.to_string());
    }

    detect_framework(path, text, &lower, analysis);
    detect_pattern(
        analysis,
        "filesystem",
        &[
            "std::fs",
            "node:fs",
            "require(\"fs\")",
            "require('fs')",
            "pathlib",
            "os.open(",
            "filemanager.",
            "system.io.file",
        ],
        path,
        text,
        Severity::Change,
        "Local filesystem use",
        "Map each path to an app-scoped Krate file capability and remove ambient filesystem access.",
        Some("fs.read:<path> / fs.write:<path>"),
    );
    // Listing, creating, and deleting are separate grants from reading and
    // writing, because "may read the files I picked" and "may delete things"
    // are different questions to put in front of a person.
    detect_pattern(
        analysis,
        "directory-management",
        &[
            "read_dir",
            "create_dir",
            "remove_dir",
            "remove_file",
            "walkdir",
            "os.listdir",
            "os.makedirs",
            "shutil.rmtree",
            "fs.readdir",
            "fs.mkdir",
            "fs.unlink",
        ],
        path,
        text,
        Severity::Change,
        "Listing, creating, or deleting files",
        "These are separate Krate capabilities from reading and writing: `fs.list`, `fs.mkdir`, and `fs.remove`. Scope each to the narrowest path the app actually needs.",
        Some("fs.list:<path> / fs.mkdir:<path> / fs.remove:<path>"),
    );
    // The system open/save dialog. Promoted to an explicit ask the day rfd
    // landed on all three hosts, which made it requestable -- and a
    // requestable capability the analyzer cannot spot means a ported app's
    // plan omits a permission it will need (the cross-check test enforces
    // exactly that).
    detect_pattern(
        analysis,
        "file-dialog",
        &[
            "rfd::",
            "filedialog",
            "file_dialog",
            "pick_file",
            "nsopenpanel",
            "nssavepanel",
            "getopenfilename",
            "getsavefilename",
            "qfiledialog",
            "showopendialog",
            "showsavedialog",
            "tkinter.filedialog",
        ],
        path,
        text,
        Severity::Change,
        "Asks the person to pick a file",
        "Krate has this as `ui.dialog:file-open` / `ui.dialog:file-save`: the person's click is the grant, and the app receives a token for the one file chosen, never a path. Declare it, and drop any code that remembers file locations between runs -- the token does not survive the session.",
        Some("ui.dialog:file-open / ui.dialog:file-save"),
    );

    // Playing sound. Distinct from capturing it: a person granting a metronome
    // permission to make noise is not granting it the microphone.
    detect_pattern(
        analysis,
        "audio-playback",
        &[
            "rodio",
            "cpal::",
            "audio_output",
            "play_sound",
            "avaudioplayer",
            "mediaplayer",
            "new audio(",
            "playsound",
            "pygame.mixer",
        ],
        path,
        text,
        Severity::Change,
        "Playing sound",
        "`audio.playback` is declared but the runtime refuses every call to it today, so a ported app cannot make a sound. Remove the audio, or wait for it. `audio.capture` -- the microphone -- does work, and is a separate capability.",
        Some("audio.playback"),
    );
    // Files dragged onto the window. Worth naming because it is the one way an
    // app receives a file without a dialog, and people do not think of a drop
    // as a permission at all.
    detect_pattern(
        analysis,
        "file-drop",
        &[
            // Deliberately specific. `on_drop` alone matched Rust's own
            // `kill_on_drop`, which is the Drop trait and has nothing to do
            // with dragging a file onto a window -- so a headless RSS tool was
            // told to declare a capability it could not use. A false positive
            // is worse than a miss here: it puts a permission in front of a
            // person that the app never needed.
            "droppedfile",
            "dropped_file",
            "ondrop=",
            "on_file_drop",
            "dragenter",
            "draggingentered",
            "wm_dropfiles",
            "drag_and_drop",
            "hovered_files",
        ],
        path,
        text,
        Severity::Change,
        "Files dragged onto the window",
        // ui.dropzone is declared in the manifest specs but the runtime does
        // not deliver drop events yet (K-175). Until it does, recommending it
        // would put a hollow promise on the consent sheet -- point at the
        // dialog that works instead, and this note flips back when K-175
        // lands.
        "Krate does not deliver drag-and-drop events yet (K-175). Use `ui.dialog:file-open` for now: a picker hands the app the same file with one extra click.",
        Some("ui.dialog:file-open"),
    );
    // Running work on the GPU, as opposed to drawing with it. Basic drawing is
    // granted to every app; compute is not, because it is a general-purpose
    // processor an app can keep busy.
    detect_pattern(
        analysis,
        "gpu-compute",
        &[
            "compute_shader",
            "computepipeline",
            "dispatch_workgroups",
            "wgpu::computepass",
            "cuda",
            "opencl",
            "metalperformanceshaders",
            "torch.cuda",
        ],
        path,
        text,
        Severity::Change,
        "GPU compute",
        "Drawing with the GPU is granted to every Krate app; running compute work on it is `gfx.gpu:compute` and has to be asked for, because it is a general-purpose processor the app can keep busy.",
        Some("gfx.gpu:compute"),
    );
    // A system menu bar. Declared in the WIT but not implemented on any host
    // yet, so this is a heads-up rather than a mapping.
    detect_pattern(
        analysis,
        "menu-bar",
        &[
            "nsmenu",
            "menubar",
            "menu_bar",
            "setmenu",
            "gtk_menu",
            "qmenubar",
            "createmenu",
        ],
        path,
        text,
        Severity::Change,
        "System menu bar",
        "Krate declares `ui.menu` but no host implements it yet, because the three systems disagree about where a menu bar lives. Move these actions into the window -- buttons or a list -- or the port will lose them.",
        Some("ui.menu"),
    );
    // Rust crates that wrap a C library. These do not compile to wasm at all --
    // there is no C toolchain in the target, and the library they bind is a
    // native object file. An image compressor that pulls mozjpeg-sys looks
    // ordinary in its own Cargo.toml and fails at the first build, an hour in.
    detect_pattern(
        analysis,
        "native-library-binding",
        &[
            // Named crates rather than a `-sys` suffix rule: `wasm-bindgen` is
            // a WebAssembly tool and matched a bare `bindgen`, marking a port
            // that works as unsupported. A false blocker is worse than a miss
            // -- it tells someone not to try something that would have worked.
            "mozjpeg-sys",
            "openssl-sys",
            "libgit2-sys",
            "ffmpeg-sys",
            "alsa-sys",
            "\nbindgen = ",
            "cc::Build::new",
        ],
        path,
        text,
        Severity::Blocker,
        "Binds a native C library",
        "A crate that wraps a C library cannot be built for Krate: the target has no C toolchain, and the library it binds is a native object file. Replace it with a pure-Rust equivalent -- `zune-png` and `zune-jpeg` for decoding (not `image`, which requires std and drags the whole `wasi:*` surface in), `oxipng` for compression, `rusqlite` bundled or Krate's own `store.sql` for databases -- or the port cannot start.",
        None,
    );
    detect_pattern(
        analysis,
        "network",
        &[
            "reqwest",
            "ureq",
            "fetch(",
            "axios",
            "node:http",
            "node:https",
            "urllib",
            "requests.",
            "urlsession",
            "httpclient",
        ],
        path,
        text,
        Severity::Change,
        "Network access",
        "Declare each destination and route requests through Krate's network capability.",
        Some("net.connect:<host>:<port>"),
    );
    detect_pattern(
        analysis,
        "process",
        &[
            "std::process::command",
            "child_process",
            "subprocess",
            "os.system(",
            "processbuilder",
            "system.diagnostics.process",
            "nstask",
        ],
        path,
        text,
        Severity::Blocker,
        "Process or shell execution",
        "Krate has no general process-spawning capability. Replace this behavior or define a reviewed narrow host capability.",
        None,
    );
    detect_pattern(
        analysis,
        "dynamic-library",
        &[
            "libloading",
            "dlopen(",
            "loadlibrary",
            "ctypes.cdll",
            "ffi_lib",
            "jna.",
        ],
        path,
        text,
        Severity::Blocker,
        "Dynamic native library loading",
        "A portable component cannot load an arbitrary host library. Port the dependency to WebAssembly or replace it with a Krate host interface.",
        None,
    );
    detect_pattern(
        analysis,
        "database",
        &[
            "sqlite",
            "postgres",
            "mysql",
            "mongodb",
            "indexeddb",
            "coredata",
        ],
        path,
        text,
        Severity::Change,
        "Database dependency",
        "Krate has an app-scoped SQLite database (store.sql): tables, queries, and transactions, addressed as SQL and never as a file. A server database (Postgres, MySQL, MongoDB) still needs replacing with local storage or a network call.",
        Some("store.sql"),
    );
    detect_pattern(
        analysis,
        "settings",
        &[
            // Every desktop framework's "remember this between launches" API.
            // These are the single most common reason a small app touches
            // storage at all, and they map exactly onto store.kv.
            "userdefaults",
            "nsuserdefaults",
            "localstorage",
            "electron-store",
            "confy",
            "preferences",
            "app.config",
            "settings.json",
            "configparser",
        ],
        path,
        text,
        Severity::Change,
        "Saved settings or preferences",
        "Krate's app-scoped key-value store covers this. The app addresses values by key and never names a path, so it does not need a filesystem grant.",
        Some("store.kv"),
    );
    // Random numbers. getrandom is the third most-downloaded crate in Rust and
    // rand and uuid sit on it, so this is one of the most common things a real
    // program does -- and the analyzer could not see any of it.
    detect_pattern(
        analysis,
        "random",
        &[
            "getrandom",
            "rand::",
            "use rand",
            "uuid::",
            "thread_rng",
            "os_rng",
            "secrets.token",
            "crypto.getrandomvalues",
        ],
        path,
        text,
        Severity::Change,
        "Random numbers",
        "Krate draws random bytes from the operating system behind the `random.bytes` capability. There is no seeded generator: an app handed a predictable stream while believing it is random has no way to find out.",
        Some("random.bytes"),
    );
    // Copy and paste. Supported on all three systems, and an app that reaches
    // for it needs to say so, because reading the clipboard reads whatever the
    // person last copied.
    detect_pattern(
        analysis,
        "clipboard",
        &[
            "arboard",
            "clipboard::",
            "nspasteboard",
            "setclipboarddata",
            "navigator.clipboard",
            "gtk_clipboard",
        ],
        path,
        text,
        Severity::Change,
        "Clipboard access",
        "Krate splits this into `ui.clipboard:read` and `ui.clipboard:write`, because reading the clipboard sees whatever the person last copied and writing to it does not.",
        Some("ui.clipboard:read / ui.clipboard:write"),
    );
    // Tokens and keys. Distinct from settings: these are the things that must
    // not sit in a plain file, and the store that holds them is encrypted.
    detect_pattern(
        analysis,
        "secrets",
        &[
            "keyring",
            "keychain",
            "secret_service",
            "credential_manager",
            "api_key",
            "access_token",
            "refresh_token",
        ],
        path,
        text,
        Severity::Change,
        "Sign-in tokens or keys",
        "Krate's `store.secret` capability keeps these encrypted at rest, per app and per machine, so a copied file does not carry usable secrets to another computer. It is separate from `store.kv` because a token is not a setting.",
        Some("store.secret"),
    );
    // Multi-user or synced state: the program talks about accounts, shared
    // lists, or a sync backend. Krate's answer is the shared store -- a
    // bucket synced between the machines holding an invite code, no
    // accounts and no server to port.
    detect_pattern(
        analysis,
        "shared-state",
        &[
            "firebase",
            "supabase",
            "realtime sync",
            "multi-user",
            "multiplayer state",
            "shared_list",
            "family_group",
            "collaborat",
        ],
        path,
        text,
        Severity::Change,
        "State shared between people or devices",
        "Krate's `store.shared` capability is a key-value bucket synced between every machine holding its invite code, through krate.tech -- no accounts, no backend to run. Ports that lean on a realtime service map their shared documents onto per-item keys there.",
        Some("store.shared"),
    );
    detect_pattern(
        analysis,
        "notifications",
        &[
            "notify-rust",
            "notify_rust",
            "unusernotification",
            "toastnotification",
            "new notification(",
            "notification.requestpermission",
        ],
        path,
        text,
        Severity::Change,
        "System notifications",
        "Krate has a notification capability (ui.notify). It shows a title and body attributed to the app, and carries no reply channel, so code that reacts to a click needs replacing.",
        Some("ui.notify"),
    );
    detect_pattern(
        analysis,
        "open-url",
        &[
            // Every framework's "open this link in the browser".
            "webbrowser::open",
            "shell.openexternal",
            "nsworkspace.*openurl",
            "start_process",
            "xdg-open",
            "webbrowser.open",
            "opener::open",
            "open::that",
        ],
        path,
        text,
        Severity::Change,
        "Opening links in a browser",
        "Krate can hand a link to the person's browser (ui.open-url). It accepts https and mailto; file:// and custom schemes are refused because they read files or start other programs.",
        Some("ui.open-url"),
    );
    detect_pattern(
        analysis,
        "tray",
        &[
            "systemtray",
            "trayicon",
            "tray_icon",
            "nsstatusitem",
            "notifyicon",
        ],
        path,
        text,
        Severity::Blocker,
        "System tray or menu bar integration",
        "Krate does not yet provide a three-host tray capability.",
        None,
    );
    detect_pattern(
        analysis,
        "microphone",
        &[
            "getusermedia",
            "navigator.mediadevices",
            "avaudiosession",
            "avcaptureaudio",
            "microphone",
            "default_input_device",
            "wasapi capture",
        ],
        path,
        text,
        Severity::Change,
        "Microphone capture",
        "Route microphone input through Krate audio.capture. The runtime exposes bounded PCM chunks only after explicit user consent. Speech recognition still needs a declared local model or network service.",
        Some("audio.capture"),
    );
    detect_pattern(
        analysis,
        "camera",
        &[
            "avcapturedevice",
            "avcapturesession",
            "getusermedia({video",
            "video: true",
            "mediafoundation",
            "imfsourcereader",
            "videocapture",
            "v4l2",
            "/dev/video",
            "webcam",
            "capturedevice",
        ],
        path,
        text,
        Severity::Change,
        "Camera capture",
        "Route camera frames through Krate camera.capture: open, start, then poll read each time round the event loop. Frames arrive as straight-alpha RGBA carrying their own width and height, which is what canvas2d::draw_pixels takes -- draw at the frame's size, never the size you asked for. Mark camera.capture required: the person is only asked about required capabilities, so an optional one is never granted.",
        Some("camera.capture"),
    );
    detect_pattern(
        analysis,
        "hardware",
        &[
            "serialport",
            "libusb",
            "hidapi",
            "corebluetooth",
            "web bluetooth",
            "navigator.bluetooth",
            "videocapturedevice",
            "avcapturevideo",
            "camera",
        ],
        path,
        text,
        Severity::Blocker,
        "Camera or hardware access",
        "This hardware boundary has no reviewed Krate capability yet.",
        None,
    );
    detect_pattern(
        analysis,
        "platform-conditional",
        &[
            "cfg(target_os",
            "process.platform",
            "operatingsystem.iswindows",
            "operatingsystem.ismacos",
            "#if os(",
            "runtime.goos",
        ],
        path,
        text,
        Severity::Change,
        "Operating-system-specific code",
        "Replace platform branches with a portable Krate interface or disclose behavior that cannot be preserved.",
        None,
    );
}

fn inspect_krate_manifest(path: &str, text: &str, analysis: &mut Analysis) {
    if !path.ends_with("manifest.toml") || !text.contains("krate:app/") {
        return;
    }
    match krate_manifest::Manifest::parse(text) {
        Ok(manifest) => {
            analysis.has_krate_manifest = true;
            if let Ok(capabilities) = manifest.declared_capabilities() {
                analysis.capabilities.extend(
                    capabilities
                        .into_iter()
                        .map(|capability| capability.to_string()),
                );
            }
        }
        Err(error) => add_evidence_finding(
            analysis,
            FindingSpec {
                id: "invalid-krate-manifest".to_string(),
                severity: Severity::Blocker,
                confidence: Confidence::High,
                title: "Invalid Krate manifest".to_string(),
                detail: format!(
                    "The existing manifest must validate before this app is ready: {error}"
                ),
                capability: None,
            },
            Evidence {
                path: path.to_string(),
                line: None,
            },
        ),
    }
}

fn should_analyze_content(path: &str) -> bool {
    let path = Path::new(path);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if matches!(
        name,
        "Cargo.lock" | "go.sum" | "package-lock.json" | "yarn.lock" | "pnpm-lock.yaml"
    ) {
        return false;
    }
    // Generated packaging manifests list every crate in the dependency tree by
    // name and download URL, so any pattern matched there describes something
    // a dependency might do rather than anything the app does. A markdown
    // viewer was reported as loading native libraries at run time because the
    // string `libloading` appeared in a crates.io URL inside its Flatpak
    // sources file. The app itself never loads a library.
    if matches!(
        name,
        "cargo-sources.json"
            | "generated-sources.json"
            | "flatpak-sources.json"
            | "cargo-lock.json"
    ) {
        return false;
    }
    if name == "bindings.rs"
        || name.ends_with(".generated.rs")
        || name.ends_with(".g.rs")
        || name.ends_with(".min.js")
    {
        return false;
    }

    !matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("md" | "txt")
    )
}

fn detect_framework(path: &str, text: &str, lower: &str, analysis: &mut Analysis) {
    let file = Path::new(path);
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    let is_package_json = name == "package.json";
    let is_js_source = matches!(extension, "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs");
    let is_native_source = matches!(
        extension,
        "rs" | "py" | "swift" | "m" | "mm" | "cs" | "xml" | "c" | "cc" | "cpp" | "h" | "hpp"
    );

    let frameworks: [(&str, &[&str], bool); 16] = [
        (
            "electron",
            &["\"electron\"", "from 'electron'", "from \"electron\""],
            is_package_json || is_js_source,
        ),
        (
            "tauri",
            &["tauri::", "\"@tauri-apps/", "\"tauri\""],
            is_package_json || is_js_source || extension == "rs",
        ),
        (
            "react",
            &["\"react\"", "from 'react'", "from \"react\""],
            is_package_json || is_js_source,
        ),
        (
            "vite",
            &["\"vite\"", "vite.config."],
            is_package_json || name.starts_with("vite.config."),
        ),
        (
            "nextjs",
            &["\"next\"", "next.config."],
            is_package_json || name.starts_with("next.config."),
        ),
        (
            "qt",
            &["qapplication", "qtwidgets", "pyside", "pyqt"],
            is_native_source,
        ),
        ("gtk", &["gtk::", "gtk4", "pygobject"], is_native_source),
        (
            "appkit",
            &["import appkit", "nsapplication", "nswindow"],
            is_native_source,
        ),
        (
            "swiftui",
            &["import swiftui", "@main struct"],
            is_native_source,
        ),
        (
            "wpf",
            &["presentationframework", "<window x:class"],
            is_native_source,
        ),
        ("winui", &["microsoft.ui.xaml", "winui"], is_native_source),
        // The Rust-native GUI toolkits. Missing these meant a real eframe app
        // was reported as "Frameworks: not detected" and handed the CLI
        // profile -- the analyzer could not see it drew a window at all, which
        // is the first thing a port of it has to know.
        (
            "egui",
            &["eframe::", "egui::", "eframe =", "egui ="],
            is_native_source,
        ),
        ("iced", &["iced::", "iced ="], is_native_source),
        (
            "slint",
            &["slint::", "slint =", "slint_build"],
            is_native_source,
        ),
        ("dioxus", &["dioxus::", "dioxus ="], is_native_source),
        // winit is the window layer the others sit on, and some apps use it
        // directly. Listed last so a more specific toolkit is named first.
        ("winit", &["winit::", "winit ="], is_native_source),
    ];
    for (framework, patterns, relevant_file) in frameworks {
        if relevant_file && patterns.iter().any(|pattern| lower.contains(pattern)) {
            analysis.frameworks.insert(framework.to_string());
            // A windowed app needs a window. Capabilities were only ever
            // suggested from host-call patterns, so an app the analyzer had
            // just identified as a GUI toolkit came back with an empty list --
            // not even the one capability its whole category requires.
            if is_windowed_framework(framework) {
                analysis.capabilities.insert("ui.window:create".to_string());
            }
            add_evidence_finding(
                analysis,
                FindingSpec {
                    id: format!("framework-{framework}"),
                    severity: Severity::Change,
                    confidence: Confidence::High,
                    title: format!("{} application framework", framework_label(framework)),
                    detail: framework_advice(framework).to_string(),
                    capability: None,
                },
                Evidence {
                    path: path.to_string(),
                    line: first_matching_line(text, patterns),
                },
            );
        }
    }
}

/// Whether this framework draws its own window, and therefore needs
/// `ui.window:create` no matter what else the app does.
fn is_windowed_framework(framework: &str) -> bool {
    matches!(
        framework,
        "appkit"
            | "swiftui"
            | "wpf"
            | "winui"
            | "qt"
            | "gtk"
            | "egui"
            | "iced"
            | "slint"
            | "dioxus"
            | "winit"
    )
}

fn framework_label(framework: &str) -> &str {
    match framework {
        "nextjs" => "Next.js",
        "appkit" => "AppKit",
        "swiftui" => "SwiftUI",
        "wpf" => "WPF",
        "winui" => "WinUI",
        "qt" => "Qt",
        "gtk" => "GTK",
        "tauri" => "Tauri",
        "react" => "React",
        "vite" => "Vite",
        "electron" => "Electron",
        "egui" => "egui/eframe",
        "iced" => "Iced",
        "slint" => "Slint",
        "dioxus" => "Dioxus",
        "winit" => "winit",
        _ => framework,
    }
}

fn framework_advice(framework: &str) -> &str {
    match framework {
        "electron" | "tauri" => {
            "Reuse portable application logic where possible, then replace the privileged native bridge with declared Krate capabilities. A restricted web UI profile is planned."
        }
        "react" | "vite" | "nextjs" => {
            "The UI may fit the planned restricted web profile. Local and server behavior must be mapped separately."
        }
        "appkit" | "swiftui" | "wpf" | "winui" | "qt" | "gtk" => {
            "This native UI must be translated to Krate's certified widget profile. Unsupported widgets and platform integrations need explicit replacements."
        }
        "egui" | "iced" | "slint" | "dioxus" | "winit" => {
            "This is an immediate-mode or declarative Rust UI. The layout and business logic usually port well; the draw loop becomes a Krate widget tree, and anything drawn to a raw canvas needs an explicit replacement."
        }
        _ => "Map this framework to a supported Krate portability profile.",
    }
}

#[allow(clippy::too_many_arguments)]
fn detect_pattern(
    analysis: &mut Analysis,
    id: &str,
    patterns: &[&str],
    path: &str,
    text: &str,
    severity: Severity,
    title: &str,
    detail: &str,
    capability: Option<&str>,
) {
    let lower = text.to_ascii_lowercase();
    if !patterns.iter().any(|pattern| lower.contains(pattern)) {
        return;
    }
    if let Some(capability) = capability {
        analysis.capabilities.insert(capability.to_string());
    }
    add_evidence_finding(
        analysis,
        FindingSpec {
            id: id.to_string(),
            severity,
            confidence: Confidence::Medium,
            title: title.to_string(),
            detail: detail.to_string(),
            capability: capability.map(str::to_string),
        },
        Evidence {
            path: path.to_string(),
            line: first_matching_line(text, patterns),
        },
    );
}

fn first_matching_line(text: &str, patterns: &[&str]) -> Option<usize> {
    text.lines().enumerate().find_map(|(index, line)| {
        let lower = line.to_ascii_lowercase();
        patterns
            .iter()
            .any(|pattern| lower.contains(pattern))
            .then_some(index + 1)
    })
}

struct FindingSpec {
    id: String,
    severity: Severity,
    confidence: Confidence,
    title: String,
    detail: String,
    capability: Option<String>,
}

fn add_evidence_finding(analysis: &mut Analysis, finding: FindingSpec, evidence: Evidence) {
    analysis
        .findings
        .entry(finding.id.clone())
        .and_modify(|existing| {
            if !existing.evidence.contains(&evidence) {
                existing.evidence.push(evidence.clone());
            }
        })
        .or_insert(Finding {
            id: finding.id,
            severity: finding.severity,
            confidence: finding.confidence,
            title: finding.title,
            detail: finding.detail,
            capability: finding.capability,
            evidence: vec![evidence],
        });
}

fn finish_plan(source: PathBuf, mut analysis: Analysis) -> Result<PortPlan> {
    if analysis.source_files == 0 && analysis.binary_files > 0 {
        add_evidence_finding(
            &mut analysis,
            FindingSpec {
                id: "binary-only".to_string(),
                severity: Severity::Blocker,
                confidence: Confidence::High,
                title: "No portable source was found".to_string(),
                detail: "Krate cannot turn opaque native binaries into one safe cross-platform component. Provide source code or a standard WebAssembly component.".to_string(),
                capability: None,
            },
            Evidence {
                path: ".".to_string(),
                line: None,
            },
        );
    }

    // A language the port pipeline cannot build. `krate port --to` builds the
    // candidate with cargo-component, so today that means Rust and nothing
    // else. Saying "needs changes" about a Python project put it in the same
    // category as a Rust project that ports cleanly, and `--prepare` then laid
    // down a Rust scaffold without ever mentioning that the language has to
    // change -- which someone would only discover by reading the file.
    if !analysis.languages.is_empty() && !analysis.languages.contains("rust") {
        let found: Vec<String> = analysis.languages.iter().cloned().collect();
        add_evidence_finding(
            &mut analysis,
            FindingSpec {
                id: "language-not-buildable".to_string(),
                severity: Severity::Blocker,
                confidence: Confidence::High,
                title: format!("Krate cannot build {} yet", found.join(", ")),
                detail: format!(
                    "The port pipeline compiles the candidate with cargo-component, so it can \
                     build Rust today and nothing else. This project is {}. Its logic can still \
                     be ported by rewriting it in Rust against the Krate SDK -- the analysis \
                     above still says which capabilities it would need -- but `krate port --to` \
                     cannot do that step for you.",
                    found.join(" and ")
                ),
                capability: None,
            },
            Evidence {
                path: ".".to_string(),
                line: None,
            },
        );
    }

    let has_blocker = analysis
        .findings
        .values()
        .any(|finding| finding.severity == Severity::Blocker);
    let verdict = if has_blocker {
        Verdict::Unsupported
    } else if analysis.has_krate_manifest
        && analysis
            .findings
            .values()
            .all(|finding| finding.severity == Severity::Info)
    {
        Verdict::Ready
    } else {
        Verdict::NeedsChanges
    };

    let profile = select_profile(&analysis);
    let next_steps = next_steps(&analysis, &verdict, &profile);
    Ok(PortPlan {
        schema: "krate.port.plan.v1".to_string(),
        source: source.to_string_lossy().into_owned(),
        verdict,
        profile,
        languages: analysis.languages.into_iter().collect(),
        frameworks: analysis.frameworks.into_iter().collect(),
        entry_points: analysis.entry_points.into_iter().collect(),
        suggested_capabilities: analysis.capabilities.into_iter().collect(),
        findings: analysis.findings.into_values().collect(),
        next_steps,
        scan: analysis.scan.finish(),
    })
}

fn select_profile(analysis: &Analysis) -> String {
    if analysis.frameworks.contains("electron") {
        return "electron-source-port".to_string();
    }
    if analysis.frameworks.contains("tauri") {
        return "tauri-source-port".to_string();
    }
    if analysis
        .frameworks
        .iter()
        .any(|framework| matches!(framework.as_str(), "react" | "vite" | "nextjs"))
    {
        return "web-local-v1-planned".to_string();
    }
    // Anything that opens a real window. The Rust-native toolkits belong here
    // for the same reason as the platform ones: the port has to translate a UI
    // boundary, not just host calls, and handing one the CLI profile prepares
    // a candidate with no window at all.
    //
    // Same predicate that decides the app needs `ui.window:create`, so the
    // profile and the suggested capability cannot disagree about whether this
    // app draws a window.
    if analysis
        .frameworks
        .iter()
        .any(|framework| is_windowed_framework(framework))
    {
        return "desktop-native-source-port".to_string();
    }
    if analysis.has_krate_manifest {
        return "krate-native".to_string();
    }
    "krate-cli-v1-candidate".to_string()
}

fn next_steps(analysis: &Analysis, verdict: &Verdict, profile: &str) -> Vec<String> {
    if *verdict == Verdict::Unsupported {
        return vec![
            "Review every blocker and decide which behavior can be removed, replaced, or added as a new Krate capability.".to_string(),
            "Run the analyzer again after the blockers have source-level replacements.".to_string(),
        ];
    }

    let mut steps = vec![
        format!("Confirm that the proposed `{profile}` profile preserves the app's required user journeys."),
        "Review the suggested capabilities and replace broad source paths or network destinations with precise scopes.".to_string(),
    ];
    if !analysis.frameworks.is_empty() {
        steps.push(
            "Translate the host and UI boundary while keeping portable business logic unchanged."
                .to_string(),
        );
    }
    steps.push(
        "Build in an isolated work directory, reject unknown component imports, and record the source diff."
            .to_string(),
    );
    steps.push(
        "Verify allow, deny, persistence, close, and reopen journeys on macOS, Windows, and Linux."
            .to_string(),
    );
    steps
}

#[cfg(test)]
mod tests {
    use super::{analyze, snapshot, Severity, Verdict};
    use std::fs;

    #[test]
    fn rust_cli_with_files_and_http_gets_a_useful_plan() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/main.rs"),
            "fn main() { let _ = std::fs::read(\"data\"); let _ = reqwest::get(\"https://example.com\"); }",
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert_eq!(plan.verdict, Verdict::NeedsChanges);
        assert_eq!(plan.profile, "krate-cli-v1-candidate");
        assert!(plan.languages.contains(&"rust".to_string()));
        assert!(plan
            .suggested_capabilities
            .contains(&"fs.read:<path> / fs.write:<path>".to_string()));
        assert!(plan
            .suggested_capabilities
            .contains(&"net.connect:<host>:<port>".to_string()));
    }

    #[test]
    fn electron_process_spawn_is_reported_as_a_blocker() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"electron":"1","react":"1"}}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("main.ts"),
            "import { app } from 'electron'; import { exec } from 'node:child_process';",
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert_eq!(plan.verdict, Verdict::Unsupported);
        assert_eq!(plan.profile, "electron-source-port");
        assert!(plan.frameworks.contains(&"electron".to_string()));
        assert!(plan.findings.iter().any(|finding| finding.id == "process"));
    }

    #[test]
    fn native_framework_is_not_misrepresented_as_ready() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("App.swift"),
            "import SwiftUI\n@main struct App: SwiftUI.App {}",
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        // Swift, which the pipeline cannot build -- the point of the test is that
        // it is not reported as ready, and unsupported is the stronger form of that.
        assert_eq!(plan.verdict, Verdict::Unsupported);
        assert_eq!(plan.profile, "desktop-native-source-port");
        assert!(plan.frameworks.contains(&"swiftui".to_string()));
    }

    #[test]
    fn a_language_the_pipeline_cannot_build_is_a_blocker_not_a_to_do() {
        // A Python project came back "needs changes" -- the same verdict a Rust
        // project that ports cleanly gets -- and `--prepare` then wrote a Rust
        // scaffold without ever saying the language has to change.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.py"), "import random\nprint(1)\n").unwrap();
        fs::write(dir.path().join("requirements.txt"), "requests==2.31.0\n").unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert_eq!(plan.verdict, Verdict::Unsupported);
        let blocker = plan
            .findings
            .iter()
            .find(|f| f.id == "language-not-buildable")
            .expect("a blocker naming the language");
        assert!(blocker.title.contains("python"), "{}", blocker.title);
        // It must say what is still possible, not only what is refused.
        assert!(
            blocker.detail.contains("rewriting it in Rust"),
            "the blocker should name the way forward: {}",
            blocker.detail
        );
    }

    #[test]
    fn a_rust_project_is_not_blocked_by_a_second_language_beside_it() {
        // Plenty of Rust projects carry a build script or a web frontend. The
        // blocker is for projects with no Rust at all, not for mixed ones.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.path().join("build.js"), "console.log(1)\n").unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert_ne!(
            plan.verdict,
            Verdict::Unsupported,
            "a Rust project with a JS file beside it is still portable"
        );
        assert!(!plan
            .findings
            .iter()
            .any(|f| f.id == "language-not-buildable"));
    }

    /// Every capability an app must ask for should be one the analyzer can spot
    /// in source, or the plan quietly leaves work for someone else to find.
    ///
    /// Enumerates the runtime's own capability list rather than sampling it, so
    /// adding a capability without teaching the analyzer to see it fails here
    /// rather than in a user's port. Capabilities granted by default are
    /// excluded: suggesting `io.stdout` would be noise, not information.
    #[test]
    fn every_requestable_capability_is_one_the_analyzer_can_detect() {
        let suggestable: String = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"),
        )
        .expect("read this file");

        let mut missing = Vec::new();
        for spec in krate_manifest::supported_capability_specs() {
            if spec.default_granted() {
                continue;
            }
            let name = spec.name();
            // The suggestion strings pair related grants ("fs.read:<path> /
            // fs.write:<path>"), so a substring match is the honest check.
            if !suggestable.contains(&format!("Some(\"{name}"))
                && !suggestable.contains(&format!("/ {name}"))
            {
                missing.push(name.to_string());
            }
        }

        assert!(
            missing.is_empty(),
            "the runtime supports these but the analyzer cannot spot them in source, \
             so a port of an app that uses one gets a plan that does not mention it: {missing:?}"
        );
    }

    #[test]
    fn a_generated_packaging_manifest_is_not_read_as_the_app_source() {
        // A markdown viewer was reported as loading native libraries at run
        // time -- a blocker -- because the string `libloading` appeared in a
        // crates.io download URL inside its Flatpak sources file. That file
        // lists every crate in the dependency tree, so any pattern matched
        // there describes something a dependency might do, not the app.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"viewer\"\n",
        )
        .unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        fs::create_dir(dir.path().join("flatpak")).unwrap();
        fs::write(
            dir.path().join("flatpak/cargo-sources.json"),
            r#"[{"type":"archive","url":"https://static.crates.io/crates/libloading/libloading-0.8.9.crate"}]"#,
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert!(
            !plan
                .findings
                .iter()
                .any(|f| f.id == "native-dynamic-loading"),
            "a download URL in a generated manifest is not the app loading a library"
        );
        assert_ne!(plan.verdict, Verdict::Unsupported);
    }

    #[test]
    fn the_lockfile_names_a_native_binding_without_condemning_every_sys_crate() {
        // The `-sys` suffix is not the signal. `js-sys` and `web-sys` are
        // WebAssembly's own bindings and `windows-sys` is generated syscall
        // declarations; every proven port carries several, and matching the
        // suffix marked all six as unsupported. What marks a real binding is a
        // build dependency on system-deps or pkg-config: those go looking for a
        // library already installed on the machine, and wasm has none.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"viewer\"\n",
        )
        .unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(
            dir.path().join("Cargo.lock"),
            r#"
[[package]]
name = "js-sys"
version = "0.3"

[[package]]
name = "windows-sys"
version = "0.52"

[[package]]
name = "dav1d-sys"
version = "0.8.3"
dependencies = [
 "libc",
 "system-deps",
]
"#,
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        let finding = plan
            .findings
            .iter()
            .find(|f| f.id == "native-library-binding")
            .expect("dav1d-sys links a system library");
        assert!(
            finding.detail.contains("dav1d-sys"),
            "the finding must name the crate: {}",
            finding.detail
        );
        for harmless in ["js-sys", "windows-sys"] {
            assert!(
                !finding.detail.contains(harmless),
                "{harmless} builds for wasm and must not be named: {}",
                finding.detail
            );
        }

        // A change, not a blocker. Every proven port carries one of these under
        // a dependency the port replaces outright -- openssl-sys under an HTTP
        // client, libsqlite3-sys under a database. Blocking would have told six
        // people not to attempt six ports that work.
        assert_eq!(finding.severity, crate::Severity::Change);
        assert_ne!(plan.verdict, Verdict::Unsupported);
    }

    #[test]
    fn a_c_library_binding_blocks_the_port_and_wasm_bindgen_does_not() {
        // A crate wrapping a C library cannot build for wasm at all, and that
        // is worth knowing before an hour is spent finding out. But the first
        // version of this matched a bare `bindgen`, which caught `wasm-bindgen`
        // -- a WebAssembly tool -- and marked a port that works as unsupported.
        // A false blocker is worse than a miss: it tells someone not to try
        // something that would have succeeded.
        let native = tempfile::tempdir().unwrap();
        fs::write(
            native.path().join("Cargo.toml"),
            "[package]\nname = \"squish\"\n\n[dependencies]\nmozjpeg-sys = \"2\"\n",
        )
        .unwrap();
        fs::write(native.path().join("main.rs"), "fn main() {}\n").unwrap();
        let plan = analyze(native.path()).unwrap();
        assert_eq!(plan.verdict, Verdict::Unsupported);
        assert!(plan
            .findings
            .iter()
            .any(|f| f.id == "native-library-binding"));

        let wasm = tempfile::tempdir().unwrap();
        fs::write(
            wasm.path().join("Cargo.toml"),
            "[package]\nname = \"web\"\n\n[dependencies]\nwasm-bindgen = \"0.2\"\n",
        )
        .unwrap();
        fs::write(
            wasm.path().join("main.rs"),
            "use wasm_bindgen::prelude::*;\n",
        )
        .unwrap();
        let plan = analyze(wasm.path()).unwrap();
        assert!(
            !plan
                .findings
                .iter()
                .any(|f| f.id == "native-library-binding"),
            "wasm-bindgen is a WebAssembly tool, not a C binding"
        );
    }

    #[test]
    fn rusts_own_drop_trait_is_not_mistaken_for_drag_and_drop() {
        // `kill_on_drop` is tokio's process API. It matched an `on_drop`
        // pattern and told a headless RSS forwarder to declare ui.dropzone --
        // a window capability, for an app with no window. A false positive is
        // worse than a miss: it puts a permission in front of a person that the
        // app never needed and cannot use.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"feeds\"\n\n[dependencies]\ntokio = \"1\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("main.rs"),
            "let mut p = Command::new(&cmd).kill_on_drop(true).spawn()?;\n",
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert!(
            !plan
                .suggested_capabilities
                .iter()
                .any(|c| c == "ui.dropzone"),
            "kill_on_drop is the Drop trait, not a file drop: {:?}",
            plan.suggested_capabilities
        );
    }

    #[test]
    fn a_real_file_drop_is_still_detected() {
        // The other half: tightening the patterns must not have turned the
        // detection off entirely.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"viewer\"\n\n[dependencies]\neframe = \"0.35\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("main.rs"),
            "for file in &ctx.input(|i| i.raw.dropped_files.clone()) { open(file); }\n",
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        // Until K-175 implements drop events, the honest suggestion for a
        // dropped-files read is the file dialog that works. When dropzone
        // lands, this flips back to ui.dropzone.
        assert!(
            plan.suggested_capabilities
                .iter()
                .any(|c| c == "ui.dialog:file-open"),
            "a real dropped_files read should suggest the working file dialog: {:?}",
            plan.suggested_capabilities
        );
    }

    #[test]
    fn the_analyzer_suggests_the_capabilities_it_actually_has() {
        // The analyzer could suggest seven capabilities while the runtime
        // supported thirty-four. Three of the gaps mattered: random (getrandom
        // is the third most-downloaded crate in Rust), the clipboard, and
        // secrets -- all supported, none detectable.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"app\"\n\n[dependencies]\nrand = \"0.9\"\narboard = \"3\"\nkeyring = \"3\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("main.rs"),
            "use rand::thread_rng;\nuse arboard::Clipboard;\nlet access_token = keyring::Entry::new();\n",
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        for expected in [
            "random.bytes",
            "ui.clipboard:read / ui.clipboard:write",
            "store.secret",
        ] {
            assert!(
                plan.suggested_capabilities.iter().any(|c| c == expected),
                "expected {expected}, got {:?}",
                plan.suggested_capabilities
            );
        }
    }

    #[test]
    fn a_rust_gui_app_is_not_mistaken_for_a_command_line_one() {
        // A real eframe app reported "Frameworks: not detected" and was handed
        // the CLI profile, which prepares a candidate with no window at all.
        // The detector knew Qt, GTK, and WPF but none of the Rust-native
        // toolkits -- the ones a Rust developer would actually reach for.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"budget\"\n\n[dependencies]\neframe = \"0.35.0\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("main.rs"),
            "use eframe::egui;\nfn main() { eframe::run_native(); }\n",
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert!(
            plan.frameworks.contains(&"egui".to_string()),
            "expected egui to be detected, got {:?}",
            plan.frameworks
        );
        assert_eq!(
            plan.profile, "desktop-native-source-port",
            "a windowed app must not get the CLI profile"
        );
    }

    #[test]
    fn every_rust_gui_toolkit_routes_to_the_desktop_profile() {
        // One entry missing from either the detector or the profile match is a
        // silent wrong answer, so check the whole set rather than a sample.
        for (dep, name) in [
            ("eframe", "egui"),
            ("iced", "iced"),
            ("slint", "slint"),
            ("dioxus", "dioxus"),
            ("winit", "winit"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join("Cargo.toml"),
                format!("[package]\nname = \"a\"\n\n[dependencies]\n{dep} = \"1\"\n"),
            )
            .unwrap();
            fs::write(dir.path().join("main.rs"), format!("use {dep}::x;\n")).unwrap();

            let plan = analyze(dir.path()).unwrap();
            assert!(
                plan.frameworks.contains(&name.to_string()),
                "{dep} was not detected as {name}: {:?}",
                plan.frameworks
            );
            assert_eq!(
                plan.profile, "desktop-native-source-port",
                "{dep} should route to the desktop profile"
            );
            // A windowed app needs a window. The analyzer used to identify the
            // toolkit and then suggest nothing at all, leaving the one
            // capability its whole category requires for someone else to work
            // out.
            assert!(
                plan.suggested_capabilities
                    .contains(&"ui.window:create".to_string()),
                "{dep} is a windowed toolkit but ui.window:create was not suggested: {:?}",
                plan.suggested_capabilities
            );
        }
    }

    #[test]
    fn tauri_project_selects_a_source_port_profile() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"@tauri-apps/api":"2","react":"19"}}"#,
        )
        .unwrap();
        fs::write(dir.path().join("main.tsx"), "import React from 'react';").unwrap();

        let plan = analyze(dir.path()).unwrap();
        // No Rust in this project, so the pipeline cannot build it -- the
        // profile is still the right one for the eventual rewrite.
        assert_eq!(plan.verdict, Verdict::Unsupported);
        assert_eq!(plan.profile, "tauri-source-port");
        assert!(plan.frameworks.contains(&"tauri".to_string()));
    }

    #[test]
    fn microphone_use_maps_to_capture_instead_of_a_hardware_blocker() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"react":"19"}}"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("prompter.ts"),
            "navigator.mediaDevices.getUserMedia({ audio: true });",
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        // TypeScript, so the project as a whole cannot be built; the microphone
        // finding below is what this test is actually about.
        assert_eq!(plan.verdict, Verdict::Unsupported);
        assert!(plan
            .suggested_capabilities
            .contains(&"audio.capture".to_string()));
        assert!(plan
            .findings
            .iter()
            .any(|finding| finding.id == "microphone"));
        assert!(!plan.findings.iter().any(|finding| finding.id == "hardware"));
    }

    #[test]
    fn translation_json_does_not_create_a_nextjs_false_positive() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"react":"19"}}"#,
        )
        .unwrap();
        fs::create_dir(dir.path().join("locales")).unwrap();
        fs::write(
            dir.path().join("locales/en.json"),
            r#"{"next":"Next page","electron":"Electron microscope"}"#,
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert!(plan.frameworks.contains(&"react".to_string()));
        assert!(!plan.frameworks.contains(&"nextjs".to_string()));
        assert!(!plan.frameworks.contains(&"electron".to_string()));
    }

    #[test]
    fn ai_tool_settings_are_not_treated_as_application_source() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname='safe'\n").unwrap();
        fs::create_dir(dir.path().join(".claude")).unwrap();
        fs::write(
            dir.path().join(".claude/settings.local.json"),
            r#"{"allow":["fetch(","sqlite","node:child_process"]}"#,
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert!(!plan
            .findings
            .iter()
            .any(|finding| { matches!(finding.id.as_str(), "network" | "database" | "process") }));
    }

    #[test]
    fn source_snapshot_excludes_credentials_and_dependency_caches() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let destination = root.path().join("snapshot");
        fs::create_dir_all(source.join("src")).unwrap();
        fs::create_dir_all(source.join("node_modules/package")).unwrap();
        fs::write(source.join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(source.join(".env"), "SECRET=do-not-copy\n").unwrap();
        fs::write(source.join("signing.pem"), "private key\n").unwrap();
        fs::write(
            source.join("node_modules/package/index.js"),
            "dependency cache\n",
        )
        .unwrap();

        let summary = snapshot(&source, &destination).unwrap();
        assert_eq!(summary.files_copied, 1);
        assert_eq!(summary.sensitive_files_excluded.len(), 2);
        assert!(destination.join("src/main.rs").is_file());
        assert!(!destination.join(".env").exists());
        assert!(!destination.join("signing.pem").exists());
        assert!(!destination.join("node_modules").exists());
        assert!(fs::metadata(destination.join("src/main.rs"))
            .unwrap()
            .permissions()
            .readonly());
    }

    #[test]
    fn python_go_and_dotnet_are_discovered_without_execution() {
        for (marker, language) in [
            ("pyproject.toml", "python"),
            ("go.mod", "go"),
            ("Example.csproj", "dotnet"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join(marker), "name = \"example\"\n").unwrap();
            let plan = analyze(dir.path()).unwrap();
            assert!(
                plan.languages.contains(&language.to_string()),
                "{marker} should detect {language}"
            );
            // None of these languages can be built by the pipeline yet.
            assert_eq!(plan.verdict, Verdict::Unsupported);
        }
    }

    #[test]
    fn a_realistic_app_maps_onto_the_capabilities_that_now_exist() {
        // Settings, a database, a notification, and a link: most of what a
        // small desktop app does beyond drawing a window. Before these
        // capabilities existed every one of these was a dead end.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"tracker\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/main.rs"),
            "use rusqlite::Connection;\n\
             use confy;\n\
             use notify_rust::Notification;\n\
             fn main() { webbrowser::open(\"https://example.com\").unwrap(); }\n",
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        for expected in ["store.kv", "store.sql", "ui.notify", "ui.open-url"] {
            assert!(
                plan.suggested_capabilities.iter().any(|c| c == expected),
                "expected {expected}, got {:?}",
                plan.suggested_capabilities
            );
        }
        // Everything it needs now exists, so nothing here should be a blocker.
        assert!(!plan
            .findings
            .iter()
            .any(|f| matches!(f.severity, crate::Severity::Blocker)));
    }

    #[test]
    fn saved_settings_map_onto_the_key_value_store() {
        // Remembering preferences between launches is the most common reason a
        // small app touches storage at all. Before store.kv existed this could
        // only be answered with a filesystem grant, which both overstated what
        // the app needed and made the permission prompt describe a folder.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"prefs\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/main.rs"),
            "fn main() { let _: String = confy::load(\"prefs\", None).unwrap(); }",
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert!(plan.findings.iter().any(|f| f.id == "settings"));
        assert!(
            plan.suggested_capabilities
                .iter()
                .any(|cap| cap == "store.kv"),
            "expected store.kv, got {:?}",
            plan.suggested_capabilities
        );
    }

    #[test]
    fn binary_only_directory_is_rejected_clearly() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("opaque.exe"), b"MZ\0opaque").unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert_eq!(plan.verdict, Verdict::Unsupported);
        assert!(plan
            .findings
            .iter()
            .any(|finding| finding.id == "binary-only"));
    }

    #[test]
    fn existing_krate_app_ignores_generated_bindings_and_abort() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname='notes'\n").unwrap();
        fs::write(
            dir.path().join("manifest.toml"),
            "[app]\nid='dev.krate.notes'\nname='Notes'\nversion='0.1.0'\nentry='code.wasm'\nworld='krate:app/gui@0.2.0'\n",
        )
        .unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "#[cfg(not(target_arch = \"wasm32\"))] std::process::abort();",
        )
        .unwrap();
        fs::write(
            dir.path().join("src/bindings.rs"),
            "pub mod net { pub fn notification_camera() {} }",
        )
        .unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert_eq!(plan.verdict, Verdict::Ready);
        assert_eq!(plan.profile, "krate-native");
        assert!(plan.findings.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.rs"), "std::process::Command").unwrap();
        symlink(outside.path(), dir.path().join("outside")).unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname='safe'\n").unwrap();

        let plan = analyze(dir.path()).unwrap();
        assert_eq!(plan.scan.symlinks_skipped, 1);
        assert!(!plan.findings.iter().any(|finding| finding.id == "process"));
    }

    /// Denis ran `report` on his PDF tool and got "needs changes, one
    /// finding, map your file paths" -- twenty-three lines, no word about
    /// std, when the true answer was "cannot port: the PDF crate needs std"
    /// (K-079). The std verdict must lead.
    #[test]
    fn a_std_only_dependency_is_reported_as_the_wall_it_is() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"pdftool\"\n[dependencies]\nlopdf = \"0.32\"\nserde = \"1\"\n",
        )
        .unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();

        let plan = analyze(dir.path()).unwrap();
        let wall = plan
            .findings
            .iter()
            .find(|f| f.id == "std-dependency-wall")
            .expect("the std wall must be a finding");
        assert!(matches!(wall.severity, Severity::Blocker));
        assert!(wall.detail.contains("lopdf"), "{}", wall.detail);
        assert!(
            !wall.detail.contains("serde"),
            "serde is known no_std and must not be accused: {}",
            wall.detail
        );
    }

    /// A project whose dependencies are all known no_std gets no std finding
    /// at all -- the check informs, it does not nag.
    #[test]
    fn known_no_std_dependencies_raise_no_wall() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"dice\"\n[dependencies]\nrand = \"0.9\"\nserde = \"1\"\n",
        )
        .unwrap();
        let plan = analyze(dir.path()).unwrap();
        assert!(
            !plan
                .findings
                .iter()
                .any(|f| f.id.starts_with("std-dependency")),
            "all-known-no_std must stay clean"
        );
    }
}
