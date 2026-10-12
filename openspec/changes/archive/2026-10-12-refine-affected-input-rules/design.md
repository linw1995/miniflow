## Context

LLVM ranges do not capture file reads or code compiled in subprocesses. cargo-affected already supports explicit input rules, but the wrapper overrides them by forcing all tests for every external input.

## Decisions

### Use one rule source

Keep input-to-test associations in `workspace.metadata.affected.rule`. The wrapper reads only literal paths from those rules as known external inputs. Wildcards still work for native selection, but cannot exempt new or renamed external inputs. Avoid a duplicate Python allowlist or a second glob implementation.

### Select verified consumers

Map individual compiler helpers to their consuming integration binaries. Keep the shared multi-nodes fixture mapped to CLI and compiler integration tests because those suites reuse it indirectly. Map telemetry fixtures to contract tests and relevant unit tests; map examples to the example suite and other direct consumers. Include the embedding consumer visible in the open embedding PR for existing helpers and examples, alongside their current consumers.

Global configuration, build scripts, and proc macros retain full execution. Unknown external Rust inputs under examples also fall back to all tests. Rules must be updated when consumers change.

### Fingerprint workspace manifests

Match the workspace member patterns at their manifest depth instead of all nested crates manifests. Fixture Cargo manifests are external inputs and use explicit rules; workspace manifests, the lockfile, and pinned toolchain/flake inputs still invalidate baselines. Restore and save use identical expressions.

### Extend existing verification

Use the existing coverage fixture for runtime JSON reads and an uninstrumented Rust subprocess. Verify actual selective execution, native rule attribution, nonempty PR coverage, and preserved untouched database rows. Keep ablation artifacts under ignored target directories. Avoid adding production matching abstractions or parallel mock suites.
