# Make Krate a tool the models can call

**Goal:** someone already talking to Claude, ChatGPT, or Cursor says "build me
X and package it as a .krate" and gets a working file back, without leaving the
chat and without learning a command.

**Why now:** this is the top-of-funnel for early users, and early users are what
the raise needs. Everything else we build is worth less than ten people outside
this machine shipping apps.

---

## The correction that shapes the design

The strategy Grok laid out assumes `.krate` is a declarative agent spec --
`add_tool`, `add_agent`, `set_memory`, "prefer declarative tool definitions over
free-form code." That is the Langchain shape, and it is not what Krate is.

**A `.krate` is a compiled WebAssembly component.** `apps/krate-pulse` is 850
lines of hand-written `#![no_std]` Rust that draws a finance dashboard pixel by
pixel on a canvas. There is no `add_tool` to call and no schema to fill in. The
model has to write real Rust, and a build takes two to five minutes.

Three consequences, and they are the whole design:

1. **There is no `create_app` that returns a structure.** The MCP tools wrap the
   authoring loop we already have, not a schema.
2. **Tools must be async-shaped.** A single call that blocks for four minutes
   will hit client timeouts. Start a job, poll it, fetch the result.
3. **Our moat is the oracle, not the format.** Anyone can prompt a model to
   write Rust. `krate check-app` is what turns "the model wrote something" into
   "this actually builds, imports zero OS calls, runs, and paints a frame."
   That is what we expose.

What we already have that maps directly:

| Grok's tool | What exists today |
|---|---|
| `validate_app` | `krate check-app <dir> --json` -- six stages, distinct exit codes 10-15 |
| `get_schema` | `KRATE_AUTHORING.md`, 411 lines, generated from the real WIT so it cannot drift |
| `package_krate` | `krate create --json` |
| `run_sandbox_test` | `krate run --shoot frame.png` -- renders the app headless to a PNG |
| `list_templates` | the built-in kinds plus the `apps/` tree |

So this is mostly plumbing around working parts, not new invention.

---

## Priority order

Grok's order is right. I would change only the framing of #2.

### 1. Krate MCP server -- the highest leverage

One binary, `krate mcp`, speaking Model Context Protocol over stdio. A user
adds it once to Claude Desktop or Cursor and then just talks.

**Tools, shaped for a slow build:**

- `krate_schema()` -> the authoring pack. Cheap, called first, teaches the model
  the real API instead of letting it guess.
- `krate_examples(kind?)` -> two or three complete shipped apps as source. This
  is the highest-value thing we can hand a model. Pulse and Nova are better
  teachers than any prose.
- `krate_start_build(description, name?)` -> starts authoring, returns a job id
  immediately. Never blocks.
- `krate_build_status(job_id)` -> stage, progress line, and the check-app
  verdict when it lands. The model polls this and can narrate real progress to
  the user.
- `krate_check(source_files)` -> run the oracle on code the model wrote itself,
  without a full build cycle. This is the tight feedback loop that makes the
  model correct itself.
- `krate_package(job_id)` -> the finished `.krate`, as a local path plus a
  base64 blob for clients that cannot reach the filesystem.
- `krate_run(job_id)` -> render the app's first frame to a PNG so the model can
  *look at what it built* and judge it. We already have `--shoot`.

**The build runs on the user's machine.** Not ours. Compiling model-written Rust
is executing model-written Rust; a hosted build service is an endpoint whose
advertised feature is running strangers' code, and it contradicts the whole
pitch that Krate code runs safely on your own computer.

**Done when:** a person with Claude Desktop installs the server, says "build me
a habit tracker and package it", and gets a working `.krate` without typing a
command.

### 2. Krate Mode -- the portable prompt (move this up)

Grok has this third. I would ship it **second, or even first**, for one reason:
it is a text file. It costs an afternoon, needs no install, works in every
client including ones we have never heard of, and it is the thing we can put in
front of people *this week* while MCP is being built.

It is `KRATE_AUTHORING.md` plus output rules plus examples, published at a
stable URL. Someone pastes it into any chat and the model writes correct Krate
code. They still need `krate create` locally to build it, which is exactly the
handoff the command builder on the site now covers.

This is also the honest fallback when MCP is not available, and MCP will not be
available for ChatGPT users for a while.

### 3. Official Custom GPT + Claude Project

Same content as Krate Mode, packaged as a shareable artifact per platform.
Fastest acquisition channel because it is one link. Depends on #2 existing.

### 4. Cursor rules + extension

A `.cursorrules` file is nearly free once #2 exists. The extension is a real
project and should wait for the MCP server, since it would call the same tools.

### 5. Everything else

ChatGPT Actions, a hosted option, signed URLs. Later.

---

## What has to be true before any of this ships

**The loop must be reliable.** Measured yesterday: 14 of 14 requests that
reached the AI produced a working app. That is a good number but a small
sample, and the rest of the corpus was never measured because the account ran
out of quota.

**There is no refusal path.** Ask for "download my email" and Krate will
probably spend three minutes producing a plausible mail-reader UI over fake
local state that builds, runs, and exits 0. Nothing compares the app to the
request. This matters far more under MCP than under a CLI: a person who typed a
command knows what they asked for, but a model that gets a green check will
confidently tell the user their email app is ready.

**Fix the refusal before shipping MCP.** The cheapest honest version is a screen
before authoring that names what Krate cannot do -- reach arbitrary hosts
without a granted capability, talk to another person's device, run in the
background -- and stops with one clear sentence instead of spending three
minutes producing something wrong.

---

## Workstations

Independent enough to run in parallel, in their own worktrees.

**W5 -- MCP server.** `krate mcp` over stdio, the seven tools above, async job
model. The biggest piece. Needs the job store, the protocol handling, and an
end-to-end test that a client can connect and drive a build.

**W6 -- Krate Mode + schema publication.** The portable prompt, the examples,
the stable URL, and the page on krate.tech that hands it over. Small, fast,
shippable first.

**W7 -- The refusal path.** Screen requests Krate cannot serve, before
authoring. Add a stage or a pre-check that says so in one sentence. Then
hand-inspect corpus requests 96-100, which the quota ran out before reaching.

**W8 -- Reliability, continued.** Fix the harness to record a rate-limit
rejection as `skipped` rather than `fail` and to pause instead of burning the
corpus in ninety seconds. Re-run on fresh quota. This is the number that tells
us whether MCP is ready for strangers.

## Order

W6 and W7 first -- one is an afternoon, the other is a correctness gate. W5 in
parallel since it is the long pole. W8 continuously.

## The honest risk

MCP raises the stakes on every reliability problem. A CLI failure is a person
reading an error. An MCP failure is a model telling someone their app is ready
when it is not. We should not put this in front of early users until the
refusal path exists and the pass rate is measured on more than fourteen
requests.
