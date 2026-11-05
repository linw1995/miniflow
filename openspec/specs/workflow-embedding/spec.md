# workflow-embedding Specification

## Purpose

Define workflow source generation for Rust hosts and the contracts for host-side preparation,
node validation, execution, and portable distribution.

## Requirements

### Requirement: Generate source without linked nodes

The compiler SHALL provide a build helper that parses and checks the entry graph's endpoints, IDs,
and cycles without a registry or linked node crates. It SHALL pack the entry and explicitly listed
source files into a `FlowSourceBundle`, emitting Cargo input tracking and named source modules without
interpreting provider configuration, executing node constructors, business operations, or a nested
Cargo build. The build-product bundle SHALL contain only its entry and resources, not a partial
`ResolvedWorkflow` or selected contracts. Generated `plan(registry)` SHALL resolve that bundle into a
host `ResolvedWorkflow` and pass it to `plan_resolved_definition`. Declared child resolution and complete
structural planning SHALL occur in the host with its selected registry.

#### Scenario: Build without node build dependencies

- **WHEN** a host declares node crates only in normal dependencies and invokes the build helper
- **THEN** its workflow module is generated and compiled with the host program

#### Scenario: Reject invalid graph structure

- **WHEN** a definition is malformed or has invalid endpoints, duplicate IDs, or a cycle
- **THEN** source generation fails with typed sources before replacing modules

### Requirement: Separate CLI discovery from final resolution

The CLI SHALL discover the selected provider dependency closure before final source resolution. Partial
`FlowDiscovery` results SHALL NOT be represented as `ResolvedWorkflow`. Once the closure stabilizes,
the selected provider-linked process SHALL fully resolve the original sources once, preserving lexical
source and dependency paths, selected-provider contracts, and public builtin namespace ownership.
The CLI SHALL freeze the resulting plan before runner compilation; standalone startup SHALL NOT load
or resolve authoring sources. Locked discovery and authoritative-lock validation SHALL retain their
existing behavior.

#### Scenario: Resolve after provider closure convergence

- **WHEN** nested sources add provider packages or features during discovery
- **THEN** final resolution uses the stable selected registrations, not the CLI host inventory
- **AND** relative provider and child-source paths keep their declaring-file directory semantics

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

Modules SHALL expose `plan(&NodeRegistry)` returning their structural `CompiledWorkflow` and derive
runtime manifests from current host metadata. Planning SHALL authorize child fields through
the supplied registry, resolve sources from the packed resources and host builtin Flow registrations,
and reuse shared compiler normalization without constructing nodes. Ordinary configuration SHALL remain
opaque. Inspection SHALL prepare metadata without executing business operations. Embedding SHALL NOT
freeze a build-time startup schema or emit executable manifest sections. Compiled plans and manifest
payload version `2026-11-04` SHALL retain unified selected configuration parameter types for each
reachable kind, including absent optional fields and empty plain-kind metadata. Flow parameters SHALL
be selected by their types, without a separate Flow-name list. Offline traversal SHALL NOT infer roles
from configuration keys or order paths. Saved-plan and generated preparation SHALL reject complete
parameter-type drift before factories. Older payloads and saved plans missing these types SHALL be
rejected without inference or adaptation.

#### Scenario: Use current node metadata

- **WHEN** a node advertises a valid interface different from an earlier invocation
- **THEN** inspection and preparation use the current interface

#### Scenario: Observe before preparing

- **WHEN** a host uses the structural plan to start observation
- **THEN** it can observe preparation and execution with the existing workflow identity

### Requirement: Rebuild inputs and run without sources

Entry and explicitly listed source edits SHALL regenerate source in warm builds. Source paths SHALL
be manifest-relative package-contained files without symlink traversal; relative references SHALL
resolve from the declaring file's embedded directory. The helper SHALL NOT infer resources from
ordinary JSON or collect undeclared files. Missing packed resources SHALL fail with typed errors
without filesystem fallback, even if a matching file exists on disk. Node changes SHALL be normal
dependency rebuilds and affect preparation without build-time node validation. Built programs SHALL
prepare and execute without definitions, generated files, or build tools.

#### Scenario: Edit a definition

- **WHEN** a host rebuilds after editing its definition without cleaning
- **THEN** the module and execution reflect the edit

#### Scenario: Resolve declared children using host-only providers

- **WHEN** only normal dependencies register a custom Flow receiver or builtin Flow
- **THEN** host planning resolves its children using those registrations without build-linked providers
- **AND** unregistered ordinary configuration never gains child-source authority

#### Scenario: Refer to an unpacked source

- **WHEN** an authorized child references a file not listed as an embedded source
- **THEN** host planning fails without reading any matching build-machine or deployment file

#### Scenario: Change a target node

- **WHEN** a normal node dependency changes valid metadata and the host rebuilds
- **THEN** inspection and preparation reflect that metadata

#### Scenario: Remove source and build files

- **WHEN** a built host runs after definitions, generated source, and build files are removed
- **THEN** it uses embedded program data and caller-supplied resources
