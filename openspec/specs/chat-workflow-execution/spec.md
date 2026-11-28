# chat-workflow-execution Specification

## Purpose

Execute persistent conversations through an application workflow that hosts user workflows over a shared input/output protocol, including optional session titles.

## Requirements

### Requirement: Embed the application workflows

The package SHALL embed an application conversation definition as a Rust module using the existing
workflow embedding facility. Its graph SHALL express durable turn admission and history/context loading,
invocation of a supplied backend, and result settlement through application-owned nodes. Built embedded
backends SHALL execute without workflow source files, generated source files, build tools, or spawning
standalone workflow runners. Executable backends SHALL invoke the supplied runner within the same
application lifecycle graph.

#### Scenario: Run without definition files

- **WHEN** a built host runs with workflow and generated source files removed
- **THEN** the composed workflow remains executable with their configured runtime resources

#### Scenario: Rebuild a workflow change

- **WHEN** an embedded definition changes and the package is rebuilt without cleaning
- **THEN** the resulting host executes the updated definition

### Requirement: Prepare the configured flow before beginning work

The host SHALL validate the selected user workflow and resolve its configuration parameters before
creating running work. It MUST NOT identify, configure, or depend on internal model nodes. Preparation
MUST NOT open the database or contact providers. Each reported call SHALL retain its actual model
identity and MAY report a workflow-selected configuration profile label.

#### Scenario: Select a different title model

- **WHEN** the supplied user workflow selects different models for title and conversation generation
- **THEN** each model node uses its selected configuration and reports its identity and attribution through the shared usages output

#### Scenario: Reject invalid preparation

- **WHEN** model configuration or a workflow node contract is invalid
- **THEN** preparation fails without creating a running turn or issuing a provider request

### Requirement: Execute conversations through the embedded flow

Embedded and executable backends SHALL execute the application Flow. Its `begin`, `workflow`, and
`finish` stages SHALL use `workflow.call` with per-invocation host bindings for admission, backend
execution, and settlement. The backend call SHALL validate and forward the conversation protocol
while retaining nested observation for embedded callable Flows. The application Flow SHALL use
existing `mf` node kinds and the package SHALL NOT register custom chat node kinds. The user workflow
SHALL define conversation, optional title generation, and usage collection without requiring
`mf-chat` nodes or database access. System instructions SHALL remain separate from history. Returned
completions SHALL identify their session, turn, and call.

#### Scenario: Complete two conversation turns

- **WHEN** a session completes two turns through the embedded conversation flow
- **THEN** the second request includes the first completed user and assistant messages before its own prompt once
- **AND** each completed turn has one saved user message and one saved assistant message

#### Scenario: Load complete successful history

- **WHEN** a session has accumulated successful history
- **THEN** the request includes all successful messages without deleting, trimming, or enforcing a local byte bound
- **AND** model context failures surface from provider execution

#### Scenario: Omit optional model fields

- **WHEN** system instructions or response usage and finish metadata are absent
- **THEN** the shared contract still routes the primary response and usage list
- **AND** response persistence executes without introducing placeholder metadata

### Requirement: Keep active sessions independent

Each invocation SHALL bind its session and call identities before execution. Changing a UI selection MUST NOT redirect its completion. The host SHALL bound total active workflows, while allowing different sessions to progress independently within that bound. A session-busy result MUST NOT implicitly enqueue or resend a prompt.

#### Scenario: Switch sessions while a reply is pending

- **WHEN** a caller switches from session A to session B while A is running
- **THEN** A's result is persisted to A and B's history is unchanged

#### Scenario: Reach the host concurrency limit

- **WHEN** active workflow execution reaches its configured total limit
- **THEN** additional execution waits or receives an explicit capacity result without starting an unbounded worker

### Requirement: Queue separate turns independently of steering

The host SHALL offer an explicit session FIFO for next-turn prompts with or without an invocation's
optional steering capability. It SHALL NOT expose that FIFO to the Flow or model. Claim and withdrawal
SHALL be mutually exclusive per entry. Waiting for a receipt, polling output, and rendering SHALL NOT
be required to dispatch a queued prompt in the embedded host.

#### Scenario: Advance after the matching successful commit

- **WHEN** an embedded or executable session with pending next-turn input commits a successful turn
- **THEN** the host may claim only the oldest remaining prompt and admit a new turn against the committed context
- **AND** neither a queued prompt nor its receipt reserves a turn identity, runs tools, or writes a user message before claim

#### Scenario: Preserve unadmitted prompts on interruption

- **WHEN** the preceding turn fails, is cancelled, retains an unsaved result, or the next prompt cannot be admitted
- **THEN** the owning client can recover unadmitted text for editing without another runner or model request
- **AND** save-only recovery and restart MUST NOT replay either queue

### Requirement: Finalize execution failures outside the success path

The host SHALL finalize begun work after provider failure, deadline expiry, or cancellation through shared storage handlers even when success-path nodes do not execute. Cleanup SHALL run independently of the cancelled request without creating a workflow or execution context. Cleanup failures MUST preserve the original error and identify unresolved work.

#### Scenario: Cancel a pending conversation

