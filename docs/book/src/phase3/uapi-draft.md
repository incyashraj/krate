# Phase 3 UAPI Draft

This page explains the first Phase 3 contract in simple terms.

Phase 2 proved that a portable app can ask Layer36 for files, network, time,
locale, and terminal input or output. Phase 3 adds the first desktop app shape.
The app still does not call macOS, Windows, or Linux directly. It asks Layer36
for a window, input events, drawing, and audio. The host adapter translates
those requests.

```mermaid
flowchart LR
    APP["Portable app component"] --> GUI["layer36:app gui world"]
    GUI --> UI["UI<br/>window, tree, events"]
    GUI --> GFX["Graphics<br/>2D canvas, GPU surface"]
    GUI --> AUD["Audio<br/>playback, capture"]
    UI --> HOST["Host adapter"]
    GFX --> HOST
    AUD --> HOST
    HOST --> OS["Native desktop OS"]

    classDef built fill:#d9fbe3,stroke:#16833a,color:#102a17,stroke-width:2px;
    classDef draft fill:#fff3bf,stroke:#b7791f,color:#2d2100,stroke-width:2px;
    classDef pending fill:#eeeeee,stroke:#999999,color:#777777,stroke-width:1px;

    class APP,GUI draft;
    class UI,GFX,AUD draft;
    class HOST,OS pending;
```

## What Exists Now

The first draft lives under `wit/layer36/phase3`.

It defines:

- `layer36:app@0.2.0` with a `gui` world
- `layer36:ui@0.1.0` for windows, widget trees, events, dialogs, clipboard, and menus
- `layer36:gfx@0.1.0` for simple 2D drawing and an early 3D surface shape
- `layer36:audio@0.1.0` for playback and capture stream shape

The checker is:

```bash
scripts/check-phase3-uapi.sh
```

That checker proves the WIT parses, the expected packages are present, the
`gui` world imports the intended interfaces, the world exports `run`, public
names follow the repo style, and the new error types include
`permission-denied`.

The CLI also recognizes this manifest world now:

```toml
[app]
id = "com.example.notes"
name = "Notes"
version = "0.1.0"
entry = "notes.wasm"
world = "layer36:app/gui@0.2.0"
```

`layer36 manifest check` accepts that world and labels it as a Phase 3 GUI
draft. `layer36 run` does not launch it yet. It exits early with a clear message
that the GUI runtime is not implemented. This is intentional. We want the
manifest and tooling path to be real before we add windows.

## What It Does Not Mean Yet

This is not a finished desktop UI layer.

It does not open a real window yet. It does not draw a real frame yet. It does
not mean the API is frozen. It is the first contract shape that lets us build
the runtime and host adapter work in the right direction.

## Why Start Here

For Layer36, the contract is the platform boundary. If the contract is vague,
each host adapter will drift. Starting with WIT gives us one shared language for
the app, runtime, SDKs, and host adapters.

The next proof should be small:

1. A host adapter prototype that can create one window.
2. A simple event loop.
3. A tiny draw call that paints something visible.
4. A small notes app skeleton that uses the same path.
5. A first pass at GUI capability names.
