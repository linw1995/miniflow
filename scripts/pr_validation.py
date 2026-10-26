#!/usr/bin/env python3
"""Accept Flow plans, evaluate coverage, and record successful PR validation."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

from coverage_selection import validate_selection


FLOW = Path(__file__).resolve().parent.parent / "tools/test-affected/flows/test-affected.json"
POLICY_SETTINGS = next(node["config"]["value"] for node in json.loads(FLOW.read_text())["nodes"]
                       if node["id"] == "validation_policy")
VERSION, POLICY = POLICY_SETTINGS["record_version"], POLICY_SETTINGS["id"]
RECORD_FILE = "validation.json"
SHA = re.compile(r"[0-9a-f]{40}(?:[0-9a-f]{24})?")


class AnalysisError(ValueError):
    pass


def command(root, *args):
    result = subprocess.run(args, cwd=root, capture_output=True, text=True,
                            env=os.environ | {"GIT_OPTIONAL_LOCKS": "0", "GIT_NO_LAZY_FETCH": "1"})
    if result.returncode:
        raise AnalysisError(f"{args[0]} failed: {result.stderr.strip()}")
    return result.stdout


def revision(root, ref):
    return command(root, "git", "rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}").strip()


def input_environment(signature):
    return {key: value for key, value in signature["environment"].items() if value is not None}


def execution_signature(root, target):
    if not 0 <= target <= 100:
        raise ValueError("coverage target must be between 0 and 100")
    return {
        "policy": POLICY, "coverage_target": target,
        "toolchain": command(root, "rustc", "-vV"),
        "features": "all", "profile": os.environ.get("NEXTEST_PROFILE", "ci"), "target": os.environ.get("CARGO_BUILD_TARGET"),
        "environment": {key: os.environ.get(key) for key in (
            "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_INCREMENTAL",
            "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG",
        )},
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


def declarations_only(contents):
    # Prove a deliberately small Rust grammar: imports and out-of-line modules.
    # Unknown attributes, macros, inline modules, and executable items stay unmapped.
    source = re.sub(r"(?m)^[ \t]*//[^\n]*$", "", contents)
    attribute = r'(?m)^[ \t]*#!?\[(?:macro_use|doc\(hidden\)|cfg\((?:[A-Za-z0-9_, ()=]|"[^"\\\r\n]*")*\)|path[ \t]*=[ \t]*"[^"\\\r\n]*")\][ \t]*$'
    source = re.sub(attribute, "", source)
    visibility = r"(?:pub(?:\((?:crate|super|self)\))?\s+)?"
    declaration = re.compile(r"\s*(?:" + visibility + r"mod\s+[A-Za-z_]\w*\s*;|"
                             + visibility + r"use\s+[A-Za-z_:][A-Za-z0-9_:,*{}\s]*;|"
                             + visibility + r"extern\s+crate\s+[A-Za-z_]\w*(?:\s+as\s+[A-Za-z_]\w*)?\s*;)")
    offset = 0
    while source[offset:].strip():
        item = declaration.match(source, offset)
        if item is None:
            return False
        offset = item.end()
    return True


def executable_items(contents):
    # The pinned Rust parser locates top-level items without interpreting literals or macros.
    # Keep every nondeclaration byte-for-byte so missing mappings cannot hide body changes.
    try:
        parsed = subprocess.run(["rust-analyzer", "parse", "--json"], input=contents,
                                capture_output=True, text=True, timeout=30, check=True)
        tree = json.loads(parsed.stdout)
        source = contents.encode()
        if tree["kind"] != "SOURCE_FILE" or tree["end"][0] != len(source):
            return None
        def has_error(node):
            return node["kind"] == "ERROR" or any(has_error(child) for child in node.get("children", []))
        if has_error(tree):
            return None
        items, offset = [], 0
        for item in tree["children"]:
            start, end = item["start"][0], item["end"][0]
            if start != offset or not start <= end <= len(source):
                return None
            offset = end
            if item["kind"] == "WHITESPACE":
                continue
            text = source[start:end].decode()
            if item["kind"] in {"USE", "MODULE", "EXTERN_CRATE"} and declarations_only(text):
                continue
            items.append((item["kind"], text))
        return items if offset == len(source) else None
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError, IndexError):
        return None


def declaration_changes_only(before, after):
    previous = executable_items(before)
    return previous is not None and previous == executable_items(after)


def evaluate_coverage(root, current, comparison, plan, report):
    verify_current(root, current)
    if (comparison["head"] != current["merge"] or plan["head"] != comparison["head"]
            or plan["base"] != comparison["base"] or plan["comparison_mode"] != "snapshot"):
        raise ValueError("Coverage and selection comparison identities differ")
    changes = changed_lines(root, comparison["base"], comparison["head"])
    changes = {p: lines for p, lines in changes.items() if p not in plan["ignored_files"]}
    if plan["packages"] or plan.get("tests"):
        provenance = json.loads(report.with_name("source.json").read_text())
        expected = {"head": current["merge"], "tree": current["tree"], "dirty": False, "initial_profiles": [],
                    "input_environment": input_environment(current["signature"]),
                    "toolchain": current["signature"]["toolchain"], "features": "all",
                    "target": current["signature"]["target"], "profile": current["signature"]["profile"]}
        if provenance != expected:
            raise ValueError("Coverage report source or execution configuration is stale")
        mappings = read_lcov(root, report)
    else:
        if changes:
            raise ValueError("Rust source changes have no selected test scope")
        mappings = {}
    missing, uncovered, non_executable = [], {}, []
    total = covered = 0
    for path, changed in sorted(changes.items()):
        contents = command(root, "git", "show", f"{current['merge']}:{path}").splitlines()
        if path not in mappings:
            after = "\n".join(contents)
            try:
                before = command(root, "git", "show", f"{comparison['base']}:{path}")
            except AnalysisError:
                before = ""
            if declarations_only(after) or declaration_changes_only(before.rstrip("\n"), after):
                non_executable.append(path)
            else:
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
            "non_executable_files": non_executable,
            "status": "not_applicable" if total == 0 else "passed" if covered * 100 >= target * total else "failed"}


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
    record = current | {"comparison": comparison, "packages": plan["packages"], "tests": plan.get("tests", []), "coverage": coverage,
                        "title_check": {"id": title_check["id"], "head_sha": title_check["head_sha"], "conclusion": "success"}}
    if not valid_coverage(record):
        raise ValueError("Incremental coverage did not succeed")
    return record


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def summarize(comparison, plan, coverage=None):
    return {"comparison": comparison, "packages": plan["packages"], "tests": plan.get("tests", []), "reasons": plan["reasons"],
            "fallback_reasons": plan["fallback_reasons"], "ignored_files": plan["ignored_files"],
            "coverage": coverage}


def accept_plan(root, output, repository, pr, run_id, attempt, target):
    if not isinstance(output, dict) or set(output) != {"plan", "validation"}:
        raise ValueError("Expected one complete Flow result")
    plan, validation = output["plan"], output["validation"]
    if not isinstance(validation, dict) or set(validation) != {"context", "comparison"}:
        raise ValueError("CI requires Flow validation provenance")
    current, comparison = validation["context"], validation["comparison"]
    for field in ("version", "pr", "run_id", "run_attempt", "workflow_id"):
        if type(current[field]) is not int or current[field] <= 0:
            raise ValueError("Invalid validation identity")
    signature = execution_signature(root, target)
    expected = current_context(root, repository, pr,
        {"id": run_id, "run_attempt": attempt, "workflow_id": current["workflow_id"]}, signature)
    if current != expected:
        raise ValueError("Flow provenance does not match this execution")
    verify_current(root, current)
    for field in ("base", "head"):
        if not isinstance(comparison[field], str) or not SHA.fullmatch(comparison[field]):
            raise ValueError("Comparison requires resolved commit identities")
    if (comparison["head"] != current["merge"] or plan["base"] != comparison["base"]
            or plan["head"] != current["merge"] or plan["tree"] != current["tree"]
            or plan["comparison_mode"] != "snapshot"):
        raise ValueError("Flow plan and comparison identities differ")
    if comparison["scope"] == "cumulative":
        if comparison["base"] != current["base"] or comparison["previous"] is not None:
            raise ValueError("Invalid cumulative comparison")
    elif comparison["scope"] == "incremental":
        previous = comparison["previous"]
        if (not isinstance(previous, dict) or previous["merge"] != comparison["base"]
                or any(type(previous[k]) is not int or previous[k] <= 0 for k in ("run_id", "run_attempt"))):
            raise ValueError("Invalid incremental comparison provenance")
    else:
        raise ValueError("Unknown comparison scope")
    if not isinstance(comparison["reason"], str) or not comparison["reason"]:
        raise ValueError("Comparison requires an explanation")
    configuration = {"target": signature["target"], "all_features": True, "features": [], "no_default_features": False}
    environment = input_environment(signature)
    if signature["target"] is not None:
        environment["CARGO_BUILD_TARGET"] = signature["target"]
    if (plan["configuration"] != configuration
            or plan["toolchain"] != signature["toolchain"] or plan["build_environment"] != environment):
        raise ValueError("CI requires a matching Flow execution configuration")
    validate_selection(plan)
    if (not isinstance(plan["packages"], list)
            or not all(isinstance(p, str) and p for p in plan["packages"])
            or not isinstance(plan["ignored_files"], list)):
        raise ValueError("Invalid Flow selection shape")
    return current, comparison, plan


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("accept", "evaluate", "record"))
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--flow-output", type=Path)
    parser.add_argument("--coverage-target", type=float)
    parser.add_argument("--report", type=Path, default=Path("target/coverage/result/lcov.info"))
    args = parser.parse_args()
    try:
        root = Path(command(Path.cwd(), "git", "rev-parse", "--show-toplevel").strip()).resolve()
        directory = args.directory.resolve()
        directory.mkdir(parents=True, exist_ok=True)
        if args.phase == "accept":
            if args.flow_output is None or args.coverage_target is None:
                parser.error("accept requires --flow-output and --coverage-target")
            event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
            pr = int(event["inputs"]["pr"] if os.environ["GITHUB_EVENT_NAME"] == "workflow_dispatch" else event["number"])
            current, comparison, plan = accept_plan(root, json.loads(args.flow_output.read_text()),
                os.environ["GITHUB_REPOSITORY"], pr, int(os.environ["GITHUB_RUN_ID"]),
                int(os.environ["GITHUB_RUN_ATTEMPT"]), args.coverage_target)
            for name, value in (("context", current), ("comparison", comparison), ("plan", plan)):
                write_json(directory / f"{name}.json", value)
            if os.environ.get("GITHUB_OUTPUT"):
                with open(os.environ["GITHUB_OUTPUT"], "a") as output:
                    output.write(f"scoped=true\nrun={str(bool(plan['packages'] or plan['tests'])).lower()}\n")
            summary = summarize(comparison, plan)
        else:
            current, comparison, plan = [json.loads((directory / f"{name}.json").read_text())
                                         for name in ("context", "comparison", "plan")]
            if args.phase == "evaluate":
                coverage = evaluate_coverage(root, current, comparison, plan, args.report)
                write_json(directory / "coverage.json", coverage)
                summary = summarize(comparison, plan, coverage)
            else:
                from ci_monitor import GitHub
                repository = os.environ["GITHUB_REPOSITORY"]
                api = GitHub(repository, os.environ["GH_TOKEN"], os.environ.get("GITHUB_API_URL", "https://api.github.com"))
                run_id, attempt = int(os.environ["GITHUB_RUN_ID"]), int(os.environ["GITHUB_RUN_ATTEMPT"])
                run = api.request(f"/actions/runs/{run_id}/attempts/{attempt}")
                if run["id"] != run_id or run["run_attempt"] != attempt:
                    raise ValueError("Workflow attempt metadata does not match the current run")
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
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
