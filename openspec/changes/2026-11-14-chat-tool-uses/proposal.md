# Proposal

## Why

Chat currently drops tool activity when its transient observation is released. The shared controller now captures
and stores bounded public tool evidence, but terminal chat does not render it. Users need actual tool status and
parameters/results inline during execution and when reopening conversations, without replaying a workflow.

## What Changes

- Persist optional, bounded, redacted tool-use journals by session/turn/call in shared `mf-chat` storage.
- Collect from validated tool-dispatch dataflow, including the Flow-property classifier merged in #294; keep
  execution outcome separate from model text and successful conversation completion.
- Display live and saved tool evidence inline in terminal chat, including failed/cancelled user-only turns,
  incomplete observation, and explicit missing legacy evidence.
- Preserve tool records through cancellation, restart, paging, new turns, and save-only recovery without generation.
- Keep this change limited to ToolUse. No desktop frontend, graphical dependency, theme, license-policy change,
  packaging work, or desktop proposal belongs to this change.

## Capabilities

### Modified Capabilities

- `chat-session-storage`: Durable public tool evidence, forward migration, settlement/recovery, and message-page access.
- `workflow-terminal-ui`: Inline tool-use status and safe details in live and historical terminal conversations.

## Non-Goals

- Desktop UI, new launch modes, settings forms, or additional dependencies.
- Model-text inference, reconstruction of unrecorded historical calls, or raw telemetry replay.
- Adding tool journals to model-input history or changing executable workflow outputs.

## Impact

Changes touch `mf-chat`, the shared executable controller and terminal transcript in `mf-tui`, CLI regression tests,
and chat documentation. The forward migration adds nullable `model_calls.tool_uses`; legacy data remains unrecorded.
