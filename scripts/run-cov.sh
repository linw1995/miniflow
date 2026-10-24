#!/usr/bin/env bash

set -euo pipefail

workspace_root="$(pwd -P)"
coverage_dir="${workspace_root}/target/coverage"
coverage_input_environment="$(python3 - <<'PYTHON'
import json, os
print(json.dumps({name: os.environ[name] for name in (
    "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_INCREMENTAL",
    "CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG",
) if name in os.environ}))
PYTHON
)"
export CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR="${coverage_dir}/build"
export RUSTFLAGS="-Cinstrument-coverage -Ccodegen-units=1 -Copt-level=0"
export LLVM_PROFILE_FILE="${coverage_dir}/data/mf-%p-%m.profraw"
export MF_COVERAGE_BINARIES_DIR="${coverage_dir}/extra-binaries"
report_format="${MF_COVERAGE_REPORT_FORMAT:-full}"
case "$report_format" in
  full|lcov) ;;
  *) echo "error: unknown coverage report format: $report_format" >&2; exit 1 ;;
esac

report_timing() {
  printf 'Coverage timing (%s): %ss\n' "$1" "$((SECONDS - $2))" >&2
}

# Full coverage runs may opt into per-test profile collection for the
# coverage graph; --coverage-graph must precede every other argument.
coverage_graph=""
if [[ "${1:-}" == "--coverage-graph" ]]; then
  coverage_graph="${2:?--coverage-graph requires an output SQLite path}"
  shift 2
fi

coverage_graph_requested() {
  [[ -f "${coverage_dir}/coverage-graph.path" ]]
}

activate_per_test_profiles() {
  MF_COVERAGE_PER_TEST_DIR="${coverage_dir}/per-test"
  export MF_COVERAGE_PER_TEST_DIR
  mkdir -p "$MF_COVERAGE_PER_TEST_DIR"
}

build_coverage_graph() {
  python3 -B scripts/coverage_graph.py build --coverage-dir "$coverage_dir" --output "$1"
}

mark_target_directory() {
  # Cargo may omit tags under another cache directory, but clean requires one.
  mkdir -p "$1"
  printf '%s\n' 'Signature: 8a477f597d28d172789f06886806bc55' > "$1/CACHEDIR.TAG"
}

mark_fixture_target() {
  if [[ -n "${MF_TEST_BUILD_CACHE_DIR:-}" ]]; then
    mark_target_directory "${MF_TEST_BUILD_CACHE_DIR}/target"
  fi
}

reset_reports() {
  rm -rf "${coverage_dir}/data" "${coverage_dir}/result" "$MF_COVERAGE_BINARIES_DIR"
  rm -rf "${coverage_dir}/per-test" "${coverage_dir}/graph-export"
  rm -f "${coverage_dir}/binaries.json" "${coverage_dir}/cargo-metadata.json"
  rm -f "${coverage_dir}/selection.json" "${coverage_dir}/inventory.json"
  rm -f "${coverage_dir}/coverage-graph.path"
  mkdir -p "${coverage_dir}/data" "${coverage_dir}/result" "$MF_COVERAGE_BINARIES_DIR"
  MF_TEST_BUILD_CACHE_SESSION="$(python3 -c 'import uuid; print(uuid.uuid4())')"
  export MF_TEST_BUILD_CACHE_SESSION
  printf '%s\n' "$MF_TEST_BUILD_CACHE_SESSION" > "${coverage_dir}/build-cache-session"
}

prepare_coverage() {
  # Offline fixture builds resolve the complete lockfile, including other targets.
  cargo fetch --locked
  cargo metadata --offline --locked --no-deps --format-version 1 > "${coverage_dir}/workspace.json"
  rustc -vV > "${coverage_dir}/identity.current"
  printf '%s\n' "$RUSTFLAGS" >> "${coverage_dir}/identity.current"
  jq -r '.workspace_members[]' "${coverage_dir}/workspace.json" >> "${coverage_dir}/identity.current"
  if ! cmp -s "${coverage_dir}/identity.current" "${CARGO_TARGET_DIR}/.mf-coverage-identity"; then
    rm -rf "${CARGO_TARGET_DIR}"
  fi
  mkdir -p "${CARGO_TARGET_DIR}"
  mv "${coverage_dir}/identity.current" "${CARGO_TARGET_DIR}/.mf-coverage-identity"
  mark_target_directory "$CARGO_TARGET_DIR"
  mark_fixture_target

  # grcov scans every binary in its search directory. Remove old workspace mappings while retaining dependencies.
  cargo clean --workspace --profile dev
  python3 - "$coverage_input_environment" <<'PYTHON'
import json
import os
from pathlib import Path
import subprocess
import sys

def git(*args):
    return subprocess.check_output(["git", *args], stderr=subprocess.DEVNULL, text=True).strip()

head = tree = None
dirty = True
initial_profiles = []
try:
    if Path(git("rev-parse", "--show-toplevel")).resolve() == Path.cwd().resolve():
        head, tree = git("rev-parse", "HEAD"), git("rev-parse", "HEAD^{tree}")
        dirty = bool(git("status", "--porcelain", "--untracked-files=all"))
        initial_profiles = [p for p in os.fsdecode(subprocess.check_output([
            "git", "ls-files", "--others", "--exclude-standard", "-z", "--", "*.profraw"
        ])).split("\0") if p]
except subprocess.CalledProcessError:
    pass
identity = {"head": head, "tree": tree, "dirty": dirty, "initial_profiles": initial_profiles,
            "input_environment": json.loads(sys.argv[1]),
            "toolchain": subprocess.check_output(["rustc", "-vV"], text=True),
            "features": "all", "target": os.environ.get("CARGO_BUILD_TARGET"),
            "profile": os.environ.get("NEXTEST_PROFILE", "default")}
Path("target/coverage/result/source.json").write_text(json.dumps(identity) + "\n")
PYTHON
}

