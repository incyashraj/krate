# What Krate does today with a request it cannot serve

Date: 2026-08-05. Machine: M-series Mac. Binary: `target/release/krate` built
from this worktree and invoked by absolute path -- a `krate` on PATH is an older
installed release and would have measured the wrong thing.

This file records what actually happened, measured before any refusal code was
written, so the design that follows argues from evidence rather than from a
guess. `FINDINGS.md` predicted this behaviour but explicitly did not measure it:
the impossible requests sit at the end of the corpus (96-100) and the account
ran out of quota before reaching them.

Quota was checked first this time. A one-token probe returned
`"rate_limit_info":{"status":"allowed"}`, so nothing here is a rate-limit
artefact.

## Method

Five requests that Krate genuinely cannot serve, each run through the real
authoring loop:

    krate create "<request>" --agent claude --output app.krate \
      --work-dir <dir> --transcript <file> --yes

For each one: did it produce a `.krate`, how long did it take, and -- the part
an exit code cannot tell you -- what is actually in the generated `src/lib.rs`.

## The headline

**Nothing stops an impossible request. The loop treats it exactly like a
possible one and spends the full authoring budget on it.**

The first request alone ran for over ten minutes of continuous agent work,
producing 1511 lines of Rust, and at no point did anything -- not
`validate_create_request`, not the prompt, not any of the six `check-app`
stages -- ask whether the app it was building could ever do what was asked.

## Case 1: "download my email and show me the unread ones"

The predicted case, and the one in the plan.

**What it did:** wrote a complete 1511-line mail client. Nine distinct
work steps recorded in the progress output -- read the API reference, write the
code, set up the build, declare capabilities, check, then loop back and do it
again twice more. No refusal, no hesitation, no question about feasibility.

