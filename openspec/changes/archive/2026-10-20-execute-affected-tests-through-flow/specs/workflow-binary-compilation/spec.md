## ADDED Requirements

### Requirement: Forward standalone execution interrupts to cancellation

Generated Unix runners SHALL translate SIGINT and SIGTERM into existing execution cancellation during finite, streaming, and selected-Task execution. Signal capture SHALL begin before node execution and remain bound to the current run. Embedded runtimes SHALL not install process-wide handlers automatically. Nodes SHALL retain ownership of cooperative cleanup; forced supervisor termination remains a separate boundary.

#### Scenario: Interrupt an executing process node

- **WHEN** a generated runner receives SIGINT or SIGTERM while an owned process Task is active
- **THEN** cancellation reaches the Task so its process group can be stopped before the runner exits unsuccessfully

#### Scenario: Interrupt startup

- **WHEN** a signal arrives after registration but before a stream cancellation handle is available
- **THEN** the pending signal is delivered after binding rather than being lost
