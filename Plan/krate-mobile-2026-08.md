# Krate on smartphones: the player goes where the people are

Written 2026-08-09. The question: how does Krate work on phones? The answer
has a shape most plans for "mobile support" get wrong, so it goes first.

## The one decision that matters: phones receive, desktops make

Our own two-paths split (User path: open and run; Maker path: build and
share) decides this. The maker sits at a desktop -- the AI CLI, the
toolchain, the TUI all live there and should. But the *receiver* of a
shared app is on a phone more often than not: a .krate arrives in WhatsApp,
a krate.tech/a/ link arrives in a group chat, and today both are dead ends
on the device where they were opened. Every shared link whose recipient is
on a phone is a lost first impression -- and sharing is the product.

So "Krate on smartphones" means **the player**: open a .krate or a link,
show the permission wall, run the app. Not the CLI, not authoring, not a
port of the TUI. One job.

## Why this is cheaper than it sounds (measured against our stack, not hoped)

The runtime's split does most of the work already:

- **The painter is pure CPU Rust.** vello_cpu + parley + fontique in
  adapter-common -- every modern-UI primitive we just built (SDF cards,
  shadows, gradient stops, styled text) is portable pixels with no GPU
  dependency. krate-gram's ~6.5 ms/frame is *host-side native* raster; it
  comes along unchanged.
- **Apps already handle the phone's window model.** ui.window on a phone is
  one fullscreen surface. W13 forced every app to lay out from canvas-size
  and survive resize; K-067 made coordinates logical pixels with the host
  owning display scale. A notch is just insets (M3).
- **The wheel event was designed for this.** Its contract says the host
  converts whatever the platform gives into logical-pixel deltas. A touch
  pan is one more thing the host converts. krate-gram's momentum scrolling
  works by touch the day the adapter synthesizes wheel deltas from drags.
- **The guest is small; the host is fast.** App logic is a few hundred KB
  of wasm doing arithmetic; the heavy work (raster, text shaping) is native
  host code. This is what makes the iOS no-JIT constraint survivable.

What does NOT come along: whisper/speech (native C++, heavy -- degrade to
Unsupported like clipboard on macOS does today), dialogs (phones have no
folder picker in our sense -- M3 decides the mobile shape of
pick-is-the-grant), and the six-platform release matrix grows new targets.

## M0 -- measure demand before writing device code (1 day, do it now)

The hub sees the user agent of every krate.tech/a/ link hit. Count mobile
UAs as their own blob in the krate_usage dataset, and put a soft "Krate
runs on computers today -- send this to your laptop" line on the /a/ page
when the UA is mobile, with a copy-link button.

That gives a number: what fraction of shared-link opens die on a phone
today. The campaign is about to drive real traffic; M0 turns that traffic
into the evidence that ranks M1 against everything else. Data first, then
device code -- the same rule as the GPU decision.

## M1 -- Android player (first, because everything is allowed)

Android is the easy half and proves the whole path:

- **Engine:** wasmtime 43 JIT runs fine on aarch64 Android. No changes.
- **Adapter:** `adapter-android` in the adapter-linux mold: winit 0.30
  (android-activity backend) + softbuffer NDK surface + the shared painter.
  Touch: taps and drags become pointer events; pans synthesize wheel
  deltas. Fullscreen window, canvas-size layout.
- **The wall:** a bottom sheet before first run, same plain words, same
  deny-still-opens behavior. This is the soul of the product; it ships in
  M1 or M1 does not ship.
- **Entry points:** file association for .krate (share sheet, Files,
  downloads) and an App Link for krate.tech/a/ so tapping a shared link
  opens the app running.
- **Distribution:** sideloaded APK on krate.tech first (our audience can),
  Play Store second. Play's executable-code rule has an explicit carve-out
  for code run in a VM or interpreter with no direct OS access -- a wasm
  component behind a capability wall is the textbook case.
- **Acceptance test (testing like a stranger):** a phone that has never
  seen Krate taps a link someone sent, sees the wall, runs krate-gram, and
  scrolls it by thumb at 60fps. Mid-range phone, not a flagship.

