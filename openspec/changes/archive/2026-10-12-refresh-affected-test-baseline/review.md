# Review

The cache expressions use the same repository manifests and pinned build inputs for restoration and publication. Generated target directories and local environment manifests are excluded. Existing cache actions retain default-branch ownership; PRs only consume the mapping database.

The main collection branch publishes its output only after a successful collector exit. Full fallback covers missing data, incompatible fingerprints, unavailable anchors, and inputs outside coverage tracking. Untouched records retain their original source coordinates and commit anchors. The companion anchor is updated after success, and partial collections are not uploaded as complete Codecov reports.

Documentation exemptions are limited to known repository documentation. Existing fixture, snapshot, example, build-script, and proc-macro guards remain active. Inspection of Rust includes and test inputs found no consumers of the exempt documentation paths.

The Shell wrapper owns instrumentation and build preparation. The helper uses standard-library functions to select execution or collection; duplicate preparation calls were removed. The existing Rust fixture replaces command-only mocks and verifies real process failure, compatibility fallback, preserved mappings, and coverage output. Mutation controls support retaining the remaining protections.

AGENTS.md requirements are satisfied through the pinned toolchain, installed hooks, required validation, conventional CI commit scope, English repository artifacts, and ignored experiment files. Rust visibility, module boundaries, error types, dependencies, and release versions are unaffected. Subsequent repository CI will exercise actual cache publication on GitHub; local verification exercises the cache identity contract with GitHub's glob implementation.
