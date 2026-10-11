import unittest

import ci_monitor as monitor


def job(name, start=10, end=70, conclusion="success"):
    return {
        "name": name,
        "started_at": f"2026-10-11T00:{start // 60:02d}:{start % 60:02d}Z",
        "completed_at": f"2026-10-11T00:{end // 60:02d}:{end % 60:02d}Z",
        "conclusion": conclusion,
        "steps": [{
            "name": "Build",
            "started_at": "2026-10-11T00:00:20Z",
            "completed_at": "2026-10-11T00:01:00Z",
            "conclusion": conclusion,
        }],
    }


def run(run_id=10, created="2026-10-11T01:00:00Z", updated="2026-10-11T02:00:00Z", conclusion="success"):
    return {
        "id": run_id,
        "workflow_id": 7,
        "run_number": run_id,
        "run_attempt": 1,
        "created_at": created,
        "updated_at": updated,
        "conclusion": conclusion,
        "html_url": f"https://github.com/owner/repo/actions/runs/{run_id}",
        "head_sha": "a" * 40,
    }


class FakeGitHub:
    def __init__(self, runs=(), jobs=None, pulls=None, comments=()):
        self.runs = list(runs)
        self.run_jobs = jobs or {}
        self.pulls = pulls or {}
        self.comments = list(comments)
        self.writes = []

    def pages(self, path, key=None, max_pages=10):
        if path.startswith("/actions/workflows/"):
            return iter(self.runs)
        if path.startswith("/commits/"):
            return iter(self.pulls.values())
        return iter(self.comments)

    def jobs(self, candidate):
        return self.run_jobs[candidate["id"]]

    def request(self, path, method="GET", body=None):
        if method != "GET":
            self.writes.append((path, method, body))
            return {}
        return self.pulls[int(path.rsplit("/", 1)[1])]


class TimingTests(unittest.TestCase):
    def test_rerun_does_not_include_time_since_the_original_attempt(self):
        current = run(created="2026-10-10T00:00:00Z")
        current.update(run_attempt=2, run_started_at="2026-10-11T00:00:00Z")
        self.assertEqual(monitor.wall_time(current, {"lint": job("lint")}), 70)

    def test_wall_time_uses_matching_jobs_and_includes_queueing(self):
        current = run(created="2026-10-11T00:00:00Z")
        jobs = monitor.timings([job("lint"), job("coverage", 90, 210), job("skipped", conclusion="skipped")])
        self.assertEqual(monitor.wall_time(current, jobs), 210)
        self.assertEqual(monitor.duration(jobs["coverage"]), 120)
        self.assertNotIn("skipped", jobs)

    def test_baseline_excludes_future_incomplete_and_extra_macos_jobs(self):
        candidates = [
            run(1, updated="2026-10-11T03:00:00Z"),
            run(2, updated="2026-10-11T00:20:00Z"),
            run(3, created="2026-10-11T00:00:00Z", updated="2026-10-11T00:30:00Z"),
        ]
        api = FakeGitHub(candidates, {
            2: [job("lint")],
            3: [job("lint"), job("coverage"), job("check (macos-14)", end=900)],
        })
        samples = monitor.baseline_runs(api, run(), "main", {"lint": {}, "coverage": {}})
        self.assertEqual([candidate["id"] for candidate, _ in samples], [3])
        self.assertEqual(set(samples[0][1]), {"lint", "coverage"})
        self.assertEqual(monitor.wall_time(*samples[0]), 70)

    def test_baseline_is_bounded_and_requires_successful_matching_jobs(self):
        candidates = [run(i, updated="2026-10-11T00:20:00Z") for i in range(8)]
        api = FakeGitHub(candidates, {i: [job("lint", conclusion="failure" if i == 0 else "success")] for i in range(8)})
        samples = monitor.baseline_runs(api, run(), "main", {"lint": {}})
        self.assertEqual([candidate["id"] for candidate, _ in samples], [1, 2, 3, 4, 5])

    def test_report_uses_median_not_mean_and_handles_zero_baseline(self):
        current = run(created="2026-10-11T00:00:00Z")
        samples = [(current, {"lint": job("lint", 10, value)}) for value in [30, 70, 600]]
        report = monitor.render_report(current, {"lint": job("lint", 10, 130)}, samples, "main")
        self.assertIn("+1m 00s (+100.0%)", report)
        self.assertIn("median of 3", report)
        self.assertEqual(monitor.delta(20, 0), "N/A")

    def test_failed_and_missing_baseline_are_not_claimed_as_speedups(self):
        current = run(created="2026-10-11T00:00:00Z", conclusion="failure")
        report = monitor.render_report(current, {"lint": job("lint", conclusion="failure")}, [], "main")
        self.assertIn("incomplete or unsuccessful", report)
        self.assertIn("No successful baseline", report)
        self.assertIn("| <code>lint</code> (<code>failure</code>) | 1m 00s | N/A | N/A |", report)

    def test_untrusted_labels_cannot_break_the_table(self):
        name = "unsafe|<img>\n`label`"
        current = run(created="2026-10-11T00:00:00Z")
        report = monitor.render_report(current, {name: job(name)}, [], "main")
        self.assertIn("unsafe&#124;&lt;img&gt; &#96;label&#96;", report)

    def test_mentions_and_markdown_links_remain_literal_metadata(self):
        self.assertEqual(monitor.label("@someone [link](https://example.com)"),
                         "<code>@someone [link](https://example.com)</code>")


