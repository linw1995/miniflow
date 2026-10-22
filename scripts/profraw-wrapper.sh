#!/bin/sh
# nextest run-wrapper: argv is <test-binary> <libtest-args...>
#
# Active only when MF_COVERAGE_PER_TEST_DIR is set (full coverage runs that
# collect the per-test coverage graph); every other invocation execs the test
# process unchanged. When active, each test gets its own LLVM_PROFILE_FILE
# keyed by NEXTEST_BINARY_ID + NEXTEST_TEST_NAME; the -%p suffix keeps
# instrumented subprocesses of one test from concurrently corrupting a
# single fixed path (their coverage merges into the spawning test, which is
# the desired attribution).
if [ -z "${MF_COVERAGE_PER_TEST_DIR:-}" ]; then
  exec "$@"
fi
name="${NEXTEST_TEST_NAME:-}"
bid="${NEXTEST_BINARY_ID:-unknown}"
if [ -z "$name" ]; then
  prev=""
  for a in "$@"; do
    if [ "$prev" = "--exact" ]; then name="$a"; fi
    prev="$a"
  done
fi
# Binary ids like mf-cli::bin/mf contain path separators; LLVM_PROFILE_FILE
# interprets % tokens, so the key encodes / as @ instead (neither @ nor /
# otherwise appears in cargo target ids or Rust test names).
# scripts/coverage_graph.py applies the same encoding when it maps profiles
# back to the test inventory.
key=$(printf '%s__%s' "$bid" "${name:-UNATTRIBUTED}" | sed 's|/|@|g')
if [ -n "$name" ]; then
  LLVM_PROFILE_FILE="$MF_COVERAGE_PER_TEST_DIR/${key}-%p.profraw"
else
  LLVM_PROFILE_FILE="$MF_COVERAGE_PER_TEST_DIR/${key}-%p-%m.profraw"
fi
export LLVM_PROFILE_FILE
exec "$@"
