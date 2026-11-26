//! Krate's own Rust, for building apps on a machine that has none (IC-1005).
//!
//! An app is compiled Rust, so building one needs `rustc`, `cargo`, the
//! standard library for `wasm32-wasip1`, and `cargo-component`. A person who
//! installed Krate from Claude Desktop has none of these, and asking them to
//! install rustup, pick a toolchain and add a target is the developer-shaped
//! step the extension exists to remove.
//!
//! [`install`] fetches the four official Rust release archives this Krate
//! builds with -- `rustc`, `cargo`, the host's `rust-std` (build scripts are
//! host programs) and `rust-std` for `wasm32-wasip1` -- checks each against a
//! SHA-256 pinned below before anything is unpacked, and lays them out as one
//! toolchain in Krate's own folder (`~/.krate/toolchains/rust-<ver>-<host>`).
//! It never runs rustup and never touches `~/.rustup` or `~/.cargo`: its
//! crate downloads go to its own `cargo-home` beside it.
//!
//! The host linker is the part a fresh machine lacks even with Rust in hand.
//! Build scripts and proc macros are host programs and must be linked:
//!
//! - **macOS** links with `cc`, which is Xcode's, and a Mac without the
//!   Command Line Tools answers with a 1.5 GB install dialog. Krate links
//!   them with the `rust-lld` that ships inside `rustc`, against a small
//!   `libSystem` stub Krate writes itself (symbols are found at run time from
//!   the real `/usr/lib/libSystem.B.dylib`, which every Mac has). No Xcode,
//!   no SDK, no dialog.
//! - **Windows** uses the `gnullvm` host toolchain, whose own `rust-lld`
//!   links without Visual Studio Build Tools (the road K-130 proved).
//! - **Linux** keeps the system `cc`; a desktop without one is told so.
//!
//! [`apply`] gives a build command this toolchain when the person has no
//! working Rust of their own. A person who already has rustup with the wasm
//! target keeps using theirs exactly as before.

use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};

/// The Rust release apps build with. Must equal `rust-toolchain.toml`.
pub const VERSION: &str = "1.95.0";
/// That release's folder on static.rust-lang.org.
const DIST_DATE: &str = "2026-04-16";
/// The target every app compiles to.
pub const WASM_TARGET: &str = "wasm32-wasip1";

