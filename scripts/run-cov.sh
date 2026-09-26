#!/usr/bin/env bash

set -euo pipefail

workspace_root="$(pwd -P)"
coverage_dir="${workspace_root}/target/coverage"
export CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR="${coverage_dir}"
export RUSTFLAGS="-Cinstrument-coverage -Ccodegen-units=1 -Copt-level=0"
export LLVM_PROFILE_FILE="${coverage_dir}/data/mf-%p-%m.profraw"

rm -rf "${coverage_dir}"
mkdir -p "${coverage_dir}/data" "${coverage_dir}/result"

# Offline fixture builds resolve the complete lockfile, including other targets.
cargo fetch --locked
cargo nextest run --workspace --all-features "$@"

grcov "${coverage_dir}/data" \
  --llvm \
  --branch \
  --source-dir "${workspace_root}" \
  --ignore-not-existing \
  --ignore '../*' \
  --ignore '/*' \
  --binary-path "${coverage_dir}/debug/deps" \
  --output-types html,cobertura,lcov,markdown \
  --output-path "${coverage_dir}/result"

test -s "${coverage_dir}/result/lcov"
test -s "${coverage_dir}/result/cobertura.xml"
cp "${coverage_dir}/result/lcov" "${coverage_dir}/result/lcov.info"
