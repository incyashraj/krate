# Testing Windows changes without cutting a release

Three releases went out in one day and two were broken on Windows, because the
only way to get a Windows binary was to publish one. This is the loop that
replaces that.

## One-time setup in the VM

**Take a snapshot first.** Testing a first-run experience only works once per
machine, and rolling back is how you get to be a first-time user again.

```powershell
winget install --id Git.Git -e
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override `
  "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Build Tools are needed and there is no way around them. Two things were checked
before accepting that:

- **Building Krate itself** pulls 282 crates including wasmtime and whisper.
  That obviously needs a linker, and nobody but us builds it.
- **Building a Krate app** pulls five, all pure Rust targeting wasm -- but one
  of them, `wit-bindgen-rt`, ships a `build.rs`. A build script is a host
  executable, so it has to be compiled and linked for Windows. rustup's
  `gnullvm` toolchain would avoid MSVC, but only for its own targets; the build
  script still links for the host.

So a Windows user who wants to **make** apps needs Rust and Build Tools.
Opening an app someone sent them needs neither.

## Getting a test binary

Do not build Krate from source in the VM unless you want to wait through 282
crates. Ask for a build instead and download the artifact:

```
gh workflow run testbuild.yml -f target=aarch64-pc-windows-msvc
```

Then Actions -> the run -> Artifacts -> `krate-test-aarch64-pc-windows-msvc`.
Unzip and run `krate.exe`.

## What to check

**1. It asks about the toolchain before anything else.** Type `krate`, pick
**Make an app**, describe something. The compiler question should come before
the AI picker, not after "cooking with grok".

**2. cargo-component is not compiled from source.** It ships in the archive,
so `cargo install cargo-component` and its 378 crates should never appear.

**3. The app builds and opens.**

## If it fails

Copy the whole error. The useful part is usually the line naming a linker or a
missing target, and the two commands after it.

## What is not being tested here

Signing. Windows builds are unsigned and SmartScreen will warn until the
project has download reputation -- an EV certificate is several hundred dollars
a year and cannot buy reputation anyway. That is a separate decision, not part
of this.