Estimate: 2-4 weeks of agent time. New CI target, player versioned and
shipped separately from the CLI (the player is an app, not the dev tool).

### M1 status 2026-08-09: the whole stack cross-compiles, the .so exists

Five gates passed in one sitting, each verified by compiling, not hoped:
the runtime (wasmtime 43 JIT, cpal, everything but speech and rfd) checks
for aarch64-linux-android; the painter (vello_cpu + parley + fontique)
checks; `crates/adapter-android` -- the Linux adapter's winit pump model
with the AndroidApp deposited from android_main instead of a display-server
probe -- checks for the target and passes its 38 unit tests on desktop;
the runtime dispatches to it behind `cfg(target_os = "android")`; and
`crates/player-android` builds to a release `libkrate_player.so` (19 MB,
ARM64, embeds krate-gram as the first-light app). `scripts/build-android.sh`
holds the whole fragile env (rustup-not-Homebrew rustc, NDK compilers, API
26 floor -- cpal's AAudio needs 26+). rfd is desktop-only now; Android
dialogs answer as cancelled until the document picker (M3).

**First light achieved, same day.** `scripts/package-android.sh` builds the
7.9 MB APK by hand (aapt2 + zipalign + apksigner, hasCode=false, no
Gradle); a Pixel 7 AVD ran it; the screenshot shows Krategram rendering --
wasmtime ran the component, the wall granted, the painter rasterized,
softbuffer hit the NativeActivity surface. Two launch bugs found by
probing stderr through logcat, both fixed: window creation pumped once and
gave up before Android delivered Resumed (now a bounded pump-wait), and
Android's first winit window is id 0, which the shared handle validation
reads as null (now offset locally; the desktop null-check stays). Two
bugs found by looking at the pixels, filed: K-088 (blit ignores display
scale -- the K-067 lesson, again, on a new surface) and K-087 (krate-gram
hardcodes 390x720 instead of reading canvas-size -- example-bug). Both
fixed and emulator-verified: full-bleed, native-sharp, desktop untouched.

**Touch and phone behaviors, all device-verified.** A finger becomes
Krate's existing event model, decided by movement: stay inside the 8 px
slop and it is a tap (pointer press + release at the point -- double-tap
detection in apps sees two of them), move past it and every further move
is a synthesized wheel delta, so app-side momentum gives fling for free.
Extra fingers are ignored rather than corrupting the gesture. The system
back button (winit spells it BrowserBack, found by probing, not guessing)
maps to close-requested, the guest exits, and the player ends its process
so no blank activity shell lingers. Suspend drops every softbuffer
surface (Android tears the native window down); the draw path lazily
recreates on resume -- home-and-return keeps drawing. Verified on the
Pixel 7 AVD: swipe scrolls, fling glides, double-tap likes (adb's ~0.4 s
injection gap needed a temporarily widened window to prove it; human
taps are ~0.25 s and the app keeps 400 ms), back exits, resume redraws.
Not done, deliberately: long-press-as-secondary (the pointer-sample
pipeline is primary-only), pinch (no WIT event for it), and the
on-screen keyboard (M3, with insets and lifecycle events for guests).

**M1 COMPLETE 2026-08-10: link -> fetch -> wall -> run, all on device.**

The wall is itself a Krate guest (apps/krate-wall): a bottom sheet drawn
by the same renderer the apps use -- rationale-first rows, cap names
dimmed beneath, required rows locked on, the rest togglable, Open and
Cancel. The player passes the capability lines in through args and reads
one decision line back from captured stdout; it owns both sides of that
pipe, so the sheet cannot be bypassed and cannot be lied to. Deny still
opens: whatever was toggled off is simply absent from the session policy.
Sequential guests in one process (wall window, then app window) work --
proven before the design was committed to.

Intents are read over JNI with no Java in the app: the VM and Activity
handles come from android-activity (ndk-context is a trap here -- it
stores the Application, which has no getIntent; found by making JNI
describe its exception). Three ways in: an https hub link (fetched with
rustls, 6 MB cap), a content:///file:// .krate (read through the
ContentResolver), or a plain launch (the embedded demo, which faces the
same wall). The manifest claims krate.tech/a/ links and .krate files;
without a release signing cert there is no assetlinks auto-verify yet,
so Android offers a chooser -- the honest default, noted for M2-era
polish.