class CommentTests(unittest.TestCase):
    def setUp(self):
        self.current = run()
        self.pull = {"number": 12, "state": "open", "head": {"sha": self.current["head_sha"]}, "base": {"ref": "main"}}

    def test_fork_run_without_payload_pr_uses_commit_association(self):
        api = FakeGitHub(pulls={12: self.pull})
        self.assertEqual(monitor.current_pull_requests(api, self.current), [self.pull])

    def test_superseded_head_is_ignored(self):
        self.pull["head"]["sha"] = "b" * 40
        api = FakeGitHub(pulls={12: self.pull})
        self.assertEqual(monitor.current_pull_requests(api, self.current), [])
        monitor.publish(api, self.pull, self.current, "report")
        self.assertEqual(api.writes, [])

    def test_merging_before_the_monitor_starts_does_not_lose_the_report(self):
        self.pull.update(state="closed", merged=True)
        api = FakeGitHub(pulls={12: self.pull})
        self.assertEqual(monitor.current_pull_requests(api, self.current), [self.pull])
        monitor.publish(api, self.pull, self.current, "report")
        self.assertEqual(api.writes, [("/issues/12/comments", "POST", {"body": "report"})])

    def test_comment_is_created_then_updated_without_touching_human_comments(self):
        api = FakeGitHub(pulls={12: self.pull}, comments=[{
            "id": 1, "user": {"type": "User"}, "body": monitor.MARKER,
        }])
        monitor.publish(api, self.pull, self.current, "report")
        self.assertEqual(api.writes, [("/issues/12/comments", "POST", {"body": "report"})])
        api.comments.append({"id": 2, "user": {"type": "Bot"}, "body": monitor.MARKER + "\n<!-- ci-run:9:1 -->"})
        monitor.publish(api, self.pull, self.current, "updated")
        self.assertEqual(api.writes[-1], ("/issues/comments/2", "PATCH", {"body": "updated"}))

    def test_older_run_or_attempt_cannot_overwrite_newer_report(self):
        for run_id, attempt in [(11, 1), (10, 2)]:
            with self.subTest(run_id=run_id, attempt=attempt):
                api = FakeGitHub(pulls={12: self.pull}, comments=[{
                    "id": 2, "user": {"type": "Bot"},
                    "body": f"{monitor.MARKER}\n<!-- ci-run:{run_id}:{attempt} -->",
                }])
                monitor.publish(api, self.pull, self.current, "report")
                self.assertEqual(api.writes, [])


if __name__ == "__main__":
    unittest.main()
