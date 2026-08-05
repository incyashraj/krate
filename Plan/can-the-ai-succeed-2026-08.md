# Can the AI succeed? A gap audit of what we hand it

The framing that matters: **if an AI writes an app that does not work, and the
material we gave it could not have produced a working one, that is our bug.**
The AI is doing what we told it. The question is not "did the model do well" but
"could any model have done well with what we provided".

Measured against the first real MCP session, the answer is no for four of the
five failures. Every one of them traces to something we did not give it.

---

## The three kinds of gap

Sorting the failures this way makes the work obvious.

### A. The runtime cannot do it (a real hole)

| Gap | Evidence |
|---|---|
| **No wheel or scroll event** | `wit/krate/phase3/deps/ui/ui.wit` defines exactly ten event variants: `close-requested`, `resized`, `redraw-requested`, `pointer`, `key`, `text-input`, `text-changed`, `action`, `focus-changed`, `theme-changed`. There is no wheel, no scroll delta, no trackpad gesture. `scroll` appears once, as a widget *kind*, not an event. |

**No canvas app can scroll.** Not because the AI failed to write it, but because
there is no event to write against. A person adds a seventh item to a list that
shows six and it vanishes. This is the clearest example of the principle: no
prompt, no example, and no amount of model capability fixes it. Only we can.

**Fix:** add a `wheel` event to the WIT with a scroll delta, plumb it through
each adapter, then use it in the reference apps.

### B. The runtime can do it, but we never told the AI (a teaching hole)

| Gap | Evidence |
|---|---|
| **`canvas_size` is never taught** | The host implements it (`phase3_gui_host.rs:1463`). The authoring pack mentions it **zero** times. |
| **The `resized` event is never taught** | The WIT emits it. The pack mentions resize **zero** times. |
| **The event loop is never taught** | The pack has five sections: SDK surface, capabilities, no_std, GUI WIT, example index. It lists `events::wait` as a signature and never shows a loop. `events::wait|Event::Pointer|event loop` appears **zero** times outside the raw signature list. |
| **`entry = "code.wasm"` is never taught** | `krate pack` requires it. Neither the pack nor Krate Mode mentions `code.wasm` at all. The model in the session hit the error and worked around it with an unexplained `sed`. |

These are worse than the runtime hole in one way: the capability is *right
there*, and we hid it. The AI wrote fixed-coordinate hit-testing because every
example it was shown does fixed-coordinate hit-testing and nothing told it a
window could change size.

**Fix:** the pack needs a section it does not have -- how to build an
interactive window. Not signatures; the actual shape. Read canvas_size, lay out
from it, handle resized, hit-test against what you drew, redraw on change.

### C. Our examples teach the bug (a contamination hole)

| Gap | Evidence |
|---|---|
| **Auto-close after 9.9s** | Was in six shipped apps. The AI copied it faithfully. Fixed. |
| **Fixed-constant layout** | Every canvas example lays out from `const WIDTH`/`HEIGHT`. Only 3 of 34 apps call `canvas_size`. |
| **No scrolling** | No example scrolls, so no generated app scrolls. Downstream of gap A. |

This is the highest-leverage category, because `krate_examples` hands the model
complete app source and tells it these are "proven patterns to adapt". Whatever
is wrong in an example is wrong in every app generated from it, forever.

**Fix:** treat the reference apps as the product they are. A bug in
`krate-checklist` is a bug in every app anyone will ever generate from it.

---

## What this means for "any possible app"

The goal is that a person can ask for any reasonable app and get one that works.
Today the honest boundary is narrower, and it is bounded by us, not by the model:

- **Cannot scroll** -- no event exists. Any app whose content exceeds one screen
  is broken.
- **Breaks when resized** -- no example reads `canvas_size`, and the pack never
  mentions the `resized` event.
- **Cannot be packed by following our docs** -- the entry rule is undocumented.

None of these are model problems. All three are fixed by us.

## The test that would have caught this

Every gate we have asks whether an app is *valid*: does it build, does it import
only `krate:*`, does it run, does it paint a frame. Nothing asks whether it is
*usable*: does a click land where the pixel is, does the list scroll, does the
window survive being resized, does it stay open.

`check-app` needs a usability stage. Concretely, for a canvas app:

1. Resize the window and confirm the app re-laid out (canvas_size changed and
   the frame differs).
2. Send a pointer event at a drawn control and confirm the app's state changed.
3. Confirm the app is still open after the idle timeout.

That is harder than the existing stages and it is the only kind that would have
caught what the person actually reported.

---

## Order

1. **The teaching hole (B)**, because it is cheap and unblocks resize handling
   immediately. A new pack section on the interactive window loop, plus the
   `code.wasm` rule.
2. **The contamination hole (C)**, because every generated app inherits it.
   Rebuild the canvas examples to lay out from `canvas_size` and handle
   `resized`.
3. **The runtime hole (A)**, the wheel event. Largest change, touches the WIT
   and every adapter, and nothing can scroll until it lands.
4. **The usability stage in check-app**, so the next one of these is caught by
   us rather than reported by a person.
