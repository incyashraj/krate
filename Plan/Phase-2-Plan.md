# OneOS — Phase 2 Detailed Plan: UAPI v0.1 (CLI)

> **Phase:** 2 of 8
> **Duration:** Months 4–6 (90 calendar days, ~40–50 engineering days of work)
> **Phase sentence:** *Ship the first useful cross-platform CLI app through our runtime.*
> **Prerequisite:** Phase 1 complete — runtime + CLI + CI green on three desktop hosts.
> **Supersedes:** `oneos:phase1/host` WIT interface.
> **Superseded by:** nothing.

---

## Table of Contents

0. [How to Use This Document](#0-how-to-use-this-document)
1. [Phase Objective](#1-phase-objective)
2. [Prerequisites from Phase 1](#2-prerequisites-from-phase-1)
3. [Success Criteria](#3-success-criteria)
4. [What Phase 2 Is and Is Not](#4-what-phase-2-is-and-is-not)
5. [Architecture](#5-architecture)
6. [Technology Decisions](#6-technology-decisions)
7. [UAPI v0.1 Module Specifications](#7-uapi-v01-module-specifications)
8. [Host Adapter Design](#8-host-adapter-design)
9. [Language Bindings](#9-language-bindings)
10. [UCap v0.1 (Soft Enforcement)](#10-ucap-v01-soft-enforcement)
11. [Sample Applications](#11-sample-applications)
12. [Week-by-Week Breakdown](#12-week-by-week-breakdown)
13. [Task Details](#13-task-details)
14. [Code Skeletons](#14-code-skeletons)
15. [Testing Strategy](#15-testing-strategy)
16. [Performance Targets](#16-performance-targets)
17. [Security & Threat Model v0.2](#17-security--threat-model-v02)
18. [Documentation Deliverables](#18-documentation-deliverables)
19. [Architecture Decision Records](#19-architecture-decision-records)
20. [Exit Criteria Checklist](#20-exit-criteria-checklist)
21. [Phase 2 Risks](#21-phase-2-risks)
22. [Handoff to Phase 3](#22-handoff-to-phase-3)
23. [Appendices](#23-appendices)

---

## 0. How to Use This Document

Phase 2 is where OneOS becomes useful. Phase 1 proved we could run a WASM binary; Phase 2 proves we can run a WASM binary that *does something*. It is also the first phase where the decisions you make will live forever — WIT interfaces become ABI commitments, and breaking a v0.1 module costs you every app that ever depended on it.

- Every task has an ID (`P2-UAPI-01`, etc.) matching §7 of the main Build Plan.
- The WIT specs in §7 are the most important artifacts of this phase. They will be copied, pasted, generated from, and depended on by every future phase. Get them right, not fast.
- Phase 2 is ~4× the scope of Phase 1. Resist the urge to compress the calendar. Three months is the right answer.
- If a task feels like it belongs to Phase 3 (GUI) or later, it does. Defer it.

---

## 1. Phase Objective

### 1.1 One-sentence objective

**A developer writes a CLI app in Rust, Go, or TypeScript against UAPI v0.1, compiles it to a `.wasm` component, and runs it through `oneos` on Linux, macOS, and Windows — where it reads files, makes HTTP calls, prints to stdout, and uses time/locale primitives correctly.**

### 1.2 Why this matters

Phase 1 had one host import (`print`) and no real I/O. A runtime that cannot read a file or make a network request is not a runtime, it is a demo. Phase 2 is the first phase where an outside developer can reasonably attempt to build a real thing with OneOS. Everything after Phase 2 — GUI, mobile, marketplace — is an extension of primitives that must be correct here.

### 1.3 The five deliverables of Phase 2

1. **Five UAPI modules** — `io`, `fs`, `net`, `time`, `locale` — designed as WIT, implemented on three desktop hosts.
2. **Language bindings** for Rust (first-class), Go via TinyGo, TypeScript via jco.
3. **Three sample CLI apps** — `oneos-curl`, `oneos-cat`, `oneos-clock` — all passing cross-host identity tests.
4. **UCap v0.1 (soft enforcement)** — manifest-declared capabilities, runtime-checked at UAPI call sites, one-time grant at launch via terminal prompt.
5. **An integration test harness** that builds each sample, runs it via `oneos run` on all three hosts, and asserts byte-identical stdout.

---

## 2. Prerequisites from Phase 1

Before touching a single line of Phase 2 code, verify:

- [ ] All Phase 1 exit criteria met (see Phase 1 Plan §15).
- [ ] `oneos run hello.wasm` works on Linux, macOS, Windows.
- [ ] `cargo build --release --workspace` produces `oneos` binary on all three hosts.
- [ ] Release artifacts have been cut at least once (`v0.1.0-rc1` or later).
- [ ] Phase 1 benchmarks recorded and published.
- [ ] Threat Model v0.1 published.
- [ ] ADR-0002 (Wasmtime) and ADR-0003 (Component Model) merged.
- [ ] `wasm32-wasip2` is the default WASM target.

If any box is unchecked, finish Phase 1 first. Phase 2 assumes a stable runtime foundation — building on a shaky Phase 1 compounds problems.

---

## 3. Success Criteria

Phase 2 is **done** when, and only when, every row below is true.

| # | Criterion | Measured How |
|---|-----------|--------------|
| 1 | All five UAPI modules defined as stable v0.1 WIT | `wit/oneos/*.wit` frozen |
| 2 | Each module implemented in Linux, macOS, Windows host adapters | CI green on all hosts |
| 3 | Rust bindings generated and usable | `cargo add oneos && use oneos::fs` works |
| 4 | Go bindings (TinyGo) generated and usable | Sample builds and runs |
| 5 | TypeScript bindings (jco) generated and usable | Sample builds and runs |
| 6 | `oneos-curl <url>` works identically on all three hosts | Integration test |
| 7 | `oneos-cat <file>` works identically on all three hosts | Integration test |
| 8 | `oneos-clock` prints time in user's locale on all three hosts | Integration test |
| 9 | UCap v0.1: manifest-declared caps enforced at UAPI boundary | Attempting unauthorized call traps cleanly |
| 10 | Startup overhead for a UAPI-using app < 150 ms | Benchmark suite |
| 11 | UAPI hot-path dispatch < 1 µs | Microbenchmark |
| 12 | A developer who knows Rust but not WASM can write a new UAPI-using CLI in < 30 min | Timed walkthrough |
| 13 | UAPI Reference docs auto-generated from WIT | Published on docs site |
| 14 | WIT Style Guide merged | `docs/book/src/wit-style.md` |
| 15 | ADRs 0006 through at least 0012 merged | Git log |

---

## 4. What Phase 2 Is and Is Not

### 4.1 Phase 2 IS

- The first real UAPI — stable interface types for five fundamental modules.
- Three per-host adapter implementations.
- Three language binding pipelines.
- The beginning of UCap (soft enforcement, terminal prompts only).
- Three sample CLI apps that prove the UAPI works end-to-end.
- A capability-aware runtime: `oneos run --grant net,fs:./data/**` style.

### 4.2 Phase 2 is NOT

- Not a GUI. No window, no pixels, no input events. That is Phase 3.
- Not mobile. No iOS, no Android. That is Phase 4.
- Not a server. No HTTP server, no socket listeners. Phase 3+ (and even then, client-first).
- Not the complete UAPI. Missing from v0.1: storage, crypto, sensors, UI, GPU, audio, IPC, notifications, accessibility, AI, identity. Those come later.
- Not complete UCap. No user-facing grant UI (that is Phase 3), no policy DB (that is Phase 6), no revocation flow (Phase 6).
- Not a bundle format. `.oneapp` is Phase 6. Phase 2 ships `.wasm` components with a sidecar `manifest.toml` consumed by the CLI.

### 4.3 The scope discipline

Every request during Phase 2 will sound like "can we also add X?" The answer is almost always "Phase N." Keep a running list in `docs/book/src/phase2/deferred.md` so deferrals feel like decisions, not refusals.

---

## 5. Architecture

### 5.1 System overview at end of Phase 2

```mermaid
flowchart TB
    subgraph Dev["Developer Side"]
        SRC["main.rs / main.go / main.ts"]
        WIT["wit/oneos/*.wit"]
        WITG["wit-bindgen / jco / TinyGo"]
        CC["compile to component"]
        MANIFEST["manifest.toml"]
        COMP["component.wasm"]
        SRC --> CC
        WIT --> WITG --> CC
        CC --> COMP
        MANIFEST -.sidecar.-> COMP
    end

    subgraph RT["OneOS Runtime"]
        CLI["oneos CLI"]
        LOAD["Loader + Manifest Parser"]
        UCAP["UCap Enforcer v0.1"]
        ENGINE["Wasmtime"]
        DISP["UAPI Dispatcher"]
        CLI --> LOAD --> UCAP --> ENGINE
        ENGINE <--> DISP
    end

    subgraph ADPT["Host Adapters"]
        LINUX["Linux: libc, hyper, chrono, ICU"]
        MACOS["macOS: libc, hyper, chrono, ICU"]
        WIN["Windows: Win32 + hyper + chrono + ICU"]
    end

    COMP -->|oneos run| CLI
    MANIFEST -->|declared caps| LOAD
    DISP --> LINUX
    DISP --> MACOS
    DISP --> WIN
```

### 5.2 UAPI call flow

```mermaid
sequenceDiagram
    participant App as WASM App
    participant RT as Runtime
    participant UC as UCap Enforcer
    participant Ad as Host Adapter
    participant OS as Host OS

    App->>RT: fs.open("data.txt", read)
    RT->>UC: check cap "fs.read:data.txt"
    alt cap not granted
        UC-->>RT: deny
        RT-->>App: error::permission-denied
    else cap granted
        UC-->>RT: allow
        RT->>Ad: linux_fs_open(path, mode)
        Ad->>OS: open(2) syscall
        OS-->>Ad: fd = 5
        Ad-->>RT: Ok(Handle(5))
        RT-->>App: file resource handle
    end
```

### 5.3 Startup flow with manifest + UCap

```mermaid
flowchart TD
    A([oneos run app.wasm]) --> B{manifest.toml<br/>next to wasm?}
    B -- no --> C[No capabilities declared]
    B -- yes --> D[Parse manifest]
    D --> E[Extract capability list]
    E --> F{any caps not<br/>in --grant?}
    F -- yes --> G{--auto-grant<br/>or interactive?}
    G -- interactive --> H[Terminal prompt per cap]
    H --> I{user approves?}
    I -- no --> REJECT[Exit 5, permission denied]
    I -- yes --> J[Add to session grants]
    G -- auto-grant --> J
    F -- no --> J
    C --> J
    J --> K[Load component]
    K --> L[Create Store with granted caps]
    L --> M[Instantiate + call run]
    M --> DONE([exit])
```

### 5.4 Crate layout at end of Phase 2

```mermaid
flowchart LR
    CLI[oneos-cli] --> RT[oneos-runtime]
    CLI --> MAN[oneos-manifest]
    RT --> WT[wasmtime]
    RT --> POL[oneos-policy]
    RT --> AD_LINUX[oneos-adapter-linux]
    RT --> AD_MAC[oneos-adapter-macos]
    RT --> AD_WIN[oneos-adapter-windows]
    AD_LINUX --> TOKIO[tokio, hyper, chrono, icu4x]
    AD_MAC --> TOKIO
    AD_WIN --> TOKIO
    POL --> MAN

    BIND_RUST[oneos-bindings-rust] -.generated from.-> WIT[wit/oneos]
    BIND_GO[oneos-bindings-go] -.generated from.-> WIT
    BIND_TS[oneos-bindings-ts] -.generated from.-> WIT
```

### 5.5 Trust boundaries (Phase 2)

```mermaid
flowchart LR
    subgraph T1[Untrusted]
        APP[App WASM]
        MAN[manifest.toml]
    end
    subgraph T2[Trusted]
        RT[Runtime]
        POL[UCap enforcer]
        AD[Host adapter]
    end
    subgraph T3[Platform]
        OS[Host OS kernel]
    end
    APP --UAPI calls--> RT
    MAN --cap declarations--> POL
    POL --enforces--> RT
    RT --> AD --syscalls--> OS
```

Change from Phase 1: the manifest is now a trust-boundary input, and UCap enforcement sits between the WASM and the adapter.

---

## 6. Technology Decisions

Every choice below is frozen for Phase 2. Changes require an ADR.

### 6.1 Interface definition: **WIT + Component Model** (continues from ADR-0003)

All UAPI modules are WIT interfaces. Phase 2 produces `wit/oneos/*.wit` files that are *normative* — once published they cannot break in v0.1.x.

### 6.2 WIT package versioning: **semver per module**

- `oneos:io@0.1.0`
- `oneos:fs@0.1.0`
- `oneos:net@0.1.0`
- `oneos:time@0.1.0`
- `oneos:locale@0.1.0`

Every module versions independently. v0.1.0 is *pre-stable* — breaking changes allowed until v1.0.0. Bumping a module version requires an ADR.

### 6.3 Bindings: **Rust (wit-bindgen), Go (TinyGo), TypeScript (jco)**

- **Rust** is first-class: the runtime is Rust, so we eat our own binding output first.
- **Go** via **TinyGo** — the only Go toolchain producing WASM components at acceptable quality today. Full Go's stdlib is too large.
- **TypeScript** via **jco** (ComponentizeJS). Produces components from JS/TS using SpiderMonkey under the hood; binary size is large but DX is best-in-class.

### 6.4 HTTP: **`hyper` + `rustls` + `tokio`**

- `hyper` for HTTP/1.1 and HTTP/2.
- `rustls` for TLS (no OpenSSL dependency).
- `tokio` for async runtime.
- `hickory-dns` for DNS resolution.

HTTP/3 / QUIC deferred to Phase 3 or later.

### 6.5 Async model inside runtime: **tokio single-threaded reactor per `Store`**

- Each WASM component instance gets a dedicated Tokio `current_thread` runtime.
- This avoids `Send` constraints on the host state and keeps per-app isolation simple.
- Multi-threaded Tokio is deferred — Phase 2 apps are CLIs, they don't need work-stealing.

### 6.6 Async exposure to WASM: **synchronous WIT, async under the hood**

Per ADR-0006 (see §19): UAPI functions appear synchronous from WASM's point of view. The runtime drives async I/O while the WASM is parked. This matches the Component Model's `async` proposal trajectory and avoids asking apps to reason about futures across the ABI boundary.

### 6.7 Filesystem path representation: **UTF-8 strings with platform normalization**

- Paths are `string` in WIT.
- On Windows, non-UTF-8 paths are rejected with `error::invalid-path`.
- Tilde expansion (`~/`) is runtime-owned, not app-owned.
- Relative paths are resolved against the app's *sandbox root*, not the CWD.

### 6.8 Time representation: **Unix milliseconds + explicit timezone**

- No `Duration` in v0.1 — just `u64` millis since epoch.
- Timezone is a separate query (`locale::timezone()`).
- Monotonic clock is distinct from wall-clock.

### 6.9 Locale / i18n: **ICU4X**

- Rust-native ICU implementation.
- Bundled statically in the runtime.
- Exposes formatting, collation, case conversion — not translation strings (that's app-owned).

### 6.10 Manifest format: **TOML**

- `manifest.toml` co-located with `.wasm` file (Phase 2 sidecar).
- Parsed by `oneos-manifest` crate using `toml` + `serde`.
- Schema validated on load.

### 6.11 Capability strings: **stable format from Phase 2**

Format `<module>.<action>[:<resource>]` is frozen in Phase 2 per Build Plan §10.2. Future phases may add new modules and actions but cannot change the syntax.

### 6.12 What we explicitly DEFER

| Feature | Deferred to | Reason |
|---|---|---|
| UI, graphics | Phase 3 | Largest surface; needs its own phase |
| Storage (SQLite) | Phase 3 | Needs manifest schema declarations |
| Crypto | Phase 3 | Low priority for CLI; ring needs hardening review |
| Sensors | Phase 4 | Mobile-first feature |
| Audio | Phase 3 | Needs GUI phase's event loop |
| HTTP server | Phase 5 or later | CLIs don't need it |
| WebSockets | Phase 5 | Client-first for now |
| IPC | Phase 5 | Requires process-model decisions |
| Notifications | Phase 3 | GUI-adjacent |
| Accessibility | Phase 3 | GUI-adjacent |
| AI inference | Phase 5+ | Standalone module, not prerequisite |
| Identity | Phase 6 | Requires marketplace |
| Bundle format | Phase 6 | Sidecar manifest adequate until then |

---

## 7. UAPI v0.1 Module Specifications

These five WIT files are the core deliverable of Phase 2. Each is presented with design rationale followed by the authoritative WIT text.

### 7.1 `oneos:io@0.1.0`

**Purpose:** standard I/O, logging.

**Design notes:**
- `stdin`, `stdout`, `stderr` are opened as resources, not raw functions.
- Binary-safe: no implicit encoding conversion.
- `log` is structured: level + message + key-value pairs.

```wit
// wit/oneos/io.wit
package oneos:io@0.1.0;

interface types {
    enum log-level {
        trace,
        debug,
        info,
        warn,
        error,
    }

    variant io-error {
        closed,
        interrupted,
        unexpected-eof,
        invalid-utf8,
        other(string),
    }
}

interface streams {
    use types.{io-error};

    resource input-stream {
        read: func(n: u32) -> result<list<u8>, io-error>;
        read-to-string: func() -> result<string, io-error>;
    }

    resource output-stream {
        write: func(bytes: list<u8>) -> result<u32, io-error>;
        write-all: func(bytes: list<u8>) -> result<_, io-error>;
        flush: func() -> result<_, io-error>;
    }
}

interface stdio {
    use streams.{input-stream, output-stream};

    stdin:  func() -> input-stream;
    stdout: func() -> output-stream;
    stderr: func() -> output-stream;
}

interface log {
    use types.{log-level};

    record field {
        key: string,
        value: string,
    }

    emit: func(level: log-level, message: string, fields: list<field>);
}

world consumer {
    import stdio;
    import log;
}
```

### 7.2 `oneos:fs@0.1.0`

**Purpose:** filesystem access within granted paths.

**Design notes:**
- Paths resolved against a per-app sandbox root.
- `file` is a resource — explicit close via drop.
- Directory listing returns names only, not full stat results (stat separately).
- No `O_DIRECTORY`, `O_APPEND`, symlink semantics, or permissions in v0.1.

```wit
// wit/oneos/fs.wit
package oneos:fs@0.1.0;

interface types {
    record file-stat {
        size: u64,
        modified-millis: u64,
        is-dir: bool,
    }

    variant open-mode {
        read,
        write,       // truncates
        read-write,  // truncates
        append,
    }

    variant fs-error {
        not-found,
        permission-denied,
        already-exists,
        invalid-path,
        not-a-directory,
        is-a-directory,
        io(string),
    }
}

interface files {
    use types.{file-stat, open-mode, fs-error};

    resource file {
        read: func(n: u32) -> result<list<u8>, fs-error>;
        write: func(bytes: list<u8>) -> result<u32, fs-error>;
        seek-set: func(pos: u64) -> result<u64, fs-error>;
        seek-end: func() -> result<u64, fs-error>;
        stat: func() -> result<file-stat, fs-error>;
    }

    open: func(path: string, mode: open-mode) -> result<file, fs-error>;
    stat: func(path: string) -> result<file-stat, fs-error>;
    list: func(path: string) -> result<list<string>, fs-error>;
    remove-file: func(path: string) -> result<_, fs-error>;
    remove-dir: func(path: string) -> result<_, fs-error>;
    mkdir: func(path: string) -> result<_, fs-error>;
    rename: func(from: string, to: string) -> result<_, fs-error>;
}

world consumer {
    import files;
}
```

### 7.3 `oneos:net@0.1.0`

**Purpose:** outbound HTTP client.

**Design notes:**
- v0.1 supports HTTP/1.1 and HTTP/2 only.
- TLS mandatory for `https://` (rustls).
- No streaming request bodies yet — full body buffered. Streaming is v0.2.
- Headers are a simple list of pairs (multimap semantics preserved).
- Connect-scoped caps: `net.connect:host:port` — wildcards allowed per Build Plan §10.2.

```wit
// wit/oneos/net.wit
package oneos:net@0.1.0;

interface types {
    enum http-method {
        get,
        post,
        put,
        delete,
        patch,
        head,
        options,
    }

    record header {
        name: string,   // lower-cased
        value: string,
    }

    record request {
        method: http-method,
        url: string,
        headers: list<header>,
        body: list<u8>,
        timeout-millis: option<u32>,
    }

    record response {
        status: u16,
        headers: list<header>,
        body: list<u8>,
    }

    variant net-error {
        invalid-url,
        dns-failure(string),
        connect-failure(string),
        tls-failure(string),
        timeout,
        body-too-large,
        permission-denied,
        protocol(string),
        other(string),
    }
}

interface http-client {
    use types.{request, response, net-error};

    fetch: func(req: request) -> result<response, net-error>;
}

world consumer {
    import http-client;
}
```

### 7.4 `oneos:time@0.1.0`

**Purpose:** clocks and sleeping.

**Design notes:**
- Wall-clock and monotonic explicitly distinct.
- `sleep` is synchronous-from-WASM (runtime handles async yielding).
- No timers as resources in v0.1 (that is Phase 3 for UI loops).

```wit
// wit/oneos/time.wit
package oneos:time@0.1.0;

interface clock {
    /// Milliseconds since Unix epoch. Wall-clock; can jump.
    now-millis: func() -> u64;

    /// Monotonic nanoseconds since some arbitrary origin.
    /// Guaranteed non-decreasing; suitable for measuring intervals.
    monotonic-nanos: func() -> u64;
}

interface sleep {
    /// Block the calling task for at least `millis` milliseconds.
    sleep-millis: func(millis: u32);
}

world consumer {
    import clock;
    import sleep;
}
```

### 7.5 `oneos:locale@0.1.0`

**Purpose:** formatting, timezone, user locale.

**Design notes:**
- Thin wrapper around ICU4X.
- No translation tables — apps own their translations.
- Locale ID is BCP 47 (e.g. `en-US`, `de-DE`, `hi-IN`).

```wit
// wit/oneos/locale.wit
package oneos:locale@0.1.0;

interface types {
    record locale-id {
        bcp47: string,     // e.g. "en-US", "hi-IN"
    }

    enum date-style {
        short,
        medium,
        long,
        full,
    }

    enum number-style {
        decimal,
        percent,
        currency,
    }
}

interface info {
    use types.{locale-id};

    /// The user's preferred locale as reported by the host.
    current: func() -> locale-id;

    /// IANA timezone name, e.g. "Asia/Singapore".
    timezone: func() -> string;
}

interface format {
    use types.{locale-id, date-style, number-style};

    format-date: func(
        millis: u64,
        tz: string,
        style: date-style,
        loc: locale-id,
    ) -> string;

    format-number: func(
        value: f64,
        style: number-style,
        loc: locale-id,
    ) -> string;
}

world consumer {
    import info;
    import format;
}
```

### 7.6 Consolidated `world`

Phase 2 apps target this composite world:

```wit
// wit/oneos/app.wit
package oneos:app@0.1.0;

world cli {
    import oneos:io/stdio@0.1.0;
    import oneos:io/log@0.1.0;
    import oneos:fs/files@0.1.0;
    import oneos:net/http-client@0.1.0;
    import oneos:time/clock@0.1.0;
    import oneos:time/sleep@0.1.0;
    import oneos:locale/info@0.1.0;
    import oneos:locale/format@0.1.0;

    export run: func() -> s32;
}
```

---

## 8. Host Adapter Design

### 8.1 Adapter structure

Each host adapter is a Rust crate (`crates/adapter-{linux,macos,windows}`) exposing a single trait:

```rust
pub trait HostAdapter: Send + Sync {
    fn io(&self) -> &dyn IoAdapter;
    fn fs(&self) -> &dyn FsAdapter;
    fn net(&self) -> &dyn NetAdapter;
    fn time(&self) -> &dyn TimeAdapter;
    fn locale(&self) -> &dyn LocaleAdapter;
}
```

Each sub-trait mirrors the WIT interface. Binding layer (`crates/runtime/src/uapi/`) translates WIT resource/function calls to `HostAdapter` method calls.

### 8.2 Why separate crates per host

- **Conditional compilation hell avoided.** `#[cfg(target_os)]` across a shared implementation becomes a maintenance nightmare at scale.
- **Cleaner CI.** Each platform builds only its own adapter — faster, clearer failures.
- **Independent evolution.** macOS might need a new Cocoa API that Linux doesn't — platform-specific code stays platform-specific.

### 8.3 Shared adapter logic

Cross-platform logic lives in `crates/adapter-common`:

- HTTP client (all platforms use same `hyper + rustls`).
- Time arithmetic.
- ICU4X formatting.
- Path canonicalization rules.

Per-platform crates consume `adapter-common` for shared bits and implement only the truly OS-specific parts (filesystem case-sensitivity rules, system locale detection, DNS resolver behavior).

### 8.4 Platform-specific gotchas

| Module | Linux | macOS | Windows |
|---|---|---|---|
| `fs` | UTF-8 paths OK | UTF-8 NFD/NFC normalization | UTF-16 internally; reject non-UTF-8 input |
| `fs.stat.modified-millis` | `stat.st_mtime` | Same, but 1-second resolution on some FS | `FILETIME` → millis conversion (100ns ticks) |
| `net` HTTP | Standard | Standard | Firewall prompt on first connect |
| `locale.current` | `LANG` / `LC_ALL` env vars | `CFLocaleCopyCurrent` via CoreFoundation | `GetUserDefaultLocaleName` |
| `locale.timezone` | `/etc/localtime` symlink read | `CFTimeZoneCopySystem` | `GetTimeZoneInformation` |
| `time.monotonic-nanos` | `clock_gettime(CLOCK_MONOTONIC)` | `mach_absolute_time` + scale | `QueryPerformanceCounter` |

Document each gotcha in `docs/book/src/phase2/adapter-notes-{linux,macos,windows}.md` so Phase 3+ contributors inherit the tribal knowledge.

### 8.5 Sandbox root

Every app gets a per-app sandbox directory:

- Linux: `~/.local/share/oneos/apps/<app-id>/`
- macOS: `~/Library/Application Support/oneos/apps/<app-id>/`
- Windows: `%LOCALAPPDATA%\oneos\apps\<app-id>\`

Relative `fs` paths resolve inside the sandbox root. Absolute paths (or `~/`-prefixed) require explicit caps in the manifest.

---

## 9. Language Bindings

### 9.1 Rust (first-class)

- `wit-bindgen` generates `oneos-bindings-rust` crate.
- Published to crates.io as `oneos`.
- Re-exports each module: `oneos::fs`, `oneos::net`, etc.
- Provides ergonomic `Result<T, Error>` wrappers around WIT-generated types.

```rust
// Developer-visible API
use oneos::fs::{open, OpenMode};

let mut file = oneos::fs::open("data.txt", OpenMode::Read)?;
let contents = file.read_to_string()?;
```

### 9.2 Go via TinyGo

- TinyGo project provides WASM component output (component-model support landed 2024).
- Bindings generated via `wit-bindgen-go`.
- Published as `github.com/oneos/oneos-go`.

```go
import "github.com/oneos/oneos-go/fs"

file, err := fs.Open("data.txt", fs.OpenModeRead)
if err != nil { ... }
contents, err := file.ReadToString()
```

**Known limitations:**
- TinyGo's Go is a subset: no `goroutines`-over-syscall, limited reflection, no `cgo`.
- Use for CLI tools that need the Go stdlib's string/parsing functions without needing full goroutine concurrency.

### 9.3 TypeScript via jco

- `jco` (Bytecode Alliance's JS component toolchain) produces components from JS/TS.
- Embeds SpiderMonkey — binary size 5–10 MB overhead.
- Best DX of the three: fastest to prototype, familiar to web developers.

```typescript
import { open, OpenMode } from 'oneos:fs/files';

const file = open('data.txt', OpenMode.Read);
const contents = new TextDecoder().decode(file.read(1_000_000));
```

### 9.4 What we are NOT doing in Phase 2

- No C/C++ bindings. Deferred to Phase 5 (SDK).
- No Python bindings. Deferred to Phase 5.
- No Swift, Kotlin, Java. All deferred.

Each of those deserves deliberate treatment — rushing them in Phase 2 dilutes the quality of the three we *are* shipping.

---

## 10. UCap v0.1 (Soft Enforcement)

### 10.1 What "soft" means

- Capabilities are declared in manifests and enforced at UAPI call sites.
- Grants are session-scoped — no persistent policy DB until Phase 6.
- UI is terminal-only — no system dialogs until Phase 3.
- Fine-grained revocation is Phase 6.

### 10.2 Grant model

Three ways an app obtains capabilities:

1. **`--grant` flag:** `oneos run --grant net,fs:./data/** app.wasm`.
2. **`--auto-grant` flag:** grants everything declared in the manifest.
3. **Interactive prompt:** default mode; prompts once per unique cap per session.

### 10.3 Interactive prompt example

```
oneos run hello.wasm

App: com.example.hello (v1.0.0)
Publisher: unsigned — caution
Requests the following capabilities:

  [1] fs.read:~/Documents/notes/**
      "Read saved notes"
  [2] net.connect:api.example.com:443
      "Sync to cloud"

Grant [A]ll / [N]one / per-cap [1,2,...] / [S]kip (exit):
>
```

### 10.4 Enforcement point

Every UAPI function annotated with one or more required capabilities. The dispatcher checks grants before calling the adapter:

```rust
fn fs_open(store: &mut Store<HostState>, path: String, mode: OpenMode)
    -> Result<File, FsError>
{
    let cap_needed = match mode {
        OpenMode::Read => Cap::FsRead(canonicalize(&path)?),
        _ => Cap::FsWrite(canonicalize(&path)?),
    };
    store.data().ucap.check(&cap_needed)?;
    // ...then delegate to adapter
}
```

### 10.5 Manifest schema (Phase 2)

```toml
[app]
id      = "com.example.hello"
name    = "Hello"
version = "1.0.0"
entry   = "hello.wasm"
world   = "oneos:app/cli@0.1.0"

[[capabilities]]
cap       = "fs.read:~/Documents/notes/**"
rationale = "Read saved notes"
required  = true

[[capabilities]]
cap       = "net.connect:api.example.com:443"
rationale = "Sync to cloud"
required  = false
```

### 10.6 Canonical capability enumeration

The complete list of capability strings usable in Phase 2 — locked for v0.1:

```
io.stdin
io.stdout
io.stderr
io.log
fs.read:<path-glob>
fs.write:<path-glob>
fs.list:<path-glob>
fs.remove:<path-glob>
fs.mkdir:<path-glob>
net.connect:<host>:<port>
time.clock
time.monotonic
time.sleep
locale.info
locale.format
```

Any UAPI call not covered by one of these caps — because someone added a new function without updating the cap table — fails a CI check.

### 10.7 Default grants

- `io.stdout`, `io.stderr`, `io.log`, `time.clock`, `time.monotonic`, `time.sleep`, `locale.info`, `locale.format` — **auto-granted**. Apps need these to do anything; no prompt necessary.
- Everything else — **must be declared and granted**.

---

## 11. Sample Applications

The three sample apps drive the UAPI design backward: if a common CLI pattern is hard to write, the UAPI is wrong. Build them in parallel with the UAPI, not after.

### 11.1 `oneos-curl`

**Purpose:** minimal HTTP client — OneOS's version of curl.

**Feature set (v0.1):**
- `oneos-curl <url>` — GET, print body.
- `-X POST` — request method.
- `-H "Name: value"` — custom headers (repeatable).
- `-d @file` or `-d 'text'` — request body.
- `-o file` — write response body to file instead of stdout.
- `-i` — include response headers in output.

**Implementation language:** Rust.
**Caps required:** `net.connect:*:*`, plus `fs.write:<out>` if `-o`, plus `fs.read:<file>` if `-d @`.
**LOC target:** < 300 lines including argument parsing.

### 11.2 `oneos-cat`

**Purpose:** minimal file reader — OneOS's version of cat.

**Feature set (v0.1):**
- `oneos-cat <file1> [file2 ...]` — concatenate and print to stdout.
- `-n` — number lines.
- `-b` — number non-blank lines only.
- Reading `-` reads from stdin.

**Implementation language:** Go (TinyGo).
**Caps required:** `fs.read:<each input path>`.
**Reason Go:** tests the TinyGo pipeline on a real app; cat is small enough that TinyGo's subset is comfortable.

### 11.3 `oneos-clock`

**Purpose:** show current time in locale-aware format.

**Feature set (v0.1):**
- `oneos-clock` — print current date and time in user's locale.
- `--tz <iana>` — override timezone (e.g. `--tz Asia/Tokyo`).
- `--format short|medium|long|full` — ICU style.
- `--json` — machine-readable output.

**Implementation language:** TypeScript (jco).
**Caps required:** `time.clock`, `locale.info`, `locale.format`.
**Reason TS:** tests jco pipeline; locale/date formatting is a natural TS fit.

### 11.4 Cross-host identity testing

All three samples have a property: **byte-identical stdout across hosts given byte-identical inputs**. The integration test harness enforces this.

Where the property cannot hold by physics (timestamps differ across runs, DNS varies), tests use frozen inputs:

- `oneos-curl http://localhost:PORT` against a test HTTP server with fixed responses.
- `oneos-cat test-fixtures/*.txt` with checked-in fixtures.
- `oneos-clock` tested with a frozen clock stub via `--test-time <millis>` flag (dev-only).

---

## 12. Week-by-Week Breakdown

Phase 2 is 12 weeks of calendar time, scaled for ~20 hours/week of active development. A full-time engineer compresses to 6–8 weeks.

### Weeks 1–2: WIT design

- Draft all five WIT files. Internal review. Iterate.
- Write WIT style guide (§18.1).
- ADR-0006 (sync UAPI over async adapter), ADR-0007 (path representation), ADR-0008 (time representation).

### Weeks 3–4: `io` + `time` + `locale`

- The easy three. Small adapters. Get the binding + dispatch pipeline working end-to-end on one module before tackling `fs` and `net`.
- Rust bindings via wit-bindgen.
- First sample: `oneos-clock` (TypeScript, jco) — also validates TS pipeline early.

### Weeks 5–6: `fs`

- Design doc for sandbox root (ADR-0009).
- Linux/macOS/Windows adapter implementations.
- Path canonicalization logic.
- `oneos-cat` sample in Go via TinyGo (validates Go pipeline).

### Weeks 7–8: `net`

- HTTP client adapter using hyper + rustls.
- DNS resolver integration.
- Timeout and error mapping.
- `oneos-curl` sample in Rust.

### Week 9: UCap v0.1

- Manifest parser.
- Cap enforcement at dispatch.
- Terminal grant prompts.
- `--grant` and `--auto-grant` CLI flags.

### Week 10: Integration + cross-host testing

- Integration test harness.
- All three samples running green in CI on all three hosts.
- Cross-host stdout diff tests.
- Benchmark suite additions.

### Week 11: Documentation

- UAPI reference auto-generated from WIT.
- Three tutorial walkthroughs (one per language).
- Threat model v0.2.
- Migration note: "from Phase 1 host-imports to Phase 2 UAPI".

### Week 12: Buffer + exit criteria + retro

- Walk the §20 checklist item by item.
- External volunteer does a tutorial cold; time and observe.
- Retrospective.
- Phase 3 kickoff plan.

---

## 13. Task Details

Each task targets approximately one engineer-week unless noted. Task IDs match Build Plan §7.3.

### P2-UAPI-01 — `wit/oneos/io.wit`

**Branch:** `p2-uapi-01-io-wit`.

**Acceptance:**
- File matches §7.1 exactly.
- Parses via `wasm-tools component wit`.
- Style guide checked.

### P2-UAPI-02 — `wit/oneos/fs.wit`

**Branch:** `p2-uapi-02-fs-wit`.

**Acceptance:**
- File matches §7.2.
- Parses via `wasm-tools`.
- Sandbox root concept documented in companion markdown.

### P2-UAPI-03 — `wit/oneos/net.wit`

**Branch:** `p2-uapi-03-net-wit`.

**Acceptance:**
- File matches §7.3.
- HTTP/1.1 and HTTP/2 explicit in module README.
- DNS error semantics documented.

### P2-UAPI-04 — `wit/oneos/time.wit`

**Branch:** `p2-uapi-04-time-wit`.

**Acceptance:**
- File matches §7.4.
- Wall vs monotonic distinction documented.

### P2-UAPI-05 — `wit/oneos/locale.wit`

**Branch:** `p2-uapi-05-locale-wit`.

**Acceptance:**
- File matches §7.5.
- BCP 47 linked from docs.

### P2-ADPT-01 — Linux adapters

**Estimate:** 3 days.
**Branch:** `p2-adpt-01-linux`.

**Acceptance:**
- `crates/adapter-linux/` implements all five UAPI modules.
- Sandbox root created on first run.
- Unit tests per module, integration tests run under `oneos run` on Ubuntu in CI.

### P2-ADPT-02 — macOS adapters

**Estimate:** 3 days.
**Branch:** `p2-adpt-02-macos`.

**Acceptance:**
- `crates/adapter-macos/` implements all five UAPI modules.
- Uses CoreFoundation for locale detection.
- CI green on macOS runners (both x86_64 and aarch64).

### P2-ADPT-03 — Windows adapters

**Estimate:** 3 days.
**Branch:** `p2-adpt-03-windows`.

**Acceptance:**
- `crates/adapter-windows/` implements all five UAPI modules.
- UTF-16 path conversion correct.
- CI green on windows-latest.

### P2-BIND-01 — Rust bindings

**Estimate:** 1 day.
**Branch:** `p2-bind-01-rust`.

**Acceptance:**
- `crates/bindings-rust/` (published as `oneos` on crates.io) re-exports each module.
- Generated via `wit-bindgen` with additional ergonomic wrappers.
- `cargo doc` generates clean reference.

### P2-BIND-02 — Go bindings

**Estimate:** 2 days.
**Branch:** `p2-bind-02-go`.

**Acceptance:**
- `oneos-go` repo with generated bindings.
- TinyGo build verified for a hello-world.
- Documented limitations of TinyGo stdlib.

### P2-BIND-03 — TypeScript bindings

**Estimate:** 2 days.
**Branch:** `p2-bind-03-ts`.

**Acceptance:**
- `@oneos/sdk` NPM package with type definitions.
- jco componentization pipeline documented.
- Hello-world TS sample < 10 MB as component.

### P2-APP-01 — `oneos-curl`

**Estimate:** 2 days.
**Branch:** `p2-app-01-curl`.

**Acceptance:**
- Source in `apps/oneos-curl/` (Rust).
- Behaves as §11.1.
- CI runs against test HTTP server, diffs stdout across hosts.

### P2-APP-02 — `oneos-cat`

**Estimate:** 1 day.
**Branch:** `p2-app-02-cat`.

**Acceptance:**
- Source in `apps/oneos-cat/` (Go via TinyGo).
- Behaves as §11.2.
- Cross-host stdout-identical against fixtures.

### P2-APP-03 — `oneos-clock`

**Estimate:** 1 day.
**Branch:** `p2-app-03-clock`.

**Acceptance:**
- Source in `apps/oneos-clock/` (TypeScript via jco).
- Behaves as §11.3.
- Snapshot tested with `--test-time` flag for determinism.

### P2-TEST-01 — Integration harness

**Estimate:** 2 days.
**Branch:** `p2-test-01-harness`.

**Acceptance:**
- `test/integration/runner.rs` builds each sample, runs via `oneos`, diffs stdout.
- Fixtures checked into `test/fixtures/`.
- Test HTTP server (simple hyper-based) for `oneos-curl` tests.
- CI runs on all three hosts.

### P2-SEC-01 — UCap soft enforcement

**Estimate:** 3 days.
**Branch:** `p2-sec-01-ucap`.

**Acceptance:**
- `crates/policy/` implements cap parsing, matching, and checking.
- Manifest parser in `crates/manifest/`.
- Dispatcher calls `policy.check()` at every UAPI entry.
- CLI flags `--grant`, `--auto-grant`, interactive prompt.

### P2-DOC-01 — WIT style guide

**Estimate:** 1 day.
**Branch:** `p2-doc-01-wit-style`.

**Acceptance:**
- `docs/book/src/wit-style.md`.
- Covers: naming, resource-vs-function, error variants, versioning.
- Linked from CONTRIBUTING.md.

### P2-DOC-02 — UAPI reference

**Estimate:** 2 days.
**Branch:** `p2-doc-02-uapi-ref`.

**Acceptance:**
- Auto-generated from WIT files via `wit-bindgen` markdown output (or custom tool).
- Included in mdBook site under `docs/book/src/reference/uapi/`.
- Regenerated in CI on every WIT change.

---

## 14. Code Skeletons

### 14.1 Runtime dispatcher pattern

```rust
// crates/runtime/src/uapi/fs.rs
use crate::policy::Cap;
use crate::adapter::HostAdapter;

pub fn wire_fs<A: HostAdapter + 'static>(
    linker: &mut wasmtime::component::Linker<HostState<A>>,
) -> anyhow::Result<()> {
    let mut fs = linker.instance("oneos:fs/files@0.1.0")?;

    fs.func_wrap(
        "open",
        |mut store: wasmtime::StoreContextMut<HostState<A>>,
         (path, mode): (String, OpenMode)|
         -> Result<(Resource<File>,), FsError> {
            let canonical = canonicalize(&store.data().sandbox_root, &path)?;
            let cap = match mode {
                OpenMode::Read => Cap::FsRead(canonical.clone()),
                _ => Cap::FsWrite(canonical.clone()),
            };
            store.data().policy.check(&cap).map_err(|_| FsError::PermissionDenied)?;

            let file = store.data().adapter.fs().open(&canonical, mode)?;
            let res = store.data_mut().resources.push(file)?;
            Ok((res,))
        },
    )?;

    // …open, stat, list, remove-file, remove-dir, mkdir, rename
    Ok(())
}
```

### 14.2 Linux `FsAdapter` implementation

```rust
// crates/adapter-linux/src/fs.rs
use std::fs;
use std::path::{Path, PathBuf};

pub struct LinuxFsAdapter;

impl FsAdapter for LinuxFsAdapter {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn File>, FsError> {
        let mut opts = fs::OpenOptions::new();
        match mode {
            OpenMode::Read       => { opts.read(true); }
            OpenMode::Write      => { opts.write(true).create(true).truncate(true); }
            OpenMode::ReadWrite  => { opts.read(true).write(true).create(true).truncate(true); }
            OpenMode::Append     => { opts.append(true).create(true); }
        }
        let f = opts.open(path).map_err(map_io_error)?;
        Ok(Box::new(LinuxFile(f)))
    }

    fn stat(&self, path: &Path) -> Result<FileStat, FsError> {
        let md = fs::metadata(path).map_err(map_io_error)?;
        Ok(FileStat {
            size: md.len(),
            modified_millis: md.modified()
                .map_err(map_io_error)?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            is_dir: md.is_dir(),
        })
    }

    fn list(&self, path: &Path) -> Result<Vec<String>, FsError> {
        let entries = fs::read_dir(path).map_err(map_io_error)?;
        let mut out = Vec::new();
        for entry in entries {
            let e = entry.map_err(map_io_error)?;
            let name = e.file_name()
                .into_string()
                .map_err(|_| FsError::InvalidPath)?;
            out.push(name);
        }
        out.sort();  // cross-platform determinism
        Ok(out)
    }

    // …remove_file, remove_dir, mkdir, rename
}

fn map_io_error(e: std::io::Error) -> FsError {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound         => FsError::NotFound,
        PermissionDenied => FsError::PermissionDenied,
        AlreadyExists    => FsError::AlreadyExists,
        _                => FsError::Io(e.to_string()),
    }
}
```

### 14.3 HTTP client skeleton (shared across hosts)

```rust
// crates/adapter-common/src/net.rs
use hyper::{Body, Client, Request as HyperRequest, Method};
use hyper_rustls::HttpsConnectorBuilder;
use tokio::runtime::Handle;

pub struct SharedNetAdapter {
    client: Client<hyper_rustls::HttpsConnector<hyper::client::HttpConnector>, Body>,
    rt: Handle,
}

impl NetAdapter for SharedNetAdapter {
    fn fetch(&self, req: Request) -> Result<Response, NetError> {
        let hreq = build_hyper_request(req)?;
        let fut = self.client.request(hreq);

        let resp = self.rt.block_on(async {
            tokio::time::timeout(
                std::time::Duration::from_millis(req.timeout_millis.unwrap_or(30_000) as u64),
                fut,
            )
            .await
        })
        .map_err(|_| NetError::Timeout)?
        .map_err(|e| NetError::ConnectFailure(e.to_string()))?;

        read_full_response(resp, self.rt.clone())
    }
}
```

### 14.4 UCap policy engine

```rust
// crates/policy/src/lib.rs
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum Cap {
    IoStdout,
    IoStderr,
    IoLog,
    IoStdin,
    FsRead(String),      // canonicalized path pattern
    FsWrite(String),
    FsList(String),
    FsRemove(String),
    FsMkdir(String),
    NetConnect { host: String, port: u16 },
    TimeClock,
    TimeMonotonic,
    TimeSleep,
    LocaleInfo,
    LocaleFormat,
}

#[derive(Debug, Error)]
#[error("capability denied: {0:?}")]
pub struct CapDenied(pub Cap);

#[derive(Default)]
pub struct Policy {
    grants: Vec<Cap>,
}

impl Policy {
    pub fn grant(&mut self, cap: Cap) {
        self.grants.push(cap);
    }

    pub fn check(&self, requested: &Cap) -> Result<(), CapDenied> {
        if self.grants.iter().any(|g| cap_covers(g, requested)) {
            Ok(())
        } else {
            Err(CapDenied(requested.clone()))
        }
    }
}

fn cap_covers(granted: &Cap, requested: &Cap) -> bool {
    // Exact match for non-resource caps; glob match for fs/net.
    // Full implementation in crates/policy/src/matcher.rs
    todo!()
}
```

### 14.5 Manifest parser

```rust
// crates/manifest/src/lib.rs
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub app: AppMeta,
    #[serde(default)]
    pub capabilities: Vec<CapabilityDecl>,
}

#[derive(Debug, Deserialize)]
pub struct AppMeta {
    pub id: String,
    pub name: String,
    pub version: String,
    pub entry: String,
    pub world: String,
}

#[derive(Debug, Deserialize)]
pub struct CapabilityDecl {
    pub cap: String,
    pub rationale: String,
    #[serde(default)]
    pub required: bool,
}

impl Manifest {
    pub fn parse(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    pub fn load_sibling(wasm_path: &std::path::Path)
        -> std::io::Result<Option<Self>>
    {
        let path = wasm_path.with_file_name("manifest.toml");
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        Self::parse(&text)
            .map(Some)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}
```

### 14.6 Interactive grant prompt

```rust
// crates/cli/src/prompt.rs
use oneos_manifest::CapabilityDecl;
use oneos_policy::Cap;
use std::io::{self, Write};

pub fn interactive_grant(caps: &[CapabilityDecl], app_id: &str)
    -> io::Result<Vec<Cap>>
{
    println!("App: {app_id}");
    println!("Requests the following capabilities:");
    for (i, c) in caps.iter().enumerate() {
        println!("  [{}] {}\n      {:?}", i + 1, c.cap, c.rationale);
    }
    print!("Grant [A]ll / [N]one / per-cap numbers / [S]kip (exit): ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    parse_grant_response(input.trim(), caps)
}
```

### 14.7 Rust sample: `oneos-curl` excerpt

```rust
// apps/oneos-curl/src/main.rs
use oneos::net::http_client::{fetch, HttpMethod, Request};

fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let url = args.get(1).cloned().unwrap_or_else(|| {
        eprintln!("usage: oneos-curl <url>");
        std::process::exit(1);
    });

    let req = Request {
        method: HttpMethod::Get,
        url,
        headers: vec![],
        body: vec![],
        timeout_millis: Some(30_000),
    };

    match fetch(req) {
        Ok(resp) => {
            oneos::io::stdio::stdout().write_all(&resp.body).ok();
            if resp.status >= 400 { 22 } else { 0 }
        }
        Err(e) => {
            eprintln!("error: {e:?}");
            1
        }
    }
}
```

### 14.8 Go sample: `oneos-cat` excerpt

```go
// apps/oneos-cat/main.go
package main

import (
    "os"

    fs   "github.com/oneos/oneos-go/fs/files"
    stdio "github.com/oneos/oneos-go/io/stdio"
)

func main() {
    args := os.Args[1:]
    if len(args) == 0 {
        os.Exit(1)
    }
    out := stdio.Stdout()
    defer out.Drop()

    for _, path := range args {
        f, err := fs.Open(path, fs.OpenModeRead())
        if err != nil {
            os.Exit(2)
        }
        for {
            chunk, err := f.Read(8192)
            if err != nil || len(chunk) == 0 {
                break
            }
            out.WriteAll(chunk)
        }
        f.Drop()
    }
}
```

### 14.9 TypeScript sample: `oneos-clock` excerpt

```typescript
// apps/oneos-clock/src/main.ts
import { current, timezone } from 'oneos:locale/info@0.1.0';
import { formatDate, DateStyle } from 'oneos:locale/format@0.1.0';
import { nowMillis } from 'oneos:time/clock@0.1.0';
import { stdout } from 'oneos:io/stdio@0.1.0';

export function run(): number {
    const loc = current();
    const tz = timezone();
    const now = nowMillis();
    const formatted = formatDate(now, tz, DateStyle.Medium, loc);
    const out = stdout();
    const encoder = new TextEncoder();
    out.writeAll(encoder.encode(formatted + '\n'));
    return 0;
}
```

---

## 15. Testing Strategy

### 15.1 Test pyramid (Phase 2 additions)

| Level | Tool | What's new in Phase 2 |
|---|---|---|
| Unit | `cargo test` | Per-adapter modules, manifest parser, policy matcher |
| Component | `wasmtime` harness | Single UAPI module integration tests |
| Integration | runner | Full sample apps across hosts |
| Cross-host diff | runner | Byte-identical stdout across Linux/macOS/Windows |
| Fuzz | `cargo-fuzz` | Manifest parser, capability matcher, path canonicalizer |

### 15.2 Fixture strategy

`test/fixtures/` contains:
- Deterministic text files for `oneos-cat`.
- Checked-in HTTP response bodies, replayed via the test HTTP server.
- Frozen-clock test data (`--test-time` flag outputs known strings).

### 15.3 Differential testing

Cross-host diff tests take the form:

```rust
#[test]
fn oneos_cat_identical_across_hosts() {
    let output_linux = run_on_host("linux", "oneos-cat fixtures/hello.txt");
    let output_macos = run_on_host("macos", "oneos-cat fixtures/hello.txt");
    let output_windows = run_on_host("windows", "oneos-cat fixtures/hello.txt");
    assert_eq!(output_linux, output_macos);
    assert_eq!(output_linux, output_windows);
}
```

These run in a CI matrix job that collects each host's output as an artifact, then a separate job downloads and diffs them.

### 15.4 Fuzz targets

| Target | Input | What we're looking for |
|---|---|---|
| `manifest_parser` | arbitrary bytes | panics, infinite loops |
| `cap_matcher` | pair of cap strings | false grants/denies |
| `path_canonicalize` | arbitrary paths | sandbox escapes via `..`, symlinks, null bytes |

Phase 2 fuzz runs: 4 hours nightly minimum.

### 15.5 Snapshot testing for CLIs

Each sample's `--help` output snapshotted via `insta`. Errors surface on help-text drift — catches regressions in clap config and argument docs.

---

## 16. Performance Targets

| Metric | Target | Measured how |
|---|---|---|
| UAPI dispatch overhead (hot) | < 1 µs | `criterion` microbench |
| UCap check overhead (cached grant) | < 500 ns | microbench |
| `oneos run` cold start w/ UAPI app | < 150 ms | `hyperfine` |
| `oneos run` warm start | < 30 ms | `hyperfine` |
| `oneos-curl` vs native `curl` (GET localhost) | < 3× overhead | comparison bench |
| `oneos-cat` vs native `cat` (1 MB file) | < 2× overhead | comparison bench |
| Runtime binary size | < 50 MB (up from 30 in Phase 1, due to adapters) | artifact size |
| RSS per running app | < 60 MB | `ps`/`Get-Process`/`Activity Monitor` |

Misses > 10% from any target trigger an issue blocking exit criteria.

---

## 17. Security & Threat Model v0.2

### 17.1 What v0.2 adds over v0.1

| v0.1 (Phase 1) | v0.2 (Phase 2) |
|---|---|
| One boundary: WASM ↔ runtime | Plus: WASM ↔ UCap, UCap ↔ adapter, adapter ↔ host |
| No filesystem access | Sandboxed filesystem; cap-scoped |
| No network access | Cap-scoped outbound HTTP |
| No concept of grants | Session-scoped grants |
| No manifest | Manifest is a trust-boundary input |

### 17.2 Updated STRIDE table

| Category | Threat | v0.2 mitigation |
|---|---|---|
| **S**poofing | Fake manifest claiming wrong app ID | Manifest signing deferred to Phase 6; Phase 2 accepts unsigned; CLI warns "unsigned app" |
| **T**ampering | Modify capability list post-grant | Grants are held in trusted Policy; WASM cannot modify |
| **R**epudiation | Deny capability was granted | `--log-grants` writes each grant to a log file; full audit DB is Phase 6 |
| **I**nformation Disclosure | Read files outside sandbox | Path canonicalization rejects `..` escapes; symlink-following disabled across sandbox boundary |
| **D**enial of Service | Infinite HTTP request, huge file | Timeouts on net; fuel/memory limits still available |
| **E**levation | Use HTTP to reach localhost when granted `net.connect:api.com:443` | Cap matching is strict on host AND port; localhost requires explicit separate grant |

### 17.3 New attack surfaces

- **Path canonicalization.** Classic source of sandbox escape. Extensive fuzzing mandatory. Also run `https://github.com/google/honggfuzz` if budget allows.
- **TLS verification.** rustls defaults are strict; we do not expose "ignore cert" toggle in v0.1.
- **DNS resolution.** Resolver uses system resolver by default. `hickory-dns` under evaluation for a controlled-resolver future.
- **Time-of-check / time-of-use (TOCTOU).** Policy is checked at `open`, but long-lived file handles persist. We do not yet handle "grant revoked while file is open"; Phase 6 will.

### 17.4 Explicitly out of scope (still)

- Persistent policy DB (Phase 6).
- Revocation (Phase 6).
- Code signing (Phase 6).
- System-UI grant prompts (Phase 3 for desktop, Phase 4 for mobile).
- Rate limiting / quotas (Phase 5+).

### 17.5 README update required

`SECURITY.md` should be updated at end of Phase 2 to:

> OneOS is pre-alpha. Phase 2 introduces capability-based sandboxing, but sandboxes are defense-in-depth — not a substitute for trusting the apps you run. Do not run malicious WASM expecting the sandbox to save you. Real hardening arrives in Phase 6 with signed bundles and persistent policy.

---

## 18. Documentation Deliverables

Phase 2 ships five user-facing docs. Priority order:

### 18.1 WIT Style Guide

`docs/book/src/wit-style.md`. Covers:
- Package naming: `oneos:<module>@<semver>`.
- Interface organization: `types` / functional interfaces / consumer `world`.
- Error variants: always `variant`, always include an `other(string)` bucket as last option.
- Resource vs function: resources for anything with lifecycle; functions for stateless operations.
- Versioning policy and stability markers.

### 18.2 UAPI Reference

Auto-generated from WIT. Published under `docs/book/src/reference/uapi/`. Rebuilt in CI on every merge to main that touches `wit/`.

### 18.3 Tutorial: "Your first UAPI app in Rust"

Step-by-step walkthrough: install `cargo-component`, define `world`, write Rust, compile, run, add a capability, run again.

### 18.4 Tutorial: "Your first UAPI app in Go / TypeScript"

Two more tutorials, one per non-Rust binding. Shorter than the Rust one (which is canonical).

### 18.5 Migration note: Phase 1 → Phase 2

`docs/book/src/phase2/migrating-from-phase1.md`. Tells anyone who was experimenting with Phase 1's `print`/`exit` imports how to move to real UAPI. Even if the audience is small (you), the doc discipline matters.

---

## 19. Architecture Decision Records

Expected ADRs in Phase 2:

| ID | Title | Target week |
|---|---|---|
| 0006 | Sync UAPI ABI over async runtime | W1 |
| 0007 | Filesystem path representation (UTF-8 + normalization) | W1 |
| 0008 | Time representation (Unix millis + timezone) | W2 |
| 0009 | Per-app sandbox root | W5 |
| 0010 | HTTP client stack (hyper + rustls) | W7 |
| 0011 | UCap grant model v0.1 (session-scoped, terminal UI) | W9 |
| 0012 | Adapter crate split (per-OS, not cfg-gated) | W3 |

Additional ADRs as decisions surface. Rule of thumb: if you have to ask "should I write an ADR?" the answer is yes.

---

## 20. Exit Criteria Checklist

### WIT & UAPI
- [ ] All five WIT files merged and frozen at v0.1.0.
- [ ] `wasm-tools` validates each.
- [ ] WIT style guide merged.
- [ ] UAPI reference auto-generation working in CI.

### Adapters
- [ ] Linux adapter implements all five modules; unit tests green.
- [ ] macOS adapter implements all five modules; unit tests green.
- [ ] Windows adapter implements all five modules; unit tests green.
- [ ] `crates/adapter-common/` shared logic passes on all three.

### Bindings
- [ ] `oneos` Rust crate published to crates.io (or ready to publish).
- [ ] `oneos-go` Go module; TinyGo build verified.
- [ ] `@oneos/sdk` TypeScript package; jco pipeline documented.

### Samples
- [ ] `oneos-curl` runs on all three hosts, cross-host stdout identical for fixture URLs.
- [ ] `oneos-cat` runs on all three hosts, cross-host stdout identical for fixture files.
- [ ] `oneos-clock` runs on all three hosts, snapshot-tested with frozen clock.

### UCap
- [ ] Manifest parser handles all §10.5 fields.
- [ ] Policy engine enforces caps at every UAPI entry.
- [ ] `--grant`, `--auto-grant`, interactive prompt all work.
- [ ] Attempt to open a file outside granted glob → clear error, exit code 5.

### CI & Quality
- [ ] Cross-host CI matrix green for ≥ 7 consecutive days.
- [ ] Fuzz targets defined; nightly run for ≥ 4 h succeeds without crash.
- [ ] Benchmark regressions ≤ 10% vs Phase 1 baseline.
- [ ] `cargo-deny` still clean with new deps.

### Documentation
- [ ] WIT style guide published.
- [ ] UAPI reference published.
- [ ] Three tutorials published (one per language).
- [ ] Migration note from Phase 1 published.
- [ ] Threat model v0.2 published.

### ADRs
- [ ] ADR-0006 through ADR-0012 merged (7 ADRs).

### External validation
- [ ] One external developer has built and run a UAPI app via tutorial in < 30 min.
- [ ] Retrospective written.
- [ ] Phase 3 kickoff issue opened with link to Phase 3 plan doc.

---

## 21. Phase 2 Risks

### 21.1 Technical risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| WIT + Component Model breaking changes land during Phase 2 | Medium | High | Pin versions in `Cargo.lock`. Upgrade deliberately at phase boundary. |
| TinyGo's component-model support is incomplete for our WITs | Medium | Medium | Prototype Go binding in Week 3. If blocker, degrade Go to "experimental" for Phase 2, defer to Phase 5. |
| jco-produced components are 10+ MB | High | Medium | Acknowledge and document. TS is valued for DX, not size. Bundle-size optimization is Phase 7. |
| Path canonicalization has sandbox-escape edge cases | High | Critical | Fuzz aggressively. Port tests from well-trodden capability systems (sandboxie, chromium sandbox docs). |
| Async-over-sync UAPI pattern causes deadlocks | Medium | High | Architectural RFC in Week 1 (ADR-0006); single-threaded Tokio per store enforced. |
| HTTP client memory blowup on large responses | Medium | Medium | Hard cap on response body size (16 MB v0.1). Streaming is v0.2 of `net`. |
| Cross-host stdout differs subtly (line endings, locale formatting) | High | Medium | Test fixtures explicit about encoding; LF-only output; `oneos-clock` tests use `--test-time` for determinism. |

### 21.2 Process risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Scope creep — "let's add one more UAPI module" | High | High | Every UAPI beyond the five is explicitly a Phase 3+ decision. Deferred list is a living doc. |
| WIT interfaces designed in isolation from sample apps | Medium | High | Sample apps built in parallel — if a WIT makes a sample awkward, the WIT is wrong. |
| Testing infrastructure under-built; bugs caught late | Medium | High | Integration harness in Week 1–2, not "later." Tests-first discipline. |
| Adapter implementations diverge in behavior | High | High | Shared trait defined once in `adapter-common`; cross-host diff tests catch drift. |
| Founder time eaten by ParkSure | High | Critical | Phase 2 is the longest so far. Protect 2 full days/week minimum. Extend calendar if needed — don't skip tasks. |

### 21.3 Tripwires

Stop and reassess if:
- Week 6 and `fs` is not working on all three hosts.
- Week 9 and any sample app fails the cross-host stdout-identical test.
- Binary size exceeds 80 MB.
- Cold start exceeds 300 ms on canonical machine.
- More than 2 ADRs deferred "I'll write this later" for more than 2 weeks.

The last one is the most common failure mode in real projects — document debt accumulates invisibly.

---

## 22. Handoff to Phase 3

### 22.1 What Phase 3 inherits

- Five stable UAPI modules (`io`, `fs`, `net`, `time`, `locale`).
- Three host adapter crates with clean shared-logic/platform-specific split.
- Three language bindings.
- UCap policy engine (extensible to GUI prompts in Phase 3).
- Manifest parser (extensible to UI manifests).
- Integration test harness ready to take on GUI tests.
- Sample-app pattern for stress-testing new WIT surface.

### 22.2 What Phase 3 builds on top of

- `ui.wit` (new) — imports `gfx.wit` (new) for low-level drawing.
- New adapter traits `UiAdapter`, `GfxAdapter`.
- System-UI grant dialogs (supersedes terminal prompts for GUI apps).
- `audio.wit` likely, `storage.wit` likely.

### 22.3 What Phase 3 must NOT touch

- v0.1 WIT files — frozen. If a change is needed, publish `@0.2.0` side-by-side.
- Adapter trait signatures for v0.1 modules.
- Manifest schema (additive only).
- UCap capability string format.

### 22.4 Lessons-learned capture for Phase 3

Before kicking off Phase 3, update the main Build Plan with anything the Phase 2 retrospective surfaces. Common candidates:
- WIT patterns that worked / didn't.
- Adapter structure adjustments.
- Testing harness gaps.
- Benchmark methodology improvements.

---

## 23. Appendices

### Appendix A — Complete WIT package index

At end of Phase 2, `wit/oneos/` contains:

```
wit/oneos/
├── io.wit           # 0.1.0
├── fs.wit           # 0.1.0
├── net.wit          # 0.1.0
├── time.wit         # 0.1.0
├── locale.wit       # 0.1.0
└── app.wit          # 0.1.0 (consolidated world)
```

Deleted at end of Phase 2:
- `wit/oneos/phase1.wit` — removed; apps migrated to real UAPI.

### Appendix B — Capability quick reference

```
Auto-granted (no prompt):
  io.stdout
  io.stderr
  io.log
  time.clock
  time.monotonic
  time.sleep
  locale.info
  locale.format

Requires explicit grant:
  io.stdin
  fs.read:<path-glob>
  fs.write:<path-glob>
  fs.list:<path-glob>
  fs.remove:<path-glob>
  fs.mkdir:<path-glob>
  net.connect:<host>:<port>
```

### Appendix C — Commands cheat sheet (Phase 2 additions)

```bash
# Validate WIT
wasm-tools component wit wit/oneos/fs.wit

# Generate Rust bindings
cargo component bindings

# Build the Rust sample
cd apps/oneos-curl && cargo component build --release

# Build the Go sample
cd apps/oneos-cat && tinygo build -target=wasi-p2 -o oneos-cat.wasm ./main.go

# Build the TypeScript sample
cd apps/oneos-clock && npm run build:component

# Run with specific grants
oneos run --grant net.connect:api.example.com:443,fs.read:./data/** app.wasm

# Auto-grant everything in manifest
oneos run --auto-grant app.wasm

# Interactive mode (default)
oneos run app.wasm

# Cross-host integration test locally
cargo test -p integration-runner --release -- --test-threads 1
```

### Appendix D — Debugging UAPI calls

When a UAPI call fails unexpectedly:

1. Enable UAPI tracing:
   ```bash
   ONEOS_LOG=oneos_runtime::uapi=trace oneos run app.wasm
   ```
2. Enable policy tracing:
   ```bash
   ONEOS_LOG=oneos_policy=trace oneos run app.wasm
   ```
3. Dump effective capabilities for a session:
   ```bash
   oneos run --dump-caps app.wasm
   ```
4. Validate the component's imports against the runtime's expected world:
   ```bash
   wasm-tools component wit app.wasm
   ```

### Appendix E — Retrospective template

Save as `docs/book/src/phase2/retro.md` at the end of Phase 2.

```markdown
# Phase 2 Retrospective

**Planned:** 12 weeks / **Actual:** <X> weeks
**Written:** YYYY-MM-DD
**Author:** @handle

## What shipped
- …

## What didn't ship and why
- …

## WIT design lessons
- Things that worked …
- Things we'd redo …

## Adapter lessons
- Per-OS surprises …

## Binding lessons
- Rust: …
- Go / TinyGo: …
- TypeScript / jco: …

## UCap lessons
- Prompts UX …
- Policy engine edge cases …

## Performance surprises
- …

## Concrete changes to the main Build Plan
- …

## Concrete changes to the Phase 3 plan before we start it
- …
```

---

## Closing

Phase 2 is where OneOS stops being a runtime and starts being a platform. The five WIT files you ship this phase will be cited, copied, and depended on by every OneOS app ever written. The adapter crates will be forked and ported for every new host. The UCap model will be extended, not replaced, for a decade.

Spend the time. Review the WIT aloud with someone who hasn't seen it. Write the tutorials before you think they're ready. Run the cross-host diff test until you trust it. The cost of getting Phase 2 right is three months. The cost of getting it wrong is every subsequent phase paying interest on that debt.

When you ship, `oneos-curl`, `oneos-cat`, and `oneos-clock` running byte-identically on three operating systems is the moment OneOS stops being a plan and becomes a thing. That moment is worth earning.

— end of document —
