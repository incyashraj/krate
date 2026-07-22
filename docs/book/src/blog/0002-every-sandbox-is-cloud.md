# Every AI sandbox in 2026 runs in someone else's cloud

**Published:** 2026-07-22

Modal published a comparison of the best code execution sandboxes for AI agents.
Seven providers: Modal, E2B, Northflank, Daytona, Blaxel, Vercel Sandbox,
Cloudflare Sandboxes.

Every single one is cloud-hosted. Every single one isolates with containers or
microVMs. Not one of them uses a capability-based permission model, and not one
runs on the machine the developer is already sitting at.

That is not a criticism of those products. They are good at what they do, and
for most agent workloads the cloud is the right answer. But it means an entire
half of the problem has no serious entrant, and I think that half matters more
than it looks.

## The assumption nobody states

Every one of those tools starts from the same premise: untrusted code should run
far away from the user, in an environment that can be destroyed.

That premise is correct for scale. It is wrong for a large and growing class of
work, because the interesting things an agent does are usually about your stuff.
Your files. Your local database. Your credentials. The application you are
building right now. Ship that work to a remote microVM and you have to ship your
context with it, which is either a security problem or a synchronization
problem, and usually both.

So you end up with two bad options. Run agent-written code in the cloud and lose
access to the local context that made it useful. Or run it locally with ambient
authority, which means a script that was supposed to rename some files can read
your SSH keys, because on every mainstream operating system a process inherits
everything you can do.

Isolation solved the first problem. Nobody solved the second one, because the
second one is not an isolation problem. It is a permission problem.

## Container isolation and capability permission are different things

A container answers: what machine is this code on?

A capability answers: what is this specific code allowed to touch, right now,
and who decided that?

You can have excellent isolation and terrible permissions. A microVM with a
container full of your credentials is a strong wall around a room where
everything is unlocked. The wall stops the code escaping to the host. It does
nothing about what the code does with what is already inside.

The capability model inverts the default. Code starts with nothing. Not a
restricted filesystem, not a read-only mount. Nothing. Then a specific grant is
made for a specific resource, deliberately, by a human or a policy:

```console
$ krate run --grant fs.read:./secrets.txt --manifest app.toml cat.wasm -- ./secrets.txt
```

Without that grant the same binary running the same code cannot open the file.
It does not crash on a permission error deep in a syscall. It gets a structured
denial: this capability was requested, it was not granted, here is the identity
of the thing that was denied.

That last part is the piece I think is underrated for agents. A generic `EACCES`
tells an agent that something went wrong. A structured denial tells it exactly
which authority it lacks, which means it can ask for that specific thing, or
route around it, or report to the user precisely what it needs and why. Failure
becomes information instead of a dead end.

## What I built

Krate is a runtime for that model. A program is one WebAssembly component in a
single file. It runs natively on macOS, Windows and Linux, opening a real
desktop window, not a bundled browser. It starts with zero ambient permissions
and receives capabilities only through explicit grants.

The demo application is 26 kilobytes. The same file, unmodified, runs on all
three operating systems.

I want to be precise about what is and is not proven, because this space is full
of claims.

Every push to the repository triggers CI across all three operating systems. On
all three, the component is built and executed and opens a real window. On
Linux, CI goes further: it runs the app under Xvfb, uses xdotool to click the
button, type into the text field and scroll the list, then photographs the
screen and uploads the image as a build artifact. A machine I have never touched
exercised the interface and produced the evidence, before I wrote this sentence.

![The Krate demo app running on Linux under CI: a clicked button, the text "hi
krate" typed into the field, a checkbox, a progress bar, and a list scrolled
down to line eight](../images/hello-gui-linux-scrolled-2026-07-20.png)

The click-and-photograph step is Linux only today. macOS and Windows run the
component and open real windows in the same CI run, but are not yet driven
synthetically. Extending that is on the list.

The other honest caveat: on macOS, widgets are real AppKit controls, genuine
`NSButton` and `NSTextField`. On Linux and Windows the widgets are currently
drawn through a shared rendering path rather than being native OS controls. The
window is real on all three. The controls inside it are not equally native yet.

## Why this shape, and why now

Two things had to become true for this to be buildable at all.

The WebAssembly Component Model matured to the point where one compiled artifact
can carry typed interfaces across languages and hosts. That is what makes a
single file legitimately portable rather than portable-if-you-squint.

And AI started generating far more software than humans can review. That changes
the economics of trust. When a person writes code and runs it, they have some
idea what it does. When an agent writes it, nobody has read it, and the honest
position is that you do not know. Ambient authority was always a bad default. It
becomes an untenable one at machine-generated volume.

Neither curve produces this on its own. The intersection does.

## What I am not claiming

Krate is not a replacement for cloud sandboxes. If you are running ten thousand
concurrent agent tasks, you want microVMs and a control plane, and you should
use one of the seven products above.

It is also early. There is no SDK yet, the developer experience is rough, and
the honest state of it is in [STATUS.md] rather than in a marketing page.

What I am claiming is narrower: local execution with a real permission boundary
is a missing primitive, the entire market has been building the other half, and
a portable component format with capabilities attached is a plausible shape for
the missing piece.

## If you build agents

The question I actually want answered: if you were routing agent-generated code
through something like this, what capability would you need first that does not
exist yet?

That answer is worth more to me than a star. The repo is public, the CI evidence
is in the Actions log, and my DMs are open.

- Repo: <https://github.com/incyashraj/krate>
- Four minute demo: <https://www.youtube.com/watch?v=RFefANqu0fc>

[STATUS.md]: https://github.com/incyashraj/krate/blob/main/STATUS.md
