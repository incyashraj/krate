# Ask Paul Graham

A local search over Paul Graham's essays. You ask a question, it returns the
passages he actually wrote on that subject, each one verified against the file
on disk.

## Use it

```bash
./pg "how do you get startup ideas?"
./pg -n 8 "what makes someone do great work?"
```

## What this is, and what it is not

It **is** a retrieval tool. It finds real paragraphs and shows them to you with
the essay they came from.

It is **not** a model that imitates him. That distinction is the whole point.
A model trained to sound like Paul Graham produces sentences he never wrote,
in his voice, with no way for you to tell the difference. That is the opposite
of what you want if you care whether an answer is real.

So this tool never writes prose in his voice. Every line it shows you is a line
he wrote.

## The honesty guarantee

Each passage is checked against its source `.txt` file before it is printed.
If the text is not in the file character-for-character, it is not shown. This
is enforced in two places -- when the index is built, and again at question
time in `ask.py`.

What this guarantees: **every quote you see is real, and you can open the file
and confirm it.**

What it does not guarantee: that the *right* passage came back. Search can miss.
When the best match is weak, the tool says `[weak match]` rather than presenting
a poor result confidently. When nothing matches, it returns nothing instead of
guessing.

That is the honest version of "100% reliable" -- quotes are always real,
retrieval is sometimes imperfect, and it tells you which case you are in.

## Verify any quote yourself

```bash
grep -n "the phrase you want to check" text/*.txt
```

If it prints a line, he wrote it. If it prints nothing, he did not.

## Layout

```
Essays/
  raw/     original HTML, exactly as downloaded
  text/    cleaned plain text -- the source of truth for quotes
  index/   manifest.json (essay list) and search.pkl (the search index)
  fetch_essays.py   download everything
  build_index.py    build the search index
  ask.py            ask a question
  pg                shortcut for ask.py
```

## Rebuilding

```bash
python3 fetch_essays.py   # re-download (skips what it already has)
python3 build_index.py    # rebuild the index
```

## A note on the text

These essays are Paul Graham's copyrighted work, downloaded here for personal
reading and search. Keep this local. Quote him with attribution, as this tool
does, and link to paulgraham.com rather than redistributing the text.
