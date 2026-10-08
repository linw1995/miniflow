# Design

## Context

The runtime already owns strict input codecs and wraps typed tasks as ordinary task executors. Outputs must follow that same module and execution boundary without adding serialization round trips or changing dynamic callers.

## Decisions

- Export output declarations, field presence, value encoding, and source-bearing errors through the existing runtime entry point. A blanket field implementation covers ordinary value codecs; only optional fields add different presence behavior.
- Share derive parsing and diagnostics between input and output directions. Generate output declarations and calls to runtime encoding helpers for named owned fields.
- Use a generic typed result envelope and retain the existing dynamic result name as its map specialization. This preserves inference for existing dynamic constructors and carries skips and loop summaries without a second envelope implementation.
- Require typed preparation metadata to omit both input and output declarations. Preserve derivations, context references, and resource ownership.
- Allow instance output types to narrow the struct descriptors while preserving names and requiredness. Consume a single declaration map to check this bijection. Existing compiler and execution checks enforce the refined types.
- Retain Iteration's public dynamic entry point and expose owned collected results through its typed entry point. Its body and error policy continue to determine the prepared list type.
- Encode floats explicitly so non-finite values fail with a typed nested path. List and map encoders reuse shared children and add relative error paths without repeated recursive validation.

## Validation and Scope

Retain behavioral codec tests, compile-pass/fail cases, adapter error and control-metadata checks, and existing compiler/generated-runner regressions. Extend existing Iteration preparation checks instead of adding another fake refinement provider. Detailed ablation records and coverage artifacts remain local under ignored `target/`.

Manifest startup agreement remains an input/resource contract. Output validation continues through the existing compilation and publication boundaries; this change does not add a manifest schema or framing version.
