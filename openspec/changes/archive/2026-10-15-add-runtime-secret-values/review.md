## Completion review

No blocking findings remain after implementation review and focused ablation.

- The existing exported ValueRef API owns sensitivity metadata. No facade, module split, visibility restriction, or dependency was introduced.
- Error types, Snafu selectors, and typed source chains are unchanged. Custom raw-string diagnostics remain outside the value-metadata contract.
- Recursive marking preserves extraction semantics without changing existing aliases. JSON equality remains business equality; scope merging also preserves metadata-only writes.
- Both public snapshot entry points redact before callbacks or storage. Public shared subtrees and the existing snapshot protocol remain usable without telemetry providers.
- Derived-value sensitivity is explicit. Templates remain deferred to #145; ordinary owned decoding and JSON serialization cannot preserve metadata.
- Redundant lifecycle assertions and a misleading Debug-based retention assertion were removed. Detailed ablation logs and measurements stay under ignored target/.
- Code, comments, docs, and specifications use English. Validation uses pinned Nix tooling, affected tests, and repository hooks.
