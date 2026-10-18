#!/usr/bin/env python3
"""Resolve and record successful incremental PR validation."""

import argparse
import io
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
from urllib.parse import urlencode
import zipfile

import affected_tests as affected
from affected_tests import AnalysisError, command, revision


VERSION = 1
POLICY = "incremental-pr-v1"
RECORD_FILE = "validation.json"
SHA = re.compile(r"[0-9a-f]{40}(?:[0-9a-f]{24})?")


def execution_signature(root, target):
    if not 0 <= target <= 100:
        raise ValueError("coverage target must be between 0 and 100")
    return {
        "policy": POLICY, "coverage_target": target,
        "toolchain": command(root, "rustc", "-vV"),
        "features": "all", "profile": "ci", "target": os.environ.get("CARGO_BUILD_TARGET"),
        "environment": {key: os.environ[key] for key in (
            "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_INCREMENTAL",
            "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG",
        ) if key in os.environ},
    }


def current_context(root, repository, pr, run, signature):
    merge = revision(root, "HEAD")
    parents = command(root, "git", "rev-list", "--parents", "-n", "1", merge).split()[1:]
    if len(parents) != 2:
        raise ValueError("PR validation requires the tested two-parent merge snapshot")
    if command(root, "git", "status", "--porcelain", "--untracked-files=all"):
        raise ValueError("PR validation requires a clean checkout")
    return {
        "version": VERSION, "repository": repository, "pr": pr,
        "run_id": run["id"], "run_attempt": run["run_attempt"], "workflow_id": run["workflow_id"],
        "head": parents[1], "base": parents[0], "merge": merge,
        "tree": command(root, "git", "rev-parse", "HEAD^{tree}").strip(),
        "signature": signature,
    }


def artifact_name(run):
    return f"pr-validation-{run['id']}-{run['run_attempt']}"


def cumulative(current, reason):
    return {"base": current["base"], "head": current["merge"], "scope": "cumulative",
            "previous": None, "reason": reason}


def valid_coverage(record):
    coverage = record["coverage"]
    total, covered = coverage["total"], coverage["covered"]
    if type(total) is not int or type(covered) is not int or not 0 <= covered <= total:
        return False
    target = record["signature"]["coverage_target"]
    return (
        coverage["base"] == record["comparison"]["base"]
        and coverage["head"] == record["merge"] and coverage["tree"] == record["tree"]
        and coverage["target"] == target and 0 <= target <= 100
        and ((total > 0 and coverage["status"] == "passed" and covered * 100 >= target * total)
             or (total == 0 and coverage["status"] == "not_applicable"))
    )


def eligible(root, current, run, record):
    try:
        if run["status"] != "completed" or run["conclusion"] != "success":
            return False
        for field in ("id", "run_attempt", "workflow_id"):
            if record[{"id": "run_id"}.get(field, field)] != run[field]:
                return False
        if run["head_sha"] != record["head"]:
            return False
        for field in ("version", "repository", "pr", "workflow_id", "base", "signature"):
            if record[field] != current[field]:
                return False
        if not all(isinstance(record[key], str) and SHA.fullmatch(record[key]) for key in ("head", "base", "merge", "tree")):
            return False
        if record["title_check"]["head_sha"] != record["head"] or record["title_check"]["conclusion"] != "success":
            return False
        comparison = record["comparison"]
        if (comparison["head"] != record["merge"] or comparison["scope"] not in {"incremental", "cumulative"}
                or not isinstance(comparison["base"], str) or not SHA.fullmatch(comparison["base"])
                or not isinstance(comparison["reason"], str) or not valid_coverage(record)):
            return False
        if not isinstance(record["packages"], list) or not all(isinstance(p, str) and p for p in record["packages"]):
            return False
        parents = command(root, "git", "rev-list", "--parents", "-n", "1", record["merge"]).split()[1:]
        if parents != [record["base"], record["head"]]:
            return False
        if command(root, "git", "rev-parse", record["merge"] + "^{tree}").strip() != record["tree"]:
            return False
        return command(root, "git", "merge-base", record["head"], current["head"]).strip() == record["head"]
    except (KeyError, TypeError, ValueError, AnalysisError):
        return False


def ancestry_order(root, head, run):
    return (int(command(root, "git", "rev-list", "--count", run["head_sha"] + ".." + head)),
            -run["id"], -run["run_attempt"])


def choose_baseline(root, current, candidates):
    eligible_records = [(run, record) for run, record in candidates if eligible(root, current, run, record)]
    if not eligible_records:
        return cumulative(current, "No compatible completed successful validation record")
    # Completion order is unrelated to source ancestry, especially after reruns.
    run, record = min(eligible_records, key=lambda pair: ancestry_order(root, current["head"], pair[0]))
    return {"base": record["merge"], "head": current["merge"], "scope": "incremental",
            "previous": {"run_id": run["id"], "run_attempt": run["run_attempt"], "merge": record["merge"]},
            "reason": "Reusing the nearest compatible successful PR-head ancestor"}


