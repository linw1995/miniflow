# Review

## Implementation boundaries

- Existing runtime descriptors, compatibility checks, snapshots, and scheduling remain in place. The unsigned
  numeric helper was already public inside its module; it is now re-exported beside the signed and floating helpers.
- Code owns its conversion errors and explicitly propagates sensitivity. Nested CEL errors retain typed sources
  through Snafu context. No facade module, record schema registry, collector, or runtime-wide taint policy was added.
- The chat prerequisite was integrated in `162c39e`. Structured list execution was verified before completing
  migration; the dedicated concat implementation and registration are removed. Package declarations and the Cargo
  lock are updated. Workflow lock generation remains covered by temporary-project compilation tests.
- Migration documentation covers conversion budgets, whole-output redaction, and the lack of shared-identity
  guarantees. Code type restrictions in the Loop documentation were corrected.
- Duplicate static rejection coverage was consolidated while retaining non-string map key checks. The standalone
  generated JSON fixture uses the existing shared build cache and serialized Cargo test group.

## Regression correction

The initial full run exposed a timing assumption in the imported captured-failure test: a 100 ms deadline can expire
before the server receives a request. The provider-error case now uses the ordinary 60-second deadline and verifies
HTTP 503; the deadline case verifies the deadline error and permits zero or one received request. Both still require
one explicit call record with unknown usage. No LLM production behavior was changed by this correction.

## Validation

- Group 1: 21 Code tests and the inactive-branch compiler check passed.
- Group 2: 29 Code and compiler checks passed, including snapshot redaction.
- Group 3: 11 chat/compiler integration tests and both CLI example tests passed. The examples compile and execute
  independently, including the migrated chat loop.
- All 57 repository script tests passed.
- The clean worktree selector returned no tests. Comparison from `c26c9da` selected all workspace packages because
  dependency manifests, the lock, shared test configuration, and examples changed.
- After the regression correction, the same complete package selection was rerun with `--no-fail-fast`:
  723 tests passed and 4 were skipped by default. The final Code suite contains 26 tests after consolidation.
- `nix develop --command prek -a` passed on the reviewed implementation. Both required Git hooks are installed.
- Strict change validation passed before archive. All planned implementation tasks are complete.
- Archive applied all deltas successfully. Both updated main specs pass normal validation. Main-spec strict mode
  reports the same five long-requirement warnings already present at `c26c9da` (four in Code and one in core), with
  no newly warned requirement. Existing requirement blocks were preserved rather than split for lint cleanup.

Experiment scripts, mutation logs, and the full final regression log remain under ignored
`target/code-json-ablation/`; they are not part of this change's committed artifacts.
