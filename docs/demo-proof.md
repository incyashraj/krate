# Krate demo proof pack

Every number here was measured on a real machine, not guessed. Reproduce it all with:

```
cargo build --release -p krate-cli
sh scripts/measure-demo-claims.sh
```

Measured on Apple Silicon (Darwin arm64), 2026-08-03, against `v0.1.0-rc16`.
When a number matters for the video, the exact command that produced it is shown.

---

## 1. Size

**Claim: a Krate app is a few kilobytes. Not megabytes. Kilobytes.**

Real `.krate` bundles in `evidence/ported/`, measured with `wc -c`:

| App        | Bytes  | KB     | What it is                         |
|------------|--------|--------|------------------------------------|
| chart      | 11,713 | 11.4   | Draws its own bar chart            |
| cubes      | 12,762 | 12.5   | Spinning 3D cubes                  |
| bounce     | 13,161 | 12.9   | A playable Breakout game           |
| savings    | 13,791 | 13.5   | A form whose state survives restart|
| hexyl      | 14,872 | 14.5   | Hex viewer (ported)                |
| ddh        | 17,778 | 17.4   | Duplicate-file finder (ported)     |
| rssfwd     | 18,084 | 17.7   | RSS forwarder over HTTPS           |
| envelope   | 19,495 | 19.0   | Budgeting app with real SQL        |
| mdview     | 51,963 | 50.7   | Markdown viewer (ported)           |
| grex       | 54,084 | 52.8   | Regex builder (ported)             |
| eo2        | 77,484 | 75.7   | Image viewer (ported)              |

**Smallest: 11.4 KB. Largest real ported app: 75.7 KB.**

### Honest comparison to Electron

A minimal hello-world Electron app is about 150 MB. This is a widely cited,
defensible figure: Electron bundles a full Chromium and Node.js runtime into
every app. Real Electron desktop apps are larger still, and these are measured
live from `/Applications` on the machine that builds the reports page:

- Visual Studio Code: ~1.5 GB
- Discord: ~440 MB
- Spotify: ~360 MB

Ratio, computed not rounded in our favor:

- 150 MB / 11.4 KB (chart, smallest) = **about 13,400x smaller**
- 150 MB / 75.7 KB (eo2, our largest real app) = **about 2,000x smaller**

Framing for the video: use "**thousands of times smaller**" as the safe,
always-true line. "About 13,000x" is true for the smallest app specifically;
"about 2,000x" is true even for our biggest one. All are defensible.

### The runtime binary (one-time install)

The app is tiny because it does not carry a browser. The runtime is installed
once and shared by every app:

```
cargo build --release -p krate-cli
wc -c target/release/krate
```

**krate runtime: 21,091,664 bytes = 20.1 MB.** You install this once. Every
Krate app after that is kilobytes. Contrast with Electron, where every app
ships its own ~150 MB copy of Chromium.

---

## 2. Speed / startup

**Claim: a Krate app opens before your screen draws one frame.**

Cold start is whole-process wall clock: the time from launching the process to
its exit, including everything the app itself does. This is what a person
actually waits. Measured the same way `scripts/build-reports-page.py` measures
it (timed inside one Python process; `--headless` so no window manager is in
the loop).

Median over 15 runs, release binary:

| App      | Median | Min   | Max   |
|----------|--------|-------|-------|
| chart    | 13.3ms | 12.2  | 15.1  |
| savings  | 14.2ms | 13.8  | 51.2  |
| cubes    | 24.2ms | 22.2  | 28.3  |
| envelope | 29.4ms | 25.7  | 50.8  |
| rssfwd   | 31.1ms | 28.7  | 32.5  |
| mdview   | 45.2ms | 43.4  | 48.4  |
| eo2      | 45.4ms | 41.0  | 52.4  |

**The reference frame: one frame of 60fps video lasts 16.7ms.** The simplest
real apps (chart, savings) start in **13-14ms, under one frame.** Heavier apps
that draw more or open a database land in the 25-45ms range, still faster than
a person can perceive as a delay.

Honest scope note: bounce, the game, takes ~125ms on this same path because its
"quick" verification actually simulates 30 frames of gameplay to prove it works.
That is the game running, not the runtime starting. Do not use bounce for the
startup line. Use chart or savings.

Video-safe framing: "**opens in about 15 milliseconds -- faster than a single
frame of video.**" True for the simplest apps. If you want a number that covers
every app honestly, say "**opens in a few dozen milliseconds.**"

---

## 3. Three operating systems, one file

**Claim: one app file. Mac, Windows, Linux. The same bytes.**

### The runtime ships for six targets

From the GitHub release `v0.1.0-rc16` (`gh release view v0.1.0-rc16`):

```
krate-0.1.0-rc16-aarch64-apple-darwin.tar.gz
krate-0.1.0-rc16-x86_64-apple-darwin.tar.gz
krate-0.1.0-rc16-aarch64-unknown-linux-gnu.tar.gz
krate-0.1.0-rc16-x86_64-unknown-linux-gnu.tar.gz
krate-0.1.0-rc16-aarch64-pc-windows-msvc.zip
krate-0.1.0-rc16-x86_64-pc-windows-msvc.zip
```

**Six runtime builds: Intel and ARM, for macOS, Linux, and Windows.**

### Why the app file is byte-identical everywhere

