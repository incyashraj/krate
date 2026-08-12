//! Krate Mode: the paste-in prompt that teaches any chat model to write Krate apps.
//!
//! `KRATE_AUTHORING.md` (see `crate::authoring_context`) is written into an app
//! directory for an agent that has the repo, a scaffolded skeleton, and a
//! working `krate` binary it can run in a loop. Krate Mode addresses the other
//! situation, which is most people: someone in ChatGPT, Claude, or Cursor with
//! no Krate installed and nothing on disk. The model cannot compile, cannot run
//! the oracle, and cannot read `apps/`. So the prompt has to carry three things
//! the pack gets for free:
//!
//! 1. **Complete files.** No skeleton exists to edit, so the model must emit
//!    `Cargo.toml`, `src/lib.rs`, and `manifest.toml` in full -- exactly the
//!    three files `check-app` demands at its layout stage.
//! 2. **Whole worked examples, inline.** Section 5 of the pack is an index that
//!    says "read `apps/krate-clock`". A chat model cannot. So Krate Mode inlines
//!    real shipped source instead of naming it.
//! 3. **An honest handoff.** The pack's loop is "run check-app until OK". A chat
//!    model has no oracle, so the prompt must say plainly that it cannot verify
//!    its own output and hand the build back to the person.
//!
//! Everything factual is generated from the same sources the pack uses -- the
//! SDK crate, the capability registry, the WIT, and the shipped apps, the last
//! by `include_str!` so an example cannot rot into code that no longer compiles.
//! Only the prose that is genuinely Krate-Mode-specific is authored here.
//!
//! `docs/krate-mode.md` is the published copy, and
//! `the_published_prompt_matches_the_generator` asserts it still matches. A
//! stale prompt teaches a dead API, which is worse than shipping no prompt.

use crate::authoring_context;

/// Real shipped source, baked in at compile time. These are the worked examples,
/// and they are the files CI actually builds -- not transcriptions of them. If
/// one stops compiling, it stops shipping, and the prompt stops teaching it.
const CLOCK_SOURCE: &str = include_str!("../../../apps/krate-clock/src/lib.rs");
const CLOCK_MANIFEST: &str = include_str!("../../../apps/krate-clock/manifest.toml");
const CHECKLIST_SOURCE: &str = include_str!("../../../apps/krate-checklist/src/lib.rs");
const CHECKLIST_MANIFEST: &str = include_str!("../../../apps/krate-checklist/manifest.toml");

/// Build the whole Krate Mode prompt.
///
/// Section order is deliberate: what Krate is, then the output contract, then
/// the two Cargo.toml templates, then the rules that decide between them, then
/// the generated API surface, and the worked examples last. A model that stops
/// reading early has still seen the parts that decide whether the code compiles.
pub fn generate() -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push_str(OUTPUT_RULES);
    out.push_str(CARGO_TEMPLATES);
    out.push_str(NO_STD_RULES);
    out.push_str(API_PREAMBLE);
    out.push_str(&authoring_context::sdk_surface_section());
    out.push_str(&authoring_context::capability_catalog_section());
    out.push_str(&authoring_context::gui_world_section());
    out.push_str(&worked_examples_section());
    out.push_str(HANDOFF);
    out
}

const HEADER: &str = "\
# Krate Mode

You are writing a Krate app. Read this whole file before you write any code, and
follow it exactly.

## What Krate is

A Krate app is a **WebAssembly component compiled from ordinary Rust**. It is not
a config file, not a JSON agent spec, and not a plugin manifest -- there is no
`add_tool` to call and no schema to fill in. You write real Rust against the API
in this document, and a compiler turns it into one file.

That one file is **capability-gated**: it can reach the filesystem, the network,
or the clipboard only where its manifest declares it, and the person running it
grants it. A component that tries to reach the operating system any other way is
refused before it runs.

The result is **one `.krate` file that runs unchanged on macOS, Windows, and
Linux** -- no installer, no runtime to set up on the other end.

