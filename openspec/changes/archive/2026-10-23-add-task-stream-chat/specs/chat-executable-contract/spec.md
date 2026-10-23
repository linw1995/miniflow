## MODIFIED Requirements

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

## ADDED Requirements

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
