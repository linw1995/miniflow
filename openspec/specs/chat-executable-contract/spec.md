# chat-executable-contract Specification

## Purpose

Define application-owned conversation admission, model accounting, and reply recovery for compiled
chat workflows independently of process supervision and terminal presentation.

## Requirements

### Requirement: Validate the executable conversation contract

The backend SHALL validate a compiled manifest for one finite `builtin.llm` node, optionally preceded by
one initial `builtin.parameters` node. It MUST reject unsupported nodes, loops, control edges, stdin
resources, incompatible conversation input types, and missing model information before admission.

#### Scenario: Reject an incompatible executable

- **WHEN** the manifest describes streaming execution, extra model nodes, or missing conversation inputs
- **THEN** contract construction fails before storage is opened or a model request is made

#### Scenario: Accept a single model task

- **WHEN** a valid single-model manifest is supplied
- **THEN** the contract identifies the node for isolated-task execution with all declared LLM outputs

### Requirement: Bind configuration separately from conversation inputs

The backend SHALL expose a configuration schema excluding prompt, system, and history. A parameter node
MUST feed all three conversation inputs directly through distinct parameters. Parameterized workflows
MUST select text, history, optional usage, and optional finish_reason from the LLM under those exact names.
The caller SHALL resolve configuration using the shared workflow resolver and reuse it for each invocation.

#### Scenario: Preserve application-owned conversation inputs

- **WHEN** a caller validates configuration containing conversation input overrides
- **THEN** validation rejects those inputs and the application constructs them from the admitted turn

#### Scenario: Reject incomplete selected outputs

- **WHEN** a parameterized workflow omits, aliases, or misdeclares a required LLM output contract
- **THEN** preflight fails before the model request can produce an unsavable response

#### Scenario: Supply optional conversation inputs

- **WHEN** a parameterized invocation has no system instructions or saved history
- **THEN** the backend supplies an empty string and an empty list through their connected parameters

### Requirement: Derive accounting from executable configuration

The backend SHALL evaluate the manifest model expression against resolved configuration parameters and
reject secret, blank, or nonstring model identities. Configuration fingerprints SHALL include the stored
LLM configuration and public resolved parameters while excluding resolved credentials.

#### Scenario: Override a model without recompilation

- **WHEN** a public model parameter overrides its default
- **THEN** accounting records the same resolved model forwarded to the runner

#### Scenario: Rotate credentials

- **WHEN** resolved credentials change while public configuration remains unchanged
- **THEN** the accounting fingerprint remains unchanged and secret model identities remain rejected

### Requirement: Preserve caller-owned invocation identity

The backend SHALL admit caller-allocated session, turn, and call identities through existing atomic turn
operations. Inspection and cleanup MUST verify the invocation's session, turn, call purpose, and state.
Cleanup SHALL affect only the admitted invocation and MUST preserve an already committed success.

#### Scenario: Clean up one failed invocation

- **WHEN** one session's invocation fails while another session remains active
- **THEN** cleanup finalizes only the failed invocation and leaves the other running turn unchanged

#### Scenario: Reject mismatched identities

- **WHEN** inspection combines a turn with another session or conversation call
- **THEN** inspection rejects the conflict without changing either conversation

### Requirement: Reconcile output with durable completion

The backend SHALL decode bounded unique-key JSON through the typed LLM output contract. A valid reply
for a running invocation SHALL retain its original identity for persistence. A committed database reply
SHALL take precedence over late cancellation, malformed output, or a disagreeing captured reply, while
output diagnostics remain separately available. Missing or invalid output SHALL finalize admitted work.

#### Scenario: Preserve saved success

- **WHEN** invalid or disagreeing output is reconciled after persistence
- **THEN** the saved completion is returned unchanged with separate output diagnostics

#### Scenario: Reject malformed or oversized output

- **WHEN** captured output has duplicate keys, invalid field types, or exceeds 16 MiB
- **THEN** it is not retained as a valid reply and cleanup finalizes only the admitted invocation

### Requirement: Retry saving without invoking the model

The backend SHALL retry a retained conversation completion only in its owning database and with its
original identity and response. Failed saves MUST retain the reply and typed error source. Explicit
abandon SHALL finalize unfinished work without overwriting success. Model retry SHALL instead use a
fresh invocation linked to an unsuccessful original turn. Neither saving nor abandoning invokes a model.

#### Scenario: Retry a failed persistence transaction

- **WHEN** a reply save fails and the caller retries after storage recovers
- **THEN** the original reply commits once without another model call or duplicate usage

#### Scenario: Reject a foreign recovery value

- **WHEN** a retained completion names another database or a title call purpose
- **THEN** conversation save and abandon operations reject it without modifying stored work

#### Scenario: Lose an unsaved reply on application crash

- **WHEN** the owning application crashes before persisting its in-memory reply
- **THEN** reopening storage interrupts abandoned work without automatically resending a model request