";

const OUTPUT_RULES: &str = "\
---

## Output rules -- these are hard

**Emit three complete files, every time.** Not fragments, not diffs, not \"the
rest stays the same\". A Krate app directory is exactly:

    Cargo.toml
    src/lib.rs
    manifest.toml

The build tool checks for all three before it does anything else, and a missing
one fails immediately. Give the person the full contents of each, in its own
fenced code block, labelled with its filename.

**Never invent a capability name.** The manifest may only declare names from the
capability table below. Anything else is refused when the app is packed. If the
app needs something not in that table, say so in plain words instead of guessing
a name that looks plausible.

**Never invent a function.** Call only what appears in the API sections below. If
a function you want is not listed, it does not exist -- solve the problem with
what is there, or say what is missing. Inventing a call is the single most common
way a generated Krate app fails, and it fails at compile time with a confusing
error the person has to debug.

**Never reach the operating system through `std`.** No `std::fs`, `std::io`
(including `println!` and `eprintln!`), `std::time`, `std::env`, `std::process`,
`std::net`, or `std::thread`. Use the `krate::*` equivalents. Ordinary in-memory
std -- `String`, `Vec`, `HashMap`, iterators -- is fine and does not leak.

**Handle the argument `quick`.** Before any other argument parsing, check for the
bare word `quick` (not `--quick`, not a flag). On `quick`, do the app's real work
once against a small built-in sample, print what the app is holding, and exit 0
-- never wait for input, never sit on an open window. The verification run passes
exactly this argument, and an app that parses arguments strictly and rejects it
fails after building perfectly.

**Print one `key:value` per line, and make the keys mean something.** This is the
only way anything outside the app can tell whether it did what was asked --
`check-app` reads it, CI reads it, and the benchmark reads it. Printing just
`ok` is not enough: such an app builds, runs, paints a frame, and proves
nothing about whether it works.

Print the state a person would look at to judge the app. A to-do list prints how
many items it holds and how many are done; a tip calculator prints the tip and
the total; a game prints the score and whether it is over:

    items:5
    done:2
    saved:yes

Lower-case keys, no spaces around the colon, one pair per line, numbers as bare
digits. Seed enough state in the `quick` path that the numbers are interesting --
a to-do list that prints `items:0` has proved nothing either.

This is not a style note. Five apps were measured on 2026-08-05 and every one of
them worked and could not prove it: a tip calculator computed the right answer
and printed `bill:60 tip%:18 people:2 total_cents:7080` -- three keys on one
line, with invented names -- and a dice roller printed nothing at all. All five
scored zero.

";

const CARGO_TEMPLATES: &str = "\
---

## The two Cargo.toml templates

Copy one of these verbatim and change only `NAME`. The `[profile.release]`
settings are load-bearing, not tuning: without `panic = \"abort\"` and
`opt-level = \"s\"`, dead-code elimination is not aggressive enough to drop std's
latent OS imports, and the component is rejected.

`PREFIX` is the path from the app directory to the Krate source checkout. Leave
it as the literal `PREFIX` and tell the person to replace it, or better, tell
them to run `krate create` (see the handoff at the end), which fills these paths
in correctly for their machine.

### A CLI app (no window)

