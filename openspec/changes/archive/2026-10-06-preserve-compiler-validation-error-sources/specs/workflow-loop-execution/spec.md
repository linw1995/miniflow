## ADDED Requirements

### Requirement: Preserve typed Loop preparation sources

Loop preparation failures SHALL retain the original typed configuration or graph error and the complete Loop scope path. Domain-only failures SHALL remain domain errors without an invented decoding source.

#### Scenario: Preserve assignment decoding failure

- **WHEN** an assignment configuration is missing its required variable field
- **THEN** planning retains the `serde_json::Error` source and identifies the enclosing Loop and assignment node

#### Scenario: Preserve body graph failure

- **WHEN** a Loop body edge references an unknown node
- **THEN** planning retains the inner `WorkflowCompileError` and the enclosing Loop path

#### Scenario: Reject a blank assignment target

- **WHEN** an assignment variable decodes successfully but is blank
- **THEN** planning reports a Loop domain error instead of a JSON decoding error
