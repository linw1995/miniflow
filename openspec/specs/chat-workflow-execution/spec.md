# chat-workflow-execution Specification

## Purpose

Execute persistent conversations and independent session title generation through embedded Rust workflow modules with explicit host ownership of invocation lifecycle.

## Requirements

### Requirement: Embed the application workflows

The package SHALL embed conversation and session title definitions as Rust modules using the existing workflow embedding facility. Built hosts SHALL execute them without workflow source files, generated source files, build tools, or spawning standalone workflow runners.

#### Scenario: Run without definition files

- **WHEN** a built host runs with workflow and generated source files removed
- **THEN** both embedded workflows remain executable with their configured runtime resources

#### Scenario: Rebuild a workflow change

- **WHEN** an embedded definition changes and the package is rebuilt without cleaning
- **THEN** the resulting host executes the updated definition

### Requirement: Prepare the configured flow before beginning work

The host SHALL apply the selected nonsecret model configuration during flow preparation and validate node contracts before creating running work. Conversation and title configurations SHALL be independently selectable. Preparation MUST NOT read credentials, open the database, or contact providers. Each call SHALL retain its selected profile and actual model attribution.

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

The host SHALL finalize begun work after provider failure, deadline expiry, or cancellation through shared storage handlers even when success-path nodes do not execute. Cleanup SHALL run independently of the cancelled request without creating a workflow or execution context. Cleanup failures MUST preserve the original error and identify unresolved work.

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

With automatic titles enabled, the host SHALL explicitly schedule titles after conversation persistence and during startup or polling. Candidates SHALL be provisional sessions with a successful turn and no previous title call; scheduling SHALL use the first successful turn and be idempotent. Disabling automatic titles SHALL suppress automatic scheduling and dispatch. Explicit scheduling and retry SHALL remain available.

#### Scenario: Complete the first turn

- **WHEN** the first turn of a provisionally titled session commits
- **THEN** its response is available before title scheduling or generation
- **AND** the automatic host separately queues at most one title request

#### Scenario: Restart before title scheduling

- **WHEN** the host restarts after a successful turn commits but before any title request is queued
- **THEN** automatic dispatch discovers that session and schedules its first successful turn once

#### Scenario: Restart before title execution

- **WHEN** the host restarts with a never-started pending title request
- **THEN** that request retains its identity and remains eligible for execution

#### Scenario: Disable automatic titles

- **WHEN** automatic titles are disabled and a conversation commits
- **THEN** no title request is automatically queued or started
- **AND** explicit scheduling and dispatch remain available

#### Scenario: Fail title generation

- **WHEN** the title provider fails or its call is cancelled or interrupted
- **THEN** the conversation remains committed and its provisional title remains available
- **AND** later conversations and automatic dispatch do not repeat that title attempt

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
