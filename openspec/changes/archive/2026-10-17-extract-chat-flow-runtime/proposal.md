## Why

Applications need the executable chat backend independently of the terminal interface. The existing
workflow manifest and input resolver already provide the runtime contracts; chat should add only
conversation admission, model accounting, and durable reply reconciliation.

## What Changes

- Extract the executable contract and persistence lifecycle into `mf-chat::executable`.
- Support one LLM task, optionally fed by a parameter node, using existing runner execution modes.
- Share save-only recovery with the embedded host and retain typed errors and invocation ownership.
- Include a parameterized workflow example, focused regressions, and application integration guidance.

## Capabilities

### New Capabilities

- `chat-executable-contract`: Application-owned conversation execution and recovery for compiled workflows.

### Modified Capabilities

None. Existing storage and embedded workflow contracts remain authoritative.

## Impact

Changes are limited to `mf-chat`, a CLI example regression, the workflow example, and documentation.
Process supervision, scheduling, terminal interaction, and CLI chat flags remain outside this change.
No runtime/compiler protocol extension or database migration is introduced.
