#!/usr/bin/env python3

import argparse
from datetime import datetime
import html
import io
import json
import os
from pathlib import Path
import re
import subprocess
from statistics import median
from urllib.parse import urlencode
from urllib.request import Request, urlopen
import zipfile


MARKER = "<!-- miniflow-ci-timing -->"
SAMPLE_SIZE = 5


class GitHub:
    def __init__(self, repository, token, api_url="https://api.github.com"):
        self.root = f"{api_url}/repos/{repository}"
        self.token = token

    def request(self, path, method="GET", body=None):
        request = Request(
            self.root + path,
            data=json.dumps(body).encode() if body is not None else None,
            method=method,
            headers={
                "Authorization": f"Bearer {self.token}",
                "Accept": "application/vnd.github+json",
                "Content-Type": "application/json",
                "X-GitHub-Api-Version": "2022-11-28",
            },
        )
        with urlopen(request, timeout=30) as response:
            return json.load(response)

    def pages(self, path, key=None, max_pages=10):
        separator = "&" if "?" in path else "?"
        for page in range(1, max_pages + 1):
            data = self.request(f"{path}{separator}per_page=100&page={page}")
            items = data[key] if key else data
            yield from items
            if len(items) < 100:
                break

    def jobs(self, run):
        # Attempts must not mix timings from jobs completed before a rerun.
        return list(self.pages(
            f"/actions/runs/{run['id']}/attempts/{run['run_attempt']}/jobs", "jobs"
        ))


def timestamp(value):
    return datetime.fromisoformat(value.replace("Z", "+00:00"))


def duration(item):
    if not item.get("started_at") or not item.get("completed_at"):
        return None
    return max(0, (timestamp(item["completed_at"]) - timestamp(item["started_at"])).total_seconds())


def timings(jobs):
    return {job["name"]: job for job in jobs if duration(job) is not None and job["conclusion"] != "skipped"}


def run_start(run):
    # Reruns keep the original created_at, which can precede this attempt by days.
    return timestamp(run["run_started_at"] if run["run_attempt"] > 1 else run["created_at"])


def wall_time(run, jobs):
    if not jobs:
        return None
    end = max(timestamp(job["completed_at"]) for job in jobs.values())
    return max(0, (end - run_start(run)).total_seconds())


def format_duration(seconds):
    if seconds is None:
        return "N/A"
    seconds = round(seconds)
    return f"{seconds // 60}m {seconds % 60:02d}s"


def delta(current, baseline):
    if current is None or baseline is None or baseline == 0:
        return "N/A"
    difference = current - baseline
    sign = "+" if difference >= 0 else "-"
    return f"{sign}{format_duration(abs(difference))} ({difference / baseline:+.1%})"


def label(value):
    value = html.escape(str(value)).replace("|", "&#124;").replace("\n", " ").replace("\r", " ").replace("`", "&#96;")
    # Job names come from PR workflows; literal text must not become links or mentions.
    return f"<code>{value}</code>"


def baseline_runs(api, run, branch, current_jobs):
    query = urlencode({"branch": branch, "event": "push", "status": "success"})
    samples = []
    for candidate in api.pages(f"/actions/workflows/{run['workflow_id']}/runs?{query}", "workflow_runs", max_pages=3):
        if timestamp(candidate["updated_at"]) > run_start(run):
            continue
        jobs = timings(api.jobs(candidate))
        # main also runs macOS. Compare only the exact jobs present in this PR.
        if not all(name in jobs and jobs[name]["conclusion"] == "success" for name in current_jobs):
            continue
        samples.append((candidate, {name: jobs[name] for name in current_jobs}))
        if len(samples) == SAMPLE_SIZE:
            break
    return samples


