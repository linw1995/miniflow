## ADDED Requirements

### Requirement: Retain snapshot sink failure provenance

The snapshot recorder SHALL accept source-bearing sink errors and retain the original cause until its owner presents diagnostics. After a recording failure, finishing the recorder MUST NOT erase that cause.

#### Scenario: Finish after a sink failure

- **WHEN** a snapshot sink fails with an I/O error during recording and the recorder is subsequently finished
- **THEN** its diagnostic retains the typed I/O cause and error kind
