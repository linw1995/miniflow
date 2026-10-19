## ADDED Requirements

### Requirement: Freeze elapsed time after local run completion

The TUI SHALL advance elapsed time while a local run is active. Once completion is published, elapsed time SHALL remain fixed at the observed child process exit time throughout final-view inspection, regardless of exit outcome or missing terminal telemetry. Time spent draining output or inspecting the completed run MUST NOT extend its final duration.

#### Scenario: Observe an active run

- **WHEN** a local run is still active
- **THEN** elapsed time continues to advance as the TUI refreshes

#### Scenario: Inspect a run without terminal telemetry

- **WHEN** the child exits without sending terminal telemetry and the user keeps its completed view open
- **THEN** elapsed time remains fixed across subsequent refreshes using the observed process exit time
