//! Compile Doom's C (c/doom, from doomgeneric) and its small C library
//! (c/libc.c) to WebAssembly objects, and hand cargo one static library.
//!
//! The compiler is a clang that knows the wasm32 target -- Homebrew's LLVM on
//! a Mac (`KRATE_CLANG` overrides). Every object is built freestanding, with
//! no system headers: c/include is the whole C library Doom sees, so nothing
//! can pull in a WASI import that Krate would refuse.
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let clang =
        std::env::var("KRATE_CLANG").unwrap_or_else(|_| "/opt/homebrew/opt/llvm/bin/clang".into());
    let ar =
        std::env::var("KRATE_AR").unwrap_or_else(|_| "/opt/homebrew/opt/llvm/bin/llvm-ar".into());
    let resource = String::from_utf8(
        Command::new(&clang)
            .arg("-print-resource-dir")
            .output()
            .expect("run clang")
            .stdout,
    )
    .unwrap();
    let resource = resource.trim();

    // The shareware WAD (DOOM1.WAD v1.9, md5 f0cefca49926d00903cf57551d901abe,
    // 4196020 bytes), freely distributable unmodified; Debian ships it as
    // doom-wad-shareware. DOOM_WAD overrides where it is read from.
    let wad = std::env::var("DOOM_WAD")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("../krate-doom-wad/doom1.wad"));
    assert!(
        wad.is_file(),
        "no WAD at {}: put the shareware doom1.wad there (or set DOOM_WAD)",
        wad.display()
    );
    println!("cargo:rustc-env=DOOM_WAD={}", wad.display());
    println!("cargo:rerun-if-changed={}", wad.display());

    let mut sources: Vec<PathBuf> = std::fs::read_to_string(root.join("c/SOURCES"))
        .expect("c/SOURCES")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| root.join("c/doom").join(l.trim()))
        .collect();
    sources.push(root.join("c/libc.c"));

    let mut objects = Vec::new();
    for src in &sources {
        println!("cargo:rerun-if-changed={}", src.display());
        let obj = out.join(src.file_name().unwrap()).with_extension("o");
        let status = Command::new(&clang)
            .args([
                "--target=wasm32",
                "-O2",
                "-ffreestanding",
                "-nostdinc",
                "-w",
            ])
            .arg("-isystem")
            .arg(root.join("c/include"))
            .arg("-isystem")
            .arg(format!("{resource}/include"))
            .args(["-DDOOMGENERIC_RESX=320", "-DDOOMGENERIC_RESY=200"])
            .arg("-c")
            .arg(src)
            .arg("-o")
            .arg(&obj)
            .status()
            .expect("run clang");
        assert!(status.success(), "clang failed on {}", src.display());
        objects.push(obj);
    }
    let lib = out.join("libdoom.a");
    let _ = std::fs::remove_file(&lib);
    let status = Command::new(&ar)
        .arg("crs")
        .arg(&lib)
        .args(&objects)
        .status()
        .expect("run llvm-ar");
    assert!(status.success(), "llvm-ar failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=doom");
    println!("cargo:rerun-if-changed=c/include");
    println!("cargo:rerun-if-changed=c/SOURCES");
}
