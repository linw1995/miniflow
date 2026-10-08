## MODIFIED Requirements

### Requirement: Validate workflow structure before code generation

Before generating a runner, the CLI SHALL validate nonblank unique node IDs, existing edge endpoints, selected
output
node references and names, and acyclicity. During the generated Cargo build, the build script SHALL
validate
registered kinds, configuration, existing ports, required input connections or promoted initial-node
parameters, and at most one connection per input. Promotion SHALL apply only to top-level initial nodes in
schema `2026-10-03`; noninitial nodes, nested bodies, and older schemas SHALL retain their existing
required-edge rules.
Build validation MUST NOT call node execution methods. The normal mode SHALL execute statically generated
orchestration.
Compiler validation SHALL accept statically safe assignments from refined types to the same type, compatible
refined
collection types, legacy broad supertypes, or `Any`. It SHALL also accept broad or `Any` outputs feeding a
refined input
when the source value is unknown and the shared runtime validates the actual value before invoking the target.
Known source values MUST be checked against target port types during validation; a known mismatch MUST fail
before
installation even when its inferred source descriptor is broad. Disjoint concrete types and incompatible
collection
shapes MUST fail validation; no implicit coercion SHALL occur.
Validation failures MUST include diagnostics that identify the relevant definition node, port, output, or
edge, and MUST NOT produce a successful binary.
The system MUST produce a deterministic topological execution order, using ascending definition ID to break
ties between ready nodes.
Port validation SHALL use a node instance's complete configuration-dependent port description when supplied,
and otherwise its static registration. Port names MUST be nonempty and unique within each direction. Base
descriptions and output derivations MUST depend only on configuration. Resolved output types MAY additionally
depend on upstream data bindings and MUST be consistent between validation and execution for the same
definition and linked plugins. Every node and branch MUST be validated even when it will be skipped during
execution.
Graph structure and topological order SHALL include both existing data edges and explicit control edges.
Control edges
MUST reference an existing source output and target node, MUST NOT create target input bindings, and MUST NOT
contain
duplicate identical entries. Required data-input and type compatibility rules SHALL continue to apply to data
edges.
Plugin validation SHALL build a unique index of `${node_id}.${output_name}` for all effective source outputs
and check
all declared context references by exact qualified-ID lookup. Qualified-ID collisions MUST fail with both
source pairs
before execution, regardless of whether the conflicting outputs could be skipped. Each referenced producer
MUST be a strict ancestor of the consumer through explicit data or control dependencies. Context references
MUST NOT
implicitly add dependencies. Unordered, self, and descendant references MUST fail even when a topological
tie-break
would place the referenced node first.

#### Scenario: Reject a cyclic workflow

- **WHEN** dependency edges in a definition form a cycle
- **THEN** compilation fails and reports a closed path containing only nodes in the cycle

#### Scenario: Stable order for independent nodes

- **WHEN** multiple nodes are ready to execute at the same point in a DAG
- **THEN** the compiler orders them by ascending definition ID regardless of node or edge declaration order

#### Scenario: Reject an invalid port connection

- **WHEN** an edge references a missing output or input port, or the connected port types are incompatible
- **THEN** compilation fails and reports both endpoints and the validation reason

#### Scenario: Validate a runtime-checked connection

- **WHEN** a source port of type `Any` feeds a target port of type `Int64`
- **THEN** compilation accepts the edge and the runner checks the actual JSON value before invoking the target

#### Scenario: Reject an incompatible refined connection

- **WHEN** a `List(String)` output feeds a `List(Int64)` input
- **THEN** compilation rejects the edge with both endpoints and their types

#### Scenario: Reject a known constant conflict

- **WHEN** a constant value `42` feeds a `String` input, directly or through an identity node
- **THEN** build validation fails before installation and identifies both edge endpoints and their types

#### Scenario: Reject a nested known conflict

- **WHEN** a constant value `[1, "x"]` feeds a `List(Int64)` input on an inactive branch
- **THEN** build validation fails without executing the branch and reports the failing path `/1`

#### Scenario: Keep an unknown broad source checked

