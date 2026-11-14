# Structured Tool Message Contract

## ADDED Requirements

### Requirement: Expose optional structured conversation messages

Chat workflows MAY accept complete input messages and return complete authoritative context messages, with an optional declared immutable message-event stream. A Flow MAY compact, reorder, replace, or remove history and system instructions. The host SHALL retain support for workflows omitting these ports. The Flow SHALL NOT need a persistence boundary count or host cursor. Raw message text SHALL NOT replace canonical final text implicitly.

#### Scenario: Receive a message and its final-list replay

- **WHEN** an assistant tool-call message arrives in the stream and is repeated in the terminal message result
- **THEN** its presence in a context version does not create another execution or erase the immutable event evidence

#### Scenario: Expose an Agent transcript

- **WHEN** the Agent's internal context contains system/history input, the admitted prompt, and new assistant/tool messages
- **THEN** it returns that complete context directly, without maintaining a second transcript or excluding a host-owned prefix

#### Scenario: Replace earlier history

- **WHEN** a custom Flow summarizes, edits, reorders, or removes earlier messages and returns a valid complete context
- **THEN** the next invocation receives exactly that successful returned context plus its new user message
- **AND** older history and removed system instructions are not reinserted from admission or audit records

#### Scenario: Publish a message fact

- **WHEN** a Flow uses the message publisher with complete messages and optional model origin
- **THEN** the publisher generates a scoped event identity and the host assigns/replays its journal cursor
- **AND** a different final context does not retroactively rewrite the published fact

#### Scenario: Run an older workflow

- **WHEN** a compatible workflow has no structured message output
- **THEN** its normal reply still works and any actual tool observations remain available as observation-only evidence

#### Scenario: Postprocess the final answer

- **WHEN** a workflow supplies both canonical terminal text and structured messages after postprocessing
- **THEN** its final assistant message represents that canonical reply and the host does not append another reply from text
- **AND** incompatible final content is rejected explicitly rather than silently rewriting the conversation

### Requirement: Retain accepted messages as recoverable conversation data

The host SHALL persist every structured message accepted by the chat contract, including roles, content, metadata, assistant tool calls, arguments, and tool-result links. ToolUse extraction SHALL NOT replace message storage. Successful completion SHALL require durable, reconciled message data; save-only recovery SHALL retain that data without another execution. A legacy workflow omitting messages SHALL retain the existing text fallback.

#### Scenario: Save only a tool display summary

- **WHEN** a workflow returns valid structured messages containing tool calls and results
- **THEN** persisting only display summaries or final text is insufficient to report the conversation saved

#### Scenario: Restore and continue after restart

- **WHEN** the host reopens a successful conversation and submits its next turn
- **THEN** the next Flow receives the latest successful complete context and metadata in its exact stored order
- **AND** the provider adapter receives the supported conversation fields, without promoting opaque metadata to provider parameters
- **AND** model context is not reconstructed from diagnostic observations or clipped tool display strings

#### Scenario: Persist a valid message larger than the preview budget

- **WHEN** an accepted message or argument exceeds a tool-display limit
- **THEN** the host retains its complete valid recovery representation independently of the bounded display copy
