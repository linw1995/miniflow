# flow-node-dependencies Specification

## Purpose

Allow Flow projects to select and lock third-party node dependencies independently of the installed CLI's own dependencies.

## Requirements

### Requirement: Declare node dependencies inside the Flow definition

The system SHALL read node dependencies from a required top-level `dependencies` object in workflow JSON schema
`2026-09-26`. Each dependency SHALL have an alias, a package name, exactly one supported source, and optional feature
selections. Supported sources SHALL be a crates.io version requirement, a Git URL with a full commit revision, or a local
path. Default features SHALL be enabled unless explicitly disabled. Unknown fields and conflicting or incomplete source
declarations MUST fail with JSON-field diagnostics. An explicit empty dependencies object SHALL be valid.

#### Scenario: Declare third-party sources and features

- **WHEN** a Flow declares registry, pinned Git, and local node crates with feature selections in its dependencies object
- **THEN** the system resolves those crates and applies the requested features without a separate dependency manifest or changes to the installed CLI

#### Scenario: Reject an ambiguous dependency

- **WHEN** an entry declares both a local path and a Git source
- **THEN** compilation fails and identifies the dependency field and conflicting sources

#### Scenario: Reject missing dependencies or unknown fields

- **WHEN** a new-schema definition omits dependencies or contains an unknown dependency field
- **THEN** compilation fails with a JSON-field diagnostic before compiling plugins

### Requirement: Locate Flow build inputs deterministically

`mf compile` SHALL use the dependencies embedded in the supplied definition without a separate manifest or manifest
discovery. Local dependency paths SHALL resolve relative to the canonical definition's directory. The dependency lock
SHALL be adjacent to that definition, replacing its last extension with `.lock`, or appending `.lock` when no extension
exists. Compilation MUST NOT rewrite the definition. Definitions with distinct derived lock paths SHALL have independent
dependency resolution, even when stored in the same directory.

#### Scenario: Compile from a different directory

- **WHEN** a user compiles a definition from another working directory and it declares a relative local dependency
- **THEN** that dependency resolves relative to the canonical definition's directory, independently of the working directory

#### Scenario: Build two Flows in one directory

- **WHEN** `order.json` and `refund.json` in one directory declare different node dependencies
- **THEN** each build uses its own declarations and its corresponding `order.lock` or `refund.lock`

#### Scenario: Preserve the user-authored definition

- **WHEN** compilation succeeds and creates or updates the dependency lock
- **THEN** the original JSON definition remains byte-for-byte unchanged

#### Scenario: Invoke through a symlink

- **WHEN** a symlink points to a definition in another directory
- **THEN** compilation uses the target definition's directory for local dependencies and its adjacent lock

### Requirement: Select plugins through dependencies without kind remapping

All declared node dependencies SHALL participate in the build. A dependency SHALL be allowed to register multiple node kinds. Dependency aliases MUST NOT rename or prefix registered kinds. Built-in nodes SHALL require explicit declarations under the same rules as third-party nodes. Duplicate kinds MUST fail before installing the executable.

#### Scenario: Use multiple kinds from one package

- **WHEN** one declared package registers two distinct kinds referenced by the workflow
- **THEN** both kinds are available without additional dependency entries or CLI registration changes

#### Scenario: Reject conflicting registrations

- **WHEN** selected dependencies register the same node kind
- **THEN** compilation fails and identifies the duplicate kind

#### Scenario: Reject an undeclared built-in

- **WHEN** a workflow references a built-in kind without declaring a package that registers it
- **THEN** compilation fails with the node ID and unknown kind

### Requirement: Lock the full build dependency resolution

The system SHALL persist dependency resolution for validation and execution in the per-definition lock file. An unlocked build SHALL
reuse compatible locked dependencies and update resolution only when required. A `--locked` build MUST require an
existing compatible lock, MUST reject resolution changes, and MUST NOT rewrite the lock. Validation and executable
construction MUST use the same resolved package identities and feature selections. Locking MUST NOT be described as
freezing local source contents or guaranteeing identical binary bytes.

#### Scenario: Create and reuse a dependency lock

- **WHEN** a build succeeds without an existing lock and is subsequently repeated with `--locked`
- **THEN** the first build creates the lock and the second uses its dependency resolution without modifying it

#### Scenario: Reject missing or incompatible locked inputs

- **WHEN** `--locked` is requested and the lock is missing or cannot satisfy the embedded dependencies and required support packages
- **THEN** compilation fails without updating the lock or replacing an existing executable

#### Scenario: Keep validation and execution consistent

- **WHEN** a selected feature changes a node's behavior or registration
- **THEN** validation and the generated executable use the same resolved feature selection and package versions

### Requirement: Reject incompatible runtime dependencies

The system MUST reject node dependency graphs containing multiple runtime package identities or a runtime identity incompatible with the build's required runtime. Diagnostics SHALL identify the conflicting versions or sources and the dependency paths introducing them, rather than reporting the resulting missing registrations as ordinary unknown kinds.

#### Scenario: Detect a second runtime identity

- **WHEN** a third-party package resolves a different runtime version or source from the required runtime
- **THEN** compilation fails before plugin-aware validation and identifies the runtime dependency conflict

### Requirement: Protect project state during compilation

The system SHALL prevent concurrent builds sharing a dependency lock from overwriting one another's resolution and SHALL report contention with a retry diagnostic. Build failures before lock persistence MUST preserve the prior lock. Lock writes SHALL be atomic. If final executable installation fails after lock persistence, diagnostics MUST state that the lock was updated. Compilation MUST reject output paths that overwrite the definition or dependency lock, including canonical path aliases.

#### Scenario: Preserve inputs on an invalid output path

- **WHEN** the executable output resolves to the definition or dependency lock path
- **THEN** compilation fails without overwriting that input

#### Scenario: Serialize builds sharing a dependency lock

- **WHEN** another build holds the project build lock
- **THEN** the new build fails with a contention diagnostic and leaves the dependency lock and executable unchanged

#### Scenario: Preserve the lock after a failed plugin build

- **WHEN** dependency resolution succeeds but runner compilation or validation fails
- **THEN** the existing dependency lock remains unchanged and the failed build's resolved project is retained for inspection

#### Scenario: Reject a derived lock collision

- **WHEN** the derived dependency lock path aliases the definition itself
- **THEN** compilation fails before lock persistence and leaves the definition unchanged

#### Scenario: Isolate cached working lock aliases

- **WHEN** the cached working lock is a symbolic link or Unix hard link to the authoritative Flow lock
- **THEN** compilation materializes an independent working file before dependency resolution so a failed attempt cannot mutate the authoritative lock through that alias
