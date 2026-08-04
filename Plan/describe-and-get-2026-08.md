# Describe and get

**Goal:** a person describes an app in plain words and gets a working `.krate`.
No commands typed by hand, no flags to get wrong, no terminal if they do not
want one.

**Constraint that shapes everything:** the loop must be reliable before it is
made easy to reach. A connect-your-AI button in front of a loop that fails is
worse than no button, because the failure arrives with no way to debug it.

---

## Where we actually are (measured 2026-08-04, not estimated)

| | Result |
|---|---|
| `krate create` with the built-in template | **Was broken for every request.** Fixed in 9763f5d. |
| `krate create --agent claude`, 5 plain requests | **5/5 passed**, 110-275s, 548-946 lines each |
| Commands published on the website | One was unrunnable (missing `--output`). Fixed. |
| What the user sees while waiting | Was a row of dots. Now real steps. Fixed. |

So the loop works when it is driven correctly. What we have never measured is
how it behaves across a *wide* range of requests, and that is the whole
question. Five apps is an anecdote. The plan below turns it into a number.

---

## The four workstations

Each is independent enough to run in its own worktree and be worked on without
blocking the others. W1 is the gate: W3 does not ship to real users until W1's
number is good.

### W1 — Reliability. Make it work for any request.

**Owns:** the authoring loop, `check-app`, the context pack, the templates.

**The one metric:** *pass rate across a broad, honest request corpus.*
Not "does it work", but "out of 100 varied requests, how many produce an app
that builds, imports zero OS calls, runs, and paints a frame."

Work:

1. **Build the corpus.** 100 requests across categories a real person would
   ask for: productivity, games, utilities, calculators, viewers, trackers,
   creative tools, reference tools. Include deliberately awkward ones -- vague
   ("something for my morning"), too big ("a spreadsheet"), impossible
   ("download my email"), and ambiguous ("a timer" -- countdown or stopwatch?).
2. **Build the harness.** One command runs the corpus, records per-request
   pass/fail, the failing stage (`check-app` already has distinct exit codes
   10-15), wall time, and the transcript. Output is a table, not prose.
3. **Run it, then fix by frequency.** Every failure gets classified. Fix the
   most common cause first. Re-run. The loop is: measure, fix the top cause,
   measure again.
4. **Push fixes into the oracle, not the docs.** The `krate` dependency case
   is the lesson: the context pack said "KEEP the krate dependency" in capitals
   and it still went missing. Where we know the right answer, enforce or repair
   it in code. Documentation is for things we cannot check.
5. **Handle the requests we should refuse.** "Download my email" cannot be a
   Krate app. Saying so clearly in one sentence is a pass, not a failure --
   silently producing a checklist named `download-my-email` is the failure.

**Done when:** pass rate is stable above a bar we set after seeing the first
number, and every remaining failure is a category we can name and explain.

### W2 — The command builder. Ship this week.

**Owns:** a small interactive block on
`https://krate.tech/docs/pages/make-an-app-with-ai.html`.

A textarea, a picker for which AI (Claude Code today; the list grows with W3),
and a generated command with a copy button. Pure client-side HTML, no backend.

Why it matters even though W3 supersedes it: you got the command wrong twice by
copying from our own pages, which means everyone will. This removes that class
of failure permanently, works for people who prefer a terminal, and doubles as
a demo -- type "a habit tracker" and watch a real command appear.

**Done when:** it is impossible to copy a command from our site that does not
run.

### W3 — Connect your AI. The describe-and-get product.

**Owns:** the flow where someone connects an AI account once and then only ever
describes apps.

Design principle: **provider-agnostic from the first line of code.** Not
"Claude, and we will add others later" -- an `AiProvider` trait with Claude as
the first implementation, so adding OpenAI or Gemini is a new file, not a
refactor. This matters commercially too: "works with your AI" is a much better
story to an investor than "works with Claude."

Three things to decide before building, in order:

1. **Where the AI runs.** Bring-your-own (their key or their installed agent,
   Krate drives it locally) or Krate-hosted (we pay for inference). BYO is
   cheap, private, and shippable now. Hosted is the magic demo and a cost
   centre. My recommendation: build BYO first behind the provider trait, so
   hosted becomes a provider we add rather than a rewrite.
2. **Where the build runs.** Local is safe and free. Server-side means
   compiling untrusted AI-generated Rust on our machines, which is a real
   security surface at exactly the moment our whole pitch is that code runs
   safely on the *user's* machine. Strong default: build locally, always.
3. **What "connect" means per provider.** OAuth where the provider supports it,
   an API key where it does not, and a detected local agent where one exists.
   All three land on the same trait.

**Blocked on W1.** Not by dependency, by judgement: this workstation makes the
loop easy to reach, so it must not ship before the loop is worth reaching.

### W4 — The professional path. Do not break it.

**Owns:** everything someone building an app by hand needs -- `krate create`
without an agent, hand-written apps, `check-app`, `port`, the SDK docs.

This is small and mostly maintenance, but it needs an owner so it does not rot
while attention goes to AI authoring. The people most likely to write a Krate
app *well* today are developers, and they are also the ones who will tell us
what is wrong with the SDK.

**Done when:** every path a developer takes has a test, and `check-app` is
their first-class tool rather than an internal detail.

---

## Order

1. **W1 corpus + harness first.** Everything else is guesswork without the
   number.
2. **W2 in parallel** -- it is an afternoon and it fixes a live problem.
3. **W1 fix cycles** until the pass rate stops moving.
4. **W3** once the number is good, provider-agnostic from the start.
5. **W4** continuously, as a standing duty rather than a phase.

## The honest risk

"Cover literally every random possible request" is not reachable, and promising
it would be a lie. What is reachable: a high pass rate on the kinds of app
Krate is actually for -- self-contained, local, single-window tools and games
-- plus a clear, fast, honest refusal for everything else. A person who asks
for something impossible and gets told why in one sentence has had a good
experience. A person who waits four minutes for a broken app has not.

The corpus is what turns that from an opinion into a measurement.
