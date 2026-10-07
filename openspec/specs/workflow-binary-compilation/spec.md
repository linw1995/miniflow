# workflow-binary-compilation Specification

## Purpose

This capability converts declarative DAG workflow definitions into deployable native executables that include the required node plugins and a fixed workflow execution plan.

## Requirements

### Requirement: Compile workflow definitions into executable binaries

The system SHALL provide a compilation entry point that reads workflow definitions with a `YYYY-MM-DD` date-formatted version and produces an executable workflow binary. The system MUST reject malformed or unsupported definition versions. The binary MUST encode the validated node order and port bindings as generated executable code, run that workflow when started, and require no source definition file at runtime.
An installed CLI SHALL compile a project with declared third-party nodes without rebuilding the CLI, editing a predefined bundle, or requiring a miniflow source checkout. Compilation SHALL require Cargo, a compatible Rust toolchain, available versioned support packages, and the declared dependencies. The executable MUST run without the dependency lock, Cargo, or plugin source directories.

#### Scenario: Compile a valid workflow

- **WHEN** a user compiles a structurally valid DAG definition that references registered node kinds
- **THEN** the system generates and compiles a runner and outputs an executable workflow binary

#### Scenario: Run a compiled workflow

- **WHEN** a user runs the generated binary
- **THEN** the system executes nodes according to the defined dependencies and returns the selected workflow outputs

#### Scenario: Build third-party nodes with an installed CLI

- **WHEN** a user with the required build tools compiles a project outside the miniflow checkout using available declared third-party node dependencies
- **THEN** the installed CLI produces the executable without modification or rebuilding of the CLI

#### Scenario: Run without build inputs

- **WHEN** the generated executable is moved to a compatible runtime environment without its project files, plugin sources, or Rust build tools
- **THEN** it executes the embedded workflow and produces the selected outputs

### Requirement: Resolve plugins from the compile-time registry

The system SHALL resolve node kinds from the registry assembled from the definition's embedded dependencies during
compilation, independently of the plugins linked into the installed CLI. Plugin-dependent configuration and port
validation SHALL use those selected implementations before the executable is installed. A generated workflow
binary MUST link the same selected plugin implementations and use the same resolved package identities and features. The
iteration order of the plugin registry MUST NOT affect node resolution or workflow execution order.

#### Scenario: Compile with a linked plugin

- **WHEN** a definition references a kind registered by a declared plugin linked into the project's validation target
- **THEN** compilation constructs that node to validate configuration and includes the selected implementation in the generated binary

#### Scenario: Reject an unavailable plugin

- **WHEN** a definition references a kind unregistered by the selected dependencies
- **THEN** compilation fails and identifies the node ID and unknown kind

#### Scenario: Validate third-party configuration

- **WHEN** a declared third-party node rejects its workflow configuration
- **THEN** compilation fails before executable installation and reports the node ID, kind, and configuration error

### Requirement: Validate workflow structure before code generation

Before generating a runner, the CLI SHALL validate nonblank unique node IDs, existing edge endpoints, selected
output
node references and names, and acyclicity. Before installing the executable, its validation mode SHALL
validate
registered kinds, configuration, existing ports, required input connections or promoted initial-node
parameters, and at most one connection per input. Promotion SHALL apply only to top-level initial nodes in
schema `2026-10-03`; noninitial nodes, nested bodies, and older schemas SHALL retain their existing
required-edge rules.
Validation mode MUST NOT call node execution methods. The normal mode SHALL execute statically generated
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
- **THEN** runner validation fails before installation and identifies both edge endpoints and their types

#### Scenario: Reject a nested known conflict

- **WHEN** a constant value `[1, "x"]` feeds a `List(Int64)` input on an inactive branch
- **THEN** runner validation fails without executing the branch and reports the failing path `/1`

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

#### Scenario: Runner validation fails

- **WHEN** the runner fails validation or exits unsuccessfully
- **THEN** compilation fails, preserves the existing executable, and identifies the retained project

#### Scenario: A plugin prints during construction

- **WHEN** a valid plugin factory writes diagnostics to standard output
- **THEN** compilation still uses the validation exit status and can install the executable

### Requirement: Migrate definitions to the dependency-aware schema

