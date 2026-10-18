## ADDED Requirements

### Requirement: Expose the executing node independently of observation

Task execution contexts SHALL expose the current definition-facing node ID without requiring telemetry.
Nested calls SHALL restore their parent identity after success, error, or unwinding, and forked execution
domains SHALL not inherit an actively executing node. Existing scope paths SHALL identify enclosing loop
invocations. This metadata MUST NOT install an implicit application usage collector.

#### Scenario: Report a generated node invocation

- **WHEN** a generated task requests its executing identity
- **THEN** it receives the same node ID and scope path as in-memory execution

#### Scenario: Restore parent identity after nested failure

- **WHEN** a nested task fails or panics
- **THEN** its parent context retains the parent node identity and the outer caller is restored afterward