def read_record(archive):
    with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
        if zipped.namelist() != [RECORD_FILE] or zipped.getinfo(RECORD_FILE).file_size > 1024 * 1024:
            raise ValueError("unexpected validation artifact contents")
        record = json.loads(zipped.read(RECORD_FILE))
        if not isinstance(record, dict):
            raise ValueError("Validation artifact must contain an object")
        return record


def discover_baseline(root, current, api, branch, download):
    started = time.monotonic()
    try:
        query = urlencode({"branch": branch, "status": "success", "per_page": 100})
        runs = api.request(f"/actions/workflows/{current['workflow_id']}/runs?{query}")["workflow_runs"]
        if len(runs) >= 100:
            return cumulative(current, "Validation discovery limit reached")
        rejection = None
        ancestors = set(command(root, "git", "rev-list", current["head"]).splitlines())
        runs = [run for run in runs if run["head_sha"] in ancestors]
        for run in sorted(runs, key=lambda r: ancestry_order(root, current["head"], r)):
            if time.monotonic() - started > 60:
                return cumulative(current, "Validation discovery deadline reached")
            if run["id"] == current["run_id"] or run["workflow_id"] != current["workflow_id"]:
                continue
            if run["status"] != "completed" or run["conclusion"] != "success":
                continue
            artifacts = api.request(f"/actions/runs/{run['id']}/artifacts?per_page=100")["artifacts"]
            matches = [a for a in artifacts if a["name"] == artifact_name(run) and not a["expired"]]
            if len(matches) != 1:
                continue
            try:
                record = read_record(download(matches[0]["id"]))
                if record.get("repository") != current["repository"] or record.get("pr") != current["pr"]:
                    continue
                if record.get("base") != current["base"]:
                    rejection = rejection or "Target base changed since the successful validation"
                    continue
                if record.get("signature") != current["signature"]:
                    rejection = rejection or "Execution settings changed since the successful validation"
                    continue
                merge = record.get("merge", "")
                if not isinstance(merge, str) or not SHA.fullmatch(merge):
                    continue
                # Fetching is explicit CI orchestration; local selectors never fetch.
                present = subprocess.run(["git", "cat-file", "-e", merge + "^{commit}"], cwd=root, capture_output=True)
                if present.returncode:
                    subprocess.run(["git", "fetch", "--no-tags", "origin", merge], cwd=root,
                                   capture_output=True, check=True, timeout=30)
                result = choose_baseline(root, current, [(run, record)])
                if result["scope"] == "incremental":
                    return result
            except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError, zipfile.BadZipFile) as error:
                # An unusable record cannot invalidate another proven ancestor.
                rejection = rejection or f"Recorded validation unavailable: {error}"
        return cumulative(current, rejection or "No compatible completed successful validation record")
    except (OSError, ValueError, KeyError, TypeError, AnalysisError, subprocess.SubprocessError, zipfile.BadZipFile) as error:
        return cumulative(current, f"Validation history unavailable: {error}")


