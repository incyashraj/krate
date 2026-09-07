# ADR-0016: Security feed refresh, before there is a feed

**Status:** Accepted
**Date:** 2026-09-07
**Authors:** @incyashraj
**Supersedes:** -
**Superseded by:** -

---

## Context

Krate will eventually need to learn that a bundle it can open has been found
malicious, or that a runtime version must stop being trusted. That means a
security feed: a small signed record the runtime refreshes and consults.

There is no such feed today. Nothing in the runtime fetches one and nothing
consults one, which is exactly why this decision is being written now.

The register (IC-096) names what a careless implementation would do, and none
of it is hypothetical -- every item is a shape that has shipped in real
products:

- query the feed on every app open, turning a launch into a network round trip
- send the app's digest with that query, so opening a file tells a server which
  file you opened
- drain a battery or a metered connection because refresh was tuned on a
  desktop with mains power and unlimited data
- ignore an organisation's policy about what may reach the network
- leave an urgent revocation undelivered for days because the interval was
  chosen for tidiness rather than measured against time-to-protection

The temptation is to defer all of this until the feed exists. That is how the
mistakes get made: by the time somebody is writing the fetch, the interval is
a guess in a hurry and the digest is already in the query string because it
was convenient.

## Decision

**No per-app request, ever.** Opening an app must never cause a network
request that identifies that app. Which apps a person opens is theirs. The
feed is fetched on its own schedule, as one compact record covering
everything, and consulted locally.

**Refresh at eligible runtime startup and through OS-permitted background
work.** Not on app open. "Eligible" means the platform's own rules are
respected first: battery saver, metered network, and an organisation policy
that forbids the request each mean the refresh does not happen.

**Last verified state on failure.** Offline, timed out, corrupt, or refused by
policy, the runtime uses the last record it verified, and says how old it is
when asked. It never fails open into "no restrictions known" and never blocks
an app because the feed could not be reached.

**Intervals are measured, not chosen.** Ordinary and urgent refresh intervals
are set from measurements on every supported OS -- feed size, bandwidth,
battery cost, delivery reliability, and time-to-protection -- not from a round
number that looks reasonable. Until those measurements exist, no interval is
published and no protection claim is made.

**No claim before evidence.** Krate does not say it revokes malicious apps, or
state any time-to-protection, until the feed exists and the measurements are
retained. The claim record (`evidence/claims/performance.json`) is where such a
statement would have to be bound, the same as any other public number.

## Consequences

Anyone implementing the feed has the constraints in front of them rather than
discovering them in review. The one that costs real design effort is the first:
a per-app query is the obvious implementation and it is forbidden, so the feed
has to be compact enough to ship whole and consult locally.

Deferring an urgent revocation is the risk this accepts. A feed refreshed on
its own schedule reaches a machine later than one queried on every open. That
is the trade: a launch that tells a server nothing, against a revocation that
may arrive hours late. Measured urgent intervals are how that gap is made
small, which is why the measurements are a precondition rather than a
follow-up.

Until this is built, Krate's honest position is that it has no revocation
mechanism at all -- which is what the permission wall is for, and why the wall
is the claim we make.
