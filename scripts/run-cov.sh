#!/usr/bin/env bash

set -euo pipefail

test_mode="full"
case "${1:-}" in
  --affected)
    test_mode="affected"
    shift
    ;;
  --collect-affected)
    test_mode="collect"
    shift
    ;;
  --refresh-affected-cov)
    test_mode="refresh-cov"
    shift
    ;;
  --refresh-affected)
    test_mode="refresh"
    shift
    ;;
esac

if [[ "$test_mode" != "full" && "$#" != 0 ]]; then
  printf '%s\n' "Affected coverage modes do not accept additional test filters." >&2
  exit 2
fi

workspace_root="$(pwd -P)"
coverage_dir="${workspace_root}/target/coverage"
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

if [[ "$test_mode" == "collect" || "$test_mode" == "refresh" || "$test_mode" == "refresh-cov" ]]; then
  # Keep the ambient flags identical to affected runs, or the coverage DB always misses.
  cargo fetch --locked
  mark_target_directory "${workspace_root}/target/affected/build"
  mark_fixture_target
  cargo clean --workspace --profile dev --target-dir "${workspace_root}/target/affected/build"
  if [[ "$test_mode" == "refresh-cov" ]]; then
    rm -rf "${coverage_dir}/data" "${coverage_dir}/result"
    mkdir -p "${coverage_dir}/data" "${coverage_dir}/result"
    export CARGO_TARGET_DIR="${workspace_root}/target/affected/build"
    export CARGO_AFFECTED_PROFILE_EXPORT_DIR="${workspace_root}/target/affected/profiles"
    python3 scripts/run_affected_tests.py --collect --bundle
    python3 - <<'PYTHON'
from pathlib import Path
import subprocess

profiles = sorted(Path("target/affected/profiles").rglob("*.proftext"))
if not profiles:
    import json
    report = json.loads(Path("target/affected/report.json").read_text())
    if report.get("selection", {}).get("summary", {}).get("selected") == 0:
        raise SystemExit(0)
    raise RuntimeError("Collector did not export aggregate coverage profiles")
sysroot = subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip()
version = subprocess.check_output(["rustc", "-vV"], text=True)
host = next(line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: "))
tool = Path(sysroot) / "lib/rustlib" / host / "bin/llvm-profdata"
subprocess.run([str(tool), "merge", "--sparse", *map(str, profiles),
                "-o", "target/coverage/data/merged.profdata"], check=True)
PYTHON
    if [[ ! -s "${coverage_dir}/data/merged.profdata" ]]; then
      printf '%s\n' "No tests selected; no coverage report will be uploaded."
      exit 0
    fi
  elif [[ "$test_mode" == "refresh" ]]; then
    export CARGO_TARGET_DIR="${workspace_root}/target/affected/build"
    python3 scripts/run_affected_tests.py --collect
  else
    cargo affected collect -- --locked --workspace --all-targets --all-features --profile ci --no-fail-fast
    git rev-parse HEAD > "${workspace_root}/target/affected/baseline.sha"
  fi
  if [[ "$test_mode" != "refresh-cov" ]]; then
    exit 0
  fi
else

  rm -rf "${coverage_dir}/data" "${coverage_dir}/result"
  mkdir -p "${coverage_dir}/data" "${coverage_dir}/result"

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
  mark_target_directory "${CARGO_TARGET_DIR}"
  mark_fixture_target

  # grcov scans every binary in its search directory. Remove old workspace mappings while retaining dependencies.
  cargo clean --workspace --profile dev
  if [[ "$test_mode" == "affected" ]]; then
    python3 scripts/run_affected_tests.py
    if jq -e '.selection.summary.mode == "selection" and .selection.summary.selected == 0' \
      target/affected/report.json > /dev/null; then
      # Build-script profiles alone cannot describe the coverage of a test run.
      printf '%s\n' "No tests selected; no coverage report will be uploaded."
      exit 0
    fi
  else
    cargo nextest run --workspace --all-features "$@"
  fi

fi

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
