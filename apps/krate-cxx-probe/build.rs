//! Build cpp/probe.cpp with wasi-sdk and link wasi-libc and libc++ (no
//! exceptions); krate-c-runtime answers their WASI calls inside the app.
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let sdk = PathBuf::from(std::env::var("WASI_SDK").unwrap_or_else(|_| {
        format!(
            "{}/.krate-toolchains/wasi-sdk-34.0-arm64-macos",
            std::env::var("HOME").unwrap()
        )
    }));
    let sysroot = sdk.join("share/wasi-sysroot");
    let obj = out.join("probe.o");
    let ok = Command::new(sdk.join("bin/clang++"))
        .args([
            "--target=wasm32-wasip1",
            "-O2",
            "-fno-exceptions",
            "-std=c++17",
        ])
        .arg(format!("--sysroot={}", sysroot.display()))
        .arg("-c")
        .arg(root.join("cpp/probe.cpp"))
        .arg("-o")
        .arg(&obj)
        .status()
        .expect("clang++")
        .success();
    assert!(ok, "clang++ failed");
    let lib = out.join("libprobe.a");
    let _ = std::fs::remove_file(&lib);
    assert!(Command::new(sdk.join("bin/llvm-ar"))
        .arg("crs")
        .arg(&lib)
        .arg(&obj)
        .status()
        .unwrap()
        .success());
    println!("cargo:rustc-link-search=native={}", out.display());
    println!(
        "cargo:rustc-link-search=native={}",
        sysroot.join("lib/wasm32-wasip1/noeh").display()
    );
    println!(
        "cargo:rustc-link-search=native={}",
        sysroot.join("lib/wasm32-wasip1").display()
    );
    println!("cargo:rustc-link-lib=static=probe");
    println!("cargo:rustc-link-lib=static=c++");
    println!("cargo:rustc-link-lib=static=c++abi");
    println!("cargo:rustc-link-lib=static=c");
    println!("cargo:rerun-if-changed=cpp/probe.cpp");
}
