//! The context pack an AI author is handed: `KRATE_AUTHORING.md`.
//!
//! The authoring agent used to be given a paragraph of rules and left to guess
//! everything else -- what functions exist, what capabilities a manifest may
//! name, which of the shipped apps is the closest example to adapt. Guessing is
//! the predictable outcome of asking someone to write against an API they
//! cannot see, and it is why the agent was template-bound.
//!
//! This module generates one Markdown file, dropped into the app directory, so
//! the agent builds against real facts:
//!
//! 1. the SDK API surface (every callable `krate::*` function),
//! 2. the capability catalog (every manifest capability name and what it does),
//! 3. the no_std discipline (the rules, with the why),
//! 4. the GUI world's interfaces (ui / gfx / audio / speech), and
//! 5. an index of the shipped example apps, by shape.
//!
//! Sections 1, 2, 4, and 5 are generated from the same sources the runtime
//! compiles against -- the SDK crate, the capability registry, the WIT, and the
//! `apps/` tree -- so the pack cannot drift out of date the way a hand-written
//! reference would. Section 3 is authored prose because it is guidance, not
//! data; the facts it points at (helpers, versions) live in the generated
//! sections beside it.

use std::path::Path;

use crate::sdk_reference;

/// The phase-3 GUI-world WIT, baked in at compile time so the generated GUI
/// reference always matches the interfaces this binary builds against. These
/// four are the interfaces an app reaches through its generated `bindings`
/// rather than through the `krate::*` SDK functions.
const UI_WIT: &str = include_str!("../../../wit/krate/phase3/deps/ui/ui.wit");
const GFX_WIT: &str = include_str!("../../../wit/krate/phase3/deps/gfx/gfx.wit");
const AUDIO_WIT: &str = include_str!("../../../wit/krate/phase3/deps/audio/audio.wit");
const CAMERA_WIT: &str = include_str!("../../../wit/krate/phase3/deps/camera/camera.wit");
const SPEECH_WIT: &str = include_str!("../../../wit/krate/phase3/deps/speech/speech.wit");

/// Generate the full `KRATE_AUTHORING.md` for an app in `app_dir`.
///
/// `app_dir` is used only to find the workspace root so the example index can
/// read the sibling `apps/` tree; when the apps tree cannot be found (a checkout
/// without it, a released binary), that one section is omitted rather than
/// failing -- the rest of the pack is the load-bearing part.
pub fn generate(app_dir: &Path) -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push_str(&sdk_surface_section());
    out.push_str(&capability_catalog_section());
    out.push_str(DESIGN_PATTERNS_SECTION);
    out.push_str(BACKEND_CLIENT_SECTION);
    out.push_str(SENSES_SECTION);
    out.push_str(GAME_FEEL_SECTION);
    out.push_str(NO_STD_SECTION);
    out.push_str(&gui_world_section());
    out.push_str(&example_index_section(app_dir));
    out
}

const HEADER: &str = "\
# Building a Krate app

This file is generated from the SDK, the capability registry, the WIT, and the
shipped example apps, so everything in it is accurate for the exact toolchain
that will build your app. Read it before you write code, and keep it open.

The loop: write code, then run `krate check-app .` in this directory. It builds
the app, checks it imports only Krate APIs, and runs it once. On failure it
names the stage and the fix. Do not stop until it prints `OK`.

For anything visual, `OK` is the floor, not the finish. check-app is a
correctness oracle, not a taste oracle: it will pass a frame with a seam
through the sky and a reflection floating loose. The loop that actually
improves a visual app is: render a frame headless with
`krate run <entry.wasm> --shoot frame.png -- quick`, LOOK at the picture,
name the one specific defect you can see, fix that, render again. Add
`krate run <entry.wasm> --check-layout -- quick` to catch text drawn over
text (it is a `run` flag, not a `check-app` one -- a real build wasted
attempts discovering that). Every pass through that
loop is worth more than another green check.

The one hard rule: a Krate component may import ONLY `krate:*` interfaces.
Reaching the operating system through `std` instead of through Krate pulls
`wasi:*` imports in, and the app is rejected at the import check. Everything
below is in service of writing an app that passes that check.

";

/// The patterns that separate a modern app from a dated one, and the grant
/// shape that separates a trustworthy manifest from a scary one. Hand-written
/// because these are judgments, not signatures -- the WIT docs carry the
/// per-function detail.
const DESIGN_PATTERNS_SECTION: &str = "\n---\n\n\
# 2b. Two patterns that decide how good the app feels\n\n\
## Working on a folder the person chooses\n\n\
Never declare a wide fs scope like `fs.list:**` -- check-app refuses it, and \
a manifest that reads as \"everything\" fails the person reading it even \
though the sandbox holds. For an app that works on THEIR folder (a tidier, a \
batch renamer, a photo shrinker), declare only `ui.dialog:open-folder` and \
call `ui.dialog.open-folder(window, title)`. The person's pick IS the grant: \
you get `{ name, token }`, and every ordinary fs call works under \
`picked/<token>/...` for this run -- list it, read files, write results, \
make subfolders. No fs capability at all. `apps/krate-tidy` is the worked \
example: a folder tidier whose manifest has zero fs lines. For output the \
app keeps between runs, use its own folder with a narrow scope like \
`fs.write:./exports/**`.\n\n\
## Filling the window, whatever size it is\n\n\
A person resizes windows, and every screen is a different shape. An app \
that draws from constants gets scaled up by the host to fit -- blurry \
text, and in a game the world pushed off the edge. This was reported by \
a real person on Windows: the character and the ground were off the \
screen, and changing the window size did not help.\n\n\
Two honest choices, and check-app enforces one of them:\n\n\
1. **Lay out from the window** -- call `canvas2d::canvas_size(canvas)` at \
   the top of every frame and compute positions from the answer. Best for \
   anything list-shaped or text-heavy, where extra room should be used. \
   Redraw on `Event::Resized`.\n\
2. **Fix your coordinate system** -- call \
   `canvas2d::set_design_size(canvas, Size { width: W, height: H })` once \
   after `bind`, then keep drawing in those numbers forever. The host \
   scales them uniformly to any window and centres what is left over, so \
   proportions are never distorted, and pointer events arrive in the same \
   coordinates. Best for games and anything whose layout IS its design.\n\n\
Doing neither is the single commonest way a generated app fails the \
person using it.\n\n\
**Pick 1 unless the layout is genuinely a fixed picture.** A design size is \
right for a game board, a clock face, a diagram -- things where the \
arrangement is the point and extra room should stay empty. It is the wrong \
choice for a dashboard, a feed, a list, a settings pane, or anything with \
cards and rows: those should USE a bigger window, and under a design size \
they instead sit letterboxed with dead bands around them while the text \
scales up soft. When in doubt, lay out from `canvas_size`. It is the \
recoverable mistake; a design size baked into 2000 lines of drawing is not.\n\n\
**A design size does not excuse you from handling the wheel.** This is the \
trap it sets, and a real generated weather dashboard fell into it: because \
the design box never overflows itself, it looks like there is nothing to \
scroll, so `Event::Wheel` gets left out entirely. But the person is looking \
at a window, not at your box -- they see cards running to the bottom edge, \
they scroll, and nothing moves. Handle `Event::Wheel` in any app with a \
list, a feed, rows, or cards, whichever layout choice you made.\n\n\
## Pixel buffers composited over a scene need faded edges\n\n\
`draw-pixels` puts your buffer on screen as an exact rectangle. If the \
buffer's border pixels are not fully transparent, that rectangle's edge is \
visible as a hard seam over whatever is behind it -- light, glow, smoke \
and sky have no straight edges, so the box gives the trick away instantly. \
Fade alpha to zero across the last few rows and columns of the buffer \
(multiply by x/fade, (w-x)/fade, same for y) so the content dissolves \
before the rectangle ends. Any buffer that fills the whole canvas is \
exempt -- its edges are the window's.\n\n\
## An app about someone's data ships with the data, and says so\n\n\
When the request comes with a spreadsheet or a document, that file IS the \
request. Each sheet arrives converted to CSV beside the original: read the \
CSVs, never the binary. Embed the data (or the meaningful parts of it) as \
constants or seeded storage, so the app opens already showing THEIR numbers \
-- an empty first screen after they handed you their data reads as a bug. \
For data that keeps changing on disk, use the folder picker \
(pick-is-the-grant) instead of baking a path.\n\n\
And name the data plainly in every capability rationale: \"keep your \
budget entries on this computer\" beats \"local storage\". The consent \
wall is the one moment the person decides whether to trust the app; an \
app built around personal finances that asks vaguely loses exactly the \
person it was built for.\n\n\
## Draining input: the difference between smooth and laggy\n\n\
A touch panel reports a drag up to 120 times a second, and every report \
becomes an event. An app that handles ONE event per frame and then draws \
can never catch up while a finger keeps moving: the backlog grows for as \
long as the gesture lasts, and the screen falls seconds behind the thumb. \
This was measured on a real iPhone -- the first swipe felt fine and every \
later one felt broken. Always drain what is queued before drawing:\n\n\
```rust\n\
// Take everything already waiting, then draw once.\n\
loop {\n\
    match events::poll() {\n\
        Some(Event::Wheel(w)) => scroll += w.dy,\n\
        Some(Event::CloseRequested(_)) => return 0,\n\
        Some(_) => {}\n\
        None => break,\n\
    }\n\
}\n\
// Then block for the next one (None) or poll briefly while animating.\n\
match events::wait(if settled { None } else { Some(16) }) { /* ... */ }\n\
```\n\n\
`poll` returns immediately when nothing is queued, so this costs nothing \
when the app is idle and everything when a finger is moving.\n\n\
## Making a game: everything, in one place\n\n\
A game asks for the same six facts every time, and hunting them costs more \
than writing the game. Measured: three attempts at a side-scrolling shooter \
spent their entire budget grepping the WIT for key names, audio, and \
sprites, and one ended with the words \"Now I'll write the game\" as the \
clock ran out. So: all of it, here, copy-ready.\n\n\
**1. The loop.** Poll input at the top, move, draw, present, pace with the \
clock. Never `request-redraw` (see the redraw rule); never bound the loop.\n\n\
**2. Input.** `events::key_held(\"ArrowLeft\")` for held keys -- exact \
names below. `events::poll()` for one-shot events (fire on the press, not \
every frame it is held). Gamepad: `gamepad_held(\"south\")`, \
`gamepad_axis(\"left-x\")`, and `gamepad_connected()` before showing a \
controller prompt.\n\n\
**3. Sprites.** Two ways, both real: `draw_pixels` blits a straight RGBA \
buffer at a rect (fastest for tiles and unrotated sprites), and \
`draw_sprite(canvas, center, dst, angle, w, h, rgba)` rotates one. For a \
tile-based level, build ONE atlas buffer at startup and blit windows out of \
it -- never rebuild pixel data per frame.\n\n\
**4. Sound, the whole recipe.** No file, no asset, no decoder needed: \
generate the samples. Open one stream at startup, load each effect once, \
play handles as things happen.\n\n\
\u{20}\u{20}\u{20}\u{20}// once, at startup\n\
\u{20}\u{20}\u{20}\u{20}let stream = audio::playback::open(audio::types::StreamConfig {\n\
\u{20}\u{20}\u{20}\u{20}    sample_rate: 22050, channels: 1,\n\
\u{20}\u{20}\u{20}\u{20}    format: audio::types::SampleFormat::PcmS16,\n\
\u{20}\u{20}\u{20}\u{20}    buffer_frames: 1024,\n\
\u{20}\u{20}\u{20}\u{20}})?;\n\
\u{20}\u{20}\u{20}\u{20}audio::playback::start(stream)?;\n\
\u{20}\u{20}\u{20}\u{20}// a square-wave blip: NES sound in twelve lines\n\
\u{20}\u{20}\u{20}\u{20}fn blip(hz: f32, ms: u32, duty: f32) -> Vec<u8> {\n\
\u{20}\u{20}\u{20}\u{20}    let n = (22050 * ms / 1000) as usize;\n\
\u{20}\u{20}\u{20}\u{20}    let mut out = Vec::with_capacity(n * 2);\n\
\u{20}\u{20}\u{20}\u{20}    for i in 0..n {\n\
\u{20}\u{20}\u{20}\u{20}        let phase = (i as f32 * hz / 22050.0) % 1.0;\n\
\u{20}\u{20}\u{20}\u{20}        // fade out so it clicks like a chip, not a pop\n\
\u{20}\u{20}\u{20}\u{20}        let env = 1.0 - (i as f32 / n as f32);\n\
\u{20}\u{20}\u{20}\u{20}        let v = if phase < duty { 0.35 } else { -0.35 } * env;\n\
\u{20}\u{20}\u{20}\u{20}        let s = (v * 32767.0) as i16;\n\
\u{20}\u{20}\u{20}\u{20}        out.extend_from_slice(&s.to_le_bytes());\n\
\u{20}\u{20}\u{20}\u{20}    }\n\
\u{20}\u{20}\u{20}\u{20}    out\n\
\u{20}\u{20}\u{20}\u{20}}\n\
\u{20}\u{20}\u{20}\u{20}let shot = audio::playback::load_sound(stream, &blip(880.0, 60, 0.5))?;\n\
\u{20}\u{20}\u{20}\u{20}// in the frame the shot happens:\n\
\u{20}\u{20}\u{20}\u{20}let _ = audio::playback::play_sound(stream, shot, 0.8);\n\n\
Music is the same trick with a note table: keep an index, advance it on a \
clock, play the next note's blip. Declare `audio.playback` in the manifest \
with a rationale in plain words (\"make the game's sounds\"). One stream \
for the whole app -- opening one costs ~2ms, which a game cannot spend \
mid-frame.\n\n\
**5. Pixel art, generated.** Do not ask for image files. Write small \
functions that fill RGBA buffers from a palette and a compact pattern (a \
`&[&str]` of rows, one character per pixel, is enough) -- that is how a \
16x16 soldier, a tile, and an explosion all get made with no assets. Scale \
by blitting at a bigger rect.\n\n\
**6. Scope.** Ship the vertical slice that actually plays: one level end to \
end, the movement, the shooting, one boss, lives and a game-over screen. A \
half-finished five-level game is worth less than one complete stage, and \
the person can always ask for more.\n\n\
## The key names, exactly\n\n\
`events::key-held(name)` and `key-event` use these strings, and nothing \
else. Getting this wrong is silent -- the app builds, runs, and simply \
never moves -- so they are listed here in full rather than guessed at:\n\n\
- **Arrows**: `ArrowLeft`, `ArrowRight`, `ArrowUp`, `ArrowDown`\n\
- **Letters and digits**: the character itself, lowercase: `\"a\"`, \
`\"w\"`, `\"z\"`, `\"1\"`. Not `KeyA`, not `\"A\"`.\n\
- **Named keys**: `Space`, `Enter`, `Tab`, `Backspace`, `Escape`, \
`Home`, `End`, `PageUp`, `PageDown`, `Delete`\n\
- Modifiers arrive on the event (`modifiers`), not as key names.\n\n\
A run-and-gun game therefore reads: \
`key_held(\"ArrowLeft\")`, `key_held(\"Space\")` to jump, \
`key_held(\"z\")` to shoot. `apps/krate-nova` and `apps/krate-bounce` \
are shipped examples doing exactly this.\n\n\
## Motion that reads as polish\n\n\
The SDK ships `krate::motion` (no_std, no capability): `ease_out`, \
`ease_in_out`, `smoothstep`, and a critically-damped `Spring`. Measure dt \
from `time.clock` each frame, tick, draw, and schedule the next frame \
(see the redraw rule below):\n\n\
```rust\n\
use krate::motion::Spring;\n\
let mut x = Spring::rest_at(0.0, 20.0);   // stiffness 10 calm, 30 snappy\n\
// each frame:\n\
x.tick(target, dt);\n\
draw_at(x.value);\n\
if !x.settled(target) { /* request another frame */ }\n\
```\n\n\
The full modern vocabulary, all in `krate::motion`, all free of host calls:\n\n\
- **Bounce**: `BouncySpring::rest_at(v, 30.0, 0.4)` overshoots and rings \
down -- the signature modern arrival. Use it for panels sliding in, a \
selected card popping to size, a value snapping to a detent. `bounce` 0.3 \
is a subtle wink, 0.5 clearly bounces, 0.7 is playful. Springs for input, \
bouncy springs for arrivals people should FEEL.\n\
- **Overshoot ease**: `ease_out_back(t)` for fixed-duration arrivals -- a \
menu that lands with a wink in 220ms. It peaks ~10% past the target, so \
give it room.\n\
- **Staggered reveals**: when a list first appears, each row fades and \
rises a beat after the one before:\n\n\
\u{20}\u{20}\u{20}\u{20}let t = motion::stagger(elapsed_ms, i, 40, 220);\n\
\u{20}\u{20}\u{20}\u{20}let a = motion::ease_out(t);\n\
\u{20}\u{20}\u{20}\u{20}// draw the row at (y + (1.0-a) * 14.0) with ink mixed toward\n\
\u{20}\u{20}\u{20}\u{20}// the background by (1.0-a): motion::mix(bg, ink, a)\n\n\
- **Ambient glow / breathing**: `pulse(now_ms, 2400)` is a 0..1 sine; feed \
it through `mix` for a badge that breathes or an accent that glows. This \
is the ONE motion allowed to loop forever.\n\
- **Flowing gradients**: the canvas has real gradient primitives -- \
`linear-gradient-stops` (any number of stops, any angle) and \
`radial-gradient`. A modern flowing backdrop is those stops with their \
colors moving: each frame, shift every stop's color with \
`motion::mix(c1, c2, motion::pulse(now_ms + phase_per_stop, 6000))` and \
redraw. Slow (5-8s periods), low-contrast color pairs, and it reads as \
alive rather than busy.\n\
- **Press feedback**: on pointer-down over a control, draw it 3-4% \
smaller (inset its rect) for as long as it is held; release springs it \
back. 60ms of feel that separates an app from a screenshot.\n\
- **Soft-blob backdrops** (the look of every friendly modern hero screen): \
layer 3-5 big `radial-gradient` discs over a light base, each with a \
saturated `inner` and an `outer` whose ALPHA IS ZERO -- the falloff to \
transparent is what makes them read as soft light instead of circles. \
Overlap them generously, keep radii huge (half the window and up), and \
drift their centers slowly with `pulse` for a living background. \
Gradients are dithered by the runtime, so slow color ramps stay smooth, \
never banded.\n\n\
## The redraw rule: never `request-redraw` from a loop that already draws\n\n\
`window::request-redraw` posts an event that comes straight back to you. \
In a loop that draws every frame on its own schedule, that means the queue \
is never empty, every `events::wait` returns instantly, and the app pins a \
whole CPU core while looking idle -- fans up, battery down, and nothing on \
screen moves any faster. Measured: two shipped animation apps at ~100% of \
a core from exactly this.\n\n\
`request-redraw` exists to WAKE a loop that would otherwise sit blocked in \
`wait(None)` -- an event-driven app that just changed something and needs \
one more frame. A continuously animating loop is never idle, so it must \
never call it. Pace with the clock instead: keep a `next_frame` deadline, \
add 1_000_000_000/60 after each draw, and spend the remainder blocked in \
`events::wait(Some(remaining_ms))` draining input. `apps/krate-glow` and \
`apps/krate-aurora` both show the exact loop.\n\n\
Rules that keep it tasteful: ease-out for anything arriving, springs for \
anything following input, bounce only where attention belongs (one bounce \
per screen, not one per widget), 150-300ms for interface moves, and \
nothing loops forever except ambient glow and flowing backdrops. Cards get \
`fill-round-rect` + `drop-shadow-round-rect` (shadow first, offset a few \
pixels down); progress is `stroke-arc` from -90 degrees; big numbers read \
best at weight 600-700 with slightly negative letter-spacing via \
`draw-text-styled`.\n";

