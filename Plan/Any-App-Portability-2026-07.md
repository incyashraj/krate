# Krate Any App Portability

**Status:** Technical audit and implementation plan  
**Date:** 2026-07-30  
**Scope:** The work required to make Krate a credible target for existing and newly created applications, while preserving one `.krate` file, three desktop operating systems, and enforceable user consent.

## 1. Executive verdict

Krate already proves a valuable core:

- one WebAssembly component can open on macOS, Windows, and Linux;
- the component imports a small, versioned Krate interface rather than ambient operating system APIs;
- a `.krate` file carries the component and its requested capabilities;
- the runtime decides which capabilities the app receives;
- AI can author a working Krate app from a request;
- the create pipeline builds, checks, packages, and tests the result.

This is a strong foundation, but it is not yet an arbitrary application platform.

The phrase **any app** must have an engineering definition:

> Krate should accept any application whose behavior can be expressed by a supported Krate portability profile, or whose source can be transformed to one, and it should explain unsupported behavior before attempting the port.

Krate cannot honestly or safely turn every opaque macOS, Windows, or Linux binary into one portable component. Native binaries contain operating system ABIs, framework assumptions, dynamic libraries, installation logic, and privileged behavior that cannot be reconstructed reliably from the binary. Wrapping three native binaries in one archive would create a larger installer, not a universal Krate app, and it would weaken the permission model.

The route to broad compatibility is therefore not one converter. It is a platform with several controlled lanes:

1. **Krate-native components** for the strongest portability and security.
2. **Standard WASI components** adapted to Krate capabilities.
3. **AI-assisted source ports** for existing repositories.
4. **Web application ports** for apps whose UI and behavior fit a restricted web profile.
5. **Declared host extensions** for important native behavior that Krate adds deliberately.

The first product to build is `krate port`. It should analyze a source repository, classify the app, inventory its operating system dependencies, map those dependencies to Krate capabilities, and produce a deterministic port plan. It must never promise a conversion before it knows the app's requirements.

### Implementation status on 30 July 2026

The first end-to-end source-port slice now exists in the development tree:

1. `krate port <source> --plan` performs a bounded, read-only scan and emits
   `krate.port.plan.v1` as text or JSON.
2. `--prepare <dir>` creates an isolated workspace containing the exact plan,
   an agent task, a credential-filtered read-only source snapshot, a compiling
   Krate candidate, and the guest contract.
3. `--agent claude --to <app.krate>` lets Claude Code transform only the
   candidate. The agent is pointed at the snapshot rather than the live
   project, and Krate re-analyzes the original source afterward as a second
   integrity check.
4. The pipeline builds the candidate and gives the agent the exact compiler,
   import, or manifest error for up to two bounded repair attempts.
5. It rejects imports outside `krate:*`, packs the bundle, runs it with all
   declared grants, and verifies that withholding a required grant refuses
   execution.
6. The workspace records behavior journeys, passed and unverified journey
   results, and content hashes for both the plan and final bundle. These records
   are designed to become inputs to Krate Cloud signing and distribution.
7. A lower-level `--author-cmd` seam lets other agents and CI exercise the same
   deterministic checks.
8. Microphone use is now recognized as a portable `audio.capture` capability
   rather than a generic hardware blocker.
9. The runtime checks microphone permission before reaching the operating
   system, opens the host's default input device through CPAL, and normalizes
   its sample rate, channel count, and sample format to the contract requested
   by the app.
10. The built-in author recognizes voice prompter requests and produces a
   native teleprompter with manual controls, visible listening state,
   microphone capture, and local word matching.
11. AI authoring is now kind-aware. A voice prompter request gives an external
    coding agent the working microphone starter and its real constraints
    instead of incorrectly steering every request toward a checklist.
12. The bundle can now carry a bounded, nested `assets/` tree. Packing rejects
    symlinks, unsafe paths, oversized files, excessive file counts, and
    decompression bombs. Opening extracts assets only below a temporary
    `assets/` root.
13. Both app worlds now import `krate:resources/assets@0.1.0`. Guests can read
    or list only the resources carried in their own bundle. This does not grant
    access to the user's filesystem, and traversal outside the extracted asset
    root is rejected.
