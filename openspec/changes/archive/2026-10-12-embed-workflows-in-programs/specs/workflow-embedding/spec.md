# Workflow Embedding

## ADDED Requirements

### Requirement: Generate source without linked nodes

The compiler SHALL provide a build helper that reads and structurally plans definitions without a
registry or linked node crates. It SHALL emit Cargo input tracking and named source modules without
executing node constructors, business operations, or a nested Cargo build.

#### Scenario: Build without node build dependencies

- **WHEN** a host declares node crates only in normal dependencies and invokes the build helper
- **THEN** its workflow module is generated and compiled with the host program

#### Scenario: Reject invalid graph structure

- **WHEN** a definition is malformed or has invalid endpoints, duplicate IDs, or a cycle
- **THEN** source generation fails with typed sources before replacing modules

### Requirement: Include isolated modules in Rust hosts

A macro SHALL include named modules without target-side compiler codegen. Renamed compiler/runtime
dependencies SHALL work. Names SHALL be portable Rust identifiers; invalid or duplicate names in a
batch SHALL fail before overwriting earlier artifacts.

#### Scenario: Include overlapping workflows

- **WHEN** a host includes task and streaming workflows with overlapping node IDs
- **THEN** module symbols and output directories remain independent

#### Scenario: Rename dependencies

- **WHEN** a host renames compiler, runtime, and node dependencies
- **THEN** modules prepare and execute with those ordinary dependencies

### Requirement: Validate nodes during host preparation

Modules SHALL use the host registry and existing task/stream preparation APIs to validate node
configuration, inferred ports, and execution contracts before execution. Errors SHALL preserve
typed sources and definition attribution. Preparations SHALL construct independent nodes while
repeated execution of one instance retains its state.

#### Scenario: Reject a node contract during preparation

- **WHEN** a structurally valid definition references an unknown kind, invalid configuration, or incompatible ports
- **THEN** the host builds and preparation fails before business execution

#### Scenario: Reuse and isolate state

- **WHEN** a host executes one stateful instance twice and prepares another
- **THEN** the first retains state and the second starts independently

### Requirement: Preserve host ownership and shared execution

Cargo SHALL select node packages and features. Hosts SHALL link registrations through normal Rust
dependencies without provider mappings. Execution SHALL preserve intermediate outputs,
Loop/Iteration, observation, and host ownership of inputs, streaming resources, cancellation,
and snapshots through existing runtime APIs.

#### Scenario: Inspect typed intermediate outputs

- **WHEN** a host executes typed nodes in its own context
- **THEN** intermediate outputs remain available under the ordinary runtime contract

#### Scenario: Observe preparation failure

- **WHEN** observed task preparation fails with an attributed node construction error
- **THEN** existing semantics record node preparation failure and close the run

### Requirement: Expose plans and current manifests

Modules SHALL expose their structural `CompiledWorkflow` and derive existing-format manifests from
current host metadata. Inspection SHALL prepare metadata without executing business operations.
Embedding SHALL NOT freeze a build-time startup schema or emit executable manifest sections.

#### Scenario: Use current node metadata

- **WHEN** a node advertises a valid interface different from an earlier invocation
- **THEN** inspection and preparation use the current interface

#### Scenario: Observe before preparing

- **WHEN** a host uses the structural plan to start observation
- **THEN** it can observe preparation and execution with the existing workflow identity

### Requirement: Rebuild inputs and run without sources

Definition edits SHALL regenerate source in warm builds. Node changes SHALL be normal dependency
rebuilds and affect preparation without build-time node validation. Built programs SHALL prepare
and execute without definitions, generated files, or build tools.

#### Scenario: Edit a definition

- **WHEN** a host rebuilds after editing its definition without cleaning
- **THEN** the module and execution reflect the edit

#### Scenario: Change a target node

- **WHEN** a normal node dependency changes valid metadata and the host rebuilds
- **THEN** inspection and preparation reflect that metadata

#### Scenario: Remove source and build files

- **WHEN** a built host runs after definitions, generated source, and build files are removed
- **THEN** it uses embedded program data and caller-supplied resources
