# Onboarding: what actually stops people, and what to do

**Date:** 2026-07-31
**Method:** walked both paths on this machine with the tools stripped out of
`PATH`, timing each step and reading every message a person sees. Numbers here
are measured, not estimated.

---

## 1. There are two paths, and only one of them is broken

### Path A — someone sent me an app

```
1. curl -fsSL https://krate.tech/install.sh | sh     ~10 s
2. krate run groceries.krate                         prompts, you approve, it opens
```

**Two steps, under a minute, no account, no configuration.** This path is
genuinely good and needs no redesign.

One thing worth naming, because it looked like a bug and is not: running with
stdin piped or redirected produces a refusal rather than a prompt. That is
correct — a script must not be able to answer a permission question — and a real
terminal prompts properly. A double-clicked bundle goes through the native
consent window.

### Path B — I want to make an app

```
1. install krate                    ~10 s
2. krate create "a checklist app"   STOPS. needs Rust and cargo-component
3. install Rust via rustup          ~2-4 min
4. install cargo-component          ~3-6 min, compiled from source
5. krate create again               ~4 s
```

**Five steps and six to eleven minutes**, and step 2 is a wall met by surprise.

---

## 2. Why the wall is worse than its duration

Six minutes of waiting is survivable. What makes this expensive is *where* the
six minutes land.

**It arrives after commitment, not before.** The person has already installed
Krate and typed a request. They are past the point of browsing and into the part
where they expect a result. Being told at that moment that the real setup starts
now reframes everything before it as a false start.

**It is a second decision, and it is bigger than the first.** "Install Krate"
is a small yes: one tool, ten seconds, easily undone. "Install a language
toolchain" is a different size of ask. Someone who agreed to the first has not
implicitly agreed to the second, and being escalated feels like a bait.

**Nothing has worked yet.** At the wall the person has seen Krate produce
exactly one thing: an error. There is no memory of success to spend against the
frustration. Order matters enormously here — the same six minutes after a first
win reads as "worth it", and before any win reads as "this is going to be a
project".

**The message describes the tools, not the outcome.** It named Rust,
cargo-component, and a wasm target. To someone who does not write Rust, that
list is not information; it is evidence they are in the wrong place.

---

## 3. Three defects found while measuring, all now fixed

Not theory — these were live.

1. **The install command Krate printed could not be pasted.** The rustup
   bootstrap is a script that must be piped into a shell. It was stored as an
   argv array and printed with a plain join, so it came out as
   `curl … https://sh.rustup.rs` with no `| sh`. Copying it printed the
   installer to the terminal, and the next `krate create` failed identically.
   The person could do everything right and get nowhere. Fixed, with a test.

2. **The installer ended on what you cannot do.** It closed by listing the
   build tools `krate create` needs. To someone who has just installed and has
   nothing to open, that reads as homework. It now ends with one command that
   opens a real app.

3. **A stale bundle would have been the first thing anyone ran.** The demo app
   was being taken from a working copy in the tree rather than the published
   release. That copy was old, and old bundles fail to instantiate against the
   current runtime — so the first command a new person ran would have crashed
   with a linker error. Now always fetched from the release at deploy time.

---

## 4. The order to put things in

The single highest-value change is not removing the six minutes. It is making
sure **something works before anything is asked of you.**

**Open before make.** The installer now ends with `krate run
https://krate.tech/notes.krate`. In about fifteen seconds a person has seen a
real app open, seen the permission window, and understood the whole product.
Every later cost is paid from a position of "I have seen this work".

**Say what the wait buys, in the outcome's terms.** When the toolchain is
needed, the message should be about what is being unlocked and roughly how long
it takes, not a list of tool names:

> Making your own apps needs a compiler, about 5 minutes to set up once.
> Install it now? [Y/n]

The consent prompt already exists and already defaults to yes. The wording is
what needs work.

**Make the wait feel bounded and productive.** An unlabelled wait with no end
in sight is where people leave. Naming the number up front, and showing progress
through it, costs nothing and changes the experience of the same duration.

---

## 5. The one structural fix

`cargo-component` publishes **no prebuilt binaries** — its latest release has
zero assets, so it is compiled from source on every machine. That is three to
six of the six to eleven minutes, and it cannot be optimised away locally.

Two ways out, and they are not equal:

**Build elsewhere.** The direction already recorded for this: the AI runs on the
person's machine, the *build* happens on a server, and the person receives a
`.krate`. No toolchain locally, so Path B collapses to the two steps of Path A.
This is the real answer and it is also Krate Cloud's first genuine product
surface, which makes it worth doing properly rather than as a shortcut.

**Ship a prebuilt cargo-component.** Krate could publish per-platform binaries
of it alongside its own release, turning a six-minute compile into a
ten-second download. Much smaller than the above and available now, but it makes
Krate responsible for redistributing someone else's tool.

The first is the product. The second is a bridge to it.

---

## 6. What to measure afterwards

The honest test is not whether the flow reads well, it is where people stop:

- how many who install ever run an app;
- how many who run one ever try to create one;
- how many who hit the toolchain wall come back at all.

The last of those is the number that matters, and it is the one nobody
instruments until it is too late.