/// Each archive Krate may fetch: name, SHA-256, size in bytes.
///
/// Read from https://static.rust-lang.org/dist/channel-rust-1.95.0.toml
/// (the release's own manifest) on 2026-11-26. An archive missing here is
/// never downloaded. macOS takes the `.tar.xz` (80 MB smaller, and the
/// system `tar` reads it); Windows and Linux take `.tar.gz`, which every
/// `tar` there reads without an `xz` program.
const PINNED: &[(&str, &str, u64)] = &[
    (
        "rustc-1.95.0-aarch64-apple-darwin.tar.xz",
        "149e85a285b6eba58eb6c8bdf7deb1b93763890598e62cb635a712e3a8454f04",
        67_652_240,
    ),
    (
        "cargo-1.95.0-aarch64-apple-darwin.tar.xz",
        "6c2ffed8e1ac9cf4dc9e80f282a869a6b237a153e7c55cca039d33de29d80aaf",
        8_731_256,
    ),
    (
        "rust-std-1.95.0-aarch64-apple-darwin.tar.xz",
        "9b30089b0f767cb91b2190ffec55a9beeb2a21a1405d8da0f664d7e09d08e6d8",
        27_317_176,
    ),
    (
        "rust-std-1.95.0-wasm32-wasip1.tar.xz",
        "86e5b6d98c7520bb9c3ad4f8cbbbf14beaf230b0f06b437db398e3c4f7dae43e",
        22_750_712,
    ),
    (
        "rustc-1.95.0-x86_64-apple-darwin.tar.xz",
        "33db457715446a69ed6f69f78f5fbb9ca8e17a16585d1d7a0060479bfe4c7afc",
        79_765_616,
    ),
    (
        "cargo-1.95.0-x86_64-apple-darwin.tar.xz",
        "e2e1131ade2dddc0d779e0ab3a6a990085c7a654951235742823c3a1ce0f190f",
        9_397_524,
    ),
    (
        "rust-std-1.95.0-x86_64-apple-darwin.tar.xz",
        "2be13c14122b8d4d09b7f7c434fca9ae7215ec72049944189c88c4d9128ce504",
        28_745_188,
    ),
    (
        "rustc-1.95.0-x86_64-pc-windows-gnullvm.tar.gz",
        "f36c14ab7bdd62a5e4b4aaed0c557ffff143d1b35c0ac9ac1fe55a113221563e",
        135_751_882,
    ),
    (
        "cargo-1.95.0-x86_64-pc-windows-gnullvm.tar.gz",
        "858d9612146f97d60eab9a3861a231027aa04fdecb5ff14d4c83761aa88ed5b9",
        13_791_343,
    ),
    (
        "rust-std-1.95.0-x86_64-pc-windows-gnullvm.tar.gz",
        "98e54d3c36a945e4eb507cf1420b4a74cdd7e2be07c00b5755040e704d1c186a",
        42_029_002,
    ),
    (
        "rust-std-1.95.0-wasm32-wasip1.tar.gz",
        "ddee2aad61c73cededcaef2a8063dd8727074e0f4980d6f3a01e8cdba1bb5d3b",
        34_041_142,
    ),
    (
        "rustc-1.95.0-aarch64-pc-windows-gnullvm.tar.gz",
        "bbe34868ce3b85564fefaf867d692d85370bb941a5f83afc8e25ff43c3bff014",
        128_479_575,
    ),
    (
        "cargo-1.95.0-aarch64-pc-windows-gnullvm.tar.gz",
        "a66d016444cfe246fe6e82dd769458f9268dbca6f8058b01a74f7e5855fe3092",
        13_076_516,
    ),
    (
        "rust-std-1.95.0-aarch64-pc-windows-gnullvm.tar.gz",
        "48a758c153f2a4b64ba629d87e8038045663f7c0a137e1299002f8d1466d1428",
        38_933_980,
    ),
    (
        "rustc-1.95.0-x86_64-unknown-linux-gnu.tar.gz",
        "fef749c4abb4b4bde5ebf773bec550003ce5b4410579cecd69a365e5c0c5106a",
        135_179_262,
    ),
    (
        "cargo-1.95.0-x86_64-unknown-linux-gnu.tar.gz",
        "47ebc468721a6ff3fb27dff33e632a4cb6246d0ea061814bcd4fe601d18c69a8",
        15_405_121,
    ),
    (
        "rust-std-1.95.0-x86_64-unknown-linux-gnu.tar.gz",
        "edbd20f8fc0a617f85ffb79fa6c22aa6def0e570de3f94be1a6e5ab1f77f763c",
        49_435_219,
    ),
    (
        "rustc-1.95.0-aarch64-unknown-linux-gnu.tar.gz",
        "f488f7eced24d2fed2e4745f3dbccf1ffe24bfe02720e5415406195ceaf45f7a",
        102_041_932,
    ),
    (
        "cargo-1.95.0-aarch64-unknown-linux-gnu.tar.gz",
        "f37440984c4c06d38487138adae2c5c80ec4ddfbdfe57ae387357995bfa502e9",
        15_348_987,
    ),
    (
        "rust-std-1.95.0-aarch64-unknown-linux-gnu.tar.gz",
        "a68e71b09f4e09f7071c650ec38ee1845ddfe66c46917c0ccf0b1636994b01c6",
        46_976_152,
    ),
];

/// The largest single archive accepted (the real ones are 9-136 MB).
const MAX_DOWNLOAD_BYTES: u64 = 400 * 1024 * 1024;

/// The file a finished install writes last. A folder without it is a
/// half-finished install and is never used.
const COMPLETE_MARKER: &str = "krate-toolchain.json";

/// The Rust host this machine's toolchain is for, if Krate offers one.
pub fn host() -> Option<&'static str> {
    host_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn host_for(os: &str, arch: &str) -> Option<&'static str> {
    Some(match (os, arch) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        // gnullvm, not msvc: it links with its own rust-lld, so a build
        // needs no Visual Studio Build Tools (K-130).
        ("windows", "x86_64") => "x86_64-pc-windows-gnullvm",
        ("windows", "aarch64") => "aarch64-pc-windows-gnullvm",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        _ => return None,
    })
}

