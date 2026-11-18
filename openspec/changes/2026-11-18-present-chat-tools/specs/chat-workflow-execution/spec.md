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

### Requirement: Tool execution metrics from Flow observations

The chat host SHALL expose bounded invocation-local tool execution metrics from traced Flow call boundaries, independently of model streams. Observation records SHALL NOT change tool dispatch, usage, settlement, or persisted conversation state.

#### Scenario: Read metrics before the turn settles

- **WHEN** an opted-in Router invokes a tool Flow
- **THEN** the chat host can query its observed execution by tool-call ID during the turn
- **AND** a recoverable business error is failed evidence even when the Router returns a valid error envelope

#### Scenario: Do not infer missing execution evidence

- **WHEN** a route is rejected, records are missing or conflicting, or the observation closes without a terminal record
- **THEN** chat does not infer an execution or a successful outcome
