## MODIFIED Requirements

### Requirement: Send one text generation request

Each invocation SHALL accept nonempty string `prompt`, optional string `system`, and optional message-list `history`, then send one Chat Completions request ordered as system, history, prompt. Connected output SHALL stream; unconnected output SHALL permit unary execution with zero emissions. The node MUST NOT execute tools, retain conversation state, follow redirects, or retry requests.

#### Scenario: Use an explicit compatible endpoint

- **WHEN** an invocation supplies prompt and system inputs with a custom base URL
- **THEN** one request reaches that prefix's `/chat/completions` route with the configured model, credentials, and text messages
- **AND** no Responses API request is made

#### Scenario: Continue a conversation

- **WHEN** an invocation supplies completed user and assistant messages as history and a new prompt
- **THEN** the request preserves those messages in order before appending the current prompt once
- **AND** supplied system instructions appear first without being duplicated in history

#### Scenario: Reject an empty current prompt

- **WHEN** an invocation supplies an empty prompt
- **THEN** validation fails before network I/O instead of returning history that the next invocation cannot consume

#### Scenario: Receive a retryable or redirect response

- **WHEN** the endpoint returns a rate limit, server failure, or redirect
- **THEN** the invocation fails without resending the request or contacting the redirect target

#### Scenario: Disconnect a streaming request

- **WHEN** an active streaming request fails before valid completion
- **THEN** the node does not retry as a nonstreaming request or report its delivered prefix as a successful answer

### Requirement: Publish a terminal text result atomically

Success SHALL publish one terminal result with complete string `text`, updated message-list `history`, succeeded status, one-call `usages`, and optional `usage` and `finish_reason`. Text blocks SHALL retain response order. Conversion and validation MUST precede publication. Empty or excessive text, unsupported content, and invalid counts SHALL fail without terminal outputs or transcript mutation by default. Earlier increments SHALL remain provisional.

#### Scenario: Receive a complete text answer

- **WHEN** a provider returns multiple text blocks and a terminal finish reason
- **THEN** the node publishes their ordered concatenation, updated history, and finish reason in one Task result
- **AND** any earlier live increments do not create additional terminal results or history messages

#### Scenario: Reject unsupported output

- **WHEN** the reply has no nonempty text or contains a tool call or unsupported content
- **THEN** with capture_errors disabled the invocation fails without publishing terminal text, updated history, or metadata
- **AND** supplied history remains unchanged

#### Scenario: Exceed the text limit

- **WHEN** decoded generated text exceeds the configured UTF-8 byte limit
- **THEN** the node rejects the result before terminal publication and stops further live publication
- **AND** streaming checks the bound incrementally while unary buffering retains its documented transport-memory limitation

#### Scenario: Apply the reply limit independently of history

- **WHEN** the generated reply fits the output text limit but the full transcript is larger
- **THEN** that text limit does not trim or reject the transcript solely because of its aggregate size

#### Scenario: Complete at the token limit

- **WHEN** a reply contains nonempty text with a `length` finish reason
- **THEN** the result includes that text and reason without claiming an untruncated answer

## ADDED Requirements

### Requirement: Publish typed live text independently of final metadata

Connected live text SHALL carry ordered text increments and the actual model-call identity through a declared stream port. Complete terminal text/history and usages SHALL remain directly available without collecting that stream. Only explicitly exported nodes SHALL become public chat streams; title and other internal model calls MUST NOT be selected by topology inference.

#### Scenario: Stream an answer and generate a title

- **WHEN** a workflow exports only its answer node and subsequently invokes a title model
- **THEN** the public answer stream contains only answer increments while final usages can report both calls

#### Scenario: Preserve sparse terminal usage

- **WHEN** a streaming endpoint reports partial or no usage counters
- **THEN** the final call record preserves reported counts and absent counts remain unknown

### Requirement: Preserve streaming bounds and captured failures

The resolved request deadline SHALL cover setup, all streaming reads, and final conversion. Cancellation, malformed/truncated frames, unsupported content, and incremental text limits SHALL stop publication. Existing captured-provider-failure policy SHALL retain reported call facts without making partial text successful. Delivery failure and cancellation MUST remain execution failures.

#### Scenario: Handle a provider error after text

- **WHEN** capture_errors is enabled and the provider reports a failure after live text
- **THEN** the node can return its failed terminal result with reported usages while the live prefix remains provisional

#### Scenario: Wait for a stalled stream

- **WHEN** an endpoint sends an initial increment and then stalls beyond the total deadline
- **THEN** the invocation terminates without a successful terminal answer and blocked consumers receive closure/failure
