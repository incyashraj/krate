# What Krate cannot do yet

Read this before you spend a weekend. Everything below is current as of
2026-09-01 and is checked against the shipped capability list rather than
written from memory.

## Works today

| Area | What you get |
|---|---|
| Windows | One window, widgets, canvas, events, resize, DPI |
| Drawing | 2D canvas, GPU rendering, a 3D scene graph, text measurement |
| Audio | Playback and microphone capture |
| Speech | Speech to text, on device, behind the microphone grant |
| Camera | Frame capture, behind a permission prompt |
| Files | A scoped folder the app owns, plus a file picker for anything else |
| Network | HTTP and WebSockets, granted per host and port |
| Storage | Key-value, SQL, a secret store, and a shared store across machines |
| System | Clipboard, dialogs, notifications, open-a-URL, locale, time, random |
| Input | Keyboard, pointer, wheel |

## Not yet

| Missing | What it blocks |
|---|---|
| Text to speech | The answering half of a voice app |
| Printing | Invoices, labels, anything a business prints |
| Multiple windows | A dashboard with a separate controls window |
| File watching | Folder-driven tools that react to changes |
| Serial and USB | Arduino, ESP32, live sensor apps |
| Bluetooth LE | Heart-rate monitors, scales, beacons |
| Screen capture | Screenshot and recording tools |
| Tray and menu bar | Utilities that live at the edge of the screen |
| Scheduled wake | Reminders that fire with the app closed |
| Local server | Receiving webhooks, serving a page on the LAN |

## Deliberately never

Some things are absent by design and will stay absent:

- **Sending input to other applications.** A Krate app cannot type into
  your editor or click your browser.
- **Ambient full-disk access.** An app reaches the folder it owns and
  whatever a person picks in a dialog. There is no "read everything".
- **Processes that outlive their window.** An app is running or it is not.

These are the wall. A capability that could be added later is listed under
"Not yet"; anything here is a decision.

## The honest state of the sandbox

The capability boundary is enforced in code and it works. Krate is young,
and we do not claim production hardening against deliberately hostile
third-party code today. Run apps from people you know, or from the
gallery, or ones you built.

## Language and platform

- **Rust only** for building a real app. Go and TypeScript SDKs exist in
  the tree, but they are early CLI-shaped work with no UI bindings, so
  nothing you can ship a window from. The guest is a WebAssembly
  component, so other languages are possible; none are finished.
- **Desktop only.** macOS, Windows and Linux, on Intel and ARM. iOS and
  Android exist in the tree as reference ports and are not shipping.
- **Heavy GPU work is early.** The renderer draws real scenes, but a
  demanding 3D game is not what this runs well today.

## How to check for yourself

```sh
krate run app.krate --dump-caps     # what one app asks for
krate manifest capabilities         # every capability this runtime knows
```

That second command is the authority. This page is written from it, and
if the two ever disagree, the command is right.

If a capability you need is missing, that is worth telling us: what dies
weekly is what gets built next.
