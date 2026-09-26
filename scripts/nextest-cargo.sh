#!/usr/bin/env bash

set -euo pipefail

# The compiler sets a project-local target; redirect only tests selected by setup.
export CARGO_TARGET_DIR="${MF_TEST_TARGET_DIR:?}"
exec "${MF_TEST_REAL_CARGO:?}" "$@"