The CLI SHALL accept workflow schema `2026-09-26`, with explicit embedded dependencies, and reject the previous
`2026-09-24` schema with instructions to update the version and declare the packages providing its node kinds. Existing
node configuration, graph connections, and selected-output semantics SHALL remain unchanged. Unsupported and malformed
versions MUST continue to fail rather than selecting implicit dependencies.

#### Scenario: Compile an old definition

- **WHEN** the user supplies a definition with version `2026-09-24`
- **THEN** compilation fails before dependency resolution and explains how to migrate its version and dependencies

#### Scenario: Compile a migrated definition

- **WHEN** the user updates an old definition to `2026-09-26` and explicitly declares its required node packages
- **THEN** the same graph retains its execution and selected-output semantics

### Requirement: Reuse generated build directories

The system SHALL automatically reuse an application-owned build directory for repeated compilation of the same canonical
Flow definition with a compatible CLI and generated-project layout. `--build-dir <path>` SHALL select an explicit build
directory relative to the invocation directory. Successful and failed builds SHALL retain generated projects and Cargo
artifacts for subsequent use. Changing only the output path MUST NOT prevent reuse. Diagnostics SHALL identify the
selected directory and whether it was created or reused. Removing an inactive directory MUST NOT remove required Flow
inputs or prevent a subsequent clean build.

#### Scenario: Repeat an unchanged build

- **WHEN** a Flow is compiled twice with unchanged build inputs
- **THEN** the second invocation reuses the same directory and fresh Cargo artifacts, still validates the Flow, and does not rewrite identical generated files

#### Scenario: Select a reusable temporary directory

- **WHEN** successive invocations select the same compatible directory with `--build-dir`
- **THEN** they retain and reuse its generated project and Cargo artifacts across process exits

#### Scenario: Change only the executable destination

- **WHEN** the same Flow is compiled with a different output path
- **THEN** its default build directory remains the same and the requested output is installed after successful validation and compilation

#### Scenario: Recreate a removed cache entry

- **WHEN** an inactive build directory has been deleted before compilation
- **THEN** the system recreates it from the current definition and dependency lock and builds normally

### Requirement: Reuse artifacts only after validating current inputs

Directory reuse MUST NOT bypass current definition validation, dependency-lock rules, runtime compatibility checks, or
Cargo freshness checks. Generated inputs SHALL reflect the current graph, configuration, dependencies, and features.
The adjacent Flow lock MUST remain authoritative over a cached working lock. Validation artifacts MUST come from the
current successful attempt; failed or interrupted attempts and old executables MUST NOT be accepted as current results.
Local source and toolchain changes SHALL be handled through Cargo on every invocation.

#### Scenario: Update graph or configuration

- **WHEN** a previously built Flow changes its graph or node configuration
- **THEN** the next build validates the changed definition and produces updated behavior while retaining reusable dependency artifacts

#### Scenario: Update dependencies or build environment

- **WHEN** dependencies, features, the Flow lock, local plugin source, or the Rust toolchain change
- **THEN** the system resynchronizes applicable generated inputs and invokes Cargo to rebuild affected artifacts rather than directly returning the cached executable

#### Scenario: A cached working lock differs from the Flow lock

- **WHEN** a prior failed attempt left a different working Cargo lock in the reused directory
- **THEN** the next invocation uses the current Flow lock and honors `--locked`, including failure when the Flow lock is absent

#### Scenario: Retry after interrupted validation

- **WHEN** a prior validation attempt failed or was interrupted before completing its checks
- **THEN** the next build requires a successful validation of the current executable and cannot install a runner using the abandoned results

### Requirement: Protect reusable build directories

The system SHALL serialize access to a selected build directory and report contention without modifying an active
build. Reuse SHALL require ownership and compatibility metadata matching the definition, CLI, and project layout.
A nonempty unrecognized or incompatible explicit directory MUST be rejected without clearing its contents. A recognized
partial directory SHALL be recoverable by regenerating owned inputs. Definitions, dependency locks, and final outputs
MUST NOT reside inside the managed build directory, including through canonical path aliases. Generated source and Cargo target directories MUST NOT be symbolic links.

#### Scenario: Reject another Flow's directory

- **WHEN** an explicit directory belongs to a different Flow or incompatible CLI/layout version
- **THEN** compilation reports the mismatch and leaves the existing contents intact

#### Scenario: Reject concurrent reuse

- **WHEN** another process holds the selected build directory lock
- **THEN** the invocation reports contention and leaves that build's generated inputs and artifacts unchanged

