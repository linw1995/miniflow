# Verification

- The affected-package selector required the complete workspace because dependency and shared input files changed.
- The final instrumented regression passed all 718 tests, with four default skips.
- Repository hooks passed formatting, Clippy, Cargo check, Markdown, rust-analyzer, workflows, and license audit.
- Single-call and loop chat examples compiled and executed as standalone runners against local provider fixtures.
- Multi-model tests cover success, a failed earlier model followed by success, and a failed final model.
- Loop tests verify explicit accumulation, distinct call IDs, loop indices, and stopping after model failure.
- Storage tests cover old-database upgrade, metadata reopen and byte limits, unchanged earlier history,
  failed-write rollback, save-only recovery, duplicate call rejection, and per-call UTC attribution.
- Runtime tests cover unobserved nested body paths and node identity restoration after failure and panic.
- Provider tests preserve typed default errors, capture request/response failures explicitly, retain reported
  counters through invalid replies, omit invented totals, and keep metadata out of requests.
- The change specification passed strict OpenSpec validation before archival.

Validation used the pinned Nix toolchain and local HTTP fixtures; no live model service was required.
