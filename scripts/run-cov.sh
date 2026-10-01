#!/usr/bin/env bash

set -euo pipefail

workspace_root="$(pwd -P)"
coverage_dir="${workspace_root}/target/coverage"
export CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR="${coverage_dir}/build"
export RUSTFLAGS="-Cinstrument-coverage -Ccodegen-units=1 -Copt-level=0"
export LLVM_PROFILE_FILE="${coverage_dir}/data/mf-%p-%m.profraw"

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

# grcov scans every binary in its search directory. Remove old workspace mappings while retaining dependencies.
cargo clean --workspace --profile dev
cargo nextest run --workspace --all-features "$@"

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
