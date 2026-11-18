# Design

## Provider and conversation streams

Rig buffers Chat Completions arguments before yielding its tool-call event. The existing SSE normalizer captures nonempty string fragments from the primary choice before that assembly. The synchronous publisher drains them on the same poll that reads the frame, including when Rig is still waiting for the rest of the call. This adds no second loader or provider request. Rig still owns validation and completed model output.

`tool_call_delta` carries raw arguments, model-call identity, provider tool index, and optional wire ID/name hints. The shared Agent forwards only this optional model-output port. ToolUses and `llm.tool_result` keep their ordinary execution contracts. The chat host accepts only declared previews, shares acknowledgement and cancellation with text previews, and admits terminal alternatives before preview validation.

## Presentation

The live cache groups tool arguments by invocation, model call, and index. It bounds retained text and metadata without blocking preview consumption. It never labels a preview as a tool execution or uses it to dispatch a tool. An authoritative result replaces the preview rather than appending a duplicate call.

Results come from the authoritative display transcript, including retained replies. The TUI places each visible result under its matching call within the same turn and assistant batch, in call order. A page without the matching call shows the result on its own with its call ID. Grouping does not reorder stored messages.

Results are collapsed by default and keyed by turn and wire call ID; fold state survives save and page reload. The literal `[+]` or `[-]` marker begins the result header, before any ID that could wrap. Browse-mode Tab/Shift+Tab selects results in display order; Space or Ctrl+T toggles the selection. Arguments and results remain literal. Assistant text keeps Markdown rendering.
