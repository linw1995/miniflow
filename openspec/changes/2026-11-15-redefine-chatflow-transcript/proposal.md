## Why

`mfn-llm::ChatMessage` already represents assistant tool-call batches and tool results, but the
`mf-chat` boundary still reduces a successful workflow to one text reply. Storage reserves two
messages per turn and cannot retain tool messages or repeated assistant messages. The built-in
Agent keeps tool exchanges inside its Loop and exports only its last text. Consequently, later
chat turns cannot receive the actual Agent transcript. Agents also need to edit, remove, reorder,
or summarize historical context across turns; an append-only model history cannot support that.

## What Changes

- **BREAKING:** replace chatflow `prompt`, `system`, and `history` inputs with one required
  `messages` input containing the complete request; retain optional application `context`.
- **BREAKING:** expose append, replace_context, or both as independent complete result outputs.
  Each selected payload owns status/usages and optional error/title; common facts are not separate
  outputs that would require another branch merge. Produce exactly one alternative per invocation.
- The append result carries only a messages increment and its facts, without a full context snapshot.
  A simple Flow needs only that result output, not a router, unused compact branch, or kind wrapper.
- The replace_context result carries full context_messages, display_messages, and that branch's own
  facts. The host appends nothing to the snapshot. No author-side convergence is required.
- The host normalizes these named outputs into MessageUpdate::Append or MessageUpdate::ReplaceContext.
  With both outputs declared, optional selections omit the skipped branch; the host rejects zero,
  both, repeated, or undeclared results instead of choosing the first one.
- Keep the built-in conversation Flow responsible for application lifecycle orchestration only:
  admission, invocation, routing/validation, optional application titles, and shared-handler settlement.
  Answer/tool/compaction policy remains in the user chatflow; resource management and abort recovery
  remain in the host rather than depending on success-path nodes.
- Reuse the existing role-tagged `mfn-llm::ChatMessage`, `ToolCall`, and tool-result representation.
  Share transcript validation rather than introducing a parallel chat-only message schema.
- Make the built-in Agent consume messages and return every produced assistant and tool message,
  including its final assistant response, without returning the input prefix.
- Separate editable, revisioned model history from immutable turn/message audit records. A Flow
  can persist content, metadata, deletion, reordering, and summary changes without falsifying the
  original user submissions, ToolCalls, tool effects, or accounting.
- Persist arbitrary-length display transcripts, selected alternatives, and exact terminal results atomically.
  Publish context only on success against the admitted context revision. Retain valid failed updates
  for audit without publishing their candidate context.
- Extend save-only recovery and idempotency to both successful and failed terminal results.
- Render only each variant's display additions in chat, never its replacement context, including
  retained unsaved results and reloads. Keep original records and accounting per turn.
- Preserve the optional text-only `answer` stream as provisional presentation. Tool records become
  visible from the authoritative terminal transcript; live tool lifecycle events are separate work.

## Non-goals

No legacy workflow adapter, protocol negotiation, host-side tool execution, external tool handoff,
resumable tool rounds, approval protocol, exactly-once tool side effects, live tool lifecycle stream,
multimodal content, reasoning persistence, host-managed compaction policy, per-message patch/merge
operations, destructive audit editing, or new runtime execution primitives. Flow-authored history
compaction is supported through explicit replacement.

The model node's existing text and finish-reason projections remain available outside this chatflow
contract.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `chat-executable-contract`: message-list admission, alternative result outputs, and strict normalization.
- `chat-session-storage`: variant-specific context publication, immutable transcripts, and atomic settlement.
- `chat-workflow-execution`: append/replacement composition, display-only presentation, and titles.
- `llm-node-execution`: reusable message validation and built-in Agent transcript exports.

## Impact

Changes affect `mf-chat` public contracts, revisioned context storage, application Flow adapters,
title preparation, `mfn-llm` shared message validation and Agent Flows, `mf-tui` presentation, chat examples,
fixtures, and documentation. Existing chat executables must be updated and recompiled. A forward
SQLite migration preserves existing text conversations and accounting; missing historical tool
exchanges cannot be reconstructed. Existing direct/optional output selection and ordinary object
construction support single or branching results without a Flow-format, ValueType, or manifest
version change. Compiler/runtime execution semantics do not change. The host performs enum
normalization and invocation-wide exactly-one validation; graph convergence is not required.
