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
  rm -rf "${coverage_dir}/data" "${coverage_dir}/result"
  rm -f "${coverage_dir}/binaries.json" "${coverage_dir}/cargo-metadata.json"
  mkdir -p "${coverage_dir}/data" "${coverage_dir}/result"
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
try:
    if Path(git("rev-parse", "--show-toplevel")).resolve() == Path.cwd().resolve():
        head, tree = git("rev-parse", "HEAD"), git("rev-parse", "HEAD^{tree}")
        dirty = bool(git("status", "--porcelain", "--untracked-files=all"))
except subprocess.CalledProcessError:
    pass
identity = {"head": head, "tree": tree, "dirty": dirty,
            "input_environment": json.loads(sys.argv[1]),
            "toolchain": subprocess.check_output(["rustc", "-vV"], text=True),
            "features": "all", "target": os.environ.get("CARGO_BUILD_TARGET"),
            "profile": os.environ.get("NEXTEST_PROFILE", "default")}
Path("target/coverage/result/source.json").write_text(json.dumps(identity) + "\n")
PYTHON
}

report_coverage() {
  grcov "${coverage_dir}/data" \
    --llvm \
    --branch \
    --source-dir "${workspace_root}" \
    --ignore-not-existing \
    --ignore '../*' \
    --ignore '/*' \
    --binary-path "${CARGO_TARGET_DIR}/debug/deps" \
    --output-types html,cobertura,lcov,markdown \
    --output-path "${coverage_dir}/result"

  test -s "${coverage_dir}/result/lcov"
  test -s "${coverage_dir}/result/cobertura.xml"
  cp "${coverage_dir}/result/lcov" "${coverage_dir}/result/lcov.info"
}

# CI keeps one instrumented build and accumulates profiles across package steps.
case "${1:-}" in
  --test-package)
    [[ $# -ge 2 && -n "$2" ]]
    selected_package="$2"
    shift 2
    MF_TEST_BUILD_CACHE_SESSION="$(cat "${coverage_dir}/build-cache-session")"
    export MF_TEST_BUILD_CACHE_SESSION
    # Some packages, such as derive macros, are tested only by their consumers.
    cargo nextest run \
      --cargo-metadata "${coverage_dir}/cargo-metadata.json" \
      --binaries-metadata "${coverage_dir}/binaries.json" \
      -E "package(=$selected_package)" --no-tests warn "$@"
    exit 0
    ;;
  --prune-cache)
    [[ $# -eq 1 ]]
    # Reporting must finish before its instrumented binaries are removed.
    test -s "${coverage_dir}/result/lcov.info"
    targets=("$CARGO_TARGET_DIR")
    if [[ -n "${MF_TEST_BUILD_CACHE_DIR:-}" ]]; then
      targets+=("${MF_TEST_BUILD_CACHE_DIR}/target" "${MF_TEST_BUILD_CACHE_DIR}/packaged-target")
      rm -f "${MF_TEST_BUILD_CACHE_DIR}/prepared-session.json"
    fi
    python3 scripts/prune-test-cache.py "${targets[@]}"
    rm -f "${coverage_dir}/binaries.json" "${coverage_dir}/cargo-metadata.json"
    exit 0
    ;;
  --report)
    [[ $# -eq 1 ]]
    report_coverage
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
if [[ "${1:-}" == "--affected-plan" ]]; then
  plan="${2:?--affected-plan requires a JSON plan path}"
  shift 2
  jq -e '(.packages | type == "array") and all(.packages[]; type == "string" and length > 0)' "$plan" > /dev/null
  test_packages=()
  while IFS= read -r package; do
    test_packages+=(-p "$package")
  done < <(jq -r '.packages[]' "$plan")
  if [[ ${#test_packages[@]} -eq 0 ]]; then
    echo "No affected Rust package tests."
    exit 0
  fi
fi

prepare_coverage
if [[ "$phase" == "prepare" ]]; then
  [[ $# -eq 0 ]]
  # Preserve feature unification across selected packages and compile only once.
  cargo metadata --offline --locked --all-features --format-version 1 > "${coverage_dir}/cargo-metadata.json"
  cargo nextest list --locked --all-features "${test_packages[@]}" \
    --list-type binaries-only --message-format json > "${coverage_dir}/binaries.json"
  exit 0
fi
cargo nextest run --locked --all-features "${test_packages[@]}" "$@"
report_coverage