/// Section 1: the SDK API surface, reusing the same generated reference the
/// authoring contract has always used.
///
/// `pub(crate)` because Krate Mode (`crate::krate_mode`) publishes the same
/// generated section into its paste-in prompt. One generator, two consumers, so
/// the published prompt cannot teach an API the pack has moved on from.
pub(crate) fn sdk_surface_section() -> String {
    let functions = sdk_reference::parse_sdk(sdk_reference::GUEST_SDK_SOURCE);
    let mut out = String::from("\n---\n\n# 1. The SDK: every `krate::*` function you can call\n\n");
    out.push_str(
        "This is the whole CLI-and-shared API surface, generated from the SDK source.\n\
         GUI apps also use the interfaces in section 4, reached through the generated\n\
         `bindings` module.\n\n",
    );
    // render_reference carries its own "if it is not here, do not invent it"
    // preamble and the widget-kind list, so it follows directly.
    out.push_str(&sdk_reference::render_reference(&functions));
    out
}

/// Section 2: the capability catalog, from the runtime's own registry. Richer
/// than the bare name list the old contract used: it names each capability's
/// phase, whether it is granted to every app by default, and how a scoped one
/// is written.
pub(crate) fn capability_catalog_section() -> String {
    let mut out = String::from("\n---\n\n# 2. Capabilities: what a manifest may declare\n\n");
    out.push_str(
        "Declare in `manifest.toml` only the capabilities the app actually uses. A\n\
         name outside this list is refused when the app is packed. The ones marked\n\
         *default* are granted to every app and must NOT be declared -- declaring one\n\
         is an error. Scoped names (`<path-glob>`, `<host>:<port>`) must be narrowed\n\
         to exactly what the app needs, e.g. `fs.read:notes/**`.\n\n",
    );

    // Group by phase so a CLI author sees the phase-2 set first and a GUI author
    // sees the windowing set clearly separated.
    out.push_str("## Available to every app (CLI and GUI)\n\n");
    out.push_str("| capability | default-granted? | notes |\n");
    out.push_str("|---|---|---|\n");
    for spec in krate_manifest::supported_capability_specs() {
        if !matches!(spec.phase(), krate_manifest::CapabilityPhase::Phase2) {
            continue;
        }
        out.push_str(&capability_row(spec));
    }

    out.push_str("\n## GUI apps only (a window, drawing, sound)\n\n");
    out.push_str("| capability | default-granted? | notes |\n");
    out.push_str("|---|---|---|\n");
    for spec in krate_manifest::supported_capability_specs() {
        if !matches!(spec.phase(), krate_manifest::CapabilityPhase::Phase3) {
            continue;
        }
        out.push_str(&capability_row(spec));
    }

    out.push_str(
        "\nMark `required = true` on a capability the app cannot begin without -- the \
         verification run withholds it and the app must refuse to start (exit 5). A \
         saving app marks `fs.write` required. `ui.window:create` is declared \
         `required = true` by convention, but withholding a window just closes the \
         app, so it is not the withheld gate; a GUI app whose only non-default \
         capability is its window has nothing to withhold, which is fine. Do not mark \
         required a capability the `quick` verification path never reaches, or the app \
         fails its own wall test after building cleanly.\n\n\
         \"Cannot begin without\" means the app it CLAIMS to be, not just a window that \
         opens. A music player without `audio.playback` is not a music player -- it is \
         a silent picture of one -- so mark that capability required. Ask: if this one \
         capability were withheld, would the app still be the thing the request asked \
         for? If no, it is required. A drawing app's canvas, a music player's audio, a \
         recorder's microphone, **a webcam app's camera**: required. A capability the \
         app only uses for a nice-to-have corner is genuinely optional; the app's whole \
         reason for existing is not.\n\n\
         This has real consequences, not just tidiness. The person is only asked about \
         capabilities marked required -- an optional one is never mentioned and never \
         granted, so the app opens without it and without a question. A generated \
         webcam viewer marked `camera.capture` optional and opened to a permanently \
         empty viewfinder: no camera, no prompt, nothing on screen explaining why. It \
         looked broken, and from the person's side it was.\n",
    );
    out
}

fn capability_row(spec: &krate_manifest::CapabilitySpec) -> String {
    let default = if spec.default_granted() { "yes" } else { "no" };
    let note = capability_note(&spec.name());
    format!(
        "| `{}` | {} | {} |\n",
        spec.display_pattern(),
        default,
        note
    )
}

/// A one-line human note for a capability, keyed by its `module.action` name.
/// Kept short: the full story is in the SDK reference and the no_std section.
fn capability_note(name: &str) -> &'static str {
    match name {
        "io.stdin" => "read stdin",
        "io.stdout" => "print to stdout",
        "io.stderr" => "print to stderr",
        "io.args" => "read command-line args",
        "io.log" => "structured logging",
        "fs.read" => "read files under a folder",
        "fs.write" => "write files under a folder",
        "fs.list" => "list a folder",
        "fs.remove" => "delete under a folder",
        "fs.mkdir" => "make folders",
        "store.kv" => "the app's own key-value store",
        "store.sql" => "the app's own SQL database",
        "store.secret" => "OS keychain (passwords, tokens)",
        "random.bytes" => "entropy (also what getrandom/rand need)",
        "net.connect" => "reach a host and port",
        "time.clock" => "wall-clock time",
        "time.monotonic" => "a monotonic timer",
        "time.sleep" => "sleep",
        "locale.info" => "the user's locale",
        "locale.format" => "locale-aware number/date formatting",
        "ui.window" => "open a window (every GUI app declares this)",
        "ui.clipboard" => "read/write the clipboard",
        "ui.menu" => "a system menu",
        "ui.open-url" => "hand a link to the browser",
        "ui.notify" => "a desktop notification",
        "ui.dropzone" => "accept dragged files",
        "ui.dialog" => "system file dialogs (choose a file)",
        "gfx.gpu" => "GPU drawing (canvas2d present today)",
        "audio.playback" => "play sound",
        "audio.capture" => "record from the microphone",
        "store.shared" => "share a key-value bucket with everyone holding its invite code",
        "camera.capture" => "see through the camera",
        _ => "",
    }
}

