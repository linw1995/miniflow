# Verification

Controlled-clock node tests cover exact count thresholds, first-item deadlines, boundary arrivals,
replaced deadlines, empty closure, ordered tails, value-handle retention, and deadline overflow.
Integration tests cover collection typing, exact-value evidence, conditional skipping, full/tail
outputs, and explicit provider selection through the in-memory runtime.

- Focused core, Batch, and collection inference tests: 19 passed.
- Coverage of those targets: 19 passed; reports remain under `target/`.
- Repository hooks and specification validation passed.
- `nix flake check -L`: all host checks passed; 332 tests passed, none skipped.

Validation ran on `aarch64-darwin`. This layer adds no runner transport or observation fields. Flush
reporting belongs to the later observation change; batching values and timer behavior are verified here.
