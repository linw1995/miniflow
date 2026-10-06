## Decisions

- Use existing runtime error types as Snafu sources and attach compiler context with `with_context`.
- Reuse `LoopBody` for graph failures and the existing transparent plan conversion for assignment failures.
- Keep domain-only validation failures separate from JSON decoding errors.
- Box recursive compiler errors and output derivation errors. An unboxed derivation error exceeds the repository's Clippy limit for returned errors; known-output type mismatches do not need boxing.
- Validate duplicate Loop body node IDs through the shared graph validator once.
- Keep compiler tests focused on source propagation and nested context. Runtime tests already cover individual derivation rejection rules.

## Review

Compare isolated ablations with the existing behavioral suite and source-chain negative controls. Keep experiment scripts, logs, measurements, and reports under Git-ignored `target/`. Archive only after the retained implementation and specification pass review and validation.
