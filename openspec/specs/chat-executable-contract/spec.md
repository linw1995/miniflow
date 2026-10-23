# chat-executable-contract Specification

## Purpose

Define application-owned conversation admission, model accounting, and reply recovery for embedded and compiled
chat workflows independently of process supervision and terminal presentation.

## Requirements

### Requirement: Validate the executable conversation contract

The backend SHALL validate a finite executable through its external startup and final-result contracts. It MUST NOT restrict internal node kinds, model counts, control edges, loop bodies, or output source nodes. Advertised finite live-output protocols SHALL be supported alongside final JSON. Top-level streaming execution and required stdin resources remain unsupported. The caller SHALL execute the whole workflow rather than selecting an internal model task.

#### Scenario: Accept a multi-model workflow

- **WHEN** a finite workflow exposes compatible conversation inputs and result outputs
- **THEN** preflight accepts it regardless of its internal model and preprocessing nodes

#### Scenario: Execute repeated model calls

- **WHEN** a workflow explicitly accumulates usage records inside a bounded loop
- **THEN** each reported call remains identifiable and the loop is not rejected by chat preflight

#### Scenario: Reject an incompatible executable

- **WHEN** the manifest describes top-level streaming execution or missing required conversation inputs
- **THEN** contract construction fails before storage is opened or a model request is made

#### Scenario: Accept a single model task

- **WHEN** a valid single-model manifest is supplied
- **THEN** the contract accepts its external interface for whole-workflow execution

#### Scenario: Admit a finite workflow with live output

- **WHEN** a finite executable advertises a supported live protocol and compatible public answer stream
- **THEN** preflight accepts it without inspecting or reconfiguring internal model nodes

### Requirement: Bind configuration separately from conversation inputs

The backend SHALL bind an unambiguous prompt string and optional system/history/context inputs by their external
port names. It SHALL reject ambiguous or mistyped conversation inputs and expose all remaining startup
parameters for ordinary resolution. Parameter node kinds, source node counts, and direct LLM edges SHALL
NOT be required. The output interface SHALL declare status, text, and a nonoptional usages list; text may
be absent on failure. Metadata, error, finish_reason, and title outputs are optional. Both execution paths SHALL use the same ConversationInputs, ConversationResult, argument binding, and reconciliation.

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
collect hidden node outputs. Admission SHALL NOT require configuration fingerprints.

#### Scenario: Combine multiple models

- **WHEN** a workflow concatenates two LLM usage lists
- **THEN** accounting retains both call identities, models, and reported counters

#### Scenario: Reject duplicated call facts

- **WHEN** one result contains repeated call IDs
- **THEN** result validation rejects it rather than charging the same call twice

#### Scenario: Override a model without recompilation

- **WHEN** a public model parameter overrides its default
- **THEN** accounting records the same resolved model forwarded to the runner

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
summary and SHALL NOT create an assistant message or propose a title. Metadata belongs only to a successful new assistant
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
text, metadata, title, and usages; failed-result retries SHALL retain the captured result until storage facts
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

- **WHEN** a retained completion names another database or another invocation
- **THEN** conversation save and abandon operations reject it without modifying stored work

#### Scenario: Lose an unsaved reply on application crash

- **WHEN** the owning application crashes before persisting its in-memory reply
- **THEN** reopening storage interrupts abandoned work without automatically resending a model request

### Requirement: Recover without executable metadata

Decoding and reconciling captured output SHALL require only the admitted invocation and result bytes,
without rebuilding an executable manifest or conversation input contract. Existing bounded parsing,
ownership checks, and idempotent persistence SHALL remain in effect.

#### Scenario: Retry saving after the executable is unavailable

- **WHEN** the application retains result bytes and the admitted invocation
- **THEN** it can decode and save the result without loading the original executable or manifest

### Requirement: Keep title eligibility and failure policy inside the flow

The host SHALL pass a title-state snapshot as context and SHALL accept an optional title from a
successful result. Flow branches SHALL determine title eligibility, usage composition, and handling
of captured model failures. The host MUST NOT inspect internal model outputs or independently retry
stages. Saving SHALL validate the captured title revision and preserve concurrent manual names.

#### Scenario: Bind context without exposing an override parameter

- **WHEN** a workflow declares a context startup input
- **THEN** the application supplies title state and removes context from caller configuration parameters

#### Scenario: Preserve workflows without optional title support

- **WHEN** a workflow declares neither context nor title
- **THEN** its existing prompt, history, and result contract remains executable

#### Scenario: Retain a title across save retries

- **WHEN** a successful result contains a title and its completion write fails
- **THEN** retry retains the same title candidate and usages without another provider call

### Requirement: Negotiate live output from external contracts

Executable callers SHALL choose live mode only from advertised protocol and public stream contracts. A public `answer` stream SHALL use the shared typed answer-increment contract; incompatible declared contracts MUST fail preflight. Executables without live capability SHALL retain final-JSON execution. Unsupported protocol versions MUST fail before model work rather than trigger a second invocation.

#### Scenario: Use an older conversation executable

- **WHEN** a valid executable exposes only the final conversation contract
- **THEN** chat runs it once and displays its complete result without requiring live records

#### Scenario: Reject a mistyped answer export

- **WHEN** an executable declares public `answer` increments with an incompatible value contract
- **THEN** admission fails before starting the executable's business work

### Requirement: Reconcile only authoritative final results

Live increments and stream-close controls SHALL remain provisional and MUST NOT settle a conversation, update history/title, or synthesize usage. Only a complete validated final conversation result SHALL enter existing settlement and save-retry handlers. Already committed success SHALL retain precedence over later diagnostics.

#### Scenario: End the answer stream before title generation

- **WHEN** the answer stream closes while a title node remains active
- **THEN** the turn remains running until the final workflow result is reconciled

#### Scenario: Lose the executable after a live prefix

- **WHEN** the child exits or is cancelled before a valid final result is captured
- **THEN** the turn follows failed/cancelled/interrupted cleanup and partial text is not saved as an assistant message

### Requirement: Capture live output independently of rendering

Executable supervision SHALL parse bounded live records and drain stdout while the UI is idle. It MUST retain only the bounded final-result recovery value rather than spool the complete live history as final JSON. Protocol validation SHALL reject wrong execution identity, ordering, lifecycle, and size violations while retaining typed capture causes.

#### Scenario: Generate more output than a pipe can buffer

- **WHEN** a valid child writes enough live records to fill an undrained pipe and rendering is paused
- **THEN** independent capture continues consuming subject to bounded delivery and cancellation remains available

#### Scenario: Receive an incomplete final record

- **WHEN** the process stops halfway through a final JSON-lines record
- **THEN** recovery does not treat its earlier deltas as a complete reply or confirmed usage
