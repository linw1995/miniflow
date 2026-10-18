## Why

Compiled conversation workflows need a persistent terminal client that owns sessions and process supervision.
The mainline chat backend already validates workflow contracts and implements storage, recovery, message metadata,
and per-call usage accounting. The terminal should consume these APIs directly.

## What Changes

- Add explicit `mf run --tui --chat --chat-db` invocation with runtime workflow parameters.
- Run the complete supplied workflow for every turn, including branches and repeated model calls.
- Provide persistent sessions, editable prompts, cancellation, explicit model retries, and save-only recovery.
- Share existing graph, trace, log, and data inspection between ordinary execution and chat turns.
- Show reported call usage and unreported invocations separately.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-terminal-ui`: Persistent chat interaction and supervised conversation execution.

## Impact

Changes affect `mf-cli`, `mf-tui`, test configuration, and documentation. Existing `mf-chat` executable,
storage, and recovery APIs remain authoritative. No compiler protocol, workflow format, or database migration is added.
