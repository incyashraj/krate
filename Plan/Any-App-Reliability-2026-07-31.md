# From "it works" to "any app": a measured audit and the plan

**Date:** 2026-07-31
**Author:** development
**Question this answers:** what stands between Krate today and a product an
early user, an AI company, or an investor would call reliable — and what is the
fastest honest route there.

---

## 0. The verdict in one paragraph

The foundation is genuinely good and is not the problem. One component runs on
three operating systems, the permission wall is real and enforced before host
access, AI authoring works, and `krate port` already refuses to promise
conversions it cannot deliver. What is thin is the **surface**: the set of
things an app is allowed to do. Krate declares 17 widgets and renders 6 on all
three systems. It has no key-value store, no database, no secrets, no
notifications, no way to open a URL. Those are not exotic — they are what
ordinary apps are made of. The gap between "Krate runs a checklist" and "Krate
runs your app" is almost entirely this surface, and the current cost of closing
it is eight hand-edited places per capability. That cost, not the architecture,
is what makes the system feel taped together.

The plan below does three things: replace the one-by-one grind with a **capability
pack** shipped as one coherent release, make unknown capabilities a **first-class
recorded outcome** instead of a dead end, and put a **measured parity table** in
front of users so the product stops over-claiming.

---

## 1. What I measured, not what I assumed

Every number here came from reading the current tree, not from the plan docs.

### 1.1 The three-OS promise does not currently hold for UI

| | Widgets |
| --- | --- |
| Declared in WIT | 17 |
| Rendered on macOS (AppKit) | 9 |
| Rendered on Linux/Windows (drawn path) | 7 |
| **Working on all three** | **6** |

The six that work everywhere: `button`, `list-view`, `stack`, `text`,
`text-area`, `text-field`.

Worse than the count is the **asymmetry**, which is invisible to an app author:

- macOS only: `checkbox`, `scroll`, `slider`
- Linux/Windows only: `tree-view`
- Implemented nowhere: `canvas`, `grid`, `image`, `progress`, `radio`,
  `switch`, `tabs`

An app that uses a checkbox works on the machine it was built on and silently
degrades elsewhere. That is precisely the failure mode Krate exists to abolish,
and it is currently reachable through the front door. This is the single most
damaging fact in this report, because it contradicts the core promise rather
than merely limiting it.

### 1.2 Host functions that accept a call and do nothing

Six declared areas answer `Unsupported` at runtime:

```
window state changes      menus
system dialogs            graphics
widget enable state       audio playback
```

A declared interface that returns `Unsupported` is worse than an absent one: the
app compiles, packages, passes the import check, ships, and fails in the user's
hands.

### 1.3 The capability vocabulary is 14 keys wide

```
fs.read  fs.write  net.connect  net.listen
io.args  io.stdin  io.stdout
ui.window  ui.clipboard  ui.dialog  ui.dropzone
gfx.gpu  audio.capture  audio.playback
```

Absent, and each one blocks a large class of ordinary applications:

| Missing | What it blocks |
| --- | --- |
| key-value / preferences | almost every app's settings |
| SQLite or structured storage | notes, trackers, anything with a list that grows |
| secrets / keychain | any app that signs in |
| notifications | anything that tells you something happened |
| open-url | "click here to open the docs" |
| local HTTP server | OAuth callbacks, local tools |
| menus, tray | desktop-native feel |

`fs.read`/`fs.write` are the only persistence, which pushes every app into
hand-rolled file formats. That is why the generated apps all look like the
checklist: it is close to the only shape the platform currently supports.

### 1.4 The analyzer sees ~10 things

`krate port` detects roughly ten categories (filesystem, process, network,
database, camera, microphone, notifications, tray, dynamic linking, plus
framework fingerprints). It is honest and it is read-only — both correct. But
for the classes it detects, most have **no capability to map onto**, so the
verdict is `unsupported` and the journey ends. The analyzer is not the
bottleneck; the surface behind it is.

### 1.5 The structural reason it is slow to add capabilities

```rust
pub fn module(&self) -> &'static str
pub fn action(&self) -> &'static str
```

