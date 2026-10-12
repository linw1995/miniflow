#!/usr/bin/env python3

from datetime import datetime, timedelta, timezone
import os
from urllib.parse import urlencode

from ci_monitor import GitHub, timestamp


FULL_TEST_JOBS = {"Full tests (ubuntu-latest)", "Full tests (macos-14)", "Full coverage"}


def should_run(api, branch, head_sha, now):
    week_ago = now - timedelta(days=7)
    query = urlencode({"branch": branch, "status": "success"})
    latest = None
    for run in api.pages(f"/actions/workflows/Nightly.yaml/runs?{query}", "workflow_runs"):
        started = timestamp(run["run_started_at"])
        if started <= week_ago or (latest is not None and started <= timestamp(latest["run_started_at"])):
            continue
        jobs = {job["name"] for job in api.jobs(run) if job["conclusion"] == "success"}
        # A workflow whose test jobs were skipped still has a success conclusion.
        if not FULL_TEST_JOBS.issubset(jobs):
            continue
        # Reruns retain their original creation time and can appear after newer runs.
        latest = run
    if latest is None:
        return True, "No successful full suite in the last week."
    if latest["head_sha"] != head_sha:
        return True, "The default branch has changes since the last successful full suite."
    recent = urlencode({"sha": branch, "since": (now - timedelta(days=1)).isoformat(), "per_page": 1})
    if api.request(f"/commits?{recent}"):
        return True, "The default branch has commits in the last 24 hours."
    return False, "No recent changes; a successful full suite ran within the last week."


def main():
    if os.environ["GITHUB_EVENT_NAME"] == "workflow_dispatch":
        run, reason = True, "Full suite requested manually."
    else:
        api = GitHub(os.environ["GITHUB_REPOSITORY"], os.environ["GH_TOKEN"], os.environ["GITHUB_API_URL"])
        run, reason = should_run(api, os.environ["DEFAULT_BRANCH"], os.environ["GITHUB_SHA"], datetime.now(timezone.utc))
    print(reason)
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"run={str(run).lower()}\n")
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as summary:
        summary.write(f"{reason}\n")


if __name__ == "__main__":
    main()