fn extension_for(host: &str) -> &'static str {
    if host.ends_with("-apple-darwin") {
        "tar.xz"
    } else {
        "tar.gz"
    }
}

/// The four archives a host's toolchain is made of, by file name.
fn assets_for(host: &str) -> Vec<String> {
    let ext = extension_for(host);
    vec![
        format!("rustc-{VERSION}-{host}.{ext}"),
        format!("cargo-{VERSION}-{host}.{ext}"),
        format!("rust-std-{VERSION}-{host}.{ext}"),
        format!("rust-std-{VERSION}-{WASM_TARGET}.{ext}"),
    ]
}

fn pinned(asset: &str) -> Option<(&'static str, u64)> {
    PINNED
        .iter()
        .find(|(name, _, _)| *name == asset)
        .map(|(_, digest, size)| (*digest, *size))
}

/// What setting up downloads on this machine, in bytes.
pub fn download_bytes() -> Option<u64> {
    let host = host()?;
    assets_for(host)
        .iter()
        .map(|asset| pinned(asset).map(|(_, size)| size))
        .sum()
}

/// Where this machine's toolchain lives.
pub fn root() -> Option<PathBuf> {
    Some(crate::wasi_sdk::toolchains_dir().join(format!("rust-{VERSION}-{}", host()?)))
}

/// Krate's own registry and crate cache, so a build never writes into the
/// person's `~/.cargo`.
fn cargo_home() -> PathBuf {
    crate::wasi_sdk::toolchains_dir().join("cargo-home")
}

/// Krate's own Rust on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    pub root: PathBuf,
    host: String,
}

impl Toolchain {
    fn tool(&self, name: &str) -> PathBuf {
        self.root
            .join("bin")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    }

    pub fn rustc(&self) -> PathBuf {
        self.tool("rustc")
    }

    pub fn cargo(&self) -> PathBuf {
        self.tool("cargo")
    }

    fn host_bin(&self) -> PathBuf {
        self.root
            .join("lib")
            .join("rustlib")
            .join(&self.host)
            .join("bin")
    }

    /// A folder is a finished toolchain when it has both tools, the wasm
    /// standard library and the marker an install writes last.
    fn at(root: PathBuf, host: &str) -> Option<Toolchain> {
        let toolchain = Toolchain {
            root,
            host: host.to_string(),
        };
        let complete = toolchain.rustc().is_file()
            && toolchain.cargo().is_file()
            && toolchain
                .root
                .join("lib/rustlib")
                .join(WASM_TARGET)
                .join("lib")
                .is_dir()
            && toolchain.root.join(COMPLETE_MARKER).is_file();
        complete.then_some(toolchain)
    }
}

/// Krate's own toolchain, if setup has finished on this machine.
pub fn find() -> Option<Toolchain> {
    Toolchain::at(root()?, host()?)
}

/// Whether the person's own Rust can build an app: a rustc reachable the way
/// app builds reach it (rustup's shims, else rustup's active toolchain), with
/// the wasm target installed.
pub fn person_rust_ready() -> bool {
    let rustc_name = format!("rustc{}", std::env::consts::EXE_SUFFIX);
    let shim = crate::home_dir()
        .map(|home| home.join(".cargo").join("bin").join(&rustc_name))
        .filter(|path| path.is_file());
    let rustc = shim.or_else(|| {
        crate::rustup_toolchain_bin()
            .map(|bin| bin.join(&rustc_name))
            .filter(|path| path.is_file())
    });
    let Some(rustc) = rustc else {
        return false;
    };
    let Ok(output) = Command::new(&rustc)
        .args(["--print", "sysroot"])
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let sysroot = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    sysroot.join("lib/rustlib").join(WASM_TARGET).is_dir()
}

/// The toolchain an app build should use instead of the person's own, if
/// any: Krate's, when it is set up and theirs cannot build an app.
pub fn for_builds() -> Option<Toolchain> {
    let own = find()?;
    (!person_rust_ready()).then_some(own)
}