Capabilities are compile-time `&'static str` constants. Adding one means editing
**eight** places: WIT contract, manifest parsing, policy matching, host
dispatch, the macOS adapter, the Linux/Windows adapter, the generated UAPI
reference, and the freeze lock. Nothing is data-driven. Nothing can be
registered at runtime. **This is the root cause of the pace**, and it is why the
one-by-one approach you want to abandon has felt necessary.

### 1.6 What is genuinely strong, and must not be broken

State this plainly, because the plan must protect it:

- the permission check happens **before** the adapter is called, not inside it;
- `krate port` never builds, executes, or edits the source;
- non-`krate:*` imports are rejected before packaging;
- the create pipeline proves the wall by withholding a grant and requiring
  refusal;
- the `no_std` guest SDK makes `wasi:*` leaks structurally impossible;
- the audit trail (plan hash, bundle hash, journeys) is already the shape Krate
  Cloud needs.

That list is a real moat. The work below adds surface **without** spending any of
it.

---

## 2. The three things to build

### 2.1 A capability pack, shipped as one release, not a drip

Stop adding capabilities singly. Define one **Desktop App Profile v1** that
covers the behaviour of the majority of small-to-medium desktop apps, and ship it
as a unit with parity on three systems as the entry condition.

The contents, chosen by what real apps use rather than what is easy:

**Tier 1 — storage and state.** Without this nothing real ports.
- `store.kv` — app-scoped key-value for settings and small state.
- `store.sql` — SQLite behind a Krate interface, never a raw host file. The app
  gets a query API scoped to its own database; it never learns a path.
- `store.secret` — OS keychain behind a capability, so sign-in becomes possible.

**Tier 2 — the widget set finished and made symmetric.** Complete the 11 missing
widgets on **all three** systems: `checkbox`, `radio`, `switch`, `slider`,
`progress`, `tabs`, `grid`, `scroll`, `image`, `tree-view`, `canvas`. Symmetry is
the deliverable, not count.

**Tier 3 — desktop integration.**
- `ui.menu`, `ui.notify`, `ui.open-url`, window state, dialogs, `ui.tray`.

**Tier 4 — connectivity.**
- a real HTTP client with honest errors and redirects;
- `net.serve` — a local HTTP listener with explicit port consent, which unlocks
  OAuth callbacks and local tools.

The rule that makes this a *pack* and not another drip: **nothing ships until it
works on macOS, Windows, and Linux, proven by a generated parity table.** A
capability that works on one system does not exist.

### 2.2 Self-healing: make "unknown" a recorded outcome, not a dead end

This is your idea and it is the right one, with one correction that makes it
safe. Krate must never let a port *invent* authority for itself — that would
destroy the property the whole product rests on. But it can absolutely stop
throwing away what it learns.

The mechanism:

```
port analysis hits something it cannot map
        |
        v
emit a capability REQUEST, not a failure
  { need: "system.notifications", evidence: [file:line],
    proposed: "ui.notify:app", os_support: unknown }
        |
        v
record it in the port plan and in the local registry
        |
        v
  ┌─────────────────────┴─────────────────────┐
  |                                           |
known-and-implemented                  unknown
  map it, continue                     three outcomes:
                                       a) a shim exists -> use it
                                       b) a safe fallback exists ->
                                          use it and DISCLOSE the change
                                       c) neither -> stop, and file the
                                          request with evidence
```

The self-healing is real but bounded:

1. **The registry is data, not code.** Move capabilities from `&'static str` to a
   loadable descriptor: name, arguments, policy shape, host binding, per-OS
   support. Adding a capability becomes editing one descriptor plus the adapter,
   not eight files. *This single change is what makes everything else fast.*
2. **Shims can be composed at port time.** A missing `store.kv` is implementable
   on top of `fs.read`/`fs.write` inside the guest. Krate can carry a library of
   such shims and apply them automatically — real self-healing, with zero new
   host authority.
3. **Unknowns aggregate into a roadmap.** Every request lands in one file with
   its evidence and how often it was hit. The most-requested unknown becomes the
   next capability. The system tells us what to build instead of us guessing.
4. **What it must never do:** grant itself a capability the user did not approve,
   invent a host binding, or silently drop behaviour. A dropped feature is a
   disclosed line in the port report, always.

This is the honest version of "self-healing": the system heals its *knowledge*
automatically and its *authority* only with a human in the loop.