/// Section 2c: the app as a client of the person's own backend. Authored:
/// the individual calls are documented in the WIT sections, but the PATTERN
/// -- token in the secret store, Authorization header on every call, offline
/// degradation -- is a judgment agents were never taught, and the first
/// developer who asked ("does your tech support multiple-user accounts?")
/// already had a backend that made the question easy.
const BACKEND_CLIENT_SECTION: &str = "\n---\n\n\
# 2c. Talking to a backend the person already has\n\n\
Many requests are really \"a client for my existing service\": the person has \
an API with its own logins (their web app's backend, a home server, a SaaS \
they use), and wants a small desktop app that talks to it. Krate does not do \
user accounts itself -- the backend keeps owning identity -- but a Krate app \
makes a first-class client for one. The pattern:\n\n\
1. **Declare** `net.http` (to reach the API) and `store.secret` (to keep the \
sign-in token encrypted at rest). Nothing else; the person sees exactly those \
two lines and both are justified by the request.\n\n\
2. **Sign in the simple way**: a text field where the person pastes an API \
token or key, saved with `store::secret::set(\"api-token\", ...)`. On later \
launches, `secret::get` restores it silently. Do not build OAuth browser \
flows unless the request demands one -- most personal backends and home \
servers hand out tokens, and a paste box ships today.\n\n\
3. **Send it on every call**: build the `net` request with a header \
`{ name: \"authorization\", value: \"Bearer <token>\" }`. App-provided \
headers pass through; only transport headers (host, content-length) are \
host-controlled. POST/PUT/PATCH bodies are plain `list<u8>` -- serialize \
JSON with a no_std-friendly approach (build the string by hand for small \
payloads; that beats pulling a heavy crate through the import check).\n\n\
4. **Never block the UI on the network**: use the non-blocking form -- \
`http-client.begin(request)` then poll its `fetch-status` from the event \
loop (`pending` is a normal answer, not an error). A 401/403 response means \
the token expired: clear it and show the paste box again, with the app still \
usable for whatever it keeps locally.\n\n\
5. **Degrade offline**: cache what the person last saw in `store.kv` and \
render from the cache when requests fail, with a quiet \"offline -- showing \
last synced\" line. An app that opens to an error screen because the wifi \
dropped is worse than the web page it replaced.\n\n\
`apps/krate-fetch` shows the non-blocking request loop; add the header and \
the secret on top of its shape.\n\n\
6. **Live connections are the same grant.** For chat, live feeds, and \
multiplayer, `krate:net/ws` opens a WebSocket to any host the person \
granted -- `ws::open(\"wss://host/path\")` is checked against the same \
`net.connect` line a fetch is, so nothing new appears on the consent \
sheet. It never blocks: `open` returns a handle, and the event loop calls \
`ws::poll(handle)` each tick -- `pending` is the normal answer; `opened` \
arrives once; `message(text|binary)` are the server's messages in order; \
`closed`/`failed` retire the handle. `ws::send(handle, message)` queues \
and returns. Reconnect on `failed` with a short backoff, and keep \
rendering the last known state meanwhile, exactly like rule 5.\n\n\
# 2d. Sharing data between people: the shared store\n\n\
For \"my wife and I see the same list\", \"a meal plan the family edits\", \
\"share this with my roommate\" -- declare `store.shared` and use \
`krate:store/shared`. It is a key-value bucket shared between every machine \
that holds a ten-character invite code, synced through krate.tech, with no \
accounts and no backend. The person is told plainly at consent that anyone \
with the code sees the data.\n\n\
The shape of a shared app:\n\n\
1. **Work local-first.** `shared::get`/`set`/`delete`/`keys` always answer \
from this machine and never block on the network. Store each item as its own \
key (`item:<id>` holding a small hand-built JSON value), never the whole \
list under one key -- per-key merging is what lets two people edit at once \
without eating each other's changes.\n\n\
2. **Offer the share in the UI.** `shared::code()` returns none until the \
person creates or joins. Show a small \"Share\" area: a Create button that \
calls `shared::create()` and displays the returned code big enough to read \
across a room, and a text field + Join button that calls `shared::join(code)` \
(it errors with `no-such-share` on a typo -- show that plainly). After \
either, show the code so it can be given to the next person.\n\n\
3. **Sync on a rhythm, redraw on change.** Call `shared::sync()` on launch, \
after each write, and from the event loop every ten seconds or so \
(`events::wait(Some(10_000))` ticks are perfect). It returns `true` when \
another machine changed something -- redraw then. Offline it returns `false` \
and queues; never show an error for being offline, show a quiet \
\"last synced\" note if anything.\n\n\
4. **Deletes are real.** `shared::delete(key)` removes the item everywhere; \
the runtime keeps the tombstone so it cannot come back. Do not implement \
soft-delete flags on top.\n\n";

/// Section 2e: capabilities the runtime has had all along that no app ever
/// used, because nothing taught them. The coverage matrix
/// (Plan/Capability-Coverage-2026-08.md) found microphone capture, speech,
/// sound, notifications, and open-in-browser all shipped and all invisible --
/// the founder himself asked for "microphone support" that already existed.
/// What the pack does not teach, no generated app does.
const SENSES_SECTION: &str = "\n---\n\n\
# 2e. Sound, microphone, notifications, and the browser\n\n\
These are real, shipped capabilities. Use them when the request calls for \
them -- do not avoid them as exotic.\n\n\
**Sound effects** (`audio.playback`, granted by default): open one output \
stream at startup and keep it: `audio::playback::open(StreamConfig { \
sample_rate: 44_100, channels: 1, format: SampleFormat::PcmS16, \
buffer_frames: 1024 })`. For effects, synthesize or embed short PCM, load \
once with `load_sound(stream, &bytes)`, then `play_sound(stream, handle, \
1.0)` on the frame it happens -- it mixes, returns immediately, and playing \
it again overlaps a second copy. A dozen lines of generated square-wave PCM \
makes a click, a beep, a hit; no asset files needed.\n\n\
**Microphone** (`audio.capture`, an explicit ask -- the person sees \
\"record from the microphone\"): same `open` shape on \
`audio::capture` with `sample_rate: 16_000, channels: 1, format: PcmS16`, \
then `start(stream)`, and in the event loop `read(stream, 32_768)` returns \
whatever arrived since the last read (an empty list is normal). `stop` when \
the person ends the recording. Draw a level meter from the samples so the \
person can SEE it hearing them.\n\n\
**Speech to text** (`krate:speech/transcription`): \
`transcribe(model_asset, &pcm_s16_le, 16_000, None)` turns captured mono \
16kHz PCM into text -- but the whisper model file must be bundled in the \
app's `assets/` directory and named by `model_asset`. Only reach for it \
when the request is really about transcription and a model is provided; \
for \"voice memo\" requests, recording + playback without transcription \
ships today and satisfies most of them.\n\n\
**Notifications** (`ui.notify`, an explicit ask): \
`ui::notify::show(\"Timer done\", \"The pasta is ready\")`. The OS \
attributes it to the app. Use it when the thing the app waits for finishes \
-- a timer, a long job -- because the person has usually switched windows. \
No reply channel exists; do not build flows that depend on the person \
clicking it.\n\n\
**Open in the browser** (`ui.open-url`, an explicit ask): \
`ui::launch::open_url(\"https://...\")` hands a link to the person's \
browser. Pairs with 2c: a \"get your API token\" button that opens the \
backend's token page beats explaining where to click. Never open a URL the \
person did not just ask for.\n\n";

/// Section 2f: game feel. The Ice Climber A/B (one request, two agents)
/// showed the runtime treats every agent the same and the results still
/// differ wildly: the agent with taste shipped a HUD, a palette, and juice;
/// the agent without shipped flat rectangles. Taste can be written down.
/// This section is the floor-raiser for every agent that is not the best
/// one.
const GAME_FEEL_SECTION: &str = "\n---\n\n\
# 2f. Making a game feel finished\n\n\
A playable game and a finished-feeling game differ by a dozen small \
decisions. Make all of them:\n\n\
1. **Fixed-step update.** Run game logic on a fixed tick (16 ms via \
`events::wait(Some(16))`), never \"whenever an event arrives\". Movement \
speeds are per-tick constants; nothing depends on frame timing.\n\n\
2. **Design size + integer scale.** Pick a small logical resolution, \
`set_design_size` once, and draw sprites on an integer grid. Pixel art \
reads as deliberate; fractional scaling reads as broken.\n\n\
3. **A real palette.** Choose 6-10 colours before drawing -- background \
darkest, one accent for the player, one for danger, one for reward. Give \
sprites a 1px darker outline; it is the difference between \"shapes on a \
gradient\" and \"a game\".\n\n\
4. **Track key STATE, not key repeats.** On key-down set a flag, on key-up \
clear it, move while it is set. OS auto-repeat as a movement source feels \
underwater. Space/Z/arrows for actions; show the controls on the title \
screen and let both arrows and WASD work.\n\n\
5. **A HUD.** Score top-left, lives as icons top-centre or top-right, \
level top-right. Six-digit zero-padded score. The HUD is what makes it \
look like a game in a screenshot.\n\n\
6. **Juice on every player action.** A sound per jump/hit/score (see 2e \
sound effects -- synthesized PCM is enough), a 2-3 frame white flash on \
damage, a 100-150 ms screen shake on big hits, particles on destruction \
(eight 2px squares with velocity and gravity are plenty).\n\n\
7. **Title, pause, game over.** Title screen: name, one-line goal, \
controls, \"press space\". Esc pauses. Game over shows the score and \
restarts on space, keeping the high score in `store.kv`.\n\n\
8. **Ramp the difficulty.** Start winnable for thirty seconds, then speed \
up or add hazards on a schedule. The first death should teach, not \
ambush.\n\n";