```toml
# An empty [workspace] table makes the app its own workspace root, so it builds
# standalone even inside another cargo workspace's directory tree.
[workspace]

[package]
name = \"NAME\"
version = \"0.1.0-dev\"
edition = \"2021\"
rust-version = \"1.91\"

[dependencies]
krate = { path = \"PREFIX/crates/bindings-rust\" }
wit-bindgen-rt = { version = \"0.44.0\", features = [\"bitflags\"] }

[lib]
crate-type = [\"cdylib\"]

[package.metadata.component]
package = \"krate:NAME\"

[package.metadata.component.target]
path = \"PREFIX/wit/krate/phase2\"
world = \"cli\"

[package.metadata.component.target.dependencies]
\"krate:io\" = { path = \"PREFIX/wit/krate/phase2/deps/io\" }
\"krate:fs\" = { path = \"PREFIX/wit/krate/phase2/deps/fs\" }
\"krate:net\" = { path = \"PREFIX/wit/krate/phase2/deps/net\" }
\"krate:time\" = { path = \"PREFIX/wit/krate/phase2/deps/time\" }
\"krate:locale\" = { path = \"PREFIX/wit/krate/phase2/deps/locale\" }
\"krate:resources\" = { path = \"PREFIX/wit/krate/phase2/deps/resources\" }
\"krate:store\" = { path = \"PREFIX/wit/krate/phase2/deps/store\" }
\"krate:random\" = { path = \"PREFIX/wit/krate/phase2/deps/random\" }

[profile.release]
panic = \"abort\"
lto = true
codegen-units = 1
opt-level = \"s\"
```

A CLI app writes `krate::export!(Component);` at the end of `src/lib.rs` and
implements `krate::Guest`.

### A GUI app (a window)

Same as above with three changes: the WIT world is `gui` under `phase3`, four
more WIT packages are listed, and the bindings need `std_feature = true`.

```toml
[workspace]

[package]
name = \"NAME\"
version = \"0.1.0-dev\"
edition = \"2021\"
rust-version = \"1.91\"

[dependencies]
krate = { path = \"PREFIX/crates/bindings-rust\" }
wit-bindgen-rt = { version = \"0.44.0\", features = [\"bitflags\"] }

[lib]
crate-type = [\"cdylib\"]

# Puts the generated `impl std::error::Error` behind a feature nobody turns on,
# which is what lets a windowed app be #![no_std] at all.
[package.metadata.component.bindings]
std_feature = true

[package.metadata.component]
package = \"krate:NAME\"

[package.metadata.component.target]
path = \"PREFIX/wit/krate/phase3\"
world = \"gui\"

[package.metadata.component.target.dependencies]
\"krate:io\" = { path = \"PREFIX/wit/krate/phase3/deps/io\" }
\"krate:fs\" = { path = \"PREFIX/wit/krate/phase3/deps/fs\" }
\"krate:net\" = { path = \"PREFIX/wit/krate/phase3/deps/net\" }
\"krate:time\" = { path = \"PREFIX/wit/krate/phase3/deps/time\" }
\"krate:locale\" = { path = \"PREFIX/wit/krate/phase3/deps/locale\" }
\"krate:resources\" = { path = \"PREFIX/wit/krate/phase3/deps/resources\" }
\"krate:store\" = { path = \"PREFIX/wit/krate/phase3/deps/store\" }
\"krate:random\" = { path = \"PREFIX/wit/krate/phase3/deps/random\" }
\"krate:ui\" = { path = \"PREFIX/wit/krate/phase3/deps/ui\" }
\"krate:gfx\" = { path = \"PREFIX/wit/krate/phase3/deps/gfx\" }
\"krate:audio\" = { path = \"PREFIX/wit/krate/phase3/deps/audio\" }
\"krate:speech\" = { path = \"PREFIX/wit/krate/phase3/deps/speech\" }

[profile.release]
panic = \"abort\"
lto = true
codegen-units = 1
opt-level = \"s\"
```

A GUI app declares `mod bindings;`, reaches the API through
`bindings::krate::*`, implements `bindings::Guest`, and ends with
`bindings::export!(Component with_types_in bindings);`. Do **not** write the
`bindings` module yourself -- the build generates it from the WIT.

";

const NO_STD_RULES: &str = "\
---

## `no_std`, and the one mistake that breaks everything

A Krate component may import only `krate:*` interfaces. Reaching the OS through
`std` pulls in `wasi:*` imports and the app is rejected at the import check.

The trap is that this happens **without you calling anything**. A reachable
panic makes std's failure path reachable: it formats a message, writes it to
stderr, and exits, which is three separate OS interfaces arriving at once. One
panic site can take a component from zero imports to more than thirty.

