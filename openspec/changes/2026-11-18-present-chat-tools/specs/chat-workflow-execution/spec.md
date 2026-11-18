## ADDED Requirements

### Requirement: Optional invocation-scoped tool-call argument previews

Chat workflows MAY expose an optional `tool_call_delta` stream independently of `answer`. The host SHALL decode declared argument fragments, attach the active invocation, and use the same delivery receipts and cancellation as text previews. Argument previews MUST NOT dispatch tools, report tool execution states, or modify history, model context, usage, or settlement.

#### Scenario: Receive fragments before the turn is complete

- **WHEN** a chatflow selects `tool_call_delta` and the model emits raw tool arguments
- **THEN** the host can deliver a fragment without waiting for the rest of the arguments, a tool result, or the terminal turn result

#### Scenario: Reject malformed or undeclared argument previews

- **WHEN** a workflow emits an undeclared tool argument stream, a malformed fragment, or a blank model-call identity
- **THEN** preview decoding fails without concealing conflicting terminal alternatives or saving a provisional value

#### Scenario: Preserve existing text-only workflows

- **WHEN** a chatflow exposes only `answer` and a terminal alternative
- **THEN** its preview and settlement behavior remains unchanged