/// Give a cargo / cargo-component build this toolchain: its tools first on
/// PATH, its own crate cache, and on a Mac its own host linker.
pub fn apply(command: &mut Command, toolchain: &Toolchain) {
    for (name, value) in variables(toolchain) {
        command.env(name, value);
    }
    // A rustup override from the person's shell names a toolchain this one
    // is not; cargo-component would otherwise go looking for rustup.
    command.env_remove("RUSTUP_TOOLCHAIN");
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let joined = std::env::join_paths(
        std::iter::once(toolchain.root.join("bin")).chain(std::env::split_paths(&existing)),
    );
    if let Ok(path) = joined {
        command.env("PATH", path);
    }
}

/// The variables [`apply`] sets, besides PATH.
pub fn variables(toolchain: &Toolchain) -> Vec<(String, String)> {
    let mut vars = vec![
        ("RUSTC".to_string(), toolchain.rustc().display().to_string()),
        ("CARGO".to_string(), toolchain.cargo().display().to_string()),
        ("CARGO_HOME".to_string(), cargo_home().display().to_string()),
    ];
    let key = toolchain.host.to_uppercase().replace('-', "_");
    if toolchain.host.ends_with("-apple-darwin") {
        let link = toolchain.root.join(HOST_LINK_DIR);
        vars.push((
            format!("CARGO_TARGET_{key}_LINKER"),
            link.join("rust-lld").display().to_string(),
        ));
        // rustc asks `xcrun` for the SDK when this is unset, and on a Mac
        // without the Command Line Tools that call is what raises the
        // install dialog.
        vars.push((
            "SDKROOT".to_string(),
            link.join("sdk").display().to_string(),
        ));
    } else if toolchain.host.ends_with("-windows-gnullvm") {
        vars.push((
            format!("CARGO_TARGET_{key}_LINKER"),
            toolchain
                .host_bin()
                .join("rust-lld.exe")
                .display()
                .to_string(),
        ));
    }
    vars
}

/// One line for `krate doctor`.
pub fn doctor_line() -> String {
    match find() {
        Some(toolchain) => format!(
            "Krate's Rust   {VERSION} ({}), used when yours cannot build an app",
            toolchain.root.display()
        ),
        None => "Krate's Rust   not set up -- `krate setup rust` gives this machine an app \
                 builder of its own (only needed without rustup)"
            .to_string(),
    }
}

/// A step of [`install`], for a progress line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Download { part: String, done: u64, total: u64 },
    Unpack { part: String },
    Done,
}

/// Where the host-link pieces go inside the toolchain.
const HOST_LINK_DIR: &str = "krate-host-link";

