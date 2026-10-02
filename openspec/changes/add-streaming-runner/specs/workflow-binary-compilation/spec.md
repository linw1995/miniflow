# Spec Delta

## MODIFIED Requirements

### Requirement: Preserve conditional semantics in generated execution

Generated binaries SHALL implement the same per-run or streaming per-message context publication and reads, control activation, ordered selection,
explicit skip propagation, missing-output errors, and optional selected-output behavior as in-memory workflows. Single-run execution MUST retain statically generated node order and bindings
and MUST NOT require graph interpretation or special recognition of built-in kind names. Streaming execution SHALL execute generated node preparations and fixed bindings through the shared prepared executor using validated message boundaries, with the same order inside each frame. It MUST NOT interpret raw definition JSON or recognize built-in kinds to infer execution capabilities. Configuration and dependency
changes MUST continue to regenerate inputs and validate the current binary before installation. Existing standalone and
failure-preservation guarantees SHALL apply to conditional workflows.

#### Scenario: Match in-memory and compiled execution

- **WHEN** the same workflow with ordered conditions and downstream execution traces runs in memory and as a compiled binary
- **THEN** both select the same branch, execute the same node methods in the same order, and return the same output JSON or contextual error

#### Scenario: Rebuild after changing branch precedence

- **WHEN** branch order changes in a definition using a reused build directory
- **THEN** the newly validated binary reflects the new precedence while retaining unchanged port bindings

#### Scenario: Run a conditional binary independently

- **WHEN** a compiled conditional workflow is run without its source definition, dependency lock, plugin sources, or Rust toolchain
- **THEN** it performs conditional execution and returns the configured results

#### Scenario: Use a third-party router

- **WHEN** a selected third-party plugin reports valid instance ports and explicit skipped outputs
- **THEN** the generated binary applies the same skip rules without a built-in kind-name check

#### Scenario: Match context visibility across execution paths

- **WHEN** the same workflow triggers routing from one predecessor while comparing a transitive predecessor's stored output
- **THEN** both execution paths expose the same completed context values, select the same branch, and publish no partial or cross-run outputs

#### Scenario: Preserve qualified output IDs in generated execution

- **WHEN** in-memory and generated execution run multiple instances that expose the same local output name
- **THEN** both publish distinct `${node_id}.${output_name}` entries and resolve the same exact condition references

#### Scenario: Match conditional behavior within a stream frame

- **WHEN** the same streaming graph runs in memory and as a generated binary under the same input and clock schedule
- **THEN** both preserve branch selection, skips, context isolation, and selected results for each emitted message

## ADDED Requirements

### Requirement: Opt in to a typed streaming definition

Schema `2026-10-02` SHALL accept an `execution` object with `mode: "stream"`, required `input_type`, and optional resource limits. Without it, execution SHALL retain single-run behavior. Existing schemas MUST reject the new field. Streaming definitions SHALL expose the engine-owned root source `%input.item` with the declared type.

#### Scenario: Compile a typed stream

- **WHEN** a new-schema workflow declares integer input and connects `%input.item` to Batch
- **THEN** validation resolves integer input and list-of-integer batch output before installation

#### Scenario: Preserve an older definition

- **WHEN** a definition uses an existing accepted schema without streaming configuration
- **THEN** its execution, dependencies, and single-result output behavior are unchanged

#### Scenario: Require explicit streaming support

- **WHEN** a single-run definition contains Batch, or an older schema contains `execution`
- **THEN** validation fails with a mode or version diagnostic

#### Scenario: Reserve the input source

- **WHEN** a user node or plugin attempts to redefine the synthetic `%input` source
- **THEN** validation rejects the conflict

### Requirement: Validate activation and message boundaries before installation

Streaming validation SHALL require an explicit data or control path from `%input` to every ordinary root node. It SHALL reject mixed-domain dependencies, cross-domain context references, and mixed-domain selected outputs, including inactive branches. Graph, configuration, and type checks MUST run without executing nodes or reading input.

#### Scenario: Activate a configured source per message

- **WHEN** a configured constant has a control edge from `%input.item`
- **THEN** it is activated within each input frame rather than treated as an implicit global value

#### Scenario: Reject an unattached root

