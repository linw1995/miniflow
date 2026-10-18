## Why

The chat terminal exposes session management, usage, multiple executions, and historical inspection at once.
The requested scope is a smaller conversation client with session switching, cancellation, and reliable recovery.

## What Changes

- Use a full-width transcript, adaptive input, contextual shortcuts, and command/session overlays.
- Create and name a session from its first prompt. Keep drafts when switching idle sessions.
- Replace the multi-worker scheduler with one execution lane and reject additional submissions instead of queuing them.
- Remove rename, archive/restore, usage dashboards, automatic-title setup, and historical-run navigation from the terminal.
- Keep the shared workflow contract, persistence, metadata, call facts, and current-run inspection.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-terminal-ui`: Conversation-first interaction with one execution lane.

## Impact

Changes affect `mf-cli` and `mf-tui` interaction, controller state, tests, and documentation. No storage schema or
workflow protocol changes are required. Existing `mf-chat` management and usage APIs remain available to applications.
