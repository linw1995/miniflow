## ADDED Requirements

### Requirement: Validate standard semantic payloads

The client SHALL use shared LSP protocol types for standard semantic request parameters, results, and capability declarations. Malformed standard results SHALL produce an incomplete analysis outcome rather than usable semantic evidence. Opaque call-hierarchy `data` and the serialized Flow evidence contract SHALL remain preserved.

#### Scenario: Reject malformed semantic evidence

- **WHEN** a server returns a reference or call-hierarchy item missing required standard fields
- **THEN** analysis reports an analyzer failure gap and closes its owned session

#### Scenario: Preserve extension data across typed calls

- **WHEN** call-hierarchy preparation returns an item containing opaque data
- **THEN** the incoming-call request retains that data unchanged
