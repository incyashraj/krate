# The evidence registry

Evidence says what happened. Claims say what Krate wants to state. They are
kept in two directories so one record can support a narrow sentence while
refusing a broad one, and so nothing about a claim's state is ever typed by
hand: `scripts/evidence-registry.py` computes it from the records.

```
records/E-*.json   append-only evidence records
claims/C-*.json    claim records, reviewed like code
```

```
python3 scripts/evidence-registry.py validate       # every file well formed
python3 scripts/evidence-registry.py state          # what the evidence supports
python3 scripts/evidence-registry.py card C-ID      # one claim, in sentences
python3 scripts/evidence-registry.py publication    # refuse an unsupported material claim
python3 scripts/evidence-registry.py export DIR     # public-safe JSON + Markdown
python3 scripts/evidence-registry.py --self-test
```

The contract these files implement is chapter 39 of the bible; the rules it
may never break are its first section. The short form:

- A record belongs to an exact subject: the source commit AND the tree,
  separately; for a binary, archive, installer or `.krate`, the binary digest
  AND the package digest, separately. One number never stands in for the other.
- A record names its environment kind -- `native`, `hosted`, `virtual` or
  `emulated` -- and the gate never promotes one to another. A virtual or
  emulated record also names the host architecture and the emulator.
- A record names its oracle and its class. A typecheck cannot satisfy an
  execution claim; a launch that was killed at a timeout is not a pass.
- A test that returned early is `skipped`. An ignored test is neither passed
  nor failed and stays visible beside every aggregate.
- Records are never deleted. A later record supersedes an earlier one by
  naming it. A failure stands until a passing record names it, and a pass
  that needed prerequisites the failure did not have cannot supersede it.
- Expiry is written down: a date, or a trigger such as `source-change`. A
  record past either is not evidence and says so.
- A claim names the platforms it is about. Evidence on one of them makes the
  claim OBSERVED there and untested elsewhere -- never PROVED. A sentence
  that says "every" or "all" is publishable only when it is PROVED.
- A corrected claim keeps its earlier wording, evidence and the reason it
  changed, in `history`; `revision` must match.
- An export replaces the fields a record lists under `privacy.redact` and
  says so; it withholds `internal` records and lists their ids; it refuses
  to write anything still shaped like a secret or a private path. It needs
  no account and no service.

## A record

```json
{
  "id": "E-2026-09-11-LOCAL-CLI-VALIDATOR-PARITY",
  "claims": ["C-VALIDATOR-PARITY"],
  "subject": {
    "kind": "source tree",
    "source": {"commit": "496cc9a15", "tree": "<git tree id>"},
    "binary_digest": null,
    "package_digest": null,
    "relationship": "built from this commit by cargo test on the named machine"
  },
  "environment": {"kind": "native", "os": "macos", "arch": "arm64", "version": "27.0"},
  "toolchain": {"rustc": "1.94.1", "cargo-component": "0.21.1"},
  "command": "cargo test -p krate-cli --test cli the_validator_and_the_runtime_agree",
  "inputs": {"fixtures": ["crates/bundle/tests/fixtures/minimal-run.wasm"], "prerequisites": []},
  "oracle": {"class": "execution",
             "description": "the packed component runs to exit 0; the refused one is refused by run, pack and publish with the same words"},
  "raw_result": {"exit": 0, "counts": {"passed": 1, "failed": 0, "ignored": 0}, "location": "<where the log is>"},
  "outcome": "pass",
  "scope": {"supports": "validator and runtime agree on this Mac at this commit",
            "exclusions": ["other operating systems", "the hub's admission path"]},
  "independence": "local reproduction",
  "time": {"start": "2026-09-11T22:10:00Z", "finish": "2026-09-11T22:10:02Z"},
  "operator": "lead workstation",
  "privacy": {"audience": "public", "redact": ["raw_result.location"]},
  "retention": {"raw": "the log path above", "until": "next release"},
  "expiry": {"triggers": ["source-change"], "on": null},
  "supersedes": null,
  "invalidated_by": null
}
```

Every field in the table in chapter 39 has a home here; the validator
requires the ones the rules above depend on and leaves the rest to the
record's author. Platform is `environment.os` + `-` + `environment.arch`.

## A claim

```json
{
  "id": "C-VALIDATOR-PARITY",
  "sentence": "A component Krate's validator refuses is refused by pack, run and publish with the same words.",
  "audience": "developers",
  "surfaces": ["docs"],
  "subject": {"commit": "496cc9a15"},
  "requires": {"evidence_classes": ["execution"],
               "platforms": ["macos-arm64", "ubuntu-x86_64", "windows-x86_64"],
               "environment": "any",
               "same_bytes": false},
  "evidence": ["E-2026-09-11-LOCAL-CLI-VALIDATOR-PARITY"],
  "declared_state": "OBSERVED",
  "exclusions": ["the hub's admission path", "components the SDK can build"],
  "owner": "lead",
  "approval": null,
  "expiry": {"triggers": ["source-change"]},
  "revision": 1,
  "history": []
}
```

`declared_state` is what the author believes. The gate computes the real
state and the card says both when they differ. `surfaces` of `website`,
`release` or `investor` are material: the `publication` command refuses a
build whose material claims are not carried by their evidence.

## Where records come from

`scripts/release-decision.py` writes one record per required CI lane every
time it runs, under `records/E-CI-<sha9>-<lane>.json`, with the lane's
conclusion as the outcome (`success` is a pass, `failure` a fail, `skipped`
is skipped, a lane that never ran is `blocked`). Re-running on the same commit
after a re-run of CI writes a new record that supersedes the old one; nothing
is overwritten. Hand-written records are for runs a person made; they follow
the same shape and the same rules.