#### Scenario: Recover recognized partial state

- **WHEN** an interrupted build left a valid ownership marker but incomplete generated source files
- **THEN** a later invocation repairs the generated inputs and requires successful validation and compilation before installing output

#### Scenario: Reject overlapping user files

- **WHEN** the selected build directory contains the Flow definition, dependency lock, or requested final output path
- **THEN** compilation fails before modifying those files

#### Scenario: Reject redirected generated directories

- **WHEN** a reused build directory has a symbolic link at its generated source or Cargo target directory
- **THEN** compilation fails before generating files or invoking Cargo and preserves the linked contents and existing user inputs

### Requirement: Validate the same binary that is installed

Compilation SHALL build one runner with a validation mode and a normal execution mode. The CLI MUST invoke validation on the newly built binary before installation, including warm builds. Successful validation MUST NOT require a second runner compilation. Validation and normal execution SHALL use the same linked plugin implementations, embedded configuration, and feature selections.

#### Scenario: Build and validate once

- **WHEN** a valid Flow is compiled
- **THEN** one runner target is built, its validation mode succeeds without calling node execution, and that binary is installed

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

### Requirement: Use resolved type metadata in generated execution

In-memory execution and generated binaries SHALL use the same resolved port types for output publication and
input validation. Generated binaries MUST derive graph-dependent metadata from their embedded definition and
linked plugins without evaluating nodes during validation, rebuilding after validation, changing generated
execution order or bindings, or reading the source definition at runtime. A generated binary MUST NOT
recognize built-in kind names to perform inference.

#### Scenario: Match resolved ports across execution paths

- **WHEN** a constant feeds an identity node and both in-memory execution and a generated binary run the workflow
- **THEN** both enforce the same inferred output types and produce the same selected JSON value

#### Scenario: Preserve a previously installed executable

- **WHEN** a changed constant creates a known type conflict in a reused build directory
- **THEN** validation rejects the new runner and leaves the previously installed executable intact

### Requirement: Describe a compiled workflow without executing nodes

Generated runners SHALL support `--describe` and return one date-versioned JSON graph description containing workflow
identity, node IDs and kinds, named data edges, control edges, and deterministic execution order. New runners SHALL return
the graph record from their embedded manifest without requiring original build inputs or invoking plugin factories.
Unconnected or dynamic port metadata MUST NOT be inferred from graph edges. Description output MUST exclude embedded
node configuration and business values.

#### Scenario: Describe a standalone binary

- **WHEN** a generated runner is invoked with `--describe` after its build inputs are removed
- **THEN** it returns its supported graph description without executing any node operation

#### Scenario: Describe a graph with configuration-dependent ports

- **WHEN** a compiled workflow contains two instances of a conditional node with different branch names
- **THEN** its description contains the configured data/control relationships without claiming to enumerate unconnected dynamic ports or including predicate configuration

#### Scenario: Avoid construction diagnostics

- **WHEN** a linked plugin factory normally prints diagnostics during validation
- **THEN** description mode does not invoke that factory and its stdout remains one JSON document

### Requirement: Require complete isolated description output before execution

Description mode SHALL dispatch before registry preparation and write one complete JSON document to stdout. For legacy
executables without manifests, CLI command-based inspection SHALL accept output only after successful process exit and
parsing exactly one supported document within bounded size/time limits. Invalid or incomplete metadata MUST prevent
execution. New manifest-bearing executables SHALL be inspected directly without starting a description process.
Both paths MUST retain the existing single-build validation/install workflow without a second runner compilation.

#### Scenario: Reject unexpected startup output

- **WHEN** linked native startup code in a legacy runner writes to stdout before dispatching command-based description mode
- **THEN** unexpected bytes cause preflight failure rather than heuristic recovery

#### Scenario: Return partial metadata

- **WHEN** legacy description output is truncated, oversized, timed out, or accompanied by an unsuccessful exit
- **THEN** the CLI reports preflight failure and does not start workflow execution

#### Scenario: Preserve one runner build

- **WHEN** a workflow is compiled, validated, described, and installed
- **THEN** manifest generation, validation, description, and installation use the same built runner without recompiling to embed metadata

#### Scenario: Avoid native startup during manifest inspection

- **WHEN** a new runner contains native startup code that writes diagnostics or performs initialization
- **THEN** direct manifest inspection does not start the runner or invoke that startup code

