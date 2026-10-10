# Verification

- Runtime codec/validator tests passed with exact numeric and signed-zero boundary cases.
- Code numeric adapters and generated/in-memory execution parity tests passed.
- Workspace Clippy and all repository hooks passed under the pinned toolchain.
- The active OpenSpec delta passed strict validation.
- Native aarch64-darwin `nix flake check -L` passed, including 496 tests with no skips.
- The full workspace nextest run independently passed all 496 tests.
- Focused coverage passed 104 tests and produced the local LCOV report.
- Five reversible guard controls failed as expected; the unmodified baseline passed and all mutations were restored.

Linux validation is delegated to the pull request checks because native Nix validation omits incompatible systems.

Detailed experiment and coverage artifacts remain under ignored target/ and are excluded from commits.
