## Decisions

- Convert `TypeMismatch` to its existing boxed source through Snafu metadata and attach edge context lazily.
- Export the existing subgraph validation selector at the runtime entry point used by the core-node factory. Keep the string-returning Loop helper API unchanged and build the domain error with the selector.
- Reuse the existing stream-to-node error conversion for producer panics; no additional error type or wrapper is needed.
- Check producer identity, diagnostic text, and typed panic provenance in the existing integration test. Remove duplicate destructuring and the redundant exact-message assertion.

## Review

The owning modules, source types, and existing exports were checked against AGENTS.md. Isolated ablations cover the conversion, regression assertion, selector export, and source metadata. The experiment runner, logs, and report remain under ignored `target/ablation/error-handling/`.
