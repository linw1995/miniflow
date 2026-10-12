# Single Node Execution Delta

## Purpose

Allow callers to independently invoke a top-level task embedded in a validated workflow, using its original
resolved contracts and explicitly supplied values without running surrounding graph operations.

## ADDED Requirements

### Requirement: Select one exact top-level task

New compiled runners SHALL accept one `--node ID` or `--node=ID` selector for a top-level Task executor,
including a Task in a stream flow. Selection MUST use the complete definition ID. Without a selector,
the runner SHALL preserve existing whole-flow behavior. Unknown, repeated, empty, and unsupported targets
MUST fail before any business operation or source read.

#### Scenario: Select a middle task

- **WHEN** a runner receives `--node transform` for a task with upstream and downstream nodes
- **THEN** it selects only that task for execution

#### Scenario: Preserve a dotted target identity

- **WHEN** the definition contains a top-level task named `group.transform`
- **THEN** `--node=group.transform` selects that exact ID without interpreting a nested scope

#### Scenario: Reject invalid selection

- **WHEN** selection names an unknown target, has no ID, has an empty ID, or occurs more than once
- **THEN** invocation fails with a target diagnostic before executing any node

#### Scenario: Reject unsupported executor selection

- **WHEN** selection names a producer, Event executor, nested-body node, or synthetic scope control
- **THEN** invocation fails with an unsupported or unknown target diagnostic without starting an instance

#### Scenario: Preserve ordinary execution

- **WHEN** the runner receives no selector
- **THEN** its ordinary graph inputs, dependency behavior, selected results, and execution mode are unchanged

### Requirement: Validate selected inputs against original resolved contracts

Selected tasks SHALL accept qualified named inputs, `--inputs`, or `--inputs-file` using their original
graph-resolved port types and required flags. Named inputs MUST NOT mix with JSON input transport. Unknown
ports, missing required values, duplicate keys, and type violations MUST fail before invocation. Values for
other workflow roots MUST NOT be required or accepted.

#### Scenario: Supply a connected task input directly

- **WHEN** a middle task normally receives a required input from an edge and isolated execution supplies it
- **THEN** the task receives the supplied value without reading the edge's producer

#### Scenario: Retain inference from the graph

- **WHEN** a target's contract was refined from its original upstream binding and a supplied value violates it
- **THEN** invocation rejects the value without weakening the contract or invoking the upstream node

#### Scenario: Ignore unrelated root requirements

- **WHEN** another root requires a path and the selected task's inputs are valid
- **THEN** selection neither requires that path nor starts that source

#### Scenario: Reject unrelated parameters

- **WHEN** selected execution supplies a qualified parameter belonging to another node
- **THEN** invocation rejects that parameter before business execution

#### Scenario: Select from an older supported definition

- **WHEN** a newly compiled runner uses a supported schema predating startup-input promotion
- **THEN** selected execution accepts the target's input ports while ordinary execution retains that schema's rules

### Requirement: Preserve argument values and presence semantics

Selected inputs SHALL follow the existing text, nullable-string, and JSON parsing rules. Complete qualified
names MUST remain exact. Optional omissions SHALL remain absent, and explicit null SHALL be validated as a
value. Runner control parsing MUST respect value boundaries rather than interpreting a port value as a
selector or context control.

#### Scenario: Supply a string resembling a runner control

- **WHEN** the caller supplies `--transform.amount=--node` to a string input
- **THEN** the target receives the string `--node` without introducing another selector

#### Scenario: Preserve omission and explicit null

- **WHEN** a caller omits an optional input and supplies null to a nullable input
- **THEN** the omitted port remains absent and the nullable port receives explicit null

#### Scenario: Reject null for a nonnullable input

- **WHEN** a caller supplies null to an optional nonnullable string port
- **THEN** validation rejects the value before invoking the target

### Requirement: Supply declared external context explicitly

Selected execution SHALL accept either `--context JSON` or `--context-file PATH`, exclusively and only with
`--node`. Context MUST be a flat object keyed by complete declared external reference IDs. Every declared
reference MUST be supplied and validated against its original resolved output type. Undeclared keys,
duplicate keys, and invalid values MUST fail before invocation. Null MUST NOT represent a skipped value.

#### Scenario: Execute a context-reading router

- **WHEN** a selected router declares `source.value` and the caller supplies that reference with a valid value
- **THEN** the router reads the supplied context without executing the source

#### Scenario: Reject missing or undeclared context

- **WHEN** a required reference is absent or the context document contains an undeclared key
- **THEN** invocation fails with reference context before the target executes

#### Scenario: Validate referenced values

- **WHEN** a referenced output is an integer and context supplies a string or null
- **THEN** validation rejects it with the reference ID and type mismatch

#### Scenario: Reject invalid context transport

- **WHEN** both context transports are present or a context transport is supplied without `--node`
- **THEN** invocation fails with an argument conflict diagnostic

### Requirement: Bound isolated argument transport

