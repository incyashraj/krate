# Testing on all three systems

For the hands-on run and the recording. One page per machine, same steps, so
the results are comparable rather than anecdotal.

**Install from the published release, not from a build.** The whole point is to
see what a stranger sees. On the Mac you have been developing on, that means
moving `target/release/krate` out of the way first, or using a different
machine.

```bash
curl -fsSL https://krate.tech/install.sh | sh
krate --version          # must say v0.1.0-rc5 or newer
```

Anything older than rc5 cannot run the game, the chart, or anything with sound.

---

## The seven steps

`sh scripts/demo-walkthrough.sh` runs steps 1–7 with a pause before each, which
is the order to film. If you are working from the release rather than the
repository, the same steps by hand:

| # | Command | What to look for |
|---|---|---|
| 1 | `krate run --prompt savings.krate` | It names what it wants, in plain words, before running |
| 2 | Answer `n` | Refuses, and says what it needed |
| 3 | Answer `A` | Real output: a budget, split |
| 4 | `krate run --auto-grant bounce.krate -- quick` | `animated:yes` and a frame count |
| 5 | `krate run --grant "fs.read:/etc/**" hexyl.krate -- /etc/passwd` | Reads `sandbox copy`, not the real file |
| 6 | `krate run --auto-grant mdview.krate -- quick` | A markdown document parsed and rendered |
| 7 | `krate run --native-window --auto-grant bounce.krate` | **A window opens and a ball bounces** |

Step 7 is the one no automated test can do. It is also the shot worth filming.

---

## What to record per machine

Fill this in as you go. Blanks are results too.

```
Machine:            (e.g. MacBook Air M2, macOS 26)
Installed version:
Install took:

1 permission prompt shown       yes / no
2 refusal is clear              yes / no
3 app produces real output      yes / no
4 animation reports frames      yes / no
5 sandbox holds                 yes / no
6 markdown viewer works         yes / no
7 WINDOW OPENS AND ANIMATES     yes / no
  - window appears              yes / no
  - ball moves smoothly         yes / no
  - closing the window exits    yes / no

Anything that looked wrong, even slightly:
```

---

## Known gaps, so they are not surprises

- **arm64 Linux has no binary in rc5.** Its build container has a libclang too
  old for one dependency. Every other platform is published. On that machine
  the installer says so and gives the build-from-source command.
- **`--native-window` works on all three**, and by different routes: AppKit on
  macOS, winit on Windows and Linux. That is precisely why step 7 is worth
  filming on each machine rather than once — the same widget tree is drawn by
  two entirely different implementations, and the widget parity table says they
  agree. Nobody has watched them side by side.
- **A bare `krate run app.krate` on a GUI app exits after about five seconds.**
  That is deliberate: a headless run has a wall-clock budget so an animated app
  cannot freeze the terminal it was opened from.

---

## If something fails

Capture, do not debug on camera:

1. The exact command
2. The whole output, including the error
3. `krate --version` and the operating system version

A failure found on a real machine is worth more than a clean recording. The
last three real bugs — a frozen terminal, a grant that could never match, a
release that could not run its own apps — were all found this way and none by a
test.
