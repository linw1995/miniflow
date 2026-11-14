#!/usr/bin/env bash

set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
scope=(--workspace)
case "${1:-}" in
  --release)
    # Archives ship the CLI, not every package sharing its development workspace.
    scope=(--manifest-path "$workspace_root/crates/mf-cli/Cargo.toml")
    shift
    ;;
  --manifest-path)
    if [[ $# -lt 2 || -z "$2" || "$2" == --* ]]; then
      printf '%s\n' '--manifest-path requires a Cargo.toml path' >&2
      exit 2
    fi
    scope=(--manifest-path "$2")
    shift 2
    ;;
esac
if [[ $# -gt 1 || "${1:-}" == --* ]]; then
  printf '%s\n' 'Usage: generate-third-party-notices.sh [--release | --manifest-path PATH] [OUTPUT]' >&2
  exit 2
fi
output_path="${1:-${workspace_root}/target/THIRD_PARTY_NOTICES.html}"
raw_output="$(mktemp "${TMPDIR:-/tmp}/miniflow-third-party-notices.XXXXXX")"
diagnostics="$(mktemp "${TMPDIR:-/tmp}/miniflow-third-party-notices-errors.XXXXXX")"

trap 'rm -f "${raw_output}" "${diagnostics}"' EXIT

cd "${workspace_root}"
mkdir -p "$(dirname "${output_path}")"

about_args=(
  "${scope[@]}"
  --config "$workspace_root/about.toml"
  --all-features
  --fail
  --locked
  --output-file "${raw_output}"
)
if [[ "${CARGO_ABOUT_OFFLINE:-}" == "1" ]]; then
  about_args+=(--offline)
fi

status=0
"${CARGO:-cargo}" about -L warn --color never generate "${about_args[@]}" about.hbs \
  2> "${diagnostics}" || status=$?
cat "${diagnostics}" >&2
if ((status != 0)); then
  exit "${status}"
fi
# cargo-about 0.9.0 warns and falls back when a clarification's files drift.
# Such a fallback can silently omit bundled-resource notices despite --fail.
if grep -Fq 'failed to validate all files specified in clarification for crate ' "${diagnostics}"; then
  printf '%s\n' 'License clarification validation failed; review the source files and checksums before generating notices.' >&2
  exit 1
fi

LC_ALL=C awk '{ sub(/\r$/, ""); sub(/[[:space:]]+$/, ""); print }' \
  "${raw_output}" > "${output_path}"