### Requirement: Generate standalone observable runners

Generated runners SHALL include workflow observation boundaries and configurable OTel export initialization with bounded
shutdown. They MUST preserve statically generated orchestration, selected-output JSON, validation behavior, and
operation without the CLI, source definition, lockfile, plugin sources, Rust toolchain, or telemetry receiver.
Support-package resolution SHALL include matching observation support without including terminal UI dependencies.
Validation and description modes MUST NOT be interpreted as workflow execution runs.

#### Scenario: Run with a Collector

- **WHEN** an independently launched runner is configured with an OTLP/HTTP Collector endpoint
- **THEN** it exports workflow observations without requiring a terminal interface or running CLI process

#### Scenario: Preserve plain execution

- **WHEN** the same runner is launched normally without export configuration
- **THEN** it follows the existing generated execution plan and returns the same selected output JSON and workflow exit behavior

#### Scenario: Validate with inherited export settings

- **WHEN** compilation invokes the generated runner with `--validate` in an environment containing export settings
- **THEN** validation remains free of node execution and does not emit a workflow execution run

### Requirement: Compile structured Loop definitions into executable binaries

The compiler SHALL encode each validated Loop body as scope-local generated execution steps with
its planned order and port bindings. A generated Loop runner SHALL match in-memory execution for
selected outputs and remain runnable without the source definition, dependency lock, Cargo, or
plugin sources. Definitions in `2026-09-26` SHALL retain their existing DAG behavior.

#### Scenario: Compile a valid Loop workflow

- **WHEN** a user compiles a valid structured Loop definition that references registered ordinary node kinds
- **THEN** the system generates and compiles a runner with the planned scope-local order

#### Scenario: Run a compiled Loop workflow

- **WHEN** a user runs a generated binary containing a Loop
- **THEN** the binary repeats its body according to the Loop contract and returns the same selected outputs as in-memory execution

#### Scenario: Run a Loop binary without build inputs

- **WHEN** a generated Loop executable is moved to a compatible runtime environment without its project files, plugin sources, or Rust build tools
- **THEN** it executes its embedded Loop workflow and produces the selected outputs

### Requirement: Validate every Loop scope before code generation

Before generating a runner, the CLI SHALL validate nonblank unique node IDs within each scope,
existing local edge endpoints, selected output node references and names, scope boundaries, reserved
engine controls, Loop count and nesting limits, and acyclicity within every graph. Before
installing the executable, its validation mode SHALL require a registered Loop declaration and
validate ordinary registered kinds, configuration, existing ports, required input connections, typed Loop variables, assignments,
termination conditions, and context references in every scope. Validation MUST NOT execute node
implementations. A failed validation MUST preserve any previously installed output binary.

#### Scenario: Reject a body cycle before code generation

- **WHEN** a Loop body has a data or control cycle
- **THEN** structural planning fails with a cycle path in that body before generating the runner

#### Scenario: Reject an invalid body plugin before installation

- **WHEN** a body node has an unknown kind or invalid configuration
- **THEN** runner validation fails before installation without running that node

#### Scenario: Preserve an existing binary after invalid Loop edit

- **WHEN** a previously valid Loop definition is edited into an invalid one and rebuilt
- **THEN** the build fails while preserving the installed executable and adjacent dependency lock

### Requirement: Validate and generate iteration bodies

The compiler SHALL plan and validate an Iteration body using the enclosing workflow's linked node packages. Runner
validation MUST require a linked `builtin.iteration` registration. Validation MUST construct and check all body nodes
and result ports even when the input array is empty. A generated runner SHALL prepare body nodes once and execute
statically generated body calls per item inside a fresh child context. In-memory and generated execution MUST use the
same input, output, skip, error, and type-checking semantics. The runner MUST remain standalone after installation.

#### Scenario: Match generated and in-memory execution

- **WHEN** the same Iteration workflow runs in memory and as an installed binary
- **THEN** both return the same ordered result array or an equivalent indexed error

#### Scenario: Reject an invalid body before installation

- **WHEN** a body node has invalid configuration or a missing required input
- **THEN** validation rejects the new runner and preserves an existing installed executable

### Requirement: Validate activation and message boundaries before installation

