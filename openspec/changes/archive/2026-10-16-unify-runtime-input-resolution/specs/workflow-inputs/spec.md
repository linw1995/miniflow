## ADDED Requirements

### Requirement: Own startup bindings as invocation state

The execution context SHALL own explicit arguments and its current startup binding through
one input-state abstraction. Replacing explicit arguments or attempting a new binding MUST
invalidate the previous binding. A failed binding MUST leave no startup values or stdin
ownership available from that previous binding. Rebinding SHALL resolve sources from the
explicit arguments rather than treating previous resolved values as explicit inputs.

#### Scenario: Invalidate a failed rebind

- **WHEN** a context with a successful binding is rebound with invalid sources or unavailable required stdin
- **THEN** binding fails and no initial node can observe the previous binding

#### Scenario: Preserve domain isolation

- **WHEN** a context forks an execution domain and the parent later replaces its arguments
- **THEN** the child retains its own binding and the parent's previous binding is invalidated

#### Scenario: Keep startup values out of message and body execution

- **WHEN** execution creates a stream or body context, or enters a nested scope
- **THEN** top-level startup values are not injected into its nodes
