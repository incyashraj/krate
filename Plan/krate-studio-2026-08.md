# Krate Studio: the app where you talk to an AI and watch your app appear

A desktop app. Chat on the left, your app running on the right. You ask for
something, you watch the AI work, the app rebuilds in front of you, and you
click it. Publishing is a button, not a command.

Written 2026-08-07, after shipping v0.1.0.

---

## Why this, and why now

The TUI works. Someone can type `krate`, describe an app, and get a file that
opens on three operating systems. But the whole middle of that is a five to
twelve minute silence, and the app appears only at the end, in a separate
window, disconnected from the conversation that produced it.

Three things are wrong with that shape, and none of them are fixable in a
terminal:

**You cannot see the app while you talk about it.** The thing you are
describing is not in front of you. You say "make the button blue", wait six
minutes, then a window opens somewhere and you compare it against memory.

**You cannot interrupt.** The AI commits to an interpretation in the first
thirty seconds and you find out whether it was right six minutes later. Every
misunderstanding costs a full round trip.

**The terminal is the wrong audience.** The people who most want "describe an
app and get one" are the people least likely to have a terminal open. Every
person who has tested this so far has been a developer. That is a sampling
problem, not a product truth.

This is also the thing that makes the demo. "An AI wrote this app while I
watched, and here is the file" is a story you can show in ninety seconds.

### What this is not

Not a code editor. Not an IDE. Somebody who wants to read the Rust can open
the folder in their own editor -- the source is right there, and that path
already works. Studio is for the person who wants the app, not the code.

Not a replacement for the TUI. `krate` in a terminal stays, works, and is what
CI and scripts use. Studio is a second front door for a different person.

---

## What already exists

Worth being precise about, because it changes the estimate. Most of the hard
parts are done.

| Piece | State | Where |
|---|---|---|
| Driving an AI headlessly | done, 5 providers | `crates/cli/src/agent_provider.rs` |
| Async job registry with status polling | done | `crates/mcp/src/jobs.rs` |
| Streaming an agent's tool calls into plain English | done | `agent_provider::progress_line` |
| A progress channel across a process boundary | done | `PROGRESS_PREFIX`, v0.1.0 |
| Six-stage verdict on generated code | done | `krate check-app` |
| Fast inner-loop check (2s vs 17s) | done | `check-app --no-run` |
| Building an app from a request | done | `create_krate` |
| Editing an existing app in place | done | `revise_app_for_tui` |
| Rendering an app's window to a PNG headless | done | `krate run --shoot` |
| Publishing to Krate Cloud | done | `krate publish` |
| Attaching files to a request | done | v0.1.0 |
| Capability sandbox and the permission wall | done | `crates/policy`, `crates/runtime` |

**What does not exist:** a window that is not a terminal, and a way to show a
running app *inside* another app's window.

That second one is the only genuinely new engineering, and it is the piece I
would build first, because everything else is already proven and it is the one
that could turn out to be hard.

---

## The shape

```
┌──────────────────────────────┬───────────────────────────────┐
│  CHAT                        │  YOUR APP                     │
│                              │                               │
│  you: a habit tracker with   │   ┌───────────────────────┐   │
│       a weekly chart         │   │                       │   │
│                              │   │   (the app, running,  │   │
│  krate: reading the pulse    │   │    clickable)         │   │
│         example              │   │                       │   │
│         writing the code     │   └───────────────────────┘   │
│         ▸ 34 lines so far    │                               │
│         compiling            │   ● running   ⟳ rebuild       │
│         ✓ opens and draws    │                               │
│                              │   ┌─ what it can reach ────┐  │
│  [ your message ]        ⏎   │   │ ✓ its own window       │  │
│  ⏸ stop   📎 attach          │   │ ✓ save to one folder   │  │
│                              │   │ ✗ nothing else         │  │
│                              │   └────────────────────────┘  │
│                              │   [ Send to someone ] [ ⋯ ]   │
└──────────────────────────────┴───────────────────────────────┘
```

