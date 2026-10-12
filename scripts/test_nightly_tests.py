from datetime import datetime, timedelta, timezone
import unittest
from urllib.parse import parse_qs, urlparse

import nightly_tests as nightly


NOW = datetime(2026, 10, 12, 18, 23, tzinfo=timezone.utc)


def run(identifier=1, age=timedelta(days=1), sha="head"):
    return {
        "id": identifier,
        "run_attempt": 1,
        "run_started_at": (NOW - age).isoformat(),
        "head_sha": sha,
    }


class FakeGitHub:
    def __init__(self, runs=(), jobs=None, commits=()):
        self.runs = runs
        self.run_jobs = jobs or {}
        self.commits = commits
        self.requests = []

    def pages(self, path, key):
        self.requests.append(path)
        query = parse_qs(urlparse(path).query)
        if "created" in query:
            cutoff = nightly.timestamp(query["created"][0].removeprefix(">="))
            return (item for item in self.runs if nightly.timestamp(item.get("created_at", item["run_started_at"])) >= cutoff)
        return iter(self.runs)

    def jobs(self, item):
        return self.run_jobs.get(item["id"], [
            {"name": name, "conclusion": "success"} for name in nightly.FULL_TEST_JOBS
        ])

    def request(self, path):
        self.requests.append(path)
        return self.commits


class NightlyTests(unittest.TestCase):
    def test_first_run_and_missing_recent_success_run_full_suite(self):
        self.assertTrue(nightly.should_run(FakeGitHub(), "main", "head", NOW)[0])

    def test_recent_commits_run_even_if_head_was_already_validated(self):
        api = FakeGitHub([run()], commits=[{"sha": "head"}])
        self.assertTrue(nightly.should_run(api, "main", "head", NOW)[0])
        query = parse_qs(urlparse(api.requests[-1]).query)
        self.assertEqual(query["since"], [(NOW - timedelta(days=1)).isoformat()])
        self.assertEqual(query["sha"], ["main"])

    def test_quiet_branch_skips_with_recent_successful_full_suite(self):
        self.assertFalse(nightly.should_run(FakeGitHub([run()]), "main", "head", NOW)[0])

    def test_week_boundary_runs_full_suite(self):
        api = FakeGitHub([run(age=timedelta(days=7))])
        self.assertTrue(nightly.should_run(api, "main", "head", NOW)[0])

    def test_new_head_retries_even_after_commit_ages_out_of_daily_window(self):
        api = FakeGitHub([run(sha="old")])
        self.assertTrue(nightly.should_run(api, "main", "head", NOW)[0])

    def test_skipped_and_partial_suites_do_not_reset_the_weekly_deadline(self):
        for conclusion in ["skipped", "failure", "cancelled"]:
            with self.subTest(conclusion=conclusion):
                jobs = [
                    {"name": name, "conclusion": conclusion if name == "Full coverage" else "success"}
                    for name in nightly.FULL_TEST_JOBS
                ]
                api = FakeGitHub([run()], jobs={1: jobs})
                self.assertTrue(nightly.should_run(api, "main", "head", NOW)[0])

    def test_skipped_daily_run_does_not_hide_earlier_full_success(self):
        api = FakeGitHub([run(2), run(1, age=timedelta(days=3))], jobs={2: []})
        self.assertFalse(nightly.should_run(api, "main", "head", NOW)[0])

    def test_recent_rerun_counts_even_if_workflow_was_created_weeks_ago(self):
        item = run()
        item["created_at"] = (NOW - timedelta(days=20)).isoformat()
        item["run_attempt"] = 2
        self.assertFalse(nightly.should_run(FakeGitHub([item]), "main", "head", NOW)[0])

    def test_latest_success_uses_attempt_time_instead_of_creation_order(self):
        api = FakeGitHub([run(2, age=timedelta(days=3), sha="old"), run(1)])
        self.assertFalse(nightly.should_run(api, "main", "head", NOW)[0])


if __name__ == "__main__":
    unittest.main()