def coverage_summary(jobs, current_wall):
    coverage = jobs.get("coverage")
    if coverage is None:
        return []
    crates = [
        (match[1], step)
        for step in coverage.get("steps", [])
        if (match := re.fullmatch(r"Coverage tests \(([^)]+)\)", step["name"]))
    ]
    if not crates:
        return []
    skipped = [name for name, step in crates if step["conclusion"] == "skipped"]
    values = [duration(step) for _, step in crates if step["conclusion"] != "skipped"]
    total = sum(values) if all(value is not None for value in values) else None
    lines = [
        "### Summary",
        "",
        "| Metric | Duration |",
        "| --- | ---: |",
        f"| CI wall time (including queueing) | {format_duration(current_wall)} |",
        f"| Coverage job (including setup and cache upload) | {format_duration(duration(coverage))} |",
        f"| Crate test steps total (excluding skipped steps) | {format_duration(total)} |",
        "",
        "Skipped crates: " + (", ".join(label(name) for name in skipped) if skipped else "None") + ".",
        "",
        "| Crate | Result | Duration |",
        "| --- | --- | ---: |",
    ]
    for name, step in crates:
        value = None if step["conclusion"] == "skipped" else duration(step)
        lines.append(f"| {label(name)} | {label(step['conclusion'])} | {format_duration(value)} |")
    lines.extend([
        "",
        "Skipped crates show N/A and are excluded from totals and timing baselines; they may be unselected or blocked by an earlier failure.",
        "Crate totals exclude shared compilation, report generation, and uploads; those steps are listed below.",
        "GitHub timestamps have one-second precision: a successful 0s step is not a skipped step.",
        "",
    ])
    return lines


def validation_record(api, repository, run, pr):
    if run["conclusion"] != "success":
        return None
    try:
        artifacts = api.request(f"/actions/runs/{run['id']}/artifacts?per_page=100")["artifacts"]
        name = f"pr-validation-{run['id']}-{run['run_attempt']}"
        matches = [a for a in artifacts if a["name"] == name and not a["expired"]]
        if len(matches) != 1:
            return None
        archive = subprocess.run(
            ["gh", "api", f"/repos/{repository}/actions/artifacts/{matches[0]['id']}/zip"],
            check=True, capture_output=True, timeout=30,
        ).stdout
        with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
            if zipped.namelist() != ["validation.json"] or zipped.getinfo("validation.json").file_size > 1024 * 1024:
                return None
            record = json.loads(zipped.read("validation.json"))
        if (record["repository"] != repository or record["pr"] != pr or record["head"] != run["head_sha"]
                or record["run_id"] != run["id"] or record["run_attempt"] != run["run_attempt"]):
            return None
        if not {"scope", "base", "head", "previous", "reason"} <= record["comparison"].keys():
            return None
        if not {"covered", "total", "status"} <= record["coverage"].keys():
            return None
        return record
    except (OSError, ValueError, KeyError, TypeError, AttributeError, subprocess.SubprocessError, zipfile.BadZipFile):
        return None


