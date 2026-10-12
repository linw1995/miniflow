# ci-test-selection Specification

## Purpose

Define how repository CI selects Rust tests, refreshes trusted per-test coverage mappings, and separates PR patch feedback from complete project coverage.

## Requirements

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

### Requirement: Refresh mappings after successful main verification

Main-branch CI SHALL execute incremental collection when the database is compatible and inputs support selective execution. It SHALL retain untouched tests' mappings and original source coordinates. Missing data, incompatible fingerprints, unavailable anchors, or untracked inputs SHALL require full collection. Only successful collection SHALL advance the companion commit anchor and publish the baseline.

#### Scenario: A Rust source change affects part of the suite

- **WHEN** main changes Rust source covered by a compatible database
- **THEN** CI collects affected tests once and preserves mappings for untouched tests without repeating aggregate coverage execution

#### Scenario: A collection fails

- **WHEN** compilation or a selected test fails during collection
- **THEN** the companion anchor remains unchanged and CI does not publish a replacement baseline

### Requirement: Apply conservative input fallbacks

Selection SHALL compare changes since collection. Exact-path Cargo metadata rules SHALL select known fixture
and example consumers and supply the wrapper's exemptions. Wildcards MUST NOT exempt unknown inputs. Global
build/test configuration, build scripts, proc macros, and unmapped fixtures, examples, snapshots, or other
non-Rust inputs SHALL require full execution. Known repository documentation SHALL be exempt. Diagnostics
SHALL identify fallback inputs and collection mode.

#### Scenario: A mapped external input changes

- **WHEN** a compatible baseline exists and a change modifies a fixture or example listed by an exact-path input rule
- **THEN** affected execution and incremental collection select its consuming tests while preserving unrelated tests' mappings

#### Scenario: An unknown input matches a wildcard rule

- **WHEN** a fixture, example, or non-Rust input changes without an exact-path rule, even if it matches a wildcard rule
- **THEN** CI retains full-suite execution

#### Scenario: Global nextest configuration changes

- **WHEN** a change modifies global nextest configuration even if an input rule matches it
- **THEN** CI retains full-suite execution

#### Scenario: Rust changes include OpenSpec documentation

- **WHEN** a change modifies Rust source and OpenSpec Markdown or metadata
- **THEN** documentation alone does not force the full suite

#### Scenario: A Markdown fixture changes

- **WHEN** a change modifies Markdown used as an unmapped fixture or an unknown input
- **THEN** CI retains full-suite execution

### Requirement: Separate partial and complete coverage reporting

PRs SHALL retain patch coverage from selected tests. Main mapping collection SHALL not publish aggregate partial reports as complete project coverage. Nightly SHALL retain full-platform verification and complete Codecov reports. Incremental collection MUST NOT relabel untouched mappings as freshly collected coverage.

#### Scenario: Main performs incremental collection

- **WHEN** main refreshes only affected tests
- **THEN** it publishes mappings while complete project coverage remains the responsibility of full coverage runs

### Requirement: Support explicit local affected-test iterations

Local development SHALL support clean-commit baseline collection, affected execution after source edits, and compatible incremental refresh after committing. Collection and execution SHALL use the same pinned toolchain, host platform, and instrumentation settings. Developers SHALL be able to reuse a persistent generated-project cache. Generated mappings and reports SHALL remain under ignored target directories, and affected execution SHALL not replace required final validation.

#### Scenario: Repeat a local Rust edit

- **WHEN** a developer edits Rust source after collecting a compatible local baseline
- **THEN** the explicit affected command selects relevant tests and can reuse prepared fixture dependencies

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
