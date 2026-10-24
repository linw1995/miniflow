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

The node SHALL require a model field and validate literal settings during preparation, rejecting unknown fields, literal credentials, unsupported options, and invalid types or ranges. Supported fields MAY use templates: preparation SHALL validate syntax and declare references; invocation SHALL validate resolved values before network I/O. Contracts MUST remain independent of the environment and resolved values.

#### Scenario: Use default connection settings

- **WHEN** a configuration supplies only a valid model
- **THEN** it selects the OpenAI API prefix, `OPENAI_API_KEY`, a 60000 millisecond deadline

#### Scenario: Reject invalid configuration

- **WHEN** literal configuration contains an empty model, unsupported URL, URL credentials, a nonfinite or out-of-range temperature, a nonpositive limit, an unrepresentable numeric value, or an unknown field
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

Success SHALL publish one terminal result with complete string `text`, updated message-list `history`, succeeded status, one-call `usages`, and optional `usage` and `finish_reason`. Text blocks SHALL retain response order. Conversion and validation MUST precede publication. Empty text, unsupported content, and invalid counts SHALL fail without terminal outputs or transcript mutation by default. Earlier increments SHALL remain provisional.

#### Scenario: Receive a complete text answer

- **WHEN** a provider returns multiple text blocks and a terminal finish reason
- **THEN** the node publishes their ordered concatenation, updated history, and finish reason in one Task result
- **AND** any earlier live increments do not create additional terminal results or history messages

#### Scenario: Reject unsupported output

- **WHEN** the reply has no nonempty text or contains a tool call or unsupported content
- **THEN** with capture_errors disabled the invocation fails without publishing terminal text, updated history, or metadata
- **AND** supplied history remains unchanged

#### Scenario: Preserve a large reply

- **WHEN** valid generated text exceeds the former client-side output byte limit
- **THEN** the node preserves the complete text, any live increments, and updated history without a byte-limit failure
- **AND** provider token limits, timeout, cancellation, and output validation remain in effect

#### Scenario: Complete at the token limit

- **WHEN** a reply contains nonempty text with a `length` finish reason
- **THEN** the result includes that text and reason without claiming an untruncated answer

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

### Requirement: Keep credentials out of artifacts and error display

Workflow definitions SHALL store only a credential reference. Credential values MUST NOT appear in generated manifests, snapshots, node diagnostic display, or request debug logging. Provider failures SHALL retain typed sources while the node displays safe operation context instead of arbitrary remote bodies or authentication headers.

#### Scenario: Report an authentication failure

- **WHEN** a provider rejects credentials and returns a body containing sensitive request data
- **THEN** the node reports a contextual failure without displaying that body or the credential value
- **AND** the original typed provider error remains available in the source chain

### Requirement: Configure LLM invocations from runtime templates

LLM model, endpoint, generation parameters, deadline, and reply limit SHALL accept
shared configuration templates. An api_key template SHALL supply an execution-time
credential and MUST NOT coexist with api_key_env. Literal credentials SHALL be
rejected. Existing literal api_key_env configuration SHALL remain supported, with
OPENAI_API_KEY as its default when no credential template is configured.

#### Scenario: Configure the provider from typed parameters

- **WHEN** an LLM node references parameter outputs with an explicit predecessor dependency
- **THEN** execution uses their resolved values for the provider request and secret parameter snapshots remain redacted

#### Scenario: Reuse a prepared node with new settings

- **WHEN** a subsequent invocation supplies a different credential, endpoint, model, or limit
- **THEN** the reused node uses the new settings without retaining invocation-specific settings in its shared client

#### Scenario: Reject invalid runtime settings

- **WHEN** a referenced configuration value has the wrong type or violates a field constraint
- **THEN** execution fails before contacting the provider and retains safe typed error sources

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

### Requirement: Publish typed live text independently of final metadata

Connected live text SHALL carry ordered text increments and the actual model-call identity through a declared stream port. Complete terminal text/history and usages SHALL remain directly available without collecting that stream. Only explicitly exported nodes SHALL become public chat streams; title and other internal model calls MUST NOT be selected by topology inference.

#### Scenario: Stream an answer and generate a title

- **WHEN** a workflow exports only its answer node and subsequently invokes a title model
- **THEN** the public answer stream contains only answer increments while final usages can report both calls

#### Scenario: Preserve sparse terminal usage

- **WHEN** a streaming endpoint reports partial or no usage counters
- **THEN** the final call record preserves reported counts and absent counts remain unknown

### Requirement: Preserve streaming deadlines and captured failures

The resolved request deadline SHALL cover setup, all streaming reads, and final conversion. Cancellation, malformed/truncated frames, and unsupported content SHALL stop publication. Existing captured-provider-failure policy SHALL retain reported call facts without making partial text successful. Delivery failure and cancellation MUST remain execution failures.

#### Scenario: Handle a provider error after text

- **WHEN** capture_errors is enabled and the provider reports a failure after live text
- **THEN** the node can return its failed terminal result with reported usages while the live prefix remains provisional

#### Scenario: Wait for a stalled stream

- **WHEN** an endpoint sends an initial increment and then stalls beyond the total deadline
- **THEN** the invocation terminates without a successful terminal answer and blocked consumers receive closure/failure

### Requirement: Emit source-timestamped model request observations

Each attempted provider operation SHALL emit request-started and request-finished observations with a call identity. Streaming calls SHALL emit one first-text observation before publishing the first nonempty text. The observation layer SHALL supply timestamps. LLM execution MUST NOT calculate request duration or first-text latency using a node-local timer.

#### Scenario: Receive a streaming answer

- **WHEN** a provider sends empty or metadata-only events before nonempty text
- **THEN** the first-text event identifies the first nonempty decoded text before publication and backpressure
- **AND** the request-finished event precedes terminal conversion or persistence

#### Scenario: Complete without a stream subscription

- **WHEN** the node executes its unary request path
- **THEN** it emits request boundary observations without a first-text event

#### Scenario: Capture a failed provider attempt

- **WHEN** a provider operation fails and the node returns a captured failure
- **THEN** request observations retain the call identity and available facts without claiming successful throughput
