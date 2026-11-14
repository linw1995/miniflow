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

- **WHEN** raw assistant messages differ from the workflow's canonical terminal text
- **THEN** tool extraction does not silently substitute the raw answer for that terminal text
