# The build service

What happens when someone types a sentence on krate.tech and presses send.

It runs `krate create` -- the same command the CLI and Studio run, not a
reimplementation -- so an app made in a browser and an app made on a
desktop cannot come out different.

## Where it runs

A Fly machine, at `build.krate.tech`. Not a Worker and not a serverless
function: one build is a quarter of an hour of solid CPU that spawns a
compiler and writes hundreds of megabytes of object files, and nothing about
that fits a request-shaped runtime.

The machine **stops when nobody is building** and starts again on the next
request, so the cost is CPU while a build runs plus a few dollars a month for
the volume, rather than a box billed around the clock for an idle service.

```bash
./deploy.sh          # first-time setup and every redeploy
```

Then, once:

```bash
flyctl certs add build.krate.tech --app krate-builder
```

and add the CNAME it prints at Cloudflare **DNS only (grey cloud)**. Proxied
(orange) breaks it: Cloudflare's edge times a request out at 100 seconds,
which is shorter than a build.

## The switch

The service ships **off**. With no AI key it is live and healthy and refuses
builds with a sentence that says so, which is exactly what you want between
"the machine exists" and "the machine is spending money".

```bash
curl https://build.krate.tech/health
# "authoring":"off"  -> live, refusing builds, spending nothing
# "authoring":"on"   -> making apps, spending on each one
```

Turn it on when you mean to:

```bash
flyctl secrets set ANTHROPIC_API_KEY=sk-... --app krate-builder
```

## Why the agent is `anthropic` and not `claude`

`--agent claude` drives the Claude Code CLI, which has to be installed **and
interactively signed in**. No server can do the second part.

`--agent anthropic` runs the same authoring prompt against the model API with
a key from the environment -- `krate` implements that loop itself, executing
the tool calls that a CLI agent would otherwise execute for itself. It is the
only authoring path that is honest on a headless machine.

## Run it locally

```bash
KRATE_BIN=/path/to/krate KRATE_AGENT=claude node src/server.js
```

| Variable | What it is |
|---|---|
| `KRATE_BIN` | The engine. Use an absolute path: a bare `krate` may resolve to an older installed release |
| `KRATE_AGENT` | Which AI writes the app (`anthropic`, `claude`, `codex`, ...) |
| `KRATE_HUB` | The hub the wall is checked against |
| `KRATE_ORIGIN` | The site allowed to call this, for CORS |
| `KRATE_BUILD_TIMEOUT_MS` | How long one build may take. Default 15 minutes |
| `KRATE_BUILDER_DEV` | `1` skips the wall, for developing the service. **Never set in production** |
| `KRATE_PLAN_PER_HOUR` | Plans one account may ask for on our key per hour. Default 12, counted on the volume so a wake does not reset it |
| `KRATE_PLAN_BUDGET_USD_PER_DAY` | What planning on our key may cost in one day, all accounts together. Default 10 |
| `KRATE_PLAN_UNPRICED_USD` | What a plan is charged when the engine prints no `krate-spend:` line. Default 0.45, the old worst case |
| `KRATE_COMPILE_PER_HOUR` / `KRATE_COMPILE_PER_DAY` | Compiles one account may start (`/compile`). Default 20 and 60, counted on the volume |
| `KRATE_COMPILE_CONCURRENT` | Compiles running at once on the whole machine. Default 2 |
| `KRATE_COMPILE_TIMEOUT_MS` | Wall clock for one compile, check and pack together. Default 10 minutes, for a cold dependency cache |
| `KRATE_COMPILE_CRATES` | The crates.io crates uploaded source may depend on, comma-separated. Default `wit-bindgen-rt,libm` (K-1293) |
| `KRATE_SDK_ROOT` | Where the SDK is, for `/compile`. Unset, the engine's own cache is used |
| `KRATE_BUILD_USER` | The user every build runs as (K-1293). Default `kratebuild`. The service must be root to hand builds to it; with `FLY_APP_NAME` set and no such user, every build door answers 503 |
| `KRATE_BUILD_ENV_PASS` | Extra environment names a build may see, comma-separated (a new engine setting, a test hook). A name that reads like a credential is refused |
| `KRATE_JUDGE_DIR` | Where uploads being judged land. `/work/judge` in the image: root's, 0711, so the judge reads by exact path only |
| `KRATE_BUILDER_SECRET` | The secret the hub's `/keys/use` asks for before it hands out a person's own API key (K-861). A Fly secret here and a Worker secret on the hub, the same value on both; unset, bring-your-own-key builds fall back to the funded path |

## The doors

```
POST /build            { request }   -> { id }
GET  /build/<id>                     -> { state, stage, line, shot, error, result }
GET  /build/<id>/file                -> the .krate itself
POST /build/<id>/stop                -> { ok }
GET  /health                         -> { ok, building, authoring, agent, isolation }

POST /compile          { name, files: [{ path, data }] }   -> 202 { id }  | 4xx { error }
GET  /guide                          -> text/markdown
```