14. The GUI world now has a local speech provider backed by whisper.cpp. Audio
    is streamed into a bounded host-owned utterance buffer, so the portable
    guest does not need a large allocation or gain filesystem or network
    access. The guest receives only a 0 to 100 match score.
15. Voice app creation provisions a pinned 75 MB English model, checks its
    SHA-256 digest, caches it once for the creator, and packages it as a
    read-only app asset. The resulting `.krate` file can recognize speech
    without a cloud service.

This is an AI-assisted source port, not proof that every existing application
is supported. The current pipeline refuses projects with detected blocking
behavior. It does not convert opaque binaries, and successful packaging still
requires behavioral comparison and the same-bundle test on macOS, Windows, and
Linux.

## 2. What exists today

### 2.1 Package

The current development `.krate` format is a ZIP archive with two required
root files and an optional portable asset tree:

```text
manifest.toml
code.wasm
assets/**
```

This is intentionally small and easy to inspect. Asset transport and the
read-only guest resource API now work with bounded reads. The bundle has no
multiple components, adapters, source provenance, software bill of materials,
signature, publisher identity, update metadata, or migration record.

### 2.2 Application worlds

The manifest accepts two worlds:

- `krate:app/cli@0.1.0`
- `krate:app/gui@0.2.0`

The CLI world covers arguments, standard streams, files, HTTP, clock, sleep, locale, and formatting.

The GUI world adds windows, a widget tree, events, dialogs, clipboard, menus,
drop zones, graphics, and audio interfaces. Microphone capture now has a native
runtime implementation behind `audio.capture`. Playback and several richer UI
operations remain partial or unsupported. A declared interface must not be
confused with a production-ready implementation on all three hosts.

### 2.3 Security model

The manifest declares capabilities such as:

```text
fs.read:./data/**
fs.write:./data/**
net.connect:api.example.com:443
ui.window:create
```

The policy layer resolves session grants. Host dispatch checks a capability before calling an adapter. Filesystem paths are normalized and sandbox-relative. Network destinations are matched against explicit host and port scopes.

This currently provides a meaningful reduction in authority. It does not yet provide:

- persistent user decisions;
- publisher trust or signatures;
- revocation;
- organization policy;
- a complete audit trail;
- adversarial hardening of every host adapter;
- isolation from vulnerabilities inside the Krate runtime itself.

The correct promise today is that Krate limits what an app receives and enforces the user's decision. It must not promise that every app is safe or bug-free.

### 2.4 Authoring

`krate create` currently:

1. prepares an embedded Rust SDK;
2. generates a known starter or lets Claude Code edit it;
3. builds a component with `cargo-component`;
4. rejects every non-`krate:*` import;
5. packs the component and manifest;
6. runs the result with all declared grants;
7. withholds one required grant and verifies a refusal.

This is real end-to-end authoring. Its present breadth is limited:

- the built-in generator has a checklist, word-frequency app, and
  microphone-aware voice prompter;
- the AI path begins from the closest maintained starter selected from the
  request or detected source capabilities;
- the AI gets up to two bounded repair attempts with the exact validation error;
- Rust is the supported authoring path;
- the app writes its capability manifest rather than having capabilities derived and checked against imports and behavior;
- automated verification checks the allow and deny results and records the
  remaining source behavior, persistence, and three-system journeys explicitly.

### 2.5 Runtime and hosts

The runtime uses Wasmtime's component model and a custom `krate:*` interface surface. It has memory and optional fuel controls.

The host implementations are uneven:

- macOS has the newest AppKit path for a subset of widgets;
- Windows and Linux primarily use a winit and softbuffer drawn path;
- Linux currently depends on X11 or XWayland rather than a complete native Wayland path;
- audio capture and local speech transcription are implemented, but playback,
  several graphics, clipboard, menu, and richer widget operations are partial
  or unsupported;
- HTTP is not yet a complete production networking stack;
- there is no process API, database API, keychain API, notification API, tray API, background service API, camera API, USB API, serial API, accessibility API, webview, dynamic linking, or native plugin API.

These are not reasons to abandon the architecture. They are the compatibility boundary that `krate port` must report accurately.

### 2.6 Voice prompter forcing function

The voice prompter separates three different product claims:

1. **Microphone capture:** working in the development tree. The app requests
   `audio.capture`, the user decides, and only bounded PCM reaches the guest.