/// `krate setup rust`: the pinned archives for this machine, verified, into
/// Krate's folder. `from` is a folder holding the archives already (by their
/// release names), used instead of downloading.
pub fn install(from: Option<&Path>, progress: &mut dyn FnMut(Step)) -> Result<Toolchain> {
    let host = host().context(
        "Krate has no Rust toolchain for this kind of computer; install Rust with rustup \
         (https://rustup.rs) and add the wasm32-wasip1 target",
    )?;
    let final_root = root().context("no toolchain folder")?;
    let dir = crate::wasi_sdk::toolchains_dir();
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let staging = dir.join(format!("rust-{VERSION}-{host}.unpacking"));
    let _ = fs::remove_dir_all(&staging);
    let prefix = staging.join("toolchain");
    fs::create_dir_all(&prefix)?;

    for asset in assets_for(host) {
        let (digest, size) = pinned(&asset)
            .with_context(|| format!("Krate has no verified checksum for {asset}"))?;
        let part = asset
            .split(&format!("-{VERSION}-"))
            .next()
            .unwrap_or(&asset)
            .to_string();
        let part = if asset.contains(WASM_TARGET) {
            format!("{part} ({WASM_TARGET})")
        } else {
            part
        };
        let archive = staging.join(&asset);
        match from {
            Some(local) => {
                let source = local.join(&asset);
                fs::copy(&source, &archive).with_context(|| {
                    format!("copy {} (expected it in --from)", source.display())
                })?;
            }
            None => {
                let url = format!("https://static.rust-lang.org/dist/{DIST_DATE}/{asset}");
                fetch(&url, &archive, size, &part, progress)?;
            }
        }
        let got = crate::wasi_sdk::sha256_of(&archive)?;
        if got != digest {
            let _ = fs::remove_dir_all(&staging);
            anyhow::bail!(
                "{asset} failed its checksum: expected SHA-256 {digest}, got {got}; nothing was \
                 installed"
            );
        }
        progress(Step::Unpack { part: part.clone() });
        let unpacked = staging.join("unpacked");
        let _ = fs::remove_dir_all(&unpacked);
        fs::create_dir_all(&unpacked)?;
        let flag = if asset.ends_with(".tar.xz") {
            "-xJf"
        } else {
            "-xzf"
        };
        let status = Command::new("tar")
            .arg(flag)
            .arg(&archive)
            .arg("-C")
            .arg(&unpacked)
            .stdin(std::process::Stdio::null())
            .status()
            .context("run tar to unpack the Rust toolchain")?;
        let _ = fs::remove_file(&archive);
        if !status.success() {
            let _ = fs::remove_dir_all(&staging);
            anyhow::bail!("tar could not unpack {asset}");
        }
        merge_components(&unpacked, &prefix)
            .with_context(|| format!("lay out {asset} as a toolchain"))?;
        let _ = fs::remove_dir_all(&unpacked);
    }

    if host.ends_with("-apple-darwin") {
        write_host_link(&prefix, host)?;
    }
    fs::write(
        prefix.join(COMPLETE_MARKER),
        serde_json::to_string_pretty(&serde_json::json!({
            "rust": VERSION,
            "host": host,
            "target": WASM_TARGET,
            "archives": assets_for(host),
            "made_by": "krate setup rust",
        }))?,
    )?;

    let _ = fs::remove_dir_all(&final_root);
    fs::rename(&prefix, &final_root)
        .with_context(|| format!("move the toolchain into {}", final_root.display()))?;
    let _ = fs::remove_dir_all(&staging);
    progress(Step::Done);
    Toolchain::at(final_root.clone(), host)
        .with_context(|| format!("{} is not a complete toolchain", final_root.display()))
}

/// A release archive holds `<name>/components` and one folder per component
/// listed there; each component's files go into the prefix as they sit in
/// its folder (what rustup's own installer does, without its shell script).
fn merge_components(unpacked: &Path, prefix: &Path) -> Result<()> {
    let top = fs::read_dir(unpacked)?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_dir())
        .context("the archive held no folder")?;
    let listed = fs::read_to_string(top.join("components"))
        .with_context(|| format!("read {}/components", top.display()))?;
    for component in listed.lines().map(str::trim).filter(|c| !c.is_empty()) {
        if component.contains(['/', '\\']) || component.starts_with('.') {
            anyhow::bail!("the archive names a component `{component}` outside itself");
        }
        move_tree(&top.join(component), prefix)?;
    }
    Ok(())
}

fn move_tree(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if name == "manifest.in" {
            continue;
        }
        let source = entry.path();
        let dest = to.join(&name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir_all(&dest)?;
            move_tree(&source, &dest)?;
        } else {
            if dest.exists() {
                fs::remove_file(&dest)?;
            }
            fs::rename(&source, &dest)
                .with_context(|| format!("move {} to {}", source.display(), dest.display()))?;
        }
    }
    Ok(())
}

