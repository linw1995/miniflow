# Spec Delta

## Purpose

Provide basic workflow nodes through one explicitly selected package while preserving stable node kinds and existing data behavior.

## ADDED Requirements

### Requirement: Supply basic nodes through one core package

The project SHALL provide `mfn-core` containing `builtin.constant`, `builtin.identity`, and `builtin.if_else`. One explicit dependency SHALL make all three kinds available. The core package MUST NOT require network clients, database drivers, expression engines, or native service dependencies. The CLI MUST NOT implicitly select this package. The workspace and release support package set SHALL replace `mfn-constant` and `mfn-identity` with `mfn-core`.

#### Scenario: Select the core package once

- **WHEN** a workflow declares `mfn-core` and references all three core kinds
- **THEN** the compiled runner resolves those kinds without extra built-in dependencies or a rebuilt CLI

#### Scenario: Omit the core dependency

- **WHEN** a workflow references `builtin.if_else` without a declared package providing that kind
- **THEN** compilation fails with the existing unknown-kind diagnostic

#### Scenario: Link conflicting providers

- **WHEN** selected dependencies include both `mfn-core` and another package registering `builtin.constant`
- **THEN** validation reports the duplicate kind before executable installation

### Requirement: Preserve existing basic node behavior during consolidation

`builtin.constant` SHALL retain its required `value` configuration and emit that JSON value on output `value`. `builtin.identity` SHALL retain required input `input` and emit the unchanged value on output `value`. Consolidation MUST preserve these kind names, port names, accepted configurations, and JSON value semantics. Migration documentation SHALL explain dependency replacement and adjacent lock regeneration without automatic edits to user definitions.

#### Scenario: Migrate an existing ordinary workflow

- **WHEN** a user replaces the old built-in package declarations with `mfn-core` and regenerates the dependency lock
- **THEN** an existing constant-to-identity graph produces the same selected outputs without changing node kinds or edges

#### Scenario: Preserve null data

- **WHEN** a constant configured with JSON `null` feeds an identity node
- **THEN** the selected identity output contains JSON `null` as a normal value