- **WHEN** a plugin with no derivation exposes `Any` and feeds an `Int64` input
- **THEN** validation accepts the edge and execution checks the actual value at the target

#### Scenario: Reject an incomplete or ambiguous input

- **WHEN** a required input has neither a permitted startup parameter binding nor a data connection, or an
  input has more than one connection
- **THEN** compilation fails and identifies the node and input port

#### Scenario: Reject an invalid selected output

- **WHEN** a selected workflow output references an unknown node or output port, or repeats an output name
- **THEN** compilation fails and identifies the selected output

#### Scenario: Validate separate instances of one dynamic kind

- **WHEN** two instances of `builtin.if_else` declare different branch IDs and branch counts
- **THEN** each instance's edges and selected outputs are checked against its own derived ports

#### Scenario: Reject malformed instance port descriptions

- **WHEN** a plugin supplies an empty port name or duplicate input or output names
- **THEN** validation fails with the node and invalid descriptor context

#### Scenario: Activate a router without a data input

- **WHEN** a conditional node has valid incoming control dependencies and valid ancestor context references
- **THEN** validation succeeds without requiring a payload or condition input port

#### Scenario: Reject an unordered context reference

- **WHEN** a predicate references an existing output from a node outside the consumer's explicit ancestor
  chain
- **THEN** validation identifies the consumer, branch, and source and requires an explicit dependency instead
  of adding one

#### Scenario: Reject self or future references

- **WHEN** a predicate references its own node or a downstream node
- **THEN** validation fails before executable installation even if that predicate follows an always-matching
  branch

#### Scenario: Reject an unknown referenced output

- **WHEN** a predicate references a qualified output ID absent from the effective output index
- **THEN** validation reports the reference and its consumer branch before execution

#### Scenario: Reject ambiguous output identities

- **WHEN** effective output pairs `(a.b, c)` and `(a, b.c)` both form `a.b.c`
- **THEN** validation reports the collision before executable installation without splitting or resolving the
  key heuristically

#### Scenario: Include control dependencies in structural planning

- **WHEN** data and control edges together form a cycle, or a control edge names an unknown endpoint or
  duplicates an existing control edge
- **THEN** structural validation fails before runner generation

#### Scenario: Allow transitive source references

- **WHEN** `load_order` precedes `audit`, `audit` precedes `route`, and a route predicate reads
  `load_order.value`
- **THEN** validation accepts the reference without requiring another direct edge from `load_order` to `route`

#### Scenario: Validate every configured predicate

- **WHEN** a later branch has malformed predicate syntax, an invalid path escape, or an invalid literal for
  its operator
- **THEN** validation fails with node and branch context even if an earlier branch could match every input

#### Scenario: Validate an unselected branch

- **WHEN** a downstream node on an unselected branch has invalid configuration or a nonexistent port binding
- **THEN** build validation fails without invoking any node execution method

#### Scenario: Validate without startup values

- **WHEN** a new-schema initial task or stream producer declares required input ports
- **THEN** compilation validates and exposes those parameter requirements without executing the node or
  requiring invocation values during the build

### Requirement: Report build failures

The system SHALL report dependency resolution, runtime compatibility, runner-validation, generated-code, and Cargo
compilation failures as workflow compilation failures and preserve sufficient diagnostics to locate the error. The
system MUST NOT report partially generated files as a successful final binary. Failure MUST leave any existing output
executable intact. Failures after generated project creation SHALL retain that project and report its location. Plugin
diagnostic output MUST NOT corrupt the transfer of generated artifacts between build stages.

#### Scenario: Cargo compilation fails

- **WHEN** the generated runner or a plugin dependency fails to compile
- **THEN** the compile command returns a failure status and displays the relevant Cargo diagnostics

#### Scenario: A support package is unavailable

- **WHEN** the CLI cannot resolve its required versioned compiler or runtime package
- **THEN** compilation fails with the package and build-stage context instead of falling back to implicit source-checkout discovery

#### Scenario: Build validation fails

- **WHEN** the runner fails validation or exits unsuccessfully
- **THEN** compilation fails, preserves the existing executable, and identifies the retained project