### Which to write

- **No dependencies beyond the bindings, and simple logic?** Plain std is fine.
  `apps/krate-bounce` is a shipped std GUI app that imports zero `wasi:*`.
- **Any real dependency (a parser, a decoder, `rand`), or enough logic that a
  stray panic is likely?** Write `#![no_std]`. This is the safer default, and it
  is what the worked examples below do.

### The `no_std` checklist

Miss a step and it fails to build with \"no global memory allocator found\" or
\"`#[panic_handler]` required\":

1. `#![no_std]` at the top of `src/lib.rs`, then `extern crate alloc;`
2. For a GUI app, also `extern crate krate as _krate_runtime;` -- linked only for
   its runtime pieces, never called directly. (A CLI app gets these by `use`ing
   the `krate` crate normally.)
3. **KEEP the `krate` dependency in `Cargo.toml`.** Do not remove it because
   \"the app does not call it\". It is what provides the global allocator, the
   `#[panic_handler]`, and the memory intrinsics a `no_std` guest needs. **This
   is the step that is missed most often, and nothing builds without it.**
4. Keep `std_feature = true` under `[package.metadata.component.bindings]`.
5. Keep `panic = \"abort\"` and `opt-level = \"s\"` in `[profile.release]`.

### What to avoid in `no_std`

- **No `format!` and no `.to_string()`.** They route through the allocator's
  out-of-memory handler. Format numbers by hand into a fixed `[u8; N]` buffer --
  the checklist example below has a `push_num` helper to copy.
- **No `.unwrap()` or `.expect()`.** Use `let Some(x) = ... else { ... }`,
  `if let`, or `.unwrap_or(default)`.
- **No indexing with `[i]`.** `buf[i]` carries a bounds check that can panic. Use
  `.get(i)` / `.get_mut(i)` and handle the `None`.
- **No panicking arithmetic.** Prefer `.saturating_sub()`, `.checked_div()`, and
  friends over bare `-` and `/` on values that could underflow or be zero.

### If the app needs randomness

`rand`, `uuid`, and `getrandom` do not build on a target with no OS entropy
unless a backend is registered. Do not hand-shim it. Add
`features = [\"getrandom-backend\"]` to the `krate` dependency, add a
`.cargo/config.toml` containing
`rustflags = [\"--cfg\", \"getrandom_backend=\\\"custom\\\"\"]`, and declare the
`random.bytes` capability. `apps/krate-diceroll` is the working example.

";

/// Bridges into the generated sections.
///
/// Those three sections are emitted verbatim by the same generator that writes
/// `KRATE_AUTHORING.md`, numbering and cross-references included, so that the
/// published prompt cannot drift from the pack. That numbering is the pack's
/// (its 3 and 5 are prose this document replaces), which reads as a gap here --
/// so say so, rather than editing generated text and forking the two.
const API_PREAMBLE: &str = "\
---

# The API surface

Everything from here to the worked examples is generated directly from the Krate
source: the SDK crate, the capability registry, and the interface definitions.
It is exactly what the compiler will accept. **These are the only functions and
capability names that exist.**

The three sections keep the numbering they have in Krate's own reference file, so
you will see 1, 2, and 4 -- their 3 and 5 are the `no_std` rules and the example
index, which this document has already covered in its own words.

";

