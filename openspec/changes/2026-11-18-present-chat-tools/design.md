# Design

## Explicit preview streams

The LLM emits raw argument fragments keyed by model-call identity and provider part index, then the finalized validated call. Shared Agent Flows forward `tool_call`, `tool_started`, and `tool_finished` as separate optional streams. Explicit start prerequisites precede dispatch; result encoding emits completion from the actual business envelope, without guessing from arbitrary result text. No private Flow bindings enter these events.

The chat host decodes only declared streams, validates event kinds and identities, and wraps them in the active invocation. Text and tool events share acknowledgement, cancellation, and delivery handling. Terminal alternatives are admitted before preview validation, so malformed previews cannot hide conflicting terminal results. Existing text-only chatflows remain valid; tool presentation requires explicit output selections.

## Presentation

The bounded live preview preserves event order and correlates model fragments with wire call IDs.
Completed calls reconcile partial argument JSON. Parallel tools keep independent states. Independently
forwarded execution events reconcile with partial call identities without duplicating tools or
downgrading finished states. Missing terminal results leave outstanding activity visibly interrupted,
never successful. Authoritative completion replaces all provisional records, including during
save-only recovery and reload.

Tool results are collapsed by default. Expansion keys combine turn identity and tool-call identity, remaining stable across preview replacement and history reload without changing persisted data. Browse-mode Tab/Shift+Tab selects a result; Space or Ctrl+T toggles it. Tool arguments and results remain literal, with control characters escaped and no image loading. Assistant text retains Markdown rendering.

The live cache bounds both payload and correlation metadata. Dropped previews do not stop event consumption, status updates, tool execution, or final settlement.
