## Decisions

- Reuse compiler `DescriptionError` for both observation entry points. A separate transparent observation wrapper adds no attribution and is removed.
- Box structural ordering errors to break the existing recursive compiler error relationship, retaining the Loop path.
- Keep error types with their existing owning modules. The runtime snapshot error describes store validation and the generic sink contract; transport and provider errors remain in telemetry, and capture errors remain in the TUI.
- Use source-bearing Snafu variants for external failures and source-free variants for domain validation. Keep transparent variants for pass-through conversions.
- Cache failures with `Arc` where APIs must return the failure again. The SDK error lacks `Clone`, so the export wrapper retains an equivalent typed SDK variant rather than formatting it.
- Prefer the cached exporter failure to the SDK flush summary, and clear the batch counter only after a successful flush.
- Keep focused tests for failure provenance, repeated failures, and presentation. Do not assert `Arc` identity, which is not part of the contract.

## Review and ablation

Compare the original implementation with the reduced compiler API and test assertions. Independently restore source loss at compiler ordering, UUID, trace ID, span ID, protobuf, snapshot sequence, digest, runtime sink, and TUI record decoding; also reverse exporter failure precedence. Each retained regression must detect its corresponding control. Keep the scripts, logs, and report under Git-ignored `target/`.

Review source ownership, existing module visibility, recursive error size, cached failure behavior, and generated runner compatibility against AGENTS.md. Run repository checks and strict OpenSpec validation before archiving.
