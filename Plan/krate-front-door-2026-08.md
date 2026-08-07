# The front door, measured against the best one we have seen

Grok's CLI onboarding was put next to ours on a real Windows machine on
2026-08-07, and ours lost. This is the plan to close that gap, written from a
trace of the actual code paths, not from impressions.

The standard the person applying it uses (their words, near enough):

1. Is it downloadable in one command?
2. Is it usable directly after download -- no closing and reopening anything?
3. Can I describe what I want immediately, and does my description survive
   everything that goes wrong around it?

Today's honest scores: 1 yes, 2 **no -- three terminal restarts**, 3 **no --
the request is discarded if no AI is connected**.

---

## Root causes, by file and line

Every one of these was traced to the line before this plan was written. None
of them is a guess.

### Three restarts, one mechanism

`install_build_tools` (`crates/cli/src/main.rs:7817`) computes the missing-tool
list once, then runs the install commands in order. On Windows the list is a
dependency chain: winget installs rustup, the *next* command is `rustup
toolchain install ...`, the one after needs `cargo`. Each command is executed
by bare name against the PATH **this process captured at startup**. The
installs before it wrote the new PATH to the registry, not into our process.
So step N+1 cannot see what step N installed, fails, and
`crates/cli/src/tui.rs:670` says "open a new terminal and try again".

A new terminal fixes it because a new terminal re-reads the registry PATH.
**A running process can do exactly that.** That is the whole fix: read
HKCU `Environment\Path` plus the machine PATH, merge into this process's PATH,
retry. No restart can then ever be needed, because anything a restart would
have picked up, we pick up.

The same staleness bites AI tools: install Grok while `krate` is open (its
installer writes `~/.grok/bin` to the registry PATH) and our probe still says
"not installed" until a restart. Same fix, same function.

### The discarded request

`choose_provider` (`crates/cli/src/tui.rs:766`) returns `None` when no AI
probes as working, the caller unwinds, and the request the person just typed
is gone. When they come back after installing an AI they type it again.
For a first-time user that is the single worst moment in the product.

### Menu-first instead of prompt-first

`run()` (`crates/cli/src/tui.rs`) lands on a five-item menu. The thing 90% of
people came to do -- describe an app -- is behind choice 1. Grok lands you in
the prompt with the menu as chrome around it, and it is the right call: the
prompt *is* the product.

### The installer narrates its internals

`scripts/install.ps1` and `scripts/install.sh` print every internal step
(release lookup, checksum, cargo-component, handler registration, a
what-if-PATH-fails paragraph). Grok prints five lines and a green "Run 'grok'
to get started!". Nobody re-reads our paragraphs; everybody sees the wall.

---

## The design

### A. No restart, ever (the PATH refresh)

One function, `refresh_process_path()`:

- **Windows**: read `HKCU\Environment\Path` and
  `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment\Path`,
  append any entry not already in this process's PATH, `set_var`. This is
  what a new terminal does, minus the terminal.
- **Unix**: append the well-known tool homes if they exist and are missing:
  `~/.cargo/bin`, `~/.grok/bin`, npm's global bin. (Unix restarts were never
  reported, but the same staleness exists in a GUI-launched terminal.)

Call it: before every toolchain probe, after every install step, and before
every AI re-probe. It is idempotent and costs microseconds.

Then `install_build_tools` **recomputes the missing list after each step**
(the list shrinks as installs land, and a tool the previous step brought in
must not be re-installed) and resolves every program through `resolve_tool`,
which already knows `~/.cargo/bin`.

Result: "setting up the compiler" is one continuous run. The failure message
"open a new terminal" is deleted -- not reworded, deleted -- because the
condition it described can no longer occur.

### B. The request survives everything (the held prompt)

The typed request becomes state that outlives whatever interrupts it:

```
> a habit tracker with a weekly chart
  No AI here can write an app yet. Get one:
  claude   npm install -g @anthropic-ai/claude-code   <- best with Krate
  grok     irm https://x.ai/cli/install.ps1 | iex
  ...
  Install one in another window, then press Enter here -- I'll find it
  and start on your habit tracker straight away.
```

Enter re-runs `refresh_process_path()` + probe. The moment a provider works,
the build starts **with the held request**. Nothing is retyped. The same
holding pattern covers the compiler bootstrap: connect AI, install toolchain,
then build, all downstream of one typed sentence.

### C. Land on the prompt

`krate` opens to: wordmark, one dim capability strip, and the prompt --
already focused, like Grok's:

```
  KRATE                                          v0.1.2
  make an app you can send to anyone

  1 connect an AI   2 open an app   3 my apps   4 history   q quit

  What do you want to make?  attach a design by typing its path

  >                                                    claude · connected
```

- Typing text = the request (today's attachment parsing unchanged).
- Typing a number/q = that action, then back to the prompt.
- The status suffix names the connected AI, or says `no AI yet -- press 1`.
- Below the answer, while a build runs: the existing stage display, then back
  to the prompt with the app's row of follow-ups (open / change / share).

No alternate screen, no redraw loop. Scrollback still works over SSH, errors
still copy out -- the constraint the TUI was built around survives; the
*order* changes. (The full-screen, chat-transcript experience is Krate
Studio's job -- `Plan/krate-studio-2026-08.md` -- not this TUI's.)

### D. The installer speaks in five lines

Target output, both platforms:

```
Installing Krate v0.1.2 (windows-x86_64)...
  Downloading... done (verified)
  Installed to C:\Users\me\AppData\Local\Krate\bin
  .krate files now open on double-click.

Run 'krate' to get started!        <- green
```

Behaviour kept, words cut: checksum still refuses on mismatch (one word,
`verified`), cargo-component still ships (silently -- it is our packaging
detail, not the user's business), the handler still registers (one line), the
PATH fallback paragraph goes (the session PATH is already patched at
`install.ps1:223`, so the failure it hedged against is the rare case -- print
that advice *only* when the session patch itself failed).

### E. Later, noted so it is not lost

- **Login**: `krate publish` already carries GitHub device-flow auth. A
  first-run `krate login` reusing it is the professional touch Grok has; it
  is not on the critical path of make-an-app and waits until it is pulled by
  a feature that needs identity (sync, gallery attribution).
- **One-shot bundle**: "download everything at once" ultimately means
  bundling toolchain setup into first run without the person watching a
  second download. The honest version today is A above (one continuous,
  no-restart setup with one progress line). Pre-bundling 3 GB of Rust into
  the installer is the wrong trade while the runtime's pitch is small-and-
  verifiable; revisit if setup friction still shows up in outsider tests.

---

## Order of work

1. **A** -- refresh + recompute + resolve. Kills the restarts. The whole
   plan is dead if this is flaky, so it lands first and gets tested by
   simulated stale-PATH runs on the Windows VM.
2. **B** -- the held request, wired through the no-AI and no-toolchain gates.
3. **C** -- prompt-first landing.
4. **D** -- installer wording, both scripts, plus the served copies.
5. Outsider re-test on the friend-class machine: fresh Windows, no dev tools,
   stopwatch from `irm | iex` to a built app, counting restarts (must be 0)
   and retypes (must be 0).

Not in scope here: alternate-screen TUI, login, streaming chat panes -- that
is Studio. This plan is the terminal telling one person "type what you want,
I will handle everything else in front of you."