### 2.3 Break one rule, deliberately: the escape hatch

You said to break rules for results. Here is the one rule worth breaking, and the
exact bounds that keep it from breaking us.

Today Krate rejects every non-`krate:*` import. That is what makes the guarantee
airtight, and it is also what makes most real code unportable. The proposal is a
**declared, visible, non-default** lane:

- an app may import a reviewed subset of WASI (streams, clocks, random, args,
  preopened dirs derived from grants, HTTP derived from `net.connect`) **through a
  Krate adapter that still policy-checks every call**;
- the bundle records the lane it used;
- the permission wall shows it in plain words: *"This app uses the compatibility
  lane. It still cannot do anything you did not allow."*;
- sockets, process spawning, and anything without a reviewed mapping stay
  rejected — no exceptions.

That is a rule broken in the useful direction: it multiplies what can port
without giving away enforcement. Capability checking still happens before any
host call.

---

## 3. Reliability is a product surface, not a feeling

Your instinct is right that this decides whether people trust us. Concretely,
for Krate, reliability means five measurable things:

1. **Never over-claim.** The parity table is generated from tests and published.
   If `tabs` does not work on Linux, the docs say so before a user finds out. We
   already have the honest instinct — `krate port` refusing bad conversions is
   exactly this — it just needs to reach the docs and the site.
2. **Every failure names the fix.** The permission wall already does this well:
   plain English plus a copy-pasteable command. Every error should meet that bar.
   That is the single most trust-building thing already in the codebase.
3. **Speed is a feature.** `krate create` is ~4s; a headless run was 8 hours
   before last week's fix. Set budgets — create under 10s, open under 1s — and
   fail CI when they regress.
4. **One journey, verified on three systems, published.** Not "it should work."
   The evidence exists in CI already; surface it.
5. **The port report is the product.** What ported, what changed, what was
   dropped and why, what could not be done. A user who sees an honest report
   trusts the tool that produced it more than one that silently succeeds.

---

## 4. Sequence

Ordered by what unblocks the most, soonest.

| # | Work | Why first | Done when |
| --- | --- | --- | --- |
| 1 | Capability registry becomes data | Cuts the cost of every later item from 8 files to 2 | A capability can be added by descriptor + adapter |
| 2 | Widget parity: finish 11 widgets on 3 OSes | Fixes the contradiction of the core promise | Generated parity table shows no asymmetry |
| 3 | `store.kv` + `store.sql` + `store.secret` | Unblocks the majority of real apps | An app with settings, a database, and a login ports |
| 4 | Shim library + unknown-capability requests | Turns dead ends into progress and a roadmap | An unmapped app yields a request file, not a refusal |
| 5 | Desktop integration (menus, notify, open-url, tray) | Makes ported apps feel native | Journeys pass on 3 OSes |
| 6 | WASI compatibility lane | Multiplies portable source | A third-party WASI CLI runs with no ambient authority |
| 7 | Parity table + budgets published | Converts internal honesty into visible reliability | Site shows the generated table |

Items 1 and 2 are the foundation; 3 is the one that changes what "any app" means
in practice.

---

## 5. What I recommend we do first, this week

**Item 1, then item 2.** They are unglamorous and they are the reason everything
else is slow. Making the registry data-driven is maybe two days of work and
permanently changes the pace of the project. Widget parity is the difference
between a promise we can defend and one that breaks on contact with a second
computer.

I would not start with the WASI lane, tempting as it is. It multiplies surface
before the surface is trustworthy, and a broken port on the compatibility lane
would damage trust more than a missing feature.

---

## 6. The honest statement of where we are

For the website, an investor deck, or an AI company's technical evaluation, this
is defensible and true today:

> Krate packages an application and its permissions into one file that runs on
> macOS, Windows, and Linux, and enforces the user's decision before the app
> touches anything. AI can author a working app from a request. `krate port`
> analyses an existing project and tells you honestly what will port, what must
> change, and what is not supported yet — it never converts blindly.

What we must **not** say until the work above lands: that any existing app can be
ported. Today the truthful claim is that apps built from the supported profile
port, and the profile is currently small. Every item in section 4 widens it.

The distance between those two sentences is the roadmap.