collect_profiles() {
  # Credential-isolated subprocesses may clear LLVM_PROFILE_FILE. Only collect
  # outputs created during this preparation session, never pre-existing profiles.
  python3 - "$coverage_dir" <<'PYTHON'
import json
import os
from pathlib import Path
import shutil
import subprocess
import uuid
import sys

coverage = Path(sys.argv[1])
identity = json.loads((coverage / "result/source.json").read_text())
if identity["head"] is not None:
    initial = set(identity["initial_profiles"])
    paths = os.fsdecode(subprocess.check_output([
        "git", "ls-files", "--others", "--exclude-standard", "-z", "--", "*.profraw"
    ])).split("\0")
    for name in filter(None, paths):
        source = Path(name)
        if name in initial or source.is_symlink() or not source.is_file():
            continue
        if source.name == "default.profraw" or source.name.startswith("default_"):
            shutil.move(source, coverage / "data" / f"subprocess-{uuid.uuid4().hex}.profraw")
PYTHON
}

report_coverage() {
  local started=$SECONDS
  collect_profiles
  report_timing collect-profiles "$started"
  started=$SECONDS
  # Compiler-cache backups can contain coverage maps from older source revisions.
  # Keep the build cache intact and expose only published Cargo artifacts to grcov.
  rm -rf "${coverage_dir}/report-binaries"
  python3 - "${CARGO_TARGET_DIR}/debug" "$MF_COVERAGE_BINARIES_DIR" "${coverage_dir}/report-binaries" <<'PYTHON'
import json
import os
from pathlib import Path
import shutil
import sys

build, extra, destination = map(Path, sys.argv[1:])
for name, source in [("deps", build / "deps"), ("build", build / "build"), ("fixtures", extra)]:
    if source.is_dir():
        shutil.copytree(source, destination / name, copy_function=os.link,
                        ignore=shutil.ignore_patterns(".kache-prestage-*"))
mappings = {}
for manifest in sorted(extra.glob("*.paths.json")):
    for source, target in json.loads(manifest.read_text()).items():
        if source in mappings and mappings[source] != target:
            raise ValueError(f"Conflicting coverage source mapping: {source}")
        mappings[source] = target
(destination.parent / "path-mapping.json").write_text(json.dumps(mappings))
PYTHON
  report_timing prepare-binaries "$started"
  grcov_inputs=("${coverage_dir}/data")
  # Per-test profiles replace the shared data/ profiles on graph runs; merge
  # both directories so the aggregate report keeps its current meaning.
  if coverage_graph_requested; then
    grcov_inputs+=("${coverage_dir}/per-test")
  fi
  local formats=html,cobertura,lcov,markdown destination="${coverage_dir}/result"
  if [[ "$report_format" == lcov ]]; then
    formats=lcov
    destination="${coverage_dir}/result/lcov"
  fi
  started=$SECONDS
  grcov "${grcov_inputs[@]}" \
    --llvm \
    --branch \
    --source-dir "${workspace_root}" \
    --ignore-not-existing \
    --ignore '../*' \
    --ignore '/*' \
    --binary-path "${coverage_dir}/report-binaries" \
    --path-mapping "${coverage_dir}/path-mapping.json" \
    --output-types "$formats" \
    --output-path "$destination"
  report_timing grcov "$started"

  test -s "${coverage_dir}/result/lcov"
  if [[ "$report_format" == full ]]; then
    test -s "${coverage_dir}/result/cobertura.xml"
  fi
  cp "${coverage_dir}/result/lcov" "${coverage_dir}/result/lcov.info"
  started=$SECONDS
  python3 -B scripts/coverage_selection.py report
  rm -rf "${coverage_dir}/report-binaries"
  report_timing publish-reports "$started"
}

