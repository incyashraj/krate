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
    finish_plan(canonical, analysis)
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

    let frameworks: [(&str, &[&str], bool); 11] = [
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
    ];
    for (framework, patterns, relevant_file) in frameworks {
        if relevant_file && patterns.iter().any(|pattern| lower.contains(pattern)) {
            analysis.frameworks.insert(framework.to_string());
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
    if analysis.frameworks.iter().any(|framework| {
        matches!(
            framework.as_str(),
            "appkit" | "swiftui" | "wpf" | "winui" | "qt" | "gtk"
        )
    }) {
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
    use super::{analyze, snapshot, Verdict};
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
        assert_eq!(plan.verdict, Verdict::NeedsChanges);
        assert_eq!(plan.profile, "desktop-native-source-port");
        assert!(plan.frameworks.contains(&"swiftui".to_string()));
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
        assert_eq!(plan.verdict, Verdict::NeedsChanges);
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
        assert_eq!(plan.verdict, Verdict::NeedsChanges);
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
            assert_eq!(plan.verdict, Verdict::NeedsChanges);
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
}
