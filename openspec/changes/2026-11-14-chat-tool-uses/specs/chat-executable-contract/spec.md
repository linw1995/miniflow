# Structured Tool Message Contract

## ADDED Requirements

### Requirement: Expose optional structured conversation messages

Chat workflows MAY return current-invocation structured messages and a declared typed message-event stream. The host SHALL retain support for workflows omitting them. Message identity/order SHALL permit idempotent event replay and final-result reconciliation. Historical inputs SHALL NOT be reingested as newly generated messages, and raw message text SHALL NOT replace canonical final text implicitly.

#### Scenario: Receive a message and its final-list replay

- **WHEN** an assistant tool-call message arrives in the stream and is repeated in the terminal message result
- **THEN** it describes one semantic call using the same source identity, not a second execution

#### Scenario: Expose an Agent transcript

- **WHEN** the Agent's internal context contains system/history input, the admitted prompt, and new assistant/tool messages
- **THEN** its public new-message output explicitly excludes the inherited prefix rather than asking the host to guess it

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
- **THEN** the provider receives its stored canonical assistant/tool message sequence and metadata in order
- **AND** model context is not reconstructed from diagnostic observations or clipped tool display strings

#### Scenario: Persist a valid message larger than the preview budget

- **WHEN** an accepted message or argument exceeds a tool-display limit
- **THEN** the host retains its complete valid recovery representation independently of the bounded display copy
