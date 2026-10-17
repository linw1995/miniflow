# chat-executable-contract Specification

## Purpose

Define application-owned conversation admission, model accounting, and reply recovery for compiled
chat workflows independently of process supervision and terminal presentation.

## Requirements

### Requirement: Validate the executable conversation contract

The backend SHALL validate a finite JSON executable through its external startup and result contracts.
It MUST NOT restrict internal node kinds, model counts, control edges, loop bodies, or output source nodes.
Streaming result protocols and required stdin resources remain unsupported. The caller SHALL execute the
whole workflow rather than selecting an internal model task.

#### Scenario: Accept a multi-model workflow

- **WHEN** a finite workflow exposes compatible conversation inputs and result outputs
- **THEN** preflight accepts it regardless of its internal model and preprocessing nodes

#### Scenario: Execute repeated model calls

- **WHEN** a workflow explicitly accumulates usage records inside a bounded loop
- **THEN** each reported call remains identifiable and the loop is not rejected by chat preflight

#### Scenario: Reject an incompatible executable

- **WHEN** the manifest describes streaming execution or missing required conversation inputs
- **THEN** contract construction fails before storage is opened or a model request is made

#### Scenario: Accept a single model task

- **WHEN** a valid single-model manifest is supplied
- **THEN** the contract accepts its external interface for whole-workflow execution

### Requirement: Bind configuration separately from conversation inputs

The backend SHALL bind an unambiguous prompt string and optional system/history inputs by their external
port names. It SHALL reject ambiguous or mistyped conversation inputs and expose all remaining startup
parameters for ordinary resolution. Parameter node kinds, source node counts, and direct LLM edges SHALL
NOT be required. The output interface SHALL declare status, text, and a nonoptional usages list; text may
be absent on failure. Metadata, error, and finish_reason outputs are optional.

#### Scenario: Preserve application-owned conversation inputs

- **WHEN** configuration arguments attempt to override a reserved conversation input
- **THEN** validation rejects the override before model work

#### Scenario: Bind conversation inputs on a custom source

- **WHEN** a custom startup node declares the compatible prompt, system, and history ports
- **THEN** the application binds those ports without requiring a parameter or LLM node

#### Scenario: Reject incomplete selected outputs

- **WHEN** a workflow omits status or usages, or makes usages optional
- **THEN** preflight fails before the model request can produce an unsavable response

#### Scenario: Supply optional conversation inputs

- **WHEN** a parameterized invocation has no system instructions or saved history
- **THEN** the backend supplies an empty string and an empty list through their connected parameters

### Requirement: Attribute usage to reported model calls

The workflow SHALL explicitly return one usage record per reported model attempt, including repeated
calls and calls without counters. Records SHALL contain unique call identities, resolved public model
names, and node/loop identity when available. The backend MUST NOT infer a model from graph topology or
collect hidden node outputs. Admission configuration identity SHALL fingerprint the workflow and public
resolved parameters, excluding resolved secrets.

#### Scenario: Combine multiple models

- **WHEN** a workflow concatenates two LLM usage lists
- **THEN** accounting retains both call identities, models, and reported counters

#### Scenario: Reject duplicated call facts

- **WHEN** one result contains repeated call IDs
- **THEN** result validation rejects it rather than charging the same call twice

#### Scenario: Override a model without recompilation

- **WHEN** a public model parameter overrides its default
- **THEN** accounting records the same resolved model forwarded to the runner

#### Scenario: Rotate credentials

- **WHEN** resolved credentials change while public configuration remains unchanged
- **THEN** the accounting fingerprint remains unchanged and built-in LLMs continue to require public model identifiers

### Requirement: Preserve caller-owned invocation identity

The backend SHALL admit caller-allocated session, turn, and execution identities through existing atomic
turn operations. One execution MAY report many actual model calls. Inspection and cleanup MUST verify
execution ownership and preserve an already committed success.

#### Scenario: Clean up one failed execution

- **WHEN** one session fails while another remains active
- **THEN** cleanup changes only the admitted execution and its turn

#### Scenario: Clean up one failed invocation

- **WHEN** one session's invocation fails while another session remains active
- **THEN** cleanup finalizes only the failed invocation and leaves the other running turn unchanged

#### Scenario: Reject mismatched identities

- **WHEN** inspection combines a turn with another session or conversation call
- **THEN** inspection rejects the conflict without changing either conversation

### Requirement: Reconcile output with durable completion

The backend SHALL decode bounded unique-key JSON and require succeeded or failed status with a usages
list. Success SHALL require nonempty text and no nonempty error. Failure SHALL require a nonblank error
summary and SHALL NOT create an assistant message. Metadata belongs only to a successful new assistant
message. Stored success MUST win over late errors or conflicting output, with separate diagnostics.

#### Scenario: Persist a failed workflow result

- **WHEN** a failed result contains successful and failed model-call records
- **THEN** the backend atomically saves every reported call and the failed turn without an assistant message

#### Scenario: Preserve saved success

- **WHEN** failure or conflicting output arrives after success was saved
- **THEN** the saved reply, metadata, and usages remain unchanged

#### Scenario: Reject malformed or oversized output

- **WHEN** captured output has duplicate keys, invalid field types, or exceeds 16 MiB
- **THEN** it is not retained as a valid reply and cleanup finalizes only the admitted invocation

### Requirement: Retry saving without invoking the model

The backend SHALL save retained results only for their owning execution. Success retries SHALL retain
text, metadata, and usages; failed-result retries SHALL retain the captured result until storage facts
can be reconciled. Repeated persistence MUST NOT duplicate messages or usage. Explicit abandon SHALL
finalize unfinished work without model execution or overwriting committed success.

#### Scenario: Recover from a failed result write

- **WHEN** the transaction saving a failed workflow's usages fails
- **THEN** no partial usages or terminal state are committed and retrying the captured result can save once

#### Scenario: Lose a result before delivery

- **WHEN** cancellation or process death prevents delivery of a final usage list
- **THEN** the execution remains unreported rather than being treated as confirmed zero model calls

#### Scenario: Retry a failed persistence transaction

- **WHEN** a reply save fails and the caller retries after storage recovers
- **THEN** the original reply commits once without another model call or duplicate usage

#### Scenario: Reject a foreign recovery value

- **WHEN** a retained completion names another database or a title call purpose
- **THEN** conversation save and abandon operations reject it without modifying stored work

#### Scenario: Lose an unsaved reply on application crash

- **WHEN** the owning application crashes before persisting its in-memory reply
- **THEN** reopening storage interrupts abandoned work without automatically resending a model request