def render_report(run, jobs, samples, branch, validation=None):
    complete = run["conclusion"] == "success"
    baseline_wall = median([wall_time(candidate, sample) for candidate, sample in samples]) if samples else None
    current_wall = wall_time(run, jobs)
    lines = [
        MARKER,
        f"<!-- ci-run:{run['id']}:{run['run_attempt']} -->",
        "## CI timing summary",
        "",
        f"[Run #{run['run_number']}, attempt {run['run_attempt']}]({run['html_url']}) "
        f"for `{run['head_sha'][:12]}`: **{label(run['conclusion'])}**.",
        "",
    ]
    if validation is not None:
        comparison, coverage = validation["comparison"], validation["coverage"]
        lines.extend([
            "### Validation range", "",
            f"Scope: {label(comparison['scope'])}.",
            f"Comparison: {label(comparison['base'])} to {label(comparison['head'])}.",
            f"Inherited validation: {label(comparison['previous'])}.",
            f"Selection reason: {label(comparison['reason'])}.",
            f"Current increment coverage: {label(coverage['covered'])}/{label(coverage['total'])} executable lines; {label(coverage['status'])}.",
            "Inherited validation does not contribute historical LCOV hits to this measurement.", "",
        ])
    lines.extend(coverage_summary(jobs, current_wall))
    lines.extend([
        "### Timing comparison",
        "",
        f"Baseline: median of {len(samples)} successful CI pushes to **{label(branch)}** completed before this run started.",
        "Only matching jobs are compared; additional base-branch jobs (including macOS) are excluded.",
        "Wall time includes queueing and job dependencies; job durations include setup and cache upload.",
        "Step baselines include only successful executions of the same step; skipped and missing steps are not zero-duration samples.",
        "",
    ])
    if not complete:
        lines.extend(["This run is incomplete or unsuccessful; its wall-time delta is not comparable.", ""])
    if not samples:
        lines.extend(["No successful baseline with all matching jobs is available yet.", ""])
    lines.extend([
        "| Metric | Current | Base median | Change |",
        "| --- | ---: | ---: | ---: |",
        f"| Comparable jobs wall time | {format_duration(current_wall)} | {format_duration(baseline_wall)} | "
        f"{delta(current_wall, baseline_wall) if complete else 'N/A'} |",
    ])
    for name, job in sorted(jobs.items()):
        value = duration(job)
        reference = median([duration(sample[name]) for _, sample in samples]) if samples else None
        change = delta(value, reference) if job["conclusion"] == "success" else "N/A"
        lines.append(f"| {label(name)} ({label(job['conclusion'])}) | {format_duration(value)} | {format_duration(reference)} | {change} |")
    lines.extend(["", "<details>", "<summary>Step timings and baseline runs</summary>", "",
                  "| Job / step | Current | Base median | Change |", "| --- | ---: | ---: | ---: |"])
    for name, job in sorted(jobs.items()):
        for step in job.get("steps", []):
            value = duration(step)
            if value is None or step["conclusion"] == "skipped":
                continue
            references = [
                duration(old_step)
                for _, sample in samples
                for old_step in sample[name].get("steps", [])
                if old_step["name"] == step["name"] and old_step["conclusion"] == "success" and duration(old_step) is not None
            ]
            reference = median(references) if references else None
            change = delta(value, reference) if step["conclusion"] == "success" else "N/A"
            lines.append(f"| {label(name)} / {label(step['name'])} | {format_duration(value)} | {format_duration(reference)} | {change} |")
    lines.append("")
    for candidate, _ in samples:
        lines.append(f"- [Run #{candidate['run_number']}]({candidate['html_url']}) ({candidate['head_sha'][:12]})")
    lines.extend(["", "</details>", "", "Timings vary with runner load and cache state; this is a trend comparison, not a controlled benchmark."])
    return "\n".join(lines) + "\n"


def current_pull_requests(api, run):
    candidates = run.get("pull_requests") or list(api.pages(f"/commits/{run['head_sha']}/pulls"))
    result = []
    for candidate in candidates:
        pull = api.request(f"/pulls/{candidate['number']}")
        if pull["head"]["sha"] == run["head_sha"]:
            result.append(pull)
    return result


def publish(api, pull, run, body):
    # A superseded run must not replace the latest head's report.
    latest_pull = api.request(f"/pulls/{pull['number']}")
    if latest_pull["head"]["sha"] != run["head_sha"]:
        return
    comments = api.pages(f"/issues/{pull['number']}/comments")
    existing = next((comment for comment in comments
                     if comment["user"]["type"] == "Bot" and comment.get("body", "").startswith(MARKER)), None)
    if existing:
        metadata = re.search(r"<!-- ci-run:(\d+):(\d+) -->", existing["body"])
        if metadata and tuple(map(int, metadata.groups())) > (run["id"], run["run_attempt"]):
            return
        api.request(f"/issues/comments/{existing['id']}", "PATCH", {"body": body})
    else:
        api.request(f"/issues/{pull['number']}/comments", "POST", {"body": body})


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--run-id", type=int, required=True)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    api = GitHub(os.environ["GITHUB_REPOSITORY"], os.environ["GH_TOKEN"], os.environ.get("GITHUB_API_URL", "https://api.github.com"))
    run = api.request(f"/actions/runs/{args.run_id}")
    if run["event"] != "pull_request" or run["status"] != "completed" or run["conclusion"] == "cancelled":
        return
    jobs = timings(api.jobs(run))
    if not jobs:
        return
    for pull in current_pull_requests(api, run):
        samples = baseline_runs(api, run, pull["base"]["ref"], jobs)
        validation = validation_record(api, os.environ["GITHUB_REPOSITORY"], run, pull["number"])
        body = render_report(run, jobs, samples, pull["base"]["ref"], validation)
        if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
            with Path(summary).open("a") as output:
                output.write(body)
        if args.dry_run:
            print(body)
        else:
            publish(api, pull, run, body)


if __name__ == "__main__":
    main()