# CI keeps one instrumented build and accumulates profiles across package steps.
case "${1:-}" in
  --test-package)
    [[ $# -ge 2 && -n "$2" ]]
    selected_package="$2"
    shift 2
    if coverage_graph_requested; then
      activate_per_test_profiles
    fi
    expression="$(python3 -B scripts/coverage_selection.py filter --package "$selected_package")"
    MF_TEST_BUILD_CACHE_SESSION="$(cat "${coverage_dir}/build-cache-session")"
    export MF_TEST_BUILD_CACHE_SESSION
    # Some packages, such as derive macros, are tested only by their consumers.
    status=0
    cargo nextest run \
      --cargo-metadata "${coverage_dir}/cargo-metadata.json" \
      --binaries-metadata "${coverage_dir}/binaries.json" \
      -E "$expression" --no-tests warn "$@" || status=$?
    collect_profiles
    if [[ "$status" -eq 0 ]]; then
      completion=()
      if [[ $# -gt 0 && "$*" != "--run-ignored all" ]]; then
        completion+=(--partial)
      fi
      python3 -B scripts/coverage_selection.py complete --package "$selected_package" "${completion[@]}"
    fi
    exit "$status"
    ;;
  --prune-cache)
    [[ $# -eq 1 ]]
    # Reporting must finish before its instrumented binaries are removed.
    test -s "${coverage_dir}/result/lcov.info"
    targets=("$CARGO_TARGET_DIR")
    if [[ -n "${MF_TEST_BUILD_CACHE_DIR:-}" ]]; then
      targets+=("${MF_TEST_BUILD_CACHE_DIR}/target" "${MF_TEST_BUILD_CACHE_DIR}/packaged-target")
      targets+=("${MF_TEST_BUILD_CACHE_DIR}/optimized/target")
      rm -f "${MF_TEST_BUILD_CACHE_DIR}/prepared-session.json"
      rm -f "${MF_TEST_BUILD_CACHE_DIR}/optimized/prepared-session.json"
    fi
    python3 scripts/prune-test-cache.py "${targets[@]}"
    rm -f "${coverage_dir}/binaries.json" "${coverage_dir}/cargo-metadata.json"
    exit 0
    ;;
  --report)
    [[ $# -eq 1 ]]
    report_coverage
    if coverage_graph_requested; then
      build_coverage_graph "$(cat "${coverage_dir}/coverage-graph.path")"
    fi
    exit 0
    ;;
esac

phase=full
if [[ "${1:-}" == "--prepare" ]]; then
  phase=prepare
  shift
fi
reset_reports
test_packages=(--workspace)
selection_plan=()
if [[ "${1:-}" == "--affected-plan" ]]; then
  plan="${2:?--affected-plan requires a JSON plan path}"
  shift 2
  package_names="$(python3 -B scripts/coverage_selection.py packages --plan "$plan")"
  selection_plan=(--plan "$plan")
  test_packages=()
  while IFS= read -r package; do
    if [[ -n "$package" ]]; then
      test_packages+=(-p "$package")
    fi
  done <<< "$package_names"
  if [[ ${#test_packages[@]} -eq 0 ]]; then
    echo "No affected Rust package tests."
    exit 0
  fi
fi

# Per-test profile collection covers full runs only; PR selection runs must
# never produce or publish a graph.
if [[ -n "$coverage_graph" ]]; then
  if [[ ${#selection_plan[@]} -gt 0 ]]; then
    echo "error: --coverage-graph cannot be combined with --affected-plan" >&2
    exit 1
  fi
  printf '%s\n' "$coverage_graph" > "${coverage_dir}/coverage-graph.path"
  activate_per_test_profiles
fi

prepare_coverage
# Preserve feature unification and reuse one instrumented build for inventory and execution.
cargo metadata --offline --locked --all-features --format-version 1 > "${coverage_dir}/cargo-metadata.json"
cargo nextest list --locked --all-features "${test_packages[@]}" \
  --list-type binaries-only --message-format json > "${coverage_dir}/binaries.json"
cargo nextest list --cargo-metadata "${coverage_dir}/cargo-metadata.json" \
  --binaries-metadata "${coverage_dir}/binaries.json" \
  --message-format json > "${coverage_dir}/inventory.json"
python3 -B scripts/coverage_selection.py resolve "${selection_plan[@]}"
if [[ "$phase" == "prepare" ]]; then
  [[ $# -eq 0 ]]
  exit 0
fi
status=0
expression="$(jq -r '[.filters[]] | join(" | ")' "${coverage_dir}/selection.json")"
cargo nextest run --cargo-metadata "${coverage_dir}/cargo-metadata.json" \
  --binaries-metadata "${coverage_dir}/binaries.json" -E "$expression" --no-tests warn "$@" || status=$?
collect_profiles
if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi
completion=()
if [[ $# -gt 0 && "$*" != "--run-ignored all" ]]; then
  completion+=(--partial)
fi
python3 -B scripts/coverage_selection.py complete "${completion[@]}"
report_coverage
if [[ -n "$coverage_graph" ]]; then
  build_coverage_graph "$coverage_graph"
fi
