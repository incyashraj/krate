//! A WASI C compiler for app builds (IC-941, K-1072).
//!
//! Crates an app leans on compile C: ring (TLS, QUIC), libsqlite3-sys
//! bundled, zstd-sys. Their build scripts (cc-rs) need a C compiler that
//! targets WASI and a WASI libc to compile against. The host's own `cc` has
//! neither, so without this an app using SQLite fails with "'stdio.h' file
//! not found" and an error that sends its author to fix code they never
//! wrote.
//!
//! [`find`] looks, in order, at `WASI_SDK_PATH`, Krate's own toolchain folder
//! (where `krate setup wasi-sdk` puts it), and a wasi-sdk `clang` on PATH.
//! [`apply`] gives a cargo build that compiler for its wasm target. When none
//! is found and a crate needed one, [`missing_compiler`] turns cargo's output
//! into the one step that fixes it.
//!
//! [`install`] is that step: the pinned wasi-sdk release for this machine,
//! checked against its SHA-256 before anything is unpacked, into Krate's
//! folder.

use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// The wasi-sdk release Krate builds with.
pub const VERSION: &str = "34.0";
/// Its tag on github.com/WebAssembly/wasi-sdk.
const TAG: &str = "wasi-sdk-34";

/// SHA-256 of each release archive Krate may fetch, by asset name.
///
/// An archive with no digest here is never downloaded: a toolchain that
/// compiles every app's C is not something to take on trust. Printed by
/// scripts/wasi-sdk-checksums.py on 2026-10-11, and each one matched the
/// digest GitHub publishes for the release asset.
const PINNED_SHA256: &[(&str, Option<&str>)] = &[
    (
        "wasi-sdk-34.0-arm64-macos.tar.gz",
        Some("9c59398106b417f8f14913380fdf0097a8cc0ff4af9eb3ce0065a859e88d49e9"),
    ),
    (
        "wasi-sdk-34.0-x86_64-macos.tar.gz",
        Some("87d27fa8adc68dee59bfbf2e22a6d34ef717c34d6bf1d8af2a56fc929d9ce0eb"),
    ),
    (
        "wasi-sdk-34.0-arm64-linux.tar.gz",
        Some("f7e243dff54d60bcc576e94d6166b69f410f2500ae4a9ceef34315be10e77971"),
    ),
    (
        "wasi-sdk-34.0-x86_64-linux.tar.gz",
        Some("b761e3a0721dbae9c09a0059e5fdb2bf917d1b4a8a7b430fb3b5aafb0984b2c4"),
    ),
    (
        "wasi-sdk-34.0-x86_64-windows.tar.gz",
        Some("cccb5c323a9b34f0349a9b09e8804a0a7632c68c3310f4b5f437ed57d7e71d8f"),
    ),
];

/// The largest archive accepted from the network (the real ones are about
/// 110-150 MB).
const MAX_DOWNLOAD_BYTES: u64 = 512 * 1024 * 1024;

/// A wasi-sdk on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WasiSdk {
    pub root: PathBuf,
}

impl WasiSdk {
    fn tool(&self, name: &str) -> PathBuf {
        let file = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        };
        self.root.join("bin").join(file)
    }

    pub fn clang(&self) -> PathBuf {
        self.tool("clang")
    }

    pub fn sysroot(&self) -> PathBuf {
        self.root.join("share").join("wasi-sysroot")
    }

    /// A folder is a wasi-sdk when it has the compiler and the WASI libc.
    fn at(root: PathBuf) -> Option<WasiSdk> {
        let sdk = WasiSdk { root };
        (sdk.clang().is_file() && sdk.sysroot().join("include").is_dir()).then_some(sdk)
    }
}

/// The folder for this machine's archive, as the release names it.
pub fn asset_stem() -> Option<String> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => return None,
    };
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" if arch == "x86_64" => "windows",
        _ => return None,
    };
    Some(format!("wasi-sdk-{VERSION}-{arch}-{os}"))
}

