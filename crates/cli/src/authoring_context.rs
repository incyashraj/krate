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
the wrong row the moment somebody scrolls. Skip rows that fall outside the \
visible region rather than drawing them over your header: there is no clip \
rectangle yet, so anything you draw above the list region lands on top of it. \
`apps/krate-checklist` scrolls this way and is the one to copy.\n\n\
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
        "\nStart from the closest example in section 5 rather than these signatures \
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
    let Some(apps_dir) = find_apps_dir(app_dir) else {
        // No apps tree to index (e.g. a released binary without the repo). The
        // rest of the pack stands on its own; say the corpus is elsewhere.
        return String::from(
            "\n---\n\n# 5. Example apps\n\nThe shipped example apps are the best teacher \
             for a new app's shape. They live under `apps/` in the Krate repository.\n",
        );
    };

    let mut examples = collect_examples(&apps_dir);
    examples.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = String::from("\n---\n\n# 5. Example apps: read the closest one first\n\n");
    out.push_str(
        "Every app here ships and passes CI, so its code is a proven pattern to adapt \
         -- especially its `#![no_std]` shape and its manifest. Find the row closest to \
         your request and start from that app's `src/lib.rs` and `manifest.toml`.\n\n",
    );
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
        assert!(pack.contains("# 5. Example apps"));
        // Real, load-bearing facts must appear, from their real sources.
        assert!(pack.contains("random.bytes"), "capability catalog");
        assert!(pack.contains("canvas2d::present"), "gfx WIT");
        assert!(pack.contains("krate-notes"), "example index");
        assert!(pack.contains("getrandom-backend"), "the getrandom remedy");
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
