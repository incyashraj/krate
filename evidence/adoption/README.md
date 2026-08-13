# The adoption record

`history.tsv` is one row per snapshot, appended by
`scripts/record-adoption.sh` and by a workflow every Monday. It exists
because Cloudflare's Analytics Engine keeps 90 days and a dashboard only ever
shows what is true now. Outreach needs the shape of the line over months --
what the numbers were when the first conversations started, and what they are
now -- and that only exists if something writes it down on a schedule.

Run it by hand before a conversation where the current number matters:

```bash
scripts/record-adoption.sh --print    # show, write nothing
scripts/record-adoption.sh            # append today's row
```

## The columns

| column | what it counts |
|---|---|
| `date` | the day the snapshot was taken, not the day the events happened |
| `source` | `live` = Analytics Engine, the current numbers. `kv-only` = the frozen KV history from 05-09 August, which means the live query failed. **Check this before quoting anything.** |
| `views` | krate.tech page loads. Started 2026-08-13; zero before that means not counted, not nobody. |
| `installs` | first run on a machine, not downloads |
| `makes` | apps authored |
| `opens` | apps opened |
| `publishes` | apps published to the hub |
| `open_failed` | opens that did not end in a running app |
| `distinct_installs` | unique machines over the window |

## Two numbers that mislead if quoted raw

**`open_failed` is not a defect rate.** As of 2026-08-14, over 30 days:

```
-              69.7%   clients older than v0.1.12, which sent no reason
refused        15.6%   the permission wall turning an app away
app-failed      4.4%
not-found       4.4%
bad-bundle      3.5%
other           2.4%
```

`refused` is the product working exactly as designed. Counting it as a
failure is how the rate ended up looking alarming and unactionable at the
same time (K-100). Exclude it. The 69.7% blank is old clients; that share
should shrink now v0.1.13 is out, and if it does not, the reason plumbing is
broken and worth checking.

Live breakdown: `curl -s https://hub.krate.tech/stats | jq
.open_failure_reasons_30d`

**`installs` counts first runs, not downloads.** Deliberately: a download
nobody runs is not adoption. It means the real top of the funnel is wider
than this number, and the gap between download and first run is not measured
at all.

## What this cannot tell you

- **Where visitors came from.** No referrer is collected, on purpose. Views
  answer "how many", never "who" or "from where".
- **Whether the same person installed twice.** The id is random per machine
  and resets if the file is deleted.
- **Anything before 2026-08-05**, which is when counting started, or before
  2026-08-13 for views.

## Honesty notes for outreach

The gap between `opens` and `makes` is the real shape of the product today:
thousands of opens against a handful of authored apps. Most opens are CI and
development traffic on our own machines, not strangers. Say that plainly
rather than letting the open count carry an implication it cannot support.
