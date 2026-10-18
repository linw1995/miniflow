## ADDED Requirements

### Requirement: Assert an explicit Flow invariant

The core provider SHALL expose `builtin.assert` as a Task with one boolean condition input and a static configured diagnostic. True SHALL forward the original value and sensitivity through a passed output. False SHALL fail execution without publishing an output or exposing the input value. Invalid inputs and cancellation SHALL remain errors.

#### Scenario: Stop invalid current identity

- **WHEN** a Code predicate produces false for a required execution invariant
- **THEN** the assertion fails with its configured diagnostic and downstream planning does not execute

#### Scenario: Preserve a sensitive successful condition

- **WHEN** a true condition carries secret metadata
- **THEN** the passed output retains its shared identity and metadata
