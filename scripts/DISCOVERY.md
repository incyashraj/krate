# Read-only discovery reporting

Use this beside the existing `adoption-report.sh` and `record-adoption.sh`.
The existing adoption history stays the long-term aggregate record. This tool
preserves GitHub's expiring daily data and keeps overlapping windows separate.
It does not install tracking, collect new runtime events or schedule anything.

Requirements: Python 3, curl with valid system certificates, and `gh`
authenticated with read access to the repository's traffic data.

Save locally, outside version control:

```sh
python3 scripts/discovery-report.py capture --output-dir /path/to/private/discovery
python3 scripts/discovery-report.py report /path/to/private/discovery/*.json
python3 scripts/discovery-report.py report /path/to/private/discovery/*.json --csv
python3 scripts/discovery-report.py --self-test
```

Capture queries only GitHub repository metadata, traffic, popular paths and
referrers, plus the existing public hub `/stats` endpoint. It never requests
issue content, emails, source files or credentials. Each snapshot is dated and
never overwritten. Unavailable endpoints are recorded as unavailable and the
capture command exits 2, while preserving successful reads.

The report uses the newest successful observation for each source/day, rather
than adding the same day from multiple rolling windows. It preserves zeros,
leaves unknowns unknown and dates retained observations. Do not sum daily
unique counts into a unique-person count. Today's observation can be partial.

GitHub clones and hub events include CI, development and bots. They are not
confirmed users. This tool does not import Google/Bing reports. Keep those
account observations and their date ranges separately, even if already read
in a browser. Outside-developer activations need direct confirmation.

No cron job, workflow, telemetry collection or paid service is added.
