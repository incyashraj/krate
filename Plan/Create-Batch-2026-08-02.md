# Generating apps with AI: eight requests, measured

Eight plain-English requests through `krate create --agent claude`, chosen to
span the runtime rather than flatter it: arithmetic, a timer, secrets, a CLI,
audio, a file dialog, a database, and the network.

**Six of eight produced a working app. Both failures were caught by Krate's own
checks before anything shipped, and neither was the AI writing bad code.**

| App | Result | Verified by running it |
|---|---|---|
| tip-calculator | pass | $42.50 at 20% → tip $8.50, total $51.00 |
| unit-converter | pass | 5 km → 3.106856 mi; 100 kg → 220.462262 lb |
| password-vault | pass\* | saves and lists sites — see the asterisk below |
| metronome | pass | click at 135 bpm through the speakers |
| expense-tracker | pass | five expenses, running total 22.50, kept between runs |
| quote-fetcher | pass | one fetch, quote shown; scoped to `zenquotes.io:443` |
| pomodoro-timer | fail | our name-slugger, then a required-capability mismatch |
| photo-frame | fail | marked the file dialog required; it can start without one |

Times ranged 214–427 seconds. Every "pass" above was run, not assumed — the
column exists because a port earlier this week "passed" while being the
untouched template.

## The asterisk, and it matters more than the score

**password-vault works and stores passwords in the wrong place.** The request
said "securely". The app used `store.kv` — ordinary app data — when
`store.secret`, backed by the operating system's keychain, exists for exactly
this. It compiles, runs, passes the import check, passes the permission wall,
and quietly does the one thing its user asked it not to do.

No validator catches this. A capability check asks *may the app do this*, not
*is this the right way to do it*. What can be fixed is the contract, which
presented three stores as interchangeable and now says the choice is meaning:
kv for settings and lists, secret for anything a person would call a password,
sql for rows you filter or sum.

That is the honest edge of "generate any app": Krate can guarantee an app
cannot exceed its permissions. It cannot yet guarantee the app makes good
choices inside them.

## What the two failures were

Neither was the model's fault.

**pomodoro-timer** — "25 minute work sessions" made our own name-deriver
produce `pomodoro-timer-25`, and WIT package labels must begin with a
lowercase letter. The build died on "invalid label", and the error handler
then buried that under boilerplate about installing the wasm target. Both
fixed: a digit-led word ends the derived name, `--name 2048` is refused
instantly with a message naming the rule, and the toolchain hint only appears
when the compiler actually mentions the toolchain.

**photo-frame** — marked `ui.dialog:file-open` as `required = true`. The
verification run withholds a required capability and demands the app refuse to
start; a photo frame can open its window and wait without a dialog, so it
started, and failed its own wall test. The contract now says `required` means
cannot-start, not cannot-be-useful.

## What this says about betting on Krate

- **Safety is not the open question.** Every generated app was sandboxed, and
  the two failures were Krate refusing to ship something inconsistent. The
  quote fetcher asked for one host and one port rather than the internet.
- **Correctness of ordinary logic is good.** Tip maths, unit conversions, and
  running totals were right when checked by hand.
- **Judgment inside the sandbox is the gap.** The vault is the example. The
  contract can nudge; it cannot decide.

The measurement to repeat: same eight requests after the contract changes, and
whether the vault reaches for the keychain unprompted.
