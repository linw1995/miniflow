# Flatten Workflow Arguments

## Why

Passing a source path currently requires a nested JSON document. Qualified command-line parameters make ordinary workflow invocations readable while preserving independent inputs for each initial node.

## What Changes

- Accept `--node.port VALUE` and `--node.port=VALUE` in standalone runners and TUI launches.
- **BREAKING**: use flat qualified keys in JSON and in-memory workflow arguments; reject the previous nested argument shape.
- Parse named values against the embedded input contract and reject ambiguous qualified names during preparation and manifest validation.
- Publish startup interface version `2026-10-11` so launchers reject older nested-argument runners before execution.
- Preserve JSON file transport, typed validation, conditional stdin ownership, and invocation isolation.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-inputs`: define flat parameter identity and validation.
- `workflow-binary-compilation`: accept typed named parameters through the shared runner parser.
- `workflow-terminal-ui`: forward named parameters through validated private transport.

## Impact

Runtime argument representation and parsing, generated entry points, CLI/TUI preflight, input binding, existing regressions, and usage documentation change. Node port contracts, graph dependencies, and manifest framing remain unchanged.
