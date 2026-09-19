#!/usr/bin/env python3
"""Fail-closed gate for refreshing Pages after the Release workflow.

Only public GitHub metadata is read. No release artifacts, code, or credentials
from the triggering run are downloaded. Manual releases run on main, so their
head SHA is not necessarily the released tag's SHA: inspect the promotion job
and stable release instead. --self-test is offline and needs only Python.
"""
import argparse
from copy import deepcopy
from datetime import datetime
import fnmatch
import json
import os
from pathlib import Path
import re
import unittest
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parent.parent
STABLE_TAG = re.compile(r"v[0-9]+\.[0-9]+\.[0-9]+")


def public_json(path):
    request = Request("https://api.github.com/" + path, headers={
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "Krate-Pages-release-check",
    })
    with urlopen(request, timeout=20) as response:
        return json.load(response)


def timestamp(value):
    return datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ")


def should_publish(event_name, event, repository, fetch=public_json):
    if event_name in ("push", "workflow_dispatch"):
        return True, "ordinary website build"
    if event_name != "workflow_run":
        return False, "not a website or release-completion event"
    run = event.get("workflow_run", {})
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        return False, "invalid repository"
    if (event.get("action") != "completed"
            or run.get("status") != "completed"
            or run.get("conclusion") != "success"
            or run.get("name") != "Release"
            or run.get("path") != ".github/workflows/release.yml"
            or run.get("event") not in ("push", "workflow_dispatch")
            or run.get("repository", {}).get("full_name") != repository
            or run.get("head_repository", {}).get("full_name") != repository
            or type(run.get("id")) is not int):
        return False, "not a successful trusted Release completion"

    # Public GETs need no Actions permission or bearer token. API/network
    # errors propagate and fail the gate, never silently allow deployment.
    jobs = []
    for page in range(1, 101):
        batch = fetch(f"repos/{repository}/actions/runs/{run['id']}/jobs"
                      f"?per_page=100&page={page}")["jobs"]
        jobs.extend(batch)
        if len(batch) < 100:
            break
    else:
        raise ValueError("Release job pagination did not terminate")
    promotions = [job for job in jobs if job.get("name") == "Promote to stable"
                  and job.get("status") == "completed"
                  and job.get("conclusion") == "success"]
    if len(promotions) != 1:
        return False, "no successful stable promotion (for example an rc run)"
    release = fetch(f"repos/{repository}/releases/latest")
    tag = release.get("tag_name", "")
    if (release.get("draft") is not False
            or release.get("prerelease") is not False
            or not STABLE_TAG.fullmatch(tag)):
        return False, "latest release is not a published stable version"
    if run["event"] == "push" and run.get("head_branch") != tag:
        return False, "the triggering tag is not the current stable release"
    # A CHANNEL pin exits the promotion job successfully without changing
    # the stable release. Reject that no-op. This also rejects an unrelated
    # promotion that finished while this run was queued. No clock tolerance:
    # both timestamps come from GitHub, and ambiguous cases fail closed.
    promotion = promotions[0]
    if not (timestamp(promotion["started_at"])
            <= timestamp(release["updated_at"])
            <= timestamp(promotion["completed_at"])):
        return False, "stable release was not updated during this promotion"
    return True, f"successful stable promotion of {tag}"


