## ADDED Requirements

### Requirement: Explicit invocation-scoped tool previews

Chat workflows MAY expose optional `tool_call`, `tool_started`, and `tool_finished` streams. The host SHALL validate each event against its declared stream and attach the active invocation. Tool previews SHALL share text-preview acknowledgement and cancellation semantics and MUST NOT mutate audit history, model context, usages, or settlement decisions.

#### Scenario: Stream parallel calls before completion

- **WHEN** an Agent streams argument fragments and dispatches multiple tools
- **THEN** fragments are correlated by model-call identity and part index, and execution events identify the actual tool-call ID
- **AND** each result is delivered before the next model response without requiring final turn settlement

#### Scenario: Reject invalid or undeclared activity

- **WHEN** a workflow emits an undeclared tool stream, mismatched event kind, or blank identity
- **THEN** preview decoding fails without concealing conflicting terminal alternatives or saving provisional events

#### Scenario: Preserve existing text-only workflows

- **WHEN** a chatflow exposes only `answer` and a terminal alternative
- **THEN** its preview and settlement behavior remains unchanged