/// The worked examples: two complete shipped apps, inlined.
///
/// The pack can say "go read `apps/krate-clock`" because the agent has the repo.
/// A chat model does not, so the source has to travel with the prompt. Both are
/// `include_str!`'d rather than transcribed, so what the prompt teaches is
/// exactly what CI builds.
fn worked_examples_section() -> String {
    let mut out = String::from(
        "\
---

# Worked examples

Both of these are real shipped Krate apps, copied here from the repository
verbatim. They compile, they pass the import check, and they run. Start from
whichever is closer to what you were asked for and adapt it -- do not write the
`no_std` discipline from a blank page.

",
    );

    out.push_str(
        "\
## Example 1 -- a CLI app: `krate-clock`

Prints the time, the timezone, and the locale. This is the smallest complete
shape of a CLI app: `#![no_std]`, `use krate::...`, `impl Guest for Component`,
`krate::export!`. Note that it never calls `println!` -- output goes through
`stdio::stdout()`.

`src/lib.rs`:

```rust
",
    );
    out.push_str(CLOCK_SOURCE.trim_end());
    out.push_str("\n```\n\n`manifest.toml`:\n\n```toml\n");
    out.push_str(CLOCK_MANIFEST.trim_end());
    out.push_str("\n```\n\n");

    out.push_str(
        "\
## Example 2 -- a GUI app: `krate-checklist`

A checklist that saves. This is the shape to copy for almost any windowed app,
and it shows every part in one file: opening a window, building the widget tree,
binding a canvas, drawing the whole interface by hand, hit-testing clicks against
the rectangles it drew, handling typed text, saving to the key-value store, and
handling the `quick` verification run.

Read how it stays panic-free: fixed-size arrays instead of `Vec`, `[u8; N]` text
buffers, `.get()` everywhere instead of `[i]`, and `push_num` to format numbers
without `format!`.

`src/lib.rs`:

```rust
",
    );
    out.push_str(CHECKLIST_SOURCE.trim_end());
    out.push_str("\n```\n\n`manifest.toml`:\n\n```toml\n");
    out.push_str(CHECKLIST_MANIFEST.trim_end());
    out.push_str("\n```\n\n");

    out
}

const HANDOFF: &str = "\
---

# The handoff: you cannot build this, and you must say so

**You have no compiler.** You cannot run the app, you cannot see whether it
compiles, and you cannot check its imports. Do not tell the person their app
works, is tested, or is ready. Say what you wrote and hand it to them to build.

After the three files, end with the build instructions, adapted to their name:

> Save these three files into a folder, then run:
>
>     krate check-app .
>
> That builds the app, checks it imports only Krate APIs, and runs it once. If
> it prints `OK`, the app is real. If it fails, it names the stage and the exact
> fix -- paste that back to me and I will correct the code.
>
> If you do not have Krate yet, get it from https://krate.tech and let
> `krate create \"<what you want>\"` set the folder up for you first, which also
> fills in the paths marked `PREFIX`.

**When they paste back a failure, fix it against this document** -- the failing
stage names what went wrong, and the rules above say why. Common ones:

- *failed at imports, `wasi:*` found* -- a panic path is reachable. Look for
  `format!`, `.unwrap()`, `[i]` indexing, or a missing `krate` dependency.
- *no global memory allocator found* -- the `krate` dependency was removed from
  `Cargo.toml`. Put it back.
- *unresolved import / no function named ...* -- a function was invented. Find
  the real one in the API sections above.
- *failed at run* -- the app probably did not handle the bare `quick` argument,
  or it opened a window and waited forever.

**If the request is something Krate cannot do, say so instead of writing code.**
Krate apps cannot reach another person's device, run in the background, or read
your email or accounts. Producing a plausible-looking app over invented local
data wastes the person's build and is worse than a straight answer.
";

#[cfg(test)]
mod tests {
    use super::*;

    /// Both authoring paths must teach the same `quick` contract.
    ///
    /// They did not, and it cost a benchmark run. The authoring pack that
    /// `krate create` uses was fixed to demand one `key:value` per line; this
    /// prompt -- what a person pastes into a chat model -- still said only
    /// "print something", which is the exact contract under which five apps
    /// scored zero on 2026-08-05 (K-102). Same product, two answers.
    ///
    /// The assertion is on the substance rather than the phrasing: the prompt
    /// has to name the format, and must not tell anyone that printing
    /// anything at all is enough.
    #[test]
    fn the_quick_contract_matches_what_the_authoring_pack_teaches() {
        let prompt = generate();
        assert!(
            prompt.contains("key:value"),
            "the prompt must name the key:value format, not just say to print"
        );
        assert!(
            prompt.contains("items:5"),
            "the prompt must show a worked example of the output"
        );
        assert!(
            !prompt.contains("print something, and exit 0"),
            "this is the 2026-08-05 wording that scored 0/5 -- see K-102"
        );

        // And the pack the `create` path uses must still agree, so a future
        // edit to either one cannot silently split them again.
        let pack = crate::authoring_context::generate(std::path::Path::new("."));
        assert!(
            pack.contains("key:value"),
            "the authoring pack must teach the same contract"
        );
    }

    /// The prompt must carry every part a model needs: the framing, the output
    /// contract, both Cargo templates, the generated API, and the examples.
    #[test]
    fn the_prompt_carries_every_section() {
        let prompt = generate();
        assert!(prompt.contains("# Krate Mode"), "header");
        assert!(prompt.contains("Output rules"), "output rules");
        assert!(prompt.contains("The two Cargo.toml templates"), "templates");
        assert!(prompt.contains("`no_std`, and the one mistake"), "no_std");
        assert!(prompt.contains("# 1. The SDK"), "generated SDK surface");
        assert!(prompt.contains("# 2. Capabilities"), "generated catalog");
        assert!(prompt.contains("# 4. The GUI world"), "generated GUI WIT");
        assert!(prompt.contains("# Worked examples"), "examples");
        assert!(prompt.contains("The handoff"), "handoff");
    }

    /// The load-bearing facts, each from its real source. If the SDK, the
    /// capability registry, or the WIT moves, these are what catch it.
    #[test]
    fn the_prompt_states_the_facts_that_decide_whether_it_compiles() {
        let prompt = generate();
        // Generated from the capability registry.
        assert!(
            prompt.contains("random.bytes"),
            "capability catalog is real"
        );
        // Generated from the gfx WIT.
        assert!(prompt.contains("canvas2d::present"), "gfx WIT is real");
        // The mistake the brief singles out as most common.
        assert!(
            prompt.contains("KEEP the `krate` dependency"),
            "the dependency rule must be stated"
        );
        // The verification argument that silently fails otherwise.
        assert!(prompt.contains("quick"), "the quick argument");
        // The honesty requirement.
        assert!(
            prompt.contains("You have no compiler"),
            "must admit it cannot build"
        );
    }

    /// The examples must be the real shipped apps, not prose about them. If
    /// `krate-checklist` is rewritten, this prompt carries the rewrite.
    #[test]
    fn the_worked_examples_are_real_shipped_source() {
        let prompt = generate();
        // A distinctive line from each app's actual source.
        assert!(
            prompt.contains("bindings::export!(Component with_types_in bindings);"),
            "the checklist's real export line"
        );
        assert!(
            prompt.contains("krate::export!(Component);"),
            "the clock's real export line"
        );
        assert!(
            prompt.contains("fn push_num"),
            "the no_std number formatting helper the prompt points at"
        );
    }

    /// The published prompt must match what the generator produces.
    ///
    /// This is the anti-drift test. `docs/krate-mode.md` is what people paste,
    /// and it is a checked-in copy of generated output -- so the moment the SDK,
    /// the capability registry, the WIT, or either example app changes, this
    /// fails until the file is regenerated with `krate krate-mode`.
    #[test]
    fn the_published_prompt_matches_the_generator() {
        let published =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/krate-mode.md");
        let on_disk = std::fs::read_to_string(&published)
            .expect("docs/krate-mode.md exists; regenerate it with `krate krate-mode`");
        assert_eq!(
            on_disk.trim_end(),
            generate().trim_end(),
            "docs/krate-mode.md is stale -- regenerate it with \
             `krate krate-mode > docs/krate-mode.md`. A stale prompt teaches \
             models an API that no longer exists."
        );
    }
}