/// The macOS host linker: `rust-lld` in its ld64 mode, linking against a
/// `libSystem` stub with symbols resolved when the program runs.
///
/// `-undefined dynamic_lookup` is what lets the stub list no symbols: the
/// program still names `/usr/lib/libSystem.B.dylib` as its library, every
/// Mac has it (in the shared cache), and the loader finds each symbol there
/// at start. Measured 2026-11-26 on macOS 27: a std program with threads,
/// files and env links with PATH empty and no SDK, and runs.
fn write_host_link(prefix: &Path, host: &str) -> Result<()> {
    let link = prefix.join(HOST_LINK_DIR);
    let lib = link.join("sdk").join("usr").join("lib");
    fs::create_dir_all(&lib)?;
    for (name, install_name) in STUB_LIBRARIES {
        fs::write(lib.join(format!("{name}.tbd")), stub_tbd(install_name))?;
    }
    // Named `rust-lld` because rustc picks the argument style from the
    // linker's name: `rust-lld` gets ld64's own flags (`-arch`,
    // `-platform_version`, `-syslibroot`), while `ld64.lld` -- whose file
    // stem is `ld64` -- was taken for a C compiler driver and handed
    // `-Wl,-dead_strip`, which lld refuses (measured 2026-11-26).
    let script = link.join("rust-lld");
    fs::write(&script, linker_script(host))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// Every library a host build script or proc macro is linked against on a
/// Mac, and the real library each one is.
const STUB_LIBRARIES: &[(&str, &str)] = &[
    ("libSystem", "/usr/lib/libSystem.B.dylib"),
    ("libc", "/usr/lib/libSystem.B.dylib"),
    ("libm", "/usr/lib/libSystem.B.dylib"),
    ("libpthread", "/usr/lib/libSystem.B.dylib"),
    ("libdl", "/usr/lib/libSystem.B.dylib"),
    ("libiconv", "/usr/lib/libiconv.2.dylib"),
    ("libresolv", "/usr/lib/libresolv.9.dylib"),
];

fn stub_tbd(install_name: &str) -> String {
    format!(
        "--- !tapi-tbd\ntbd-version:     4\ntargets:         [ arm64-macos, x86_64-macos ]\n\
         install-name:    '{install_name}'\ncurrent-version: 1\n...\n"
    )
}

fn linker_script(host: &str) -> String {
    format!(
        "#!/bin/sh\n\
         # Krate's host linker for a Mac with no Xcode (IC-1005): rust-lld from\n\
         # this toolchain, symbols resolved from the system's libraries at run time.\n\
         here=\"$(cd \"$(dirname \"$0\")\" && pwd)\"\n\
         lld=\"$here/../lib/rustlib/{host}/bin/rust-lld\"\n\
         if [ \"$1\" = -flavor ]; then exec \"$lld\" \"$@\" -undefined dynamic_lookup; fi\n\
         exec \"$lld\" -flavor darwin \"$@\" -undefined dynamic_lookup\n"
    )
}

fn fetch(
    url: &str,
    destination: &Path,
    expected: u64,
    part: &str,
    progress: &mut dyn FnMut(Step),
) -> Result<()> {
    let response = krate_http::get(url)
        .call()
        .with_context(|| format!("download {url}"))?;
    let mut reader = response.into_reader();
    let mut output =
        File::create(destination).with_context(|| format!("create {}", destination.display()))?;
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut written = 0_u64;
    let mut last_report = 0_u64;
    progress(Step::Download {
        part: part.to_string(),
        done: 0,
        total: expected,
    });
    loop {
        let count = reader.read(&mut buffer).context("read the download")?;
        if count == 0 {
            break;
        }
        written += count as u64;
        if written > MAX_DOWNLOAD_BYTES {
            let _ = fs::remove_file(destination);
            anyhow::bail!("the download from {url} was larger than any Rust release archive");
        }
        output.write_all(&buffer[..count])?;
        if written - last_report >= 8 * 1024 * 1024 {
            last_report = written;
            progress(Step::Download {
                part: part.to_string(),
                done: written,
                total: expected,
            });
        }
    }
    output.sync_all()?;
    progress(Step::Download {
        part: part.to_string(),
        done: written,
        total: expected,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The toolchain setup fetches is the one the repository builds with, so
    /// an app built on a fresh machine compiles with the same Rust as CI.
    #[test]
    fn the_pinned_release_is_the_one_the_repository_builds_with() {
        let pin = include_str!("../../../rust-toolchain.toml");
        assert!(
            pin.contains(&format!("channel = \"{VERSION}\"")),
            "rust-toolchain.toml and rust_toolchain::VERSION disagree"
        );
        assert!(pin.contains(WASM_TARGET));
    }

    /// Every machine setup offers has all four archives pinned, each a real
    /// SHA-256, and nothing is pinned that no machine uses.
    #[test]
    fn every_archive_setup_may_fetch_has_a_pinned_digest_and_size() {
        let mut used = std::collections::BTreeSet::new();
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("windows", "x86_64"),
            ("windows", "aarch64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
        ] {
            let host = host_for(os, arch).expect("a host");
            for asset in assets_for(host) {
                let (digest, size) =
                    pinned(&asset).unwrap_or_else(|| panic!("{asset} is not pinned"));
                assert_eq!(digest.len(), 64, "{asset}");
                assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()), "{asset}");
                assert!(size > 1_000_000, "{asset} size looks wrong");
                used.insert(asset);
            }
        }
        for (asset, _, _) in PINNED {
            assert!(used.contains(*asset), "{asset} is pinned but never used");
        }
    }

    #[test]
    fn windows_builds_with_gnullvm_so_no_visual_studio_is_needed() {
        assert_eq!(
            host_for("windows", "x86_64"),
            Some("x86_64-pc-windows-gnullvm")
        );
        assert_eq!(host_for("freebsd", "x86_64"), None);
        assert!(assets_for("x86_64-pc-windows-gnullvm")
            .iter()
            .all(|a| a.ends_with(".tar.gz")));
        assert!(assets_for("aarch64-apple-darwin")
            .iter()
            .all(|a| a.ends_with(".tar.xz")));
    }

    #[test]
    fn a_mac_build_links_with_its_own_lld_and_never_asks_xcrun() {
        let toolchain = Toolchain {
            root: PathBuf::from("/k/rust"),
            host: "aarch64-apple-darwin".to_string(),
        };
        let vars = variables(&toolchain);
        let get = |name: &str| {
            vars.iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| panic!("{name} not set: {vars:?}"))
        };
        // `rust-lld` by name, so rustc passes ld64's flags, not a cc driver's.
        assert!(get("CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER").ends_with("/rust-lld"));
        assert!(get("SDKROOT").ends_with("sdk"));
        assert!(get("CARGO_HOME").ends_with("cargo-home"));
        assert!(get("RUSTC").contains("bin"));
        let script = linker_script("aarch64-apple-darwin");
        assert!(script.contains("lib/rustlib/aarch64-apple-darwin/bin/rust-lld"));
        assert!(script.contains("-undefined dynamic_lookup"));
    }

    #[test]
    fn a_half_finished_install_is_never_used() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("rust");
        let host = "aarch64-apple-darwin";
        fs::create_dir_all(root.join("bin")).unwrap();
        let exe = std::env::consts::EXE_SUFFIX;
        fs::write(root.join(format!("bin/rustc{exe}")), "").unwrap();
        fs::write(root.join(format!("bin/cargo{exe}")), "").unwrap();
        fs::create_dir_all(root.join("lib/rustlib/wasm32-wasip1/lib")).unwrap();
        assert_eq!(Toolchain::at(root.clone(), host), None, "no marker yet");
        fs::write(root.join(COMPLETE_MARKER), "{}").unwrap();
        assert!(Toolchain::at(root, host).is_some());
    }

    /// The layout step on a hand-made archive tree: components listed are
    /// merged, `manifest.in` is dropped, and a component naming a path
    /// outside the archive is refused.
    #[test]
    fn components_are_merged_into_one_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let unpacked = dir.path().join("unpacked");
        let top = unpacked.join("rust-std-1.95.0-wasm32-wasip1");
        let lib = top.join("rust-std-wasm32-wasip1/lib/rustlib/wasm32-wasip1/lib");
        fs::create_dir_all(&lib).unwrap();
        fs::write(lib.join("libcore.rlib"), "core").unwrap();
        fs::write(top.join("rust-std-wasm32-wasip1/manifest.in"), "file:x").unwrap();
        fs::write(top.join("components"), "rust-std-wasm32-wasip1\n").unwrap();
        let prefix = dir.path().join("prefix");
        fs::create_dir_all(&prefix).unwrap();
        merge_components(&unpacked, &prefix).unwrap();
        assert_eq!(
            fs::read_to_string(prefix.join("lib/rustlib/wasm32-wasip1/lib/libcore.rlib")).unwrap(),
            "core"
        );
        assert!(!prefix.join("manifest.in").exists());

        fs::write(top.join("components"), "../escape\n").unwrap();
        assert!(merge_components(&unpacked, &prefix).is_err());
    }

    #[test]
    fn the_whole_download_is_said_before_it_starts() {
        // About 126 MB on an Apple-silicon Mac: the number the MCP server
        // tells a person before the one-time setup.
        let mac: u64 = assets_for("aarch64-apple-darwin")
            .iter()
            .map(|a| pinned(a).unwrap().1)
            .sum();
        assert!((120_000_000..135_000_000).contains(&mac), "{mac}");
    }
}