Streaming validation SHALL recognize initial tasks and stream producers without requiring a path from an
engine input node. It SHALL bind their declared inputs through the workflow interface and validate
startup dependencies. It MUST reject initial event executors, mixed-domain dependencies, cross-domain context
references, and mixed-domain selected outputs, including inactive branches. Graph, configuration, resource,
and type checks MUST run without executing nodes or reading input data.

#### Scenario: Activate a configured source per message

- **WHEN** a configured constant has a control edge from an explicit source's `item` output
- **THEN** it is activated within each emitted frame and its output stays in that message domain

#### Scenario: Start an unattached task once

- **WHEN** a streaming graph contains an initial task with valid startup bindings
- **THEN** it executes once in the startup frame without an external trigger

#### Scenario: Reject an unattached root

- **WHEN** an unattached root has an event executor whose contract defines no startup callback
- **THEN** validation requires an explicit activation dependency instead of inventing an input or timer event

#### Scenario: Reject a control edge across a batch boundary

- **WHEN** a batch-domain node is also gated by an upstream item-domain control output
- **THEN** validation reports the incompatible domains even if the value types otherwise fit

#### Scenario: Join initial task outputs

- **WHEN** two initial tasks feed an ordinary consumer in the startup frame
- **THEN** validation accepts the shared frame and preserves deterministic dependency order

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

### Requirement: Deliver streaming results without waiting for EOF

Each selected stream result SHALL be written and flushed as one JSON line without building a final aggregate.
Slow output MUST apply message-count backpressure, and output failure MUST fail execution. In stream mode,
runner transport MUST reserve result stdout and direct plugin stdout diagnostics to stderr, including
construction diagnostics.

#### Scenario: Deliver a complete batch early

- **WHEN** the first batch finishes while its explicit source remains active
- **THEN** its result line becomes available immediately to the output consumer

#### Scenario: Preserve result framing with a noisy plugin

- **WHEN** a plugin prints diagnostics during construction and execution
- **THEN** stdout still contains only workflow result JSON lines and diagnostics appear on stderr

#### Scenario: Fail a broken output pipe

- **WHEN** the result sink closes before the runner has delivered all selected outputs
- **THEN** the instance fails and the runner exits unsuccessfully

### Requirement: Describe streaming requirements without starting an instance

New streaming graph descriptions SHALL identify execution mode and required observation protocol without exposing
business values or configuration. Their manifest interface record SHALL describe startup parameters and runtime input
resources with matching workflow identity. Direct manifest inspection, `--describe`, `--describe-interface`, and
`--validate` MUST NOT consume source input, invoke executors, arm timers, or emit workflow execution events.

#### Scenario: Validate with an idle open stdin

- **WHEN** a streaming runner is invoked with `--validate` while stdin remains open
- **THEN** validation completes without waiting for input or executing any source or Batch callback

#### Scenario: Inspect a streaming runner

- **WHEN** a standalone runner's graph and startup interface are inspected
- **THEN** the host can determine execution mode, required parameters, resource ownership, and observation compatibility before launching execution

### Requirement: Declare source-driven streaming execution

Schema `2026-10-03` SHALL accept `execution` with `mode: "stream"` and optional resource limits. Startup
parameters SHALL come from initial node ports, and stream item types from explicit source outputs. The engine
input field, global sender, synthetic source, and their special-case validators SHALL be removed. Older
single-run schemas SHALL retain their existing behavior.

#### Scenario: Compile a typed source

- **WHEN** an explicit integer source feeds Batch
- **THEN** validation resolves integer items and list-of-integer batches through ordinary node ports

#### Scenario: Handle a removed field normally

- **WHEN** a definition supplies a field absent from the execution schema
- **THEN** normal schema validation reports the unknown field without selecting a legacy input mode

#### Scenario: Treat node names uniformly

- **WHEN** a user declares a node named `%input` with a registered kind
- **THEN** that node has the same ordinary graph and execution rules as any other user node, with no implicit outputs

#### Scenario: Report an absent endpoint normally

- **WHEN** an edge names `%input` and no such node is declared
- **THEN** normal graph validation reports an unknown endpoint without injecting or specially banning that node

### Requirement: Inspect configured startup interfaces in the installed runner

New runners SHALL retain `--describe-interface`, returning the existing versioned interface JSON with workflow identity,
startup input types/required flags, and resource requirements from the embedded manifest. Both inspection commands MUST
remain factory-free and work without build inputs or a second compilation. Their output MUST agree with direct manifest
inspection; interface discovery MUST NOT infer requirements from built-in kind names.

