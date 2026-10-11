# llm-node-execution Specification

## Purpose

Provide explicitly selected builtin text generation with caller-supplied conversation history that publishes one complete terminal response and an updated transcript while preserving ordinary task preparation, dependency validation, and failure semantics.

## Requirements

### Requirement: Select the LLM package explicitly

Workflows SHALL select the package registering `builtin.llm` through ordinary node dependencies. The node SHALL execute as a task with fixed reflected ports and MUST NOT require streaming workflow execution or node-specific compiler dispatch.

#### Scenario: Compile a terminal generation workflow

- **WHEN** a oneshot workflow declares the LLM package and connects a string to its prompt input
- **THEN** the compiler prepares the node through ordinary registry and typed-port validation

#### Scenario: Reference an undeclared LLM node

- **WHEN** a workflow references `builtin.llm` without selecting its package
- **THEN** preparation reports the unknown kind before workflow execution

### Requirement: Validate static generation configuration

The node SHALL require a nonblank model and validate endpoint, credential-variable name, generation parameters, deadline, and text limit during preparation. It MUST reject unknown fields, direct credentials, invalid numeric ranges, and a streaming option. Configuration SHALL determine contracts independently of the process environment.

#### Scenario: Use default connection settings

- **WHEN** a configuration supplies only a valid model
- **THEN** it selects the OpenAI API prefix, `OPENAI_API_KEY`, a 60000 millisecond deadline, and a 1048576 byte output text limit

#### Scenario: Reject invalid configuration

- **WHEN** configuration contains an empty model, unsupported URL, URL credentials, a nonfinite or out-of-range temperature, a nonpositive limit, an unrepresentable numeric value, or an unknown field
- **THEN** preparation fails with configuration context without acquiring external resources

### Requirement: Prepare without external execution resources

Node preparation and workflow build validation MUST NOT read credentials, initialize network clients or asynchronous executors, or issue provider requests. Manifest inspection SHALL remain independent of execution resources. Missing or empty credentials SHALL fail at invocation before a request is sent.

#### Scenario: Build without credentials

- **WHEN** a valid workflow is compiled with its configured credential variable unset
- **THEN** compilation succeeds without contacting the model endpoint

#### Scenario: Execute without credentials

- **WHEN** the node executes with a missing or empty configured credential variable
- **THEN** execution fails with node attribution and publishes no result

### Requirement: Send one text generation request

Each invocation SHALL accept required nonempty string `prompt`, optional string `system`, and optional message-list `history`, and send one nonstreaming Chat Completions request. Request order SHALL be system instructions, supplied history, and the current prompt. The node MUST NOT execute tools, retain node-owned conversation state, follow redirects, or automatically retry requests.

#### Scenario: Use an explicit compatible endpoint

- **WHEN** an invocation supplies prompt and system inputs with a custom base URL
- **THEN** one request reaches that prefix's `/chat/completions` route with the configured model, credentials, and text messages
- **AND** no Responses API request or streaming response is requested

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

### Requirement: Validate explicit text conversation history

History SHALL be an ordered list of objects containing only required string `role` and nonempty string `content`. Accepted roles SHALL be `user` and `assistant`; system instructions SHALL use the separate input. Omitted or empty history SHALL start a fresh conversation. Invalid history MUST fail before network I/O with an indexed diagnostic. Supplied order and content MUST NOT be silently changed.

#### Scenario: Start without history

- **WHEN** an invocation omits history or supplies an empty list
- **THEN** it sends only optional system instructions and the current user prompt as conversation messages

#### Scenario: Reject malformed history

- **WHEN** history is null or contains a missing field, unknown field, nonstring content, empty content, or an unsupported role
- **THEN** input validation fails with the offending history path and no provider request

#### Scenario: Preserve caller supplied history

- **WHEN** valid history contains consecutive messages with the same supported role
- **THEN** the request preserves their order and content without enforcing alternation, trimming, summarization, or deduplication

