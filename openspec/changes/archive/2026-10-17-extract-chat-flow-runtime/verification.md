# Verification

- The affected-package selector selected the complete workspace because dependency configuration and a
  shared example changed. All 705 selected tests passed, with four default skips.
- The backend and embedded host independently passed 48 focused tests without a test-side compiler codegen
  feature, with one fixture skip.
- Regression coverage includes parameterized model accounting, credential-independent fingerprints,
  conversation input ownership, exact invocation cleanup, unique-key bounded reply decoding, failed-save
  retention, foreign recovery rejection, and committed-success precedence.
- The chat workflow example compiles and executes against the local provider fixture with the complete
  text, history, usage, and finish-reason output contract.
- Repository hooks passed: formatting, Clippy, Cargo check, Markdown, rust-analyzer, workflows, and license audit.
- The change specification passed strict OpenSpec validation before archival.

No external provider or remote Git operation was needed. Validation used the pinned Nix toolchain.