- **WHEN** cancellation occurs while the LLM node waits for a response
- **THEN** the host finalizes the turn and call as cancelled without committing a reply
- **AND** subsequent submissions can proceed after cleanup succeeds

#### Scenario: Fail cleanup

- **WHEN** a provider failure is followed by a database failure during finalization
- **THEN** the host returns both failure contexts and the affected identities rather than reporting successful cleanup

### Requirement: Retry persistence without another model request

If response persistence fails after the workflow has delivered a valid final result, the host SHALL expose an uncommitted completion with its identities and retainable response data. Retrying that completion SHALL invoke only persistence. It MUST NOT issue another model request or report the turn committed before storage succeeds.

#### Scenario: Recover from a transient completion failure

- **WHEN** a completion write fails and its retained response is submitted again after storage recovers
- **THEN** the original response and call facts are persisted once without contacting the model again

### Requirement: Compose optional titles in the conversation workflow

The host SHALL supply title source, revision, recorded-attempt state, and enabled policy as application
context. The workflow SHALL decide whether to produce an optional title. The supplied title example SHALL call
the title model only after a successful primary call when the session is provisional, titles are enabled,
and no title attempt has been recorded. The host MUST NOT schedule independent title jobs or inspect
intermediate nodes to decide recovery. One submission SHALL finish after the composed execution.

#### Scenario: Complete the first turn

- **WHEN** a provisional session completes its first successful composed execution
- **THEN** the result includes the answer, optional title, and explicitly combined usage records
- **AND** the host saves the completion in one transaction

#### Scenario: Disable automatic titles or reuse a named session

- **WHEN** the context disables titles, records a prior attempt, or identifies a non-provisional title
- **THEN** the title example skips the title model and returns only conversation usages

#### Scenario: Handle a title provider failure in the flow

- **WHEN** the title example receives a captured title provider failure
- **THEN** it returns the successful primary answer, no title, and both calls' usage records
- **AND** the host persists that result without introducing a separate title retry

#### Scenario: Cancel before a final result

- **WHEN** cancellation prevents delivery of the composed result
- **THEN** the host finalizes the invocation without recovering intermediate replies or guessing usage

#### Scenario: Upgrade pending legacy title jobs

- **WHEN** an existing database contains never-started pending title jobs
- **THEN** migration retires those jobs and startup does not dispatch them
- **AND** previously started title attempts remain available in future title context

### Requirement: Preserve manual titles and account for discarded results

Title generation SHALL use the primary answer and an admission-time title revision. The result SHALL apply only while the title remains provisional at that revision. Manual renames MUST take precedence. A valid generated result SHALL retain its call facts even when the title update is skipped, and MUST NOT append chat messages.

#### Scenario: Rename during generation

- **WHEN** a user manually renames a session while its title model call is running
- **THEN** the generated title cannot overwrite the manual title
- **AND** its reported usage remains associated with the title call

#### Scenario: Generate a title successfully

- **WHEN** a valid title is produced for a matching provisional title revision
- **THEN** the title is applied in the primary completion transaction without creating a separate title message

### Requirement: Join active work before host teardown

Shutdown SHALL stop accepting requests, cancel or complete active workflows, finalize recoverable state, and join host-owned execution workers before releasing database ownership. It MUST NOT leave detached local provider tasks or permit later results to write through a closed host.

#### Scenario: Close with conversation and title calls active

- **WHEN** the host closes while composed conversation workflows are running
- **THEN** their local execution is joined and database ownership is released only after finalization is attempted
- **AND** unresolved work is recoverable as interrupted on the next open

### Requirement: Export previews independently of settlement

The application-owned conversation Flow SHALL export provisional previews from the backend call's
incremental port. Hosts MUST NOT forward model deltas around the graph. Title, compaction, and
settlement SHALL depend on a validated terminal result from a normally completed backend call,
not on preview delivery or acknowledgment.

#### Scenario: Receive previews during a pending call

- **WHEN** a backend emits answer text before it returns its terminal result
- **THEN** the conversation Flow can deliver previews without settling the turn

#### Scenario: Fail after a provisional prefix

- **WHEN** the backend fails before producing a valid final result
- **THEN** no settlement inputs are published and no partial assistant message is saved

### Requirement: Drain native previews without waiting for subscribers

The host SHALL acknowledge native preview deliveries after handling or discarding them. Channel
subscriptions SHALL be bounded and MUST NOT block the Flow when full or disconnected. Terminal-only
submissions SHALL allocate no preview subscription. Explicit cancellation SHALL interrupt active work,
and host teardown SHALL join active instances before releasing storage.

#### Scenario: Leave previews unread or drop the subscription

- **WHEN** a preview subscriber stops reading or closes its receiver without cancelling the turn
- **THEN** a successfully completed backend result can settle without waiting for that subscriber

### Requirement: Preserve application and model identities in previews

Application previews SHALL identify their owning chat invocation and actual model call. Distinct model calls SHALL remain separate presentation segments. Runtime-specific stream lifetimes and node paths MUST NOT be required by the chat preview protocol.

#### Scenario: Repeat a model call within one turn

- **WHEN** answer events carry different actual model-call identities
- **THEN** the application preserves their distinction within the same chat invocation
