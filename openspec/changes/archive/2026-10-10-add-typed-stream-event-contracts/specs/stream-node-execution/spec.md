## ADDED Requirements

### Requirement: Adapt typed producer execution through ordinary admission

Typed producers SHALL receive decoded input values and a borrowed callback accepting only their declared typed result.
Runtime conversion SHALL encode every output field before ordinary emission admission, including unobserved fields.
The callback SHALL retain bounded pressure, cancellation, attribution, and incremental delivery semantics. State SHALL
require `Send` without `Sync`, and the callback MUST NOT outlive its invocation.

#### Scenario: Bind typed startup arguments

- **WHEN** an initial typed producer starts with valid workflow arguments
- **THEN** execution receives the associated input struct and emitted values use the reflected output contract

#### Scenario: Reject an unobserved invalid output

- **WHEN** an emitted typed result includes a non-finite field with no downstream observer
- **THEN** encoding fails before admission and retains the producer identity and typed output cause

#### Scenario: Retain incremental failure boundaries

- **WHEN** a typed producer emits valid results and later returns an error
- **THEN** ordinary stream failure and drain rules apply without treating the whole invocation as one atomic event batch