#### Scenario: A plugin prints during construction

- **WHEN** a valid plugin factory writes diagnostics to standard output
- **THEN** compilation still uses the validation exit status and can install the executable

### Requirement: Preserve conditional semantics in generated execution

Generated binaries SHALL implement the same per-run or streaming per-message context publication and reads, control activation, ordered selection,
explicit skip propagation, missing-output errors, and optional selected-output behavior as in-memory workflows. Single-run execution MUST retain statically generated node order and bindings
and MUST NOT require graph interpretation or special recognition of built-in kind names. Streaming execution SHALL execute generated node preparations and fixed bindings through the shared prepared executor using validated message boundaries, with the same order inside each frame. It MUST NOT interpret raw definition JSON or recognize built-in kinds to infer execution capabilities. Configuration and dependency
changes MUST continue to regenerate inputs and validate providers during the current build before installation. Existing standalone and
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

### Requirement: Describe a compiled workflow without executing nodes

Generated runners SHALL embed a date-versioned JSON graph description containing workflow identity, node IDs and kinds,
named data edges, control edges, and deterministic execution order. Hosts SHALL read the graph record directly from the
embedded manifest without requiring original build inputs or invoking plugin factories. Unconnected or dynamic port
metadata MUST NOT be inferred from graph edges. The record MUST exclude node configuration and business values.

#### Scenario: Inspect a standalone binary

- **WHEN** a generated runner's manifest is read after its build inputs are removed
- **THEN** it provides its supported graph description without executing any node operation

#### Scenario: Describe a graph with configuration-dependent ports

- **WHEN** a compiled workflow contains two instances of a conditional node with different branch names
- **THEN** its description contains the configured data/control relationships without claiming to enumerate unconnected dynamic ports or including predicate configuration

#### Scenario: Avoid construction diagnostics

- **WHEN** a linked plugin factory normally prints diagnostics during validation
- **THEN** manifest inspection does not invoke that factory or start a process

### Requirement: Generate standalone observable runners

Generated runners SHALL include workflow observation boundaries and configurable OTel export initialization with bounded
shutdown. They MUST preserve statically generated orchestration, selected-output JSON, build validation, and
operation without the CLI, source definition, lockfile, plugin sources, Rust toolchain, or telemetry receiver.
Support-package resolution SHALL include matching observation support without including terminal UI dependencies.
Build validation and manifest inspection MUST NOT emit workflow execution events.

#### Scenario: Run with a Collector

- **WHEN** an independently launched runner is configured with an OTLP/HTTP Collector endpoint
- **THEN** it exports workflow observations without requiring a terminal interface or running CLI process

#### Scenario: Preserve plain execution

- **WHEN** the same runner is launched normally without export configuration
- **THEN** it follows the existing generated execution plan and returns the same selected output JSON and workflow exit behavior

#### Scenario: Build with inherited export settings

- **WHEN** compilation runs in an environment containing export settings
- **THEN** build validation remains free of node execution and does not emit a workflow execution run

### Requirement: Validate every Loop scope before code generation

Before generating a runner, the CLI SHALL validate nonblank unique node IDs within each scope,
existing local edge endpoints, selected output node references and names, scope boundaries, reserved
engine controls, Loop count and nesting limits, and acyclicity within every graph. Before
installing the executable, its build script SHALL require a registered Loop declaration and
validate ordinary registered kinds, configuration, existing ports, required input connections, typed Loop variables, assignments,
termination conditions, and context references in every scope. Validation MUST NOT execute node
implementations. A failed validation MUST preserve any previously installed output binary.

#### Scenario: Reject a body cycle before code generation

- **WHEN** a Loop body has a data or control cycle
- **THEN** structural planning fails with a cycle path in that body before generating the runner

#### Scenario: Reject an invalid body plugin before installation

- **WHEN** a body node has an unknown kind or invalid configuration
- **THEN** build validation fails before installation without running that node

#### Scenario: Preserve an existing binary after invalid Loop edit

- **WHEN** a previously valid Loop definition is edited into an invalid one and rebuilt
- **THEN** the build fails while preserving the installed executable and adjacent dependency lock

