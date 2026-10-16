## ADDED Requirements

### Requirement: Resolve explicitly selected execution configuration

Providers MAY enable the shared configuration template facility for execution-only
fields. Templates SHALL be parsed during preparation and resolved on each invocation.
Ports, bindings, output derivations, execution kinds, and subgraph structure MUST NOT
depend on resolved values. Ordinary strings SHALL remain literal.

#### Scenario: Preserve prepared contracts

- **WHEN** a prepared node executes twice with different referenced values
- **THEN** it uses each invocation's values without reconstructing its factory or changing its contracts

### Requirement: Use declared context dependencies

Providers SHALL expose template references in prepared context metadata. The producer
SHALL be an explicit ancestor through ordinary data or control edges. Existing scope
and message-domain restrictions SHALL apply. Missing outputs, skipped values, and
missing selected paths SHALL fail without fallback.

#### Scenario: Reject an undeclared predecessor

- **WHEN** a template names an existing producer without an explicit dependency
- **THEN** compilation rejects the workflow before execution

### Requirement: Preserve types and sensitivity

Whole-value references SHALL preserve JSON types and secret metadata, including
selected JSON Pointer subtrees. Explicit string interpolation SHALL accept only
strings and inherit sensitivity from all selected values. Consumers SHALL validate
resolved field types and domain constraints without including payloads in diagnostics.

#### Scenario: Resolve a secret parameter

- **WHEN** a whole-value reference selects a secret parameter output
- **THEN** business access retains its original value and the result retains its secret marker

#### Scenario: Interpolate secret text

- **WHEN** an interpolated string includes a secret selected value
- **THEN** the entire derived string is secret
