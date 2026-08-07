# krate.tech, rebuilt to the standard of x.ai/build

Written 2026-08-08 after measuring both sites -- x.ai's live DOM through a
browser (Cloudflare blocks curl), ours from `docs/landing/`. Every number in
here was read from a computed style, not eyeballed from a screenshot.

The brief, in Yashraj's words: simple yet perfect, like x.ai/build -- but
"don't copy everything completely, see where what is suitable." This plan
says exactly what we take, what we adapt, and what we refuse.

---

## What x.ai/build actually is, measured

**Tokens** (from `getComputedStyle` on the live page):

| Thing | Value |
|---|---|
| Background | `#0a0a0a` -- neutral near-black, no blue cast |
| Text | `#fff`; muted is `rgba(255,255,255,0.5)` -- one alpha, not a gray ramp |
| Borders | `rgb(31,34,40)` -- barely-there, 1px everywhere |
| Font | `universalSans` / `universalSansDisplay` (licensed, theirs) |
| Hero h1 | 60px, weight 500, letter-spacing **-1.5px**, line-height 1.0 |
| Section h2 | 30px, weight 400 |
| Big section header | 36px; closing CTA 48px |
| Body / muted | 16px |
| Buttons | pill, `border-radius: 9999px`; primary = white bg, near-black text |

**The scroll-pinned terminal** (their signature move, and the thing Yashraj
named): a two-column grid `grid-cols-[1fr_1.3fr]`, left column is five
feature sections (Skills, Plan, Plugins, Q&A, Subagents), right column is
**one** element, 652x440, `position: sticky; top: calc(50vh - 220px)` -- so
it sits vertically centered while the text scrolls past, and its *content*
swaps to match the section in view. The terminal is not an image: it is HTML
text styled as a terminal, with traffic lights, a path label, a context-used
meter top-right, and a status line bottom-right (`grok-4.5 · always-approve`).

**The gradient underline** (hero, rotating word): a real element, not
text-decoration --

```
position: absolute; bottom: -4px; left: 0; right: 0;
height: 3px; border-radius: 9999px;
background: linear-gradient(90deg,
  rgba(255,255,255,.25) 0% 35%,
  #6366f1 40%, #a855f7 45%, #ec4899 50%, #f97316 55%, #eab308 60%,
  rgba(255,255,255,.25) 65% 100%);
```

-- with the colored band swept across by animating `background-position`.
The word above it rotates (code → imagine → analyze...) by animating each
letter separately: `opacity`, `blur(4px)`, `translateY(14px)`, inside a
`clip-path: inset(-4px 0)` container so letters slide in from below.

**Header**: sparse links at 50% opacity that go full on hover, dropdown
panels on hover, pill CTA on the right. **Footer**: a full sitemap grid.

## What krate.tech is today

Thirteen stacked sections in `docs/landing/index.html` (hero, numbers, how,
demo, capabilities, start, real-parts, gallery, tested, platforms, cloud,
about, final CTA), a flat link nav, and a blue-tinted dark theme
(`--bg: #0b0d12`, `--blue: #6291ff`, Inter + system mono). All static HTML +
one CSS file on GitHub Pages: no build step, no framework. Honest content,
2019 presentation: every section has the same weight, nothing moves, nothing
is pinned, and the screenshots are images rather than living terminals.

That last point is the gap that matters. **Our product's most demoable
surface is a terminal, and our website shows pictures of it.** x.ai renders
the terminal as HTML and it feels alive. We have real TUI transcripts -- the
cooking stages, the permission wall, the publish URL -- that can be that.

---

## The design system we adopt

### Tokens (`krate.css` `:root`, replacing the current set)

```css
--bg: #0a0a0c;              /* neutral, drop the blue cast */
--panel: #101014;
--line: #1f2228;            /* their border, it is right */
--text: #ffffff;
--muted: rgba(255,255,255,.55);
--quiet: rgba(255,255,255,.35);
--accent: #6291ff;          /* OURS. Krate is blue; Grok's orange stays theirs */
--radius-card: 14px;
--radius-pill: 999px;
--max: 1100px;
```

### Type

Font stays **Inter** (we already ship it; `universalSans` is licensed and
imitating it with a knockoff would look like what it is). What we take is
their *treatment*, which is most of the look:

- Display: clamp(40px, 7vw, 64px), weight 500, letter-spacing **-0.02em**,
  line-height 1.0
- Section h2: 30px / weight 400 -- lighter than ours today, calmer
- One muted color at ~55% alpha instead of our three-step gray ramp
- Mono (terminals, commands): our existing mono stack, 13-14px

### Components

- **Pill buttons**: primary white-on-black, secondary 1px `--line` outline,
  both `border-radius: 999px`. The split "Try for free ˅" pattern becomes
  our `Install ˅` (dropdown: macOS/Linux command, Windows command, GitHub
  releases).
- **Command box**: `$ curl -fsSL https://krate.tech/install.sh | sh` in a
  bordered pill with a copy button -- this replaces our current hero
  buttons as the primary action, exactly as on x.ai/build. A small
  macOS/Linux | Windows toggle above it (docs.x.ai does this; we genuinely
  need it, they only kind of do).
- **The terminal component**: one reusable HTML/CSS block -- traffic
  lights, title `krate`, right meta slot, dark body, mono text, status
  bar. Used in the hero, the pinned scroller, and inside nav panels.

---

## The page, section by section

### 1. Hero

```
        Make an app you can  send.
                            ~~~~~~ <- gradient bar, word rotates:
                                      send / run / trust / keep
   An AI writes it. Krate makes it real -- one file,
   runs on every desktop, can't touch what you didn't allow.

     [ $ curl -fsSL https://krate.tech/install.sh | sh   ⧉ ]
              Free and open source · v0.1.2

        Read docs ›   Progress ›   Open source ›
```