### Requirement: Return history for the next turn

Each successful invocation SHALL return required message-list `history` containing the supplied history followed by the current user prompt and assistant text once each. System instructions SHALL be excluded. The node MUST NOT mutate input history or reuse an earlier invocation's transcript implicitly. Workflows SHALL be able to pass the returned history to a subsequent invocation.

#### Scenario: Return the first completed turn

- **WHEN** generation succeeds without supplied history
- **THEN** output history contains one user message with the prompt followed by one assistant message with the returned text

#### Scenario: Feed completed history into another turn

- **WHEN** a second invocation receives the first invocation's output history and a new prompt
- **THEN** the next output preserves that history and appends exactly the second user message and reply

#### Scenario: Keep conversations independent

- **WHEN** the same prepared node handles separate transcripts or a later invocation omits history
- **THEN** each result depends only on its supplied conversation and system inputs
- **AND** no transcript from another invocation is added

### Requirement: Publish a terminal text result atomically

A successful invocation SHALL publish exactly one result containing complete string `text`, updated message-list `history`, and optional `usage` and `finish_reason`. Text blocks SHALL retain response order. Conversion and validation MUST finish before publication. Empty text, unsupported output content, invalid usage counts, and excessive output text SHALL fail without any outputs or transcript mutation.

#### Scenario: Receive a complete text answer

- **WHEN** a provider returns multiple text blocks and a terminal finish reason
- **THEN** the node publishes their ordered concatenation, updated history, and finish reason in one task result
- **AND** no intermediate chunks or incremental outputs are published

#### Scenario: Reject unsupported output

- **WHEN** the reply has no nonempty text or contains a tool call or unsupported content
- **THEN** the invocation fails without publishing text, updated history, or metadata
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

### Requirement: Preserve optional generation metadata

Usage SHALL expose available normalized input, output, and total token counts as signed integers. Missing counters MUST NOT become invented zeros, and counts outside the signed range MUST fail. Unknown finish reasons SHALL preserve their provider value. Absent metadata SHALL omit its output port rather than supplying null.

#### Scenario: Receive sparse usage

- **WHEN** a provider reports only an output-token count
- **THEN** usage contains that count without inventing input or total counts

#### Scenario: Receive no metadata

- **WHEN** a provider reports neither supported usage counters nor a finish reason
- **THEN** the successful result omits both metadata ports

#### Scenario: Preserve an unfamiliar finish reason

- **WHEN** the endpoint reports a terminal reason not known to the node
- **THEN** the finish-reason output preserves that value rather than guessing success or truncation

### Requirement: Bound request waiting and honor cancellation

The node SHALL apply a positive invocation deadline and honor workflow cancellation while awaiting a reply. Deadline expiry or cancellation SHALL drop the owned pending operation and publish no result. A deadline elapsed during synchronous initialization MUST prevent subsequent request polling. Local cancellation MUST NOT be described as guaranteed remote termination.

#### Scenario: Wait on a stalled endpoint

- **WHEN** an endpoint accepts a request but does not complete before the deadline
- **THEN** the invocation terminates with a deadline failure and no outputs
- **AND** supplied history remains unchanged

#### Scenario: Cancel an active request

- **WHEN** workflow cancellation occurs while the node awaits the provider
- **THEN** local waiting ends without publishing a later reply
- **AND** no updated history is published

### Requirement: Keep credentials out of artifacts and error display

Workflow definitions SHALL store only a credential reference. Credential values MUST NOT appear in generated manifests, snapshots, node diagnostic display, or request debug logging. Provider failures SHALL retain typed sources while the node displays safe operation context instead of arbitrary remote bodies or authentication headers.

#### Scenario: Report an authentication failure

- **WHEN** a provider rejects credentials and returns a body containing sensitive request data
- **THEN** the node reports a contextual failure without displaying that body or the credential value
- **AND** the original typed provider error remains available in the source chain
