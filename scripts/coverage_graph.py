#!/usr/bin/env python3
"""Build per-test coverage graphs from instrumented full coverage runs.

Collects the per-test LLVM profiles written by scripts/profraw-wrapper.sh
(one NEXTEST_BINARY_ID__NEXTEST_TEST_NAME key per test, with path separators
encoded and a per-process suffix), merges and exports each
test's line coverage with the pinned toolchain's llvm-profdata/llvm-cov,
assembles the coverage manifest consumed by graph::CoverageManifest, and
indexes it through the mfn-test-affected coverage-graph command. Every test
must attribute and export cleanly; any failure publishes nothing.
"""

import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time


PROFRAW_SUFFIX = re.compile(r"-\d+(?:-\d+)?\.profraw$")
WORK_DIR = "graph-export"
PER_TEST_DIR = "per-test"


def llvm_tools():
    sysroot = subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip()
    verbose = subprocess.check_output(["rustc", "-vV"], text=True)
    host = next(line.split(":", 1)[1].strip() for line in verbose.splitlines()
                if line.startswith("host:"))
    tools = Path(sysroot) / "lib/rustlib" / host / "bin"
    return tools / "llvm-profdata", tools / "llvm-cov"


def toolchain_identity(verbose):
    # The platform-independent toolchain identity bound into graph signatures:
    # line coverage attributes source lines to tests regardless of the host
    # architecture, so only release and commit-hash participate. Consumers on
    # other platforms stay sound through inventory widen-on-mismatch.
    fields = {}
    for line in verbose.splitlines():
        key, separator, value = line.partition(": ")
        if separator:
            fields.setdefault(key, value.strip())
    if not fields.get("release") or not fields.get("commit-hash"):
        raise ValueError("rustc -vV output lacks a release or commit-hash identity")
    return f"rustc {fields['release']} ({fields['commit-hash']})"


def execution_signature(source):
    # Normalized JSON aligned with the coverage identity scripts/run-cov.sh
    # records in result/source.json; see the graph module documentation.
    return json.dumps({
        "environment": source["input_environment"],
        "features": source["features"],
        "profile": source["profile"],
        "target": source["target"],
        "toolchain": toolchain_identity(source["toolchain"]),
    }, sort_keys=True)


def group_profiles(per_test_dir):
    groups = {}
    for path in sorted(per_test_dir.rglob("*.profraw")):
        if path.parent != per_test_dir:
            raise ValueError(f"Per-test profile escaped its flat namespace: {path.name}")
        key = PROFRAW_SUFFIX.sub("", path.name)
        if key == path.name:
            raise ValueError(f"Unrecognized per-test profile name: {path.name}")
        groups.setdefault(key, []).append(path)
    return groups


def profile_key(binary_id, name):
    # LLVM_PROFILE_FILE interprets % tokens, so path separators in binary ids
    # like mf-cli::bin/mf are encoded as @ (neither @ nor / otherwise appears
    # in cargo target ids or Rust test names). scripts/profraw-wrapper.sh
    # applies the same encoding.
    return f"{binary_id}__{name}".replace("/", "@")


def plan_exports(inventory, groups):
    tests = {}
    for binary_id, suite in inventory["rust-suites"].items():
        if suite.get("status") != "listed":
            continue
        for name, case in suite["testcases"].items():
            if case["filter-match"]["status"] == "matches":
                tests[profile_key(binary_id, name)] = {
                    "package": suite["package-name"],
                    "binary": binary_id,
                    "name": name,
                    "binary_path": suite["binary-path"],
                }
    unknown = sorted(key for key in groups if key not in tests)
    if unknown:
        raise ValueError("Per-test profiles do not match the test inventory: "
                         + ", ".join(unknown[:10]))
    exports = []
    for key, profiles in sorted(groups.items()):
        test = tests[key]
        binary = Path(test["binary_path"])
        if not binary.is_file():
            raise ValueError(f"Test binary is missing for {key}: {binary}")
        exports.append({"key": key, "profiles": profiles, "binary_path": binary,
                        "package": test["package"], "binary": test["binary"],
                        "name": test["name"]})
    return exports


def export_test(tools, work_dir, export, workspace_root):
    profdata, cov = tools
    merged = work_dir / "profdata" / f"{export['key']}.profdata"
    lcov = work_dir / "lcov" / f"{export['key']}.lcov"
    merge = subprocess.run(
        [str(profdata), "merge", "-sparse", "--num-threads=1",
         *[str(profile) for profile in export["profiles"]], "-o", str(merged)],
        capture_output=True, text=True)
    if merge.returncode:
        raise ValueError(f"llvm-profdata merge failed for {export['key']}: "
                         + merge.stderr.strip().splitlines()[-1])
    with open(lcov, "w") as output:
        result = subprocess.run(
            [str(cov), "export", "--format=lcov", "-object", str(export["binary_path"]),
             f"--instr-profile={merged}", "--skip-functions", "--skip-branches",
             "--num-threads=1",
             f"--include-filename-regex=^{re.escape(str(workspace_root) + os.sep)}"],
            stdout=output, stderr=subprocess.PIPE, text=True)
    if result.returncode:
        raise ValueError(f"llvm-cov export failed for {export['key']}: "
                         + result.stderr.strip().splitlines()[-1])
    return export


