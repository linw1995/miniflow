#!/usr/bin/env bash

set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
output_path="${1:-${workspace_root}/target/THIRD_PARTY_NOTICES.html}"
raw_output="$(mktemp "${TMPDIR:-/tmp}/miniflow-third-party-notices.XXXXXX")"

trap 'rm -f "${raw_output}"' EXIT

cd "${workspace_root}"
mkdir -p "$(dirname "${output_path}")"

about_args=(
  --workspace
  --all-features
  --fail
  --locked
  --output-file "${raw_output}"
)
if [[ "${CARGO_ABOUT_OFFLINE:-}" == "1" ]]; then
  about_args+=(--offline)
fi

cargo about generate "${about_args[@]}" about.hbs

LC_ALL=C awk '{ sub(/\r$/, ""); sub(/[[:space:]]+$/, ""); print }' \
  "${raw_output}" > "${output_path}"
