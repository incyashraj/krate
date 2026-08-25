# Capability coverage matrix -- 2026-08-25

One row per shipped capability family. The question each column answers:
does the runtime do it, on which platforms, does the authoring pack TEACH it
(a snippet an agent can imitate, not a name in a list), does a reference app
use it, and has a from-scratch generated app proven it. A capability is only
real when every column is yes -- a "yes" runtime with a "no" teaching column
is invisible to every agent and therefore to every user (K-098).

Method: capability list extracted from `KRATE_CAPABILITY_SPECS`
(crates/manifest/src/lib.rs); teaching = name appears near a code snippet in
the generated KRATE_AUTHORING.md; examples = declared in `apps/*/manifest.toml`;
wiring = grepped per adapter/host. Date of audit: 2026-08-25.

## Fully real (runtime + teaching + example)

| capability | notes |
|---|---|
| ui.window:create | the core; taught everywhere, every GUI example |
| io.stdout / io.args | taught, ubiquitous |
| store.kv | 8+ examples; teaching adequate via examples |
| time.clock | taught, clock/timer examples |
| net.connect (net.http) | taught (section 2c backend pattern), fetch/curl examples |
| random.bytes | taught (getrandom bridge), diceroll example |
| fs.read / fs.write | taught, mdview/notes examples |
| ui.dialog (message/confirm/file-open/file-save/open-folder) | examples (mdview, tidy); teaching thin but present |
| ui.clipboard | all three adapters have clipboard.rs (the 2026-08 Linux-only gap is CLOSED); clip/mdview/notes examples |

## Real but INVISIBLE -- runtime works, pack never teaches it, no example uses it

| capability | wiring | what fixes it |
|---|---|---|
| audio.capture (microphone) | native capture in runtime (cpal), consent string exists | pack snippet + a voice-memo proof app |
| speech (transcribe) | WIT interface + whisper host fn shipped | same voice-memo app proves both |
| audio.playback | shipped, default-granted | pack snippet; games want it |
| ui.notify | macOS osascript, Linux notify-send, Windows adapter | pack snippet ("remind me" apps) |
| ui.open-url | all three (open/start/xdg-open) | pack snippet; pairs with 2c for OAuth-style backends |
| camera.capture | taught in pack, NO example app | example or proof app |
| store.secret | taught (2c), no example | fold into 2c example when one exists |
| store.shared | taught (2d), proven live (grocery A/B), no in-repo example | acceptable; example later |
| gfx.gpu | basic default-granted | teaching later; presenter-gpu work ongoing (K-112) |
| locale.info/format | shipped, clock example only | low priority |
| io.stdin / io.log / io.stderr / time.monotonic / time.sleep | shipped | catalog line is enough; low priority |

## Declared but NOT implemented

| capability | state |
|---|---|
| ui.dropzone | manifest validates it, consent wording exists ("accept files you drag onto it"), the PORT ANALYZER RECOMMENDS it -- and there is no WIT interface, no host function, no DroppedFile handling anywhere. K-175. |

## Platform gaps

| capability | gap |
|---|---|
| ui.menu:system | macOS only; Windows/Linux have no system-menu wiring |
| speech | absent from arm64-Linux and ARM-Windows release builds (whisper cross-compile, known) |

## Missing entirely (candidates, in priority order)

1. Text-to-speech -- we transcribe but cannot speak; accessibility, timers, kids' apps.
2. Printing -- later, by demand.
3. Background/scheduled runs -- apps live only while open; real design work, not a capability row.

## The order of work

1. K-175: implement ui.dropzone (or stop declaring it) -- a promise the consent
   sheet already makes.
2. One pack section teaching the invisible essentials with imitable snippets:
   mic+speech, audio playback, notify, open-url (paired with 2c so "sign in
   with your browser" backends work), plus the game-feel teachings the
   Ice Climber A/B surfaced.
3. Voice-memo proof app, generated from scratch, exercising mic + speech +
   store.kv -- the app IS the regression test and the example.
4. TTS as the next new capability, through the six gates.