Port input and context transports SHALL each fit within 1 MiB, and their combined canonical JSON payload
MUST fit within 1 MiB. Context files SHALL resolve from the invocation directory, be read once with a bounded
reader, and require regular files. Nonregular context files MUST be rejected without waiting for data.

#### Scenario: Reject excessive combined values

- **WHEN** individually valid input and context documents exceed 1 MiB after combined canonical encoding
- **THEN** invocation fails before business execution

#### Scenario: Read a relative context file

- **WHEN** selection supplies a relative context-file path
- **THEN** the runner reads and validates it once relative to its invocation directory

#### Scenario: Reject a context FIFO

- **WHEN** the context-file path names a FIFO without a writer
- **THEN** invocation reports a nonregular input diagnostic without waiting for a writer

### Requirement: Isolate selected business execution

Each selected Task SHALL receive fresh invocation state and execute once through normal runtime input,
execution, and publication boundaries. External data and control predecessors and successors MUST NOT
execute. Selecting a Task from a stream flow MUST NOT start a stream instance. Whole-plan factory
preparation MAY still occur and fail, but MUST NOT invoke business methods or consume source data.

#### Scenario: Bypass a control predecessor

- **WHEN** the selected task normally waits for an upstream control output
- **THEN** isolated execution invokes it without executing or requiring that control predecessor

#### Scenario: Avoid typed neighbors

- **WHEN** the target belongs to a generated typed serial segment
- **THEN** only the target executes and adjacent tasks in the segment remain uninvoked

#### Scenario: Select a task from a stream flow

- **WHEN** a stream binary selects an ordinary downstream task with valid supplied values
- **THEN** it returns one finite result without starting its producer, timers, or downstream consumers

#### Scenario: Keep invocation state independent

- **WHEN** the same prepared target is invoked twice with different values
- **THEN** each invocation observes only its own port arguments, context seeds, and results

#### Scenario: Report an unrelated preparation failure

- **WHEN** ordinary whole-plan preparation fails in an unrelated provider
- **THEN** selection reports a preparation failure and no target or unrelated business method executes

### Requirement: Execute composite targets with their existing scopes

A selected top-level Loop or Iteration SHALL execute as a complete prepared Task operation. Its compiled
body, resource limits, worker ownership, ordering, exit behavior, and error policy SHALL retain existing
semantics. Context visibility inside the body MUST remain governed by the original scope contract.

#### Scenario: Select a Loop

- **WHEN** a top-level Loop is selected with valid outer inputs
- **THEN** its body runs under normal pass and exit semantics without executing surrounding graph nodes

#### Scenario: Select a parallel Iteration

- **WHEN** a top-level parallel Iteration is selected with a supplied collection
- **THEN** it preserves ordered results, configured error policy, and bounded shared workers

#### Scenario: Preserve a stream worker ceiling

- **WHEN** a parallel Iteration is selected from a stream flow whose worker limit is one
- **THEN** its body retains that ceiling even when caller runtime options allow more workers

### Requirement: Route only selected input resources

Selected execution SHALL evaluate the target's prepared stdin requirement against its supplied inputs.
Unavailable required resources MUST fail before invocation. Other nodes' stdin or source requirements MUST
NOT cause resource consumption. Selection MUST NOT expose stdin to a target that does not declare it.

#### Scenario: Bypass conditional stdin

- **WHEN** the selected task requires stdin only unless an input is supplied and that input is present
- **THEN** execution does not consume stdin

#### Scenario: Reject unavailable selected stdin

- **WHEN** the target requires stdin and the execution host does not provide it
- **THEN** resource validation fails before the task executes

### Requirement: Return validated target port results

Successful selected execution SHALL return one JSON object containing all produced target outputs under
exact port names, independently of workflow output aliases or selection. Explicitly skipped ports SHALL be
omitted; explicit null SHALL remain a value. Zero produced outputs SHALL return an empty object. Invalid
outputs or target failures MUST return a nonzero status and MUST NOT emit a successful result object.

#### Scenario: Return an output absent from workflow selection

- **WHEN** a selected task produces `value` but the workflow does not select that port
- **THEN** the finite result contains `value` under its own port name

#### Scenario: Preserve output presence

- **WHEN** a task produces one null output and explicitly skips another port
- **THEN** the result retains the null output and omits the skipped port

#### Scenario: Reject invalid publication

- **WHEN** the selected task produces a value violating a declared output contract
- **THEN** execution fails with producer and port context before publishing a successful result

### Requirement: Preserve isolated result framing and portability

Selected Task execution SHALL reserve result stdout before provider construction and route plugin stdout
diagnostics to stderr. Newly compiled runners SHALL retain isolated execution after build inputs are
removed, with and without telemetry support. Existing executables MUST be recompiled to gain selection.

#### Scenario: Keep noisy diagnostics separate

- **WHEN** a plugin prints during construction and selected execution
- **THEN** stdout contains only the finite result JSON and diagnostics appear on stderr

#### Scenario: Execute a moved runner

- **WHEN** a new runner is moved without its definition, generated project, plugin sources, or Rust toolchain
- **THEN** valid selected Task execution still succeeds, including a runner built without telemetry
