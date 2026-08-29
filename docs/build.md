# Building Krate from source

This page is for working on the player itself. To make or open apps you
do not need any of it -- [download Studio](https://krate.tech/studio/) or
grab a [release](https://github.com/incyashraj/krate/releases/latest).

```bash
git clone https://github.com/incyashraj/krate
cd krate

cargo build --workspace
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

You need the Rust toolchain named in `rust-toolchain.toml`, Git, your
platform's build tools, and `cargo-component` for building apps. Check
your machine with `krate doctor`.

## Linux

Five development packages, found one at a time on a machine with nothing
installed. Each is a separate failed build, and none of the errors names
the package you actually need:

```bash
sudo apt-get install build-essential pkg-config libssl-dev cmake \
  libasound2-dev libwayland-dev libxkbcommon-dev libudev-dev
```

```bash
sudo dnf install gcc-c++ pkgconf openssl-devel cmake \
  alsa-lib-devel wayland-devel libxkbcommon-devel systemd-devel
```

What each is for, and what it looks like when it is missing:

- **libasound2-dev**: the microphone capability. Stops in `alsa-sys` with a
  `pkg-config` error that does not explain itself.
- **libwayland-dev**: windowing. The worst failure of the five: a *panic
  inside `wayland-sys`'s build script*, which reads as a broken crate rather
  than a missing package.
- **libxkbcommon-dev**: the keyboard.
- **libudev-dev**: gamepads.
- **cmake**: builds whisper.cpp for speech-to-text. Skip the whole thing with
  `--no-default-features` if you do not need it.

### Running an app on Linux, without building anything

The list above is for building Krate. Someone who only opens a `.krate`
needs one package, and only on an X11 desktop:

```bash
sudo apt install libxkbcommon-x11-0
```

Ubuntu splits the keyboard library in two. `libxkbcommon0` ships with Ubuntu
Desktop; the X11 bridge in `libxkbcommon-x11-0` does not, and X11 windows need
it. Fedora calls it `libxkbcommon-x11`, and on Arch it is part of
`libxkbcommon`. A Wayland-only session never loads it.

Without it, Krate says so in a sentence and names the package. It used to be a
Rust panic quoting a crate path (K-036).

## Windows

Visual Studio Build Tools with the "Desktop development with C++" workload.
Two things beyond that, both of which cost real time to discover:

- **libclang**, for the default feature set: whisper's bindgen needs it.
  Without it, build with `--no-default-features`.
- **A pagefile.** With 16 GB of RAM and no pagefile, the release link runs out
  of memory and Windows kills the process **with no message and an empty
  log**. Three builds died silently before that was the answer. An automatic
  pagefile fixes it.

Build just the CLI:

```bash
cargo build --release -p krate-cli
target/release/krate --version
```

## Installing the runtime alone, from a terminal

macOS and Linux:

```bash
curl -fsSL https://krate.tech/install.sh | sh
```

Windows PowerShell, no administrator rights needed:

```powershell
irm https://krate.tech/install.ps1 | iex
```

`irm` is a PowerShell alias, so that line does not work in Command Prompt.
From `cmd.exe`, type `powershell` first and then paste it.

Then run an app, from a file or straight from a URL:

```bash
krate run notes.krate --prompt
```

`--prompt` shows you each thing the app asks for and waits for your answer.

The `krate-app` zip on the
[latest release](https://github.com/incyashraj/krate/releases/latest) is the
signed, notarised macOS opener, and
[`scripts/install-krate-desktop.ps1`](../scripts/install-krate-desktop.ps1) /
[`scripts/install-krate-desktop.sh`](../scripts/install-krate-desktop.sh)
register the `.krate` file type for the bare CLI on Windows and Linux.
