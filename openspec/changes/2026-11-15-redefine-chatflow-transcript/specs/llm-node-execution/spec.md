## ADDED Requirements

### Requirement: Share strict transcript validation independently of providers

`mfn-llm` SHALL expose provider-independent validation of its existing `ChatMessage` transcript.
Request preparation and chat persistence SHALL share role/content, call-identity, and pending-result
rules. Validation SHALL preserve indexed typed errors without constructing provider requests or
acquiring transport resources. Complete requests MUST resolve all calls; partial-prefix validation
MAY leave only the final batch unresolved.

#### Scenario: Validate without credentials

- **WHEN** a caller validates a complete transcript without provider configuration or environment credentials
- **THEN** structural validation succeeds or returns a typed indexed error without network setup

#### Scenario: Distinguish complete requests from failed prefixes

- **WHEN** a transcript ends with an assistant call batch without its results
- **THEN** complete request validation rejects it
- **AND** explicit partial-prefix validation can retain it for unsuccessful audit data

#### Scenario: Reject invalid partial prefixes

- **WHEN** a failed prefix contains orphan results, duplicate IDs, or continuation beyond unresolved calls
- **THEN** partial-prefix validation rejects it with an indexed error just as complete validation does

#### Scenario: Validate a context snapshot separately from display

- **WHEN** the same tool exchange is present once in a continuation snapshot and once in its display delta
- **THEN** callers can validate each transcript independently without a collision across the two lists
- **AND** an empty snapshot is structurally valid even though an actual model request still requires messages

#### Scenario: Validate an explicitly rewritten tool exchange

- **WHEN** a caller deletes a complete exchange or coherently edits a call ID and its matching results in model context
- **THEN** validation accepts the structurally valid resulting transcript without requiring equality to an audit log
- **AND** orphaned results or an ID collision in the resulting context still fail

### Requirement: Persist messages through the canonical codec

JSON serialization for persistence SHALL agree with strict runtime decoding of the shared
`ChatMessage`, `ToolCall`, and `ToolResult` payloads. Role-specific fields, unknown-field rejection,
optional metadata, and nested tool arguments MUST NOT acquire different semantics through a
parallel storage DTO.

#### Scenario: Round-trip strict message data

- **WHEN** a valid message containing calls, arguments, results, or metadata is encoded for storage and decoded again
- **THEN** it retains the same canonical value as runtime input/output conversion
- **AND** malformed fields are not accepted only because the value came through JSON storage

### Requirement: Export built-in Agent transcript additions

The Agent SHALL preserve supplied messages and return only generated assistant/tool additions.
Its append policy MUST NOT restrict enclosing Flow context replacement. Each assistant, including
the final answer, SHALL appear once; parallel tool results SHALL follow call order. The Agent SHALL
continue while the model requests tools, without an Agent-specific round limit. Captured failures
SHALL return available messages/usages with failed status, not an invented assistant or echoed input prefix.

#### Scenario: Return one tool round and a final answer

- **WHEN** the Agent receives calls, executes their tools, and then receives a final assistant answer
- **THEN** messages contains the call-bearing assistant, ordered tool results, and final assistant exactly once
- **AND** the supplied input messages are unchanged and absent from the returned additions

#### Scenario: Supply a transformed request

- **WHEN** an enclosing Flow replaces old exchanges with edited messages or a summary before calling the Agent
- **THEN** the Agent uses that supplied request and returns its generated suffix without restoring the discarded context
- **AND** the enclosing Flow can use those additions in a complete append result or a separate replace_context result without merging alternatives

#### Scenario: Capture a later model failure

- **WHEN** a provider failure is captured after earlier completed tool rounds
- **THEN** the Agent returns failed status with those rounds and every collected usage
- **AND** it does not add an empty placeholder for the failed model response

#### Scenario: Check a completed tool batch before continuing

- **WHEN** the caller supplies a compaction Flow and its policy replaces a prefix after a completed tool batch
- **THEN** the next conversation request uses the checkpoint and only subsequent additions, with no replay of the batch
- **AND** the generated transcript remains complete and auxiliary calls retain separate usage attribution

#### Scenario: Compaction fails before the next request

- **WHEN** the configured policy reports a failed summary after tools have completed
- **THEN** the Agent returns the completed tool transcript, captured usages, and an error without another conversation request
- **AND** cancellation still aborts the workflow rather than publishing a partial summary

#### Scenario: Continue a long tool exchange

- **WHEN** the model requests tools for nine consecutive rounds and then returns a final answer
- **THEN** the Agent returns success with all nine exchanges, the final assistant, and usage for all ten calls

### Requirement: Preserve generic Agent composition boundaries

The Agent SHALL remain independent of chat storage and host-side tool execution. Tool catalog
projection SHALL run outside the model loop. The LLM node's text, message, finish_reason, and usage
projections SHALL remain available to other workflows. Application-owned update enums SHALL NOT
move into LLM contracts; enclosing Flows compose complete results from generic message/status/usage
outputs. Optional answer streaming SHALL remain provisional and separate from generated additions.

#### Scenario: Add a tool without changing the Agent graph

- **WHEN** a caller supplies a different native tool map and matching properties
- **THEN** the fixed Agent definition uses the same messages contract and projects its catalog once before model iteration
