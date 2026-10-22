#!/usr/bin/env python3
"""Resolve semantic test selections and publish only complete crate reports."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("test_runner", ROOT / "tools/test-affected/run-tests.py")
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


def validate_selection(plan):
    packages, tests = plan["packages"], plan.get("tests", [])
    baseline = plan.get("baseline_packages", packages)
    for names in (packages, baseline):
        if (not isinstance(names, list)
                or not all(isinstance(p, str) and re.fullmatch(r"[A-Za-z0-9_-]+", p) for p in names)
                or len(set(names)) != len(names)):
            raise ValueError("Invalid package selection")
    if not set(packages) <= set(baseline) or not isinstance(tests, list):
        raise ValueError("Invalid selection baseline")
    for test in tests:
        if (not isinstance(test, dict) or set(test) != {"package", "binary", "name"}
                or not all(isinstance(v, str) and v for v in test.values())
                or test["package"] not in baseline):
            raise ValueError("Invalid exact test identity")
    if len({tuple(sorted(test.items())) for test in tests}) != len(tests):
        raise ValueError("Duplicate exact test identity")
    return packages, tests, baseline


def resolve(plan, inventory):
    packages, tests, baseline = validate_selection(plan)
    available = runner.identities(inventory)
    # Ignored cases are never part of the runnable selection.
    runnable = {
        (suite["package-name"], binary, name)
        for binary, suite in inventory["rust-suites"].items()
        for name, case in suite["testcases"].items()
        if case["filter-match"]["status"] == "matches" and not case["ignored"]
    }
    exact = {(t["package"], t["binary"], t["name"]) for t in tests}
    fallback = None
    if not exact <= runnable:
        packages, tests = baseline, []
        fallback = "exact identities are absent from the runnable head inventory"
    if len(runner.filterset(packages, tests).encode()) > 64 * 1024:
        packages, tests = baseline, []
        fallback = "filter expression exceeded its size budget"
    selected = set(packages) | {test["package"] for test in tests}
    complete = set(packages)
    for package in selected:
        required = {key for key in runnable if key[0] == package}
        if required and required <= exact:
            complete.add(package)
    filters = {
        package: runner.filterset([package] if package in complete else [],
                                  [test for test in tests if test["package"] == package])
        for package in sorted(selected)
    }
    return {"packages": sorted(selected), "complete_packages": sorted(complete),
            "partial_packages": sorted(selected - complete), "filters": filters,
            "fallback": fallback, "completed": [], "available_count": len(available)}


def publish(root, directory, selection, metadata):
    members = set(metadata["workspace_members"])
    owners = sorted(((Path(p["manifest_path"]).resolve().parent, p["name"] if p["id"] in members else None)
                     for p in metadata["packages"]),
                    key=lambda item: len(item[0].parts), reverse=True)
    complete = set(selection["complete_packages"]) & set(selection["completed"])
    reports = {package: [] for package in complete}
    for block in (directory / "lcov.info").read_text().split("end_of_record"):
        sources = [line[3:] for line in block.splitlines() if line.startswith("SF:")]
        if not sources:
            continue
        if len(sources) != 1:
            raise ValueError("Invalid LCOV source record")
        source = (root / sources[0]).resolve()
        owner = next((name for path, name in owners if source.is_relative_to(path)), None)
        if owner in reports:
            reports[owner].append(block.strip() + "\nend_of_record\n")
    destination = directory / "crates"
    destination.mkdir(exist_ok=True)
    for previous in destination.glob("*.lcov"):
        previous.unlink()
    uploaded = []
    for package, blocks in sorted(reports.items()):
        if blocks:
            (destination / f"{package}.lcov").write_text("".join(blocks))
            uploaded.append(package)
    summary = selection | {"report_packages": uploaded}
    (directory / "selection.json").write_text(json.dumps(summary, indent=2) + "\n")
    outputs("complete_packages", uploaded)


def outputs(name, value):
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"{name}={json.dumps(value)}\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("packages", "resolve", "filter", "complete", "report"))
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--directory", type=Path, default=Path("target/coverage"))
    parser.add_argument("--package")
    parser.add_argument("--partial", action="store_true")
    args = parser.parse_args()
    directory = args.directory
    if args.phase == "packages":
        plan = json.loads(args.plan.read_text())
        packages, tests, baseline = validate_selection(plan)
        print("\n".join(baseline if tests else packages))
        return
    selection_file = directory / "selection.json"
    if args.phase == "resolve":
        metadata = json.loads((directory / "workspace.json").read_text())
        names = [p["name"] for p in metadata["packages"] if p["id"] in metadata["workspace_members"]]
        plan = json.loads(args.plan.read_text()) if args.plan else {"packages": names}
        packages, tests, baseline = validate_selection(plan)
        if not set(baseline) <= set(names):
            raise ValueError("Selection does not match workspace packages")
        selection = resolve(plan, json.loads((directory / "inventory.json").read_text()))
        selection_file.write_text(json.dumps(selection, indent=2) + "\n")
        outputs("packages", selection["packages"])
        return
    selection = json.loads(selection_file.read_text())
    if args.phase == "filter":
        print(selection["filters"][args.package])
    elif args.phase == "complete":
        packages = [args.package] if args.package else selection["packages"]
        for package in packages:
            if package not in selection["filters"]:
                raise ValueError("Package was not prepared")
            if package not in selection["completed"]:
                selection["completed"].append(package)
            if args.partial and package in selection["complete_packages"]:
                selection["complete_packages"].remove(package)
                selection["partial_packages"].append(package)
        selection_file.write_text(json.dumps(selection, indent=2) + "\n")
    else:
        if set(selection["completed"]) != set(selection["packages"]):
            raise ValueError("Selected package tests have not all succeeded")
        publish(Path.cwd(), directory / "result", selection,
                json.loads((directory / "cargo-metadata.json").read_text()))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
