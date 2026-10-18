## MODIFIED Requirements

### Requirement: Validate explicit text conversation history

History SHALL be an ordered list of objects containing required string `role`, nonempty string `content`, and optional object-valued `metadata`.
Metadata SHALL be preserved but MUST NOT be sent to the provider automatically. Accepted roles SHALL be `user` and `assistant`; system instructions SHALL use the separate input. Omitted or empty history SHALL start a fresh conversation. Invalid history MUST fail before network I/O with an indexed diagnostic. Supplied order and content MUST NOT be silently changed.

#### Scenario: Start without history

- **WHEN** an invocation omits history or supplies an empty list
- **THEN** it sends only optional system instructions and the current user prompt as conversation messages

#### Scenario: Reject malformed history

- **WHEN** history is null or contains a missing field, unknown field, nonobject metadata, nonstring content, empty content, or an unsupported role
- **THEN** input validation fails with the offending history path and no provider request

#### Scenario: Preserve caller supplied history

- **WHEN** valid history contains consecutive messages with the same supported role
- **THEN** the request preserves their order and content without enforcing alternation, trimming, summarization, or deduplication

### Requirement: Publish a terminal text result atomically

A successful invocation SHALL publish exactly one result containing complete string `text`, updated message-list `history`, succeeded status, a one-call usages list, and optional scalar `usage` and `finish_reason`. Text blocks SHALL retain response order. Conversion and validation MUST finish before publication. Empty text, unsupported output content, invalid usage counts, and excessive output text SHALL fail without any outputs by default or transcript mutation.

#### Scenario: Receive a complete text answer

- **WHEN** a provider returns multiple text blocks and a terminal finish reason
- **THEN** the node publishes their ordered concatenation, updated history, and finish reason in one task result
- **AND** no intermediate chunks or incremental outputs are published

#### Scenario: Reject unsupported output

- **WHEN** the reply has no nonempty text or contains a tool call or unsupported content
- **THEN** with capture_errors disabled the invocation fails without publishing text, updated history, or metadata
- **AND** supplied history remains unchanged

#### Scenario: Exceed the text limit

- **WHEN** the decoded terminal text exceeds the configured UTF-8 byte limit
- **THEN** the node rejects the result before publication
- **AND** the limit is documented as a text output limit rather than a hard transport-memory bound

#### Scenario: Apply the reply limit independently of history

- **WHEN** the generated reply fits the output text limit but the full transcript is larger
- **THEN** that text limit does not trim or reject the transcript solely because of its aggregate size

#### Scenario: Complete at the token limit

- **WHEN** a reply contains nonempty text with a `length` finish reason
- **THEN** the result includes that text and reason without claiming an untruncated answer

### Requirement: Bound request waiting and honor cancellation

The node SHALL apply a positive invocation deadline and honor workflow cancellation while awaiting a reply. Deadline expiry SHALL drop the pending operation; it SHALL publish no result by default, or a failed call result when capture_errors is enabled. Cancellation SHALL publish no result. A deadline elapsed during synchronous initialization MUST prevent subsequent request polling. Local cancellation MUST NOT be described as guaranteed remote termination.

#### Scenario: Wait on a stalled endpoint

- **WHEN** an endpoint accepts a request but does not complete before the deadline
- **THEN** the invocation terminates with a deadline failure and, by default, no outputs
- **AND** supplied history remains unchanged

#### Scenario: Cancel an active request

- **WHEN** workflow cancellation occurs while the node awaits the provider
- **THEN** local waiting ends without publishing a later reply
- **AND** no updated history is published

### Requirement: Preserve optional generation metadata

Usage SHALL expose available normalized input, output, and total token counts as signed integers. Missing counters MUST NOT become invented zeros or inferred totals, and counts outside the signed range MUST fail. Unknown finish reasons SHALL preserve their provider value. Absent metadata SHALL omit its output port rather than supplying null.

#### Scenario: Receive sparse usage

- **WHEN** a provider reports only an output-token count
- **THEN** usage contains that count without inventing input or total counts

#### Scenario: Receive no metadata

- **WHEN** a provider reports neither supported usage counters nor a finish reason
- **THEN** the successful result omits both metadata ports

#### Scenario: Preserve an unfamiliar finish reason

- **WHEN** the endpoint reports a terminal reason not known to the node
- **THEN** the finish-reason output preserves that value rather than guessing success or truncation

## ADDED Requirements

### Requirement: Emit explicit per-attempt usage records

Each completed LLM task SHALL expose a usages list containing one uniquely identified attempt with its
resolved public model, available independent counters, and executing node plus loop path when available.
Direct calls outside a workflow MAY omit node identity. Optional absent ports SHALL be marked skipped so
workflow output selection can omit them. Message metadata SHALL remain application data, and scalar usage
SHALL remain available for existing single-call consumers.

#### Scenario: Execute one node twice

- **WHEN** the same node runs in two loop iterations
- **THEN** each usage record has a different call ID and the correct loop index

#### Scenario: Preserve annotations without sending them

- **WHEN** input history carries message metadata
- **THEN** output history retains it while the provider receives only role and content

### Requirement: Support explicit failure results for workflow aggregation

With capture_errors enabled, request failures, node deadlines, and response validation failures SHALL
return failed status, an error summary, and a usage entry without appending failed conversation messages.
Any valid already reported counters SHALL survive conversion failure. Without capture_errors, errors
SHALL retain their existing typed sources. Setup failures and workflow cancellation may abort before a
final result, and MUST NOT be described as known zero remote usage.

#### Scenario: Aggregate a failed provider attempt

- **WHEN** a provider returns an error and capture_errors is enabled
- **THEN** the workflow receives one failed usage entry with unknown unreported counters and no retry occurs

#### Scenario: Reject unusable text after receiving usage

- **WHEN** a provider response reports counters but its text cannot be accepted
- **THEN** the failed result retains those counters for workflow accounting
