# Chat Workflow Execution

## Purpose

Execute persistent conversations and independent session title generation through embedded Rust workflow modules with explicit host ownership of invocation lifecycle.

## ADDED Requirements

### Requirement: Embed the application workflows

The package SHALL embed conversation, session title, and usage statistics definitions as Rust modules using the existing workflow embedding facility. Built hosts SHALL execute them without workflow source files, generated source files, build tools, or spawning standalone workflow runners.

#### Scenario: Run without definition files

- **WHEN** a built host runs with workflow and generated source files removed
- **THEN** all three embedded workflows remain executable with their configured runtime resources

#### Scenario: Rebuild a workflow change

- **WHEN** an embedded definition changes and the package is rebuilt without cleaning
- **THEN** the resulting host executes the updated definition

### Requirement: Prepare the configured flow before beginning work

The host SHALL apply the selected nonsecret model configuration during flow preparation and validate node contracts before creating running work. Conversation and title configurations SHALL be independently selectable. Preparation MUST NOT read credentials, open the database, or contact providers. Each call SHALL retain its actual configuration identity.

#### Scenario: Select a different title model

- **WHEN** title generation uses a model configuration different from conversation generation
- **THEN** each workflow uses its selected model and records that identity with its own call

#### Scenario: Reject invalid preparation

- **WHEN** model configuration or a workflow node contract is invalid
- **THEN** preparation fails without creating a running turn or issuing a provider request

### Requirement: Execute conversations through the embedded flow

The conversation flow SHALL begin a durable turn, supply ordered history and a separate prompt to the existing LLM implementation through an application adapter, and persist its terminal response through storage nodes. System instructions SHALL remain separate from history. Returned results SHALL identify their session, turn, and call.

#### Scenario: Complete two conversation turns

- **WHEN** a session completes two turns through the embedded conversation flow
- **THEN** the second request includes the first completed user and assistant messages before its own prompt once
- **AND** each completed turn has one saved user message and one saved assistant message

#### Scenario: Exceed the configured context bound

- **WHEN** loaded history exceeds the host's configured finite history byte bound
- **THEN** the request fails before provider execution without silently deleting or trimming stored messages

#### Scenario: Omit optional model fields

- **WHEN** system instructions or response usage and finish metadata are absent
- **THEN** required object request and response values still route through the conversation flow
- **AND** response persistence executes without introducing placeholder metadata

### Requirement: Keep active sessions independent

Each invocation SHALL bind its session and call identities before execution. Changing a UI selection MUST NOT redirect its completion. The host SHALL bound total active workflows, while allowing different sessions to progress independently within that bound. A session-busy result MUST NOT implicitly enqueue or resend a prompt.

#### Scenario: Switch sessions while a reply is pending

- **WHEN** a caller switches from session A to session B while A is running
- **THEN** A's result is persisted to A and B's history is unchanged

#### Scenario: Reach the host concurrency limit

- **WHEN** active workflow execution reaches its configured total limit
- **THEN** additional execution waits or receives an explicit capacity result without starting an unbounded worker

### Requirement: Finalize execution failures outside the success path

The host SHALL finalize begun work after provider failure, deadline expiry, or cancellation through application nodes even when success-path nodes do not execute. Cleanup SHALL use a fresh execution context independent of the cancelled request. Cleanup failures MUST preserve the original error and identify unresolved work.

#### Scenario: Cancel a pending conversation

- **WHEN** cancellation occurs while the LLM node waits for a response
- **THEN** the host finalizes the turn and call as cancelled without committing a reply
- **AND** subsequent submissions can proceed after cleanup succeeds

#### Scenario: Fail cleanup

- **WHEN** a provider failure is followed by a database failure during finalization
- **THEN** the host returns both failure contexts and the affected identities rather than reporting successful cleanup

### Requirement: Retry persistence without another model request

If response persistence fails after the LLM node has produced a valid result, the host SHALL expose an uncommitted completion with its identities and retainable response data. Retrying that completion SHALL invoke only persistence. It MUST NOT issue another model request or report the turn committed before storage succeeds.

#### Scenario: Recover from a transient completion failure

- **WHEN** a completion write fails and its retained response is submitted again after storage recovers
- **THEN** the original response and call facts are persisted once without contacting the model again

### Requirement: Schedule the first automatic title independently

The first successful turn of a provisionally titled session SHALL durably schedule one automatic title request. Title execution SHALL occur independently after conversation commit. The host SHALL resume pending title work after restart, but MUST NOT automatically resend an interrupted provider call.

#### Scenario: Finish the first turn

- **WHEN** the first turn of a provisionally titled session commits
- **THEN** its response is available without waiting for title generation
- **AND** one pending automatic title request exists

#### Scenario: Restart before title execution

- **WHEN** the host restarts after the first turn commits but before its title request starts
- **THEN** the pending title request remains eligible for execution

#### Scenario: Fail title generation

- **WHEN** the title provider fails or its call is cancelled
- **THEN** the conversation remains committed and its provisional title remains available
- **AND** later conversation turns do not automatically repeat the failed title request

### Requirement: Preserve manual titles and account for discarded results

Title generation SHALL use a completed conversation snapshot and captured title revision. The result SHALL apply only while the title remains provisional at that revision. Manual renames MUST take precedence. A valid generated result SHALL retain its call facts even when the title update is skipped, and MUST NOT append chat messages.

#### Scenario: Rename during generation

- **WHEN** a user manually renames a session while its title model call is running
- **THEN** the generated title cannot overwrite the manual title
- **AND** its reported usage remains associated with the title call

#### Scenario: Generate a title successfully

- **WHEN** a valid title is produced for a matching provisional title revision
- **THEN** the title and its call facts are persisted without changing conversation messages

### Requirement: Join active work before host teardown

Shutdown SHALL stop accepting requests, cancel or complete active workflows, finalize recoverable state, and join host-owned execution workers before releasing database ownership. It MUST NOT leave detached local provider tasks or permit later results to write through a closed host.

#### Scenario: Close with conversation and title calls active

- **WHEN** the host closes while conversation and title workflows are running
- **THEN** their local execution is joined and database ownership is released only after finalization is attempted
- **AND** unresolved work is recoverable as interrupted on the next open
