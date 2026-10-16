## ADDED Requirements

### Requirement: Share node input resolution across execution paths

Task, generated-task, and event execution SHALL share input assembly and validation.
Startup values and data dependencies SHALL populate ports; control dependencies SHALL
only determine readiness. Routing MUST preserve shared payloads and secret metadata.
Missing dependencies SHALL take precedence over skips. Skipped nodes SHALL bypass input
validation and invocation. Validated typed transfers SHALL not require dynamic payload lookup.

#### Scenario: Match task and event failure behavior

- **WHEN** task and event execution resolve the same inputs with a skipped dependency and a missing dependency
- **THEN** both report the missing dependency with its typed source before any node invocation

#### Scenario: Validate only runnable nodes

- **WHEN** supplied input values have an invalid type but a dependency is explicitly skipped
- **THEN** the node is skipped without type validation, unless a dependency is also missing

#### Scenario: Preserve partial failure snapshots

- **WHEN** task input resolution fails after collecting some input values
- **THEN** the failure snapshot retains those values with existing secret redaction and node attribution

#### Scenario: Preserve direct typed transfers

- **WHEN** a generated task receives an input through a typed slot with validated presence
- **THEN** shared input resolution does not require a corresponding dynamic value or duplicate the payload
