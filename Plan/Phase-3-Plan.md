# Krate — Phase 3 Detailed Plan: UI + Graphics

> **Phase:** 3 of 8
> **Duration:** est. 6–10 weeks for the first desktop UI proof; later hardening depends on host findings.
> **Phase sentence:** *First GUI app running natively on Windows, macOS, and Linux.*
> **Prerequisite:** Phase 2 engineering proof green; final outside review and formal freeze still tracked separately.
> **Supersedes:** nothing.
> **Superseded by:** nothing.

---

## Table of Contents

0. [How to Use This Document](#0-how-to-use-this-document)
1. [Phase Objective](#1-phase-objective)
2. [Prerequisites from Phase 2](#2-prerequisites-from-phase-2)
3. [Success Criteria](#3-success-criteria)
4. [What Phase 3 Is and Is Not](#4-what-phase-3-is-and-is-not)
5. [The Core Design Question](#5-the-core-design-question)
6. [Architecture](#6-architecture)
7. [Technology Decisions](#7-technology-decisions)
8. [UAPI v0.2 Module Specifications](#8-uapi-v02-module-specifications)
9. [Widget Protocol Design](#9-widget-protocol-design)
10. [Layout Engine](#10-layout-engine)
11. [Rendering & 2D Canvas](#11-rendering--2d-canvas)
12. [3D GPU (WebGPU-Compatible Subset)](#12-3d-gpu-webgpu-compatible-subset)
13. [Input, Events, and Text](#13-input-events-and-text)
14. [Accessibility](#14-accessibility)
15. [Per-Host Adapters](#15-per-host-adapters)
16. [UCap v0.2 (System-UI Grants)](#16-ucap-v02-system-ui-grants)
17. [The `krate-notes` Flagship Sample](#17-the-krate-notes-flagship-sample)
18. [Week-by-Week Breakdown](#18-week-by-week-breakdown)
19. [Task Details](#19-task-details)
20. [Code Skeletons](#20-code-skeletons)
21. [Testing Strategy](#21-testing-strategy)
22. [Performance Targets](#22-performance-targets)
23. [Security & Threat Model v0.3](#23-security--threat-model-v03)
24. [Documentation Deliverables](#24-documentation-deliverables)
25. [Architecture Decision Records](#25-architecture-decision-records)
26. [Exit Criteria Checklist](#26-exit-criteria-checklist)
27. [Phase 3 Risks](#27-phase-3-risks)
28. [Handoff to Phase 4](#28-handoff-to-phase-4)
29. [Appendices](#29-appendices)

---

## 0. How to Use This Document

Phase 3 is the riskiest and most ambitious phase before v1.0. Every previous phase had clear prior art — Wasmtime for the runtime, WASI for the CLI UAPI. Phase 3 has no single prior art to copy from. The cross-platform native UI problem has been attempted many times; no previous attempt has been fully satisfactory. We are making a specific bet (§5) that is informed by those attempts but not a reprint of any of them.

- Read §5 (The Core Design Question) before any other section. Everything downstream is a consequence of that bet.
- The WIT in §8–§9 is this phase's most important artifact, in the same way Phase 2's WIT was its most important artifact.
- Tasks are listed in §19 with IDs matching Build Plan §7.4.
- If you find yourself asking "why not do it the Flutter way / Electron way / React Native way?" — §7.1 is where we answer that, in full.
- Phase 3 should not be rushed, but the estimate is now written as an estimate,
  not a fixed calendar promise. We will keep the first proof narrow.

---

## 1. Phase Objective

### 1.1 One-sentence objective

**A developer writes a note-taking app in Rust, Go, or TypeScript using UAPI v0.2, compiles to a `.wasm` component, and runs it via `krate run` on Windows, macOS, and Linux — where it opens a real window with native-feeling controls, handles keyboard/mouse/text input, persists files, renders 60 fps steady-state, and passes basic accessibility checks.**

### 1.2 Why this matters

Phase 2 proved the UAPI abstraction works for batch-style programs. Phase 3 proves it works for *interactive* programs — which is where the hard cross-platform problems live. Window management, text input (IME), accessibility trees, keyboard shortcuts, menu conventions, scroll physics, DPI scaling, color spaces: every single one differs between Windows, macOS, and Linux, and every single one has to feel correct to the user. Ship Phase 3 and the rest of Krate is largely implementation. Ship it wrong and Krate inherits "doesn't feel native" as a reputation that will take years to shake.

### 1.3 The six deliverables of Phase 3

1. **Three UI-related UAPI modules** — `ui`, `gfx`, `audio` — as frozen WIT v0.1.
2. **Widget protocol** that supports both native-backed widgets and custom-drawn fallbacks in one tree.
3. **Layout engine** (Taffy-based) with flexbox semantics.
4. **2D canvas** (vello on wgpu) and **3D GPU** (WebGPU-compatible subset via wgpu).
5. **`krate-notes` flagship** — a small but real note-taking app that demonstrates the entire UI stack.
6. **UCap v0.2** — system-UI grant dialogs replacing terminal prompts for GUI apps.

---

## 2. Prerequisites from Phase 2

Before touching Phase 3 implementation code, verify:

- [ ] All Phase 2 exit criteria met (Phase 2 Plan §20).
- [ ] UAPI v0.1 frozen: `io`, `fs`, `net`, `time`, `locale` modules untouchable.
- [x] Three host adapters (Linux/macOS/Windows) green for the current CLI surface in hosted full CI evidence.
- [x] Three language tracks (Rust/Go/TS) tracked by sample or fixture evidence, with Go runtime parity still marked experimental where needed.
- [x] UCap v0.1 enforcing grants at UAPI boundary for the current Phase 2 surface.
- [x] Cross-host sample and evidence compare harness green in the hosted full CI proof.
- [x] `krate-curl`, `krate-cat`, `krate-clock` covered by sample evidence.
- [ ] ADRs 0001 through 0012 merged.

Phase 3 is starting under a narrow waiver: WIT drafts and prototypes may begin
because the engineering proof is green, but Phase 2 is not formally closed until
the final outside review and freeze packet are recorded. Phase 3 work must not
change the Phase 2 UAPI.

---

## 3. Success Criteria

Phase 3 is **done** when, and only when, every row below is true.

| # | Criterion | Measured How |
|---|-----------|--------------|
| 1 | `ui`, `gfx`, `audio` WIT modules frozen at v0.1.0 | `wit/krate/*.wit` |
| 2 | Each module implemented in Linux, macOS, Windows adapters | CI green on all hosts |
| 3 | `krate-notes` runs on all three desktop OSes | Integration test |
| 4 | `krate-notes` UI feels native on each host (not Electron-style). On Linux this is measured against the drawn-widget rubric (scroll physics, focus, shortcuts, dark mode, DPI) per ADR-0015, not GTK widget identity | Qualitative test, documented rubric |
| 5 | Steady-state frame time ≤ 16.7 ms on 2020+ hardware | Frame-time histogram |
| 6 | Cold start for GUI app < 300 ms to first paint | Timestamp diff |
| 7 | IME (CJK input) works on all three hosts | Manual test + automated event capture |
| 8 | Screen reader reads `krate-notes` correctly on all three hosts | VoiceOver / Narrator / Orca |
| 9 | UCap v0.2: system-UI grant dialog fires for GUI apps | Manual walkthrough |
| 10 | 60 fps 10,000-node widget tree benchmark passes | Frame-time benchmark |
| 11 | DPI scaling: 100%, 125%, 150%, 200% all render correctly | Snapshot test |
| 12 | Dark mode: follows system preference, switches live | Manual + snapshot |
| 13 | Runtime binary size < 80 MB (up from 50 MB in Phase 2) | Artifact size |
| 14 | Per-app RSS < 120 MB for `krate-notes` | Process monitor |
| 15 | ADRs 0013 through at least 0020 merged | Git log |

Rows 4, 7, 8 are the **hardest to measure and most likely to be skipped**. They have explicit sub-criteria in §26.

---

## 4. What Phase 3 Is and Is Not

### 4.1 Phase 3 IS

- Desktop GUI. Windows, buttons, text fields, lists, menus.
- Native-backed widgets (real `NSButton`, real `<button>`, real GTK button) wherever possible.
- Custom drawing for widgets the host doesn't have.
- Flexbox-style layout.
- 2D vector rendering via `vello`.
- 3D GPU via `wgpu` / WebGPU subset.
- Keyboard, mouse, text input, IME.
- Clipboard, drag-drop (basic).
- Menus, keyboard shortcuts, tooltips.
- Dark/light mode following system.
- DPI awareness.
- Accessibility tree integration.
- Local audio playback + microphone capture.
- System-UI grant dialogs (superset of Phase 2 terminal prompts).

### 4.2 Phase 3 is NOT

- Not mobile. iOS/Android is Phase 4.
- Not web/browser. A plausible WASM-in-browser host is Phase 5+.
- Not a design system or theme library. We ship unopinionated primitives; product designers paint on top.
- Not a game engine. The 3D API is WebGPU-compatible, not Vulkan/DX12/Metal-direct.
- Not VR/AR/XR. Someday, not this year.
- Not video codecs beyond what the OS provides by default.
- Not streaming media (no WebRTC). Deferred indefinitely.
- Not WASM-in-browser. Browsers ARE a host, but hosting WASM-in-browser is a Phase 5+ concern.
- Not inter-app IPC. Deferred to Phase 5.
- Not notifications, tray icons, or background services (partial: notifications get a minimal interface, deferred in earnest to Phase 4 when mobile forces it).

### 4.3 The discipline

Phase 3 is a tire-fire of temptations. "Can we also add tray icons? Themes? Plugins? A component library?" Every yes adds a week. The answer is "Phase N" — except for the rare case where the primitive in question is genuinely prerequisite to `krate-notes` working. If you can build `krate-notes` without it, defer it.

---

## 5. The Core Design Question

### 5.1 The question

When a developer's Krate app says "I want a button," what does that button actually become on the user's screen?

### 5.2 Four possible answers

Every cross-platform UI framework has picked one of these:

**Answer A — "Draw everything ourselves."**
*Who does it:* Flutter, Slint, Dear ImGui, most game UIs.
*Upside:* Pixel-identical across OSes. Full control. No host drift.
*Downside:* Nothing is truly native. No matter how good your drawing, the scroll physics, keyboard shortcuts, selection behavior, right-click menus, and animations will drift from what the user expects. Accessibility is reinvented from zero.

**Answer B — "Call the host's native UI directly."**
*Who does it:* Qt (partially), Avalonia (partially), AppKit-on-macOS bridges.
*Upside:* Actually native. Users feel at home.
*Downside:* Huge surface. Every widget-per-platform matrix cell must be implemented. API differences leak into your abstraction. Some widgets have no equivalent on some platforms.

**Answer C — "Embed a browser."**
*Who does it:* Electron, Tauri (webview), CEF.
*Upside:* Web developers feel at home. Vast component ecosystem.
*Downside:* Not native. Large binaries. Battery-hungry. Infinite surface area. "It's a web app" is the common user reaction.

**Answer D — "Native widget tree with custom-drawn fallback for what the host doesn't have."**
*Who does it:* Partial inspirations exist (SwiftUI's platform lowering, React Native's Fabric), but no full production example.
*Upside:* Native feel where native widgets exist; unified experience where they don't.
*Downside:* Two code paths. The hybrid boundary is where bugs live.

### 5.3 Our bet

**We pick D, with explicit rules for when to use which path.**

The widget protocol is abstract: `Stack`, `Button`, `Text`, `Input`, `List`, etc. Each widget has a priority order for its rendering:

1. If the host has a native widget that matches *semantically*, use it (a Button becomes `NSButton` / `<button>` / GtkButton).
2. Otherwise, custom-draw using vello on our 2D canvas.

The abstract layer is the same everywhere. The concrete rendering differs. Our job is to pick the set of abstract widgets carefully enough that native versions exist on most hosts.

### 5.4 Why D despite its cost

- Electron's "doesn't feel native" is a decade-long developer complaint. It is not solvable within Answer C.
- Flutter's drift is subtle but accumulates. On macOS especially, Flutter apps always feel *slightly* off — scrollbar behavior, menu bar semantics, window chrome.
- Answer B is too expensive for a small team and too rigid for the long tail of custom widgets apps will need.
- Answer D accepts complexity at the framework layer to give users nativeness they can feel. This is the right trade for a platform intended to outlive individual app genres.

### 5.5 Consequence: the widget taxonomy rule

Every widget we add to the protocol must pass the **"native three of five"** test: at least three of { Windows, macOS, Linux, iOS, Android } have a native control that can render it. If fewer, we draw it ourselves and document why.

This rule keeps the protocol honest. It is checked in ADR review and in the WIT style guide.

Clarification (2026-07, ADR-0015): the "native three of five" test governs *protocol inclusion* — which widgets exist in the WIT. Per-host *lowering* may still be drawn where the host backend cannot support native embedding: in v0.1 Linux lowers every widget through the drawn fallback because GTK4 widgets cannot live inside winit windows.

### 5.6 Recorded in

ADR-0013 contains this full reasoning. It is the foundational architectural decision for Phase 3 and must be written in Week 1 before any implementation work.

---

## 6. Architecture

### 6.1 System overview at end of Phase 3

```mermaid
flowchart TB
    subgraph App["App WASM"]
        A1["Widget construction"]
        A2["Event handlers"]
        A3["Business logic"]
        A1 --> A2 --> A3
    end

    subgraph RT["Krate Runtime"]
        DISP["UAPI dispatcher"]
        UI["ui subsystem"]
        GFX["gfx subsystem"]
        RECON["Reconciler"]
        LAY["Layout engine (Taffy)"]
        VEL["2D canvas (vello + wgpu)"]
        A11Y["Accessibility tree"]
        UI --> RECON --> LAY
        GFX --> VEL
        UI --> A11Y
    end

    subgraph HA["Host Adapters"]
        MAC["macOS: AppKit + Metal"]
        WIN["Windows: Win32/XAML + D3D12"]
        LIN["Linux: GTK4 + Vulkan"]
    end

    App -- UAPI calls --> DISP
    DISP --> UI
    DISP --> GFX
    UI --> HA
    VEL --> HA
    A11Y --> HA
```

### 6.2 The "abstract widget → native-or-drawn" flow

```mermaid
flowchart TD
    W([App creates Button node]) --> C{"Host has native<br/>equivalent?"}
    C -- yes --> N[Create NSButton / button / GtkButton]
    C -- no --> D[Reserve layout slot]
    D --> DR[Custom-draw via vello]
    N --> L[Participate in layout]
    DR --> L
    L --> R[Render frame]
    R --> E([Event received])
    E --> M{Native or drawn?}
    M -- native --> NE[Host delivers event]
    M -- drawn --> DE[Hit-test against draw region]
    NE --> H[Call app handler]
    DE --> H
```

### 6.3 Frame loop

```mermaid
sequenceDiagram
    participant OS as Host OS
    participant RT as Runtime
    participant Rec as Reconciler
    participant Lay as Layout
    participant Ren as Renderer
    participant App as App WASM

    OS->>RT: vsync tick
    RT->>Rec: diff previous vs current widget tree
    Rec->>Lay: relayout dirty subtrees
    Lay->>Ren: produce draw list
    Ren->>OS: submit GPU commands
    OS-->>RT: input events (since last tick)
    RT->>App: dispatch events
    App->>App: update state
    App->>Rec: new widget tree (next frame)
```

### 6.4 Crate layout at end of Phase 3

```mermaid
flowchart LR
    CLI[krate-cli] --> RT[krate-runtime]
    RT --> UI[krate-ui]
    RT --> GFX[krate-gfx]
    RT --> AUDIO[krate-audio]
    UI --> LAY[krate-layout]
    UI --> RECON[krate-reconcile]
    UI --> A11Y[krate-a11y]
    GFX --> WGPU[wgpu]
    GFX --> VELLO[vello]
    RT --> AD_L[adapter-linux]
    RT --> AD_M[adapter-macos]
    RT --> AD_W[adapter-windows]
    AD_L --> GTK[gtk4-rs]
    AD_M --> COCOA[objc2 + AppKit bindings]
    AD_W --> WIN32[windows-rs]
```

### 6.5 Trust boundaries update

Same as Phase 2 plus:

- Clipboard content crossing WASM boundary is cap-gated.
- Drag-drop from outside the app is a system event that can only land with `ui.dropzone:*` cap.
- GPU shaders compiled from WASM-supplied source (in `gfx`) run in the host's GPU driver — a real attack surface, audited via WGSL validator.

---

## 7. Technology Decisions

### 7.1 The full "Answer D" rationale

See §5. Recorded in ADR-0013. Frozen.

### 7.2 Windowing: **`winit` under the hood on Linux and Windows, native on macOS**

- `winit` is the Rust ecosystem's cross-platform window/event crate.
- Excellent on X11, Wayland, Win32.
- macOS support works but is less idiomatic than talking to AppKit directly, and macOS users notice.
- **Decision:** use `winit` on Linux and Windows; use our own AppKit bridge on macOS. Shared `WindowAdapter` trait unifies.
- **Recorded:** ADR-0014.

### 7.3 Native widget bindings

| Host | Library | Notes |
|---|---|---|
| macOS | `objc2` + `objc2-app-kit` | Modern, maintained Rust Objective-C bridge |
| Windows | `windows-rs` (Win32 common controls as child HWNDs) | First-party Microsoft crate. XAML Islands deferred past v0.1. |
| Linux | `vello` drawn fallback — no native GTK lowering in v0.1 | GTK4 widgets cannot embed in winit windows (no foreign-window embedding). See ADR-0015. |

### 7.4 Layout: **Taffy**

- Rust-native flexbox layout engine.
- Used in production by Zed, Bevy, Dioxus.
- Fast (< 1 ms for 1000-node trees).
- **Recorded:** ADR-0015.

### 7.5 2D rendering: **vello on wgpu**

- Linebender's `vello`: compute-shader-based 2D vector renderer.
- Produces GPU-accelerated path rendering with quality matching CPU rasterizers.
- Targets Metal, Vulkan, D3D12 via `wgpu`.
- **Recorded:** ADR-0016.

### 7.6 3D: **wgpu / WebGPU subset**

- Expose a WebGPU-compatible subset in `krate:gfx/3d`.
- Shader language: **WGSL** (WebGPU Shading Language) only.
- No raw Vulkan / D3D12 / Metal API exposed to apps.
- **Recorded:** ADR-0017.

### 7.7 Text rendering & IME

- Text shaping: **`harfbuzz`** via `harfbuzz_rs`.
- Font selection: **`fontique`** (from Linebender).
- System font enumeration: per-host adapter.
- IME: per-host — `NSTextInputClient` on macOS, TSF on Windows, IBus/fcitx on Linux.
- **Recorded:** ADR-0018.

### 7.8 Accessibility

- Cross-platform tree: **`accesskit`** (Rust a11y protocol).
- Per-host: UIAutomation (Windows), NSAccessibility (macOS), AT-SPI (Linux).
- Screen readers target accesskit output.
- **Recorded:** ADR-0019.

### 7.9 Audio

- Playback / capture: **`cpal`** (cross-platform audio).
- Mixing: simple floating-point mixer in `krate-audio`.
- Decoder: **`symphonia`** for MP3/AAC/OGG/FLAC.
- No MIDI, no DAW features in v0.1.
- **Recorded:** ADR-0020.

### 7.10 Reactive / reconciliation model

- Retained-mode tree with diffing.
- App rebuilds a tree per frame (if desired) or emits mutations.
- Diff against previous tree → minimal native + drawn updates.
- Inspiration: Xilem, React Fiber, SwiftUI view identity.

### 7.11 What we considered and rejected

| Option | Rejected because |
|---|---|
| Qt | C++; licensing; huge surface to bind; not Rust-native. |
| wxWidgets | Dated API model; native-widget-only (our Answer B). |
| Iced | Pure custom-draw (our Answer A); drift risk too high. |
| egui | Immediate-mode; unsuitable for rich apps; no a11y. |
| Dioxus | JS-like but custom-draw underneath; same issue as Flutter. |
| WebView2 / WKWebView | Answer C; we explicitly rejected. |

### 7.12 What we DEFER

| Feature | Deferred to |
|---|---|
| Mobile-specific UI (touch gestures, sheet presentations, haptics) | Phase 4 |
| Cross-app IPC | Phase 5 |
| System tray icons / menu bar extras | Phase 5 |
| Complex media (video playback, WebRTC) | Phase 5+ |
| Plugin/extension API | Post-v1.0 |
| Theme engine / design tokens | Post-v1.0 |
| Animation timeline beyond CSS-transitions-level | Phase 5 |
| Printing | Phase 5 |
| Tray icons, file pickers backed by system UI | Phase 5 (file picker minimal in Phase 3 if needed) |

---

## 8. UAPI v0.2 Module Specifications

Phase 3 ships three new modules under `krate:` package. All three are versioned `@0.1.0` at module level (first release). They do not affect Phase 2's `@0.1.0` modules.

### 8.1 `krate:ui@0.1.0`

The widget protocol and windowing. Presented in full in §9. Here is the top-level structure:

```wit
// wit/krate/ui.wit
package krate:ui@0.1.0;

interface types {
    // see §9.1 and §9.2 for widget node enum, event types
}

interface window {
    use types.{window-config, window-id};
    create: func(cfg: window-config) -> result<window-id, ui-error>;
    close: func(id: window-id);
    set-title: func(id: window-id, title: string);
    // ...
}

interface tree {
    use types.{widget-node, widget-id};
    // Submit a full tree or a diff.
    submit: func(root: widget-node);
    mutate: func(ops: list<mutation>);
}

interface events {
    use types.{input-event};
    poll: func() -> option<input-event>;
    wait: func() -> input-event;
}

interface dialog {
    // System-provided dialogs for common needs
    open-file: func(cfg: file-dialog-config) -> result<list<string>, dialog-error>;
    save-file: func(cfg: file-dialog-config) -> result<string, dialog-error>;
    message: func(cfg: message-config);
}

interface clipboard {
    read-text:  func() -> result<string, ui-error>;
    write-text: func(text: string) -> result<_, ui-error>;
}

interface menu {
    set-app-menu: func(root: menu-node);
    set-window-menu: func(id: window-id, root: menu-node);
}

world consumer {
    import window;
    import tree;
    import events;
    import dialog;
    import clipboard;
    import menu;
}
```

### 8.2 `krate:gfx@0.1.0`

2D canvas + 3D GPU (WebGPU-compatible subset).

```wit
// wit/krate/gfx.wit
package krate:gfx@0.1.0;

interface canvas2d {
    use types.{color, rect, path, paint, gfx-error};

    resource canvas {
        fill-rect: func(r: rect, p: paint);
        stroke-rect: func(r: rect, p: paint, width: f32);
        fill-path: func(p: path, pt: paint);
        stroke-path: func(p: path, pt: paint, width: f32);
        draw-text: func(text: string, x: f32, y: f32, p: paint, font: font-spec);
        draw-image: func(img: image-handle, dst: rect);
        save: func();
        restore: func();
        translate: func(dx: f32, dy: f32);
        scale: func(sx: f32, sy: f32);
        rotate: func(radians: f32);
        clip-rect: func(r: rect);
    }
}

interface gpu3d {
    // WebGPU-compatible subset. WGSL shaders only.
    use types.{device, queue, shader-module, pipeline, buffer, texture, gpu-error};

    request-device: func() -> result<device, gpu-error>;
    // ... create-shader-module, create-pipeline, submit, etc.
}

world consumer {
    import canvas2d;
    import gpu3d;
}
```

### 8.3 `krate:audio@0.1.0`

Local playback + capture. Intentionally minimal; richer audio work is Phase 5+.

```wit
// wit/krate/audio.wit
package krate:audio@0.1.0;

interface playback {
    use types.{audio-buffer, playback-error};

    resource player {
        set-source: func(buf: audio-buffer) -> result<_, playback-error>;
        play: func();
        pause: func();
        stop: func();
        set-volume: func(v: f32);   // 0.0..1.0
        position-millis: func() -> u64;
    }

    create-player: func() -> result<player, playback-error>;
}

interface capture {
    use types.{capture-config, sample, capture-error};

    resource recorder {
        start: func() -> result<_, capture-error>;
        stop: func();
        read: func(n: u32) -> result<list<sample>, capture-error>;
    }

    create-recorder: func(cfg: capture-config) -> result<recorder, capture-error>;
}

world consumer {
    import playback;
    import capture;
}
```

### 8.4 Updated consolidated `world`

```wit
// wit/krate/app.wit
package krate:app@0.2.0;

world gui {
    // Phase 2 modules
    import krate:io/stdio@0.1.0;
    import krate:io/log@0.1.0;
    import krate:fs/files@0.1.0;
    import krate:net/http-client@0.1.0;
    import krate:time/clock@0.1.0;
    import krate:time/sleep@0.1.0;
    import krate:locale/info@0.1.0;
    import krate:locale/format@0.1.0;

    // Phase 3 modules
    import krate:ui/window@0.1.0;
    import krate:ui/tree@0.1.0;
    import krate:ui/events@0.1.0;
    import krate:ui/dialog@0.1.0;
    import krate:ui/clipboard@0.1.0;
    import krate:ui/menu@0.1.0;
    import krate:gfx/canvas2d@0.1.0;
    import krate:gfx/gpu3d@0.1.0;
    import krate:audio/playback@0.1.0;
    import krate:audio/capture@0.1.0;

    export run: func() -> s32;
}
```

Phase 2's `world cli` remains unchanged — GUI-less CLIs continue to work.

---

## 9. Widget Protocol Design

The core problem of Phase 3: what is the abstract widget tree that both "use native" and "draw ourselves" can render?

### 9.1 The v0.1 widget set

Fifteen widgets pass the "native three of five" test (§5.5). Others are custom-drawn.

**Containers:**
- `Stack` (vertical/horizontal layout box)
- `Grid` (2D grid layout)
- `Scroll` (scrollable viewport)
- `TabPanel`

**Controls:**
- `Button`
- `Checkbox`
- `Radio` (in `RadioGroup`)
- `Switch` (iOS-style toggle)
- `Slider`
- `ProgressBar`

**Text:**
- `Text` (read-only label)
- `TextField` (single-line input)
- `TextArea` (multi-line input)

**Collections:**
- `List` (virtualized vertical list)
- `Tree`

**Media:**
- `Image`

**Fallback (always custom-drawn):**
- `Canvas` — raw drawing surface; apps draw whatever they want via `gfx::canvas2d`.

### 9.2 Widget node WIT

```wit
// within wit/krate/ui.wit, in interface types

variant widget-node {
    stack(stack-props),
    grid(grid-props),
    scroll(scroll-props),
    tabs(tabs-props),
    button(button-props),
    checkbox(checkbox-props),
    radio(radio-props),
    switch(switch-props),
    slider(slider-props),
    progress(progress-props),
    text(text-props),
    text-field(text-field-props),
    text-area(text-area-props),
    list(list-props),
    tree(tree-props),
    image(image-props),
    canvas(canvas-props),
}

record stack-props {
    id: widget-id,
    direction: direction,
    children: list<widget-node>,
    spacing: f32,
    padding: edge-insets,
    alignment: alignment,
    style: style-props,
}

record button-props {
    id: widget-id,
    label: string,
    disabled: bool,
    style: style-props,
    // click events come through events.wait / events.poll
}

record text-field-props {
    id: widget-id,
    value: string,
    placeholder: string,
    password: bool,
    disabled: bool,
    max-length: option<u32>,
    style: style-props,
}

// ... one record per widget
```

### 9.3 Why submit a full tree, not a retained handle model

Developer experience: building a tree per frame is the mental model React, SwiftUI, Jetpack Compose, and Elm all popularized. It scales from small apps to large.

Implementation: the runtime diffs against the previous tree and emits minimal host/draw mutations. The reconciler is the one piece of code that has to be efficient.

### 9.4 Event plumbing

Events do NOT travel through the tree submission. They come through `events.poll()` / `events.wait()`:

```wit
variant input-event {
    window-closed(window-id),
    window-resized(window-id, u32, u32),
    window-focused(window-id, bool),
    key-down(window-id, key-event),
    key-up(window-id, key-event),
    text-input(window-id, string),
    ime-composition(window-id, ime-event),
    pointer-down(window-id, pointer-event),
    pointer-up(window-id, pointer-event),
    pointer-move(window-id, pointer-event),
    wheel(window-id, wheel-event),
    button-click(widget-id),
    checkbox-toggle(widget-id, bool),
    text-field-change(widget-id, string),
    list-select(widget-id, list<u32>),
    focus-change(widget-id),
    clipboard-paste(window-id),
    drop(window-id, drop-event),
    theme-change(theme),
    scale-change(f32),
}
```

Apps typically run:

```
loop {
    tree = build_tree(state)
    ui::tree::submit(tree)
    ev = ui::events::wait()
    state = handle_event(state, ev)
}
```

### 9.5 Style props v0.1

Minimum surface — resist adding more:

```wit
record style-props {
    background: option<color>,
    foreground: option<color>,
    border: option<border-spec>,
    corner-radius: option<f32>,
    font: option<font-spec>,
    padding: option<edge-insets>,
    // No margin — use Stack spacing.
    // No animation props — v0.1 is static.
}
```

Everything else is left to host defaults. Apps that want heavy styling use `Canvas`.

### 9.6 Widget identity & the reconciler

Each widget carries a `widget-id` that is stable across frames. Identity rules:

1. Apps MUST use stable IDs. The reconciler does not attempt content-based diffing; it uses ID-based match.
2. ID collision is a runtime error, logged as a warning in dev builds.
3. IDs are opaque `u64`; developers use binding helpers (`krate::ui::id("save-button")`) to derive them.

### 9.7 Hit testing and event routing

- Native widgets: host handles hit-testing.
- Drawn widgets and `Canvas`: runtime hit-tests against the draw region using the layout bounds.
- For `Canvas`, apps set an optional "opaque regions" list — rectangles that capture events they drew into.

---

## 10. Layout Engine

### 10.1 Choice recap

Taffy (ADR-0014). Flexbox-compatible.

### 10.2 Layout model

- Every widget node becomes a Taffy node.
- Styles translate to Taffy's flex properties.
- Layout runs on the reconciler thread, not the WASM thread.
- Result: per-widget computed rect in parent coordinates.

### 10.3 Layout passes

```mermaid
flowchart LR
    T[Widget tree] --> M[Measure pass]
    M --> L[Layout pass]
    L --> C[Compute clip rects]
    C --> R[Emit draw list]
```

### 10.4 Performance budget

- 10,000-node tree, full relayout: < 4 ms on 2020+ hardware.
- Incremental relayout (dirty subtree): < 500 µs.
- Budget is hard: exceeding blocks Phase 3 exit.

### 10.5 DPI and scale

- Layout units are logical pixels.
- A per-window scale factor is applied at render time.
- 1.0 = 96 DPI equivalent. 2.0 = Retina. 1.5 = Windows 150%.
- Scale changes fire `scale-change` events; apps may choose to re-layout (usually unnecessary — rendering handles it).

---

## 11. Rendering & 2D Canvas

### 11.1 The vello pipeline

```mermaid
flowchart LR
    DRAW[Draw list from reconciler] --> ENC[vello encode]
    ENC --> GPU[wgpu compute shaders]
    GPU --> SURFACE[Window surface]
```

### 11.2 Draw list

For each frame the runtime produces a draw list containing:
- Fill/stroke rects
- Paths
- Glyph runs (shaped by harfbuzz)
- Images
- Clip regions

The draw list is encoded into vello's format and dispatched to GPU.

### 11.3 Glyph pipeline

```mermaid
flowchart LR
    STR[Text + font] --> SHAPE[harfbuzz shape]
    SHAPE --> GL[Glyph IDs + positions]
    GL --> ATLAS[GPU glyph atlas]
    ATLAS --> VELLO[vello draw]
```

Glyph atlas is cached per font + size + DPI triple.

### 11.4 Color pipeline

- Internal color space: **linear sRGB**.
- Inputs from apps: **sRGB** (`color { r: 0..255, g: 0..255, b: 0..255, a: 0..255 }`).
- Conversion on ingress, rendering in linear, conversion at swapchain.
- Per-platform wide-gamut (Display P3 on macOS) is a v0.2 of `gfx`.

### 11.5 Custom-drawn widgets

For widgets the host doesn't render natively:
1. Layout produces a rect.
2. Runtime draws the widget using vello primitives matching the host's look.
3. Host-themed tokens (button fill, hover, focus ring) come from a per-host "theme pack" baked into the adapter.

This keeps drawn widgets visually closest to native without installing a full theme engine.

### 11.6 Offscreen canvases

Apps using `Canvas` widget get a `canvas` resource they draw to. Drawing is GPU-accelerated. Apps can also render to offscreen canvases for image export (`image-handle` consumable by other widgets).

---

## 12. 3D GPU (WebGPU-Compatible Subset)

### 12.1 Why WebGPU shape

- Cross-platform abstraction exists: wgpu.
- Developers targeting WebGPU in the browser can reuse most knowledge.
- WGSL is portable, validated, shader-safe.

### 12.2 What we expose

| WebGPU Feature | v0.1 Status |
|---|---|
| Device + queue | yes |
| Shader modules (WGSL) | yes |
| Render pipeline (vertex + fragment) | yes |
| Compute pipeline | yes |
| Buffers | yes |
| Textures (2D) | yes |
| Textures (3D, cube, array) | v0.2 |
| Samplers | yes |
| Bind groups | yes |
| Render passes | yes |
| Compute passes | yes |
| Mipmap generation | via compute shader |
| Timestamp queries | v0.2 |
| External surfaces (video frames) | no |
| Ray tracing | no |

### 12.3 Security

- WGSL is validated by wgpu's validator before compilation.
- Buffer bindings are type-checked.
- No raw SPIR-V / DXIL / MSL is exposed — developers can't side-step WGSL.
- GPU driver bugs remain a theoretical concern — the ecosystem-wide problem, not Krate-specific.

### 12.4 Scope discipline

`krate-notes` uses 2D only. The 3D surface exists to prove it *can* be done by Phase 3 exit and to unblock game-adjacent experiments. We do not ship 3D samples this phase.

---

## 13. Input, Events, and Text

### 13.1 Keyboard

- Physical key events + text events separate (standard model).
- Modifier state included.
- Per-OS key codes normalized to a common enum.
- Shortcuts (`Ctrl+S`, `Cmd+S`) dispatched as `key-down` events — apps interpret.

### 13.2 Mouse / pointer

- Unified pointer model (mouse / touchpad / pen).
- Pressure and tilt exposed where available (stylus).
- Hover events.
- Right-click / middle-click.
- Drag state tracked by runtime; apps receive start/move/end.

### 13.3 IME (Input Method Editor)

The single hardest input surface. Handling:

- Pre-edit (composition) text flows through `ime-composition` events.
- Apps are expected to render pre-edit text with system-style underline.
- Commit events deliver final text as `text-input`.
- Per-host:
  - macOS: `NSTextInputClient` protocol.
  - Windows: TSF (Text Services Framework).
  - Linux: IBus + fcitx support via virtual protocols.

Manual CJK testing on all three hosts is a hard exit criterion.

### 13.4 Text selection

- For native text fields, host handles selection.
- For drawn text (rare — `Canvas` mostly), app implements selection; runtime provides utilities.
- Copy/paste routed through `ui::clipboard`.

### 13.5 Focus

- Focus ring follows system convention.
- Tab order is widget-tree order unless app overrides via `focus-hint`.

---

## 14. Accessibility

### 14.1 Why first-class

Accessibility added late is accessibility never. We bake it in at Phase 3 or we spend years retrofitting.

### 14.2 Architecture

- `accesskit` builds a cross-platform accessibility tree.
- Each widget registers its a11y role, label, and state.
- Per-host adapters translate accesskit nodes to UIAutomation / NSAccessibility / AT-SPI.

### 14.3 v0.1 coverage

For each widget in §9.1, define:
- Role (button, text field, list item, etc.)
- Accessible label
- Value (if applicable)
- State (disabled, focused, checked)
- Relationship (label-for, described-by)

### 14.4 Reduced-motion, high-contrast, large-text

- System preferences queried via `krate:ui/preferences` (sub-interface to add in Phase 3 or 4; Phase 3 adds minimum).
- Runtime honours `prefers-reduced-motion` by suppressing animation.
- Apps query user preferences directly; runtime does not force.

### 14.5 Screen reader testing is mandatory

Every exit criterion run on:
- VoiceOver (macOS)
- Narrator (Windows)
- Orca (Linux)

Pass = screen reader announces all widgets correctly and can navigate the tree.

---

## 15. Per-Host Adapters

### 15.1 Linux

- Windowing: `winit`.
- Widgets: drawn fallback via vello, styled with per-host theme tokens (ADR-0015 — GTK4 widgets cannot embed in winit windows; native GTK lowering is out of v0.1 scope).
- Canvas backend: `wgpu` on Vulkan.
- Audio: `cpal` on PipeWire or PulseAudio.
- Text: harfbuzz.
- A11y: AT-SPI via accesskit-atspi.
- IME: IBus protocol via `winit` callbacks + custom bridge.

Known pain:
- Wayland vs X11 behavior differences.
- Font configuration varies by distro.
- Drawn widgets carry full responsibility for focus, keyboard, and a11y behavior — nothing is inherited from a native toolkit.

### 15.2 macOS

- Windowing: custom AppKit bridge via `objc2`.
- Native widgets: `NSButton`, `NSTextField`, `NSTableView`, etc.
- Canvas backend: `wgpu` on Metal.
- Audio: `cpal` on CoreAudio.
- Text: Core Text for shaping OR harfbuzz (we use harfbuzz for cross-host consistency).
- A11y: NSAccessibility via accesskit-mac.
- IME: NSTextInputClient.

Known pain:
- Menu bar semantics (macOS uses app-wide menu bar; other OSes use per-window).
- Window chrome conventions (traffic lights, full-screen behavior).
- Retina / non-Retina DPI handling.

### 15.3 Windows

- Windowing: `winit` with optional Win32 native for menus.
- Native widgets: Win32 common controls + XAML Islands where available.
- Canvas backend: `wgpu` on D3D12.
- Audio: `cpal` on WASAPI.
- Text: harfbuzz.
- A11y: UIAutomation via accesskit-windows.
- IME: TSF.

Known pain:
- HiDPI multi-monitor scale transitions.
- Touch vs precision pointer discrimination.
- XAML Islands support varies by Windows version.

### 15.4 Shared adapter trait

```rust
pub trait UiAdapter: Send + Sync {
    fn create_window(&self, cfg: WindowConfig) -> Result<WindowId, UiError>;
    fn close_window(&self, id: WindowId);
    fn create_widget(&self, id: WidgetId, node: &WidgetNode) -> WidgetHandle;
    fn update_widget(&self, handle: &WidgetHandle, node: &WidgetNode);
    fn destroy_widget(&self, handle: WidgetHandle);
    fn draw_custom(&self, handle: &WidgetHandle, draw_list: &DrawList);
    fn poll_event(&self) -> Option<InputEvent>;
    // ...
}
```

Identical trait across hosts; implementations differ entirely.

---

## 16. UCap v0.2 (System-UI Grants)

### 16.1 What changes

- Grant prompts now use system dialogs (not terminal).
- For sensitive caps (camera, microphone, screen recording, location), use OS-native prompts (macOS TCC, Windows privacy prompts) in addition to our own.
- Session grants persist to **in-memory** store — not disk (disk is Phase 6).

### 16.2 New capability strings added in Phase 3

```
ui.window:create
ui.clipboard:read
ui.clipboard:write
ui.menu:system
ui.dropzone:<mime-type>
ui.dialog:file-open
ui.dialog:file-save
gfx.gpu:basic         # 2D canvas + basic 3D
gfx.gpu:compute       # compute shaders
audio.playback
audio.capture
```

### 16.3 Default grants for Phase 3

Expanded auto-grant list:
- `ui.window:create`
- `ui.dialog:*`
- `gfx.gpu:basic`

Not auto-granted (prompt required):
- `ui.clipboard:*`
- `ui.dropzone:*`
- `gfx.gpu:compute`
- `audio.playback` (auto-grant on GUI apps; reconsider if abused)
- `audio.capture` (always prompt; double-confirm at OS level)

### 16.4 System prompt UX

```mermaid
flowchart TD
    A[App calls ui.clipboard.read] --> B{Cap granted?}
    B -- yes --> ALLOW[Return content]
    B -- no --> C[Runtime shows system dialog]
    C --> D{User choice}
    D -- "Allow Once" --> E[Grant for this call only]
    D -- "Allow for Session" --> F[Grant until krate exits]
    D -- "Never for this app" --> G[Record deny; suppress future prompts]
    D -- "Deny" --> DENY[Return error]
    E --> ALLOW
    F --> ALLOW
    G --> DENY
```

### 16.5 App-triggered prompts

Apps cannot summon grant dialogs at will. They can only trigger them by calling the UAPI that needs the cap. This prevents nagware patterns.

---

## 17. The `krate-notes` Flagship Sample

### 17.1 Why build a flagship

A flagship app is the project's best argument. If `krate-notes` feels good, developers believe the platform. If it feels weird, no amount of documentation convinces them.

### 17.2 Scope

A minimal note-taking app:
- Sidebar: list of notes with titles and preview text.
- Main area: editor for the selected note.
- Top bar: app menu (File/Edit/View).
- Saves to `~/Documents/KrateNotes/` (requires `fs.write` cap).
- Searchable by title.
- Keyboard shortcuts (`Cmd/Ctrl+N` new, `Cmd/Ctrl+S` save, `Cmd/Ctrl+F` find).
- Dark mode follows system.
- DPI-aware.
- Screen-reader-navigable.

Out of scope: rich-text, attachments, sync, encryption, export — all Phase N.

### 17.3 Language

Rust. First-class for the flagship; we eat our own canonical bindings.

### 17.4 LOC target

< 2,000 lines including all files. If the flagship balloons past this, the UAPI is too granular — fix the UAPI, not the app.

### 17.5 What the flagship proves

- Widget tree submission / reconciliation works.
- Native widgets render natively on all three hosts.
- Layout is correct across DPI scales.
- IME works in the note editor.
- Keyboard shortcuts route correctly.
- Dark mode switches live.
- Screen readers navigate correctly.
- UCap grant flow fires on first launch.
- Save/load works via `krate:fs`.
- 60 fps in typical editing sessions.

If all ten boxes check, Phase 3 shipped.

---

## 18. Week-by-Week Breakdown

Sized for 16 weeks calendar, ~60–80 engineering days of active work. A full-time engineer compresses to 10–12 weeks; a founder splitting time with ParkSure uses the full 16.

> **Renderer slice map (scoped 2026-07-04, next implementation work):** replace flat-color drawing with real 2D rendering behind the *same* placement contract — `draw_placements` keeps its signature; only the painter changes. (1) Evaluate `vello_cpu` (the CPU sparse-strip rasterizer from the vello project) + `parley` (text layout) rendering into the CPU pixel buffer that softbuffer already presents — this keeps CI free of GPU requirements (hosted runners have no usable Vulkan) while producing antialiased vector widgets and real typography; GPU vello lands later as a swap of the same painter. (2) Widgets gain rounded corners, borders, hover/pressed fills from per-host theme tokens (§11.5). (3) The 5x7 bitmap `drawtext` module stays as the zero-dependency fallback and test baseline. (4) Verification: existing unit-shape tests must not change (placement contract untouched); the Xvfb click proof and screenshot artifact prove the new painter visually; a pixel-sample test asserts text pixels are antialiased (multiple alpha levels), not bilevel. (5) Definition of done: hello-gui's Linux screenshot artifact shows antialiased labels and the synthetic click proof stays green.
>
> **Drawn text v0 (DONE 2026-07-04):** 5x7 bitmap font module `adapter-common/src/drawtext.rs` (95 printable-ASCII glyphs, integer scaling, per-pixel clipping) wired into `draw_placements` on Linux and Windows: button labels centered, field text left-inset, `Text` widgets render glyphs instead of blocks. Explicit placeholder until the renderer slice above.
>
> **Winit input-routing map (scoped 2026-07-04 — DONE same day, certified by full matrix run `28708285696`: xdotool click under Xvfb pressed the drawn button and hello-gui exited 0):** make the drawn widgets interactive on Linux and Windows. (1) In both `winit_native` modules, handle `WindowEvent::CursorMoved` (track the last logical cursor position per window, dividing physical by `scale_factor`) and `WindowEvent::MouseInput` (left button press/release). (2) Extend the shared `WinitWindowNativeEvent` in `adapter-common` with a `Pointer { x, y, pressed }` variant; the collector, session apply path, and pump already forward variants — the new arm queues `queue_pointer_event` with `widget: None` (adapters do not hit-test; layout lives runtime-side). (3) In `Phase3GuiHost::poll_one_event`, intercept `UiEvent::Pointer` events whose `widget` is `None`: re-route them through `dispatcher.route_pointer_event` with the window's record size as the viewport so hit testing assigns the widget id, then deliver the routed event and drop the raw one. hello-gui already reacts to `pointer.widget == Some(BUTTON_ID)`, so no guest change. (4) CI proof: in the Linux Xvfb step, after launching hello-gui with `--native-window` in the background, use `xdotool` (apt-install alongside xvfb, run under the same `xvfb-run` server via a wrapper script) to click the button's known coordinates (placements are deterministic: root padding 16 logical, button 160x32 at the top of the stack), then assert exit code 0 — the first fully machine-verified click round trip on Linux. Windows lane keeps exit-1 (no synthetic input tooling there yet). (5) Definition of done: hello-gui exits 0 under Xvfb from a synthetic click on the drawn button.

> **Winit slice map (scoped 2026-07-03 — DONE 2026-07-03/04: real winit windows on Linux and Windows in the full matrix, plus drawn-widget rectangles):** real Linux/Windows winit windows behind the existing prototype boundaries. (1) Add `winit 0.30` as a target-gated dependency of `adapter-linux` (features `x11`,`wayland`) and `adapter-windows`; macOS builds stay untouched. (2) Own the `EventLoop` thread-locally per process (mirror `APPKIT_PROTOTYPE_SESSIONS` in `adapter-macos`), created with `EventLoop::new()` + `EventLoopExtPumpEvents::pump_app_events(Some(Duration::ZERO), ..)` for the non-blocking tick — winit 0.30 drives an `ApplicationHandler`; implement it to push `WindowEvent`s (CloseRequested, Resized, Focused, ScaleFactorChanged, RedrawRequested) into the existing `WinitWindowEventCollector` as `WinitWindowNativeEvent`s, which the shared pump already drains. (3) Window creation via `event_loop.create_window(WindowAttributes)` inside the first pump; attach the handle through the existing `attach_winit_session` path. (4) Linux widgets: drawn fallback only per ADR-0015 — window + events first, drawing later; Windows native Win32 lowering comes after windows exist. (5) Verification loop: unit-shape tests headless; fast CI's ubuntu lane compiles the Linux path on every push; the full matrix compiles Windows; real window smokes run under `xvfb-run` on ubuntu (install in the full-test lane) and natively on the windows runner, both behind an env-gated ignored test like the AppKit smoke. (6) Definition of done for the slice: `krate run --native-window` opens a real window for hello-gui on a Linux host with events flowing (drawn widgets may still be absent — window + close/resize/focus round trip is the slice).

> **Amended 2026-07** (`Plan/Plan-Amendments-2026-07.md` A2): the next implementation milestone is **P3-VS-01** — the macOS vertical slice (one WASM component drives a real AppKit window with a native `NSButton` and `NSTextField` and receives the click back). It executes *before* Linux/Windows Winit window broadening, because it validates the core native-lowering bet end-to-end first. Winit work resumes after P3-VS-01 passes, following ADR-0015 for Linux widgets. The agent-embedding tasks P3-EMB-01..03 follow the slice as a parallel, non-exit-blocking track.

### Weeks 1–2: Foundational decisions + WIT draft

- Write ADR-0013 (widget lowering), ADR-0014 (Taffy layout), ADR-0015 (vello), ADR-0016 (WebGPU subset), ADR-0017 (harfbuzz + IME), ADR-0018 (accesskit), ADR-0019 (cpal), and the windowing ADR when the first native backend starts.
- Draft `ui.wit`, `gfx.wit`, `audio.wit`.
- Internal review; iterate.

### Weeks 3–4: Window + event loop

- Windowing trait + per-host implementations.
- Basic event loop: open window, receive close event, close.
- Smoke test: `krate run blank-window.wasm` opens a blank window on all three hosts.

### Weeks 5–6: Widget tree + reconciler

- Widget node WIT finalized.
- Reconciler: diff tree, emit mutations.
- Native widget creation for Button, Text, Stack (macOS first, for momentum).

### Weeks 7–8: Layout + more widgets

- Taffy integration.
- Remaining widgets: Checkbox, Radio, Switch, Slider, TextField, TextArea, List, Grid, Scroll, Image, Canvas.
- Windows + Linux widget parity with macOS.

### Weeks 9–10: 2D canvas + rendering

- vello integration.
- wgpu surface setup per host.
- Custom-drawn widget fallback path.
- Glyph pipeline + font enumeration.

### Week 11: Input + text

- Keyboard, mouse, pointer normalization.
- Text input + IME per host.
- CJK manual testing.

### Week 12: Accessibility

- accesskit integration.
- Per-host a11y tree.
- Screen reader validation on each host.

### Week 13: 3D GPU (minimal)

- `gpu3d` interface implementation.
- Small test: clear a texture to red from a WASM app.
- Enough to unblock future games/visualizations, not a full demo.

### Week 14: Audio + dialog + clipboard + menus

- `cpal` playback + capture.
- System dialogs (file open/save, message).
- Clipboard read/write.
- App + window menus.

### Week 15: `krate-notes`

- Build the flagship.
- This is where the UAPI gets stress-tested. Expect gaps — each gap is a lesson, not a failure.

### Week 16: Hardening + exit

- DPI scaling testing (100/125/150/200%).
- Dark/light mode switching.
- Performance tuning to hit §22 targets.
- Exit criteria walkthrough (§26).
- Retrospective.
- Phase 4 kickoff plan.

---

## 19. Task Details

Matches Build Plan §7.4 task IDs.

### P3-VS-01 — macOS vertical slice (native widget end-to-end) *(added 2026-07, amendment A2)*

**Estimate:** 8–10 days.
**Branch:** `p3-vs-01-macos-vertical-slice`.
**What:** A demo WASM component (new `apps/` or `examples/` entry) that opens a window through the Phase 3 dispatcher → the AppKit prototype creates the real `NSWindow` → the component submits a widget tree containing one `Button` and one `TextField` → the macOS adapter lowers them to a real `NSButton`/`NSTextField` placed by the Taffy layout → a physical click on the `NSButton` flows back through the delegate/event bridge → the component dequeues it via the event path and updates the label text.
**Builds on:** `AppKitWindowSession`, the delegate bridge, the draw view surface, the shared widget tree, and the `Phase3UiDispatcher` layout + event routes — all already landed.
**Out of scope:** reconciler diffing beyond naive re-submit; drawn-fallback rendering; any Linux/Windows work; styling beyond defaults.
**Acceptance:**
- A recorded local run (extend the `smoke-phase3-appkit-runtime.sh` pattern) shows the click round-trip.
- STATUS.md updated; this recorded run is the centerpiece of the public demo.
**Failure escalation:** if native lowering hits a structural wall (event routing, layout-to-AppKit coordinates, ownership), STOP and write the findings into an ADR draft before writing workaround code — this milestone exists to validate the bet, so discovering a flaw is a success condition of the task.

**Implementation map (scoped 2026-07-02 against the current code):**
1. *Native lowering* — extend `crates/adapter-macos/src/appkit.rs`: given the window's `WidgetTree` (from `UiAdapter::widget_tree`) and a `LayoutSnapshot` (from `Phase3UiDispatcher::compute_layout`), create `NSButton`/`NSTextField` subviews on the prototype window's content view. AppKit's origin is bottom-left — flip Y from the layout's top-left rects using the content view height. Store created views keyed by `WidgetId` on `AppKitWindowSession`.
2. *Click round-trip* — declare an Objective-C target object with an action selector using the same `declare_class!` pattern as the existing `AppKitWindowDelegate`; the action pushes into the existing `AppKitWindowDelegateQueue`-style FIFO, drained by `AppKitWindowEventLoopDriver::pump_session_once` into the shared event stream as a pointer/activation event carrying the `WidgetId`.
3. *Sub-slice order* — prove 1+2 Rust-side first via an extension of the `smoke-phase3-appkit-runtime.sh` opt-in pattern (this validates the whole bet without touching WASM); only then wire the WASM component path: minimal `krate:ui` host imports (window create/show, tree submit subset, events.poll) into the `krate run` linker for the `gui` world, and the demo component under `apps/`.
4. *Main-thread rule* — all AppKit widget creation stays on the main process thread, matching the existing `NSWindow` prototype gating.

### P3-EMB-01 — Runtime embedding API *(added 2026-07, amendment A3; parallel track, non-blocking for Phase 3 exit)*

**Estimate:** 4 days.
**Branch:** `p3-emb-01-embedding-api`.
**Acceptance:**
- A documented public API on `crates/runtime`: load component + manifest, supply grants programmatically (no interactive prompt), run, receive a structured result (exit class, stdout/stderr handles, per-capability grant/deny log).
- An external Rust program (doc-tested example) embeds Krate in under 30 lines with no interactive TTY.

### P3-EMB-02 — `krate run --json` *(added 2026-07, amendment A3)*

**Estimate:** 3 days.
**Branch:** `p3-emb-02-run-json`.
**Acceptance:**
- Machine-readable run output: run status, exit code + classification, capabilities requested/granted/denied (names + boundaries), timing.
- Output parses against a schema documented in the book for the success, permission-denied, and invalid-input paths of the three sample apps.

### P3-EMB-03 — MCP server wrapper *(added 2026-07, amendment A3)*

**Estimate:** 4 days.
**Branch:** `p3-emb-03-mcp-server`.
**Acceptance:**
- A minimal MCP server (a `tools/` binary) exposing `run_component` (artifact path/bytes, manifest, grants) backed by P3-EMB-01, returning P3-EMB-02-shaped results.
- An MCP-capable agent client can execute `krate-cat` with and without grants and observe the deny/allow difference.
- Scope bound: no agent orchestration, no model calls, no tool registry — Krate executes artifacts safely; the agent ecosystem does the rest.

### P3-UI-01 — Widget protocol design RFC

**Estimate:** 3 days.
**Branch:** `p3-ui-01-widget-rfc`.
**Acceptance:**
- RFC in `docs/rfc/0003-widget-protocol.md`.
- Reviewed by at least one external engineer with UI background.
- §9 of this doc as starting point.
- Merged into `docs/adr/` as ADR-0013 + ADR-0014 where decisions are final.

### P3-UI-02 — `wit/krate/ui.wit`

**Estimate:** 2 days.
**Branch:** `p3-ui-02-ui-wit`.
**Acceptance:**
- File complete, matching §8.1 + §9.2.
- Parses via `wasm-tools component wit`.
- Style guide updated if patterns diverge from Phase 2.

### P3-UI-03 — Layout engine (Taffy integration)

**Estimate:** 3 days.
**Branch:** `p3-ui-03-layout`.
**Acceptance:**
- `crates/layout/` wraps Taffy with WIT types.
- Unit tests: 100 layouts of varying tree shapes.
- Benchmark: 10k-node layout < 4 ms.

### P3-UI-04 — Window + event loop abstractions

**Estimate:** 3 days.
**Branch:** `p3-ui-04-window`.
**Acceptance:**
- `WindowAdapter` trait in `crates/adapter-common/`.
- Implementations started on all three hosts.
- Blank-window smoke test green.

### P3-UI-05 — macOS widget bridge

**Estimate:** 5 days.
**Branch:** `p3-ui-05-macos-widgets`.
**Acceptance:**
- `NSButton`, `NSTextField`, `NSTableView`, `NSStackView`, etc. for each Phase 3 widget.
- Drawn-fallback path working.
- Manual test: build a 3-widget screen, interact, observe native behavior.

### P3-UI-06 — Windows widget bridge

**Estimate:** 5 days.
**Branch:** `p3-ui-06-windows-widgets`.
**Acceptance:**
- Same widget set rendered via Win32 common controls / XAML Islands.
- Drawn-fallback path working.
- Manual test on Windows 11.

### P3-UI-07 — Linux drawn-widget backend

*(Redefined 2026-07 per ADR-0015 — GTK4 widgets cannot embed in winit windows, so Linux v0.1 uses the drawn fallback for every widget.)*

**Estimate:** 5 days.
**Branch:** `p3-ui-07-linux-widgets`.
**Acceptance:**
- Same widget set rendered through the vello drawn-fallback path inside winit windows.
- Per-host theme tokens applied for visual fit.
- Focus, keyboard, and accessibility metadata working without a native toolkit underneath.
- Manual test on Ubuntu 22.04 + Fedora.

### P3-GFX-01 — `wit/krate/gfx.wit`

**Estimate:** 2 days.
**Branch:** `p3-gfx-01-wit`.
**Acceptance:**
- File matches §8.2.
- Paths for 2D + 3D documented.

### P3-GFX-02 — wgpu integration

**Estimate:** 3 days.
**Branch:** `p3-gfx-02-wgpu`.
**Acceptance:**
- Device creation across Metal / D3D12 / Vulkan.
- Surface acquisition per window.
- Smoke test: clear surface to red.

### P3-GFX-03 — 2D canvas via vello

**Estimate:** 5 days.
**Branch:** `p3-gfx-03-vello`.
**Acceptance:**
- `canvas2d` interface implemented.
- Draws rects, paths, text, images.
- Smoke test: paint a gradient + text + circle, compare across hosts.

### P3-GFX-04 — 3D API subset

**Estimate:** 5 days.
**Branch:** `p3-gfx-04-gpu3d`.
**Acceptance:**
- Device, queue, shader module, pipeline, buffer, texture resources.
- WGSL validation.
- Smoke test: triangle rendered from WASM app.

### P3-INPUT-01 — Keyboard + mouse input

**Estimate:** 2 days.
**Branch:** `p3-input-01-kbmouse`.
**Acceptance:**
- Normalized keyboard events per host.
- Pointer events with pressure/tilt where available.
- Unit tests for key code mapping tables.

### P3-INPUT-02 — Text input + IME

**Estimate:** 3 days.
**Branch:** `p3-input-02-ime`.
**Acceptance:**
- Pre-edit + commit events routed per host.
- CJK manual test pass on all three hosts.
- Test plan in `docs/book/src/phase3/ime-testing.md`.

### P3-APP-01 — `krate-notes`

**Estimate:** 5 days.
**Branch:** `p3-app-01-notes`.
**Acceptance:**
- App in `apps/krate-notes/` (Rust).
- Implements scope per §17.2.
- Runs on all three hosts in CI.

### P3-A11Y-01 — Screen reader integration

**Estimate:** 5 days.
**Branch:** `p3-a11y-01-accesskit`.
**Acceptance:**
- accesskit tree built from widget tree.
- Per-host bridges into UIAutomation / NSAccessibility / AT-SPI.
- `krate-notes` reads correctly on all three screen readers.

### P3-TEST-01 — UI snapshot testing

**Estimate:** 3 days.
**Branch:** `p3-test-01-snapshots`.
**Acceptance:**
- Snapshot harness that renders a widget tree and compares against a reference image.
- Threshold-based diff (1% pixel variance tolerated).
- Snapshots per host stored under `test/snapshots/{linux,macos,windows}/`.

### P3-PERF-01 — Frame budget dashboard

**Estimate:** 2 days.
**Branch:** `p3-perf-01-dashboard`.
**Acceptance:**
- Frame-time histogram per sample app.
- CI publishes to `docs/book/src/phase3/perf/`.
- Regression check: > 10% frame-time increase fails PR.

### P3-DOC-01 — "Build a GUI app" tutorial

**Estimate:** 3 days.
**Branch:** `p3-doc-01-tutorial`.
**Acceptance:**
- Step-by-step in `docs/book/src/tutorial/build-gui.md`.
- Ends with a working mini-app (smaller than `krate-notes`).
- External reader test: < 45 min to working app.

---

## 20. Code Skeletons

### 20.1 Widget node in Rust bindings

```rust
// crates/bindings-rust/src/ui.rs

pub enum Widget {
    Stack {
        id: WidgetId,
        direction: Direction,
        children: Vec<Widget>,
        spacing: f32,
        padding: EdgeInsets,
        alignment: Alignment,
        style: Style,
    },
    Button {
        id: WidgetId,
        label: String,
        disabled: bool,
        style: Style,
    },
    Text {
        id: WidgetId,
        content: String,
        style: Style,
    },
    TextField {
        id: WidgetId,
        value: String,
        placeholder: String,
        password: bool,
        disabled: bool,
        max_length: Option<u32>,
        style: Style,
    },
    // ...
    Canvas {
        id: WidgetId,
        draw_fn: Box<dyn Fn(&mut CanvasContext)>,
        style: Style,
    },
}
```

### 20.2 The main app loop pattern

```rust
// in an app
use krate::ui::{Widget, Event, window, tree, events};

fn main() -> i32 {
    let win = window::create(WindowConfig {
        title: "My App".into(),
        width: 960,
        height: 640,
        ..Default::default()
    }).unwrap();

    let mut state = AppState::new();

    loop {
        let ui = build_ui(&state);
        tree::submit(ui);

        match events::wait() {
            Event::WindowClosed(_) => break,
            Event::ButtonClick(id) if id == krate::ui::id("save-btn") => {
                state.save();
            }
            Event::TextFieldChange(id, value) if id == krate::ui::id("editor") => {
                state.editor_value = value;
            }
            _ => {}
        }
    }
    0
}

fn build_ui(state: &AppState) -> Widget {
    Widget::Stack {
        id: krate::ui::id("root"),
        direction: Direction::Vertical,
        spacing: 8.0,
        padding: EdgeInsets::all(16.0),
        children: vec![
            Widget::TextField {
                id: krate::ui::id("editor"),
                value: state.editor_value.clone(),
                placeholder: "Type here".into(),
                ..Default::default()
            },
            Widget::Button {
                id: krate::ui::id("save-btn"),
                label: "Save".into(),
                disabled: false,
                style: Style::default(),
            },
        ],
        alignment: Alignment::Start,
        style: Style::default(),
    }
}
```

### 20.3 Reconciler skeleton

```rust
// crates/reconcile/src/lib.rs

pub enum Mutation {
    Create { parent: WidgetId, index: usize, node: Widget },
    Update { id: WidgetId, diff: WidgetDiff },
    Remove { id: WidgetId },
    Reorder { parent: WidgetId, new_order: Vec<WidgetId> },
}

pub fn diff(prev: &Widget, next: &Widget) -> Vec<Mutation> {
    let mut ops = Vec::new();
    diff_into(prev, next, &mut ops);
    ops
}

fn diff_into(prev: &Widget, next: &Widget, ops: &mut Vec<Mutation>) {
    // If IDs differ, remove + create.
    // If same ID but different variant, remove + create.
    // If same ID and variant, diff fields; recurse into children.
    // ...
}
```

### 20.4 macOS Button bridge excerpt

```rust
// crates/adapter-macos/src/widgets/button.rs

use objc2_app_kit::{NSButton, NSButtonTypeMomentaryPushIn};
use objc2_foundation::NSString;

pub struct MacButton {
    view: Retained<NSButton>,
    id: WidgetId,
}

impl MacButton {
    pub fn create(id: WidgetId, props: &ButtonProps) -> Self {
        unsafe {
            let title = NSString::from_str(&props.label);
            let btn = NSButton::buttonWithTitle_target_action(
                &title,
                None,
                None,
            );
            btn.setButtonType(NSButtonTypeMomentaryPushIn);
            btn.setEnabled(!props.disabled);
            Self { view: btn, id }
        }
    }

    pub fn update(&mut self, props: &ButtonProps) {
        unsafe {
            if &**self.view.title() != &*NSString::from_str(&props.label) {
                self.view.setTitle(&NSString::from_str(&props.label));
            }
            self.view.setEnabled(!props.disabled);
        }
    }
}

impl WidgetHandle for MacButton {
    fn id(&self) -> WidgetId { self.id }
    // ...
}
```

### 20.5 vello draw skeleton

```rust
// crates/gfx/src/canvas.rs
use vello::{Scene, SceneBuilder, kurbo::{Rect, Affine}, peniko::{Color, Fill}};

pub struct Canvas {
    scene: Scene,
}

impl Canvas {
    pub fn fill_rect(&mut self, rect: Rect, color: Color) {
        let mut sb = SceneBuilder::for_scene(&mut self.scene);
        sb.fill(Fill::NonZero, Affine::IDENTITY, color, None, &rect);
    }

    pub fn draw_text(&mut self, text: &str, x: f32, y: f32, font: &FontSpec, color: Color) {
        // shape via harfbuzz, produce glyph run, append to scene
        // ...
    }
}
```

### 20.6 accesskit bridge skeleton

```rust
// crates/a11y/src/lib.rs
use accesskit::{Node, NodeId, Role, Tree, TreeUpdate};

pub fn widget_to_a11y(widget: &Widget) -> Node {
    match widget {
        Widget::Button { id, label, disabled, .. } => {
            let mut node = Node::new(Role::Button);
            node.set_name(label.as_str());
            if *disabled { node.set_disabled(); }
            node
        }
        Widget::TextField { id, value, placeholder, .. } => {
            let mut node = Node::new(Role::TextInput);
            node.set_value(value.as_str());
            if value.is_empty() {
                node.set_placeholder(placeholder.as_str());
            }
            node
        }
        // ...
        _ => Node::new(Role::Unknown),
    }
}
```

---

## 21. Testing Strategy

### 21.1 What Phase 3 adds

| Level | Addition |
|---|---|
| Snapshot | Per-host widget renders compared against reference PNGs |
| E2E | Drive `krate-notes` via scripted input; assert UI state |
| Accessibility | Automated screen-reader simulation via accesskit trees |
| Performance | Frame-time histogram, layout time, reconcile time |
| IME | Manual test matrix per host with CJK inputs |

### 21.2 Snapshot strategy

- Rendered snapshots stored per-host (because native widgets differ).
- Each snapshot: 1% pixel variance tolerance (font AA differences).
- Separate "strict" snapshots for `Canvas`-only widgets that should be pixel-identical.

### 21.3 Screen reader testing

Manual on first release, scripted where possible:
- macOS: AppleScript driving VoiceOver announcements.
- Windows: UIA test automation.
- Linux: AT-SPI introspection script.

Test plan documented in `docs/book/src/phase3/a11y-testing.md`.

### 21.4 IME testing matrix

| Host | Input Method | Test sequence |
|---|---|---|
| macOS | Japanese (Hiragana) | Type "konnichiwa" → expect こんにちは after space |
| macOS | Chinese (Pinyin) | Type "nihao" → expect 你好 after selection |
| macOS | Korean (2-Set) | Type sequence → expect 한글 |
| Windows | MS Japanese IME | Same |
| Windows | MS Pinyin | Same |
| Windows | MS Korean | Same |
| Linux | IBus / fcitx Japanese | Same |
| Linux | IBus / fcitx Pinyin | Same |
| Linux | IBus / fcitx Korean | Same |

Ten-minute manual test per host per input method. Documented in `docs/book/src/phase3/ime-testing.md`. Result: green/yellow/red table with notes.

### 21.5 Performance tests

- 1k-node layout: < 2 ms.
- 10k-node layout: < 4 ms.
- Reconcile a 1k-node tree with 10% changed: < 1 ms.
- First paint after window creation: < 200 ms.
- `krate-notes` typing: 60 fps sustained.

---

## 22. Performance Targets

| Metric | Target | Measured how |
|---|---|---|
| Cold start GUI app to first paint | < 300 ms | Timestamped launch → first frame |
| Warm start | < 80 ms | Same |
| Steady-state frame time | ≤ 16.7 ms | Frame histogram |
| 1k-node layout | < 2 ms | Criterion bench |
| 10k-node layout | < 4 ms | Criterion bench |
| Reconcile 1k nodes, 10% changed | < 1 ms | Criterion bench |
| UAPI dispatch (UI) | < 1 µs | Microbench |
| `krate-notes` typing 60 fps sustained | 60 fps | Frame histogram |
| Runtime binary size | < 80 MB | Artifact size |
| `krate-notes` RSS | < 120 MB | Process monitor |
| First-open file dialog | < 150 ms | Manual timing |

Miss > 10% = issue blocking exit criteria.

---

## 23. Security & Threat Model v0.3

### 23.1 What v0.3 adds over v0.2

| v0.2 (Phase 2) | v0.3 (Phase 3) |
|---|---|
| Terminal grant prompts | System-UI grant prompts |
| No clipboard | Clipboard read/write cap-gated |
| No GPU | GPU access via WebGPU subset |
| No audio | Microphone capture cap-gated |
| Only CLI UI | Windows, input events, accessibility tree |

### 23.2 New attack surfaces

- **GPU shaders.** WGSL validator catches most; driver bugs are residual risk.
- **Clipboard snooping.** Capped; apps cannot read clipboard without permission.
- **Microphone / camera eventually.** Phase 3 only ships microphone; camera is Phase 4.
- **Screen recording.** Not exposed in Phase 3; deferred indefinitely pending strong need.
- **Accessibility tree as attack vector.** Untrusted app can publish a11y tree that claims to be system UI — mitigated by tree belonging to a specific window owned by app.
- **IME spoofing.** IME is trusted; apps cannot inject IME events into other apps.

### 23.3 STRIDE delta

Only changes from v0.2:

| Category | New threat | Mitigation |
|---|---|---|
| S | App spoofs system grant dialog | Grant dialogs owned by runtime, marked at OS level where possible |
| T | App tampers with GPU shader at runtime | Re-validation on submission |
| I | App reads clipboard without prompt | Cap-gated |
| I | App captures microphone secretly | Cap-gated + OS-level prompt (TCC on macOS, privacy prompt on Windows) |
| E | Shader triggers GPU DoS or driver crash | Bounded resource limits; wgpu's validator |

### 23.4 Explicitly deferred

- Screen recording / screenshotting — not exposed.
- Global keyboard shortcuts — not exposed.
- Camera access — Phase 4.
- Process monitoring — not exposed.
- Background execution — Phase 4+.

---

## 24. Documentation Deliverables

### 24.1 Updated UAPI reference

Auto-generated from WIT. Now includes `ui`, `gfx`, `audio`.

### 24.2 "Build a GUI app" tutorial

Step-by-step construction of a mini-app (counter + text field) — smaller than `krate-notes`. Language: Rust. Reader should finish in < 45 min.

### 24.3 Widget catalog

`docs/book/src/reference/widgets.md` — each widget with screenshot per host + prop reference.

### 24.4 Host-specific adapter notes

`docs/book/src/phase3/adapter-notes-{linux,macos,windows}.md` — the tribal knowledge that Phase 4 iOS/Android developers will need.

### 24.5 A11y + IME test plans

`docs/book/src/phase3/a11y-testing.md` and `ime-testing.md`. Running these plans is a Phase 3 exit criterion and a Phase 4 prerequisite.

### 24.6 Threat model v0.3

Update `docs/book/src/security/threat-model.md`.

### 24.7 Migration note

`docs/book/src/phase3/migrating-from-phase2.md` — minimal; Phase 2's WIT is unchanged so migration is additive.

---

## 25. Architecture Decision Records

Expected ADRs in Phase 3 (8 at minimum):

| ID | Title | Week |
|---|---|---|
| 0013 | Widget lowering model: native-with-drawn-fallback | W1 |
| 0014 | Windowing: winit (Linux/Windows) + AppKit bridge (macOS) | W1 |
| 0015 | Layout engine: Taffy | W1 |
| 0016 | 2D renderer: vello on wgpu | W2 |
| 0017 | 3D API: WebGPU-compatible subset, WGSL only | W2 |
| 0018 | Text: harfbuzz + per-host IME | W2 |
| 0019 | Accessibility: accesskit | W2 |
| 0020 | Audio: cpal + symphonia | W2 |

Further ADRs as specific decisions surface (grant dialog design, theme token layering, reconciler model).

---

## 26. Exit Criteria Checklist

### WIT & UAPI
- [ ] `ui.wit`, `gfx.wit`, `audio.wit` merged and frozen at v0.1.0.
- [ ] UAPI reference regenerated.
- [ ] Consolidated `gui` world published.

### Widgets
- [ ] All 15 widgets render natively on all three hosts where a native equivalent exists.
- [ ] All 15 widgets render correctly as drawn fallback on all three hosts.
- [ ] Layout correct at 100%, 125%, 150%, 200% DPI on each host.
- [ ] Dark mode follows system and switches live.

### Input
- [ ] Keyboard events normalized across hosts.
- [ ] Mouse / pointer events correct including hover, right-click, wheel.
- [ ] IME test matrix green on all 9 cells (3 hosts × 3 languages).

### Rendering
- [ ] vello draws correctly on all three hosts.
- [ ] Font rendering passes side-by-side review with native controls.
- [ ] 3D "clear-to-red" smoke test green on all three hosts.
- [ ] 10,000-node layout bench: < 4 ms.

### Accessibility
- [ ] accesskit tree built for `krate-notes`.
- [ ] VoiceOver (macOS) announces all widgets correctly.
- [ ] Narrator (Windows) announces all widgets correctly.
- [ ] Orca (Linux) announces all widgets correctly.
- [ ] Keyboard-only navigation works end-to-end in `krate-notes`.

### Audio
- [ ] `krate-audio` playback of 44.1 kHz WAV, MP3, AAC works on all three hosts.
- [ ] Microphone capture fires UCap prompt; works when granted.

### UCap v0.2
- [ ] System-UI grant dialog replaces terminal prompt for GUI apps.
- [ ] Dialog offers Once / Session / Deny options.
- [ ] Denial propagates as error to app.
- [ ] OS-native privacy prompts integrate (macOS TCC, Windows privacy).

### Flagship
- [ ] `krate-notes` runs on Windows, macOS, Linux.
- [ ] Passes native-feel rubric (see §29 Appendix C).
- [ ] 60 fps sustained while typing.
- [ ] Screen readers pass on all three hosts.

### Performance
- [ ] All §22 targets met within 10%.
- [ ] Frame-time dashboard published.
- [ ] Benchmarks regressed ≤ 10% vs Phase 2 baseline.

### CI & Quality
- [ ] Cross-host CI green for ≥ 10 consecutive days.
- [ ] Snapshot tests green (with documented per-host variance allowance).
- [ ] Fuzz targets extended for `ui.wit` parsers and shader validator.

### Documentation
- [ ] UAPI reference for `ui`, `gfx`, `audio` published.
- [ ] Widget catalog with per-host screenshots.
- [ ] "Build a GUI app" tutorial.
- [ ] Host-specific adapter notes.
- [ ] A11y and IME test plans.
- [ ] Threat Model v0.3.

### ADRs
- [ ] ADR-0013 through ADR-0020 merged.

### External validation
- [ ] One external developer builds and runs a GUI app via tutorial in < 45 min.
- [ ] One external user runs `krate-notes` and describes it as "feels native."
- [ ] Retrospective written.
- [ ] Phase 4 kickoff issue opened with link to Phase 4 plan doc.

---

## 27. Phase 3 Risks

### 27.1 Technical risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Widget lowering produces subtle per-host inconsistencies | Very high | High | Snapshot tests per host; native-feel rubric; dogfood `krate-notes` on each host weekly |
| IME edge cases (surrogate pairs, pre-edit in list widgets) | High | High | Manual test matrix early; catch in Week 11, not Week 16 |
| Accessibility retrofits found to be incomplete | Medium | High | Bake a11y at Week 6, not Week 12; screen-reader review per widget, not end of phase |
| vello maturity not yet production-ready | Medium | Medium | Pinned version; fallback to simpler CPU rasterizer for edge cases; monitor upstream |
| wgpu driver bugs on niche hardware | Medium | Medium | Test on broadest possible GPU matrix; document known bad drivers |
| DPI scaling edge cases (multi-monitor, mid-session change) | High | Medium | Dedicated DPI test plan; Week 16 hardening time |
| Reconciler bugs cause flicker or stale widgets | High | High | Heavy randomized property tests; Week 5–8 is reconciler forever-under-test |
| Native menu semantics mismatch (macOS app-menu vs window-menu) | Medium | Medium | Host-specific menu logic in adapter; document expected per-host behavior |
| "Answer D" complexity exceeds team capacity | Low-Medium | Critical | Phase has 4 months and is explicit about this bet; if failing by Week 10, reassess to either pure-draw or pure-native |
| Font fallback chains break for non-Latin scripts | High | Medium | Font enumeration per host; document fallback strategy |

### 27.2 Process risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Scope creep: "let's add tray icons / notifications / themes" | Very high | High | Deferred list is a live doc; every new ask lands there first |
| WIT churn as widgets are implemented | High | Medium | Week 1–2 is WIT-locked after review; changes post-Week-4 need ADR |
| `krate-notes` ambition scope | High | Medium | LOC cap (2000); features frozen Week 14 |
| Perf tuning eats all buffer | Medium | Medium | Performance targets set Week 1; accept trailing regressions within 10% |
| Founder time pressure | Very high | Critical | Phase 3 is the longest so far. Phase 3 cannot be compressed without sacrificing quality. Extend calendar, do not drop scope. |

### 27.3 Tripwires

Stop and reassess if:
- Week 8 and any one host doesn't have widgets rendering.
- Week 12 and screen reader test fails on any host.
- Week 14 and frame time is > 25 ms for `krate-notes`.
- Any critical widget (Button, TextField, List) lacks native rendering on any host by Week 10.
- IME matrix has > 3 red cells at Week 13.

---

## 28. Handoff to Phase 4

### 28.1 What Phase 4 inherits

- Three frozen v0.1 UI/GFX/Audio modules.
- Widget protocol that worked on three desktops.
- Layout engine that works everywhere Taffy works.
- Renderer that targets wgpu (which also targets Metal, Vulkan — mobile-friendly).
- UCap v0.2 with system-UI grants (adapted for mobile in Phase 4).
- Flagship app that proves the abstraction holds.

### 28.2 What Phase 4 extends

- Add `krate:ui/touch`, `krate:ui/gesture`, `krate:ui/haptics`.
- Mobile lifecycle events: background, foreground, memory pressure.
- Mobile-appropriate default layouts (bottom sheets, tab bars).
- iOS-specific adapter using UIKit.
- Android-specific adapter using View system.
- Sensor UAPI module (`krate:sensors`).

### 28.3 What Phase 4 must NOT touch

- `ui.wit`, `gfx.wit`, `audio.wit` — frozen. Additions are new modules or `@0.2.0` side-by-side.
- Widget identity model.
- Layout semantics.
- Reconciler behavior.

### 28.4 Lessons-learned capture

Before Phase 4 kickoff, update the main Build Plan and Phase 4 Plan with:
- Which widgets proved hard to lower; may need mobile re-thinking.
- Adapter patterns that worked / didn't.
- Reconciler perf wins and losses.
- Specific regressions to watch for when mobile lifecycle enters.

---

## 29. Appendices

### Appendix A — Widget-to-native mapping table

| Widget | macOS | Windows | Linux (GTK4) | Fallback |
|---|---|---|---|---|
| Stack | `NSStackView` | Stack panel (Win32 layout) | `GtkBox` | Drawn |
| Grid | custom | custom | `GtkGrid` | Drawn |
| Scroll | `NSScrollView` | Scroll host | `GtkScrolledWindow` | Drawn |
| Tabs | `NSTabView` | Tab control | `GtkNotebook` | Drawn |
| Button | `NSButton` | Win32 button | `GtkButton` | Drawn |
| Checkbox | `NSButton` (checkbox) | Win32 checkbox | `GtkCheckButton` | Drawn |
| Radio | `NSButton` (radio) | Win32 radio | `GtkCheckButton` (radio) | Drawn |
| Switch | `NSSwitch` | (custom) | `GtkSwitch` | Drawn |
| Slider | `NSSlider` | Trackbar | `GtkScale` | Drawn |
| Progress | `NSProgressIndicator` | Progress bar | `GtkProgressBar` | Drawn |
| Text | `NSTextField` (non-editable) | Static text | `GtkLabel` | Drawn |
| TextField | `NSTextField` | Edit control | `GtkEntry` | Drawn (no IME) |
| TextArea | `NSTextView` in `NSScrollView` | Rich edit | `GtkTextView` | Drawn |
| List | `NSTableView` | ListView | `GtkListView` | Drawn |
| Tree | `NSOutlineView` | TreeView | `GtkColumnView` | Drawn |
| Image | `NSImageView` | Picture box | `GtkPicture` | Drawn |
| Canvas | drawn | drawn | drawn | Drawn |

### Appendix B — Key code normalization excerpt

| Logical | macOS virtual | Windows VK | Linux evdev | Web KeyboardEvent.code |
|---|---|---|---|---|
| `Enter` | `kVK_Return` | `VK_RETURN` | `KEY_ENTER` | `Enter` |
| `Escape` | `kVK_Escape` | `VK_ESCAPE` | `KEY_ESC` | `Escape` |
| `A` | `kVK_ANSI_A` | `VK_A` | `KEY_A` | `KeyA` |
| `1` | `kVK_ANSI_1` | `VK_1` | `KEY_1` | `Digit1` |
| `Left` | `kVK_LeftArrow` | `VK_LEFT` | `KEY_LEFT` | `ArrowLeft` |
| `Cmd/Meta` | `kVK_Command` | `VK_LWIN` | `KEY_LEFTMETA` | `MetaLeft` |

Full table checked in as `crates/adapter-common/src/input/keymap.rs`.

### Appendix C — "Feels native" rubric

Passing rubric for `krate-notes` on each host. 10 checks per host, need ≥ 9/10:

1. Window chrome matches host convention (traffic lights / min-max-close / Wayland client-side).
2. Menu bar semantics correct (macOS app menu; Windows/Linux per-window).
3. Scrollbar style matches host.
4. Selection color matches host accent.
5. Focus ring matches host.
6. Default button visually distinct per host convention.
7. Keyboard shortcuts use host modifier (⌘ vs Ctrl).
8. Drag behavior feels native (press threshold, snap-back).
9. Right-click menus appear where expected.
10. Dark mode transition is smooth and follows system.

Documented in `docs/book/src/phase3/native-feel-rubric.md`.

### Appendix D — Commands cheat sheet (Phase 3 additions)

```bash
# Run GUI app with auto-grant
krate run --auto-grant --gui apps/krate-notes/notes.wasm

# Force software rendering (for low-spec debug)
KRATE_GPU_BACKEND=cpu krate run app.wasm

# Enable frame-time tracing
KRATE_LOG=krate_runtime::ui::frame=trace krate run app.wasm

# Dump a11y tree
krate run --dump-a11y-tree app.wasm

# Run IME test manually
krate run --ime-debug apps/krate-notes/notes.wasm
```

### Appendix E — Debugging a stuck frame

1. Enable frame trace:
   ```
   KRATE_LOG=krate_runtime::ui::frame=trace,krate_gfx=debug krate run app.wasm
   ```
2. Check reconciler time in trace output.
3. Check layout time.
4. If GPU-bound: `KRATE_GPU_CAPTURE=1` produces a RenderDoc-compatible capture.
5. If WASM-bound: `--profile wall` produces a flame graph.

### Appendix F — Retrospective template

Save as `docs/book/src/phase3/retro.md` at the end of Phase 3.

```markdown
# Phase 3 Retrospective

**Planned:** 16 weeks / **Actual:** <X> weeks
**Written:** YYYY-MM-DD
**Author:** @handle

## What shipped
- …

## What didn't ship and why
- …

## Widget protocol lessons
- …

## Per-host adapter lessons
- macOS: …
- Windows: …
- Linux: …

## Renderer + layout lessons
- …

## A11y + IME lessons
- …

## The "Answer D" bet: did it hold?
- …

## Performance surprises
- …

## Concrete changes to the main Build Plan
- …

## Concrete changes to the Phase 4 plan before we start it
- …
```

---

---

## Development Log

> **Phase Status:** Started at contract layer
> **Started:** 2026-05-19
> **Completed:** —
> **Last Updated:** 2026-06-20

### Progress Summary

Phase 3 has started with the first WIT draft, checker, GUI manifest path,
capability names, shared in-memory UI adapter scaffold, a shared `UiAdapter`
trait, an explicit `WindowAdapter` trait, host-crate UI adapter entry points,
runtime host-adapter discovery, a runtime-facing UI dispatcher, the first shared widget tree model, draft
runtime/widget-tree dispatch, the first Taffy-backed layout wrapper, generated
layout-shape coverage, a 1k/10k-node layout benchmark target, a prepared layout
path for repeated passes, a first layout hit-test helper, and a draft pointer
event route that turns logical pointer coordinates into queued events with a
hit widget ID. It now also has draft key and committed-text routes that target
the focused widget, plus FIFO event polling for the first app-facing event-loop
primitive. Draft host window events for close request, resize, focus, theme,
and scale have shared routes too. Adapter info now records both the active
headless window backend and the planned native backend for each host: AppKit on
macOS, and winit on Linux and Windows. The shared window layer can now bind an
opaque native host handle to a stable Krate `WindowId`, and the macOS adapter
exposes the first AppKit handle handoff point. The opt-in AppKit prototype can
create and show a real `NSWindow`; it now also has explicit bridge targets for
native close, resize, focus, and display-scale events, plus a snapshot helper
that reads native window state. An `AppKitWindowSession` now owns the native
window and remembers the last snapshot so the next AppKit delegate work has a
small event-loop state object instead of loose helper calls. The macOS adapter
now also exports `AppKitWindowNativeEvent`, `AppKitWindowEventState`,
`AppKitWindowDelegateCallback`, and `AppKitWindowDelegateBridge`, so delegate
callbacks have a tested Rust bridge before we add the Objective-C delegate
object itself. Redraw requests are included in that path now, which is the
handoff the first AppKit drawing surface will use. The AppKit side now also has
a draw-surface state scaffold that tracks size, scale, clear color, redraw
requests, and frame metadata. It does not paint pixels yet, but it gives the
coming `NSView` painter a checked state object to call. The first AppKit draw
view surface now attaches an owned `NSView` to the prototype window, sets a
visible clear color through AppKit, marks the view dirty, and records the first
frame snapshot for local smoke proof. The real AppKit window delegate object
now exists too: it implements `NSWindowDelegate`, records close, resize, focus,
and backing-scale callbacks, and lets `AppKitWindowSession` drain them through
the same tested Rust bridge.
ADR-0013 and RFC-0003 record the widget lowering rule
before native widget work depends on it. ADR-0014 records the layout engine
choice. This is not a frozen API and not a working desktop GUI yet. It is the
contract, runtime boundary, widget model, geometry foundation, and first
input-routing proof, plus the handle mapping needed by the next host adapter work.

### Current Slice Checklist

| Task ID | Task | Completed | Notes |
|---|---|---|---|
| P3-UAPI-00 | Draft `gui` world for Phase 3 | 2026-05-19 | `krate:app@0.2.0` parses with `ui`, `gfx`, and `audio` imports. |
| P3-TOOL-00 | Add Phase 3 UAPI checker | 2026-05-19 | `scripts/check-phase3-uapi.sh` keeps the draft WIT parseable and shaped. |
| P3-MANIFEST-00 | Recognize GUI manifests | 2026-05-19 | `manifest check` accepts `krate:app/gui@0.2.0`; `run` exits clearly until GUI runtime exists. |
| P3-UCAP-00 | Add first Phase 3 capability names | 2026-05-19 | `ui`, `gfx`, and `audio` capability strings now parse through the manifest and policy layer. |
| P3-UI-00 | Add shared draft UI adapter model | 2026-05-19 | `adapter-common::ui` tracks draft window IDs, lifecycle, title, size, visibility, redraw events, and validation. |
| P3-RUNTIME-00 | Add runtime UI dispatcher scaffold | 2026-05-21 | `runtime::phase3_ui` checks UCap before draft window operations and proves clipboard grants fail before unsupported host code. |
| P3-UI-04A | Add shared `UiAdapter` trait | 2026-05-21 | `adapter-common::ui::UiAdapter` is now the runtime boundary for window lifecycle, redraw, event drain, and draft clipboard behavior. |
| P3-UI-04B | Start host UI adapter entry points | 2026-05-21 | macOS, Linux, and Windows adapter crates expose headless draft UI adapters with blank-window smoke tests. |
| P3-RUNTIME-01 | Discover host UI adapter from runtime | 2026-05-21 | `Phase3UiRuntime` owns the UCap guard and selected host UI adapter so the dispatcher can run through the current OS entry point. |
| P3-UI-01A | Draft widget protocol decision docs | 2026-05-21 | ADR-0013, RFC-0003, and the mdBook widget protocol page record native widget lowering plus drawn fallback rules. |
| P3-UI-01B | Add shared widget tree model | 2026-05-21 | `adapter-common::ui` now has `WidgetId`, `WidgetKind`, `WidgetNode`, `WidgetStyle`, and `WidgetTree` with parent-link and style validation. |
| P3-RUNTIME-02 | Add draft widget-tree dispatcher path | 2026-05-21 | `UiAdapter` and `Phase3UiDispatcher` now support set root, upsert, remove, focus, tree lookup, and focused-widget lookup through the same UCap boundary. |
| P3-UI-03A | Add first Taffy-backed layout wrapper | 2026-05-21 | `crates/layout` maps the shared `WidgetTree` into Taffy and returns stable `WidgetId` keyed rectangles through the runtime dispatcher. |
| P3-UI-03B | Add layout coverage, benchmark target, and hit-test helper | 2026-05-21 | `krate-layout` now tests 100 generated tree shapes, compiles a Criterion 1k/10k-node benchmark target, exposes absolute rectangles, and can hit-test a point to the deepest widget. |
| P3-UI-03C | Add prepared repeated-layout path | 2026-05-21 | `PreparedLayoutTree` builds the Taffy tree once, recomputes layout for new viewports, and is reachable through `Phase3UiDispatcher::prepare_layout`. |
| P3-INPUT-01A | Add draft pointer event routing | 2026-05-21 | `Phase3UiDispatcher::route_pointer_event` computes layout, hit-tests the logical point, and queues a `UiEvent::Pointer` with the target widget ID when one is found. |
| P3-INPUT-01B | Add draft key and text input routing | 2026-05-21 | `Phase3UiDispatcher::route_key_event` and `route_text_input` look up the focused widget and queue portable key/text events through the shared adapter boundary. |
| P3-UI-04C | Add FIFO event polling | 2026-05-21 | `UiAdapter::poll_event` and `Phase3UiDispatcher::poll_event` return one queued event at a time in insertion order, matching the planned `events.poll()` shape. |
| P3-UI-04D | Add draft host window event routes | 2026-05-21 | Close-requested, host-resized, and window-focused events can now be queued through the shared adapter and runtime dispatcher without closing the window early. |
| P3-UI-04E | Add draft theme and scale event routes | 2026-05-22 | System theme changes and per-window scale-factor changes now queue through the shared adapter and runtime dispatcher, with scale validation before native DPI work starts. |
| P3-UI-04F | Split explicit `WindowAdapter` boundary | 2026-05-22 | Window lifecycle and host-window events now live under `WindowAdapter`; `UiAdapter` builds on it for widgets, input, and clipboard. Host adapters report the active headless backend plus planned native backend. |
| P3-UI-04G | Add native window handle handoff | 2026-05-22 | `WindowAdapter` can now attach, inspect, and detach an opaque native handle for a Krate window id. macOS exposes the first AppKit handoff method while the default backend remains headless draft. |
| P3-UI-04H | Add opt-in AppKit window prototype | 2026-05-22 | macOS now has an `AppKitWindowBackend` that can create an owned `NSWindow` on the main thread, attach its handle to a Krate window id, and show it through the shared window path. The default adapter still stays headless draft. |
| P3-UI-04I | Add AppKit event bridge targets | 2026-05-22 | The AppKit prototype now has explicit bridge methods for close, resize, focus, and display-scale events, plus a snapshot helper that reads content size, focus, visibility, and backing scale from the native window. |
| P3-UI-04J | Add AppKit window session state | 2026-05-22 | `AppKitWindowSession` now owns the prototype window, remembers the last native snapshot, refreshes changed state into the shared queue, and gives future delegates one place to report close requests. |
| P3-UI-04K | Add AppKit native event state | 2026-05-23 | `AppKitWindowNativeEvent` and `AppKitWindowEventState` now provide the tested Rust callback surface for resize, focus, scale, close, redraw, and full snapshot events before the Objective-C delegate lands. |
| P3-UI-04L | Add AppKit redraw bridge | 2026-05-23 | AppKit can now queue redraw requests through the same native event state and shared `WindowAdapter::request_redraw` path that the future drawing surface will use. |
| P3-UI-04M | Add AppKit delegate callback bridge | 2026-05-23 | AppKit-style callback names now translate through a tested Rust bridge into the native event state, keeping the coming Objective-C delegate thin. |
| P3-UI-04N | Add AppKit draw-surface state | 2026-06-07 | AppKit now has a small draw-surface state object for size, scale, clear color, redraw count, and frame metadata. It routes redraw requests through the delegate bridge, but it does not paint real pixels yet. |
| P3-UI-04O | Add AppKit draw view surface | 2026-06-20 | AppKit can now attach an owned `NSView` to the prototype window, set a visible clear color, mark it dirty, and record a first frame snapshot. The path is still opt-in and covered by an ignored local smoke. |
| P3-UI-04P | Add AppKit native delegate object | 2026-06-20 | AppKit now has a retained `NSWindowDelegate` object that records close, resize, focus, and backing-scale callbacks into a FIFO queue. `AppKitWindowSession` can drain those callbacks through the existing Rust delegate bridge. |
| P3-UI-04Q | Add AppKit event-loop step driver | 2026-06-20 | AppKit can now process one non-blocking native tick: refresh native state, drain delegate callbacks, and queue redraw through the shared event stream. |
| P3-RUNTIME-03 | Add selectable AppKit prototype runtime mode | 2026-06-20 | `Phase3UiRuntime` can now explicitly request a native prototype host mode. The normal runtime remains headless, while macOS can select the AppKit prototype adapter and report native window plus event-loop capability. |
| P3-RUNTIME-04 | Add shared event-loop pump boundary | 2026-06-20 | `UiAdapter` and `Phase3UiDispatcher` now expose a host-neutral non-blocking event-loop pump. Headless adapters return no native tick, while the AppKit prototype maps its native tick report into the shared shape. |
| P3-RUNTIME-05 | Add selectable AppKit runtime smoke | 2026-06-21 | Added a macOS local smoke command that requests `Phase3HostUiMode::NativePrototype`, creates and shows an AppKit window, pumps one shared event-loop tick, checks the native adapter report, and closes the window. |
| P3-UI-04R | Add Linux and Windows Winit prototype boundaries | 2026-06-21 | Linux and Windows now expose explicit Winit prototype adapter boundaries, tested native-handle handoff methods, and guarded discovery paths. They do not claim real native windows yet; this was the step before the shared session owner scaffold. |
| P3-UI-04S | Add shared Winit session owner scaffold | 2026-06-21 | Added shared Winit snapshot, native-event, event-loop-step, report, and session types. Linux and Windows prototype adapters now own tracked Winit sessions, can attach a handle, pump a prepared event step, and remove the session on close. |
| P3-UI-04T | Add Winit callback collector bridge | 2026-06-23 | Added a shared FIFO callback collector for future Winit event handlers. Linux and Windows prototype adapters can now record Winit-shaped callbacks, drain them through the normal event-loop pump, and keep the collector bound to the tracked session. |
| P3-VS-01A | First native AppKit widget lowering (vertical slice, Rust side) | 2026-07-02 | `adapter-macos` lowers `Button`/`TextField`/`Text` placements (`AppKitWidgetPlacement`) to real `NSButton`/`NSTextField` controls positioned from Taffy layout rects with AppKit Y-flip. `KrateWidgetTarget` receives target-action callbacks and pushes `WidgetActivated` through the delegate FIFO, so pumped native clicks reach the shared stream as routed pointer events with widget ids. Verified end to end by the extended `phase3_appkit_runtime_smoke` example (real window, `performClick`, routed event observed, native text updated). Remaining for P3-VS-01: WASM `gui`-world host imports and the demo component. |
| P3-VS-01B | GUI world WASM path (vertical slice complete) | 2026-07-03 | Added `phase3_gui_bindings` (gui world, `with:` reuse of Phase 2 modules), `Phase3GuiHost` serving `krate:ui` window/tree/events over the UCap-gated dispatcher with naive native re-lowering per tree change, the `krate run` gui fallback path, the `--native-window` flag, and the import-pure `apps/krate-hello-gui` sample. The `krate:ui` events interface moved to single-event `option<event>` polling to match the dispatcher FIFO design and keep guest components free of WASI imports. Verified by `scripts/smoke-phase3-gui-app.sh` (headless) and a local native run. |
| P3-EMB-01/02 | Embedding API + `krate run --json` | 2026-07-03 | Added `Runtime::run_bytes_captured`/`run_file_captured`, the `embed` module (`run_component`, `EmbedOutcome`, `EmbedExitClass`, doc-tested sub-30-line example), and the `--json` flag emitting schema `krate.run.v1` with app identity, granted/denied capabilities, exit classification, duration, and captured stdout. Book page `phase3/embedding.md`; CLI integration tests cover success, denied-before-run, and invalid-component paths. P3-EMB-03 (MCP wrapper) remains. |
| P3-EMB-03 | MCP server wrapper | 2026-07-03 | Added `krate-mcp-server` (crates/tools binary): newline-delimited JSON-RPC over stdio, one `run_component` tool backed by the embedding API, returning `krate.run.v1` reports with denials as data. Acceptance verified end to end: `krate-cat` without grants → permission-denied naming `fs.read:fixtures/**`; with auto_grant → success with captured stdout. The agent-embedding track (A3) is complete. |
| P3-WIN-01 | Real Linux winit windows | 2026-07-04 | `adapter-linux::winit_native`: thread-local winit 0.30 event loop pumped with `pump_app_events`, ApplicationHandler feeding the shared `WinitWindowEventCollector`, X11-first (ADR note: wayland-sys needs system libs; XWayland covers Wayland desktops), display-server guard for headless hosts, env-gated any-thread opt-in for tests. Prototype adapter creates/shows/closes real windows and reports the Winit backend truthfully. Definition of done met in CI: hello-gui opened a real winit window on a Linux host under Xvfb and exited cleanly (full matrix run green, all three OS lanes). Windows winit remains. |
| P3-WIN-02 | Real Windows winit windows | 2026-07-04 | `adapter-windows::winit_native`, cloned from the CI-proven Linux implementation (no display guard needed; `EventLoopBuilderExtWindows` for the test opt-in). Compiled and passed on the first Windows lane attempt. Query/pump paths on both platforms no longer lazily create the event loop (winit panics off the main thread). Full matrix green with both native window proofs: hello-gui opened real winit windows on Linux (Xvfb) and Windows and exited cleanly. One portable artifact now opens real windows on all three desktop OSes. |
| P3-DRAW-01 | First drawn-widget pass (Linux) | 2026-07-04 | `winit_native` stores lowered placements and paints them per pump through softbuffer (x11-dlopen): background fill, button block, bordered field, text block, DPI-scaled. Verified in the green full matrix with the Xvfb window run. Lessons: tiny-xlib needs headers unless dlopened; clippy on Linux-only code needs the CI lane. Next: Windows drawing clone, then vello behind the same placement contract. |

---

### Exit Criteria Status

Full criteria in [§3 Success Criteria](#3-success-criteria). Check off as each criterion is met.

| # | Criterion | Status |
|---|-----------|--------|
| 1 | `ui`, `gfx`, `audio` WIT modules frozen at v0.1.0 | Draft started; not frozen |
| 2 | All three modules implemented in Linux, macOS, Windows adapters; CI green | Not done |
| 3 | `krate-notes` runs on all three desktop OSes | Not done |
| 4 | `krate-notes` UI feels native on each host (documented rubric passed) | Not done |
| 5 | Steady-state frame time ≤ 16.7 ms on 2020+ hardware | Not done |
| 6 | Cold start for GUI app < 300 ms to first paint | Not done |
| 7 | IME (CJK input) works correctly on all three hosts | Not done |
| 8 | Screen reader reads `krate-notes` on Win/macOS/Linux (Narrator/VoiceOver/Orca) | Not done |
| 9 | UCap v0.2: system-UI grant dialog fires for GUI apps | Not done |
| 10 | 60 fps sustained with a 10,000-node widget tree benchmark | Not done |
| 11 | DPI scaling at 100%, 125%, 150%, 200% renders correctly (snapshot tests) | Not done |
| 12 | Dark mode follows system preference and switches live | Not done |
| 13 | Runtime binary size < 80 MB | Not done |
| 14 | Per-app RSS < 120 MB for `krate-notes` | Not done |
| 15 | ADRs 0013 through at least 0020 merged | Not done |

---

### Completed Tasks

| Task ID | Task | Completed | Notes |
|---------|------|-----------|-------|
| P3-UI-02 | First `krate:ui@0.1.0` WIT draft | 2026-05-19 | Covers window, tree, events, dialog, clipboard, and menu shape. |
| P3-GFX-01 | First `krate:gfx@0.1.0` WIT draft | 2026-05-19 | Covers simple 2D canvas commands and a minimal future 3D surface. |
| P3-AUDIO-01 | First `krate:audio@0.1.0` WIT draft | 2026-05-19 | Covers playback and capture stream shape. |
| P3-TEST-00 | Phase 3 WIT checker | 2026-05-19 | `scripts/check-phase3-uapi.sh` parses contracts and checks package, world, naming, docs, and permission error shape. |
| P3-RUN-00 | CLI recognizes Phase 3 GUI manifests | 2026-05-19 | `manifest check` accepts `krate:app/gui@0.2.0`; `run` exits early with a clear not-implemented message. |
| P3-UCAP-00 | Phase 3 capability names | 2026-05-19 | Manifest and policy now understand the first `ui`, `gfx`, and `audio` capability strings. |
| P3-UI-00 | Shared draft UI adapter model | 2026-05-19 | `adapter-common::ui` tracks draft windows and events without native OS windows yet. |
| P3-RUNTIME-00 | Runtime UI dispatcher scaffold | 2026-05-21 | `runtime::phase3_ui` gates draft window work through UCap and keeps unsupported clipboard paths permission-checked. |
| P3-UI-04A | Shared UI adapter trait | 2026-05-21 | `runtime::phase3_ui` now talks to `UiAdapter`, with `DraftUiAdapter` as the current in-memory implementation. |
| P3-UI-04B | Host UI adapter entry points | 2026-05-21 | `adapter-{macos,linux,windows}` each expose a headless draft UI adapter and a blank-window smoke test. |
| P3-RUNTIME-01 | Runtime host UI adapter discovery | 2026-05-21 | `Phase3UiRuntime::with_host_adapter` selects the current host adapter and exposes adapter capability info. |
| P3-UI-01A | Widget protocol decision docs | 2026-05-21 | Added ADR-0013, RFC-0003, an mdBook widget protocol page, and a design-doc checker. |
| P3-UI-01B | Shared widget tree model | 2026-05-21 | Added stable widget IDs, first widget kind enum, style hints, labels, role hints, and parent validation in `adapter-common::ui`. |
| P3-RUNTIME-02 | Draft widget-tree dispatch path | 2026-05-21 | Added draft widget-tree operations to `UiAdapter`, the headless host adapters, and `Phase3UiDispatcher`, with adapter-boundary checks. |
| P3-UI-03A | First layout wrapper | 2026-05-21 | Added `krate-layout`, Taffy integration, viewport validation, layout snapshots, runtime dispatch, ADR-0014, and mdBook layout docs. |
| P3-UI-03B | Layout proof expansion | 2026-05-21 | Added generated 100-shape layout tests, optimized child indexing, absolute rectangle helpers, hit testing, a compile-checked Criterion benchmark target, and a Phase 3 layout check script. |
| P3-UI-03C | Prepared repeated-layout path | 2026-05-21 | Added `PreparedLayoutTree`, prepared 1k/10k benchmark lanes, runtime dispatcher access, and repeated-viewport tests. |
| P3-INPUT-01A | Draft pointer event routing | 2026-05-21 | Added shared pointer event types, adapter queue support, host adapter forwarding, and runtime hit-test routing from logical coordinates to optional widget IDs. |
| P3-INPUT-01B | Draft key and text input routing | 2026-05-21 | Added shared key/text event types, validation, adapter queue support, host adapter forwarding, and runtime focused-widget routing. |
| P3-UI-04C | FIFO event polling | 2026-05-21 | Added shared adapter, headless host adapter, and runtime dispatcher polling so future `events.poll()` calls can consume one queued event at a time. |
| P3-UI-04D | Draft host window event routes | 2026-05-21 | Added close-requested, host-resized, and window-focused queue paths so native event loops can report window events before real windows land. |
| P3-UI-04E | Draft theme and scale event routes | 2026-05-22 | Added theme-change and scale-factor queue paths through the shared adapter, host adapter shims, and runtime dispatcher. |
| P3-UI-04F | Explicit `WindowAdapter` boundary | 2026-05-22 | Added a named window trait under `adapter-common`, split window lifecycle from widget/input operations, and recorded planned native backends in adapter info. |
| P3-UI-04G | Native window handle handoff | 2026-05-22 | Added `NativeWindowHandle`, native attach/lookup/detach methods, shared events for attach/detach, and a macOS `attach_appkit_window_handle` entry point. |
| P3-UI-04H | Opt-in AppKit window prototype | 2026-05-22 | Added target-specific `objc2` and AppKit wiring, an owned `AppKitWindowPrototype`, a safe main-thread gate, and ignored local smoke coverage for opening a real macOS window without disturbing normal CI. |
| P3-UI-04I | AppKit event bridge targets | 2026-05-22 | Added AppKit bridge methods that queue shared close, resize, focus, and scale events, plus a native snapshot helper for future delegate and event-loop wiring. |
| P3-UI-04J | AppKit window session state | 2026-05-22 | Added an `AppKitWindowSession` wrapper that owns the AppKit prototype, caches the last snapshot, refreshes changed state, and includes ignored local smoke coverage for the session path. |
| P3-UI-04K | AppKit native event state | 2026-05-23 | Added a delegate-shaped native event enum and event-state object, exported them from the macOS adapter crate, and covered changed-state caching and failed scale handling. |
| P3-UI-04L | AppKit redraw bridge | 2026-05-23 | Added `RedrawRequested` to the AppKit native event path with bridge methods and coverage for queueing shared redraw events. |
| P3-UI-04M | AppKit delegate callback bridge | 2026-05-23 | Added `AppKitWindowDelegateCallback` and `AppKitWindowDelegateBridge`, exported both, and covered resize, focus, scale, redraw, close, snapshot, and failed scale callback behavior. |
| P3-UI-04N | AppKit draw-surface state | 2026-06-07 | Added `AppKitColor`, `AppKitDrawFrame`, and `AppKitDrawSurfaceState`, exported them from the macOS adapter crate, and covered redraw routing plus frame metadata. |
| P3-UI-04O | AppKit draw view surface | 2026-06-20 | Added `AppKitDrawViewSurface` and `AppKitDrawViewSurfaceSnapshot`, exported them from the macOS adapter crate, enabled the needed AppKit `NSView`/`NSColor` bindings, and added an ignored local smoke for attaching a real AppKit content view. |
| P3-UI-04P | AppKit native delegate object | 2026-06-20 | Added `AppKitWindowNativeDelegate` and `AppKitWindowDelegateQueue`, exported both from the macOS adapter crate, implemented the first `NSWindowDelegate` methods, and wired session draining into the existing bridge. |
| P3-UI-04Q | AppKit event-loop step driver | 2026-06-20 | Added `AppKitWindowEventLoopDriver`, `AppKitWindowEventLoopStep`, and `AppKitWindowEventLoopStepReport`, exported them from the macOS adapter crate, and covered one non-blocking tick that refreshes state, drains callbacks, and queues redraw. |
| P3-RUNTIME-03 | Selectable AppKit prototype runtime mode | 2026-06-20 | Added `Phase3HostUiMode`, `Phase3UiRuntime::try_with_host_adapter_mode`, and `MacosAppKitPrototypeUiAdapter`, with tests proving the default stays headless and native prototype mode is explicit. |
| P3-RUNTIME-04 | Shared event-loop pump boundary | 2026-06-20 | Added `UiEventLoopTick`, `UiAdapter::pump_event_loop_once`, and `Phase3UiDispatcher::pump_event_loop_once`, with tests proving the headless path is a no-op and AppKit maps native ticks into the common report. |
| P3-RUNTIME-05 | Selectable AppKit runtime smoke | 2026-06-21 | Added `scripts/smoke-phase3-appkit-runtime.sh` and `phase3_appkit_runtime_smoke`, a main-thread local proof for the full native prototype selector path: create, show, pump, inspect, and close through `Phase3UiDispatcher`. |
| P3-UI-04R | Linux and Windows Winit prototype boundaries | 2026-06-21 | Added `LinuxWinitPrototypeUiAdapter`, `WindowsWinitPrototypeUiAdapter`, `attach_winit_window_handle`, guarded `discover_winit_prototype_ui_adapter` entry points, runtime selector wiring, and adapter-boundary checks. |
| P3-UI-04S | Shared Winit session owner scaffold | 2026-06-21 | Added `WinitWindowSession`, `WinitWindowSnapshot`, `WinitWindowNativeEvent`, `WinitWindowEventLoopStep`, and `WinitWindowEventLoopStepReport`, then wired Linux and Windows prototype adapters to track sessions and pump prepared native events through the shared queue. |
| P3-UI-04T | Winit callback collector bridge | 2026-06-23 | Added `WinitWindowEventCollector` plus Linux and Windows prototype adapter methods to record callbacks, count pending callbacks, drain them into a Winit event-loop step, and pump them through the shared UI queue. |

---

### In Progress

| Task ID | Task | Started | Blockers |
|---------|------|---------|----------|
| P3-UI-01 | Widget protocol design RFC | 2026-05-19 | Draft written; needs review before the rule is treated as accepted. |
| P3-UI-03 | Layout engine (Taffy integration) | 2026-05-21 | First wrapper, 100-shape tests, benchmark target, and prepared repeated-layout path exist; local prepared 10k layout is below the exit budget, but cold rebuild is not, so recorded cross-host benchmark results and wider style coverage are pending. |
| P3-UI-04 | Window + event loop abstractions | 2026-05-19 | Explicit `WindowAdapter`, native handle handoff, shared `UiAdapter`, widget-tree dispatch, host entry points, runtime discovery, routed input events, FIFO event polling, host window events, theme/scale events, and an opt-in macOS AppKit window prototype exist. AppKit now has event bridge targets, a snapshot helper, session state, a delegate-shaped native event state object, redraw bridge, delegate callback bridge, draw-surface state, an opt-in draw view surface, a retained native window delegate, a non-blocking event-loop step driver, an explicit native prototype runtime selector, a shared runtime event-loop pump boundary, and a local smoke command for that runtime path. Linux and Windows now have Winit prototype boundaries, handle handoff tests, a shared session owner scaffold, and a FIFO callback collector bridge that the normal event-loop pump can drain. Next step is connecting real `winit` window creation and feeding actual `winit` callbacks into this collector. |
| P3-INPUT-01 | Keyboard + mouse input | 2026-05-21 | First runtime-side pointer, key, and committed-text routes exist; real host pointer, hover, wheel, keyboard, shortcut, IME composition, and cross-host normalization are pending. |

---

### ADRs Filed This Phase

| ADR | Title | Status | Merged |
|-----|-------|--------|--------|
| ADR-0013 | Widget lowering strategy — “native three of five” rule (Answer D) | Proposed | — |
| ADR-0014 | Layout engine: Taffy (flexbox subset) | Proposed | — |
| ADR-0015 | 2D canvas: vello on wgpu | Pending | — |
| ADR-0016 | GPU API: WebGPU-compatible subset via wgpu | Pending | — |

_ADRs 0017–0020 to be determined during Phase 3 work._

---

### Blockers & Open Questions

- Phase 2 outside developer review and final freeze packet are still pending.
- ADR-0013 is now drafted. It still needs review before it becomes accepted.

---

### Notes & Learnings

- 2026-05-19: Started Phase 3 with contracts first. The `gui` world imports the
  Phase 2 CLI surface plus draft `ui`, `gfx`, and `audio` modules. The checker
  keeps this draft parseable and documented while the host work begins.
- 2026-05-19: Added the first runtime-facing guardrail. Phase 3 GUI manifests
  are valid metadata now, but `krate run` stops before execution until the
  window runtime exists.
- 2026-05-21: Drafted ADR-0013 and RFC-0003 for the widget protocol. The rule
  is native controls where the host has a semantic match, with drawn fallback
  where it does not.
- 2026-05-21: Added the first shared widget tree model in `adapter-common`.
  This gives layout and native lowering a local Rust type path before real host
  widgets are wired.
- 2026-05-21: Extended the runtime and headless host adapters with draft
  widget-tree operations. The UI path can now set a root node, update child
  nodes, remove nodes, and move focus before native widgets exist.
- 2026-05-21: Added the first shared layout wrapper. `krate-layout` computes
  logical rectangles from the shared widget tree through Taffy, and the runtime
  can request a layout snapshot for a stored draft widget tree.
- 2026-05-21: Expanded layout proof without jumping ahead to native windows.
  The layout crate now has generated shape coverage, a 1k/10k benchmark target,
  absolute rectangle helpers, and a first hit-test helper for future input
  routing.
- 2026-05-21: First local layout benchmark run shows the 10k-node path is not
  ready for the Phase 3 exit budget yet. This is useful signal: the benchmark
  is now in place, and optimization remains planned before exit.
- 2026-05-21: Added the prepared layout path. Reusing the same Taffy tree brings
  the local 10k repeated-layout benchmark under the Phase 3 budget, while cold
  tree rebuild remains above budget. This matches the planned reconciler model,
  but still needs cross-host evidence before exit.
- 2026-05-21: Added the first draft pointer route. The runtime can now take
  logical pointer coordinates, compute layout, hit-test to the deepest widget,
  and queue a portable pointer event. Native mouse, touch, hover, wheel, and
  keyboard event sources are still pending.
- 2026-05-21: Added draft key and committed-text routing. The runtime can now
  look up focused widget state and queue portable key/text events before real
  native keyboard and IME event sources are wired.
- 2026-05-21: Added FIFO event polling beside event draining. Future
  `events.poll()` wiring can now consume one queued event at a time without
  depending on batch-only adapter internals.
- 2026-05-21: Added host window event routes for close requests, resize, and
  focus changes. A native backend can now report the basic window lifecycle
  events needed by the first real event loop without directly mutating app
  state.
- 2026-05-22: Added draft theme and scale event routes. Native backends now
  have a shared place to report system dark-mode changes and DPI scale changes,
  which matters before we make the first real window draw pixels.
- 2026-05-22: Split the named `WindowAdapter` boundary from the larger
  `UiAdapter` path. Window lifecycle and host-window events are now the lower
  layer; widgets, input, and clipboard sit on top. The host adapters also
  report their planned native window backend, so the next AppKit and winit work
  has a checked target.
- 2026-05-22: Added the native handle handoff path. A native backend can now
  attach an opaque AppKit, winit, or Win32 handle to the stable Krate
  `WindowId`, look it up later, and detach it. macOS has the first AppKit
  handoff method. This is still not a visible native window, but it removes the
  awkward part before the real AppKit window code lands.
- 2026-05-22: Added the first opt-in AppKit window prototype. The macOS adapter
  can now create an owned `NSWindow` on the main thread, bind its raw handle to
  the Krate window id, and show it through the same shared window path. This
  is not the default runtime yet, because native event capture and drawing are
  still next.
- 2026-05-23: Added the AppKit redraw bridge. Redraw requests now travel
  through the delegate-shaped native event state and the shared
  `WindowAdapter::request_redraw` queue, so the first drawing surface will not
  need a separate paint path.
- 2026-05-23: Added the Rust-side AppKit delegate callback bridge. The real
  Objective-C delegate can now stay thin: it only has to translate AppKit method
  calls into `AppKitWindowDelegateCallback`, while tested Rust code handles
  resize, focus, scale, redraw, close, and snapshot routing.
- 2026-06-07: Added AppKit draw-surface state. It tracks the window size,
  display scale, clear color, redraw requests, and frame number before the real
  `NSView` painter lands. This keeps the next pixel-painting step small and
  keeps redraw requests on the same tested event path.
- 2026-06-20: Added the first AppKit draw view surface. The prototype window
  can now attach an owned `NSView`, set a visible clear color, mark the view as
  needing display, and record a first frame snapshot. This is still an opt-in
  native smoke path, not the default runtime.
- 2026-06-20: Added the first real AppKit window delegate object. It implements
  `NSWindowDelegate`, records native close, resize, focus, and backing-scale
  callbacks into a small FIFO queue, and lets `AppKitWindowSession` drain them
  through the tested Rust bridge. This still does not make AppKit the default
  runtime path.
- 2026-06-20: Added the first AppKit event-loop step driver. It can process one
  non-blocking native tick by refreshing the session snapshot, draining queued
  delegate callbacks, and queueing redraw through the shared event stream. This
  still stays opt-in; the next step is runtime selection without breaking
  headless CI.
- 2026-06-20: Added selectable AppKit prototype runtime mode. The normal
  `Phase3UiRuntime::with_host_adapter` path still uses the headless adapter,
  while `Phase3HostUiMode::NativePrototype` can select the macOS AppKit
  prototype adapter and report native window plus native event-loop support.
- 2026-06-20: Added the shared event-loop pump boundary. `UiAdapter` and
  `Phase3UiDispatcher` can now ask a host adapter to process one non-blocking
  UI tick. Headless adapters return no native work; AppKit maps its prototype
  tick into a common report.
- 2026-06-21: Added the selectable AppKit runtime smoke command. The runtime
  can now request the native prototype path, create and show a real AppKit
  window, pump one shared event-loop tick, inspect the report, and close the
  window in one main-thread local proof path.
- 2026-06-21: Added Linux and Windows Winit prototype boundaries. Both host
  adapter crates now have explicit prototype adapter types, native-handle
  handoff helpers, guarded discovery functions, runtime selector wiring, and
  boundary checks. They still do not create real native windows; that is the
  next step.
- 2026-06-21: Added the shared Winit session owner scaffold. Linux and Windows
  prototype adapters can now track Winit sessions, attach the handle token,
  apply prepared native event-loop steps, route resize/focus/scale/redraw/close
  events through the shared queue, and remove the session on close. Real winit
  OS window creation is still pending.
- 2026-06-23: Added the Winit callback collector bridge. Linux and Windows
  prototype adapters can now record Winit-shaped callbacks in FIFO order, count
  pending callbacks, drain them through the normal event-loop pump, and keep
  that callback queue tied to the tracked Winit session. Real Winit OS window
  creation is still pending, but the callback handoff point is now tested.

---

## Closing

Phase 3 is the phase where Krate either becomes a real platform or becomes another also-ran cross-platform framework. Every prior phase was infrastructure; this one is the first user-visible surface, and users judge platforms in the first thirty seconds. A button that feels slightly wrong, a scroll that lags, a menu that doesn't open where expected — these are not papercuts, they are the reasons platforms die.

Take the sixteen weeks. Build the widget protocol twice if the first draft feels off. Run the screen reader on `krate-notes` every Friday. Test IME on real hardware with real users. Measure frame times. Fix what's slow. When a native widget fights the abstract widget, side with the native widget — our abstract layer exists to serve users, not developers.

When Phase 3 ships, someone who has never heard of Krate will open `krate-notes` on their MacBook and then on their Windows work machine and think "oh, nice, is this made by Apple? Microsoft?" That confusion is the highest compliment this phase can earn. Earn it.

— end of document —
