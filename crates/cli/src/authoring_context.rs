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
## Motion that reads as polish\n\n\
The SDK ships `krate::motion` (no_std, no capability): `ease_out`, \
`ease_in_out`, `smoothstep`, and a critically-damped `Spring`. Measure dt \
from `time.clock` each frame, tick, draw, request the next frame:\n\n\
```rust\n\
use krate::motion::Spring;\n\
let mut x = Spring::rest_at(0.0, 20.0);   // stiffness 10 calm, 30 snappy\n\
// each frame:\n\
x.tick(target, dt);\n\
draw_at(x.value);\n\
if !x.settled(target) { /* request another frame */ }\n\
```\n\n\
Rules that keep it tasteful: ease-out for anything arriving, springs for \
anything following input, 150-300ms for interface moves, and nothing loops \
forever except ambient glow. Cards get `fill-round-rect` + \
`drop-shadow-round-rect` (shadow first, offset a few pixels down); progress \
is `stroke-arc` from -90 degrees; big numbers read best at weight 600-700 \
with slightly negative letter-spacing via `draw-text-styled`.\n";

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
         fails its own wall test after building cleanly.\n",
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
        _ => "",
    }
}

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
`no_std` guest needs. This is the step that is most often missed.\n\
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
rows and a \"+ 26 more\" label has lost the other twenty-six for good. The \
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
**Clip the list region before you draw the rows.** Without it a row scrolled \
past the top paints over your header. Set the clip to the list's rectangle, \
draw every row including the partly-visible ones, then clear it:\n\n\
\u{20}\u{20}\u{20}\u{20}canvas2d::set_clip(canvas, list_x, list_y, list_w, list_h);\n\
\u{20}\u{20}\u{20}\u{20}// draw every row; the ones outside the rectangle are trimmed\n\
\u{20}\u{20}\u{20}\u{20}canvas2d::clear_clip(canvas);\n\n\
Do not try to skip out-of-view rows by hand instead. It works until rows have \
different heights, and then it cannot be done correctly at all.\n\n\
## Packing: the entry name changes\n\n\
Your `manifest.toml` points `entry` at the build output, which is right for \
building and for `check-app`. Inside a packed `.krate` the component is stored \
under one fixed name, so the manifest that goes into the bundle must say \
`entry = \"code.wasm\"` instead. `krate create` handles this for you; `krate \
pack` expects the bundle form and will refuse the development one.\n\n\
## The verification run\n\n\
`check-app` (and `create`) run the app once with every capability granted and \
one argument, requiring exit 0. The argument is the bare word `quick` (not \
`--quick`), except a CLI app that declares `fs.read:` and no window is given a \
sample file path instead. Handle `quick` before any other argument parsing: on \
`quick`, do the real work once against a small built-in sample (or empty \
stdin), print what the app is holding, and exit 0. Never wait for input or open \
a window nobody will close. An app that parses arguments strictly and rejects \
the unknown `quick` fails here after building and packing correctly.\n\n\
**Print one `key:value` per line, and make the keys mean something.** This is \
the only way anything outside the app can tell whether it did what was asked. \
`check-app` reads it, CI reads it, and a benchmark reads it. \"print something\" \
is not enough: an app that prints `ok` builds, runs, paints a frame, and proves \
nothing about whether it works.\n\n\
Print the state a person would look at to judge the app. A to-do list prints \
how many items it holds and how many are done; a tip calculator prints the tip \
and the total; a game prints the score and whether it is over:\n\n\
\u{20}\u{20}\u{20}\u{20}items:5\n\
\u{20}\u{20}\u{20}\u{20}done:2\n\
\u{20}\u{20}\u{20}\u{20}saved:yes\n\n\
Lower-case keys, no spaces around the colon, one pair per line, numbers as \
bare digits. Seed enough state in the `quick` path that the numbers are \
interesting -- a to-do list that prints `items:0` has proved nothing either.\n";

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
        ("speech", SPEECH_WIT),
    ] {
        out.push_str(&render_wit_interfaces(package, wit));
    }
    out.push_str(
        "\n## Never guess how wide text is -- measure it\n\n\
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
        "krate-gram" => "GUI photo feed: canvas-size layout, momentum scroll, springs, shadows (no_std)",
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