class ReleasePolicyTests(unittest.TestCase):
    def setUp(self):
        self.repo = "incyashraj/krate"
        self.event = {"action": "completed", "workflow_run": {
            "id": 123, "name": "Release", "path": ".github/workflows/release.yml",
            "status": "completed", "conclusion": "success", "event": "push",
            "head_branch": "v0.5.0", "repository": {"full_name": self.repo},
            "head_repository": {"full_name": self.repo},
        }}
        self.jobs = [{"name": "Promote to stable", "status": "completed",
                      "conclusion": "success", "started_at": "2026-09-19T13:19:59Z",
                      "completed_at": "2026-09-19T13:20:07Z"}]
        self.release = {"tag_name": "v0.5.0", "draft": False, "prerelease": False,
                        "updated_at": "2026-09-19T13:20:05Z"}

    def fetch(self, path):
        return self.release if path.endswith("/latest") else {"jobs": self.jobs}

    def allowed(self):
        return should_publish("workflow_run", self.event, self.repo, self.fetch)[0]

    def test_successful_tag_and_manual_promotion(self):
        self.assertTrue(self.allowed())
        self.event["workflow_run"].update(event="workflow_dispatch", head_branch="main")
        self.assertTrue(self.allowed())

    def test_failed_cancelled_pending_or_untrusted_runs_never_fetch(self):
        original = deepcopy(self.event)
        for field, value in (("conclusion", "failure"), ("conclusion", "cancelled"),
                             ("status", "in_progress"), ("event", "pull_request"),
                             ("name", "Fake"), ("path", ".github/workflows/fake.yml"),
                             ("head_repository", {"full_name": "someone/krate"}),
                             ("repository", {"full_name": "someone/krate"})):
            self.event = deepcopy(original)
            self.event["workflow_run"][field] = value
            with self.subTest(field=field, value=value):
                def forbidden(_):
                    self.fail("rejected event should not read APIs")
                self.assertFalse(should_publish("workflow_run", self.event,
                                                self.repo, forbidden)[0])

    def test_prerelease_or_skipped_promotion_rejected(self):
        for conclusion in ("skipped", "failure", "cancelled"):
            self.jobs[0]["conclusion"] = conclusion
            self.assertFalse(self.allowed())
        self.jobs[0]["conclusion"] = "success"
        self.release["prerelease"] = True
        self.assertFalse(self.allowed())
        self.release["prerelease"] = False
        self.release["tag_name"] = "v0.6.0-rc1"
        self.assertFalse(self.allowed())

    def test_draft_missing_metadata_and_other_tag_rejected(self):
        self.release["draft"] = True
        self.assertFalse(self.allowed())
        del self.release["draft"]
        self.assertFalse(self.allowed())
        self.release["draft"] = False
        self.event["workflow_run"]["head_branch"] = "v0.6.0"
        self.assertFalse(self.allowed())

    def test_pin_held_or_superseded_promotion_rejected(self):
        for updated in ("2026-09-18T13:20:05Z", "2026-09-19T13:20:08Z"):
            self.release["updated_at"] = updated
            self.assertFalse(self.allowed())

    def test_network_failure_does_not_allow_build(self):
        def unavailable(_):
            raise OSError("API unavailable")
        with self.assertRaises(OSError):
            should_publish("workflow_run", self.event, self.repo, unavailable)

    def test_missing_or_invalid_timestamps_fail_closed(self):
        for value in ("not-a-date", "2026-09-19", None):
            self.release["updated_at"] = value
            with self.subTest(value=value), self.assertRaises((ValueError, TypeError)):
                self.allowed()
        del self.release["updated_at"]
        with self.assertRaises(KeyError):
            self.allowed()

    def test_jobs_pagination(self):
        paths = []
        def paginated(path):
            paths.append(path)
            if path.endswith("page=1"):
                return {"jobs": [{"name": "Build"}] * 100}
            return self.fetch(path)
        self.assertTrue(should_publish("workflow_run", self.event,
                                       self.repo, paginated)[0])
        self.assertTrue(any(path.endswith("page=2") for path in paths))

    def test_ordinary_push_and_manual_build_need_no_api(self):
        for event_name in ("push", "workflow_dispatch"):
            self.assertTrue(should_publish(event_name, {}, self.repo, None)[0])
        self.assertFalse(should_publish("pull_request", {}, self.repo, None)[0])

    def test_workflow_wiring_and_actual_build_input_filters(self):
        workflow = (ROOT / ".github/workflows/pages.yml").read_text()
        self.assertIn("workflows: [Release]\n    types: [completed]", workflow)
        self.assertEqual(workflow.count("github.event_name == 'workflow_run' && 'main' || github.sha"), 2)
        self.assertNotIn("ref: ${{ github.event.workflow_run.head_", workflow)
        self.assertIn("needs: release-context", workflow)
        self.assertIn("if: ${{ needs.release-context.outputs.publish == 'true' }}", workflow)
        self.assertIn("KRATE_PUBLIC_RELEASE: ${{ needs.release-context.outputs.version }}", workflow)
        context, build = workflow.split("  release-context:", 1)[1].split("\n  build:", 1)
        self.assertIn("permissions:\n      contents: read\n", context)
        self.assertEqual(context.count("GH_TOKEN:"), 1)
        self.assertNotIn("GH_TOKEN:", build)
        self.assertIn("if: ${{ steps.context.outputs.publish == 'true' }}", context)
        self.assertIn("persist-credentials: false", context)
        self.assertIn("group: \"pages\"\n  cancel-in-progress: false", workflow)
        self.assertIn("permissions:\n  contents: read\n  pages: write\n  id-token: write", workflow)
        paths = re.findall(r'^      - "([^"]+)"$', workflow, re.M)
        for source in ("wit/krate/phase4/world.wit", "apps/krate-checklist/src/lib.rs",
                       "scripts/pages-release.py", "crates/cli/build.rs"):
            self.assertTrue(any(fnmatch.fnmatchcase(source, glob) for glob in paths), source)
        fast = (ROOT / ".github/workflows/site-checks.yml").read_text()
        self.assertEqual(fast.count('- "scripts/pages-release.py"'), 2)
        self.assertIn("run: python3 scripts/pages-release.py --self-test", fast)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        unittest.main(argv=[__file__])
        return
    try:
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
        allowed, reason = should_publish(os.environ["GITHUB_EVENT_NAME"], event,
                                         os.environ["GITHUB_REPOSITORY"])
    except (OSError, ValueError, TypeError, KeyError) as error:
        parser.exit(1, f"Pages refresh refused: could not verify release metadata: {error}\n")
    print(f"Pages refresh: {'allowed' if allowed else 'skipped'}: {reason}")
    with Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        output.write(f"publish={str(allowed).lower()}\n")


if __name__ == "__main__":
    main()