/// Krate's own toolchain folder: `~/.krate/toolchains`, or
/// KRATE_TOOLCHAINS_DIR.
pub fn toolchains_dir() -> PathBuf {
    std::env::var_os("KRATE_TOOLCHAINS_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::krate_home().join("toolchains"))
}

/// Where to look, in order (see the module docs).
fn candidates() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for var in ["WASI_SDK_PATH", "WASI_SDK"] {
        if let Some(dir) = std::env::var_os(var).filter(|d| !d.is_empty()) {
            found.push(PathBuf::from(dir));
        }
    }
    let mut own = vec![toolchains_dir()];
    // Where Krate's own build scripts kept one before setup existed.
    if std::env::var_os("KRATE_TOOLCHAINS_DIR").is_none() {
        if let Some(home) = crate::home_dir() {
            own.push(home.join(".krate-toolchains"));
        }
    }
    for dir in own {
        if let Some(stem) = asset_stem() {
            found.push(dir.join(&stem));
        }
        // Any other release kept there, newest name last so it is tried last.
        if let Ok(entries) = fs::read_dir(&dir) {
            let mut others: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("wasi-sdk-"))
                })
                .collect();
            others.sort();
            others.reverse();
            found.extend(others);
        }
    }
    // A wasi-sdk on PATH: its bin/ holds clang, and share/wasi-sysroot is
    // beside it. A system clang has no WASI libc there and is passed over.
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        if let Some(root) = dir.parent() {
            found.push(root.to_path_buf());
        }
    }
    found
}

/// The wasi-sdk an app build uses, if this machine has one.
pub fn find() -> Option<WasiSdk> {
    candidates().into_iter().find_map(WasiSdk::at)
}

/// Give a cargo build for `target` (`wasm32-wasip1`, `wasm32-wasip1-threads`)
/// this compiler, its archiver and the WASI libc. A variable the person set
/// themselves is left as it is.
pub fn apply(command: &mut Command, sdk: &WasiSdk, target: &str) {
    for (name, value) in variables(sdk, target) {
        if std::env::var_os(&name).is_none() {
            command.env(name, value);
        }
    }
}

/// The variables [`apply`] sets, for cc-rs and for build scripts that read
/// them directly.
pub fn variables(sdk: &WasiSdk, target: &str) -> Vec<(String, String)> {
    let key = target.replace('-', "_");
    let threads = target.ends_with("-threads");
    let flags = format!(
        "--target={target} --sysroot={}{}",
        sdk.sysroot().display(),
        if threads { " -pthread" } else { "" }
    );
    vec![
        (format!("CC_{key}"), sdk.clang().display().to_string()),
        (
            format!("CXX_{key}"),
            sdk.tool("clang++").display().to_string(),
        ),
        (
            format!("AR_{key}"),
            sdk.tool("llvm-ar").display().to_string(),
        ),
        (format!("CFLAGS_{key}"), flags.clone()),
        (format!("CXXFLAGS_{key}"), flags),
    ]
}

/// When a build failed because a crate's C found no WASI compiler, the
/// sentence that says so and the one step that fixes it.
pub fn missing_compiler(output: &str) -> Option<String> {
    let c_failed = output.contains("error occurred in cc-rs")
        || output.contains("failed to find tool")
        || (output.contains("file not found") && output.contains("--target=wasm32"));
    if !c_failed {
        return None;
    }
    // "failed to run custom build command for `ring v0.17.14`", once per crate.
    let mut crates: Vec<&str> = Vec::new();
    for rest in output
        .split("failed to run custom build command for `")
        .skip(1)
    {
        if let Some(name) = rest.split('`').next() {
            if !crates.contains(&name) {
                crates.push(name);
            }
        }
    }
    let (named, verb) = match crates.as_slice() {
        [] => ("A crate this app uses".to_string(), "compiles"),
        [one] => (one.to_string(), "compiles"),
        many => (
            format!(
                "{} and {}",
                many[..many.len() - 1].join(", "),
                many[many.len() - 1]
            ),
            "compile",
        ),
    };
    Some(format!(
        "{named} {verb} C, and this machine has no C compiler for WebAssembly \
         (wasi-sdk). Run `krate setup wasi-sdk` to fetch it into Krate's folder, \
         or set WASI_SDK_PATH to a wasi-sdk you already have, then build again."
    ))
}

