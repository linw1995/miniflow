# Tasks

## 1. Compile and prepare isolated target contracts

- [x] 1.1 Emit deterministic top-level target records from the existing validated provider preparation,
  including compiled index, category, resolved ports, reference types, resources, and operation description;
  verify generated records match prepared metadata without additional factory calls.
- [x] 1.2 Bind selected targets from finite Flows and prepared streams while retaining original inference;
  verify a middle forwarding task rejects values that violate its original refined contract.
- [x] 1.3 Reject prepared target drift for executor category, ports, required flags, references, and stdin;
  verify fixture providers report affected node/port/resource context before business execution.
- [x] 1.4 Document whole-plan factory preparation and its failure limitation in `docs/compiling.md`; verify
  the documented behavior matches a fixture whose unrelated factory fails without executing any node.

## 2. Parse and validate isolated invocation values

- [x] 2.1 Add shared selector-aware invocation parsing while preserving ordinary workflow argument APIs;
  verify separated/inline selectors, repeated or missing targets, dotted identities, and string values
  resembling controls using parser regressions.
- [x] 2.2 Reuse qualified named and JSON input transports against the selected contract; verify unknown
  ports, unrelated root parameters, required values, optional omission, null, duplicate keys, and transport
  conflicts fail or bind as specified.
- [x] 2.3 Add exclusive context JSON/file transport with exact reference lookup and typed validation; verify
  valid router context, missing or undeclared references, null/type errors, duplicate keys, relative paths,
  read failures, and nonregular files including a FIFO without a writer.
- [x] 2.4 Enforce individual and combined canonical 1 MiB limits with bounded file reads; verify boundary,
  oversize, and escaping-expansion cases fail before target execution.
- [x] 2.5 Document selector, input, context, and presence semantics in `docs/compiling.md`; verify the examples
  use the same flags, flat identities, and limits accepted by parser coverage.

## 3. Execute prepared tasks in isolation

- [ ] 3.1 Add a prepared isolated task entry point using restricted input/context initialization and the
  shared invocation/publication lifecycle; verify repeated calls retain independent state and typed error
  chains without registry lookup or runtime graph construction.
- [ ] 3.2 Bypass external data/control dependencies, successors, and typed segment neighbors; verify fixture
  invocation counters show only the target executes in both finite and prepared-stream hosts.
- [ ] 3.3 Preserve Loop and Iteration body execution and resource ownership; verify Loop pass/exit behavior,
  parallel Iteration result ordering, worker bounds, and existing error policy under selected execution.
- [ ] 3.4 Validate and route only selected stdin resources; verify conditional input bypass and unavailable
  required stdin while unrelated sources remain unread.
- [ ] 3.5 Collect all validated produced target ports independently of workflow selections; verify zero
  outputs, output aliases, skipped/null distinctions, and invalid publication failure behavior.
- [ ] 3.6 Document the prepared target API and composite/body visibility boundaries in the existing runtime
  documentation; verify examples agree with retained isolation and composite regressions.

## 4. Describe and observe selected finite operations

- [ ] 4.1 Build finite selected descriptions and deterministic operation identities with original-flow
  correlation; verify equal targets with different values share identity while other targets and whole-flow
  runs differ, and metadata excludes configuration and business values.
- [ ] 4.2 Apply selected descriptions before preparation and retain failure phases/counts; verify a middle
  task reports one top-level visit, unrelated preparation failure emits no node start, and Loop/Iteration
  details preserve their existing meanings.
- [ ] 4.3 Use the selected finite identity for opt-in snapshots and bounded exporters, including a Task in a
  streaming binary; verify capture success, unavailable export diagnostics, and unchanged business results
  and exit status, with whole-stream capture still rejected.
- [ ] 4.4 Document selected identity, original-flow correlation, counts, and snapshot eligibility in
  `docs/observability.md`; verify examples align with captured lifecycle records.

## 5. Wire standalone runner dispatch and acceptance coverage

- [ ] 5.1 Dispatch `--node` through finite isolated execution in both binary feature modes without starting
  producers or timers; verify end-to-end selected Task runs from finite and streaming definitions, including
  a newly compiled older schema, and unknown/unsupported target diagnostics before business execution.
- [ ] 5.2 Reserve finite result stdout before provider construction and route selected resources; verify a
  noisy factory/task produces only result JSON on stdout, diagnostics on stderr, and no unrelated source
  reads or selected results after execution/publication failure.
- [ ] 5.3 Keep generated entry points demand-driven and whole-flow manifests unchanged; verify standard and
  custom artifact generation, warning-free finite/stream builds, existing normal execution, and manifest
  inspection with and without telemetry.
- [ ] 5.4 Add portable runner acceptance cases that remove build inputs; verify isolated execution works
  after moving binaries without the definition, lock, generated project, plugin sources, or toolchain.
- [ ] 5.5 Complete runner examples and support-boundary documentation, including producer/Event/nested/TUI
  exclusions; execute documented commands and confirm the Task phase is not presented as universal #125
  completion.

## 6. Review and repository validation

- [ ] 6.1 Review implementation against all delta scenarios, existing module/public API boundaries, and
  Snafu source-chain rules; record material findings and resolve them before final validation.
- [ ] 6.2 Run focused runtime/parser/compiler/runner regressions in `nix develop`; verify isolation,
  preparation drift, context references, composite execution, telemetry, and ordinary execution all pass.
- [ ] 6.3 Run `nix develop --command prek install`, `nix develop --command prek -a`, strict OpenSpec
  validation, and `nix flake check -L`; verify required checks pass. When coverage is relevant, run
  `nix develop --command bash scripts/run-cov.sh` and inspect reports under `target/coverage/result/`.

## Workflow follow-up

- Keep this change active during planning and implementation. Archive only after implementation review and
  verification, then validate synchronized main specifications.
- Plan producer selection using existing emitter, bounded backpressure, NDJSON delivery, acknowledgment,
  cancellation, and drain semantics before implementing that phase.
- Specify Event message transport, serial callbacks, timer progression, close delivery, and drain before
  adding Event selection; do not model it as one Task call.
- Plan a versioned per-target manifest interface and TUI preflight separately before adding
  `mf run --tui --node`.
- Record remaining producer/Event and scoped-selection coverage against #125 before deciding whether its
  arbitrary-node acceptance is complete. Issue updates require explicit authorization.
