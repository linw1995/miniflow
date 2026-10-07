# Spec Delta

## ADDED Requirements

### Requirement: Share struct-defined Iteration inputs across task interfaces

Iteration SHALL expose a typed input struct containing one required shared JSON value named items and use its declaration during preparation. Its existing dynamic task interface SHALL remain supported through runtime-owned decoding. Missing or unknown bindings SHALL retain typed runtime decode errors. Supplied scalar items SHALL retain the array-or-object domain error. Body-dependent output declarations and iteration policies SHALL remain unchanged.

#### Scenario: Prepare a typed iteration

- **WHEN** the provider is prepared with a typed body result or continue-on-error policy
- **THEN** its items input is derived as required Any and its results output retains the body-dependent list descriptor

#### Scenario: Preserve dynamic callers

- **WHEN** a caller invokes Iteration through the existing dynamic task interface with valid array or map inputs
- **THEN** runtime conversion supplies the typed struct and iteration preserves ordering and shared child payloads

#### Scenario: Invoke the typed interface

- **WHEN** a caller supplies the typed Iteration input struct directly
- **THEN** its body executes with the same item scopes and result behavior as dynamic invocation

#### Scenario: Reject invalid bindings before the body

- **WHEN** direct dynamic invocation omits items or supplies an undeclared binding
- **THEN** runtime decoding rejects the binding with a typed source and does not invoke the body

#### Scenario: Keep scalar rejection in the provider

- **WHEN** either task interface receives a supplied scalar items value
- **THEN** Iteration reports its array-or-object diagnostic before executing any item
