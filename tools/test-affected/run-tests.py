#!/usr/bin/env python3
"""Validate and execute a test-affected plan without invoking its analysis Flow."""

import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time


class PlanError(Exception):
    pass


def execute(argv, root, capture=True):
    process = subprocess.Popen(argv, cwd=root, stdout=subprocess.PIPE if capture else sys.stderr,
                               stderr=subprocess.PIPE if capture else None, start_new_session=True)
    try:
        output, errors = process.communicate()
    except BaseException:
        try:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=2)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            pass
        # A reaped group leader does not imply that its descendants have exited.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
        raise
    if process.returncode:
        if capture:
            sys.stderr.write(os.fsdecode(errors))
        raise subprocess.CalledProcessError(process.returncode, argv)
    return os.fsdecode(output) if capture else ""


def load_plan(path):
    data = json.load(sys.stdin) if Path(path) == Path("-") else json.loads(Path(path).read_text())
    if isinstance(data, dict) and set(data) in ({"plan"}, {"plan", "validation"}):
        data = data["plan"]
    if isinstance(data, dict):
        data.setdefault("ignored_files", [])
        data.setdefault("comparison_mode", "merge-base")
    required = {"comparison_mode", "ignored_files", "repository", "base", "head", "snapshot", "tree", "configuration", "toolchain", "build_environment",
                "baseline_packages", "packages", "tests", "reasons", "fallback_reasons", "analysis_status",
                "baseline_count", "selected_count", "observed_relationships", "analysis_ms", "planning_ms"}
    if not isinstance(data, dict) or set(data) != required:
        raise PlanError("expected a complete test-affected plan or its Flow output wrapper")
    if data["comparison_mode"] not in ("merge-base", "snapshot", "worktree", "commit"):
        raise PlanError("invalid comparison mode")
    for name in ("base", "head", "tree"):
        if not isinstance(data[name], str) or not re.fullmatch(r"[0-9a-f]{40}(?:[0-9a-f]{24})?", data[name]):
            raise PlanError(f"invalid {name} identity")
    for name in ("snapshot", "repository"):
        if not isinstance(data[name], str) or not Path(data[name]).is_absolute():
            raise PlanError(f"invalid {name} path")
    for name in ("baseline_packages", "packages"):
        if not isinstance(data[name], list) or not all(isinstance(p, str) and re.fullmatch(r"[A-Za-z0-9_-]+", p) for p in data[name]):
            raise PlanError(f"invalid {name}")
        if len(set(data[name])) != len(data[name]):
            raise PlanError(f"duplicate {name}")
    if not set(data["packages"]) <= set(data["baseline_packages"]):
        raise PlanError("selected packages exceed the recorded baseline")
    if not isinstance(data["tests"], list):
        raise PlanError("invalid exact test list")
    for test in data["tests"]:
        if not isinstance(test, dict) or set(test) != {"package", "binary", "name"} or not all(isinstance(v, str) and v for v in test.values()):
            raise PlanError("invalid exact test identity")
        if test["package"] not in data["baseline_packages"]:
            raise PlanError("exact test exceeds the recorded baseline")
    configuration = data["configuration"]
    if not isinstance(configuration, dict) or set(configuration) != {"target", "all_features", "features", "no_default_features"}:
        raise PlanError("invalid build configuration")
    if configuration["target"] is not None and not isinstance(configuration["target"], str):
        raise PlanError("invalid target")
    if type(configuration["all_features"]) is not bool or type(configuration["no_default_features"]) is not bool:
        raise PlanError("invalid feature flags")
    if not isinstance(configuration["features"], list) or not all(isinstance(f, str) and f and not f.startswith('-') for f in configuration["features"]):
        raise PlanError("invalid features")
    if configuration["all_features"] and (configuration["features"] or configuration["no_default_features"]):
        raise PlanError("conflicting feature configuration")
    if not isinstance(data["build_environment"], dict) or not all(isinstance(k, str) and isinstance(v, str) for k, v in data["build_environment"].items()):
        raise PlanError("invalid build environment")
    if len({tuple(sorted(t.items())) for t in data["tests"]}) != len(data["tests"]):
        raise PlanError("duplicate exact test identity")
    return data


def verify(plan):
    root = Path(plan["snapshot"])
    if not root.is_dir():
        raise PlanError("snapshot has been removed")
    if Path(execute(["git", "rev-parse", "--show-toplevel"], root).strip()).resolve() != root.resolve():
        raise PlanError("snapshot is not an independent worktree root")
    if execute(["git", "rev-parse", "HEAD"], root).strip() != plan["head"]:
        raise PlanError("snapshot revision changed")
    if execute(["git", "rev-parse", "HEAD^{tree}"], root).strip() != plan["tree"]:
        raise PlanError("snapshot tree changed")
    if execute(["git", "status", "--porcelain", "--untracked-files=all"], root):
        raise PlanError("snapshot is dirty")
    if execute(["rustc", "-vV"], root) != plan["toolchain"]:
        raise PlanError("toolchain changed")
    expected_environment = {name: os.environ[name] for name in (
        "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET", "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG", "CARGO_INCREMENTAL"
    ) if name in os.environ}
    if plan["build_environment"] != expected_environment:
        raise PlanError("build environment changed")
    return root


