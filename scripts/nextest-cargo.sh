#!/usr/bin/env bash

set -euo pipefail

# The compiler sets a project-local target; redirect only tests selected by setup.
export CARGO_TARGET_DIR="${MF_TEST_TARGET_DIR:?}"
if [[ -n "${MF_TEST_SOURCE_CONFIG:-}" ]]; then
  set -- "$@" --config "$MF_TEST_SOURCE_CONFIG"
fi
exec "${MF_TEST_REAL_CARGO:?}" "$@"