### Requirement: Share prepared streaming orchestration across execution paths

Generated runners SHALL retain generated node preparation and fixed port bindings, prepare node instances once per workflow instance, and share prepared execution, resolved capabilities, and boundary validation with in-memory execution. Normal execution MUST use the installed runner without a second build or source files at runtime.

#### Scenario: Run standalone after removing build inputs

- **WHEN** a streaming runner is moved without its definition, lockfile, plugins, or Rust toolchain
- **THEN** it still accepts inputs, flushes batches, and drains using the embedded workflow

#### Scenario: Preserve a valid installed executable

- **WHEN** a changed streaming graph fails type or domain validation during a warm build
- **THEN** the existing installed executable remains intact

### Requirement: Describe streaming requirements without starting an instance

New streaming graph descriptions SHALL identify execution mode and required observation protocol without exposing
business values or configuration. Their manifest interface record SHALL describe startup parameters and runtime input
resources with matching workflow identity. Direct manifest inspection and build validation MUST NOT consume source
input, invoke executors, arm timers, or emit workflow execution events.

#### Scenario: Inspect with an idle open stdin

- **WHEN** a streaming runner's manifest is inspected while stdin remains open
- **THEN** inspection completes without waiting for input or executing any source or Batch callback

#### Scenario: Inspect a streaming runner

- **WHEN** a standalone runner's graph and startup interface are inspected
- **THEN** the host can determine execution mode, required parameters, resource ownership, and observation compatibility before launching execution

### Requirement: Inspect configured startup interfaces in the installed runner

Runners SHALL embed a versioned interface record with workflow identity, startup input types/required flags, and resource
requirements. Hosts SHALL read it directly from the manifest without constructing providers, requiring build inputs, or
a second compilation. Interface discovery MUST NOT infer requirements from built-in kind names.

#### Scenario: Inspect dynamic input metadata

- **WHEN** an external factory defines configuration-dependent root input ports
- **THEN** interface inspection reports the complete effective input contract frozen during compilation without constructing or executing nodes

#### Scenario: Preserve clean output with noisy factories

- **WHEN** a linked factory normally writes diagnostics during preparation
- **THEN** manifest inspection does not invoke it and returns the frozen interface record

#### Scenario: Preserve one-build standalone inspection

- **WHEN** the validated executable is moved without source, plugin files, or a toolchain
- **THEN** both graph and interface inspection still work using that same executable

### Requirement: Accept startup parameters through shared execution arguments

Runners and `mf run` SHALL accept mutually exclusive `--inputs <JSON>` and `--inputs-file <PATH>`, with
omission meaning an empty object. Removed `--validate`, `--describe`, and `--describe-interface` options SHALL be rejected
as unknown workflow arguments. CLI parameter
transport SHALL be bounded to 1 MiB, preserve exact parsed values, and avoid interpreting parameter JSON as
stream data. Relative files SHALL resolve from the invocation directory.

#### Scenario: Start a file producer with a path parameter

- **WHEN** standalone and TUI launches receive the same valid nested startup argument object
- **THEN** both invoke the same initial nodes with equivalent values and selected results

#### Scenario: Read a parameter file once

- **WHEN** the TUI reads an inputs file and that user file changes before child execution
- **THEN** the child receives the values already validated by the CLI rather than rereading the changed file

#### Scenario: Reject excessive or conflicting arguments

- **WHEN** parameter transport exceeds 1 MiB or both parameter flags are supplied
- **THEN** launch fails with an argument diagnostic before node execution

### Requirement: Compile provider-dependent stream layout

The generated Cargo build SHALL resolve linked provider registrations before emitting stream layout constants. Execution
kind, message boundaries, and domain partitioning MUST come from provider construction contracts rather than built-in
kind-name assumptions. The build and executable SHALL use matching provider dependencies and features. Task and stream
layouts and their manifest interfaces SHALL come from the same validated preparation, without a separate metadata factory
API or additional preparation solely to generate the manifest.

#### Scenario: Compile an external producer