/// Section 3: the no_std discipline. Authored, because it is the reasoning
/// behind the rules, not a list that can be scraped. The concrete facts it
/// leans on (which helpers, which crate versions) are stated here and repeated
/// nowhere, so there is nothing to drift.
const NO_STD_SECTION: &str = "\
\n---\n\n# 3. Passing the import check: std, no_std, and panics\n\n\
Reaching the OS through `std` pulls `wasi:*` imports and the app is rejected. So \
never use `std::fs`, `std::io` (including `println!`/`eprintln!`/`dbg!`), \
`std::time`, `std::env`, `std::process`, `std::net`, or `std::thread`. Use the \
`krate::*` equivalents in section 1 instead.\n\n\
Ordinary in-memory std is fine: `String`, `format!`, `Vec`, `HashMap`, and \
iterators do not reach the OS and do not leak.\n\n\
**The sharp exception is anything that can panic.** A reachable panic makes \
std's failure path reachable -- it formats a message, writes it to stderr, and \
exits, which is `wasi:cli`, `wasi:filesystem`, and `wasi:io` arriving together. \
One panic site can take a component from zero wasi imports to more than thirty. \
The two that catch people:\n\n\
- **Indexing.** `buf[i]` carries a bounds check that can panic. Use `.get(i)` / \
`.get_mut(i)` and handle the `None`.\n\
- **`.to_string()` and `format!`** route through the allocator's out-of-memory \
handler. In a no_std guest, allocate the bytes directly (copy `pure_string` from \
an in-repo sample) instead.\n\n\
Keep `panic = \"abort\"` and `opt-level = \"s\"` in the release profile: that is \
what stops std's unwinding and formatting machinery dragging its own I/O in.\n\n\
## std or no_std -- pick by your dependencies\n\n\
- **No dependencies beyond the bindings?** Use plain std. `krate-notes` is a \
shipped GUI app that does exactly this and imports zero `wasi:*`.\n\
- **Any real dependency (a decoder, a parser, `rand`), or a lot of logic where \
a stray panic is likely?** Make it `#![no_std]`. Even a crate that never \
touches the OS leaks through std's panic path.\n\n\
Converting the skeleton to `#![no_std]` is a checklist -- miss a step and it \
fails to build with \"no global memory allocator found\" or \"`#[panic_handler]` \
required\":\n\
\u{20}\u{20}1. put `#![no_std]` at the top of `src/lib.rs`, then `extern crate alloc;`, \
then `extern crate krate as _krate_runtime;`\n\
\u{20}\u{20}2. KEEP the `krate` dependency in `Cargo.toml` -- do NOT remove it. It is \
what provides the allocator, `#[panic_handler]`, and the `mem*` intrinsics a \
`no_std` guest needs. This is the step that is most often missed, and the \
usual way to miss it is scaffolding: if you copied `Cargo.toml` from a plain \
std example (krate-glow, krate-hello-gui), it has no `krate` line at all, \
because a std guest does not need one. Adding `#![no_std]` to code whose \
manifest came from a std example fails until you add the dependency:\n\
\u{20}\u{20}\u{20}\u{20}\u{20}krate = {{ path = \"<sdk>/krate\" }}  # copy the exact line from krate-contacts\n\
\u{20}\u{20}3. keep `std_feature = true` under \
`[package.metadata.component.bindings]`, which puts the generated \
`impl std::error::Error` behind a feature nobody turns on.\n\
\u{20}\u{20}4. build strings with a `pure_string`-style helper and allocate directly; \
avoid `format!`, `.unwrap()`, and `a[i]` indexing.\n\n\
`apps/krate-contacts` and `apps/krate-fractal` are shipped `no_std` GUI apps to \
copy this wiring from.\n\n\
## A dependency that needs randomness (getrandom / rand / uuid)\n\n\
These do not build on a target with no OS entropy unless a backend is \
registered. Do not hand-shim it. Add `features = [\"getrandom-backend\"]` to the \
`krate` dependency, add a `.cargo/config.toml` with \
`rustflags = [\"--cfg\", \"getrandom_backend=\\\"custom\\\"\"]`, and declare the \
`random.bytes` capability. The SDK then routes every draw to the host. \
`apps/krate-diceroll` is a working example.\n\n\
## Making it look built, not sketched\n\n\
The difference between an app that looks like a prototype and one that looks \
finished is a handful of habits, and none of them are hard. Apply them to \
whatever the app is -- these are not decoration for showcase apps, they are \
what an ordinary tool needs to look like it was built on purpose:\n\n\
**Go full-bleed when the design owns its whole surface.** \
`window::set-full-bleed(win, true)` right after create extends your content \
into the title-bar band with the host's window controls overlaid -- the \
shape every modern editor and terminal has. Always `let _ =` it: a host \
that cannot do it says unsupported and keeps the standard title bar. Leave \
the top ~40 pixels of your layout free of controls so nothing sits under \
the overlaid window buttons. `apps/krate-glow` shows it.\n\n\
**Measure text before you place it.** `canvas2d::measure_text` is the \
difference between a centred label and a nearly-centred one. Never estimate a \
width from the character count -- the face is proportional, so `i` and `W` \
differ about four times.\n\n\
**Outline round things with `stroke_circle`, never `stroke_rect`.** A rim on \
a bubble, a ring, a dial, an unfilled dot -- all `stroke_circle(canvas, \
center, radius, width, colour)`. Reaching for `stroke_rect` instead puts a \
visible square box around a round shape, which a real generated app shipped \
with.\n\n\
**Use the real rounded-rect call, never a hand-built one.** \
`canvas2d::fill_round_rect(canvas, area, radii, colour)` draws a card or a \
button in one call, correctly antialiased. Do not fake it by filling a \
rectangle and four `fill_circle` corners: the seams show, the edges alias \
differently from the curves, and it costs five calls to look worse. \
`stroke_round_rect` is the bordered version. A radius of 8-12 reads as a \
control, 16-20 as a card, and half the height as a pill.\n\n\
**Put a soft shadow under anything that floats.** \
`canvas2d::drop_shadow_round_rect(canvas, area, radii, blur, colour)`, drawn \
*before* the card itself and offset a few pixels down, is the single change \
that separates a flat 2015 layout from a current one. Use a low-alpha black \
(alpha 0.15-0.3) and a blur near the corner radius. A card with no shadow on \
a flat background looks painted on; one with a shadow looks placed.\n\n\
**Reach for gradients, with stops.** `linear_gradient_stops(canvas, area, \
angle_degrees, stops)` takes an angle and a list, so a backdrop can run \
diagonally through three colours instead of straight down through two. \
`radial_gradient` with a transparent outer colour is still the way to put a \
soft glow behind something important.\n\n\
**Vary the font weight -- this is what makes text look designed.** \
`draw_text_styled` takes a weight (400 body, 600-700 headings and big \
numbers), italic, and letter spacing. Big numbers read best at 600-700 with \
slightly negative letter spacing. An app whose text is all one weight looks \
like a form no matter how good the colours are; measure with \
`measure_text_styled` so the styled width is the one you place against.\n\n\
**Give things room.** Cramped is the commonest reason a generated app looks \
wrong: 16-24px of padding inside a card, 12-16px between rows, and a clear \
margin around the window edge. Space costs nothing and reads as care.\n\n\
**Pick three colours and stop.** A dark background, one bright accent for the \
thing you want clicked, and one ink colour for text. Every extra hue makes it \
look less designed, not more.\n\n\
## Texturing a 3D scene\n\n\
`scene3d` fills flat-shaded triangles by default, which looks like 1995. \
Textures are what make it look like a game: upload an image once, then draw \
triangles wearing it.\n\n\
\u{20}\u{20}\u{20}\u{20}// Once, at startup -- never per frame.\n\
\u{20}\u{20}\u{20}\u{20}let tex = scene3d::upload_texture(scene, w, h, &rgba)?;\n\n\
\u{20}\u{20}\u{20}\u{20}// Per frame: uvs are u,v per corner, six floats per triangle.\n\
\u{20}\u{20}\u{20}\u{20}scene3d::textured(scene, &verts, &uvs, tex, white)?;\n\n\
Coordinates outside 0..1 wrap, so a small image tiles across a large surface -- \
one 64x64 tile can cover a whole road or floor. `tint` multiplies the sample, \
so one grey texture becomes a red wall and a blue one without uploading it \
twice.\n\n\
An app with no image files can still generate a texture in code: fill an RGBA \
buffer with a checker, a noise pattern, or stripes, and upload that. A tiled \
procedural texture on the ground and the walls is the single biggest \
improvement available to a 3D scene, and it costs no assets.\n\n\
## The app's icon\n\n\
Write `assets/icon.png` (square PNG, 512px or larger) and the app wears it \
everywhere: the dock while it runs, Finder when installed, the installer. \
If the person supplied a logo as an attachment, copy it there. If not, a \
simple drawn mark beats none -- and shipping no icon.png is also fine: the \
app then wears the Krate mark rather than a generic page.\n\n\
Turn `cull_back_faces` on for closed shapes (a cube, a crate, a character) and \
roughly half the triangles stop being drawn. Leave it off for a flat floor or a \
billboard, which have one visible side and vanish when seen from behind.\n\n\
## Showing a picture\n\n\
Do not reach for the `image` crate: it requires `std` unconditionally and drags \
in the whole `wasi:*` surface. Decode PNG/JPEG yourself with `zune-png` / \
`zune-jpeg` / `zune-core` at `0.5`, `default-features = false`, then hand the \
RGBA to `ui::image::set_pixels`. The exact 0.5 API (earlier versions differ, and \
the method names are easy to guess wrong):\n\n\
\u{20}\u{20}\u{20}\u{20}use zune_core::bytestream::ZCursor;\n\
\u{20}\u{20}\u{20}\u{20}// PngDecoder::new takes a ZCursor, not a &[u8] directly:\n\
\u{20}\u{20}\u{20}\u{20}let mut dec = zune_png::PngDecoder::new(ZCursor::new(bytes));\n\
\u{20}\u{20}\u{20}\u{20}let rgba = dec.decode_raw()?;            // Vec<u8>, RGBA\n\
\u{20}\u{20}\u{20}\u{20}let (w, h) = dec.dimensions().unwrap();  // (usize, usize) -- cast to u32\n\n\
The methods are `dimensions()`, `colorspace()`, `depth()` -- not `get_dimensions` \
and friends. `apps/krate-fractal` shows the set_pixels side; `krate:fs/files` \
plus `ui::dialog::open_file` is how you let the person pick the file.\n\n\
## Getting input into a CLI app\n\n\
Command-line arguments (`krate::io::args`) are single-line only: an argument \
may not contain a newline, because Phase 2 delivers all args as one \
newline-joined string. So a short, single-line input (a number, a word, a URL) \
comes in as an argument -- but any multi-line input (a table, a JSON document, \
a block of text) must be read from standard input instead:\n\n\
\u{20}\u{20}\u{20}\u{20}let text = krate::io::stdio::stdin().read_text()?;\n\n\
Read stdin to end, do the work, print the result. This is how a formatter or a \
pretty-printer should take its input -- the same shape as `column`, `jq`, or \
`sort`, where you pipe data in. An app that tries to take a multi-line document \
as an argument will be refused before it runs.\n\n\
## Making a window that actually works\n\n\
The API list above says what you may call. This says what an interactive app \
has to do with it, because getting this wrong produces an app that builds, \
passes every check, and is useless in a person's hands.\n\n\
**Lay out from the real canvas size, never from constants.** A window can be \
resized. If you hard-code `WIDTH`/`HEIGHT` and hit-test against them, the \
canvas stretches while your click targets stay where they were, and every \
click lands in the wrong place or nowhere. Ask the canvas how big it is, \
compute the layout from that, and recompute when it changes:\n\n\
\u{20}\u{20}\u{20}\u{20}let size = canvas2d::canvas_size(canvas)?;   // width, height right now\n\n\
**Handle the resize event.** The runtime sends `Event::Resized(window-size)` \
when the window changes. Re-read the canvas size, recompute the layout, and \
redraw. An app that ignores it is only correct at its opening size.\n\n\
**Fill the space you were given. Leftover emptiness is a bug, not a style.** \
Laying out from the canvas size is only half the job -- the other half is \
making the content actually consume it. A generated seating planner drew its \
eight tables at a fixed radius from the top and left a third of the window \
dead at the bottom, which reads as unfinished no matter how good the drawing \
is. Three rules that fix this for any app:\n\n\
\u{20}\u{20}1. **Divide the space, then fill each division.** Decide the \
regions as fractions of the real canvas (\"the list takes the left 62%, the \
detail panel the rest\"), not as pixel constants. Then size what is inside \
each region from that region, so a taller window makes rows taller or shows \
more of them -- it never just adds blank space at the bottom.\n\n\
\u{20}\u{20}2. **Grids compute their cell size from the area, never the other \
way round.** For `n` items in a region `w` by `h`: pick the column count \
that best fills it, then `cell = (w - gaps) / cols` and let the cell drive \
the item's radius or height. A table drawn at a hardcoded 90px radius in a \
1400px-wide room is the same bug as a hardcoded window size.\n\n\
\u{20}\u{20}3. **Give the leftover to the element that can use it.** One \
region should be elastic -- usually the list, the canvas, or the feed -- and \
absorb whatever is left after the fixed chrome (header, footer, toolbar) is \
placed. If nothing is elastic, the window grows and the app does not.\n\n\
\u{20}\u{20}This applies inside a row of things as much as to the whole \
window. A generated table measured each column to its widest cell -- \
correctly, with `measure_text` -- summed them, found the total narrower than \
the window, and centred the table. That left a 157px strip down the right \
side with nothing in it for the full height of the window, and the bottom \
row clipped, in an app whose whole subject was fitting to content. Measuring \
to content is the right first step and the wrong last one: once every column \
has its minimum, hand the surplus to whichever column benefits, usually the \
one holding the longest free text.\n\n\
\u{20}\u{20}\u{20}\u{20}let natural: f32 = widths.iter().sum();\n\
\u{20}\u{20}\u{20}\u{20}let surplus = (avail - natural).max(0.0);\n\
\u{20}\u{20}\u{20}\u{20}widths[longest_text_column] += surplus;\n\n\
\u{20}\u{20}4. **Draw inside the region you were given, and derive every \
position from it.** Reserving a band is only half of it -- the drawing has to \
stay in the band. Two generated games got this wrong in two different ways, \
and both looked broken at a glance.\n\n\
\u{20}\u{20}A tic tac toe put its board at y 132 with height 380, so its \
score card started at 536 and ran to 612. Its \"New round\" button was a \
constant, `y: 556` -- computed sensibly on its own, never checked against \
what came before it, and it landed on top of the word \"Draws\". A memory \
game did reserve a footer and gave the rest to the cards correctly, then drew \
its hint text at `size.height - FOOTER_H - 6.0`, meaning \"just above the \
footer\" -- which is *outside* the footer, in the band already given to the \
cards, so the hint printed across the bottom row.\n\n\
\u{20}\u{20}The fix for both is the same. Compute the regions once, in one \
function, and let every draw take its coordinates from the region it belongs \
to:\n\n\
\u{20}\u{20}\u{20}\u{20}// `gfx::Rect` is a plain record -- x, y, width, \
height, no methods --\n\
\u{20}\u{20}\u{20}\u{20}// so name the edge you need as you go.\n\
\u{20}\u{20}\u{20}\u{20}let header_h = 96.0;\n\
\u{20}\u{20}\u{20}\u{20}let footer_h = 84.0;\n\
\u{20}\u{20}\u{20}\u{20}let header = rect(0.0, 0.0, w, header_h);\n\
\u{20}\u{20}\u{20}\u{20}let footer = rect(0.0, h - footer_h, w, footer_h);\n\
\u{20}\u{20}\u{20}\u{20}let board  = rect(0.0, header_h, w, footer.y - \
header_h);\n\n\
\u{20}\u{20}Then the button is `footer.y + 16.0`, never a literal, and the \
hint is *inside* the footer too, not six pixels above it. If a value that \
positions one element is a number you typed rather than a field of a region, \
it is the tic tac toe bug waiting to happen. Two elements may share a region \
only when you stack them within it and their heights add up to less than the \
region's.\n\n\
\u{20}\u{20}Check it the way a person would: text is the thing that shows a \
collision first, so for every string you draw, name the region it sits in. If \
you cannot, it has no home and will land on something.\n\n\
**Measure from the outermost edge, not the shape's own size.** When \
decorations stick out past a shape -- chairs around a table, a badge on a \
card corner, a glow, a selection ring -- the next element must clear the \
*decoration*, not the shape. A generated seating planner placed its guest \
names at `centre.y + table_radius + gap` while its chairs sat at \
`table_radius + chair_radius + 3`, so the first name landed exactly on the \
bottom chair in every table. Compute one `outer` value and lay out against \
it:\n\n\
\u{20}\u{20}\u{20}\u{20}let outer = ring + decoration_r;   // what the shape really occupies\n\
\u{20}\u{20}\u{20}\u{20}let text_y = centre.y + outer + gap;\n\n\
Reserve space for the same `outer` when you compute how big the shape may \
be, so the two calculations cannot disagree. Anything that overlaps text is \
a defect, however good it looks in one window size.\n\n\
**Weight the space by importance.** Within a region, the thing the app is \
*for* gets the most room: a chart app is mostly chart, a list app is mostly \
list. Headers and footers are chrome -- 8-14% of the height each is plenty. \
An app that spends a third of its window on a title is telling the person the \
title matters more than their data.\n\n\
**Never block the window on a request a person is watching.** `net::get` and \
`net::fetch` block until the response is complete, and an app is single \
threaded -- so a slow server freezes everything: no frame, no click, no cancel \
button. Measured against a server that stalled three seconds, a blocking fetch \
did zero work in that time; the same request through the calls below did 258 \
turns of it.\n\n\
Use `net::begin` for anything a person waits on. It returns a handle at once, \
and `net::poll` answers immediately every time:\n\n\
\u{20}\u{20}\u{20}\u{20}let handle = net::begin(req)?;   // returns in ~1ms\n\
\u{20}\u{20}\u{20}\u{20}loop {\n\
\u{20}\u{20}\u{20}\u{20}    match net::poll(handle) {\n\
\u{20}\u{20}\u{20}\u{20}        FetchStatus::Pending => {}            // draw a spinner\n\
\u{20}\u{20}\u{20}\u{20}        FetchStatus::Ready(res) => { show(res); break }\n\
\u{20}\u{20}\u{20}\u{20}        FetchStatus::Failed(e) => { show_error(e); break }\n\
\u{20}\u{20}\u{20}\u{20}        FetchStatus::UnknownHandle => break,\n\
\u{20}\u{20}\u{20}\u{20}    }\n\
\u{20}\u{20}\u{20}\u{20}    drain_events();  draw_frame();          // the app stays alive\n\
\u{20}\u{20}\u{20}\u{20}}\n\n\
`net::cancel(handle)` is what a cancel button calls, and it is safe to call \
twice. The permission check happens at `begin`, so an ungranted host fails \
there rather than at a later poll. Blocking `net::get` is still right for a \
one-shot CLI tool where nobody is looking at a window.\n\n\
**If the app has live data, fetch it when it starts.** Sample data is a \
fallback for when the network fails, never the state a person sees first. An \
app asked for \"open source news\" declared `net.connect:hnrss.org:443`, wrote a \
correct fetch, and put it behind a Refresh button -- so it opened showing \
fifteen hardcoded headlines and looked like it was faking. Fetch on startup, \
show what you got, and fall back to samples only after a real failure, saying \
so.\n\n\
**Do not close yourself. This is the most common way a generated app fails.** \
A real session ends when the person closes the window \
(`Event::CloseRequested`), and only then. Do NOT put a round limit, a frame \
count, or an idle timeout on the interactive loop.\n\n\
Eight apps were written from this pack by an AI and every single one bounded \
its loop -- `const MAX_ROUNDS: u32 = 800` at 50ms a round, so each app quit \
itself after forty seconds while somebody was using it. A flashcard app that \
closes during revision is not a working app, however well the rest is written. \
The bound is only ever correct on the `quick` path, where nobody is watching:\n\n\
\u{20}\u{20}\u{20}\u{20}if quick {\n\
\u{20}\u{20}\u{20}\u{20}    // seed state, draw one frame, print key:value, exit 0\n\
\u{20}\u{20}\u{20}\u{20}}\n\
\u{20}\u{20}\u{20}\u{20}// interactive: loop with no round limit at all\n\
\u{20}\u{20}\u{20}\u{20}loop {\n\
\u{20}\u{20}\u{20}\u{20}    match events::wait(None) {\n\
\u{20}\u{20}\u{20}\u{20}        Some(Event::CloseRequested(_)) => break,\n\
\u{20}\u{20}\u{20}\u{20}        // ... handle the rest, redraw when something changed\n\
\u{20}\u{20}\u{20}\u{20}        _ => {}\n\
\u{20}\u{20}\u{20}\u{20}    }\n\
\u{20}\u{20}\u{20}\u{20}}\n\n\
**Hit-test against what you actually drew.** There is no widget tree behind a \
canvas: a drawn button is only clickable because you compare the pointer \
position to the same rectangle you filled. Keep the layout in one place so the \
drawing and the hit-testing cannot disagree.\n\n\
**Make content taller than the window scroll.** A list that outgrows the window \
is the normal case, not an edge case, and an app that draws six of thirty-two \
rows and a \"+ 26 more\" label has lost the other twenty-six for good. Judge \
this by what the app SHOWS -- rows, cards, a feed, a document -- not by \
whether your own layout maths happens to overflow today: a fixed design size \
never overflows itself, and skipping the wheel on that reasoning ships an app \
that ignores every scroll gesture a person makes at it. The \
runtime sends `Event::Wheel(wheel-event)` for a mouse wheel, a trackpad, or a \
scroll gesture, with `dx` and `dy` in logical pixels (positive `dy` scrolls \
down, further into the list) already normalized across all three systems. Keep \
one scroll offset, add the delta, clamp it, and redraw:\n\n\
\u{20}\u{20}\u{20}\u{20}Some(types::Event::Wheel(w)) => {\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}// content_height is the height of everything you would draw;\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}// view_height is the region you draw it into.\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}let max = (content_height - view_height).max(0.0);\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}scroll = (scroll + w.dy).clamp(0.0, max);\n\
\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}dirty = true;\n\
\u{20}\u{20}\u{20}\u{20}}\n\n\
Then subtract `scroll` from every row's y when you draw it, and **subtract it \
again when you hit-test** -- the two must use the same number or clicks land on \
the wrong row the moment somebody scrolls.\n\n\
**Text views scroll by pixels too -- never round the offset to whole lines.** \
The natural way to draw a document is by line index, and the natural mistake \
is `first_line = scroll / line_height` with the remainder thrown away: every \
wheel tick then REPLACES a row of text instead of gliding it, which reads as \
a 90s terminal next to any real editor. Keep the offset in pixels and split \
it only at draw time:\n\n\
\u{20}\u{20}\u{20}\u{20}let first = (scroll / line_height) as usize; // which line starts on screen\n\
\u{20}\u{20}\u{20}\u{20}let within = scroll % line_height;\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}// how far INTO it we are\n\
\u{20}\u{20}\u{20}\u{20}let mut y = list_y - within;\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}// first line starts above the top\n\n\
Draw from `first` until y passes the bottom. A partly-visible line at the top \
and bottom is the smoothness; the clip region above trims them.\n\n\
**Clip the list region before you draw the rows.** Without it a row scrolled \
past the top paints over your header. Set the clip to the list's rectangle, \
draw every row including the partly-visible ones, then clear it:\n\n\
\u{20}\u{20}\u{20}\u{20}canvas2d::set_clip(canvas, list_x, list_y, list_w, list_h);\n\
\u{20}\u{20}\u{20}\u{20}// draw every row; the ones outside the rectangle are trimmed\n\
\u{20}\u{20}\u{20}\u{20}canvas2d::clear_clip(canvas);\n\n\
Do not try to skip out-of-view rows by hand instead. It works until rows have \
different heights, and then it cannot be done correctly at all.\n\n\
## Packing\n\n\
`krate pack <built.wasm> --manifest manifest.toml --output app.krate` takes \
your development manifest as-is: inside a bundle the component is stored as \
`code.wasm`, and pack rewrites the bundle's copy of `entry` itself, leaving \
your file untouched. No second manifest is needed.\n\n\
## The verification run\n\n\
`check-app` (and `create`) run the app once with every capability granted and \
one argument, requiring exit 0. The argument is the bare word `quick` (not \
`--quick`), except a CLI app that declares `fs.read:` and no window is given a \
sample file path instead. Handle `quick` before any other argument parsing: on \
`quick`, do the real work once against a small built-in sample (or empty \
stdin), print what the app is holding, and exit 0. Never wait for input or open \
a window nobody will close. An app that parses arguments strictly and rejects \
the unknown `quick` fails here after building and packing correctly.\n\n\
**The headless run has a compute budget, and per-pixel work is what hits \
it.** Verification executes on a fuel meter, not a wall clock, so an \
expensive-but-correct render can exhaust it with no loop bug anywhere -- \
the failure says \"fuel budget\" and exit 4. If you are computing pixels \
(noise fields, gradients through `draw-pixels`), three habits keep you \
far from the ceiling, and they are the same habits that make the app fast \
for real: hoist per-column and per-row work out of the per-pixel loop, \
skip pixels that cannot contribute (fully transparent, off-screen), and \
keep the `quick` frame count SMALL. In quick mode `t` advances per frame, \
synthetic time -- 10 frames and 90 animate identically, but every frame \
spends fuel. Draw about 10 and break.\n\n\
**On `quick`, operate your own controls -- do not just print a snapshot.** \
The request names the things a person will do, and `quick` is where the app \
proves it can do them. A shopping list asked for \"add and remove items\" \
should add one and remove one, then report both; printing \
`items:5 remaining:2` describes a list without showing it can be changed, \
and nothing outside the app can tell the difference between working buttons \
and a picture of some. Measured: a shopping list did exactly this and was \
scored a failure while being, as far as anyone could tell, correct.\n\n\
So drive the verbs in the request and report their result:\n\n\
\u{20}\u{20}\u{20}\u{20}items:5      // after seeding\n\
\u{20}\u{20}\u{20}\u{20}added:1      // it added one\n\
\u{20}\u{20}\u{20}\u{20}removed:1    // and removed one\n\
\u{20}\u{20}\u{20}\u{20}saved:yes\n\n\
This is also the only evidence that clicking works at all: there is no \
scripted-input path, so an app that cannot exercise its own controls cannot \
show that they respond.\n\n\
**Every value you print must be read out of the app's own state, at the \
moment you print it.** Never write the numbers as literals. A generated \
countdown timer ended its quick run with:\n\n\
\u{20}\u{20}\u{20}\u{20}out.write(b\"duration:300\\nstarted:yes\\nreset:yes\\nremaining:297\\n\");\n\n\
Every line is a constant. That output is identical whether the timer works or \
whether the Start button is deleted outright -- so it is not a check, it is a \
picture of one, and it reads as passing while measuring nothing. It cost real \
debugging time: the hardcoded `started:yes` was taken as proof the button \
worked, and the hunt went looking in the runtime for a bug that was never \
there. Format the real fields (`timer.remaining_secs()`, \
`list.len()`, `if timer.running {...}`) so a broken app prints something \
different from a working one.\n\n\
**Do not let the last thing you drive undo the rest.** You print once, at the \
end, so the final state has to still show the work. A generated countdown \
timer exercised itself thoroughly -- started, ticked, paused, lengthened, \
shortened, reset -- and reset last, so it printed:\n\n\
\u{20}\u{20}\u{20}\u{20}duration:1500\n\
\u{20}\u{20}\u{20}\u{20}remaining:1500   // the full duration: it looks like it never ran\n\
\u{20}\u{20}\u{20}\u{20}elapsed:1\n\
\u{20}\u{20}\u{20}\u{20}ticks:1\n\
\u{20}\u{20}\u{20}\u{20}reset:yes\n\n\
Every line is true and the app is correct, but `remaining` equals the \
starting value, so the one number a person would check to see a timer \
counting says it did not. Put the undoing operations -- reset, clear, \
cancel, close -- in the middle and end on a state that shows the app \
working, or print the telling value where it is true (`remaining_at_pause`) \
as well as at the end.\n\n\
**Print one `key:value` per line, and make the keys mean something.** This is \
the only way anything outside the app can tell whether it did what was asked. \
`check-app` reads it, CI reads it, and a benchmark reads it. \"print something\" \
is not enough: an app that prints `ok` builds, runs, paints a frame, and proves \
nothing about whether it works.\n\n\
Print the state a person would look at to judge the app. A to-do list prints \
how many items it holds and how many are done; a tip calculator prints the tip \
and the total; a game prints the score, whether it is over, **and the \
position it is in** -- a board, a level, whose turn it is:\n\n\
\u{20}\u{20}\u{20}\u{20}items:5\n\
\u{20}\u{20}\u{20}\u{20}done:2\n\
\u{20}\u{20}\u{20}\u{20}saved:yes\n\n\
Lower-case keys, no spaces around the colon, one pair per line, numbers as \
bare digits. Seed enough state in the `quick` path that the numbers are \
interesting -- a to-do list that prints `items:0` has proved nothing either.\n\n\
**Seed at the scale the request describes.** If the person asked for \
something long, deep or busy, the `quick` path has to actually be long, \
deep or busy, or nothing it prints can show the behaviour they asked \
about. Measured: a log viewer asked to \"keep the newest line in view\" \
seeded 29 lines, which is fewer than fit on screen -- so it had nothing to \
scroll, printed `scrolls:0`, and could not demonstrate the one thing the \
request named. A long list means hundreds of rows, a big file means \
thousands of lines, a busy log means more lines than a window holds. \
Seeding is cheap; generate the rows in a loop rather than typing a \
handful.\n\n\
**Name the key after the plain noun for the thing, and print the number \
bare.** Whatever reads the output has to guess the name otherwise, and it \
guesses the ordinary one -- a tip calculator prints `bill` and `total`, a \
dice roller prints `die1`, `die2` and `total`, a countdown prints \
`remaining`. Reach for the word a person would use for that quantity, not \
the one your variable happens to be called. Measured: six of seven benchmark \
failures were apps that worked perfectly and were scored wrong because of a \
name --\n\n\
\u{20}\u{20}\u{20}\u{20}count:60          not  clicks:60\n\
\u{20}\u{20}\u{20}\u{20}height:178        not  height_cm:178\n\
\u{20}\u{20}\u{20}\u{20}marked:2          not  done-today:2\n\
\u{20}\u{20}\u{20}\u{20}password:hT7x...  not  length:32 with no password\n\
\u{20}\u{20}\u{20}\u{20}elapsed:140       not  elapsed:2:20.18\n\n\
Four rules that follow. Prefer the ordinary word for the quantity: a click \
counter prints `count` rather than `clicks`, a habit tracker prints `marked` \
rather than `done-today`. Put units in the value or leave them out, never in \
the key -- `height:178`, not `height_cm`. Print durations and amounts as \
bare numbers; `elapsed:2:20.18` cannot be compared to anything, so print \
`elapsed:140` and add a formatted `elapsed_text` beside it if it helps a \
person, and never put a currency symbol in a number -- `total:289.93`, not \
`total:$289.93`. If the app generates something -- a password, a colour, an \
id -- print the thing itself, not only facts about it.\n\n\
**Print the position, not only the result.** An outcome says what happened; \
the position says where things stand, and it is the thing a person looks at \
to judge whether the app works. Measured: a tic tac toe app played fourteen \
moves across two rounds and rejected two illegal ones, then printed only \
`winner`, `draws` and `rounds` -- never the board or whose turn it is. \
Everything it reported was true, and you still could not tell from the \
output whether the game had a board.\n\n\
So a game prints its board and whose turn it is; an editor prints the \
current text or its first line; a viewer prints which line is at the top. \
A grid can go on one line with a separator, and that is enough:\n\n\
\u{20}\u{20}\u{20}\u{20}board:X.O|.X.|O..\n\
\u{20}\u{20}\u{20}\u{20}turn:O\n\
\u{20}\u{20}\u{20}\u{20}moves:14\n\
\u{20}\u{20}\u{20}\u{20}winner:X\n\n\
**If you have a count, print the count, not `yes`.** A word like `yes` \
tells a reader the thing happened; a number tells them how much, and \
anything comparing the value can only work with the number. Measured: an \
app that wrapped 230 words into 18 lines printed `wrapped:yes`, which \
proves less than the `18` it already had. Prefer `wrapped:18`, \
`scrolled:1200`, `matched:2`. Keep `yes`/`no` for things that genuinely \
have no quantity -- `saved:yes` is right, because saving either happened or \
did not.\n\n\
**When a name is genuinely ambiguous, print both.** Extra lines cost \
nothing and no reader is confused by them, while a missing one is \
invisible. A search box can print `query` and `search`; a list can print \
`items` and `entries`. This is cheap insurance against the one thing you \
cannot know from the request -- what the reader on the other side decided \
to call it.\n";