Verified on the Pixel 7 AVD, every leg: a tapped hub link fetched "Add
dvd screensaver" -- a real bundle a stranger could have published -- and
its own manifest rendered as the wall ("Draw the bouncing DVD logo on a
canvas"); Open ran it; Cancel on a fresh launch printed "the person said
no; nothing runs" and the process exited -- fail closed is the wall's
resting state, a crashed sheet grants nothing. The plain launch walls
the demo the same way. Desktop: 1102 tests, untouched.

What M2 (iOS) inherits ready-made: the player flow, the wall guest, the
bundle path, and every touch behavior -- all of it above the adapter
line.

## M2 -- iOS player (second, because Apple makes you earn it)

Two hard constraints, both survivable:

- **No JIT on iOS.** Executable pages are forbidden. wasmtime 43 ships
  Pulley, its portable interpreter backend, for exactly this. Guests are
  small and the painter is native, so the interpreter tax lands on the
  lightest part of the system. First task of M2 is the measurement:
  krate-gram under Pulley on an iPhone -- if frame logic stays under a few
  ms, the whole question is settled. (AOT .cwasm still needs executable
  mappings, so Pulley is the path, not AOT.)
- **App Store review.** Since 2024, guideline 4.7 explicitly allows apps
  that run downloaded mini-apps and games in an interpreter (the retro
  emulators went through on it). Our pitch is stronger than an emulator's:
  every app declares what it wants, the runtime enforces it, and the
  player can show a reviewer the wall. TestFlight first, App Store under
  4.7, EU alternative distribution as the documented fallback if review
  stalls. Needs the Apple developer account (also unblocks K-063's
  Gatekeeper cert wish -- one purchase, two problems).

Adapter: no winit -- mirror the macOS adapter's approach with objc2-ui-kit,
blitting the painter's buffer into a CALayer the way adapter-macos already
blits into an NSImageView (the autoreleasepool lesson from the 46 GB leak
carries straight over). Estimate: 6-10 weeks including review roulette.

## M3 -- the contract grows mobile truths (with M1/M2, not after)

Small WIT additions, all degrading to no-ops on desktop so apps never fork:

- **Insets:** safe areas (notch, home bar) as a record on window events.
- **Lifecycle:** a suspended/resumed event; the OS kills backgrounded apps,
  so store.kv writes flush on suspend.
- **Keyboard:** focusing a text widget raises the on-screen keyboard; the
  tree's text_cursor already models the state.
- **Pick-is-the-grant on mobile:** the document picker is the platform's
  native equivalent of our folder dialog. Same token design, same wall
  line. The apps written against open-folder should work unmodified.

## M4 -- making on the phone (explicitly not now)

The Tier B shape (client describes, cloud builds) is the only way a phone
can be a maker, and it is gated twice: on the open-core boundary decision
(no Krate Cloud buildout before the gates), and on G5 evidence that
desktop makers exist first. The player app grows a "make" tab only after
both. Written here so nobody mistakes silence for it being forgotten.

## What we deliberately do not do

- **No web player fork.** Reimplementing the host in the browser doubles
  the surface that has to tell the truth. Revisit only if both stores
  block us -- the stores, not the effort, are the trigger.
- **No GPU requirement, same as desktop.** The CPU painter is the
  portability story on phones too.
- **No phone-local toolchain.** Ever, probably.

## Sequencing against the campaign

The campaign (1,000 users) is desktop and is live-or-imminent. M0 rides it
-- one day, and the campaign's own traffic produces the demand number. M1
starts after the campaign's first week is digested (stranger bugs come
first; that loop is the marketing). M2 starts once M1's player has real
opens in the dataset. Each phase has a number in front of it, so the plan
stops if the numbers say stop.
