## ADDED Requirements

### Requirement: Configure native Flow startup inputs from producer context

A native Flow producer SHALL accept explicit startup input configuration using the shared literal, reference, and interpolation mechanisms. Preparation SHALL validate declarations without resolving context or external sources. Execution SHALL resolve templates per producer invocation and retain only the configured native values, never the producer context.

#### Scenario: Publish an immutable configured capability

- **WHEN** a Flow constant resolves input templates against its execution context
- **THEN** the published capability owns those configured values with their types, payload identities, and secret markers
- **AND** its residual signature excludes configured inputs
- **AND** inspecting its properties does not expose configured values or execute the body

#### Scenario: Invoke through either shared boundary

- **WHEN** a configured capability is invoked through a native Flow call or a frozen Router branch
- **THEN** configured and supplied inputs use the same ordinary startup binding and validation
- **AND** configured inputs cannot be overridden through flat or qualified aliases
- **AND** static dispatch keeps its existing compiled body and observation scope

#### Scenario: Resolve independently for each producer execution

- **WHEN** the same prepared producer executes with different context values
- **THEN** its capabilities retain their respective configured values without stale state or cross-call leakage
- **AND** no implicit dependency or ambient context access is introduced

#### Scenario: Reject invalid configuration before body effects

- **WHEN** a configured name, alias, template, literal type, or resolved value is invalid
- **THEN** execution or preparation fails through a typed source chain before the body is invoked
- **AND** diagnostics do not reveal resolved secret payloads
