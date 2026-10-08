# Spec Delta

## RENAMED Requirements

- FROM: `### Requirement: Share struct-defined Iteration inputs across task interfaces`
- TO: `### Requirement: Share struct-defined Iteration inputs and outputs across task interfaces`

## MODIFIED Requirements

### Requirement: Share struct-defined Iteration inputs and outputs across task interfaces

Iteration SHALL expose owned inputs containing required shared items and owned outputs containing collected shared results. Preparation SHALL retain body-dependent list types and error policies. Its dynamic task interface SHALL use runtime conversion and remain supported. Missing or unknown bindings SHALL retain typed decode errors; scalar items SHALL retain the array-or-object domain error.

#### Scenario: Prepare a typed iteration

- **WHEN** the provider is prepared with a typed body result or continue-on-error policy
- **THEN** its items input is derived as required Any and its results output retains the body-dependent list descriptor

#### Scenario: Preserve dynamic callers

- **WHEN** a caller invokes Iteration through the existing dynamic task interface with valid array or map inputs
- **THEN** runtime conversion supplies the typed struct and iteration preserves ordering and shared child payloads

#### Scenario: Invoke the typed interface

- **WHEN** a caller supplies the typed Iteration input struct directly
- **THEN** its body executes with the same item scopes and ordering, returning typed collected results whose shared payloads match dynamic invocation

#### Scenario: Reject invalid bindings before the body

- **WHEN** direct dynamic invocation omits items or supplies an undeclared binding
- **THEN** runtime decoding rejects the binding with a typed source and does not invoke the body

#### Scenario: Keep scalar rejection in the provider

- **WHEN** either task interface receives a supplied scalar items value
- **THEN** Iteration reports its array-or-object diagnostic before executing any item