#### Scenario: Inspect dynamic input metadata

- **WHEN** an external factory defines configuration-dependent root input ports
- **THEN** interface inspection reports the complete effective input contract frozen during compilation without constructing or executing nodes

#### Scenario: Preserve clean output with noisy factories

- **WHEN** a linked factory normally writes diagnostics during preparation
- **THEN** `--describe-interface` does not invoke it and stdout contains one complete interface document

#### Scenario: Preserve one-build standalone inspection

- **WHEN** the validated executable is moved without source, plugin files, or a toolchain
- **THEN** both graph and interface inspection still work using that same executable

### Requirement: Accept startup parameters through shared execution arguments

Runners and `mf run` SHALL accept mutually exclusive `--inputs <JSON>` and `--inputs-file <PATH>`, with
omission meaning an empty object. Inspection/validation modes MUST reject execution arguments. CLI parameter
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

### Requirement: Route only declared input resources

Streaming runners SHALL obtain data through explicit source nodes and SHALL supply only their declared runtime
input resources. Runners MUST reserve result stdout before plugin construction and isolate diagnostics. A
workflow with no stdin source MUST start and complete independently of stdin. EOF SHALL close only its owning
source; result delivery remains incremental and backpressured.

#### Scenario: Execute with null stdin

- **WHEN** an autonomous file producer has valid startup parameters and no stdin resource requirement
- **THEN** the standalone runner executes with null stdin and emits its complete result sequence

#### Scenario: Keep one source active after stdin EOF

- **WHEN** a graph has an explicit stdin source and a separate active producer
- **THEN** stdin EOF drains only that source and does not declare the entire workflow complete

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
- **THEN** the current build regenerates affected layouts and the manifest together and validates the current binary before installation

### Requirement: Preserve ownership metadata parsing errors

The compiler SHALL reject invalid build ownership metadata with a source-bearing parsing error that identifies the `.mf-owner.json` path and retains the original `serde_json::Error` in its error chain.

#### Scenario: Reject a corrupted ownership marker

- **WHEN** a selected build directory contains a `.mf-owner.json` marker that cannot be deserialized
- **THEN** opening the build directory fails before cached artifacts are reused
- **AND** the error identifies the marker path and exposes the original JSON parsing error through `Error::source()`

### Requirement: Embed a versioned workflow manifest

New generated executables SHALL contain one versioned manifest with their graph description and complete startup interface. Their workflow identities MUST match. The manifest MUST be available without source files, sidecars, a symbol table, or executing the runner. It MUST exclude node configuration and supplied business values and bound its combined JSON payload to 16 MiB.

#### Scenario: Inspect a moved executable

- **WHEN** a newly compiled executable is moved without source, plugins, a toolchain, or generated build files
- **THEN** its graph, observation compatibility, startup input declarations, and stdin conditions remain readable from that executable alone

#### Scenario: Preserve metadata exclusions

- **WHEN** a node has secret configuration or a workflow invocation supplies business values
- **THEN** neither the configuration nor supplied values are serialized into the manifest

#### Scenario: Describe a newly compiled older schema

- **WHEN** a supported definition predating startup-input promotion is compiled by the new compiler
- **THEN** its executable has a manifest retaining its existing graph protocol and empty startup interface without introducing promoted inputs

### Requirement: Retain manifests through supported artifact processing

Manifest data SHALL survive supported release optimization, LTO, stripping, and installation on Linux and macOS. Retention MUST also work without telemetry support. Manifest generation and retention MUST NOT require a second compilation or rewriting the linked executable after validation.

#### Scenario: Strip an optimized runner

- **WHEN** a release runner is built with LTO, stripped with supported platform tooling, and installed
- **THEN** its manifest remains readable and agrees with the runner's compatibility inspection output

#### Scenario: Disable telemetry

- **WHEN** a workflow is compiled without telemetry support
- **THEN** its manifest remains embedded and readable with the same startup input contract

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
- **THEN** unused convenience functions are not generated, compilation remains warning-free, and runner execution and inspection commands remain available

#### Scenario: Generate a custom runner

- **WHEN** a caller requests general-purpose artifacts for a custom runner
- **THEN** the existing convenience functions remain available for startup arguments, observation, and runtime options, with the same workflow plan and runtime behavior
