# Tasks

## 1. Unified value contract

- [x] 1.1 Add the canonical `NodeValue` trait and common `NodeValues` alias with existing directional errors; verify
  existing dynamic callers compile with the retained `Inputs` and `Outputs` aliases.
- [x] 1.2 Add the unified derive and explicit directional compatibility implementations through the existing derive
  parser; verify same-struct input/output use, old derives and manual implementations, generics, aliases, renames,
  private fields, and renamed runtime dependencies with compile cases.
- [x] 1.3 Preserve strict codec and presence behavior; verify missing and unknown ports, optional null/omission,
  integer/float representations, non-finite descendants, depth limits, escaped pointers, and shared payload identity
  through existing codec tests and focused additions.
- [x] 1.4 Migrate Identity and Iteration to the unified derive without changing task associated-type bounds; verify
  forwarding identity, body-dependent output refinements, and direct dynamic callers in their existing regressions.
- [x] 1.5 Document the unified contract, compatibility period, and port-bag versus nested-record boundary in
  `docs/node-development.md`; verify documented examples compile with the pinned toolchain.

## 2. Provider generation contract

- [x] 2.1 Add optional structured generation descriptors at the registration boundary without introducing a
  runtime-to-compiler dependency; verify providers lacking descriptors prepare and execute unchanged.
- [x] 2.2 Add provider-owned exported construction/invocation and field-access shims while keeping executor
  implementations private; verify an external multi-node fixture compiles against the documented public boundary.
- [x] 2.3 Resolve crate references and concrete type candidates through dependency aliases and selected package
  identities; verify aliased dependencies work and equal JSON descriptors cannot establish Rust type equality.
- [x] 2.4 Validate descriptor agreement with ordinary configured metadata and preserve construction error sources;
  verify contradictory interfaces and malformed advertised references fail before execution while missing optional
  support falls back.
- [x] 2.5 Document provider opt-in, certified codec validation, and the context-read contract; verify the external
  fixture demonstrates both opted-in and ordinary dynamic providers.

## 3. Conservative segment planning

- [x] 3.1 Compute typed connection eligibility and deterministic fallback reasons from resolved ports and generation
  descriptors; verify required same-type chains qualify while optional connections, opaque codecs, unsupported
  refinements, and distinct Rust types fall back.
- [x] 3.2 Extend use analysis to data/control readers, context references, selected outputs, retained context
  visibility, and payload observation; verify no moved value loses a second observer and no hidden `Clone` bound is
  introduced.
- [x] 3.3 Lower maximal eligible serial segments inside existing top-level oneshot domains; verify forks, joins,
  cross-domain connections, streams, nested bodies, and custom-runner generation preserve dynamic execution and existing
  domain layouts.
- [x] 3.4 Document supported connections and build inspection fallback reasons; verify the documented eligible and
  fallback examples match planner results.

## 4. Common runtime lifecycle

- [x] 4.1 Extract shared per-node lifecycle operations needed by typed bodies within existing runtime modules; verify
  ordinary task behavior remains unchanged for skip/missing precedence, observations, error phases, and staged effects.
- [x] 4.2 Add prepared generated-domain binding with a dynamic fallback to the existing scheduler; verify worker limits,
  private contexts, repeated invocation, failure draining, and launch without graph reconstruction. Verify both
  strategies share one initialized provider instance without duplicate factory calls or resource acquisition.
- [x] 4.3 Add certified typed result validation before field availability; verify invalid unused outputs, nested
  non-finite floats, prepared refinements, explicit skips, and escaped source-bearing failures prevent successor
  invocation and partial publication.
- [x] 4.4 Dispatch affected domains to their prepared dynamic fallback for payload snapshots; verify snapshots and
  custom-runner context inspection retain ordinary intermediate values while lifecycle-only telemetry remains supported
  on typed segments.
- [x] 4.5 Document lifecycle and boundary-validation responsibilities for provider shims; verify an external fixture
  cannot bypass required validation simply by advertising a matching Rust type.

## 5. Generated execution

- [x] 5.1 Emit typed domain source alongside frozen layouts in the linked-provider Cargo build; verify target
  compilation, warning-free symbols, private-provider shims, and retained diagnostics for incorrect advertised Rust
  assignments.
- [x] 5.2 Generate typed input assembly and direct field moves with dynamic entry/exit conversion; verify generated
  source conversion counts and isolated owned-allocation identity checks show no internal maps or encode/decode
  round trips for owned string/list chains.
- [x] 5.3 Bind generated segments and fallbacks during ordinary preparation; verify matching configured metadata,
  construction failures, startup manifest comparison, and dynamic mixed-provider execution.
- [x] 5.4 Track generator and descriptor inputs through reusable build directories; verify
  provider/configuration/feature changes regenerate source and failed builds preserve the previously installed
  executable.
- [x] 5.5 Document standard-runner fast paths and custom-runner compatibility; verify documented task, stream, and
  telemetry-disabled examples remain warning-free and manifest inspection needs no generation sidecars.

## 6. Integration and completion

- [x] 6.1 Run differential generated/in-memory regressions covering successes, business failures, dynamic boundary
  mismatches, omission/skip/missing precedence, context reads, snapshots, refinements, and concurrency; verify
  equivalent selected outputs, node attribution, phases, and typed error provenance.
- [x] 6.2 Measure allocations, runtime, generated binary size, and build time for eligible chains and mixed-provider
  graphs; retain reports under ignored `target/` and verify conversion elimination without timing-dependent pass
  thresholds.
- [x] 6.3 Review implementation against AGENTS.md module boundaries and Snafu error rules; verify no visibility-only
  facade, source stringification, accidental dependency update, or unrequested remote operation is introduced.
- [x] 6.4 Run `nix develop --command prek install`, `nix develop --command prek -a`, and `nix flake check -L`; verify
  required repository checks pass and record any environment limitations accurately.
- [x] 6.5 Run focused coverage when runtime/compiler behavior changes and `openspec validate
  unify-node-values-and-add-typed-fast-paths --strict`; verify spec scenarios are covered and mark tasks complete only
  after their stated evidence exists.

## 7. Final review and simplification

- [x] 7.1 Review the complete change against AGENTS.md and resolve blocking findings; record the outcome in `review.md`.
- [x] 7.2 Remove unnecessary production/test design through reversible ablations and retained-guard negative controls;
  keep all experiment details under ignored `target/` and verify the simplified paths with focused regressions.
- [x] 7.3 Run required pinned hooks, complete Nix checks, relevant coverage, and strict OpenSpec validation after the
  final simplifications; commit the reviewed implementation before archival.

## Workflow follow-up

- Archive the reviewed change and synchronize its capability deltas after the implementation commit.
- Validate the synchronized specifications and commit the archive result.

## Archive outcome

The reviewed implementation was committed as `73e6af8` before archival. The archive synchronized four capabilities,
adding thirteen requirements and modifying one existing requirement. This archive has no incomplete tasks or validation
issues, and all nineteen main specifications pass normal validation. Repository-wide archived-task validation retains
an unrelated pre-existing incomplete archive at `2026-10-05-unify-flow-runtime`; it was not changed by this work.