/// Section 4: the GUI world interfaces, extracted from the WIT. A GUI app calls
/// these through its generated `bindings::krate::{ui,gfx,audio,speech}::*`, not
/// through the `krate::*` SDK functions in section 1.
pub(crate) fn gui_world_section() -> String {
    let mut out = String::from("\n---\n\n# 4. The GUI world: ui / gfx / audio / speech\n\n");
    out.push_str(
        "A windowed app reaches these through its generated `bindings` module, e.g.\n\
         `bindings::krate::gfx::canvas2d::present(canvas)`. Records live in each\n\
         package's `types` interface (with two exceptions the samples show:\n\
         `ui::image::ImagePixels` and `ui::dialog`). Signatures are WIT, so\n\
         `list<u8>` is a Rust `Vec<u8>`/`&[u8]`, `result<t, e>` is `Result<T, E>`,\n\
         and kebab-case names become snake_case in Rust.\n\n\
         IMPORTANT for a GUI app: reach the *shared* modules through `bindings::krate`\n\
         too, not the `krate::` SDK helpers in section 1. Their shapes differ. In the\n\
         generated bindings the action is a nested module, so it is\n\
         `bindings::krate::random::bytes::get(count)`,\n\
         `bindings::krate::random::bytes::below(bound)`,\n\
         `bindings::krate::random::bytes::next_u64()`, and\n\
         `bindings::krate::store::kv::get(key)` -- not `random::bytes(count)` or\n\
         `store::get(key)`, which are the SDK free-function forms and do not exist on the\n\
         GUI world's `bindings`. When in doubt, expand the module path: the leaf that\n\
         takes the arguments is the function.\n\n",
    );
    for (package, wit) in [
        ("gfx", GFX_WIT),
        ("ui", UI_WIT),
        ("audio", AUDIO_WIT),
        ("camera", CAMERA_WIT),
        ("speech", SPEECH_WIT),
    ] {
        out.push_str(&render_wit_interfaces(package, wit));
    }
    out.push_str(
        "\n## Showing a camera feed\n\n\
         A live preview is a poll in the event loop, not a callback. Open once, \
         start once, then read a frame each time round and draw whatever came \
         back:\n\n\
         \u{20}\u{20}\u{20}\u{20}let id = camera::capture::open(\"\", &config)?;  // \"\" = default camera\n\
         \u{20}\u{20}\u{20}\u{20}camera::capture::start(id)?;                  // the indicator light comes on here\n\
         \u{20}\u{20}\u{20}\u{20}// ... each time round the loop:\n\
         \u{20}\u{20}\u{20}\u{20}if let Ok(Some(frame)) = camera::capture::read(id) {\n\
         \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}last = Some(frame);       // keep it: read() gives each frame once\n\
         \u{20}\u{20}\u{20}\u{20}}\n\
         \u{20}\u{20}\u{20}\u{20}if let Some(f) = &last {\n\
         \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}// f.width and f.height, NOT the size you asked for.\n\
         \u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}\u{20}canvas2d::draw_pixels(canvas, area, f.width, f.height, &f.bytes)?;\n\
         \u{20}\u{20}\u{20}\u{20}}\n\n\
         Five things that decide whether this works:\n\n\
         **`read` returns `None` constantly, and that is normal.** It means no \
         new frame since you last asked, which happens whenever your loop is \
         faster than the camera. Keep the last frame and redraw it; an app that \
         draws only when `read` returns `Some` flickers between the picture and \
         a blank rectangle.\n\n\
         **Wake up often enough to see frames.** Use \
         `events::wait(Some(16))` while the camera is running, the same way a \
         network poll does it. `events::wait(None)` blocks until somebody \
         clicks, so the preview freezes on the first frame.\n\n\
         **Draw at `frame.width` and `frame.height`, never at the size you \
         asked for.** A camera opens at its nearest supported mode: ask a Mac \
         for 640x480 and you get 1920x1080. Every frame carries its own size \
         for exactly this reason, so the numbers cannot disagree with the \
         bytes. Reading a size once at startup and reusing it is the trap -- \
         the true size is not known until the first frame arrives, so an app \
         that did that laid 1080p bytes out as 480p and showed a black window \
         with the camera light on. `info(id)` exists for sizing a viewfinder \
         before frames start; the frame itself is what you draw with.\n\n\
         **The bytes are already the format `draw_pixels` takes**: straight- \
         alpha RGBA, `width * height * 4`, top-left first. No conversion.\n\n\
         **`stop` when the preview is hidden, and `close` on the way out.** The \
         indicator light is wired to the hardware, so a person can see whether \
         an app that says it stopped looking actually did. A photo button is \
         just the last frame you were already holding -- there is no separate \
         capture call, and taking a still does not need a second stream.\n\n\
         Declare `camera.capture` in the manifest, required if the app IS the \
         camera. It is never granted by default: the person is asked, and on \
         macOS the system asks a second time on its own. If `open` returns \
         `system-denied` rather than `permission-denied`, that second wall is \
         the one blocking -- say so plainly, because the fix is in system \
         settings and no amount of clicking inside your app will help.\n\n\
         ## Never guess how wide text is -- measure it\n\n\
         `canvas2d::measure_text(canvas, text, font_size)` returns `width`, \
         `height`, `ascent`, and `descent` for the run `draw_text` is about to \
         draw. Use it any time a position depends on the size of text: \
         centring a label, right-aligning a number, placing a caret after \
         what has been typed, sizing a card or a pill around its own text, or \
         stacking lines of a paragraph.\n\n\
         **Do not write a `text_width` helper that multiplies character count \
         by a constant.** It is wrong, not approximate. The face is \
         proportional -- `i` and `W` differ about four times in real width -- \
         so `\"iiii\"` and `\"WWWW\"` get the same made-up answer while the \
         drawn pixels differ several times over. That single mistake is why \
         labels are not really centred, captions overflow their cards, and \
         carets sit beside the text instead of after it. The host already \
         knows the true number; ask for it.\n\n\
         \u{20}\u{20}\u{20}\u{20}// centre a label in a box\n\
         \u{20}\u{20}\u{20}\u{20}let m = canvas2d::measure_text(canvas, label, 17.0)?;\n\
         \u{20}\u{20}\u{20}\u{20}let x = box_x + (box_w - m.width) * 0.5;\n\n\
         \u{20}\u{20}\u{20}\u{20}// a caret sitting just after the typed text\n\
         \u{20}\u{20}\u{20}\u{20}let caret_x = text_x + canvas2d::measure_text(canvas, typed, 16.0)?.width;\n\n\
         `draw_text` takes a **baseline** as its origin, which is what `ascent` \
         is for: to put a run's top edge at `y`, draw it at `y + m.ascent`; to \
         centre it vertically in a box of height `h`, draw at \
         `y + (h - m.height) * 0.5 + m.ascent`. Stack paragraph lines by \
         `m.height`.\n\n\
         The measurement is single-line and unwrapped, because `draw_text` is \
         too. To wrap a paragraph, measure words and break the lines yourself.\n\n\
         Start from the closest example in section 5 rather than these signatures \
         alone -- the samples show the call order (bind a canvas, draw, present) that \
         the signatures do not.\n",
    );
    out
}

