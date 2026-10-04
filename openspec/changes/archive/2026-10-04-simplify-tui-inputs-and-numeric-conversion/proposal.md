# Simplify TUI input handling and verify numeric text conversion

## Why

TUI observation does not need every standalone input mode. File sources already receive paths through
workflow parameters. CEL already provides explicit string numeric conversions through its standard library.

## What Changes

- Remove the TUI stream-input option and file-descriptor forwarding; always launch children with null stdin.
- Reject active stdin requirements before TUI execution and retain file-source parameter support.
- Verify and document `int(text)` and `double(text)`, including invalid numeric text and range failures.
- Replace generic resource vectors and execution-resource wrappers with optional stdin metadata and input.
- Remove default-frame heap allocations and the redundant CEL dependency API smoke test.
- Update the CEL scalar example to convert a string input explicitly.

## Impact

Capabilities: workflow-terminal-ui, stream-sources, code-node-execution, node-preparation. Standalone stdin ingestion remains
available. No numeric coercion or new conversion backend is introduced.
