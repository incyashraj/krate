# The authoring conversation: ask before building

Written 2026-08-17, from the founder's report of a friend's first real
session. This is the plan of record for the gap it exposed.

## What happened, exactly

A first-time user pasted a prompt written for ChatGPT into Krate Studio.
The prompt assumed things ChatGPT had and we did not: it ran "in a folder"
with the Excel and document files the app was supposed to be built around.
The Studio accepted the prompt silently, built without the data, and the
result missed the point. Separately, the founder typed "Sadas" -- three
characters of nothing -- and the pipeline dutifully built and named an app
after it. It is on the public store today.

Both failures are one failure: **the request is treated as an order when
it is actually the opening line of a conversation.** Every chat AI a user
has ever met asks a question when the request is thin, and says what it is
about to do before doing it. We compare ourselves to compilers; users
compare us to ChatGPT.

## The design

One new engine door and one Studio behavior.

### 1. `krate plan "<request>" [--attach FILE]...` (engine)

A short, non-building agent call. Input: the request and any attachments.
Output, as JSON on stdout, exactly one of:

- `{"ask": ["question", ...]}` -- up to three short questions, only when
  answering them would change what gets built. A thin or unintelligible
  request ("Sadas") always lands here ("What should this app do?").
- `{"plan": "...", "needs": ["..."]}` -- one paragraph in plain words:
  what will be built, what screens it has, what data it works on, and
  `needs`: things the person must supply (an attached file, an API choice,
  a permission it will request). This is what ChatGPT does with "here's
  what I'll make" and it is where "this mentions an Excel file -- attach
  it and I'll use it" happens naturally.

Fast by construction: one agent call, no build, target under 30 seconds.

### 2. The Studio converses (studio)

Send no longer goes straight to create. The flow becomes:

    person: <request>
    KRATE:  <questions>            (when asked)
    person: <answers>
    KRATE:  <the plan + needs>     always, one message
            [Build it] [Change something]

Build passes the full conversation (request + answers + plan) to create as
the enriched request. "Change something" keeps talking. The gate is one
extra click on the happy path and it is the difference between a partner
and a vending machine.

The CLI keeps direct `create` for people who script it; the Studio always
plans first.

### 3. Data files, both halves

- **Authoring-time**: attachments already flow to the agent. The plan step
  makes them discoverable by asking for them when the request implies
  them. For tabular files (xlsx), the authoring pipeline converts to CSV
  at build time so guests stay no_std -- the app embeds or copies the
  converted data, and the pack teaches this pattern.
- **Runtime**: an app that keeps reading the person's files uses the
  folder picker (pick-is-the-grant), never a baked path. Already shipped;
  the pack already teaches it; the plan's `needs` list is where the app
  says so up front.

## Stages

- S1: `krate plan` engine door with the JSON contract, tested against the
  "Sadas" case (must ask, never build) and a rich request (must plan).
- S2: Studio conversation UI on top of it; enriched-request handoff to
  create; measure completion rate vs today.
- S3: xlsx/csv attachment conversion in the authoring pipeline.
- S4: the pack's own conversation guidance: generated apps that work on
  personal data must name it in the consent wall in plain words.

## Acceptance

- "Sadas" never becomes an app; it becomes a question.
- The friend's pasted ChatGPT prompt produces, before any build, a plan
  that asks for the Excel file it mentions.
- The happy path costs one extra click and under 30 seconds.
