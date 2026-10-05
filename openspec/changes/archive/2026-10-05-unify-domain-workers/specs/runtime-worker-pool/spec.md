# runtime-worker-pool Specification

## MODIFIED Requirements

### Requirement: Execute typed jobs on reusable bounded workers

The runtime SHALL provide a worker pool for owned, sendable jobs and a shared synchronous handler. The worker count SHALL bound the number of worker threads. Each worker SHALL execute at most one job body at a time. While waiting for nested pool work, a worker MAY suspend its current job and help execute ready queued jobs on that same thread. The waiting queue SHALL have the same capacity as the worker count. Callers SHALL own result delivery and task panic handling.

#### Scenario: Reuse a worker for later jobs

- **WHEN** a caller submits more jobs than the worker count over time
- **THEN** existing worker threads process those jobs within the configured concurrency bound

#### Scenario: Help nested jobs without creating threads

- **WHEN** every worker is waiting for child jobs submitted to the same pool
- **THEN** each waiting worker can execute ready child work on its existing thread and all started jobs can complete
- **AND** the pool starts no additional worker thread

#### Scenario: Keep helping within the configured worker bound

- **WHEN** a worker helps execute a nested job while its parent waits
- **THEN** the parent job remains suspended on that thread until the child settles
- **AND** the number of worker threads does not exceed the configured count