- **WHEN** a streaming graph contains an ordinary node with no path from `%input`
- **THEN** validation requires an explicit activation dependency

#### Scenario: Reject a control edge across a batch boundary

- **WHEN** a batch-domain node is also gated by an input-domain control output
- **THEN** validation reports the incompatible domains even if the value types otherwise fit

### Requirement: Keep synchronous bodies scoped to one message

Root Loop and Iteration nodes in streaming workflows SHALL execute as ordinary operations on one message under their existing scope, ordering, and failure contracts. Event-driven nodes, including Batch, MUST be rejected inside their synchronous bodies before installation, with the containing scope identified.

#### Scenario: Process a batch using an existing Iteration

- **WHEN** an emitted batch feeds the `items` input of an ordinary Iteration node
- **THEN** that invocation returns one collected result array using the existing item behavior

#### Scenario: Reject a Batch inside Iteration

- **WHEN** an Iteration body contains Batch
- **THEN** validation explains that the body does not support streaming state or timer events

### Requirement: Share prepared streaming orchestration across execution paths

Generated runners SHALL retain generated node preparation and fixed port bindings, prepare node instances once per workflow instance, and share prepared execution, resolved capabilities, and boundary validation with in-memory execution. Validation and execution MUST use the installed runner without a second build or source files at runtime.

#### Scenario: Run standalone after removing build inputs

- **WHEN** a streaming runner is moved without its definition, lockfile, plugins, or Rust toolchain
- **THEN** it still accepts inputs, flushes batches, and drains using the embedded workflow

#### Scenario: Preserve a valid installed executable

- **WHEN** a changed streaming graph fails type or domain validation during a warm build
- **THEN** the existing installed executable remains intact

### Requirement: Read bounded JSON Lines input while timers progress

Normal streaming runner mode SHALL read one UTF-8 JSON value per stdin line and validate it against the declared input type. It SHALL accept LF, CRLF, and a final nonempty record without a newline. Empty, malformed, oversized, or type-invalid records MUST fail with their line number. EOF SHALL close input; idle reads MUST NOT block timers.

#### Scenario: Flush while a pipe stays open

- **WHEN** a writer sends one valid line, leaves stdin open, and waits longer than the batch deadline
- **THEN** the runner emits the partial batch without needing another line or EOF

#### Scenario: Treat an array as one input value

- **WHEN** a valid input line contains an array
- **THEN** that array is admitted as one item rather than implicitly expanded

#### Scenario: Reject an invalid later line

- **WHEN** valid earlier lines have produced results and a later line is malformed
- **THEN** the runner exits unsuccessfully with the line number and leaves the already delivered output prefix intact

### Requirement: Deliver streaming results without waiting for EOF

Each selected stream result SHALL be written and flushed as one JSON line without building a final aggregate. Slow output MUST apply bounded backpressure, and output failure MUST fail execution. In stream mode, runner transport MUST reserve result stdout and direct plugin stdout diagnostics to stderr, including construction diagnostics.

#### Scenario: Deliver a complete batch early

- **WHEN** the first batch finishes while stdin remains open
- **THEN** its result line becomes available immediately to the output consumer

#### Scenario: Preserve result framing with a noisy plugin

- **WHEN** a plugin prints diagnostics during construction and execution
- **THEN** stdout still contains only workflow result JSON lines and diagnostics appear on stderr

#### Scenario: Fail a broken output pipe

- **WHEN** the result sink closes before the runner has delivered all selected outputs
- **THEN** the instance aborts and the runner exits unsuccessfully

### Requirement: Describe streaming requirements without starting an instance

Streaming runner descriptions SHALL identify execution mode, synthetic input, and required message-boundary protocol without exposing business values or configuration. `--describe` and `--validate` MUST NOT consume stdin, run event callbacks, arm timers, or emit workflow execution events.

#### Scenario: Validate with an idle open stdin

- **WHEN** a streaming runner is invoked with `--validate` while stdin remains open
- **THEN** validation completes without waiting for input or executing Batch

#### Scenario: Inspect a streaming runner

- **WHEN** a standalone runner is invoked with `--describe`
- **THEN** its description allows a host to detect the streaming input and observation requirements before launching execution
