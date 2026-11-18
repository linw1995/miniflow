## ADDED Requirements

### Requirement: Stream raw tool-call argument fragments

The LLM node SHALL expose an optional `tool_call_delta` stream with original nonempty string fragments from the primary Chat Completions choice. Each fragment SHALL carry the model-call identity and provider tool index. The existing completed `tool_call` event and terminal outputs SHALL retain their contracts. Raw fragments are previews only and MUST NOT affect validation of a completed call.

#### Scenario: Forward fragments before assembly

- **WHEN** the provider sends a tool's arguments in multiple SSE events
- **THEN** each raw fragment is available without waiting for a closing SSE event or parseable JSON
- **AND** fragments for distinct indices remain distinguishable while the completed call is still assembled and validated by the provider adapter

#### Scenario: Forward identity before or after argument data

- **WHEN** the provider sends a nonblank call ID or name in an SSE event without a nonempty argument fragment
- **THEN** `tool_call_delta` forwards the hint with its model-call identity and index and an empty `arguments` string
- **AND** the hint does not append argument bytes or establish completed-call validity

#### Scenario: Preserve incomplete-call behavior

- **WHEN** the provider response ends with malformed or truncated arguments
- **THEN** its raw fragments may have been previewed but no completed event is synthesized from the partial arguments

## MODIFIED Requirements

### Requirement: Emit source-timestamped model request observations

Each attempted provider operation SHALL emit request-started and request-finished observations with a call identity. Streaming calls SHALL emit one first-output observation before publishing the first nonempty text or tool-call delta. The observation layer SHALL supply timestamps. LLM execution MUST NOT calculate request duration or first-output latency using a node-local timer.

#### Scenario: Receive a streaming answer

- **WHEN** a provider sends empty or metadata-only events before nonempty text
- **THEN** the first-output event identifies the first nonempty decoded text before publication and backpressure
- **AND** the request-finished event precedes terminal conversion or persistence

#### Scenario: Start with a tool call

- **WHEN** a provider sends a tool-call identity, name, or argument delta before any text
- **THEN** the first-output event precedes publication of that delta
- **AND** later text or tool-call deltas do not emit another first-output event for the same call

#### Scenario: Complete without a stream subscription

- **WHEN** the node executes its unary request path
- **THEN** it emits request boundary observations without a first-output event

#### Scenario: Capture a failed provider attempt

- **WHEN** a provider operation fails and the node returns a captured failure
- **THEN** request observations retain the call identity and available facts without claiming successful throughput