2. **Voice activity:** working in the generated sample. The app shows when
   voice is present and identifies the end of a spoken segment.
3. **Word following:** implemented in the development tree. The runtime
   transcribes the bounded utterance locally and returns a word-overlap score
   for the line currently shown.

Word following is a mediated speech provider, not an ambient network call
hidden inside every app. The implemented boundary is:

```text
audio.capture
    |
    v
bounded PCM segment
    |
    v
speech.transcription provider
  bundled local model
    |
    v
timestamped text
    |
    v
teleprompter follows the matching words
```

The current provider stays local. It resolves the model only inside the
bundle's extracted asset root, validates the path and size, and performs
inference in the native runtime. Krate Cloud may later supply an optional
managed provider, but local execution does not depend on the Cloud.

The downloaded model is pinned by SHA-256 before it enters a bundle. General
per-asset hashes, publisher provenance, and a shared runtime model store remain
future work. A live microphone and real-model inference recording is still
required before calling the full voice journey externally proven.

## 3. What “any app” can mean

### 3.1 Compatibility classes

| Class | Example | Near-term result | Security |
| --- | --- | --- | --- |
| A | checklist, notes, local tracker, small utility | Direct Krate-native port | Strongest Krate capability boundary |
| B | WASI CLI tool, file transformer, HTTP client | Adapt standard WASI imports to Krate | Strong if every import is mediated |
| C | Rust, Go, Python, or JS source with portable business logic | AI rewrites the host boundary and UI | Strong after import and behavior verification |
| D | React or static web app | Restricted local web profile or UI translation | Depends on webview and bridge design |
| E | Electron or Tauri app | Reuse web UI, replace privileged bridge | Medium effort, good source-level opportunity |
| F | Native Swift, AppKit, WinUI, WPF, GTK, Qt app | AI-assisted source migration | High effort and framework-specific |
| G | Opaque `.app`, `.exe`, or ELF binary | Unsupported for true universal conversion | Cannot preserve the core promise |
| H | Driver, kernel tool, antivirus, hypervisor, game with native engine | Out of scope for the safe portable profile | Incompatible with the intended boundary |

### 3.2 Product language

Externally, Krate can work toward:

> Bring an app or describe one. Krate tells you what will port, converts the supported parts, and produces one permissioned file for Mac, Windows, and Linux.

The product must show three outcomes, not only success or failure:

- **Ready:** the source fits an existing profile and can be ported automatically.
- **Needs changes:** most behavior is portable, with a precise list of replacements.
- **Not supported yet:** Krate identifies the missing host capability or incompatible runtime dependency.

That honesty increases trust. A converter that silently removes behavior or produces an app that only opens is worse than no converter.

## 4. Target architecture

```text
Source repository or plain request
              |
              v
     Project discovery
 language, framework, entry points
              |
              v
    Dependency and API inventory
 files, network, UI, OS APIs, binaries
              |
              v
      Portability classifier
 profile, supported mappings, blockers
              |
              v
       Deterministic port plan
 capabilities, transforms, tests, risks
              |
              v
       AI transformation loop
 edit, build, inspect imports, repair
              |
              v
      Behavior verification
 journeys, allow/deny, persistence, restart
              |
              v
          `.krate` bundle
 component, assets, manifest, provenance
              |
              v
 macOS host     Windows host     Linux host
```

### 4.1 Project discovery

The analyzer should detect, at minimum:

- Rust through `Cargo.toml`;
- JavaScript and TypeScript through `package.json`;
- Python through `pyproject.toml`, `requirements.txt`, or `setup.py`;
- Go through `go.mod`;
- C and C++ through CMake, Meson, Make, and common project files;
- Swift and Xcode projects;
- .NET projects;
- Electron, Tauri, React, Vite, Next.js, Qt, GTK, WPF, WinUI, and AppKit indicators.

The discovery result must be data, not prose. Proposed schema:

```json
{
  "schema": "krate.port.plan.v1",
  "source": "/path/to/project",
  "languages": ["typescript"],
  "frameworks": ["electron", "react"],
  "entry_points": ["src/main.ts", "src/renderer.tsx"],
  "profile": "electron-source-port",
  "verdict": "needs-changes"
}
```

### 4.2 Dependency and API inventory