/// [`missing_compiler`], when this machine really has no wasi-sdk: with one
/// found, a failing C build is the C's own error and is left to speak.
pub fn explain_c_failure(output: &str) -> Option<String> {
    if find().is_some() {
        return None;
    }
    missing_compiler(output)
}

/// One line for `krate doctor`.
pub fn doctor_line() -> String {
    match find() {
        Some(sdk) => format!("wasi-sdk        found ({})", sdk.root.display()),
        None => "wasi-sdk        not found -- an app whose crates compile C (SQLite, ring, \
                 zstd) needs it: `krate setup wasi-sdk`"
            .to_string(),
    }
}

/// `krate setup wasi-sdk`: the pinned release for this machine, verified,
/// into Krate's folder. `from` installs a local copy of the archive instead
/// of fetching it, checked against `sha256` when given and the pinned digest
/// otherwise.
pub fn install(from: Option<&Path>, sha256: Option<&str>) -> Result<WasiSdk> {
    let stem = asset_stem().context(
        "wasi-sdk publishes no build for this machine; set WASI_SDK_PATH to one you built",
    )?;
    let asset = format!("{stem}.tar.gz");
    let expected = match sha256 {
        Some(digest) => digest.to_ascii_lowercase(),
        None => PINNED_SHA256
            .iter()
            .find(|(name, _)| *name == asset)
            .and_then(|(_, digest)| *digest)
            .map(str::to_string)
            .with_context(|| {
                format!(
                    "Krate has no verified checksum for {asset} yet, so it will not fetch it. \
                     Download wasi-sdk {VERSION} from \
                     https://github.com/WebAssembly/wasi-sdk/releases/tag/{TAG}, unpack it, and \
                     set WASI_SDK_PATH to its folder."
                )
            })?,
    };
    let dir = toolchains_dir();
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let archive = dir.join(format!("{asset}.download"));
    match from {
        Some(local) => {
            fs::copy(local, &archive)
                .with_context(|| format!("copy {} to {}", local.display(), archive.display()))?;
        }
        None => {
            let url =
                format!("https://github.com/WebAssembly/wasi-sdk/releases/download/{TAG}/{asset}");
            fetch(&url, &archive)?;
        }
    }
    let digest = sha256_of(&archive)?;
    if digest != expected {
        let _ = fs::remove_file(&archive);
        anyhow::bail!(
            "{asset} failed its checksum: expected SHA-256 {expected}, got {digest}; \
             nothing was installed"
        );
    }
    // Unpack beside the final folder, then move it in, so a half-unpacked
    // toolchain is never found by a build.
    let staging = dir.join(format!("{stem}.unpacking"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let unpacked = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&staging)
        .status()
        .context("run tar to unpack wasi-sdk")?;
    let _ = fs::remove_file(&archive);
    if !unpacked.success() {
        let _ = fs::remove_dir_all(&staging);
        anyhow::bail!("tar could not unpack {asset}");
    }
    // The archive holds one folder, named for the release.
    let inner = fs::read_dir(&staging)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .context("the wasi-sdk archive held no folder")?;
    let target = dir.join(&stem);
    let _ = fs::remove_dir_all(&target);
    fs::rename(&inner, &target)
        .with_context(|| format!("move wasi-sdk into {}", target.display()))?;
    let _ = fs::remove_dir_all(&staging);
    WasiSdk::at(target.clone()).with_context(|| {
        format!(
            "{} has no bin/clang and share/wasi-sysroot: not a wasi-sdk",
            target.display()
        )
    })
}

fn fetch(url: &str, destination: &Path) -> Result<()> {
    let response = krate_http::get(url)
        .call()
        .with_context(|| format!("download {url}"))?;
    let mut reader = response.into_reader();
    let mut output =
        File::create(destination).with_context(|| format!("create {}", destination.display()))?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut written = 0_u64;
    loop {
        let count = reader.read(&mut buffer).context("read the download")?;
        if count == 0 {
            break;
        }
        written += count as u64;
        if written > MAX_DOWNLOAD_BYTES {
            let _ = fs::remove_file(destination);
            anyhow::bail!("the download from {url} was larger than any wasi-sdk release");
        }
        output.write_all(&buffer[..count])?;
    }
    output.sync_all()?;
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every platform setup offers has its digest pinned, in the form a
    /// SHA-256 takes, and the Dockerfile's default is the Linux one.
    #[test]
    fn every_release_archive_setup_may_fetch_has_a_pinned_digest() {
        for (asset, digest) in PINNED_SHA256 {
            let digest = digest.unwrap_or_else(|| panic!("{asset} has no digest"));
            assert_eq!(digest.len(), 64, "{asset}");
            assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()), "{asset}");
        }
        let dockerfile = include_str!("../../../cloud/builder/Dockerfile");
        let linux = PINNED_SHA256
            .iter()
            .find(|(a, _)| *a == "wasi-sdk-34.0-x86_64-linux.tar.gz")
            .and_then(|(_, d)| *d)
            .unwrap();
        assert!(
            dockerfile.contains(&format!("ARG WASI_SDK_SHA256={linux}")),
            "the builder image pins the same digest"
        );
    }

    #[test]
    fn a_build_for_each_wasm_target_gets_its_own_compiler_and_libc() {
        let sdk = WasiSdk {
            root: PathBuf::from("/opt/wasi-sdk"),
        };
        let plain = variables(&sdk, "wasm32-wasip1");
        let get = |vars: &[(String, String)], name: &str| {
            vars.iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        // The tools carry .exe on Windows and the sysroot is joined with the
        // system's separator, so both are asked for the way this system
        // writes them.
        let exe = std::env::consts::EXE_SUFFIX;
        assert!(get(&plain, "CC_wasm32_wasip1").ends_with(&format!("clang{exe}")));
        assert!(get(&plain, "AR_wasm32_wasip1").ends_with(&format!("llvm-ar{exe}")));
        let flags = get(&plain, "CFLAGS_wasm32_wasip1");
        assert!(flags.contains("--target=wasm32-wasip1"), "{flags}");
        assert!(
            flags.contains(&format!("--sysroot={}", sdk.sysroot().display())),
            "{flags}"
        );
        assert!(!flags.contains("-pthread"), "{flags}");
        let threads = variables(&sdk, "wasm32-wasip1-threads");
        assert!(
            get(&threads, "CFLAGS_wasm32_wasip1_threads").ends_with("-pthread"),
            "a threaded app's C is built for threads"
        );
    }

    #[test]
    fn a_c_build_without_a_compiler_names_the_crate_and_the_step() {
        let cargo = "error: failed to run custom build command for `libsqlite3-sys v0.35.0`\n\
                     ...\n  cargo:warning=sqlite3/sqlite3.c:15049:10: fatal error: 'stdio.h' \
                     file not found\n  error occurred in cc-rs: command did not execute \
                     successfully: \"clang\" \"--target=wasm32-wasip1\"";
        let said = missing_compiler(cargo).expect("recognised");
        assert!(
            said.starts_with("libsqlite3-sys v0.35.0 compiles C"),
            "{said}"
        );
        assert!(said.contains("krate setup wasi-sdk"), "{said}");
        assert!(said.contains("WASI_SDK_PATH"), "{said}");
        assert_eq!(
            missing_compiler("error[E0425]: cannot find value `x`"),
            None
        );
        let two = format!(
            "{cargo}\nerror: failed to run custom build command for `ring v0.17.14`\n\
             error: failed to run custom build command for `ring v0.17.14`"
        );
        let said = missing_compiler(&two).expect("recognised");
        assert!(
            said.starts_with("libsqlite3-sys v0.35.0 and ring v0.17.14 compile C"),
            "{said}"
        );
    }

    #[test]
    fn a_folder_is_a_wasi_sdk_only_with_its_compiler_and_libc() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("wasi-sdk-34.0-arm64-macos");
        assert_eq!(WasiSdk::at(root.clone()), None);
        fs::create_dir_all(root.join("bin")).unwrap();
        let clang = if cfg!(windows) { "clang.exe" } else { "clang" };
        fs::write(root.join("bin").join(clang), "").unwrap();
        assert_eq!(WasiSdk::at(root.clone()), None, "no WASI libc yet");
        fs::create_dir_all(root.join("share/wasi-sysroot/include")).unwrap();
        assert_eq!(WasiSdk::at(root.clone()), Some(WasiSdk { root }));
    }
}
