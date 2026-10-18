## Decision

Use `lsp-types` at the standard semantic protocol boundary. Bind request methods to their parameter and result types using the crate's `Request` trait. Keep evidence serialization compatible with existing consumers.

The published `async-lsp` 0.2.4 reader allocates a buffer directly from Content-Length without a configurable size limit and reads unbounded header lines. Preserving our pre-allocation limits would require a second framing parser or a fork. Its client event loop therefore does not simplify this bounded synchronous client enough to justify adoption.

The current transport continues to own framing limits, cancellation, process cleanup, and request budgets. Standard payload validation belongs to the dependency; source-range validity, snapshot ownership, partial-result budgets, and conservative outcomes remain local.

## Validation and ablation

Reuse existing fake-server scenarios, repairing abbreviated valid fixtures to satisfy the standard schema. Extend existing malformed-result cases instead of adding a separate test framework. Verify actual rust-analyzer and downstream planning. Record dependency costs and removal experiments only under ignored `target/`.
