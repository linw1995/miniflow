## MODIFIED Requirements

### Requirement: Restore compatible default-branch mappings

CI SHALL restore per-test coverage mappings and their companion commit anchor from the default branch. Restore
and save SHALL use the same source-input fingerprint, scoped by host platform. Only the root manifest,
workspace member manifests, the lockfile, and pinned build inputs SHALL contribute to that fingerprint. Non-
workspace manifests MUST NOT change cache identity. PRs MUST NOT publish replacement main mappings; PR
snapshots SHALL use an isolated namespace.

#### Scenario: Collection creates generated manifests

- **WHEN** a successful collection creates Cargo manifests in generated directories
- **THEN** its saved baseline remains compatible with a clean checkout using the same source manifests and pinned build inputs

#### Scenario: A source manifest changes

- **WHEN** a workspace manifest, lockfile, or pinned build input changes
- **THEN** CI does not assume the old database is compatible and retains full-suite fallback

#### Scenario: A standalone fixture manifest changes

- **WHEN** a change modifies a standalone fixture manifest with an explicit test-input mapping
- **THEN** the baseline cache identity remains compatible and the rule selects the fixture's consuming tests

## ADDED Requirements

### Requirement: Reuse isolated successful PR baselines

PR CI SHALL restore its latest compatible successful snapshot before falling back to main. Snapshot keys SHALL
include PR, host, environment fingerprint, and tested commit. Only successful collection SHALL publish
mappings, their anchor, and required Git objects. Compilation artifacts MUST use a separate cache. Selected
tests SHALL execute once for both mappings and patch coverage. Retention SHALL rely on GitHub's automatic
eviction.

#### Scenario: Another commit arrives in the same PR

- **WHEN** a PR adds a commit after compatible successful collection
- **THEN** CI selects from its preserved per-test anchors and saves a new isolated snapshot while retaining untouched mappings

#### Scenario: The previous merge commit is not advertised

- **WHEN** a fresh runner restores mappings anchored to an earlier PR merge commit
- **THEN** cached Git objects make the actual tested source tree available before selection

#### Scenario: Collection fails or is cancelled

- **WHEN** selected test execution or coverage extraction fails or is cancelled
- **THEN** no replacement PR snapshot is published

#### Scenario: The PR baseline was evicted

- **WHEN** no compatible PR snapshot remains available
- **THEN** CI falls back to compatible main mappings or a full collection

#### Scenario: A mapped fixture changes between PR commits

- **WHEN** an exact input rule selects a fixture consumer without a Rust coverage overlap
- **THEN** incremental collection executes that consumer once and updates its mappings and current patch report

#### Scenario: An earlier PR change remains unmodified

- **WHEN** a later commit does not reselect a test that covered an earlier PR change
- **THEN** its retained valid counters still contribute to current PR patch coverage

#### Scenario: A test is removed

- **WHEN** incremental collection prunes a test's mappings
- **THEN** its cached counters are also removed rather than contributing obsolete hits
