# Porting an app you already have

`krate port` looks at a project you already wrote and tells you what it
would take to run it as a Krate app. It never runs your code, never
copies your source into the output, and never changes anything in the
directory you point it at.

## Look first

```sh
krate port ./my-project
```

You get a verdict, the capabilities the app appears to need, and a list
of findings with the file and line that caused each one. Nothing is
written anywhere.

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