/// Extract `interface { func... }` shapes from one WIT package into a compact
/// Markdown list. A deliberately small parser: it reads interface headers and
/// function signatures line by line. It does not resolve types -- the goal is an
/// accurate index of what exists and its shape, not a type-checked reference.
fn render_wit_interfaces(package: &str, wit: &str) -> String {
    let mut out = format!("## `{package}`\n\n");
    let mut current_interface: Option<String> = None;
    let mut depth: i32 = 0;
    let mut wrote_any = false;
    // When a function declaration spans several lines (the speech interface
    // wraps its parameters), accumulate them until the terminating `;`.
    let mut pending: Option<String> = None;

    for raw in wit.lines() {
        let line = raw.trim();

        // Continuing a multi-line function signature: keep appending until the
        // declaration ends at a semicolon.
        if let Some(mut acc) = pending.take() {
            acc.push(' ');
            acc.push_str(line);
            if line.contains(';') {
                let iface = current_interface.clone().unwrap_or_default();
                out.push_str(&format!("- `{iface}::{}`\n", normalize_wit_signature(&acc)));
                wrote_any = true;
            } else {
                pending = Some(acc);
            }
            continue;
        }

        // Track brace depth so we only read function declarations that sit
        // directly inside an interface body, not inside a record or variant.
        let opens = line.matches('{').count() as i32;
        let closes = line.matches('}').count() as i32;

        if let Some(name) = line.strip_prefix("interface ") {
            let name = name.trim_end_matches(" {").trim_end_matches('{').trim();
            current_interface = Some(name.to_string());
            depth = opens - closes;
            continue;
        }

        // A function declaration inside an interface: `name: func(...) -> ...;`
        // or `name: async func(...) ...`. Only at one level deep in an
        // interface (records/variants nest deeper and are skipped).
        if current_interface.is_some() && depth == 1 {
            if let Some((sig_name, rest)) = line.split_once(':') {
                let rest = rest.trim();
                if rest.starts_with("func") || rest.starts_with("async func") {
                    let acc = format!("{}: {rest}", sig_name.trim());
                    if line.contains(';') {
                        let iface = current_interface.clone().unwrap_or_default();
                        out.push_str(&format!("- `{iface}::{}`\n", normalize_wit_signature(&acc)));
                        wrote_any = true;
                    } else {
                        // Multi-line: the parameters wrap onto following lines.
                        pending = Some(acc);
                    }
                    continue;
                }
            }
        }

        depth += opens - closes;
        if depth <= 0 {
            current_interface = None;
            depth = 0;
        }
    }

    if !wrote_any {
        out.push_str("_(no callable functions)_\n");
    }
    out.push('\n');
    out
}

/// Tidy a WIT function signature onto one line: collapse all internal
/// whitespace (so a wrapped multi-line declaration reads as one line) and drop
/// the trailing semicolon.
fn normalize_wit_signature(sig: &str) -> String {
    let joined = sig.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.trim_end_matches(';').trim().to_string()
}