Left is a conversation. Right is the truth. The permission panel sits under the
app deliberately: it is Krate's actual differentiator and it should be visible
while somebody uses the app, not buried in a dialog they dismiss.

---

## The one hard problem: showing the app inside Studio

Everything else is plumbing. This is the decision the whole project turns on,
so it gets its own section.

A Krate app today creates a **top-level OS window** through the adapter. Studio
needs it to appear *inside* a pane. Four ways to do that:

### Option A -- render to a texture, no OS window

The runtime already paints into a CPU buffer (`CanvasSurface`), and `--shoot`
already publishes that buffer as a PNG. So the pixels exist in memory before
any window is involved.

Studio asks the runtime for the buffer each frame and draws it into its own
pane. Input goes the other way: Studio's clicks and keys are injected as
`Event::Pointer` and key events on the same path `--shoot`'s usability driver
already uses to click and resize apps headlessly.

- **Works the same on all three systems**, because it never touches a native
  window at all.
- **Reuses the two things already proven**: the raster path and the synthetic
  input the usability checker drives apps with.
- Costs a buffer copy per frame. At 640x480 that is 1.2 MB a frame, which is
  nothing next to the 400M pixels/second the rasterizer already sustains.
- The app does not get a real window, so anything genuinely OS-level (a native
  file dialog) has to be brokered. Those already go through capabilities, so
  there is a place to put that.

### Option B -- embed the child's native window

Reparent the app's real OS window into Studio's pane: `NSView` on macOS,
`SetParent` on Windows, XEmbed or a subsurface on Linux/Wayland.

- The app is genuinely itself, native input, no copying.
- **Three completely different implementations**, and the Wayland one is
  actively unpleasant. This is the kind of surface that produced K-036, K-042
  and K-047 -- one per platform, each found only by running it there.

### Option C -- keep it a separate window, just coordinated

Studio owns the chat; the app opens beside it and Studio positions it.

- Almost no work. Ships in days.
- But it is the current experience with extra steps. The point is seeing them
  together.

### Option D -- Studio is a web UI, app rendered to a canvas

Chat in a webview, app frames pushed as image data.

- Fast to build a good-looking UI.
- Adds a browser engine to a product whose whole pitch is "one small file, no
  runtime to install". Wrong for the brand and heavy for the download.

**Recommendation: A, with C as the escape hatch.**

Option A reuses two paths that already exist and work identically everywhere.
Its risk is latency and input fidelity, and both are measurable in a day --
which is exactly what Phase 0 below is for. If it fails, C ships a lesser
version of the product rather than nothing.

I want to be honest that A has a real unknown: a game running at 60fps with a
buffer copy and cross-thread handoff per frame may feel soft. Measure before
committing.

---

## Phases

Ordered so the riskiest unknown is settled first and each phase is usable on
its own.

### Phase 0 -- prove the pane (2-3 days)

No chat, no polish. One window, hardcoded app, one question: **can we show a
running Krate app inside another window and click it, at an acceptable frame
rate, on all three systems?**

Build: a minimal winit window that runs a `.krate` through the runtime,
requests the raster each frame, blits it, and forwards mouse and keyboard.

Ship criteria:
- `krate-nova` (the canvas game) is playable in the pane at >30fps on this Mac
- Clicking a button in `krate-checklist` inside the pane does what clicking it
  in its own window does
- The same binary does both on Windows and Linux

**If this fails, stop and reconsider.** Everything after it assumes the pane.

### Phase 1 -- the app pane, for real (1 week)

Around a working pane: run, stop, restart. The permission panel, from the
manifest, live. Resize handling. Errors from the app shown in the pane rather
than a log.

Usable on its own: a `.krate` viewer with a visible permission wall. That alone
is worth shipping.

### Phase 2 -- chat and the build loop (1.5 weeks)

The left half. A conversation that drives `create_krate` and
`revise_app_for_tui` -- both existing functions.

Live progress comes from the channel v0.1.0 already added: today it drives a
terminal display, and it will drive a chat transcript instead. The provider's
`progress_line` already turns tool calls into readable sentences.

