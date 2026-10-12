#!/usr/bin/env python3

from pathlib import Path, PurePosixPath
import subprocess


def requires_full_suite(paths):
    # LLVM does not map build inputs, proc macros, or external fixture code to tests.
    return any(
        PurePosixPath(path).suffix != ".rs"
        or PurePosixPath(path).name == "build.rs"
        or path.startswith("crates/mf-runtime-derive/")
        or "/tests/fixtures/" in path
        for path in paths
    )


def use_full_suite(baseline):
    if not baseline.is_file():
        return True
    result = subprocess.run(
        ["git", "diff", "--name-only", "--no-renames", "-z", baseline.read_text().strip(), "--"],
        capture_output=True,
        check=False,
    )
    if result.returncode:
        return True
    return requires_full_suite(result.stdout.decode().rstrip("\0").split("\0") if result.stdout else [])


def main():
    affected = Path("target/affected")
    affected.mkdir(parents=True, exist_ok=True)
    full = use_full_suite(affected / "baseline.sha")
    print("Running full suite: no baseline or coverage cannot track changed inputs." if full
          else "Running affected tests against the nightly baseline.", flush=True)
    subprocess.run(["cargo", "fetch", "--locked"], check=True)
    # Restored artifacts can be newer than checkout mtimes; retain only dependency builds.
    subprocess.run(["cargo", "clean", "--workspace", "--profile", "dev"], check=True)
    command = ["cargo", "affected", "run", "--report-json", str(affected / "report.json")]
    if full:
        command.append("--all")
    subprocess.run(command + [
        "--", "--locked", "--workspace", "--all-targets", "--all-features", "--profile", "ci", "--no-fail-fast",
    ], check=True)


if __name__ == "__main__":
    main()
