# llm-node-execution Specification

## Purpose

Provide explicitly selected model calls with a caller-owned message list and function tools, publishing only the current assistant response while preserving ordinary task preparation, dependency validation, streaming, and failure semantics.

## Requirements

### Requirement: Select the LLM package explicitly

Workflows SHALL select the package registering `builtin.llm` through ordinary node dependencies. The node SHALL execute as a task with fixed reflected ports and MUST NOT require streaming workflow execution or node-specific compiler dispatch.

#### Scenario: Compile a terminal generation workflow

- **WHEN** a oneshot workflow declares the LLM package and connects a complete message list to its messages input
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

### Requirement: Send one model request from explicit messages

Each invocation SHALL accept a required nonempty `messages` list and optional `tools`, then send one Chat Completions request. The node SHALL NOT expose prompt, system, or history input ports, insert synthetic messages, or reorder caller messages. Connected stream outputs SHALL select streaming; unconnected outputs SHALL permit unary execution with zero emissions. The node MUST NOT execute tools, retain conversation state, follow redirects, or retry requests.

#### Scenario: Use an explicit compatible endpoint

- **WHEN** an invocation supplies messages with a custom base URL
- **THEN** one request reaches that prefix's `/chat/completions` route with the configured model, credentials, and supplied messages
- **AND** no Responses API request is made

#### Scenario: Continue a conversation

- **WHEN** a caller appends a prior assistant response and its next user message or tool results to its conversation
- **THEN** the next invocation encodes the supplied list without adding another prompt or system message

#### Scenario: Reject an empty request

- **WHEN** messages is absent, null, or empty
- **THEN** validation fails before network I/O

#### Scenario: Reject removed conversation assembly inputs

- **WHEN** an invocation supplies prompt, system, or history as node input ports
- **THEN** input decoding rejects those unknown ports even when messages is also supplied

#### Scenario: Receive a retryable or redirect response

- **WHEN** the endpoint returns a rate limit, server failure, or redirect
- **THEN** the invocation fails without resending the request or contacting the redirect target

#### Scenario: Disconnect a streaming request

- **WHEN** an active streaming request fails before valid completion
- **THEN** the node does not retry as a nonstreaming request or report its delivered prefix as a successful answer

### Requirement: Validate the explicit message list

Messages SHALL contain string role and content fields and optional application metadata. Accepted roles SHALL be system, user, assistant, and tool. System and user content SHALL be nonempty; assistant messages MAY carry tool_calls with empty text. Tool results SHALL identify a pending call through tool_call_id. Application metadata MUST NOT be sent to the provider. Invalid messages MUST fail before network I/O with an indexed diagnostic. Supplied order and content MUST NOT be silently changed.

#### Scenario: Preserve system message placement

- **WHEN** a valid request contains multiple system messages among completed conversation exchanges
- **THEN** their supplied positions and content are preserved without extracting a separate preamble

#### Scenario: Reject malformed messages

- **WHEN** a message contains a missing field, unknown field, nonobject metadata, nonstring content, or an unsupported role
- **THEN** input validation fails with the offending messages path and no provider request

#### Scenario: Preserve consecutive roles

- **WHEN** valid messages contain consecutive entries with the same supported role
- **THEN** the request preserves their order and content without enforcing alternation, trimming, summarization, or deduplication

### Requirement: Return only the current assistant message

A response containing text or completed tool calls SHALL produce one assistant `message`. Responses without either SHALL skip message instead of creating an empty assistant entry. The node SHALL NOT return an updated history or echo input messages as part of the response. Captured request failures SHALL omit and explicitly skip message while retaining failure status and available usage. Conversation assembly, append policy, persistence, and trimming SHALL belong to the Flow or caller.

#### Scenario: Return one response

- **WHEN** generation succeeds after receiving an existing conversation
- **THEN** message contains only the current assistant response, and the caller's message list is unchanged

#### Scenario: Compose a loop explicitly

- **WHEN** a Loop invokes the model repeatedly
- **THEN** its Flow constructs each request and appends each returned assistant message through ordinary nodes and loop assignments

#### Scenario: Preserve loop state on failure

- **WHEN** a captured model failure skips the message output
- **THEN** a required message binding skips the Flow's append node and history assignment, leaving the prior loop state unchanged

#### Scenario: Keep conversations independent

- **WHEN** the same prepared node handles separate message lists
- **THEN** each request depends only on its supplied messages and tools

### Requirement: Publish the terminal response atomically

A valid invocation SHALL publish one terminal result containing any available assistant message, text and tool-call projections, outcome, succeeded status, one-call usages, and optional usage and finish_reason. Text blocks SHALL retain response order. Conversion and validation MUST precede publication. Unsupported content, unsolicited tool calls, and invalid counts SHALL fail by default. Captured failures SHALL not publish an assistant message. Earlier stream emissions SHALL remain provisional.

#### Scenario: Receive a complete text answer

- **WHEN** a provider returns multiple text blocks and a terminal finish reason
- **THEN** the node publishes their ordered concatenation in one assistant message and its text projection
- **AND** earlier live increments do not create additional terminal messages

#### Scenario: Preserve a large reply

- **WHEN** valid generated text exceeds the former client-side output byte limit
- **THEN** the node preserves the complete response without a byte-limit failure
- **AND** provider token limits, timeout, cancellation, and output validation remain in effect

#### Scenario: Complete at the token limit

- **WHEN** a reply reaches a length finish reason
- **THEN** its outcome is truncated rather than completed or actionable tool_calls

### Requirement: Support caller-managed function tools

Tools SHALL declare unique valid names, descriptions, and object input schemas. Calls SHALL carry id, name, and parsed JSON arguments. Call IDs SHALL be nonblank and unique within the supplied transcript. Every pending call SHALL receive exactly one result before another system, user, or assistant message or the next request. The node SHALL NOT expose tool_choice, perform tool execution, or retain provider-specific reasoning state.

#### Scenario: Continue from tool results

- **WHEN** messages contains an assistant tool-call message followed by matching tool results
- **THEN** the adapter encodes their IDs and arguments without adding a user prompt

#### Scenario: Receive a tool-only response

- **WHEN** advertised tools produce a valid response containing calls without text
- **THEN** message contains the assistant calls, text is empty, and outcome is tool_calls

#### Scenario: Observe completed tool calls

- **WHEN** the tool_call stream port is consumed
- **THEN** each completed call emits its model_call_id and call without a synthetic text delta
- **AND** the other stream port is explicitly skipped for that emission
- **AND** terminal outcome remains authoritative for the whole request

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
- **AND** supplied messages remain unchanged

#### Scenario: Cancel an active request

- **WHEN** workflow cancellation occurs while the node awaits the provider
- **THEN** local waiting ends without publishing a later reply
- **AND** no assistant message is published

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

The resolved request deadline SHALL cover setup, all streaming reads, and final conversion. Cancellation, malformed/truncated frames, and unsupported content SHALL stop publication. Existing captured-provider-failure policy SHALL retain reported call facts without making partial text successful. Delivery failure and cancellation MUST remain execution failures. Emission waits SHALL follow native backpressure and terminate through consumer failure or host cancellation without a publication deadline.

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

### Requirement: Keep application performance outside model execution

Raw node usage contracts and observations SHALL contain provider call facts. Derived timing and token-rate types and calculations SHALL belong to the application. The model node MUST NOT depend on application performance models.

#### Scenario: Leave performance measurements to the application

- **WHEN** a model request completes
- **THEN** node outputs and request observations expose provider call facts without application timing or rate fields
- **AND** the application derives performance from source events and owns its enriched call model