The rebuild loop: when a build finishes, the pane restarts the app. The app is
already rebuilt into the same directory by the existing edit path, so this is
a restart, not new machinery.

**Stop** cancels the running agent -- the child process is already tracked and
already killed on timeout, so the kill path exists.

### Phase 3 -- the things that make it feel alive (1 week)

- **Attachments**: drag a screenshot onto the chat. The parsing and staging
  already exist from v0.1.0; this is a drop target wired to them.
- **Editing a message**: change what you asked and re-run from there.
- **Thinking, shown honestly**: Claude streams tool calls, so show them. Grok
  does not, so say so -- the `reports_progress` flag added in v0.1.0 already
  carries this distinction.
- **Diff view**: what changed in the last edit. `revise` works in place, so a
  git-style snapshot before each edit gives this cheaply.
- **History**: every version of the app, restorable. Same snapshots.

### Phase 4 -- share without leaving (3-4 days)

Publish, copy link, reveal the file, send to someone. All wrapping
`publish_bundle`, which exists. The one new thing is a QR code for the link,
because the natural next move after making an app is showing it to somebody
holding a phone.

### Phase 5 -- the front door (1 week)

What Studio looks like with nothing open: recent apps, example apps to open and
poke at, and a first-run flow that gets an AI connected without a terminal.

This is also where the toolchain install lives. Studio can show a real progress
bar for the 3 GB of Rust and Build Tools, which the terminal does badly.

---

## What to build it in

**Rust, winit, and the painter that already exists.**

Studio's own UI is drawn with `krate_adapter_common::painter` -- the same code
that draws every Krate app. That is not purity for its own sake, it is three
concrete wins:

1. **Studio is a Krate app.** The strongest possible proof that the runtime is
   good enough for real software, and the thing that makes every complaint
   about Krate's UI a complaint we feel first.
2. **No new dependency.** No Electron, no webview, no second UI toolkit to keep
   working on three systems.
3. **One rendering path to fix.** A text bug fixed for Studio is fixed for
   every generated app.

The honest cost: our painter has no text input widget worth the name, no
scrollable text view, and no list virtualisation. A chat transcript needs all
three. That is real work -- but it is work that makes *every* Krate app better,
which an Electron shell would not.

If that proves too slow, the fallback is egui: immediate-mode, pure Rust, uses
winit already. Not the webview.

---

## Risks, and which ones actually worry me

**The pane may feel soft.** A copy per frame plus a thread handoff might make a
game feel laggy in a way a native window would not. *Settled in Phase 0, before
anything is built on it.*

**Text input is genuinely missing.** Selection, IME for non-Latin input,
copy/paste. This is the biggest chunk of unglamorous work in the plan and the
easiest to underestimate. *Consider egui for Studio's own chrome if it bites.*

**Scope.** Every item on this list is defensible and the whole list is months.
Phases 0-2 are the product; 3-5 make it good. Ship 0-2 and judge.

**It competes with the thing that is working.** The TUI just shipped and has
users. Studio must not divide attention from bugs found in v0.1.0. *Studio
waits for a week of v0.1.0 being stable in the field.*

**Per-user AI cost stays the user's.** Studio drives the AI the person already
pays for, exactly as the TUI does. Nothing here changes that, and nothing here
should -- an "included AI" is a business model, not a feature, and it is not
one we can fund today.

---

## How this connects to the goals

`GOALS.md` names G5 -- ten people outside this machine making an app and
sending it -- as the only gate that cannot be faked.

Every person who has tested Krate so far has been a developer, because the
front door is a terminal. Studio is the most direct attack on G5 there is: it
removes the terminal from the path entirely.

It also serves G4 (the outsider path works cold): a first-run flow with a
visible progress bar for the toolchain is a much better cold start than a
terminal that appears to hang for three minutes.

---

## What I would do first

Phase 0, and nothing else, until the pane question is answered. Two or three
days for a definite yes or no on the one thing that could sink the project.

Everything after it is assembly of parts that already work.
