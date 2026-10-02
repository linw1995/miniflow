# Spec Delta

## MODIFIED Requirements

### Requirement: Bind prepared bodies through node providers

The registration contract SHALL provide explicit subgraph factories that construct complete executors
from validated bodies, configuration, and execution options. Registered providers SHALL own their
container execution policies. The compiler SHALL prepare and inject bodies through these factories.
Both in-memory Flows and generated direct node calls SHALL implement the same prepared-body contract.
An executor MUST NOT require a later body-binding method to become runnable.

#### Scenario: Bind either execution backend

- **WHEN** the compiler prepares a Loop or Iteration body in memory or generates a standalone runner
- **THEN** the registered factory receives a prepared body with the same selected output types and constructs the node that owns its invocation policy
