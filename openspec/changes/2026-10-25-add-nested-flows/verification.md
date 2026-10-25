## Initial checks

- Installed the repository hooks and passed every `prek -a` hook, including Clippy, Cargo check,
  rust-analyzer, Markdown, workflow validation, and the license audit. Checked newly added files too.
- Passed 137 library tests across runtime, compiler, telemetry, core, and code providers.
- Passed 24 focused compiler integration tests covering nested calls and typed error sources.
- Passed 52 focused runtime, telemetry contract, and TUI state tests.
- Passed the source-free Rust embedding host acceptance test, including rebuilding and dependency
  ownership. Passed the compiler build without default features.
- Compiled the shared affected-test Flow with telemetry enabled and generated its immutable worktree plan.

## Environment limitations

The initial sandbox could not connect to the Nix daemon. Validation used the repository's cached Nix development
environment and its pinned Rust 1.99.0 toolchain.

The affected-test execution selected 1,059 tests. It stopped after 41 passes and eight conversation
integration failures because the sandbox rejects local TCP listeners with `Operation not permitted`.
A broader library run exposed the same restriction in TUI receivers; the independent core library
suites passed after fixing the embedding compatibility regression it also identified. At that stage, the real HTTP
title-composition test could not run. An in-process provider double verifies the shipped
title policy's success and captured-failure branches, answer preservation, and exact usage merging.

Full TCP-backed acceptance is being rerun after enabling local listeners and network access. Local logs
and affected-test plans remain under the Git-ignored `target/` directory.

## Provider ownership correction

The title JSON and its metadata factory now belong to `mf-chat`. A generic runtime registry exposes
linked Flow providers, and the compiler resolves that catalog without chat-specific source or version
policy. Chat profile path loading retains host-owned Cargo dependencies.

The generic nested-call suite, chat title suite, catalog validation, copied-workflow packaging, and
all documented example executions passed with network access enabled. The first unrestricted affected
run exposed the copied-provider-path and missing-example-case regressions; both have focused passing
regressions. The complete affected suite and remote CI are being rerun on the corrected implementation.