def matcher(value):
    # Equality patterns have their own escaping grammar; they are not JSON strings.
    return "=" + "".join(c if c.isascii() and (c.isalnum() or c in "_:-.") else f"\\u{{{ord(c):x}}}" for c in value)


def filterset(packages, tests):
    parts = [f"package({matcher(p)})" for p in packages]
    parts += [f"(package({matcher(t['package'])}) & binary_id({matcher(t['binary'])}) & test({matcher(t['name'])}))" for t in tests]
    return " | ".join(parts) or "none()"


def cargo_arguments(plan, operation):
    args = ["cargo", "nextest", operation, "--locked", "--all-targets", "--profile", "default", "--user-config-file", "none"]
    # Older snapshots can otherwise reuse newer workspace binaries from a shared target.
    args += ["--target-dir", str(Path(plan["snapshot"]) / "target/test-affected")]
    configuration = plan["configuration"]
    if configuration["target"]:
        args += ["--target", configuration["target"]]
    if configuration["all_features"]:
        args += ["--all-features"]
    if configuration["features"]:
        args += ["--features", ",".join(configuration["features"])]
    if configuration["no_default_features"]:
        args += ["--no-default-features"]
    for package in plan["baseline_packages"]:
        args += ["-p", package]
    return args


def identities(inventory):
    suites = inventory.get("rust-suites")
    if not isinstance(suites, dict):
        raise PlanError("invalid nextest inventory")
    return {(suite["package-name"], binary, name)
            for binary, suite in suites.items() for name, case in suite["testcases"].items()
            if case["filter-match"]["status"] == "matches"}


def consume(plan, list_only=False, inventory_out=None):
    root = verify(plan)
    if not plan["packages"] and not plan["tests"]:
        print(json.dumps({"selected_count": 0, "executed": False, "reason": "empty plan"}))
        return 0
    metadata = json.loads(execute(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], root))
    members = set(metadata["workspace_members"])
    package_names = {p["name"] for p in metadata["packages"] if p["id"] in members}
    if not set(plan["baseline_packages"]) <= package_names:
        raise PlanError("baseline packages do not match the snapshot")
    started = time.monotonic()
    inventory = json.loads(execute(cargo_arguments(plan, "list") + ["--message-format", "json"], root))
    list_seconds = time.monotonic() - started
    if inventory_out:
        Path(inventory_out).write_text(json.dumps({"head": plan["head"], "configuration": plan["configuration"],
                                                 "toolchain": plan["toolchain"], "build_environment": plan["build_environment"], "inventory": inventory}, indent=2) + "\n")
    available = identities(inventory)
    exact = {(t["package"], t["binary"], t["name"]) for t in plan["tests"]}
    packages, tests = plan["packages"], plan["tests"]
    fallback = None
    if not exact <= available:
        packages, tests = plan["baseline_packages"], []
        fallback = "exact identities are absent from the runnable head inventory"
    expression = filterset(packages, tests)
    if len(expression.encode()) > 64 * 1024:
        packages, tests = plan["baseline_packages"], []
        expression = filterset(packages, tests)
        fallback = "filter expression exceeded its size budget"
    selected_exact = exact if tests else set()
    selected = {key for key in available if key[0] in packages or key in selected_exact}
    # Listing can execute build scripts, so check source identity again before running tests.
    verify(plan)
    report = {"selected_count": len(selected), "inventory_seconds": round(list_seconds, 3),
              "fallback": fallback, "filterset": expression, "executed": False}
    if selected and not list_only:
        started = time.monotonic()
        execute(cargo_arguments(plan, "run") + ["-E", expression, "--no-tests", "fail"], root, capture=False)
        report.update(executed=True, test_seconds=round(time.monotonic() - started, 3))
    print(json.dumps(report))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("plan", type=Path, help="Plan path, or - to read JSON from stdin")
    parser.add_argument("--list-only", action="store_true")
    parser.add_argument("--inventory-out", type=Path)
    args = parser.parse_args()
    def interrupt(_signal, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupt)
    try:
        return consume(load_plan(args.plan), args.list_only, args.inventory_out)
    except (PlanError, ValueError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as error:
        return error.returncode if error.returncode > 0 else 128 - error.returncode
    except KeyboardInterrupt:
        print("cancelled", file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
