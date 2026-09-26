#!/usr/bin/env bash

set -euo pipefail

workspace_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
generated_notices="$(mktemp "${TMPDIR:-/tmp}/miniflow-third-party-notices-check.XXXXXX")"

trap 'rm -f "${generated_notices}"' EXIT

"${workspace_root}/scripts/generate-third-party-notices.sh" "${generated_notices}"
test -s "${generated_notices}"
