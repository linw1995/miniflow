## Context

The generated Cargo build uses selected providers to validate graph contracts and produce execution layouts and a frozen manifest. Normal runner startup prepares providers again and verifies that their declarations match that manifest before dispatching nodes or consuming sources.

## Decisions

- Keep one build-time validation path and normal startup validation. Remove post-build runner execution, including for warm builds; Cargo fingerprints invalidate changed configuration and provider inputs.
- Parse execution parameters directly into `WorkflowArguments`. There is no command-mode enum after inspection and validation options are removed.
- Read graph and interface records from executable sections. The manifest reader returns a manifest or a typed error; absence no longer selects an optional compatibility path.
- Preserve stream protocol checks, parameter validation, and stdin ownership checks at TUI launch. Preserve typed error chains through Snafu selectors.
- Keep executable section and standalone inspection coverage in CLI tests and manifest generation coverage in compiler tests. Compiler execution tests focus on behavioral parity and failure preservation without a second binary parser.
- Keep the native manifest-bearing shell wrapper used by terminal-supervision tests. It exercises process supervision without compiling a business workflow for each terminal failure scenario.

## Compatibility and Tradeoffs

The removed runner flags now report unknown arguments. Manifest-free runners remain directly executable but must be recompiled for TUI inspection. Runtime resource initialization failures and host/target interface disagreements are reported at startup instead of installation. No new inspection command or fallback protocol is introduced.

## Validation

Review module boundaries and Snafu handling against AGENTS.md. Exercise simplification and guard-removal variants locally, retaining reports only under ignored `target/`. Run parser, manifest, terminal-supervision, generation, and complete pinned Nix regressions, plus repository hooks and strict OpenSpec validation.