def changed_lines(root, base, head):
    paths = command(root, "git", "diff", "--no-renames", "--name-only", "-z", base, head, "--").split("\0")
    result = {}
    for path in paths:
        if not path.endswith(".rs"):
            continue
        patch = command(root, "git", "diff", "--no-ext-diff", "--no-textconv", "--no-color",
                        "--no-renames", "--unified=0", base, head, "--", path)
        if "Binary files " in patch:
            raise ValueError(f"Cannot map binary Rust source: {path}")
        lines = set()
        for start, count in re.findall(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@", patch, re.MULTILINE):
            start, count = int(start), int(count) if count else 1
            lines.update(range(start, start + count))
        if lines:
            result[path] = lines
    return result


def read_lcov(root, path):
    root = root.resolve()
    mappings = {}
    source, lines, declared = None, {}, None
    for line in path.read_text().splitlines():
        if line.startswith("SF:"):
            if source is not None:
                raise ValueError("LCOV source record is unterminated")
            source, lines, declared = line[3:], {}, None
            if not source:
                raise ValueError("LCOV source path is empty")
        elif line.startswith("DA:"):
            if source is None:
                raise ValueError("LCOV executable line has no source")
            fields = line[3:].split(",")
            if len(fields) not in (2, 3):
                raise ValueError("Malformed LCOV executable line")
            number, hits = int(fields[0]), int(fields[1])
            if number <= 0 or hits < 0:
                raise ValueError("Invalid LCOV executable line")
            lines[number] = lines.get(number, False) or hits > 0
        elif line.startswith("LF:"):
            declared = int(line[3:])
        elif line == "end_of_record":
            if source is None or (declared is not None and declared != len(lines)):
                raise ValueError("LCOV line count does not match its record")
            absolute = (root / source).resolve()
            if absolute.is_relative_to(root):
                destination = mappings.setdefault(absolute.relative_to(root).as_posix(), {})
                for number, hit in lines.items():
                    destination[number] = destination.get(number, False) or hit
            source, lines, declared = None, {}, None
    if source is not None or not mappings:
        raise ValueError("LCOV report is incomplete or has no workspace mappings")
    return mappings


def verify_current(root, current):
    if (revision(root, "HEAD") != current["merge"]
            or command(root, "git", "rev-parse", "HEAD^{tree}").strip() != current["tree"]
            or command(root, "git", "status", "--porcelain", "--untracked-files=all")):
        raise ValueError("Current checkout no longer matches the validation source")


def evaluate_coverage(root, current, comparison, plan, report):
    verify_current(root, current)
    if (comparison["head"] != current["merge"] or plan["head"] != comparison["head"]
            or plan["base"] != comparison["base"] or plan["comparison_mode"] != "snapshot"):
        raise ValueError("Coverage and selection comparison identities differ")
    changes = changed_lines(root, comparison["base"], comparison["head"])
    changes = {p: lines for p, lines in changes.items() if p not in plan["ignored_files"]}
    if plan["packages"]:
        provenance = json.loads(report.with_name("source.json").read_text())
        expected = {"head": current["merge"], "tree": current["tree"], "dirty": False,
                    "input_environment": current["signature"]["environment"],
                    "toolchain": current["signature"]["toolchain"], "features": "all",
                    "target": current["signature"]["target"], "profile": current["signature"]["profile"]}
        if provenance != expected:
            raise ValueError("Coverage report source or execution configuration is stale")
        mappings = read_lcov(root, report)
    else:
        if changes:
            raise ValueError("Rust source changes have no selected test scope")
        mappings = {}
    missing, uncovered = [], {}
    total = covered = 0
    for path, changed in sorted(changes.items()):
        contents = command(root, "git", "show", f"{current['merge']}:{path}").splitlines()
        if path not in mappings:
            # A wholly blank/comment-only file is independent evidence. Missing
            # compiler mappings for other source are uncertainty, not a pass.
            if not all(not line.strip() or line.lstrip().startswith("//") for line in contents):
                missing.append(path)
            continue
        executable = mappings[path]
        if any(number > len(contents) for number in executable):
            raise ValueError(f"Coverage mapping exceeds current source: {path}")
        selected = changed & executable.keys()
        total += len(selected)
        covered += sum(executable[number] for number in selected)
        misses = sorted(number for number in selected if not executable[number])
        if misses:
            uncovered[path] = misses
    if missing:
        raise ValueError("No executable mapping or non-executable proof for: " + ", ".join(missing))
    target = current["signature"]["coverage_target"]
    return {"base": comparison["base"], "head": current["merge"], "tree": current["tree"],
            "covered": covered, "total": total, "target": target, "uncovered": uncovered,
            "status": "not_applicable" if total == 0 else "passed" if covered * 100 >= target * total else "failed"}


def download_artifact(repository, artifact_id):
    return subprocess.run(["gh", "api", f"/repos/{repository}/actions/artifacts/{artifact_id}/zip"],
                          check=True, capture_output=True, timeout=30).stdout


def make_record(root, current, comparison, plan, coverage, needs, run, title_check):
    if set(needs) != {"lint", "check", "coverage"} or any(job["result"] != "success" for job in needs.values()):
        raise ValueError("All required CI jobs must succeed before publishing validation")
    expected = current_context(root, current["repository"], current["pr"], run, current["signature"])
    if current != expected or run["head_sha"] != current["head"] or current["signature"]["policy"] != POLICY:
        raise ValueError("Validation input belongs to another source or workflow attempt")
    if (title_check["head_sha"] != current["head"] or title_check["conclusion"] != "success"
            or title_check["name"] != "pr-title" or title_check["app"]["slug"] != "github-actions"):
        raise ValueError("The required PR title check has not succeeded")
    if (comparison["head"] != current["merge"] or plan["base"] != comparison["base"]
            or plan["head"] != current["merge"] or plan["comparison_mode"] != "snapshot"):
        raise ValueError("Selection belongs to another comparison")
    record = current | {"comparison": comparison, "packages": plan["packages"], "coverage": coverage,
                        "title_check": {"id": title_check["id"], "head_sha": title_check["head_sha"], "conclusion": "success"}}
    if not valid_coverage(record):
        raise ValueError("Incremental coverage did not succeed")
    return record


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def summarize(comparison, plan, coverage=None):
    return {"comparison": comparison, "packages": plan["packages"], "reasons": plan["reasons"],
            "fallback_reasons": plan["fallback_reasons"], "ignored_files": plan["ignored_files"],
            "coverage": coverage}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("select", "evaluate", "record"))
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--coverage-target", type=float)
    parser.add_argument("--report", type=Path, default=Path("target/coverage/result/lcov.info"))
    parser.add_argument("--cumulative", action="store_true")
    args = parser.parse_args()
    try:
        root = Path(command(Path.cwd(), "git", "rev-parse", "--show-toplevel").strip()).resolve()
        directory = args.directory.resolve()
        directory.mkdir(parents=True, exist_ok=True)
        if args.phase in {"select", "record"}:
            from ci_monitor import GitHub
            repository = os.environ["GITHUB_REPOSITORY"]
            api = GitHub(repository, os.environ["GH_TOKEN"], os.environ.get("GITHUB_API_URL", "https://api.github.com"))
            run_id, attempt = int(os.environ["GITHUB_RUN_ID"]), int(os.environ["GITHUB_RUN_ATTEMPT"])
            run = api.request(f"/actions/runs/{run_id}/attempts/{attempt}")
            if run["id"] != run_id or run["run_attempt"] != attempt:
                raise ValueError("Workflow attempt metadata does not match the current run")
        if args.phase == "select":
            if args.coverage_target is None:
                parser.error("select requires --coverage-target")
            event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
            manual = os.environ["GITHUB_EVENT_NAME"] == "workflow_dispatch"
            pr = int(event["inputs"]["pr"] if manual else event["number"])
            pull = api.request(f"/pulls/{pr}") if manual else event["pull_request"]
            current = current_context(root, repository, pr, run, execution_signature(root, args.coverage_target))
            if current["head"] != pull["head"]["sha"] or current["head"] != run["head_sha"]:
                raise ValueError("Tested merge and workflow must identify the requested PR head")
            if manual and (revision(root, event["inputs"]["merge_sha"]) != current["merge"]
                           or pull["merge_commit_sha"] != current["merge"]):
                raise ValueError("Manual PR validation requires the current pinned PR merge SHA")
            if manual or args.cumulative:
                comparison = cumulative(current, "Cumulative PR validation explicitly requested")
            else:
                comparison = discover_baseline(root, current, api, pull["head"]["ref"],
                                               lambda identifier: download_artifact(repository, identifier))
            options = argparse.Namespace(base=comparison["base"], head=current["merge"], comparison="snapshot",
                                         worktree=False, commit=None, target=current["signature"]["target"],
                                         rust_analyzer="rust-analyzer", timeout=120)
            plan = affected.make_plan(root, options)
            write_json(directory / "context.json", current)
            write_json(directory / "comparison.json", comparison)
            write_json(directory / "plan.json", plan)
            if os.environ.get("GITHUB_OUTPUT"):
                with open(os.environ["GITHUB_OUTPUT"], "a") as output:
                    output.write(f"scoped=true\nrun={str(bool(plan['packages'])).lower()}\npackages={json.dumps(plan['packages'])}\n")
            summary = summarize(comparison, plan)
        else:
            current, comparison, plan = [json.loads((directory / f"{name}.json").read_text())
                                         for name in ("context", "comparison", "plan")]
            if args.phase == "evaluate":
                coverage = evaluate_coverage(root, current, comparison, plan, args.report)
                write_json(directory / "coverage.json", coverage)
                summary = summarize(comparison, plan, coverage)
            else:
                event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
                pr = int(event["number"] if os.environ["GITHUB_EVENT_NAME"] == "pull_request" else event["inputs"]["pr"])
                if current["repository"] != repository or current["pr"] != pr:
                    raise ValueError("Validation record belongs to another repository or PR")
                checks = api.request(f"/commits/{current['head']}/check-runs?check_name=pr-title&per_page=100")["check_runs"]
                checks = [c for c in checks if c["name"] == "pr-title" and c["app"]["slug"] == "github-actions"]
                if not checks:
                    raise ValueError("The required PR title check is unavailable")
                coverage = json.loads((directory / "coverage.json").read_text())
                record = make_record(root, current, comparison, plan, coverage,
                                     json.loads(os.environ["VALIDATION_NEEDS"]), run, max(checks, key=lambda c: c["id"]))
                write_json(directory / RECORD_FILE, record)
                summary = summarize(comparison, plan, coverage)
        print(json.dumps(summary, indent=2))
        if os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as output:
                output.write("### PR validation\n\n```json\n" + json.dumps(summary, indent=2) + "\n```\n")
        return 1 if args.phase == "evaluate" and coverage["status"] == "failed" else 0
    except (AnalysisError, OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