The rotating word + gradient bar is the one piece of theatre we take
whole: per-letter blur/translate rotation, the measured gradient with our
blue family instead of their rainbow center (`#6291ff → #6cf4d7 → #a9c2ff`
band on the same 25%-white base). Below the fold line, the hero terminal
plays a looped, CSS-only typing of the real first-run: `krate` → the ask →
a request being typed.

### 2. The pinned-terminal feature scroller (the centerpiece)

Our five sections, same mechanics as theirs, all content real:

| Left text | Right terminal shows (real transcripts) |
|---|---|
| **Describe it** -- the prompt-first TUI | the ask, a typed request, cooking stages ticking |
| **The permission wall** -- our actual differentiator | the real grant prompt: what the app can reach, allow/deny |
| **One file, any desktop** -- send it | `open habit.krate` on mac/win/linux lines, sizes, "no installer" |
| **Prove it yourself** -- check-app + --shoot | six-stage verdict output, `frame.png written` |
| **Publish** -- a URL anyone can run | `krate publish` → `https://krate.tech/a/...` + run-by-URL |

Implementation, no framework: grid `1fr 1.3fr`; right column child
`position: sticky; top: calc(50vh - 220px); height: 440px`. Each left
section `min-height: 85vh`. One IntersectionObserver marks the active
section; the five terminal contents are stacked in the sticky panel and
crossfade 200ms via `[data-active]`. With JS off, the panel simply stops
swapping and shows the first demo -- content is never lost. Mobile
(<1024px): the grid stacks and each section carries its own terminal
inline, exactly like their responsive fallback.

The permission wall gets the second slot deliberately: it is the section
Grok has no equivalent of. Their page sells convenience; this row is where
ours sells trust.

### 3. "Everything you need" grid

Their 3x6 icon grid pattern, our facts: sandboxed by default ·
plain-English permissions · six platform builds · one small file ·
`--shoot` pixel proof · check-app verdicts · run by URL · publish to Cloud
· works with any AI CLI · MCP for Claude/Cursor · no runtime to install ·
open source. Icon + name + one-liner, borders `--line`, no cards-in-cards.

### 4. Mid-page install banner

Their "Try it in your terminal" band, verbatim pattern: one sentence, the
command box again. People who scrolled past the hero get a second door.

### 5. Closing CTA + footer

48px "Make something. Send it to someone." over the command box, then a
sitemap footer (Product / Develop / Cloud / Legal-ish columns mapping our
existing pages: Start, Gallery, Platforms, Docs, Reports, Progress, FAQ,
Publish, Cloud, GitHub, Contact). Our glyph bottom-left with
"© 2026 Krate Labs" and a small "this site is static HTML, view source"
wink -- on brand for us in a way it could never be for them.

### 6. Header

Left: glyph + KRATE. Center: **Product**, **Develop**, **Cloud**, each a
hover panel (hover + `:focus-within` on desktop, click on touch):

- *Product* → Start / Gallery / Platforms / FAQ, beside a mini terminal
  looping the cooking stages (the "live component in the menu" Yashraj
  asked for -- a 200x120 instance of the same terminal component with a
  CSS keyframe loop, no video).
- *Develop* → Docs (book) / Reports / Progress / GitHub, beside a mini
  check-app verdict.
- *Cloud* → Gallery / Publish / How run-by-URL works, beside a mini
  publish transcript.

Right: `Install ˅` split pill. Current flat links exist today at
`index.html:311-321`; they become these three groups plus the pill.
Mobile keeps the existing hamburger, panels become accordion rows.

### The secondary pages

faq / progress / reports / cloud / publish / contact adopt the same shell
(tokens, header, footer) in the same pass -- restyled, not rewritten. Their
content is fine; it is the frame that is behind.

---

## What we deliberately do not copy

- **Their fonts** -- licensed. Inter with their sizing discipline gets 90%
  of the look legally.
- **Their orange** and the word "Grok-anything". Accent stays Krate blue.
- **Their copy voice.** Ours is plainer and it is working; the redesign is
  visual.
- **Their weight.** x.ai/build is a Next.js app shipping megabytes to show
  static content. We stay hand-written HTML/CSS + ~150 lines of vanilla JS
  (observer, nav, copy buttons, word rotation). Hard budgets: **< 150 KB
  transferred, zero layout shift, Lighthouse ≥ 95**, everything readable
  with JS disabled, `prefers-reduced-motion` kills the rotation and
  crossfades. Being *faster* than the site we are inspired by is the point
  -- fast is the product's whole pitch.
- **Sections we have no truth for** (pricing, enterprise, marketplaces).
  Nothing goes on the page that `krate` cannot do today; that rule
  survives from the current site (see [[krate-publish-marketable-not-humble]]
  -- marketable framing, but true claims only).

---

## Order of work

1. **Tokens + type + components** -- new `:root`, heading scale, pill
   buttons, command box, the terminal component. Every page picks this up
   through `krate.css` at once.
2. **Hero** -- rotating word + gradient bar + command box + hero terminal.
3. **The pinned scroller** with the five real transcripts. The riskiest
   piece (sticky + observer across browsers), so it gets built as a
   standalone page first and merged when it is right.
4. **Grid, banner, CTA, footer; secondary pages restyled.**
5. **Header hover panels** with the mini live terminals.
6. **The pass that makes it professional**: mobile, reduced-motion,
   keyboard focus states, Lighthouse, and a real check on Windows
   (fonts render heavier there; -0.02em tracking can clot at 14px).

Ship after 4; 5-6 land behind it. Each step is a normal commit to
`docs/landing/`, previewed locally before push since Pages deploys on push.
