## ADDED Requirements

### Requirement: Retain snapshot decoding failures until presentation

Snapshot capture SHALL retain typed transport, JSON record decoding, and store failures across repeated admission attempts. History presentation SHALL format the retained error into a diagnostic while leaving the received history available.

#### Scenario: Admit a malformed snapshot record repeatedly

- **WHEN** a snapshot envelope contains an invalid record and another record is subsequently admitted
- **THEN** both admission failures retain the original typed JSON decoding cause and the history view displays the failure
