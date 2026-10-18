## Why

Local users and CI should obtain test plans by executing the same Flow through `mf`. Today CI retains a separate Python selector and baseline-discovery algorithm, duplicating planning behavior. The Flow should own environment routing and baseline policy, while providers expose bounded external facts.

## What Changes

- Compose one Flow with existing parameters, if-else, structured-JSON Code, Loop/state assignment, and exclusive input edges.
- Express CI candidate filtering, eligibility, early stopping, and cumulative fallback in the graph instead of introducing a baseline-policy node.
- Prove that composition with recorded evidence before defining minimal GitHub access and Git revision I/O interfaces.
- Reuse `git.diff`, `lsp.analyze`, and `test_affected.plan` after the local/CI convergence; extend comparison inputs for local worktrees and single commits.
- Replace Python selection ownership with `mf` invocation and retain only necessary parameter/artifact adapters.
- Keep test execution, coverage evaluation, final all-job success recording, and full main/nightly verification outside the planning Flow.

## Capabilities

### New Capabilities

- `planning-evidence-access`: bounded external GitHub and Git facts consumed by Flow decisions, with explicit availability, provenance, and failure contracts.

### Modified Capabilities

- `core-nodes`: an explicit boolean assertion guard identified by the composition gate.

- `test-affected`: one conditional local/CI planning Flow with immutable local snapshots.
- `ci-test-selection`: baseline selection expressed with existing Flow nodes and CI selection executed through `mf`.

## Impact

First integrate origin/main through `01e5e1c` (#165 exclusive inputs), which includes `3f42ae6` (#161 structured JSON Code). Reuse their contracts without adding merge machinery or copying their test matrix.

No new `validation.baseline`, record-eligibility, environment-router, retry, or request-transform kind is planned. The executable gate identified `github.read`, `git.inspect`, and `git.fetch` as missing I/O capabilities, plus the reusable `builtin.assert` guard needed for explicit fatal invariants. Keep developer tooling outside product releases and reuse existing workspace dependencies.

Invocation remains `mf compile <definition> --output <runner>` then `mf run <runner>`. Automatic execution of JSON definitions and test-execution nodes are outside scope.
