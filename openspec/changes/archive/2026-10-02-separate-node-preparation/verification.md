# Verification

## Preparation and execution

The base builds independently from `main` without streaming definitions, event executors, or scheduler
changes. Existing compiler suites cover metadata inference, context references, conditional publication,
Loop and Iteration construction, external plugin discovery, and generated execution. Missing subgraph
bodies and bodies supplied to plain factories fail before returning an executor.

Review found and corrected an unintended snapshot enum rename during migration. A golden wire-format
test preserves the published `node` record tag; the existing TUI history integration test also verifies
that consumers can still decode it.

## Validation

- `nix develop --command prek -a`: all hooks passed.
- `nix develop --command env CARGO_NET_OFFLINE=true bash scripts/run-cov.sh`: 291 tests passed, none skipped.
- `nix flake check -L`: all host checks passed, including 291 tests with none skipped.
- Compiler integration tests include generated runners and external plugin fixtures.

Validation ran on `aarch64-darwin`; Nix omitted incompatible systems. Nextest reported a passing CLI
argument test as leaky. Coverage reports and command logs remain local under `target/`.
