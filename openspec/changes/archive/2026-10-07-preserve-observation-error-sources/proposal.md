## Why

Compiler observation setup and snapshot pipelines discard typed causes before diagnostics are presented. UUID and trace/span parsing also replace parser errors with contract text, preventing callers from inspecting the failure.

## What Changes

- **BREAKING**: return compiler-owned `DescriptionError` from single-run observation setup, preserving Loop ordering causes and paths.
- **BREAKING**: return typed telemetry export, snapshot transport, runtime recorder, and TUI capture errors instead of strings.
- Retain UUID, trace/span, integer, digest, protobuf, JSON, and sink causes through the affected boundaries.
- Preserve cached delivery failures across flush, emit, and finish; format diagnostics at presentation boundaries.
- Use Snafu derives and selectors for the affected error paths.

## Capabilities

### Modified Capabilities

- `workflow-observability`: preserve description, identifier, transport, and export causes.
- `workflow-runtime-execution`: preserve snapshot sink failures until the recorder owner presents them.
- `workflow-terminal-ui`: retain snapshot decoding failures across admission and history presentation.

## Impact

Public error return types and exhaustive error matches change. Callers of snapshot sinks now return boxed `Error + Send + Sync` values. Generated runners are updated with the APIs; wire formats and workflow results remain compatible.
