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

Before generating a runner, the CLI SHALL validate nonblank unique node IDs, existing edge endpoints, selected output node references and names, and acyclicity. Before installing the executable, its validation mode SHALL validate registered kinds, configuration, existing ports, required input connections, and at most one connection per input. Validation mode MUST NOT call node execution methods. The normal mode SHALL execute statically generated orchestration.
A concrete output type SHALL connect only to an input of the same type or type `Any`; an output of type `Any` SHALL connect only to an `Any` input.
Validation failures MUST include diagnostics that identify the relevant definition node, port, output, or edge, and MUST NOT produce a successful binary.
The system MUST produce a deterministic topological execution order, using ascending definition ID to break ties between ready nodes.
Port validation SHALL use a node instance's complete configuration-dependent port description when supplied, and otherwise its static registration. Port names MUST be nonempty and unique within each direction. Descriptions MUST depend only on configuration and remain stable between validation and execution. Every node and branch MUST be validated even when it will be skipped during execution.
Graph structure and topological order SHALL include both existing data edges and explicit control edges. Control edges
MUST reference an existing source output and target node, MUST NOT create target input bindings, and MUST NOT contain
duplicate identical entries. Required data-input and type compatibility rules SHALL continue to apply to data edges.
Plugin validation SHALL build a unique index of `${node_id}.${output_name}` for all effective source outputs and check
all declared context references by exact qualified-ID lookup. Qualified-ID collisions MUST fail with both source pairs
before execution, regardless of whether the conflicting outputs could be skipped. Each referenced producer
MUST be a strict ancestor of the consumer through explicit data or control dependencies. Context references MUST NOT
implicitly add dependencies. Unordered, self, and descendant references MUST fail even when a topological tie-break
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

#### Scenario: Reject an incomplete or ambiguous input

- **WHEN** a required input has no connection or an input has more than one connection
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

- **WHEN** a predicate references an existing output from a node outside the consumer's explicit ancestor chain
- **THEN** validation identifies the consumer, branch, and source and requires an explicit dependency instead of adding one

#### Scenario: Reject self or future references

- **WHEN** a predicate references its own node or a downstream node
- **THEN** validation fails before executable installation even if that predicate follows an always-matching branch

#### Scenario: Reject an unknown referenced output

- **WHEN** a predicate references a qualified output ID absent from the effective output index
- **THEN** validation reports the reference and its consumer branch before execution

#### Scenario: Reject ambiguous output identities

- **WHEN** effective output pairs `(a.b, c)` and `(a, b.c)` both form `a.b.c`
- **THEN** validation reports the collision before executable installation without splitting or resolving the key heuristically

#### Scenario: Include control dependencies in structural planning

- **WHEN** data and control edges together form a cycle, or a control edge names an unknown endpoint or duplicates an existing control edge
- **THEN** structural validation fails before runner generation

#### Scenario: Allow transitive source references

- **WHEN** `load_order` precedes `audit`, `audit` precedes `route`, and a route predicate reads `load_order.value`
- **THEN** validation accepts the reference without requiring another direct edge from `load_order` to `route`

#### Scenario: Validate every configured predicate

- **WHEN** a later branch has malformed predicate syntax, an invalid path escape, or an invalid literal for its operator
- **THEN** validation fails with node and branch context even if an earlier branch could match every input

#### Scenario: Validate an unselected branch

- **WHEN** a downstream node on an unselected branch has invalid configuration or a nonexistent port binding
- **THEN** build validation fails without invoking any node execution method

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

Generated binaries SHALL implement the same per-run context publication and reads, control activation, ordered selection,
explicit skip propagation, missing-output errors, and optional selected-output behavior as in-memory workflows. They MUST retain statically generated node order and bindings
and MUST NOT require graph interpretation or special recognition of built-in kind names. Configuration and dependency
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
