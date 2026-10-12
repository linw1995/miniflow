# Workflow Embedding

## ADDED Requirements

### Requirement: Validate embedded workflows during the host build

The compiler SHALL provide a build helper that reads a workflow definition and validates its structure,
provider configuration, inferred ports, and execution contracts using the registry linked into the
host build script. It SHALL reuse existing workflow planning and generated execution validation.
Embedding SHALL NOT spawn a nested Cargo build or execute node business operations.

#### Scenario: Build a valid embedded workflow

- **WHEN** a host build script submits a valid definition with its selected providers linked
- **THEN** the build emits validated immutable execution layouts and a workflow manifest for inclusion

#### Scenario: Reject an invalid provider contract

- **WHEN** an embedded workflow contains an unknown kind, invalid configuration, or incompatible ports
- **THEN** the host build fails with definition/node attribution before producing a usable workflow module

### Requirement: Include named modules in an existing Rust program

A declarative macro SHALL include a named generated workflow module without requiring the compiler's
`codegen` feature in the target dependency. The macro and generated code SHALL support configured
host dependency names. Standalone generation SHALL retain its existing default crate bindings.

#### Scenario: Separate build and execution dependencies

- **WHEN** a host enables compiler code generation in build dependencies and disables it in normal dependencies
- **THEN** its macro-expanded workflow compiles and executes using the generated artifacts

#### Scenario: Use a renamed typed provider

- **WHEN** the host maps a workflow provider alias to a renamed Cargo dependency with typed generation
- **THEN** the module links that dependency and executes it using existing custom-runner fallback rules

### Requirement: Isolate workflows and prepared instances

Each embedding SHALL own a named output directory and module scope. Invalid or duplicate output names
in a request batch SHALL fail before overwriting another workflow's artifacts. One program SHALL support multiple oneshot
and streaming workflows. Prepared instances SHALL have independent provider state while reusing
immutable layouts and the shared runtime's existing execution semantics.

#### Scenario: Include multiple workflows with overlapping node IDs

- **WHEN** a host includes a oneshot workflow and a streaming workflow with overlapping definition IDs
- **THEN** generated files, constants, and preparation symbols do not collide and both workflows execute

#### Scenario: Prepare a stateful workflow twice

- **WHEN** a host prepares two instances of the same embedded workflow
- **THEN** provider state and execution context are independent across those instances

### Requirement: Preserve host ownership of dependencies and resources

The host Cargo manifest and Cargo.lock SHALL select dependency sources, versions, and features.
Workflow dependency descriptors SHALL NOT modify the host's dependency graph. Explicit provider
bindings SHALL anchor selected target crates. Hosts SHALL own inputs, execution options, stdin,
streaming resources, cancellation, snapshots, and observation setup through existing runtime APIs.

#### Scenario: Run without source or build files

- **WHEN** a built host is moved without workflow definitions or generated source artifacts
- **THEN** it prepares and executes embedded workflows without reading those files or invoking build tools

#### Scenario: Supply streaming resources explicitly

- **WHEN** a host starts an embedded streaming workflow with supplied options and stdin resources
- **THEN** execution uses those resources without implicit global stdio or telemetry initialization

### Requirement: Expose and enforce each compiled manifest

Every module SHALL expose its existing-format workflow manifest and enforce the same prepared
startup schema and applicable metadata checks as existing generated execution before business operations.
Embedding SHALL preserve typed construction and validation sources. It SHALL NOT emit additional
standalone executable manifest sections or imply support for inspecting a multi-workflow host as one runner.

#### Scenario: Reject startup contract drift

- **WHEN** target preparation produces a startup interface incompatible with its build-time manifest
- **THEN** preparation fails with a typed contract error before the workflow executes

#### Scenario: Observe one of several embedded workflows

- **WHEN** a host uses a module's manifest with the existing observation APIs
- **THEN** lifecycle events retain that workflow's compiled identity and existing observation semantics

### Requirement: Preserve the custom-runner execution contract

Embedded execution SHALL retain intermediate context outputs and existing observation behavior.
Typed providers SHALL remain supported. The initial embedding profile SHALL preserve the current
custom-runner fallback rather than enabling standard-runner optimizations that elide those outputs.

#### Scenario: Inspect intermediate outputs

- **WHEN** a host executes a workflow with typed providers in its own execution context
- **THEN** intermediate outputs remain available according to the existing general-purpose execution contract

### Requirement: Regenerate artifacts on relevant changes

The helper SHALL emit Cargo tracking for helper-read inputs. Linked provider changes SHALL regenerate
artifacts through ordinary build-script dependency tracking. A warm build SHALL NOT reuse stale
layouts, typed eligibility, or manifests after a relevant definition or provider change.

#### Scenario: Edit a workflow after a successful build

- **WHEN** the workflow definition changes and the host is built again without cleaning
- **THEN** its generated layout, manifest, and resulting behavior reflect the new definition

#### Scenario: Change provider metadata

- **WHEN** a linked build provider changes its advertised ports or typed generation contract
- **THEN** the next host build revalidates the workflow and regenerates or rejects the affected artifacts
