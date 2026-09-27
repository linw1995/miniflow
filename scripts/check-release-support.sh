#!/usr/bin/env bash

set -euo pipefail

usage() {
  printf 'Usage: %s --owner <registry-owner>\n' "$0"
}

if [[ "${1:-}" == --help && $# -eq 1 ]]; then
  usage
  exit 0
fi
if [[ $# -ne 2 || "${1:-}" != --owner || -z "${2:-}" ]]; then
  usage >&2
  exit 2
fi

expected_owner="$2"
workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cargo_bin="${CARGO:-cargo}"
case "$cargo_bin" in
  /*) ;;
  */*) cargo_bin="$PWD/$cargo_bin" ;;
esac
packages=(mf-telemetry mf-runtime mf-compiler mfn-core)

workspace_metadata="$("$cargo_bin" metadata --no-deps --format-version 1 --manifest-path "$workspace_root/Cargo.toml")"
version="$(jq -er '.packages[] | select(.name == "mf-cli") | .version' <<< "$workspace_metadata")"
probe_dir="$(mktemp -d "${TMPDIR:-/tmp}/mf-release-support.XXXXXX")"
trap 'rm -rf "$probe_dir"' EXIT
cd "$probe_dir"

for package in "${packages[@]}"; do
  owners="$("$cargo_bin" owner --list --registry crates-io --color never "$package")"
  found=false
  while read -r listed_owner _owner_details; do
    if [[ "$listed_owner" == "$expected_owner" ]]; then
      found=true
      break
    fi
  done <<< "$owners"
  if [[ "$found" != true ]]; then
    printf '%s: required owner %s is absent; verify registry ownership before releasing the CLI\n' "$package" "$expected_owner" >&2
    exit 1
  fi
done

mkdir src
printf 'fn main() {}\n' > src/main.rs
{
  printf '[package]\nname = "mf-release-support-check"\nversion = "0.0.0"\nedition = "2024"\n[workspace]\n[dependencies]\n'
  for package in "${packages[@]}"; do
    if [[ "$package" == mf-telemetry ]]; then
      printf '%s = { version = "=%s", features = ["otlp"] }\n' "$package" "$version"
    else
      printf '%s = "=%s"\n' "$package" "$version"
    fi
  done
} > Cargo.toml

"$cargo_bin" metadata --format-version 1 > /dev/null
printf 'Support packages for CLI %s are available and owned by %s\n' "$version" "$expected_owner"
