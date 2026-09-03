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

## The doors

```
POST /build            { request }   -> { id }
GET  /build/<id>                     -> { state, stage, line, shot, error, result }
GET  /build/<id>/file                -> the .krate itself
POST /build/<id>/stop                -> { ok }
GET  /health                         -> { ok, building, authoring, agent }
```

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
