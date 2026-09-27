# Spec Delta

## ADDED Requirements

### Requirement: Describe a compiled workflow without executing nodes

Generated runners SHALL support `--describe` and return one date-versioned JSON graph description containing workflow
identity, node IDs and kinds, named data edges, control edges, and deterministic execution order. Description SHALL use
the embedded compiled plan without requiring original build inputs or invoking plugin factories. Unconnected or dynamic
port metadata unavailable from the embedded graph MUST NOT be inferred from its edges. Description output MUST exclude
embedded node configuration and business values.

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

Description mode SHALL dispatch before registry preparation and write one complete JSON document to stdout. The CLI
SHALL accept the description only after successful process exit and parsing exactly one complete supported document
within bounded size/time limits. Invalid or incomplete graph metadata MUST prevent execution. Description mode MUST
retain the existing single-build validation/install workflow and MUST NOT require a second runner compilation.

#### Scenario: Reject unexpected startup output

- **WHEN** linked native startup code writes to stdout before the runner dispatches description mode
- **THEN** unexpected bytes cause preflight failure rather than heuristic recovery

#### Scenario: Return partial metadata

- **WHEN** description output is truncated, oversized, timed out, or accompanied by an unsuccessful exit
- **THEN** the CLI reports preflight failure and does not start workflow execution

#### Scenario: Preserve one runner build

- **WHEN** a workflow is compiled, validated, described, and installed
- **THEN** validation, description, and installation use the same built runner without recompiling to embed metadata

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