def write_manifest(work_dir, source, exports):
    if not source.get("head"):
        raise ValueError("Coverage graph collection requires a git HEAD identity")
    manifest = {
        "commit_sha": source["head"],
        "run_id": os.environ.get("GITHUB_RUN_ID", "local"),
        "attempt": int(os.environ.get("GITHUB_RUN_ATTEMPT", "1")),
        "signature": execution_signature(source),
        "tests": [{"package": export["package"], "binary": export["binary"],
                   "name": export["name"], "lcov": f"lcov/{export['key']}.lcov"}
                  for export in exports],
    }
    path = work_dir / "manifest.json"
    path.write_text(json.dumps(manifest, indent=2) + "\n")
    return path


def build_indexer(root):
    build = subprocess.run(
        ["cargo", "build", "--offline", "--locked", "-p", "mfn-test-affected",
         "--bin", "coverage-graph", "--message-format", "json"],
        cwd=root, capture_output=True, text=True)
    if build.returncode:
        sys.stderr.write(build.stderr)
        raise ValueError("could not build the coverage-graph indexer")
    for line in build.stdout.splitlines():
        message = json.loads(line)
        target = message.get("target", {})
        if (message.get("reason") == "compiler-artifact" and "bin" in target.get("kind", [])
                and target.get("name") == "coverage-graph" and message.get("executable")):
            return Path(message["executable"])
    raise ValueError("coverage-graph indexer artifact is missing")


def index(root, indexer, manifest, output, work_dir):
    # The indexer is built inside the instrumented coverage target; keep its
    # own profile output in the scratch directory, away from grcov's inputs.
    environment = os.environ | {"LLVM_PROFILE_FILE": str(work_dir / "indexer-%p.profraw")}
    result = subprocess.run(
        [str(indexer), "index", "--workspace-root", str(root), str(manifest), str(output)],
        cwd=root, capture_output=True, text=True, env=environment)
    for stale in work_dir.glob("indexer-*.profraw"):
        stale.unlink()
    if result.returncode:
        sys.stderr.write(result.stderr)
        raise ValueError("coverage graph indexing failed")
    return json.loads(result.stdout)


def write_metadata(output, manifest_path, summary):
    manifest = json.loads(manifest_path.read_text())
    metadata = {
        "commit_sha": manifest["commit_sha"],
        "run_id": manifest["run_id"],
        "attempt": manifest["attempt"],
        "schema_version": summary["schema_version"],
        "signature": manifest["signature"],
    }
    destination = output.with_suffix(".metadata.json")
    destination.write_text(json.dumps(metadata, indent=2) + "\n")
    return destination


def build(coverage_dir, output):
    root = Path(subprocess.check_output(
        ["git", "rev-parse", "--show-toplevel"], text=True).strip()).resolve()
    # Graphs are write-once per commit; drop stale local outputs so repeated
    # runs at the same commit rebuild instead of failing on the duplicate.
    output.unlink(missing_ok=True)
    output.with_suffix(".metadata.json").unlink(missing_ok=True)
    inventory = json.loads((coverage_dir / "inventory.json").read_text())
    source = json.loads((coverage_dir / "result/source.json").read_text())
    work_dir = coverage_dir / WORK_DIR
    (work_dir / "profdata").mkdir(parents=True, exist_ok=True)
    (work_dir / "lcov").mkdir(parents=True, exist_ok=True)
    exports = plan_exports(inventory, group_profiles(coverage_dir / PER_TEST_DIR))
    if not exports:
        raise ValueError("Per-test coverage collection produced no profiles")
    tools = llvm_tools()
    started = time.monotonic()
    with ThreadPoolExecutor(max_workers=min(10, os.cpu_count() or 1)) as pool:
        list(pool.map(lambda export: export_test(tools, work_dir, export, root), exports))
    export_seconds = time.monotonic() - started
    manifest = write_manifest(work_dir, source, exports)
    output.parent.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    indexer = build_indexer(root)
    build_seconds = time.monotonic() - started
    started = time.monotonic()
    summary = index(root, indexer, manifest, output, work_dir)
    index_seconds = time.monotonic() - started
    metadata = write_metadata(output, manifest, summary)
    print(json.dumps({"tests": summary["tests"], "files": summary["files"],
                      "ranges": summary["ranges"], "export_seconds": round(export_seconds, 3),
                      "build_seconds": round(build_seconds, 3),
                      "index_seconds": round(index_seconds, 3),
                      "graph": str(output), "metadata": str(metadata)}, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("phase", choices=("build",))
    parser.add_argument("--coverage-dir", type=Path, default=Path("target/coverage"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    build(args.coverage_dir, args.output.resolve())


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
