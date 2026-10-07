# Completion review

## Decision

The implementation is ready to archive. The four specification deltas match the generated manifest, startup validation, and TUI behavior. No blocking review finding remains.

## Repository rules

- Shared contracts remain in `mf-runtime`; code generation remains in `mf-compiler`; object-file parsing and terminal launch remain in `mf-tui`. Generated runners have no object-reader or terminal dependencies.
- Existing module declarations and exports control the public surface. New shared APIs use plain `pub`; the three scoped helpers serve specific sibling-module boundaries. No facade or visibility-only test relocation was introduced.
- Fallible operations retain typed sources through Snafu context or transparent conversions. Checked invariants use `ensure!`, and immediate domain failures use selectors. Original file I/O errors are recovered from the owned reader backend.
- No node-specific error type was moved into the runtime, and interface mismatch remains a shared runtime contract.
- Commits follow the repository's semantic type/scope convention. Experimental reports, backups, coverage, and logs remain under ignored `target/`.

## Final implementation

The reader keeps its byte, section-count, name-length, framing, and semantic limits. Original I/O errors remain owned by the cache backend until parsing completes. The dedicated static section remains reachable from runner inspection, validation, and execution.

Retention regression coverage belongs to the CLI integration suite, where it uses the same section reader as launch preflight. It still exercises release optimization, LTO, platform stripping, both telemetry configurations, command framing, and noisy construction. Its relocation follows that responsibility and replaces a separate scanning oracle.

Metadata framing and protocol versions are unchanged. Legacy inspection remains available only when a recognized supported executable lacks the manifest section; corrupt or unsupported data still prevents launch. Generated runtime preparation still rejects interface drift before node dispatch or source consumption.

## Verification

The final macOS Nix check passed all 441 workspace tests. The final native Linux focused check passed 19 tests covering every affected manifest, preflight, schema, generation, standalone, and retention path. Earlier full Linux validation passed all 441 tests. The final targeted coverage run passed 14 tests, and pinned hooks and strict OpenSpec validation passed.

See [verification.md](verification.md) for requirement traceability and platform scope. Experimental reports and source variants are local under `target/ablation/embed-workflow-manifest/` and are excluded from commits.
