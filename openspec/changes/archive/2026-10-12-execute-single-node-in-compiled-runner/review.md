# Implementation Review

## Contract and module boundaries

- Target records are derived from the existing validated provider preparation. No second factory API or
  preparation pass was introduced to obtain metadata. Original graph inference determines output and
  external-reference types; supplied values do not specialize the target contract.
- Runtime target binding checks identity, index, execution capability, port types and required flags,
  declared references and referenced types, and stdin conditions before invocation. The owning runtime
  module exports the target and prepared-target APIs through its existing crate entry point.
- Shared per-node invocation validates inputs, executes the dynamic Task adapter, records lifecycle and
  snapshots, and validates publication before collecting port results. It bypasses the containing graph
  scheduler and generated typed segment. Existing whole-flow execution and custom generator APIs remain.
- Restricted context initialization replaces prior invocation output state and removes stdin when the
  target does not require it. Loop/Iteration bodies retain their original context visibility and execution
  mechanisms, including shared worker limits and exit/error policies.
- New source-bearing errors retain Snafu chains for JSON, I/O, type mismatches, and observation contracts.
  Diagnostic formatting for events and stderr does not replace typed errors returned to callers.

## Observation and runner review

- A selected operation uses a finite description at position zero and a deterministic identity distinct
  from the original flow. Root spans expose original-flow and target correlation without business values.
- Rejected invocation bindings are preparation failures with zero visits. Dependency-phase terminal
  records require an actual visited node, so pre-invocation rejection must not use that phase.
- Generated dispatch reserves result descriptors before provider construction. Noisy provider output
  reaches stderr; stdout contains only a successful finite result. Selection from a streaming executable
  prepares its operators without starting producers, Event callbacks, timers, or a streaming instance.
- Real OTLP receiver regressions accept selected lifecycle and snapshot identities from finite and stream
  binaries. Snapshot sink/export failures retain business results, and whole-stream snapshot rejection
  remains unchanged.

## Findings resolved in task group 6

- Building each target by cloning and pruning the complete description introduced quadratic copying.
  Target descriptions now contain one root directly, with Loop bodies indexed by their top-level owner.
- The public metadata-to-target helper could accept an incomplete node list or panic on an unresolved
  reference. It now rejects mismatched cardinality and returns a typed domain error for unknown references.
- A metadata-boundary regression added another user of factory counters. Its shared counter fixture now
  serializes those tests, preserving deterministic assertions under ordinary parallel `cargo test` runs.

- Independently selected stream Tasks initially used only runtime defaults. Target records now retain
  the original stream worker ceiling, binding checks it, and invocation applies the lower of that ceiling
  and caller options. A parallel Iteration regression verifies the original one-worker limit.
- The existing generated-project test referenced the previous parser entry point. Its assertion now names
  the selector-aware shared parser while retaining alias, dependency-feature, and single-binary checks.

## Post-implementation review refinements

- The frozen context contract now contains each exact reference ID and its resolved type once. It no
  longer repeats provider reference records or carries an unused required flag; every declared reference
  remains required. Diagnostic label wording does not change the binding contract.
- Removed the unused prepared-target metadata getter and kept the task-capability check local to its
  owning module. Existing shared public planning and execution APIs retain their module boundaries.
- Removed a duplicate stream preparation fixture and its manual inference setup. Its distinct capability
  and lookup checks remain in the stream execution regression. Programmatic context rejection, reference
  type drift, and preexisting stdin isolation now have stronger behavioral assertions.
- Positive simplification experiments and guard-removal counterexamples were executed with the pinned
  toolchain. Detailed evidence remains local under Git-ignored `target/ablation/issue125/`.
- Rechecked Snafu typed source chains, visibility, commit style, report locations, and pinned validation
  commands against AGENTS.md. No remote repository action or new dependency was introduced.
- Delta requirement descriptions were made concise while retaining all scenarios and behavior. Baseline
  main-spec length warnings were recorded before synchronization to distinguish existing findings from
  new specification regressions.

## Retained delivery limits

Whole-plan factory initialization remains part of selection and can fail in an unrelated provider.
Independent producer, Event, nested-body/control, and TUI selection are still outside this Task phase.
The embedded whole-flow manifest is unchanged. Existing binaries require recompilation to gain selection.
This implementation does not establish universal arbitrary-node support for Issue #125.