/// Section 5: an index of the shipped example apps. The single best teacher --
/// each app is proven working (it ships and passes CI), so the agent reads the
/// closest one and adapts real no_std code rather than writing blind.
fn example_index_section(app_dir: &Path) -> String {
    example_index_section_for(find_apps_dir(app_dir))
}

/// The section body, given an already-resolved apps tree.
///
/// Split out so the no-tree branch -- the one a person who installed a release
/// actually gets -- can be tested. Resolving inside made that untestable here:
/// `find_apps_dir` falls back to the workspace this binary was built in, which
/// always exists on a development machine and never exists on a user's.
fn example_index_section_for(apps_dir: Option<std::path::PathBuf>) -> String {
    let mut out = String::from("\n---\n\n# 5. A complete worked example\n\n");
    out.push_str(
        "This is a whole working GUI app, inlined rather than referenced, because \
         adapting proven code beats writing the `#![no_std]` and `krate:*` discipline \
         from a blank page. Copy its shape: the imports, the panic-free style, the \
         event loop, the redraw-when-dirty pattern.\n\n",
    );
    out.push_str(WORKED_EXAMPLE);
    out.push('\n');

    // The index of every shipped app is only useful when those files are
    // actually on this machine. Printing paths that do not exist is worse than
    // printing nothing: an agent told to read `apps/krate-paint/src/lib.rs`
    // goes looking for it, and `find /` on a machine without the repo is how a
    // two-minute authoring run became an eight-minute one on Windows.
    let Some(apps_dir) = apps_dir else {
        out.push_str(
            "\n## Other examples\n\nKrate ships around thirty more example apps, but \
             their source is **not on this machine** -- it lives in the Krate \
             repository. Do not go looking for an `apps/` directory and do not search \
             the filesystem for one. Everything you need to write this app is in this \
             file: the example above, the API reference in section 1, and the rules in \
             section 3.\n",
        );
        return out;
    };

    let mut examples = collect_examples(&apps_dir);
    examples.sort_by(|a, b| a.name.cmp(&b.name));

    out.push_str("\n## Other examples on this machine\n\n");
    out.push_str(&format!(
        "These are readable at `{}`. Find the row closest to your request and read \
         that app's `src/lib.rs` and `manifest.toml`.\n\n",
        apps_dir.display()
    ));
    out.push_str("| app | kind | capabilities | what it shows |\n");
    out.push_str("|---|---|---|---|\n");
    for ex in &examples {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            ex.name,
            ex.kind,
            if ex.caps.is_empty() {
                "(defaults only)".to_string()
            } else {
                ex.caps.join(", ")
            },
            ex.shape,
        ));
    }
    out.push('\n');
    out
}

/// A whole small GUI app, carried in the pack itself.
///
/// Every other example lives under `apps/` in the repository, which a person who
/// installed a release does not have. Naming those paths sent the agent hunting
/// the filesystem for them. This one is always present, so there is real working
/// code to adapt no matter where the pack is generated.
const WORKED_EXAMPLE: &str = r####"### `src/lib.rs`

```rust
#![no_std]

// The SDK owns the allocator, the panic handler, and the memory intrinsics
// this guest needs. Nothing calls it directly, so link it explicitly or the
// build fails with "`#[panic_handler]` function required".
extern crate krate as _krate_runtime;

#[allow(warnings)]
mod bindings;

use bindings::krate::gfx::{canvas2d, types as gfx};
use bindings::krate::io::{args, stdio};
use bindings::krate::ui::{events, tree, types, window};

// Widget ids are yours to choose. Keep them as constants: the tree refers to
// them and so does `canvas2d::bind`.
const ROOT_ID: u64 = 1;
const CANVAS_ID: u64 = 2;

const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 320.0;

struct Component;

/// The button rectangle, defined once. Drawing and hit-testing both call this,
/// so they cannot disagree about where the button is.
fn button_rect(width: f32, height: f32) -> gfx::Rect {
    gfx::Rect { x: width / 2.0 - 60.0, y: height - 80.0, width: 120.0, height: 40.0 }
}

fn rgb(r: f32, g: f32, b: f32) -> gfx::Color {
    gfx::Color { r, g, b, a: 1.0 }
}

impl bindings::Guest for Component {
    // The world exports `run: func() -> s32`. No arguments, no Result: read
    // arguments with `args::raw()` and return 0 for success.
    fn run() -> i32 {
        // check-app runs every app once with the bare word `quick`, and kills
        // it after 60 seconds. `args::raw()` is one string, arguments
        // separated by newlines.
        let raw = args::raw();
        let quick = raw
            .as_bytes()
            .split(|byte| *byte == b'\n')
            .any(|arg| arg == b"quick");

        let size = types::WindowSize { width: WIDTH as u32, height: HEIGHT as u32 };
        let Ok(win) = window::create("Counter", size) else { return 30 };
        if window::show(win).is_err() {
            return 31;
        }
        // A window has no widgets until you build the tree.
        if tree::set_root(win, &node(ROOT_ID, None, types::WidgetKind::Stack)).is_err()
            || tree::upsert_node(
                win,
                &node(CANVAS_ID, Some(ROOT_ID), types::WidgetKind::Canvas),
            )
            .is_err()
        {
            let _ = window::close(win);
            return 32;
        }
        let Ok(canvas) = canvas2d::bind(win, CANVAS_ID) else {
            let _ = window::close(win);
            return 33;
        };

        let mut count: i32 = 0;
        let mut dirty = true;
        // Wait in short rounds rather than blocking forever or spinning.
        //
        // This is the shape that passes the usability stage, and getting it
        // wrong is the most common way a finished app fails its last check.
        // `wait(None)` blocks until an event arrives, so a headless run hangs
        // and is killed at 60 seconds. `poll()` never waits, so the loop spins
        // a core and a quick run ends before the checker can click anything.
        // A timeout does both jobs: idle costs nothing, and a quick run gives
        // the checker its ten seconds and then exits on its own.
        const ROUND_MILLIS: u32 = 33;
        const QUICK_IDLE_ROUNDS: u32 = 300; // ~10 seconds of quiet
        let mut idle: u32 = 0;

        loop {
            if dirty {
                let Ok(size) = canvas2d::canvas_size(canvas) else { break };
                canvas2d::clear(canvas, rgb(0.11, 0.12, 0.15)).ok();

                let mut label = [0u8; 12];
                let text = format_int(count, &mut label);
                canvas2d::draw_text(
                    canvas,
                    text,
                    gfx::Point { x: size.width / 2.0 - 14.0, y: size.height / 2.0 },
                    48.0,
                    rgb(0.93, 0.94, 0.96),
                )
                .ok();

                let b = button_rect(size.width, size.height);
                canvas2d::fill_rect(canvas, b, rgb(0.22, 0.45, 0.85)).ok();
                canvas2d::draw_text(
                    canvas,
                    "Add one",
                    gfx::Point { x: b.x + 20.0, y: b.y + 26.0 },
                    16.0,
                    rgb(1.0, 1.0, 1.0),
                )
                .ok();

                canvas2d::present(canvas).ok();
                dirty = false;
            }

            // Interactive: block until something happens, so the app sits idle
            // instead of burning a core. Quick: never block, and stop after a
            // bounded number of rounds.
            let event = if quick {
                rounds += 1;
                if rounds > QUICK_ROUNDS {
                    break;
                }
                events::poll()
            } else {
                events::wait(None)
            };

            match event {
                // Always handle this. It is what the window's close button and
                // Ctrl-C both send; an app that ignores it cannot be closed.
                Some(types::Event::CloseRequested(_)) => break,
                // One Pointer event covers press and release; check `pressed`.
                Some(types::Event::Pointer(p)) if p.pressed => {
                    if let Ok(size) = canvas2d::canvas_size(canvas) {
                        let b = button_rect(size.width, size.height);
                        if p.x >= b.x && p.x <= b.x + b.width
                            && p.y >= b.y && p.y <= b.y + b.height
                        {
                            count += 1;
                            dirty = true;
                        }
                    }
                }
                Some(types::Event::Resized(_)) => dirty = true,
                _ => {}
            }
        }

        if quick {
            let out = stdio::stdout();
            let _ = out.write(b"counter: window ran\n");
        }
        let _ = window::close(win);
        0
    }
}

/// Integers to text without `alloc` or `format!`.
fn format_int(mut value: i32, buffer: &mut [u8; 12]) -> &str {
    let negative = value < 0;
    let mut at = buffer.len();
    if value == 0 {
        at -= 1;
        buffer[at] = b'0';
    }
    while value != 0 {
        at -= 1;
        buffer[at] = b'0' + (value % 10).unsigned_abs() as u8;
        value /= 10;
    }
    if negative {
        at -= 1;
        buffer[at] = b'-';
    }
    core::str::from_utf8(&buffer[at..]).unwrap_or("0")
}

/// One widget node with the default style.
fn node(id: u64, parent: Option<u64>, kind: types::WidgetKind) -> types::WidgetNode {
    types::WidgetNode {
        id,
        parent,
        kind,
        label: None,
        role: None,
        style: types::Style { width: None, height: None, grow: 0.0, padding: 0.0 },
        checked: None,
        value: None,
        selected: None,
        text_cursor: None,
    }
}

bindings::export!(Component with_types_in bindings);
```

### `manifest.toml`

**Keep the world the starter chose.** The skeleton you were handed already
declares the right world for this request -- `krate:app/gui@0.2.0` for an app
with a window, `krate:app/cli@0.1.0` for a command-line tool that prints and
exits (check `KRATE_APP_KIND`, or the `world =` line already in the manifest).
Do not change it to match this example: a request for "a command-line tool"
built as a window app fails the checks over and over, because a window must
stay open and a CLI must exit. The example below happens to be a GUI app;
a CLI manifest is identical except `world = "krate:app/cli@0.1.0"` and no
`ui.window` capability.

```toml
[app]
id = "dev.krate.counter"
name = "Counter"
version = "0.1.0"
entry = "target/wasm32-wasip1/release/counter.wasm"
world = "krate:app/gui@0.2.0"

[[capabilities]]
cap = "ui.window:create"
rationale = "Open the counter window"
required = true

[[capabilities]]
cap = "io.stdout"
rationale = "Report the count on a quick run"
required = true

[[capabilities]]
cap = "io.args"
rationale = "Read the quick-run flag used by automated checks"
required = true
```
"####;

/// One row of the example index.
struct Example {
    name: String,
    /// "GUI" or "CLI", from the app's world.
    kind: &'static str,
    /// The non-default capabilities it declares, by name.
    caps: Vec<String>,
    /// A one-line description of the app's shape.
    shape: String,
}

fn collect_examples(apps_dir: &Path) -> Vec<Example> {
    let mut examples = Vec::new();
    let Ok(entries) = std::fs::read_dir(apps_dir) else {
        return examples;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let manifest_path = dir.join("manifest.toml");
        let lib_path = dir.join("src/lib.rs");
        if !manifest_path.exists() || !lib_path.exists() {
            continue;
        }
        let Ok(manifest) = krate_manifest::Manifest::parse_file(&manifest_path) else {
            continue;
        };
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let kind = if manifest
            .capabilities
            .iter()
            .any(|c| c.cap.starts_with("ui.window"))
        {
            "GUI"
        } else {
            "CLI"
        };
        let caps: Vec<String> = manifest
            .capabilities
            .iter()
            .map(|c| format!("`{}`", c.cap))
            .collect();
        let shape = app_shape(&name, &lib_path);
        examples.push(Example {
            name,
            kind,
            caps,
            shape,
        });
    }
    examples
}

/// A one-line shape for an app. Prefers the app's own `//!` doc header when it
/// is descriptive; otherwise falls back to a curated line by name, and finally
/// to a generic note. The curated map keeps the index readable for apps whose
/// header is the no_std boilerplate rather than a description.
fn app_shape(name: &str, lib_path: &Path) -> String {
    if let Some(curated) = curated_shape(name) {
        return curated.to_string();
    }
    if let Ok(source) = std::fs::read_to_string(lib_path) {
        for line in source.lines().take(6) {
            let trimmed = line.trim();
            if let Some(doc) = trimmed.strip_prefix("//!") {
                let doc = doc.trim();
                // Skip the no_std boilerplate header some apps lead with.
                if !doc.is_empty() && !doc.to_lowercase().contains("no_std") {
                    return doc.trim_end_matches('.').to_string();
                }
            }
        }
    }
    String::from("a Krate app")
}

