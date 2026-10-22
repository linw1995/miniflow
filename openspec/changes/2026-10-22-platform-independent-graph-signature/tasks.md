## 1. Signature normalization

- [x] 1.1 Derive the toolchain identity from `release` + `commit-hash` on both sides (producer `scripts/coverage_graph.py`, consumer `plan.rs`); parse failures are explicit errors.
- [x] 1.2 Cover cross-host acceptance, release/commit mismatch rejection, and producer-side parse failure with tests; synchronize docs/ci.md and AGENTS.md.
