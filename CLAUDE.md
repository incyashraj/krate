# Working on Krate

## The bug board: BUGS.md

`BUGS.md` at the repo root is the **only** bug list in this repository. Do not
start a second one in a plan doc, an evidence file, or a comment.

**Read it before you start.** It tells you what is already known and who owns
what.

**File the moment you find something.** Give the next free `K-` number, and put
the command and its output in `Evidence:` — proof, not a description.

**Do not detour to fix it.** If the bug is outside your task, file it and keep
going. Filing is the contribution. Say in your report that it is unclaimed so it
can be assigned.

**Claim before fixing.** Check `Owner:`. If it names a workstation that is not
you, leave it alone — two agents fixing one bug in two worktrees means a merge
conflict and two half-solutions. If the owner looks stuck, say so in your report
rather than taking it over.

**Keep fixed entries.** Move to Fixed with the commit that did it. A fixed bug
with its evidence is how the next person avoids reintroducing it.

Every entry has a `Class:`, which says who can fix it:

- `runtime-hole` — the runtime cannot do it. Only we can. No prompt helps.
- `teaching-hole` — the runtime can do it and the authoring pack never said so.
- `example-bug` — our reference apps teach it, so every generated app inherits
  it. Highest leverage per line changed.
- `our-code` — an ordinary defect in Krate.
- `environment` — this machine, not the product. Record it so it is not
  rediscovered and not mistaken for a product failure.


## GOALS.md: the plan of record

`GOALS.md` at the repo root holds direction and progress -- the long-term goal,
the mid-term gates, and the numbered workstations. Read it to see what your work
is for. `BUGS.md` holds defects; `GOALS.md` holds direction. There are no other
tracking files.

Work top-down: goal, then gate, then workstation. If a task has no line back up
to a goal in that file, question whether it should be done at all.

## Testing like a stranger

Users never ask for templates, so tests must not use them. Ask for arbitrary
apps nobody has built, then **actually use them** -- click every control, resize
the window, add items past the bottom of the list, leave it alone for thirty
seconds. Building is not passing. Six check-app stages passed an app that could
not be clicked.

`~/krate-outsider` is an isolated sandbox for testing from a stranger's point of
view: only the publicly installed `krate`, only krate.tech, never the repo
source. It is the only uncontaminated signal we have about what a real person
experiences.

## Two things that keep biting

**The binary on PATH is not the one you built.** A bare `krate` resolves to an
older installed release, and on this machine a debug build shadows even that.
Always invoke the binary by absolute path.

**If you are in a worktree, that path is YOUR worktree's target** --
`<your-worktree>/target/release/krate`, not the main repo's. The main repo's
binary was built by someone else, possibly before your WIT existed, and it fails
in ways that read as a broken app rather than a stale tool ("this app needs a
newer version of Krate"). Both mistakes have cost real time.

**Generated bindings churn.** Building rewrites `apps/*/src/bindings.rs`. Run
`git checkout apps/*/src/bindings.rs` before committing unless you actually
changed the WIT.

## Commits

Credited to Yashraj Pardeshi only. **Never** add AI co-author trailers or
"Generated with Claude Code". Plain language, no buzzwords, no em-dashes — use
" -- ".
