# What cannot be built, and why that is on us

The goal is not "fewer bugs in the examples". It is: **a person asks for a
reasonable app and gets one that works.** Anything short of that is our failure,
not the model's and not the user's.

So the question to answer is not "did this app have bugs" but **"what class of
app is impossible today, and what is missing that makes it impossible"**.

This is an audit of the runtime surface against what ordinary apps need. Every
line is checked against the WIT and the shipped apps, not assumed.

---

## What exists

Twelve interfaces: `audio fs gfx io locale net random resources speech store
time ui`. Seventeen widget kinds. Ten event variants. HTTPS. SQL. A keychain.
Audio in and out. Speech. 3D. That is a serious surface, and most of what is
missing is not another interface -- it is holes inside the ones we have.

## Hole 1: no text measurement -- the worst one

`gfx.wit` has `draw-text`. It has **no way to ask how wide that text will be**.
Zero matches for `measure|text-width|text-extent`.

So every app fabricates it. Seven shipped apps carry this:

    /// Approximate rendered width of a string (~0.52em avg advance).
    fn text_width(s: &str, size: f32) -> f32 {
        (s.chars().count() as f32) * size * 0.52
    }

A made-up constant, applied to a proportional font, where `i` and `W` differ by
4x. This is why generated apps have text that collides, captions that overflow
their cards, labels that are not really centred, and carets that sit in the
wrong place.

**The runtime already knows the answer.** It rasterizes with real font metrics
through parley. We compute the truth and then refuse to tell the app.

Nothing an AI can do fixes this. No prompt makes a guess correct.

**Fix:** `canvas2d::measure-text(text, font-size) -> size`. Then delete
`text_width` from seven apps and from every app ever generated from them.

## Hole 2: no scroll event

Ten event variants, none of them a wheel. Any app whose content exceeds one
window is broken, permanently, regardless of how well it is written. Already
assigned (W12).

## Hole 3: no clipping

One mention of "clip" in the whole of `gfx.wit`. Without a clip rectangle, a
scrolling list draws its rows over the header and past the bottom edge, and the
app cannot stop it. Scrolling is not really usable until this exists, so it
travels with hole 2.

## Hole 4: no frame timing

`redraw-requested` and `request-redraw` exist, but nothing gives an app a frame
callback or a vsync signal. Animation is done by polling `events::wait(Some(16))`
and hoping. That works for a slow dashboard; it is wrong for anything that
moves, and it burns CPU on things that do not.

**Fix:** a frame/tick event carrying elapsed time, so animation is time-based
rather than frame-count based.

## What this pattern says

Every one of these is the same shape: **the runtime knows something and does not
expose it.** It has the font metrics and hides them. It receives scroll events
from the OS and drops them. It knows when it composited a frame and never says.

That is the actual barrier to "any possible app". Not missing features in some
grand sense -- missing *answers to questions an app has to ask*.

## The benchmark we should be held to

A claim like "you can build any app" is worth nothing without a public,
reproducible measure. So define one and publish it, including the failures.

**The Krate App Benchmark:** a fixed, public corpus of app requests spanning
real categories -- a list tool, a form, a chart, a game, a text editor, a media
viewer, a timer, a calculator, a data browser -- each with a stated pass bar:

- it builds and imports only `krate:*`
- it does what was asked, judged by a human once, then locked as a fixture
- it survives a resize
- it responds to a click
- it stays open

Publish the score. Publish which requests fail and exactly what is missing. A
number that only moves up when the product genuinely improves is worth more to
this market than any claim, and being first to publish an honest one is a
position nobody else currently holds.

## Order

1. **Text measurement.** Smallest change, largest blast radius, fixes a defect
   in every canvas app that exists or will exist.
2. **Scroll and clipping together.** Neither is much use alone.
3. **Frame timing.** Unblocks everything that moves.
4. **The benchmark.** Turns "can any app be built" from an opinion into a number
   we are accountable to.
