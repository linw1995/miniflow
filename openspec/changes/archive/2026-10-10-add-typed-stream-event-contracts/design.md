## Context

Task execution already links `Input` and `Output` to real decoding and encoding. The previous event/stream marker
traits only declared types and were removed during simplification. A handwritten contract remained in fixed builtin
providers because their actual execution methods still used dynamic maps.

## Decisions

- `TypedEventNode` consumes `NodeEvent<Self::Input>` and returns `EventEffects<Self::Output>`. Its associated types
  determine the reflected ports and runtime conversions. Timer and upstream-close variants bypass input decoding.
- Parameterize `NodeEvent`, `EventEmission`, and `EventEffects` with existing dynamic defaults. Reuse these containers
  instead of adding a parallel typed event hierarchy. Implement `EventEffects<O>::default` without `O: Default`.
- Encode the complete event emission vector before returning effects to the scheduler. Preserve batch data, explicit
  skips, loop summaries, and deadline changes. Forward buffer observations to the same initialized provider state.
- `TypedStreamNode` receives decoded input and a borrowed `FnMut(TypedNodeResult<Output>)` callback. The callback
  encodes each result and forwards it through the existing `Emitter`; it retains validation, backpressure, cancellation,
  and incremental delivery. A separate typed emitter wrapper and extra public lifetime parameters are unnecessary.
- Keep runtime adapters private to preparation and expose conversion helpers at the existing runtime entry point for
  providers retaining dynamic entry points. Input decode selectors become accessible only at the sibling-module
  boundary that attaches their shared runtime context. Node-specific I/O errors stay in the provider crate.
- Batch and Readline implement the typed execution contracts directly. Their factories use automatic typed preparation,
  and the runtime owns map conversion. Keep Iteration's public inspection helper tied to its execution-associated types.
- Typed event/stream metadata does not certify task-only generated segments. Existing dynamic providers keep their
  current execution traits and generic event-container defaults.

## Validation

Extend existing constructor tests to cover automatic typed preparation without execution or competing declarations.
Exercise timer/close decoding bypass, input failure before state mutation, complete event encoding, buffer forwarding,
shared values, control metadata, Send-only producers, startup parameters, and invalid unobserved producer outputs.
Retain builtin batching, text source, streaming, and generated-runner regressions. Use reversible simplifications and
negative controls, keeping reports under ignored `target/`. Run pinned hooks, native Nix checks, focused coverage,
and strict delta validation before review, archival, commit, and PR updates.