**What is in the source:** an inbox list with sender, subject, a time-ago
stamp and an unread dot, plus a reading pane, plus a persisted read/unread
flag. The data is a hardcoded constant:

    /// A built-in sample mailbox, in exactly the JSON a mail endpoint returns
    const SAMPLE: &str = r#"[
    {"from":"Priya Raghunathan","subject":"Re: Thursday's design review", ...
    {"from":"GitHub","subject":"[krate/krate] CI passed on main", ...
    {"from":"Tomas Lindqvist","subject":"Invoice for August", ...

Seven invented messages with realistic names, subjects and bodies.

**The manifest asks for this:**

    [[capabilities]]
    cap = "net.connect:127.0.0.1:*"
    rationale = "Download messages from a mail endpoint over HTTP"

That is the tell. The app requests permission to reach a "mail endpoint" on
localhost -- a service that does not exist on anybody's machine. The
capability is real, the permission wall around it works, and it protects
access to nothing.

**One thing the agent did better than predicted, and it should be recorded
honestly:** it did not claim the sample data was real mail. The on-screen
status line reads

    Built-in sample mailbox - pass your mail endpoint URL to download real mail.

and the source header says the sample is "clearly-labelled" and that the app
"never claims a fetch that did not happen". So the failure here is not a lie
on screen. It is that a person asked for their unread email, waited ten
minutes, and received a mail-shaped app showing seven strangers' invented
messages, with an offer to point it at a mail server they do not run.

`FINDINGS.md` predicted "a plausible mail-reader UI over fake local state that
builds, runs, and exits 0". That is right in every mechanical respect. The
nuance worth carrying forward is that a good agent labels its demo data, which
narrows the harm but does not remove it -- and cannot be relied on, since
nothing enforces it.

**What the person is handed.** `krate create` finished with exit 0 and printed:

    Created .../app.krate
      requested access:
        - ui.window:create
        - io.stdout
        - io.args
        - net.connect:127.0.0.1:*
        - store.kv

    Send .../app.krate to someone; they can double-click it to open it.

A 21 KB `.krate`, and an invitation to send it to a friend. Nothing in that
output hints the app cannot read anyone's mail.

**What it looks like on screen.** `krate run --shoot` renders
`email/frame.png`: a polished two-pane mail client, headed **"Inbox / 5 unread
messages"**, listing Priya Raghunathan, GitHub, Tomas Lindqvist, Anna
Whitfield and Krate Digest, with the first message open in a reading pane and
a green "Mark read" button.

The honest disclaimer -- "Built-in sample mailbox - pass your mail endpoint URL
to download real mail" -- is one dim line in the far bottom-left corner, below
a large empty area, in the smallest type on the screen. It is true, and it is
the last thing anyone would read. The screenshot is the strongest single
argument for a refusal: no amount of source-code honesty fixes a window that
says "5 unread messages" over five invented ones.

## Cases 2-5: measured against the fix, by accident

The remaining four requests were queued behind the first, and by the time they
ran the release binary had been rebuilt with the refusal screen in it. That
was not planned, but it turned into the cleanest possible A/B on one machine
in one session, on the same corpus:

| Request | Binary | Result | Time |
|---|---|---|---|
| download my email and show me the unread ones | before | built a 1511-line mail client over invented data | **673 s** |
| a chat app to message my friends | after | refused, named the limit | **0 s** |
| back up my photos to the cloud | after | refused, named the limit | **0 s** |
| a Spotify client | after | refused, named the limit | **0 s** |
| show me my calendar from Google | after | refused, named the limit | **0 s** |

The four refusals, verbatim from stderr:

- **chat:** "there is no Krate server and no way for two Krate apps to find
  each other, so an app on your computer cannot reach another person's device.
  Try instead: a two-player app on this one computer, or an app that reads and
  writes a file you share yourself."
- **photos:** "a Krate app runs in a sandbox and cannot read the apps or
  libraries already on your computer, so it cannot get at your real mail,
  photos, contacts, or messages. Try instead: an app that works on files you
  pick yourself, or on data you type in."
- **spotify** and **calendar:** "a Krate app cannot sign in to another
  company's account for you: there is no login flow, no browser to redirect
  through, and nowhere safe to keep the token. Try instead: an app that works
  on a file you export from that service, or on data you paste in."

Each also ends with the way out: "If you think this is wrong, re-run with
--force and Krate will build what it can."

**673 seconds and a wrong app, versus 0 seconds and a true sentence.** That is
the whole case for the refusal path, measured rather than argued.

## What this proves about the code

Three specific things, all confirmed by reading the source rather than
inferring:

1. `validate_create_request` (`crates/cli/src/main.rs`) rejects only requests
   under 3 characters. "download my email and show me the unread ones" clears
   that bar as easily as "a tip calculator".

2. `claude_author_prompt` says *"Do not stop until `check-app` prints OK. That
   is the whole definition of done"* and offers the agent no way to say the
   request cannot be served. An agent that wanted to refuse had no channel for
   it.

3. The only guard on a wrong app is `if lib_after == starter_lib`, a
   byte-identical comparison against the blank skeleton. It catches an agent
   that wrote nothing. It cannot catch an agent that wrote 1511 correct,
   compiling, well-labelled lines of the wrong app.

None of the six `check-app` stages -- layout, manifest, build, imports, run,
shoot -- takes the request as an input. They cannot tell a good app from a
convincing wrong one, and were never meant to.

## Why this matters more under MCP

A person who typed `krate create "download my email"` remembers what they
asked for and will notice the sample mailbox. A model driving the MCP server
sees six green stages and a `.krate` on disk, and tells its user their email
app is ready. The exit code says success in both cases.

## The other half: proving the fix refuses nothing it should build

A refusal path is only worth having if it never fires on a request Krate could
have served. A false refusal is worse than today's behaviour, because it makes
the product look incapable and the person cannot argue with it.

22 requests were run through the real release binary, using `--json
--no-install` so the screen is exercised without a build. The split was exact:

**19 passed the screen -- every one of them.** Including the near-misses that
sound impossible:

    a chat UI mockup with fake conversations
    an inbox-style list UI with fake messages
    an email client mockup showing sample messages
    a spotify-style player UI with sample tracks
    a photo gallery for images in a folder I pick
    a music player for MP3 files I choose
    a monthly calendar I can add my own events to
    a two-player chess game on the same computer
    a weather app that fetches from a URL I give it
    a local-only note app

and the ordinary ones (tip calculator, to-do list, snake, expense tracker,
markdown preview), and the vague/too-big ones the corpus lists beside the
impossible block (`something for my mornings`, `a spreadsheet`, `a web
browser`, `a full email client`) -- which are hard, not impossible, and must
be attempted.

**3 were refused, all correctly:** "download my email and show me the unread
ones", "a chat app so I can message my friends", "sync my files to my phone".

Note the pairs. `a full email client` builds; `download my email` refuses. `a
spotify-style player UI with sample tracks` builds; `a Spotify client`
refuses. `a chat UI mockup` builds; `message my friends` refuses. The screen
is matching the impossible action, not the topic -- which is the whole design.

In unit tests the same guarantee is locked down against all 84 buildable
requests in `corpus.txt` (`refuses_nothing_in_the_buildable_corpus`), so a
future rule that starts over-refusing fails the build.

## The honest-output case

`show me the live weather from the internet` sits in the corpus's "impossible"
block, but it is **not** impossible: Krate ships TLS and reaches any granted
`net.connect:host:port`. Refusing it would be a false refusal.

It is not refused. It builds, and `krate create` prints, with the finished app:

    One thing to know: this app can only reach the internet if you name a host
    for it and grant that permission, so unless you asked for a specific
    address it will show built-in example data rather than live figures.

Name a host -- "a weather app that fetches from a URL I give it" -- and the
note disappears, because the data will be real.