`/compile` and `/guide` are the hub's (IC-989 phase 2, "Claude writes,
Krate builds"). Both need the builder secret in `x-krate-builder`.
`/compile`, and reading its job, also need `x-krate-account: hub:<owner>`
-- `hub:github:alice`, `hub:account:<id>` -- the account the hub vouches
for; there is no person's session on this path, and the per-account limits
count on that account. `/compile` takes source a chat model wrote -- `data`
is base64; Cargo.toml, Cargo.lock, manifest.toml, krate-check.toml,
`src/**`, `assets/**` and nothing else -- runs `krate check-app --json` and
`krate pack`, and never a model. It answers at once with a job id; the job
reads through `GET /build/<id>` (same two headers) as
`state: "building" | "done" | "failed"`, with `check` (check-app's own
verdict: `stage`, `kind`, `detail`, `fix`) so the model can correct its
code. `/build/<id>/file` is the UNSIGNED bundle: the hub signs it as the
account (`signAsAccount`), judges it and publishes it unlisted. Write the
SDK in Cargo.toml as `{KRATE_SDK}` (or Krate Mode's `PREFIX`); the builder
puts its own path there. The guide is `krate krate-mode` (about 110 KB: the
API, the permission catalog, the no_std rules, worked examples -- the
authoring pack is 195 KB and written for an agent in a starter folder) with
its "you cannot build this" handoff cut, then how to send the source and a
Cargo.toml naming only the WIT packages this engine's SDK has. Made once
per engine version. `cloud/worker/test/mcp-build-e2e.test.mjs` runs the
hub's tools against this server.

## What a build can reach (K-1293)

A build runs code nobody here wrote: the engine's AI loop, cargo, rustc,
every dependency's build script and proc-macro, the app when it is
photographed, the judge reading an upload. Any of it could read a file at
compile time -- `include_str!("/proc/1/environ")` -- and send the bytes
back in the bundle or a compiler error.

**The boundary is the uid.** The service runs as root and holds the hub
secret and the Anthropic key; every build-time child runs as `kratebuild`
(`src/sandbox.js`, the one function every such child is started through).
A process root handed to another uid cannot read root's
`/proc/<pid>/environ`, and `/work/state` -- jobs, other people's finished
apps, the ledgers -- is root's and 0700. `/health` says
`"isolation":"build-user"` when this holds, and a production box where it
does not refuses every build rather than run it as root.

**The environment is the second line.** A child gets an allow-list:
the toolchain, the engine's settings, its own HOME and TMPDIR. Never
`KRATE_BUILDER_SECRET`. A model key only on the paths that talk to a model:

| Door | Key the child gets |
|---|---|
| `/build`, `/build/<id>/revise`, `/port`, `/plan` | the agent's own, the person's if they set one, else ours |
| `/compile`, `/guide`, `/port/plan`, `/judge`, the picture of a built app | none |

On the AI paths the key necessarily reaches the agent and everything it
runs, its build scripts included: that is how it talks to the model. What
never reaches any build is anything of the service's own.

`/compile` also narrows what uploaded source may do (a crate allow-list, no
build scripts, no path or git dependencies, `include_str!` only inside the
app). That is defence in depth and stays; the uid is what holds.

After a deploy, prove it on the machine itself:

```bash
fly ssh console --app krate-builder -C krate-isolation-check
```

It checks, as the build user and through `src/sandbox.js` exactly as the
server spawns, that neither the service's nor PID 1's environ can be read,
that `/work/state` cannot be listed, and that the secret appears nowhere a
build can see. It never prints the secret.

Not covered by the uid: builds share one user, so two builds running at the
same moment can read each other's files and environment, and the caches in
`/work/build` are shared between them (K-1297).

## The two guards that matter

Every build is real inference we pay for, so:

- **The wall is asked of the hub, never of the browser.** A counter the
  page owns is a counter anyone can edit.
- **One build at a time per account.** Not politeness -- the difference
  between a bill and a bankruptcy.

Only a build that produced a file counts against the free allowance. A
failure the person did not cause never costs them one.

## Three things the image gets right on purpose

Each of these was a build failure before it was a comment:

- **Trixie, not Bookworm.** The released Linux binary links GLIBC 2.38+;
  Bookworm ships 2.36, so the engine installs fine and then will not start.
- **`cargo-component` comes out of the release tarball**, not `cargo install`.
  It is by construction the version that engine was released against, and it
  saves several minutes of image build.
- **`CARGO_HOME` stays in the image; only `HOME` moves to the volume.**
  The engine keeps its own dependency cache at `$HOME/.cache/krate/build`,
  keyed by SDK fingerprint. Setting `CARGO_TARGET_DIR` to "help" switches
  that off and replaces a correct cache with a blunter one.
