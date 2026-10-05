# runtime-worker-pool Specification

## Purpose

Provide reusable bounded worker threads for synchronous runtime jobs while leaving message scheduling
and completion policy with the caller.

## Requirements

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

### Requirement: Preserve ownership when submission fails

Nonblocking submission SHALL return the original job when the queue is full or no worker can receive
it. A zero-worker pool SHALL accept no jobs.

#### Scenario: Fill the waiting queue

- **WHEN** all workers are occupied and the waiting queue is full
- **THEN** submission returns the rejected job with a capacity error

#### Scenario: Prepare a graph with no synchronous work

- **WHEN** the caller creates a pool with zero workers
- **THEN** construction succeeds and attempted submission returns the job as disconnected

### Requirement: Close and join owned workers

Dropping a pool SHALL close its submission channel and wait for its worker threads to finish. When
handlers return normally, workers SHALL process accepted jobs before stopping. Thread startup failures
SHALL preserve their I/O source and identify the worker; construction cleanup SHALL join workers that
already started.

#### Scenario: Drop a pool with accepted jobs

- **WHEN** the owner drops the pool while accepted handlers complete normally
- **THEN** the pool waits for those jobs and releases its worker threads before returning
