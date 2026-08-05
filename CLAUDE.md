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

## Two things that keep biting

**The binary on PATH is not the one you built.** `krate` resolves to an older
installed release. Always invoke `/Users/yashrajpardeshi/Projects/layer6x6/target/release/krate`
by absolute path. A "fixed" bug appeared to come back twice because of this.

**Generated bindings churn.** Building rewrites `apps/*/src/bindings.rs`. Run
`git checkout apps/*/src/bindings.rs` before committing unless you actually
changed the WIT.

## Commits

Credited to Yashraj Pardeshi only. **Never** add AI co-author trailers or
"Generated with Claude Code". Plain language, no buzzwords, no em-dashes — use
" -- ".
