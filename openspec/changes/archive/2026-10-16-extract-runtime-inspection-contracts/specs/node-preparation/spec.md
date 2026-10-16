## ADDED Requirements

### Requirement: Collect author-selected inspection configuration

Prepared metadata SHALL support public configuration selected by node authors, who MUST exclude private values.
Task flows and stream plans SHALL collect nonempty maps under exact root node IDs without executing nodes,
inferring provider fields, or recursively publishing nested-body configuration.

#### Scenario: Collect selected fields

- **WHEN** a root node publishes configuration and another root node publishes none
- **THEN** collection preserves the selected fields and exact ID while omitting the empty node entry

### Requirement: Keep inspection configuration outside execution contracts

Public configuration SHALL be serialized through the manifest separately from generated execution metadata.
Changes confined to public configuration MUST NOT cause generated execution metadata mismatch; changes to
execution contracts SHALL remain subject to existing validation.

#### Scenario: Keep inspection changes outside execution drift checks

- **WHEN** only a prepared node's public configuration changes
- **THEN** generated execution metadata comparison succeeds, while a changed output port still fails
