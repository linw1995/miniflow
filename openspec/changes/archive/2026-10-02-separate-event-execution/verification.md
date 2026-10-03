# Verification

The public factory fixture constructs `Send`-only event state and verifies that synchronous flow
construction rejects it before invocation. Existing task, context, Loop/Iteration, external plugin,
generated-runner, and snapshot wire-format behavior remains covered by the existing suites.

- Targeted runtime, core, code, and compiler suites: 177 tests passed, none skipped.
- `prek -a`: all hooks passed.
- `nix flake check -L`: all host checks passed; 292 tests passed, none skipped.

Local validation ran on `aarch64-darwin`. This layer introduces no streaming schema, scheduler, Batch
policy, transport, or stream observation fields. Command logs remain under ignored `target/`.
