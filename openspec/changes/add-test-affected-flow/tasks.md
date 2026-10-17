# Tasks

## 1. Developer-tool boundaries

- [x] 1.1 Add `mfn-lsp` and `mfn-test-affected` under `tools/test-affected/nodes/` as unpublished workspace members; verify metadata, independent versions, unchanged CLI defaults, and no product-to-tool dependency path.
- [x] 1.2 Narrow existing CD, support-package, notice-generation, and Nix release entry points to exclude the tools without adding a new release-policy configuration; verify existing release-support tests, shared dependency notices, and workspace-wide development auditing.
- [x] 1.3 Document the tool layout, release boundary, and explicitly configured server in contributor/release guidance and the tool README; verify package paths and ordinary validation without an installed language server.

## 2. LSP Stream provider

- [x] 2.1 Define typed analysis request/outcome ports and opaque context passthrough; verify value metadata preservation, snapshot/configuration identities, and standard-only shapes without testing or Cargo fields.
- [x] 2.2 Implement stdio framing, capabilities, negotiated offsets, server requests/notifications, and partial results; verify interleaved fake-server messages, non-ASCII positions, and unsupported-method gaps.
- [x] 2.3 Implement batched standard queries and small optional server profiles with one session per snapshot/configuration per invocation; verify session reuse, base/head isolation, and no unconfigured Rust extension calls.
- [x] 2.4 Implement bounded I/O, deadlines, cancellation, and child cleanup before terminal publication; verify missing servers and deadlines yield one incomplete outcome, while cancellation/disposal stops idle reads without publishing fallback or leaking children.
- [x] 2.5 Register `lsp.analyze` as `TypedStreamNode`; verify one terminal emission, no-server `not_requested` passthrough, progress-only observations, emitter failure propagation, and side-effect-free preparation/inspection; document its node contract.

## 3. Git facts, planning, and script consumption

- [x] 3.1 Implement `git.diff` with local merge-base comparison, old/new ranges, and durable owned snapshots; verify no checkout changes, fetches, Cargo policy, or hidden LSP startup, and validate missing/mismatched revisions.
- [x] 3.2 Implement `test_affected.plan` with Cargo baseline computation and ordinary selection/union functions; verify baseline parity, all dependency kinds, test-only/production combinations, and behavior changes whose call graphs remain unchanged.
- [x] 3.3 Validate forwarded Git facts and base/head semantic outcomes; verify deleted definitions, shared helpers, macros, subprocess gaps, full fallback on analyzer failure, and identity mismatch without introducing a generic adapter framework.
- [x] 3.4 Emit structured plans and reports using an optional matching inventory; verify exact selectors, retained scopes, explicit unknown counts, and that the Flow never compiles or executes tests; document the final output format.
- [x] 3.5 Add the small `run-tests.py` plan consumer using existing helpers where appropriate; verify identity checks, inventory/build reuse, generated-name fallback, empty plans, cancellation, malformed plans, and nonzero exit for actual failures without a runner framework.

## 4. Three-node streaming Flow

- [x] 4.1 Add `flows/test-affected.json` using schema `2026-10-03` and stream execution with only git.diff, lsp.analyze, and test_affected.plan; verify normal compilation and manifest inspection require no repository I/O or live language server.
- [x] 4.2 Exercise in-memory and generated execution with an upstream Task that emits once and closes; verify the analysis finishes, exactly one final task invocation occurs, output drains, and a Git-to-plan bypass is rejected as cross-domain.
- [x] 4.3 Document and verify a Flow-to-plan command followed by a separate script command from a different working directory; verify context passthrough, output-format compatibility, local dependencies, and exclusion from release examples.

## 5. Integration and evaluation

- [x] 5.1 Include the packages in regular development/coverage paths while retaining current CI selection and coverage policy; verify mock-backed tests require no installed server and release validation excludes the tools.
- [x] 5.2 Run a pinned real-server smoke test and portable conservative-selection fixtures; replay available pinned #131, #151, #152, #157, and #159 snapshots and independently verify exact nextest identities without automatic history fetches.
- [x] 5.3 Measure the new Flow with warm-up and repeated serial trials; separately measure baseline, LSP, build/list, and test execution time, verify empty analysis batches start no server and record the cost of late full-fallback decisions, and retain raw artifacts only under `target/`.
- [x] 5.4 Run installed hooks with `prek -a`, the affected-package selector and selected suites through `nix develop`, and strict OpenSpec validation; review the final diff and release boundaries without local `nix flake check -L`.

## Workflow follow-up

- Consider CI adoption separately after correctness and end-to-end benefit are demonstrated and the existing CI specification is reconciled.
- Revisit semantic caching, generic adapters, or shared session infrastructure only when measured needs justify them.
- Archive the change after implementation and its validation evidence are complete.