/// Curated one-liners for the apps most useful as starting points. Keyed by
/// directory name. Falls through to the doc-header heuristic for anything not
/// listed, so a new app appears in the index the day it is added.
fn curated_shape(name: &str) -> Option<&'static str> {
    Some(match name {
        "krate-notes" => "GUI list + local save; the flagship, 7 capabilities",
        "krate-checklist" => "GUI checkbox list that saves; the simplest GUI+store shape",
        "krate-contacts" => "GUI over a real SQL database (no_std)",
        "krate-keyvault" => "GUI key-value store over store.kv (persistence)",
        "krate-notes-clip" | "krate-clip" => "GUI using the clipboard",
        "krate-paint" => "GUI freehand canvas drawing",
        "krate-chart" => "GUI drawing a chart on a canvas",
        "krate-nova" => "GUI canvas game: fills, circles, gradients (no sprites)",
        "krate-nova2" => "GUI canvas game with textured sprites and image assets",
        "krate-bounce" => "GUI canvas game, the smallest playable loop",
        "krate-cubes" => "GUI animated canvas, no input",
        "krate-fractal" => "GUI image widget via set_pixels (no_std)",
        "krate-tidy" => "GUI folder tidier: pick-is-the-grant, zero fs capabilities (no_std)",
        "krate-gram" => {
            "GUI photo feed: canvas-size layout, momentum scroll, springs, shadows (no_std)"
        }
        "krate-spriteproof" => "GUI sprite-drawing proof, the draw-sprite pipeline",
        "krate-hello-gui" => "the smallest GUI app: one window, one label",
        "krate-fetch" => "GUI that fetches over the network and shows the result",
        "krate-cat" => "CLI that reads files and prints them (no_std)",
        "krate-curl" => "CLI that fetches a URL over the network (no_std)",
        "krate-clock" => "CLI that prints the time (no_std)",
        "krate-diceroll" => "CLI using rand via the getrandom backend (no_std)",
        _ => return None,
    })
}

/// Find the `apps/` directory by walking up from the app dir to a workspace
/// root. The generated app usually lives in a temp dir far from the repo, so we
/// also try the workspace root of this binary's own crate.
fn find_apps_dir(app_dir: &Path) -> Option<std::path::PathBuf> {
    // Walk up from the app dir first: a `krate create --work-dir` inside the
    // repo will find the sibling apps tree this way.
    let mut current = Some(app_dir);
    while let Some(dir) = current {
        let candidate = dir.join("apps");
        if candidate.join("krate-notes").join("manifest.toml").exists() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    // Fall back to the workspace this binary was built in, so the pack still
    // carries the index when the app dir is a temp dir outside the repo.
    let build_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let candidate = build_root.join("apps");
    if candidate.join("krate-notes").join("manifest.toml").exists() {
        return Some(candidate);
    }
    None
}

/// The essentials of the pack, assembled for INLINING into the prompt.
///
/// The study measured 4 to 11 minutes of every build spent on the agent
/// reading the pack and hunting examples through its own file tools. The
/// EXAMPLE.rs pre-pick cut the hunting; this cuts the reading round-trips to
/// zero for everything except exact SDK signatures: the design patterns, the
/// backend and shared-store shapes, the no_std rules, the capability
/// catalog, and the picked example's full source ride inside the prompt
/// itself. Everything here is the same generated text the pack carries, so
/// nothing can drift.
pub fn inline_essentials(request: &str) -> String {
    let example = closest_example(request);
    let mut out = String::new();
    out.push_str(
        "

---

THE ESSENTIALS, INLINED. Everything below is authoritative and          identical to the pack on disk -- do not re-read it from files.
",
    );
    out.push_str(&capability_catalog_section());
    out.push_str(DESIGN_PATTERNS_SECTION);
    out.push_str(BACKEND_CLIENT_SECTION);
    out.push_str(SENSES_SECTION);
    // Game feel rides the prompt only when the request smells like a game;
    // for everything else it is dead weight. The pack on disk always carries
    // it, so a mis-detected game still finds it in section 2f there.
    let lower = request.to_lowercase();
    if [
        "game", "arcade", "platform", "shooter", "puzzle", "snake", "tetris", "breakout", "pong",
        "invader", "climber", "runner", "jump", "maze", "asteroid", "flappy",
    ]
    .iter()
    .any(|w| lower.contains(w))
    {
        out.push_str(GAME_FEEL_SECTION);
    }
    out.push_str(NO_STD_SECTION);
    out.push_str(&format!(
        "
---

# Your model app, inlined: `{}` ({})

         Adapt this proven, working code -- do not write the no_std/krate:*          discipline from a blank page.

### manifest.toml

```toml
{}
```

         ### src/lib.rs

```rust
{}
```
",
        example.name, example.shows, example.manifest, example.lib,
    ));
    out
}

/// One shipped example, carried inside the binary.
///
/// The pack's example index points at `apps/` -- which exists only in a repo
/// checkout. On an installed Krate the agent had exactly one worked example
/// and no model for a game, a database app, a fetch app, or a timer, and the
/// prompt's "find the closest example" sent it hunting a directory that is
/// not there. These five cover the shapes people actually ask for; Krate
/// picks the closest one for the request and writes it into the workspace as
/// `EXAMPLE.rs`, so the agent's model app costs one read on any machine.
pub struct EmbeddedExample {
    pub name: &'static str,
    /// What this example is the model FOR, in the prompt's own words.
    pub shows: &'static str,
    keywords: &'static [&'static str],
    pub lib: &'static str,
    pub manifest: &'static str,
}

pub const EMBEDDED_EXAMPLES: &[EmbeddedExample] = &[
    EmbeddedExample {
        name: "krate-bounce",
        shows: "a canvas game: its own event loop, key-held movement, collision, redraw pacing",
        keywords: &[
            "game",
            "ball",
            "arcade",
            "jump",
            "physics",
            "gravity",
            "breaker",
            "snake",
            "runner",
            "platform",
            "shoot",
            "invader",
            "tetris",
            "pong",
            "maze",
            "play",
            "score",
            "enemy",
            "level",
            "animate",
            "animation",
            "particle",
            "draw",
            "paint",
        ],
        lib: include_str!("../../../apps/krate-bounce/src/lib.rs"),
        manifest: include_str!("../../../apps/krate-bounce/manifest.toml"),
    },
    EmbeddedExample {
        name: "krate-checklist",
        shows: "a list app that saves: widget tree, text input, buttons, store.kv persistence",
        keywords: &[
            "list", "todo", "task", "check", "track", "habit", "note", "item", "grocery",
            "shopping", "journal", "log", "streak", "goal", "plan", "remember", "save", "share",
            "family", "wife", "husband", "partner", "roommate", "together",
        ],
        lib: include_str!("../../../apps/krate-checklist/src/lib.rs"),
        manifest: include_str!("../../../apps/krate-checklist/manifest.toml"),
    },
    EmbeddedExample {
        name: "krate-contacts",
        shows: "records in a real database: store.sql schema, insert, query, list and detail views",
        keywords: &[
            "database",
            "record",
            "contact",
            "crm",
            "inventory",
            "catalog",
            "sql",
            "address",
            "customer",
            "collection",
            "library",
            "expense",
            "budget",
        ],
        lib: include_str!("../../../apps/krate-contacts/src/lib.rs"),
        manifest: include_str!("../../../apps/krate-contacts/manifest.toml"),
    },
    EmbeddedExample {
        name: "krate-fetch",
        shows:
            "an app that reaches the internet: net.http requests, async polling, showing results",
        keywords: &[
            "fetch", "api", "weather", "news", "internet", "http", "online", "quote", "stock",
            "crypto", "price", "feed", "download", "search", "lookup",
        ],
        lib: include_str!("../../../apps/krate-fetch/src/lib.rs"),
        manifest: include_str!("../../../apps/krate-fetch/manifest.toml"),
    },
    EmbeddedExample {
        name: "krate-focus",
        shows: "a timer: the wait(timeout) clock loop, time.monotonic pacing, start/pause/reset",
        keywords: &[
            "timer",
            "pomodoro",
            "clock",
            "countdown",
            "stopwatch",
            "alarm",
            "session",
            "focus",
            "break",
            "interval",
            "minute",
            "remind",
        ],
        lib: include_str!("../../../apps/krate-focus/src/lib.rs"),
        manifest: include_str!("../../../apps/krate-focus/manifest.toml"),
    },
];

/// The embedded example closest to a request, by keyword hits. Ties and
/// no-hits fall to the checklist: a list that saves is the most common shape
/// asked for, and its tree/input/persist trio transfers to almost anything.
pub fn closest_example(request: &str) -> &'static EmbeddedExample {
    let lower = request.to_lowercase();
    EMBEDDED_EXAMPLES
        .iter()
        .max_by_key(|ex| ex.keywords.iter().filter(|k| lower.contains(**k)).count())
        .filter(|ex| ex.keywords.iter().any(|k| lower.contains(*k)))
        .unwrap_or(&EMBEDDED_EXAMPLES[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn wit_extraction_reads_single_line_functions() {
        let wit = "\
package krate:x@0.1.0;
interface canvas {
  /// draw
  fill-rect: func(canvas: u64, area: rect, fill: color) -> result<_, gfx-error>;
  present: func(canvas: u64) -> result<_, gfx-error>;
}
";
        let out = render_wit_interfaces("x", wit);
        assert!(out.contains(
            "canvas::fill-rect: func(canvas: u64, area: rect, fill: color) -> result<_, gfx-error>"
        ));
        assert!(out.contains("canvas::present: func(canvas: u64) -> result<_, gfx-error>"));
    }

    #[test]
    fn wit_extraction_joins_multiline_functions() {
        // The speech interface wraps its parameters across lines; the whole
        // signature must collapse onto one readable line.
        let wit = "\
package krate:s@0.1.0;
interface transcription {
  transcribe: func(
    model-asset: string,
    pcm-s16-le: list<u8>,
  ) -> result<transcript, speech-error>;
}
";
        let out = render_wit_interfaces("s", wit);
        assert!(
            out.contains("transcription::transcribe: func( model-asset: string, pcm-s16-le: list<u8>, ) -> result<transcript, speech-error>"),
            "multi-line signature should join: {out}"
        );
    }

    #[test]
    fn wit_extraction_ignores_record_fields() {
        // A field inside a record is `name: type,` and must not be mistaken for
        // a function. Only `name: func(...)` counts.
        let wit = "\
package krate:t@0.1.0;
interface types {
  record color {
    r: f32,
    g: f32,
  }
}
interface api {
  go: func() -> result<_, e>;
}
";
        let out = render_wit_interfaces("t", wit);
        assert!(!out.contains("color"), "records are not functions: {out}");
        assert!(!out.contains("r: f32"), "fields are not functions: {out}");
        assert!(out.contains("api::go: func() -> result<_, e>"));
    }

    #[test]
    fn the_pack_carries_every_section() {
        // Generated against this repo's own tree, so the example index is
        // present too. Guards that no section silently disappears.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let pack = generate(&root);
        assert!(pack.contains("# 1. The SDK"));
        assert!(pack.contains("# 2. Capabilities"));
        assert!(pack.contains("# 3. Passing the import check"));
        assert!(pack.contains("# 4. The GUI world"));
        assert!(pack.contains("# 5. A complete worked example"));
        // Real, load-bearing facts must appear, from their real sources.
        assert!(pack.contains("random.bytes"), "capability catalog");
        assert!(pack.contains("canvas2d::present"), "gfx WIT");
        assert!(pack.contains("krate-notes"), "example index");
        assert!(pack.contains("getrandom-backend"), "the getrandom remedy");
    }

    #[test]
    fn a_machine_without_the_repo_is_told_not_to_go_looking() {
        // The pack used to say the examples "live under `apps/` in the Krate
        // repository" and the prompt told the agent to read the closest one.
        // On a machine with no repo that became `find / -name krate-paint`,
        // and an authoring run that should take two minutes took eight.
        //
        // `example_index_section` is called with the apps tree already
        // resolved, so this exercises the no-repo branch directly rather than
        // relying on a temp dir -- find_apps_dir falls back to the workspace
        // this binary was built in, which exists here and never exists on a
        // user's machine.
        let pack = example_index_section_for(None);

        assert!(
            pack.contains("not on this machine"),
            "must say plainly that the example sources are absent"
        );
        assert!(
            pack.contains("do not search the filesystem"),
            "must forbid the search that caused the hang"
        );
        // And it must still carry real code, or the agent has nothing to copy.
        assert!(pack.contains("#![no_std]"), "the worked example is inlined");
        assert!(pack.contains("events::wait"), "its event loop is inlined");
    }

    #[test]
    fn the_worked_example_is_always_present() {
        // With or without the apps tree, the pack carries one complete app.
        // This is what replaced pointing at files that may not exist.
        let with_repo = find_apps_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
        assert!(with_repo.is_some(), "this repo has an apps tree");
        for apps in [None, with_repo] {
            let pack = example_index_section_for(apps);
            assert!(pack.contains("### `src/lib.rs`"), "example source");
            assert!(pack.contains("### `manifest.toml`"), "example manifest");
            // The manifest shape a real app needs, not a plausible-looking one.
            assert!(pack.contains("[app]"), "manifest [app] table");
            assert!(
                pack.contains("required = true"),
                "capabilities are required"
            );
        }
    }

    #[test]
    fn the_example_index_reports_apps_by_their_real_world() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let apps = find_apps_dir(&root).expect("apps dir found from workspace root");
        let examples = collect_examples(&apps);
        let notes = examples
            .iter()
            .find(|e| e.name == "krate-notes")
            .expect("krate-notes is indexed");
        assert_eq!(notes.kind, "GUI", "notes opens a window");
        let cat = examples
            .iter()
            .find(|e| e.name == "krate-cat")
            .expect("krate-cat is indexed");
        assert_eq!(cat.kind, "CLI", "cat has no window");
    }
}
