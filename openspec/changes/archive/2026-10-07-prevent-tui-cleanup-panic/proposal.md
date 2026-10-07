## Why

When a TUI display disconnects after a completed frame, Ratatui's cursor restoration fails and its destructor prints to the same unavailable stderr. This can panic and replace the expected CLI failure exit code with 101.

## What Changes

- Detach the rendering backend from stderr before destruction and keep terminal restoration with the existing guard.
- Wait for the first frame's cursor-hide command before the existing regression closes its display PTY.
- Keep rendering errors observable and use standard I/O types without a custom writer or panic handler.

## Capabilities

### Modified Capabilities

- `workflow-terminal-ui`: finish cleanup without panic when display output is unavailable.

## Impact

Changes affect TUI teardown and one existing CLI integration test. Runtime APIs, workflow results, and dependency versions remain unchanged.
