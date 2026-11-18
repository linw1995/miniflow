# Design

## Provider and conversation streams

Rig buffers Chat Completions arguments before yielding its tool-call event. The existing SSE normalizer captures nonempty string fragments from the primary choice before that assembly. The synchronous publisher drains them on the same poll that reads the frame, including when Rig is still waiting for the rest of the call. This adds no second loader or provider request. Rig still owns validation and completed model output.

`tool_call_delta` carries raw arguments, model-call identity, provider tool index, and optional wire ID/name hints. An identity-only record retains its hints with an empty argument string. The shared Agent forwards only this optional model-output port. ToolUses and `llm.tool_result` keep their ordinary execution contracts. The chat host accepts only declared previews, shares acknowledgement and cancellation with text previews, and admits terminal alternatives before preview validation.

## Execution observations

Router can opt into a static observation category and an optional public `observation_id`. ToolUses supplies the tool-call ID on the existing classification-to-Router data edge. Call measures the selected body inside its invocation boundary and reports the outcome before business-error packing. An unused route or rejected call cannot emit a start. Records carry an attempt identity, public labels, phase, and the existing source timestamp and trace context, not arguments, results, or error payloads.

`mf-chat` collects execution metrics per invocation through the existing local observation callback. The TUI receiver reduces the same records after run and workflow validation. Each collector bounds retention, accepts duplicate or reordered delivery, and leaves conflicting or missing terminal evidence unknown. No separate exporter, model stream, storage schema, or dispatch step participates.

## Presentation

The live cache groups arguments by invocation, model call, and index. Rendering and keyboard
navigation use the same live tool group projection. Each group folds its arguments and any result
behind a leading `[+]` or `[-]` marker. Browse-mode Tab/Shift+Tab selects groups in display order;
Space or Ctrl+T toggles the selection. A late, unambiguous wire ID transfers focus, expansion, and
scrolling to the canonical group key. Only a correlated execution observation updates the group
status. Missing or ambiguous hints do not match by position or name.

Authoritative messages replace previews without duplicating a call. If both are visible on the current history page, results appear under their matching calls within the same turn and assistant batch. A result-only page keeps the ID and the same fold key. Grouping does not reorder stored messages. Arguments and results remain literal; assistant text keeps Markdown rendering.