The analyzer must search code, configuration, lockfiles, and build scripts for:

- filesystem locations and expected data ownership;
- network endpoints and protocols;
- environment variables and credentials;
- process creation and shell commands;
- native libraries and dynamic linking;
- database engines;
- browser or webview APIs;
- windows, menus, dialogs, clipboard, drag and drop, and notifications;
- audio, graphics, camera, and hardware;
- background execution and auto-start;
- updater and installer behavior;
- platform conditionals such as `cfg(target_os)`, `#if os`, and runtime OS checks.

String matching is sufficient for the first analyzer, but mature language adapters should use parsers, compiler metadata, or language servers. Every finding needs:

- file and line;
- detected API or dependency;
- mapped Krate capability, if one exists;
- confidence;
- required transformation;
- blocking reason, if unsupported.

### 4.3 Portability profiles

A profile is a versioned contract between the analyzer, transformer, SDK, runtime, and hosts. Initial profiles:

#### `krate-cli-v1`

Files, arguments, standard streams, HTTP, time, and locale. No native UI.

#### `krate-desktop-basic-v1`

Window, text, text entry, button, checkbox, lists, basic layout, local files, HTTP, dialogs, and clipboard where host parity is certified.

#### `wasi-cli-v1`

Standard WASI CLI components whose imported interfaces can be adapted to Krate policy. This should begin with streams, filesystem, clocks, random, and HTTP only after each mapping is reviewed.

#### `web-local-v1`

Planned. Static web assets and a restricted bridge to declared Krate capabilities. No unrestricted Node.js or browser extension authority.

Profiles prevent a recurring mistake: claiming support because an interface or widget exists in WIT while one or more hosts still return unsupported.

### 4.4 Standard WASI lane

Krate currently rejects all `wasi:*` imports. This protects the custom capability boundary, but it also blocks a large existing component ecosystem and causes ordinary language runtimes to fail.

The better design is not to grant WASI directly. It is to adapt selected WASI interfaces to Krate policy:

```text
existing WASI component
        |
        v
WASI adapter component
        |
        v
Krate capability-checked host interfaces
```

The WebAssembly component model supports typed composition, so component imports can be satisfied by another component's exports. WASI capabilities are designed to be supplied, virtualized, or attenuated at instantiation. WASI-Virt demonstrates component-level virtualization of selected WASI APIs.

The first WASI lane should be narrow:

- CLI arguments;
- standard streams;
- clocks;
- random with a clearly defined policy;
- preopened filesystem directories derived from Krate grants;
- HTTP requests derived from `net.connect` grants.

It must reject sockets, process spawning, and any interface without a reviewed Krate mapping.

Primary references:

- [WebAssembly component composition](https://component-model.bytecodealliance.org/design/components.html)
- [WASI capabilities](https://github.com/WebAssembly/WASI/blob/main/docs/Capabilities.md)
- [WASI repository and Preview 2 interfaces](https://github.com/WebAssembly/WASI)
- [WASI-Virt](https://github.com/bytecodealliance/WASI-Virt)

### 4.5 AI source transformation

AI should operate inside a deterministic pipeline, not replace it.

Proposed loop:

1. Analyze the repository.
2. Generate a port plan.
3. Ask the user to accept planned behavior changes.
4. Create an isolated worktree or copy.
5. Give the model only the source, plan, current profile, SDK, and compiler diagnostics it needs.
6. Build.
7. Inspect every component import.
8. Compare declared capabilities with observed imports and test needs.
9. Run generated journeys.
10. Feed exact failures to a bounded repair loop.
11. Stop after a defined attempt limit and return the remaining blockers.
12. Produce the `.krate`, report, source diff, and verification evidence.

The model must never decide silently to remove a feature. Every removed, substituted, or deferred behavior belongs in the port report.

### 4.6 Bundle v2

Porting real apps requires more than `manifest.toml` and `code.wasm`. A compatible v2 can remain a ZIP while adding structured entries:

```text
manifest.toml
components/main.wasm
components/adapters/*.wasm
assets/**
meta/port-plan.json
meta/provenance.json
meta/sbom.cdx.json
meta/verification.json
signatures/**
```

Rules:

- v1 bundles remain readable.
- Every executable component is named in the manifest.
- Asset paths are normalized and size-limited.
- The bundle cannot declare capabilities broader than its signed manifest.
- Adapters are versioned and included in the software bill of materials.
- Provenance records the source commit, tools, model identifier if AI was used, and transformations.
- Cloud identity and signatures can be added without changing application code.

This is direct pre-work for Krate Cloud. The cloud can store immutable bundle hashes, manifests, verification records, publisher identities, channels, and update relationships.

## 5. Required capability work

The fastest route to useful apps is not to add every operating system API. It is to cover common small-app behavior completely.

### Priority 0: certify what already exists

- one parity table generated from tests, not maintained by hand;
- real widget and event journeys on all three hosts;
- HTTPS behavior and error handling;
- clipboard and dialogs on all hosts;
- file persistence and permission denial;
- keyboard shortcuts, selection, focus, IME, and accessibility basics;
- deterministic unsupported errors.

### Priority 1: common local apps

- application-scoped key-value storage;
- structured local database, likely SQLite behind a Krate interface rather than exposing a host file;
- secrets and credential storage;
- notifications;
- open URL;
- richer image and asset loading;
- better menus and keyboard shortcuts;
- background tasks with explicit limits.

### Priority 2: integration apps

- local HTTP server with explicit port consent;
- safe subprocess profiles, if added at all;
- file watching;
- system tray;
- OAuth callback handling;
- richer graphics and audio.

Each new capability needs the same work:

1. WIT contract;
2. manifest syntax;
3. policy matching;
4. dispatcher enforcement;
5. adapters on all three operating systems;
6. denial test before adapter execution;
7. positive and negative journey;
8. documentation and analyzer mapping.

## 6. Verification standard

A port is not complete because it compiles. It is complete when its declared journeys pass.

Every generated app should have a `krate.verify.v1` plan:

```json
{
  "journeys": [
    {
      "name": "create and persist an item",
      "steps": [
        "open",
        "grant required capabilities",
        "enter unique text",
        "save",
        "close",
        "reopen",
        "assert text is visible"
      ]
    },
    {
      "name": "deny storage",
      "steps": [
        "open",
        "deny fs.write",
        "assert app does not receive write access",
        "assert the user sees a clear explanation"
      ]
    }
  ]
}
```

CI must run:

- analyzer fixtures;
- port plan snapshots;
- component import checks;
- capability declaration checks;
- allow and deny tests;
- same bundle hash across host lanes when the build is meant to be identical;
- functional journeys on macOS, Windows, and Linux;
- bundle corruption and path traversal tests;
- migration tests for old bundles;
- performance budgets for startup, host calls, and package size.

## 7. Security decisions

### Never do these

- run a native installer during conversion;
- execute repository scripts during analysis;
- let an AI agent broaden permissions without showing the user;
- treat source text as trusted instructions;
- ship a component with unknown imports;
- bundle host binaries and call the result universally portable;
- claim that a signed app is safe;
- upload private source to Krate Cloud by default.

### Required isolation

Porting should eventually run in an isolated build worker with:

- no source-derived network access by default;
- a clean toolchain image;
- read-only source input;
- bounded CPU, memory, time, and output;
- dependency allowlists or recorded dependency fetches;
- secret scanning;
- complete build provenance.

Local porting must provide the same controls where practical.

## 8. Krate Cloud pre-work

Cloud should be a distribution and trust system before it is a remote execution system.

The runtime and porting work should produce the objects Cloud needs:

- content-addressed `.krate` bundle;
- normalized manifest;
- requested capability summary;
- publisher identity;
- source and build provenance;
- verification status by operating system;
- compatibility profile;
- release channel and update parent;
- vulnerability and revocation status.

The cloud flow:

```text
publish bundle
    |
verify structure, signature, imports, and declared profile
    |
run automated journeys on three operating systems
    |
store immutable bundle and evidence
    |
publish version under a verified identity
    |
runtime checks identity, hash, channel, and revocation before update
```

This makes Cloud valuable without moving private app data or normal execution away from the user's device.

## 9. Implementation sequence

### Slice 1: `krate port --plan`

Deliver:

- source project discovery;
- language and framework detection;
- platform dependency findings with file evidence;
- current capability mappings;
- supported, needs-changes, and unsupported verdicts;
- JSON schema `krate.port.plan.v1`;
- deterministic text output;
- fixture tests for Rust, Electron, Tauri, Python, Go, Swift, and .NET projects.

Exit gate:

> Running `krate port path --plan` never edits or executes the source and gives a useful, stable answer for every fixture.

### Slice 2: WASI CLI adapter proof

Deliver:

- a standard WASI component with no Krate-specific source changes;
- selected WASI imports composed or linked through a Krate adapter;
- filesystem, streams, clock, random, and HTTP mappings;
- capability denial before host behavior;
- the same adapted component on three operating systems.

Exit gate:

> A real third-party WASI CLI component runs inside Krate with no ambient authority and a generated manifest.

### Slice 3: `krate port` for Rust CLI repositories

Deliver:

- analyze;
- create a port branch or copy;
- transform host calls;
- build and repair;
- infer and reconcile capabilities;
- produce bundle, diff, plan, and verification record.

Exit gate:

> Three unrelated open source Rust CLI apps port without hand editing and preserve defined behavior.

### Slice 4: desktop basic source ports

Begin with app shapes rather than languages:

- notes and lists;
- local CRUD tools;
- file transformers;
- API clients;
- small dashboards;
- focused menu or window utilities where the required host capabilities exist.

Exit gate:

> At least ten unrelated repositories across two source ecosystems reach a working `.krate`, with every behavior difference disclosed.

### Slice 5: bundle v2 and Cloud publish contract

Deliver:

- assets;
- multiple components and adapters;
- provenance;
- verification evidence;
- SBOM;
- signature envelope;
- content hash and update relationship.

Exit gate:

> A bundle can be published, independently verified, downloaded, and opened without trusting the storage service.

## 10. Immediate repository defects found during audit

1. The audit found that `cargo test --workspace --all-targets` reached a stale startup benchmark and failed because the benchmark still imported `layer36:phase1/host@0.0.1`, while the current linker no longer installs that retired interface. Unit and integration tests before the benchmark passed. The stale Phase 1 benchmark has now been removed from the active benchmark group so the command exercises the current Phase 2 runtime benchmarks.
2. The audit found that runtime dispatch probed multiple worlds through instantiation failure rather than selecting the manifest's declared world first. Manifest-backed runs now dispatch directly to the declared CLI or GUI world. Automatic probing remains only for raw `.wasm` compatibility.
3. The audit found that the manifest's `AppWorld::is_runnable` reported only the CLI world as runnable even though the GUI path now runs. The stale method and the CLI workaround around it have now been removed. Manifest validation remains the single source of truth for supported worlds.
4. The author contract still needs more supported starter families, but the AI
   prompt is now kind-aware and preserves the checklist, word-frequency, or
   voice-prompter structure selected for the request.
5. Capability declarations are supplied by generated source rather than reconciled against a generated port plan.
6. The bundle carries normal application assets and exposes them through a
   dedicated read-only resource API. Streaming large assets, per-asset hashes,
   and provenance are not implemented yet.
7. Several public interfaces are broader than certified three-host implementation parity.

These should be corrected alongside, not confused with, new portability features.

## 11. Success metrics

The correct primary metric is not the number of languages or frameworks listed on a website.

Track:

- repositories analyzed;
- projects classified without unknowns;
- projects automatically ported;
- percentage of original declared journeys preserved;
- median manual changes per port;
- first-pass and repaired build success;
- unsupported findings that become new host capabilities;
- successful opens on each operating system;
- permission denials that occur before host access;
- time from source repository to verified `.krate`;
- external developers who complete a port without founder help.

The first meaningful target:

> 20 unrelated source repositories analyzed, 10 accepted into a supported profile, 5 converted to verified `.krate` files, and 3 completed by external developers.

## 12. Final decision

Krate should pursue broad application portability. It should not pursue arbitrary binary conversion.

The durable advantage is the combination of:

- a versioned portable application contract;
- a source-aware compatibility analyzer;
- AI-assisted migration with deterministic compiler and policy gates;
- one inspectable bundle;
- enforced capabilities;
- verified behavior across three operating systems;
- cloud identity, evidence, and distribution.

The next code change is Slice 1. A port plan is the common input required by AI conversion, capability expansion, support decisions, Cloud metadata, and honest user expectations.
