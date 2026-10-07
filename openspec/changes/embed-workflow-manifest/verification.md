## Validation environment

Rust `1.98.1` through `nix develop`. Native macOS validation runs on `aarch64-darwin`; Linux checks use the system's configured `aarch64-linux` Nix builder.

## Results

- OpenSpec strict validation: passed.
- Git hook installation and `prek -a`: passed, including formatting, Clippy, cargo check, rust-analyzer, Markdown/YAML/TOML checks, actionlint, and the dependency/license audit.
- Full workspace: `cargo nextest run --locked --workspace --all-targets --all-features --profile ci --test-threads 4` passed all 441 tests with no skipped tests.
- Coverage: `scripts/run-cov.sh` passed the 14 selected manifest/preflight tests. Reports remain local in `target/coverage/result/`, including `lcov.info`; this was targeted coverage, not a whole-workspace coverage run.
- Manifest contract line coverage: 104/107 (97.20%); executable reader line coverage: 177/203 (87.19%).
- Native macOS `nix flake check -L`: passed with the final test environment; the isolated release test suite passed all 441 tests.
- Native Linux Nix test, Clippy, format, and workflow checks: passed; the isolated release test suite passed all 441 tests, including ELF release/LTO/strip retention. The native targets exercised were aarch64 Linux and macOS; x86_64 container inspection is covered by fixtures.

## Requirement coverage

| Behavior | Evidence |
| --- | --- |
| Portable framing, exact opaque names, protocol versions, bounded payload/padding | `mf-runtime::workflow_manifest` round trips, malformed framing/version/length tests, combined serialization limit |
| Duplicate JSON members and graph/interface consistency | Runtime manifest tests retain typed JSON/graph/interface error sources and reject mismatched identities or incomplete roots |
| One build-time provider preparation for task/stream layouts and configured metadata | `mf-compiler::manifest_generation` counts construction, compares dynamic schemas, and preserves empty older-schema interfaces |
| Warm builds, layout compatibility, invalid preparation/install preservation | Compiler cache tests and CLI warm-build/configuration/failed-validation tests |
| Release/LTO/strip retention with telemetry enabled and disabled | `mf-compiler::manifest_retention` checks the native section and compatibility records after platform stripping |
| Factory-free compatibility commands, including noisy or unavailable runtime factories | Compiler runner-validation and interface-agreement fixtures |
| Runtime port membership/type/required/resource drift, before dispatch | Runtime mismatch diagnostics and generated task/stream interface-agreement tests; execution marker stays absent |
| Validation drift preserves an installed executable and lock | Interface-agreement subprocess validation fixture |
| Bounded executable reads, both container formats and supported architectures | TUI manifest fixtures include x86_64/aarch64 ELF64/Mach-O64, a 512 MiB sparse executable, forged ranges, oversized tables/payloads, duplicate sections, and wrong segment/type |
| Invalid metadata never invokes fallback | TUI preflight rejects a corrupt manifest with a typed manifest failure; unsupported files and versions are rejected by the shared reader/decoder |
| Legacy finite and parameterized executables | TUI preflight compiles native manifest-free wrappers; explicit legacy command tests retain timeout, output, exit, framing, and unsupported observation checks |
| Argument ownership, conditional stdin, and metadata preflight without execution | Non-executable manifest fixtures validate private argument transport and reject active stdin before process launch |
| Terminal/process outcomes, cleanup, snapshots, and receiver ordering | CLI native legacy PTY tests and the generated streaming TUI integration test |
| Standalone operation without build inputs or tooling | CLI manifest integration moves task/stream/Loop runners, deletes definitions/locks/build directories, empties PATH, and compares direct records, commands, validation, and results |
| Manifest graphs agree with actual observations | Loop observation integration compares the embedded graph before receiving and checking complete per-pass telemetry; the full suite covers task/stream observations and execution parity |

## Compatibility and scope

- Existing workflow/graph/interface/event protocol versions are unchanged. The manifest adds payload version `2026-10-07` and framing version `1`.
- Supported native containers are ELF64 and thin Mach-O64. Universal Mach-O and PE are outside this change.
- Inspection flags remain supported. Only a recognized executable without the section uses command fallback.
- Generated-project layout identity changes to `2026-10-07`; old explicit build directories require a compatible location.
- Startup-interface comparison does not authenticate executable contents or compare every internal node field. Existing graph/layout validation continues to own those contracts.

## Test infrastructure correction

Linux Nix isolation exposed an existing generated-build test wrapper with an unpatched `#!/usr/bin/env bash` interpreter path. The test derivation now runs `patchShebangs scripts/nextest-cargo.sh` before validation, so Cargo subprocesses use the pinned shell available inside the sandbox. This changes the test environment only; runner generation and execution semantics are unaffected.

Linux isolation also lacked a system CA bundle required when constructing the HTTP exporter client. The Linux test derivation supplies the pinned `cacert` bundle through `SSL_CERT_FILE`, preserving certificate verification. Linux Nix tests collect all failures to expose environment regressions in one run.
