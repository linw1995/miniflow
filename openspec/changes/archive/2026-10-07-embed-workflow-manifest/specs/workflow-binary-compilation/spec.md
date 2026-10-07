## ADDED Requirements

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

## MODIFIED Requirements

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
