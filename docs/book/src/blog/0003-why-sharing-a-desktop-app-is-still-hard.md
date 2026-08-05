# Why sharing a desktop app is still hard in 2026

**Published:** 2026-08-05

Someone builds a small tool. It works. They want to send it to a colleague.

On the web this takes ten seconds: deploy, send a URL, done. On the desktop it
has never worked that way, and after thirty years the reasons are worth stating
plainly, because they are not the ones people usually give.

## The four walls

**You must build it three times.** A macOS binary does not run on Windows. A
Windows binary does not run on Linux. Each needs its own toolchain, its own CI
runner, and its own set of things that go wrong only on that platform. A person
who wrote two hundred lines of Python now maintains three build pipelines.

**You must sign it, and signing costs money and time.** Apple wants a developer
account at $99 a year and a notarization round trip for every build. Microsoft
wants a code-signing certificate, which costs more and requires proving your
identity to a certificate authority. Skip either and the operating system tells
your colleague the file is dangerous.

**The recipient must install a runtime, or you must ship one.** Electron solves
this by bundling a browser: your two-hundred-line tool becomes a 150 MB
download. Python solves it by asking the recipient to install Python, which is
fine until it is the wrong version.

**Nobody knows what it does.** A `.exe` or a `.app` is opaque. The recipient
either trusts you completely or does not run it. There is no middle option, no
way to say "open a window and save its own files, and nothing else" and have
the system enforce it.

## Why AI made this worse, not better

An AI can write a working desktop app in two minutes. That part is genuinely
solved, and it moved fast.

But it made the distribution problem more visible rather than less. Before, the
person who could write an app could usually also build and sign it -- the same
skills clustered. Now someone with no programming background can produce a real
tool and immediately hit a wall built entirely out of platform-specific
tooling, certificate authorities, and installer formats. The making got easy.
The sending did not move at all.

There is a second problem AI created outright. If a machine wrote the code and
you did not read it, "do you trust the author" is no longer a question anyone
can answer. You need the system to constrain what the app can do, because
reading it is not on the table.

## What actually needs to be true

For sharing an app to feel like sharing a document, four things have to hold at
once:

1. **One file, every system.** Not three builds. One artifact that runs
   unmodified on Mac, Windows, and Linux.
2. **No per-app install.** The recipient installs one runtime, once, and every
   app after that just opens.
3. **The app declares what it wants, and the system enforces it.** Not a
   promise in a README -- a list the runtime checks before a single instruction
   runs.
4. **Small enough to email.** Kilobytes, not hundreds of megabytes.

Each of these has existed separately. WebAssembly gives you the first.
Capability-based security gives you the third. What has been missing is
something that does all four at once for ordinary desktop applications with
windows and buttons, rather than for servers or plugins.

## What Krate does about it

[Krate](https://krate.tech) is one answer. An app is a single `.krate` file,
usually 15 to 40 KB, that runs on Mac, Windows, and Linux from the same bytes.
The recipient installs Krate once and double-clicks the file.

Before the app runs, Krate shows what it asks for, in the app's own words:
"open a window", "save its own notes", "connect to hnrss.org". Anything not on
that list is unavailable -- not discouraged, unavailable. The app cannot read
your documents because the capability was never granted, not because it chose
not to.

There is no signing certificate, because there is nothing to sign for: the
sandbox is what makes an unknown app safe to open, rather than a signature
saying who to blame afterwards.

You can [see apps people have published](https://krate.tech/cloud) and open one
yourself. Each listing shows exactly what that app asks for before you download
anything.
