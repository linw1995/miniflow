## Context

Affected selection uses a SQLite database of per-test source ranges and commit anchors. Aggregate LCOV cannot reconstruct those associations. Generated fixture and vendor manifests are created during collection, so unrestricted manifest globs produce different restore and save fingerprints.

## Decisions

### Use source inputs for cache identity

Keep the existing cache namespace and include repository-root and crates manifests, the lockfile, pinned toolchain, and flake inputs. Exclude nested target directories. CI and nightly use identical fingerprint expressions and only default-branch collections publish mappings.

### Refresh mappings on main without a second test pass

Main uses the native collector instead of aggregate coverage generation. A compatible status preflight enables incremental collection; missing mappings, incompatible fingerprints, unavailable anchors, or untracked build inputs require full collection. Preserve untouched rows and their original coordinates. Advance the companion commit anchor only after collection succeeds.

PRs retain selected-test patch coverage. Nightly retains complete Linux and macOS verification and complete Codecov reports. Older per-test anchors can broaden input-rule selection until a full collection refreshes them.

### Keep preparation in the coverage wrapper

The Shell wrapper owns instrumentation, cache directory markers, dependency fetching, and stale workspace cleanup. The Python helper owns selection and collection decisions. Remove repeated fetch and cleanup calls from the helper rather than adding another preparation abstraction.

### Retain conservative input handling

Only root README/AGENTS, Markdown under docs and openspec, and OpenSpec metadata are exempt. Fixture Markdown, snapshots, examples, build scripts, proc macros, and unknown inputs still force full execution. Report fallback inputs and collection mode for diagnosis.

### Consolidate verification in the existing fixture

Use the existing small Rust workspace to verify actual selection, incremental row preservation, documentation-only changes, incompatible or missing databases, aggregate coverage, and failed collection. Remove redundant command-routing mocks after mutation controls confirm equivalent detection. Detailed experiment scripts and reports remain local.
