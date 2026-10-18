## ADDED Requirements

### Requirement: Prove exclusive candidates before execution

Compilation MUST prove every pair of candidates mutually exclusive using declared task output exclusivity and graph activation dependencies. Unknown or unproven exclusivity MUST be a compile error identifying the target input and candidate pair. Programmatic executable construction MUST enforce the same rule. Structural-only planning MAY defer node metadata validation until preparation, but MUST NOT produce an executable without proof.

#### Scenario: Prove router branches

- **WHEN** candidate producers depend transitively on different outputs of the same declared exclusive router
- **THEN** compilation accepts the group without evaluating runtime conditions

#### Scenario: Reject unproven alternatives

- **WHEN** candidates have no branch constraints, share one branch, or depend on unrelated routers
- **THEN** compilation rejects the group before any workflow node executes

#### Scenario: Preserve only common facts after convergence

- **WHEN** an exclusive input joins different branch outputs
- **THEN** its consumer inherits only facts common to every alternative, and cannot claim either individual branch as a downstream proof

#### Scenario: Reject invalid output contracts

- **WHEN** exclusive output metadata contains unknown ports, duplicate ports, a single port, or belongs to a stateful Event or Stream node
- **THEN** preparation rejects the node contract

#### Scenario: Keep facts local to an invocation

- **WHEN** candidate proofs involve a loop body, iteration body, or stream frame
- **THEN** compilation uses facts within that scope and does not propagate per-invocation facts through stateful boundaries