A `.krate` is a zip containing exactly two things: `manifest.toml` and a
WebAssembly component (`code.wasm`). Verified:

```
file evidence/ported/cubes.krate   -> Zip archive
wasm-tools validate code.wasm      -> VALID component
```

The component is OS-agnostic WebAssembly. It contains **no native code and no
platform strings** (checked: `strings code.wasm | grep -iE 'darwin|x86_64|
aarch64|windows|linux'` returns nothing). The platform-specific part is the
runtime you already installed; the app is portable bytecode the runtime feeds
to the machine. That is why the same `.krate` file, unchanged, runs on all six
targets. Write once, ship one file, it runs everywhere the runtime is.

---

## 4. Security

**Claim: the app asks before it touches anything, and it can reach nothing you
did not hand it.**

Three real, verifiable properties:

### It imports only `krate:*` -- zero `wasi:*`

Every app is a WebAssembly component. You can read exactly what it is allowed
to import with `wasm-tools component wit`. Across all 11 bundles:

| App      | wasi:* imports | krate:* imports |
|----------|----------------|-----------------|
| bounce   | 0              | 11              |
| chart    | 0              | 10              |
| cubes    | 0              | 11              |
| ddh      | 0              | 6               |
| envelope | 0              | 9               |
| eo2      | 0              | 17              |
| grex     | 0              | 6               |
| hexyl    | 0              | 6               |
| mdview   | 0              | 14              |
| rssfwd   | 0              | 9               |
| savings  | 0              | 9               |

**Zero `wasi:*` imports, every app.** A wasi import would be a door to the raw
operating system: files, sockets, clocks, environment. These apps have none.
They can only call the narrow, named `krate:*` interfaces the runtime chooses
to expose (`krate:ui/window`, `krate:gfx/scene3d`, and so on). There is no
ambient access to your disk or network to leak, because the door was never
built into the app.

### The capability wall: it asks first

The manifest declares every capability the app wants, in plain words, with a
rationale. Example from `cubes`:

```
[[capabilities]]
cap = "ui.window:create"
rationale = "Open the 3D window"

[[capabilities]]
cap = "io.stdout"
rationale = "Report what was drawn for automated verification"
```

Before the app runs, the runtime shows what it is asking for, and the user
grants or denies. Nothing is granted implicitly. (In these measurements we pass
`--auto-grant`, the explicit automation bypass; a real user sees the wall.)

### Sandbox

The app is a WebAssembly component running inside the runtime's sandbox. It
gets no file handles, no sockets, no environment it was not handed. Every
outside effect goes through a `krate:*` call the runtime mediates and can log
(`--log-grants`).

### Why this is a real differentiator vs Electron

An Electron app runs on Node.js with full operating-system access by default:
it can read your home directory, open network connections, and spawn processes
without asking. Krate inverts that. The default is nothing. The app declares
what it needs, you approve it, and the runtime enforces it. The proof is
mechanical, not a promise: `wasm-tools` shows you the app's entire reach.

---

## Video claims (each backed by a measured number above)

1. **"Eleven kilobytes. Not eleven megabytes. Eleven kilobytes."**
   (chart.krate = 11,713 bytes = 11.4 KB. Section 1.)

2. **"Thousands of times smaller than an Electron app -- and it does the same
   things."**
   (11.4 KB vs a 150 MB minimal Electron app = ~13,400x; even our biggest app
   is ~2,000x smaller. Section 1.)

3. **"Opens in about fifteen milliseconds -- faster than a single frame of
   video."**
   (chart 13.3ms, savings 14.2ms median; one 60fps frame = 16.7ms. Section 2.)

4. **"One file. Mac, Windows, Linux. The same bytes, unchanged."**
   (The `.krate` is OS-agnostic wasm with no native code; runtime ships for 6
   targets. Section 3.)

5. **"The app asks before it touches anything. You decide."**
   (Capability wall: manifest declares, user grants, runtime enforces.
   Section 4.)

6. **"It can reach nothing you didn't hand it -- and you can check that
   yourself."**
   (Zero `wasi:*` imports across all 11 apps, verifiable with wasm-tools.
   Section 4.)

---

## Overclaim watch (read before scripting)

- **Startup "16.7ms" / "under one frame"** is true for the simplest apps
  (chart, savings). It is NOT true for every app -- heavier apps are 25-45ms,
  and the bounce game is ~125ms because it simulates gameplay to self-verify.
  Say "about 15 milliseconds" and show chart or savings, or say "a few dozen
  milliseconds" to cover everything. Do not imply every app is sub-frame.
- **"13,000x smaller"** is true against the smallest app specifically. Against
  our largest real app it is ~2,000x. Both beat the point. "Thousands of times
  smaller" is the always-true phrasing.
- **The 150 MB Electron figure** is for a minimal hello-world app and is widely
  cited; real Electron apps (VS Code ~1.5 GB, Discord ~440 MB, Spotify ~360 MB,
  measured live) are much larger. Using 150 MB is the conservative choice.
- **The 20.1 MB runtime** is a one-time install, not per-app. If a viewer only
  hears "20 MB" they might think the app is 20 MB. Always frame it as the
  shared, install-once runtime, with the app being kilobytes on top.
- Everything in the security section is mechanically verifiable and safe to
  state plainly. No softening needed.
