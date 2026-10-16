## ADDED Requirements

### Requirement: Inspect the original compiled definition through the manifest

Manifest version `2026-10-16` SHALL carry the compiler's existing definition. Runtime root-node lookup SHALL borrow
that definition by exact ID without preparing or executing providers. It SHALL preserve raw configuration expressions
and return `None` for unknown IDs or legacy manifests. Full static configuration is included; invocation values
MUST NOT be materialized. Existing framing, bounds, and graph/interface validation SHALL remain in force.

#### Scenario: Read original node configuration

- **WHEN** a caller looks up a root node whose configuration contains a template and nested values
- **THEN** it receives the original node kind and configuration without resolving the template or executing a factory

#### Scenario: Read a legacy manifest

- **WHEN** a supported `2026-10-07` manifest has no definition
- **THEN** graph and interface inspection succeed and node-definition lookup returns `None`

#### Scenario: Reject incompatible payload versions

- **WHEN** `2026-10-16` omits the definition or `2026-10-07` includes it
- **THEN** both manifest encoding and decoding reject the payload

### Requirement: Reuse one compiled definition for runner inspection

Generated runners SHALL reconstruct their inspection plan from the manifest definition and execution order instead
of embedding a separate complete plan JSON at runtime. Standalone and embedded-host manifests SHALL use the same
compiled definition. Inspection MUST remain independent of providers, original build files, telemetry, and symbols.

#### Scenario: Inspect a stripped standalone runner

- **WHEN** a generated runner is moved away from its build files or built with LTO and stripped
- **THEN** its original node configuration remains readable without executing provider code

#### Scenario: Preserve execution and observation

- **WHEN** task, stream, or nested workflows use the manifest-backed plan with telemetry enabled or disabled
- **THEN** execution results and workflow observation identity remain unchanged