- **WHEN** an external package returns a Stream executor
- **THEN** the generated build emits its message boundary, execution-domain membership, and configured startup interface before the executable starts

#### Scenario: Reject invalid static construction

- **WHEN** provider metadata or workflow graph validation fails during generated construction
- **THEN** Cargo build fails before installation and preserves prior executable and dependency-lock guarantees

#### Scenario: Reuse the prepared contract

- **WHEN** generated execution layouts and a manifest are emitted for a workflow containing external providers
- **THEN** manifest generation uses the same prepared metadata as the layouts and does not repeat provider construction

#### Scenario: Regenerate changed metadata in a warm build

- **WHEN** a workflow configuration, selected provider implementation, or provider feature changes in a reusable build directory
- **THEN** the current build regenerates affected layouts and the manifest together and validates providers before installation

### Requirement: Generate standard runner entry points on demand

Standard task runners SHALL generate only workflow preparation and context execution; streaming runners SHALL generate only stream preparation. The general-purpose generator SHALL retain its existing convenience entry points. Both paths MUST share validation and execution semantics. Compilation MUST preserve Cargo diagnostics without suppressing unused-function warnings.

#### Scenario: Compile documented task workflows

- **WHEN** the CLI compiles a documented task workflow using the standard runner
- **THEN** generated workflow source contains only the preparation and context execution entry points needed by that runner, compilation emits no warnings from unused generated functions, and execution produces the documented outputs

#### Scenario: Compile a documented streaming workflow

- **WHEN** the CLI compiles a documented streaming workflow
- **THEN** generated workflow source contains stream preparation without task execution helpers, and execution produces the documented stream outputs

#### Scenario: Compile without telemetry

- **WHEN** the CLI compiles a task runner with `--no-telemetry`
- **THEN** unused convenience functions are not generated, compilation remains warning-free, and runner execution and direct manifest inspection remain available

#### Scenario: Generate a custom runner

- **WHEN** a caller requests general-purpose artifacts for a custom runner
- **THEN** the existing convenience functions remain available for startup arguments, observation, and runtime options, with the same workflow plan and runtime behavior

## ADDED Requirements

### Requirement: Validate providers during the runner build

Compilation SHALL validate selected providers and embedded configuration in the runner's Cargo build script while
generating execution layouts and a manifest. Build validation and normal execution MUST use matching provider
implementations, configuration, and features. Only a successful build SHALL be installed; the CLI MUST NOT launch
post-build validation. Cargo SHALL track generated inputs and provider dependencies.

#### Scenario: Build and validate once

- **WHEN** a valid Flow is compiled
- **THEN** one runner target is built, its build script validates providers without calling node execution, and that binary is installed

### Requirement: Require a complete embedded manifest before TUI execution

CLI inspection SHALL read supported graph and interface records directly from the executable with bounded metadata and
payload reads. Missing, corrupt, incomplete, oversized, or unsupported metadata MUST prevent TUI execution. Inspection
MUST NOT start the runner or invoke native startup code. Manifest generation and provider validation SHALL occur during
the existing single runner build without recompilation to embed metadata.

#### Scenario: Reject a runner without a manifest

- **WHEN** a supported executable has no manifest section
- **THEN** preflight reports that recompilation is required and does not start an inspection or execution process

#### Scenario: Preserve one runner build

- **WHEN** a workflow is compiled and installed
- **THEN** manifest generation and provider validation occur during that runner build without a post-build validation process

#### Scenario: Avoid native startup during manifest inspection

- **WHEN** a runner contains native startup code that writes diagnostics or performs initialization
- **THEN** direct manifest inspection does not start the runner or invoke that startup code

## REMOVED Requirements

### Requirement: Validate the same binary that is installed

**Reason**: Embedded manifests and build-time preparation replace runner inspection and validation modes.

**Migration**: Recompile workflows and inspect their embedded manifests; execute providers only during workflow startup.

### Requirement: Require complete isolated description output before execution

**Reason**: Embedded manifests and build-time preparation replace runner inspection and validation modes.

**Migration**: Recompile workflows and inspect their embedded manifests; execute providers only during workflow startup.
