#!/usr/bin/env python3

import argparse
import json
from pathlib import Path, PurePosixPath
import subprocess
import tomllib


def is_documentation(path):
    file = PurePosixPath(path)
    return (
        path in {"AGENTS.md", "README.md"}
        or (file.suffix == ".md" and file.parts[0] in {"docs", "openspec"})
        or (file.name == ".openspec.yaml" and file.parts[0] == "openspec")
    )


def mapped_inputs(manifest):
    with manifest.open("rb") as source:
        rules = tomllib.load(source).get("workspace", {}).get("metadata", {}).get("affected", {}).get("rule", [])
    # Only exact paths exempt external inputs; broad globs cannot certify new consumers.
    return {path for rule in rules for path in rule["globs"] if not any(char in path for char in "*?[]{}\\")}


def requires_full_suite(paths, inputs=()):
    # LLVM does not map build inputs, proc macros, or external fixture code to tests.
    return any(
        PurePosixPath(path).name == "build.rs"
        or path.startswith("crates/mf-runtime-derive/")
        or path.startswith((".cargo/", ".config/", ".github/", "scripts/"))
        or path in {"Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "flake.nix", "flake.lock"}
        or (path not in inputs and (
            PurePosixPath(path).suffix != ".rs"
            or "/tests/fixtures/" in path
            or path.startswith("examples/")
        ))
        for path in paths if not is_documentation(path)
    )


def full_suite_reason(baseline):
    if not baseline.is_file():
        return "baseline.sha is missing"
    result = subprocess.run(
        ["git", "diff", "--name-only", "--no-renames", "-z", baseline.read_text().strip(), "--"],
        capture_output=True,
        check=False,
    )
    if result.returncode:
        return "baseline commit is unavailable"
    paths = result.stdout.decode().rstrip("\0").split("\0") if result.stdout else []
    mapped = mapped_inputs(Path("Cargo.toml"))
    inputs = [path for path in paths if requires_full_suite([path], mapped)]
    return "changed inputs: " + ", ".join(inputs) if inputs else None


def collect_baseline(affected, reason, nextest_args):
    report_path = affected / "report.json"
    report = {}
    incremental = False
    if reason is None:
        subprocess.run([
            "cargo", "affected", "status", "--report-json", str(report_path), "--", *nextest_args,
        ], check=True)
        report = json.loads(report_path.read_text())
        status = report["cache"]["status"]
        incremental = status in {"hit-exact", "hit-with-divergence"}
        reason = "compatible baseline" if incremental else status
    mode = "incremental" if incremental else "full"
    report["collection"] = {"mode": mode, "reason": reason}
    report_path.write_text(json.dumps(report, indent=2) + "\n")
    print(f"Collecting {mode} baseline: {reason}.", flush=True)
    command = ["cargo", "affected", "collect"]
    if incremental:
        command.append("--diff")
    subprocess.run([*command, "--", *nextest_args], check=True)
    sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True)
    (affected / "baseline.sha").write_text(sha)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--collect", action="store_true", help="Refresh the baseline instead of generating aggregate coverage")
    arguments = parser.parse_args()
    affected = Path("target/affected")
    affected.mkdir(parents=True, exist_ok=True)
    reason = full_suite_reason(affected / "baseline.sha")
    nextest_args = ["--locked", "--workspace", "--all-targets", "--all-features", "--profile", "ci", "--no-fail-fast"]
    if arguments.collect:
        collect_baseline(affected, reason, nextest_args)
        return
    print(f"Running full suite: {reason}." if reason is not None
          else "Running affected tests against the baseline.", flush=True)
    command = ["cargo", "affected", "run", "--report-json", str(affected / "report.json")]
    if reason is not None:
        command.append("--all")
    subprocess.run([*command, "--", *nextest_args], check=True)


if __name__ == "__main__":
    main()
