# Brief: build a Krate canvas app with a genuinely modern UI

You are building (or rebuilding) a Krate app whose whole UI is drawn on a
`canvas2d`. The bar is a real consumer app -- think Airbnb, WhatsApp, the iOS
Weather app, a good Mac utility. Not "nice for a demo": actually modern.

Reference apps in this repo (read them first):
- `apps/krate-weather` -- the iOS-weather look: big readout, forecast strip.
- `apps/krate-savings` -- finance app: field, button, segmented bar, legend.
- `apps/krate-keyvault` -- the minimal one-big-number treatment.
- `apps/krate-nova` -- game polish: glows, particles.

## Text is REAL TYPE now (important)

Canvas `draw-text` renders antialiased system fonts (parley + vello_cpu) at the
exact `font-size` you pass. There is no pixel font anymore. Design with a real
type scale and it will render like a native app:

- Large title: 32-40px. Section/card title: 20-24px. Body: 15-17px.
- Caption/label: 12-13px. Big focal numbers: 56-96px.
- The face is the system sans (SF on macOS). Ascent/descent are normal; the
  draw-text origin is the BASELINE. A comfortable line height is ~1.4x size.
- Do NOT hand-calibrate glyph widths like older apps did; measure roughly as
  ~0.5-0.55em per char for layout, and prefer generous padding over tight fits.

## The design system (use these unless the app clearly needs its own)

- Ground: near-black blue, vertical gradient #0B0E15 -> #10141D (linear-gradient).
- Panel/card: #161B26 at radius 14-16, hairline border #232A38 when needed.
- Text: primary #F2F5FA, secondary #9AA5B5, quiet #5D6878.
- Accent: #4C8DFF (buttons, selection, progress). Success #3DD68C,
  danger #FF5D6E, warning #FFC24B. One accent per screen; semantic colors only
  for meaning.
- Spacing on an 8px grid; screen edge padding 24px; card padding 16-20px.
- Buttons: filled rounded rect (radius 10-12), label centered, 44px min height;
  ghost buttons are a hairline border + secondary text; danger is the red.
- Rounded corners: draw with fill_rect + four fill_circle corners, or compose
  non-overlapping pieces at full alpha (overlapping translucent pieces double
  up and look blotchy).
- Depth: a soft shadow = a slightly larger dark rounded rect underneath, or a
  radial_gradient pool. Use sparingly.

## The canvas toolkit (via the app's generated `bindings`)

- `bindings::krate::gfx::canvas2d`: bind(window, widget) once, then clear,
  fill-rect, stroke-rect, fill-circle, radial-gradient, linear-gradient,
  draw-text(canvas, text, origin, font-size, color), draw-pixels, draw-sprite,
  present(canvas) each frame. Colors are gfx::types::Color {r,g,b,a} 0..1.
- Input: `bindings::krate::ui::events::wait(Some(ms))` / poll. Pointer events
  carry x, y, pressed -- hit-test against the rects you drew. TextInput/Key
  events for typing. key_held for held keys.
- Shared modules for a GUI app go through bindings too:
  `bindings::krate::store::kv::get/set`, `bindings::krate::random::bytes::get`,
  `bindings::krate::time::clock::now_millis`, audio at
  `bindings::krate::audio::playback::*`.
- Honor the `quick` first argument: seed presentable demo state, draw one good
  frame, print a status line, exit 0.

## no_std discipline (non-negotiable)

`#![no_std]` + `extern crate alloc`; keep the `krate` dependency (it owns the
allocator/panic handler); `std_feature = true` stays in Cargo.toml. No
`format!`, no `.unwrap()`, no `a[i]` indexing on hostile paths -- build strings
with a pure_string-style helper (copy from a reference app). The app must
import only `krate:*` (check-app's imports stage enforces zero wasi).

## The loop you must run

1. Write the app: Cargo.toml + manifest.toml + src/lib.rs (copy the wiring of
   a reference app; adjust names and capabilities to what the app uses).
2. `target/release/krate check-app <app-dir>` until it prints OK.
3. `target/release/krate check-app <app-dir> --shoot /tmp/<app>.png`, Read the
   PNG, and judge it honestly against the consumer-app bar.
4. Iterate. At minimum 3 rounds of shoot-and-improve. Interrogate your own
   shot: is the hierarchy clear at a glance? does anything collide or touch an
   edge? is the spacing even? would this screenshot look at home on the App
   Store? Fix what fails, reshoot.

The `quick` frame IS the store screenshot -- seed state that shows the app at
its best (a few realistic items, a mid-session number, a good demo scene).

Report: final check-app verdict + an honest assessment of the final shot.
