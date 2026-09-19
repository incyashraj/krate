# Porting an app you already have

`krate port` looks at a project you already wrote and tells you what it
would take to run it as a Krate app. The default scan reads source without
building or executing the project. It is an assessment, not a conversion of
an existing native executable. Preparation and AI-assisted transformation are
separate, explicit steps described below.

## Check the fit before porting

List the behavior users depend on, then map each dependency to a Krate API:

| Part of your app | Question to answer |
|---|---|
| Business logic, parsers, calculations | Can it compile to a WebAssembly component without OS-specific dependencies? |
| User interface | Can you implement its interactions with Krate widgets/canvas rather than DOM, AppKit or Win32 calls? |
| Files and saved state | Which paths need user approval, and which data belongs in app-scoped storage? |
| Network | Which hosts/ports are needed, and are HTTP or WebSocket interfaces sufficient? |
| Native libraries and subprocesses | Is there a portable replacement? A native shared library or shell process does not become a Krate API. |
| OS integration | Is every required feature in the [current capability list](limits.md)? |

For Electron projects, inventory Node and browser/DOM dependencies. For Tauri
projects, inventory Rust commands, plugins and WebView interactions. Neither
project runs unchanged inside Krate. Pure logic may be reusable; UI and host
integration often need adaptation. A missing essential API is a stop condition,
not something a packaging flag can fix.

## Look first

```sh
krate port ./my-project
```

You get a verdict, the capabilities the app appears to need, and a list
of findings with the file and line that caused each one. The default scan
does not create an output workspace.

Run it on a small CLI that reads a file and you get back something like
this:

```
Verdict: needs changes
Profile: krate-cli-v1-candidate
Languages: rust

Likely capabilities
  - fs.read:<path> / fs.write:<path>

Findings
  [CHANGE, medium confidence] Local filesystem use
    Map each path to an app-scoped Krate file capability and remove
    ambient filesystem access.
    at src/main.rs:1

Read-only scan: 2 files seen, 2 text files scanned, 392 bytes read
```

That last line is the receipt. It says exactly how much of your project
was read.

For a machine-readable version:

```sh
krate port ./my-project --format json --output plan.json
```

## Prepare a workspace

```sh
krate port ./my-project --prepare ./port-work
```

This builds a separate directory to work in. Your original project is
still untouched. Inside you get:

| File | What it is |
|---|---|
| `PORTING.md` | The plan, written for a person |
| `AGENT_TASK.md` | The same plan, written for an AI coding agent |
| `JOURNEYS.md` | The user journeys the port has to preserve |
| `candidate/` | A Krate app skeleton that already compiles |
| `reference-source/` | A snapshot used to detect drift |
| `port-plan.json` | The plan as data |

The candidate compiles from the first minute, so you always have
something that builds while you move logic across.

## Let an agent do the transformation

```sh
krate port ./my-project --prepare ./port-work --agent claude --to my-app.krate
```

The agent works inside the prepared workspace, not in your project. When
it finishes, Krate re-scans your original source and stops if the
contents changed while the agent was running, so a port never silently
races your own edits.

With `--to`, the result is built, inspected, packaged, and permission
tested before you get the file. Add `--transcript port.log` to keep a
record of what happened.

If you use a different agent, `--author-cmd` runs any command you like
inside the workspace with `KRATE_PORT_SOURCE`, `KRATE_PORT_PLAN`,
`KRATE_PORT_CANDIDATE` and `KRATE_PORT_TASK` set.

## What ports well

The work is mostly at the edges. Pure logic moves across almost
unchanged. What needs attention is every place the app reaches the
operating system, because that is what the capability wall governs.

Ambient filesystem access becomes a scoped grant or a file picker.
Spawning processes has no equivalent and needs a rethink. Anything on the
["not yet" list](limits.md) is a genuine blocker rather than a
conversion.

Read [what Krate cannot do yet](limits.md) before you start a port. It is
the fastest way to find out if the thing you are porting is a fit.

## Decide whether the port is ready to share

A successful build is only the start. Use `JOURNEYS.md` to compare the original
and port: open representative files, edit and save, restart, try invalid input,
resize the UI and deny permissions. Check that the same `.krate` file completes
those tasks on each target OS with a compatible runtime. Record changed or
missing behavior rather than calling it feature parity prematurely.

Measure payload, complete bundle and runtime footprint separately. For speed
or memory comparisons, use the same inputs and task, include the full process
tree and distinguish cold from warm runs. See the
[distribution comparison](https://krate.tech/desktop-app-distribution.html)
for the architectural tradeoffs.

If a required feature is missing, describe the operation, expected behavior
and a minimal example in [GitHub Discussions](https://github.com/incyashraj/krate/discussions).
Do not share private project source or credentials to demonstrate the need.
