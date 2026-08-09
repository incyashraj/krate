# Hosting modern apps: the road to an Instagram-class UI

Written 2026-08-07. The complaint this answers, verbatim: "if you see our app
UI, you'll see it's pretty basic, like from few years back -- nowadays apps
have better UI with animations and colours and blur." The acceptance test the
plan is built around: **could an AI build an Instagram-looking app on Krate --
same feed, same feel -- and could Krate host it well?**

## What the runtime can actually draw today (measured, not remembered)

From `wit/krate/phase3/deps/gfx/gfx.wit` and the adapter painter:

Have: fill/stroke rect, fill/stroke circle, one vertical `linear-gradient`,
one `radial-gradient`, `draw-text` (one face, one weight -- but the host
already ships parley + harfrust + skrifa, so real shaping exists), clip
rects, raw pixels, rotated sprite, software 3D with textures. All CPU
(vello_cpu + softbuffer), ~400M px/s, `--shoot` proof harness, flat ~80 MB.

Do not have: **rounded rectangles** (the single loudest "old UI" tell),
paths/beziers, per-corner radii, transform stacks, drop shadows, any blur,
multi-stop or angled gradients, font weights/families in the API, emoji,
image-with-corner-mask, an animation clock contract, momentum scrolling,
video, streaming connections.

Decompose an Instagram screen and the mapping is exact: avatar circles with
gradient rings (have circles, need ring gradients), cards with rounded
corners and soft shadows (need both), the double-tap heart that scales and
fades (need transforms + clock), sheet modals over a blurred feed (need
blur), smooth flick scrolling (need momentum), photos over HTTPS (have
HTTPS + zune decode, need a corner mask), bold/regular text mixed (need
weights), stories bar sliding (need transforms).

So "modern" is not one big thing. It is roughly ten primitives, and eight of
them are cheap on the renderer we already have.

## Phase 1 -- the look, on the CPU painter we have (1-2 weeks)

New `canvas2d` functions (grow the interface; K-064's message and the
rebuild path already handle old apps): `fill-round-rect` /
`stroke-round-rect` with per-corner radii; `fill-path` / `stroke-path`
taking a flat verb+coords list (move/line/quad/cubic/close); `push-transform`
(translate/rotate/scale) / `pop-transform`; `linear-gradient-stops` (angle +
stop list); `draw-image-round` (pixels + corner radii, the photo-card call);
`drop-shadow-rect` (rect, radius, blur, color -- implemented as a three-pass
box blur, which at card-shadow sizes is fine on CPU); font `weight` on
draw-text/measure-text, wired to the parley stack the host already carries.

Every one lands with: painter implementation, `--shoot` pixel test, a line in
the authoring pack, and use in one example app -- the pack rule from
K-010/K-052: an untaught primitive is a teaching-hole that AIs will never use.

## Phase 2 -- the feel: animation and scroll (1 week)

Motion needs a clock and math, not new drawing. The event loop already
redraws on demand; add a `frame` event carrying a timestamp delta (the
contract games already fake with their own loops, made official), and put
easing/springs/momentum in the **SDK as pure guest code** -- no WIT, no host
work: `ease(t, curve)`, `spring(current, target, velocity)`, and a
`ScrollView` helper owning offset + velocity + rubber-banding, drawn with
Phase 1's clip + transform. The double-tap heart is then twelve lines of
app code.

## Phase 3 -- blur, and the honest GPU question (2-3 weeks, decision inside)

Backdrop blur -- frosted sheets over a live feed -- is the one thing a CPU
rasterizer cannot fake at 60fps full-screen. Two-step:

1. **Static backdrop blur now**: blur the covered region once when a sheet
   opens (three box-blur passes ≈ gaussian). The content under a modal is
   usually still; this covers most real designs on the CPU path.
2. **A wgpu backend behind the same painter interface** for live blur, big
   shadows, and full-screen 60fps. The interface does not change -- apps
   cannot tell -- so this is an adapter swap, feature-gated, with the CPU
   path kept forever as the portability fallback (the pitch is "runs
   everywhere", and a GPU-only runtime breaks it on VMs and odd drivers).
   Decide go/no-go on Phase 0 numbers, not vibes: if a mid-range Windows
   laptop scrolls the reference feed at 60fps on CPU, GPU waits.

## Phase 4 -- media and live data (parallel, 1-2 weeks)

Images: an SDK `fetch_image(url)` helper -- HTTPS (have) + zune decode
(have) + cache -- so the AI writes one line, not a decoder. Video stays
explicitly out until there is a frame clock proven under load (it is on the
public "does not work yet" list, keep it honest). Live feeds/multiplayer
need `net.stream` (WebSocket) as a new capability with its own permission
line -- the wall must say "keep an open connection to chat.example.com" in
plain words.

## Phase 0 -- before any of it: the measurement and the target app (2-3 days)

Build **krate-gram** by hand, today, with only current primitives: feed of
photo cards (square corners, no shadows -- it will look dated, that is the
point), stories row, like animation faked with redraws. Measure scroll fps
and frame times on this Mac + the Windows VM. That gives (a) the honest
baseline, (b) the exact worst gaps ranked by visual damage, (c) the app that
becomes the acceptance test: **after Phases 1-2 it must be visually mistakable
for a modern app in a screenshot, and after Phase 3 in motion.** Then it
ships as an example, because example apps are what the AI learns from --
highest leverage per line (the example-bug lesson, inverted).

## Status 2026-08-09: the acceptance test exists and passes its screenshot

`apps/krate-gram` is built: stories with gradient rings, rounded shadowed
photo cards (generative art, no network), momentum scroll with rubber-band
via the wheel event, double-tap heart on a spring, tab bar -- one privileged
capability (a window), all six check-app stages green. The Phase 0 number
on this Mac: ~6.5 ms of CPU per full feed frame headless (90 frames, 0.59 s
user), so ~150 fps of raster headroom. Per the go/no-go rule the GPU
backend waits. Still open from the plan: the official `frame` event (apps
fake it with wait(16)), `fetch_image`, static backdrop blur, `net.stream`.

## What this does not try to be

Not a browser: no CSS, no DOM -- the API stays immediate-mode drawing that
an AI writes directly, which is the thing that already works. Not a video
platform. Not GPU-required. And nothing on krate.tech claims any of it until
krate-gram passes `--shoot` diffs and a stranger scrolls it without being
told what to feel.
