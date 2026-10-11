# Verbose Workflow Execution

## Why

Issue #90 requests workflow execution through `mf run` with verbose diagnostics without requiring the terminal UI.
Standalone runners need the same diagnostics while preserving their result protocol and deployment independence.

## What Changes

- Make `--tui` optional for `mf run` and inherit the caller's standard streams for direct execution.
- Accept `-v` and `--verbose` in the CLI and generated runners.
- Print existing lifecycle metadata to stderr without requiring OTLP configuration or SDK support.
- Preserve workflow parameters, result stdout, process outcomes, and optional OTLP export.
- Forward verbose diagnostics into the existing TUI stderr capture and report verbose compilation details.
- **BREAKING**: public TUI `RunOptions` literals that enumerate all fields must include `verbose` or use default-based
  construction.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-binary-compilation`: direct CLI execution and verbose compilation.
- `workflow-observability`: local lifecycle diagnostics independent of export.
- `workflow-terminal-ui`: capture verbose child diagnostics through the existing stderr path.

## Impact

CLI dispatch, shared runner argument parsing, generated entry points, observation construction, existing acceptance
tests, and usage documentation change. Existing runners must be recompiled to accept the verbose option.
