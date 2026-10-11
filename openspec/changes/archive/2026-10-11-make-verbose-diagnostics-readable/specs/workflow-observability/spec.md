## MODIFIED Requirements

### Requirement: Emit local verbose lifecycle diagnostics

Generated runners and `mf run` SHALL accept `-v` and `--verbose` to print existing lifecycle metadata on stderr.
Diagnostics SHALL preserve result stdout and workflow outcomes without automatically dumping node configuration or
input/output records. They SHALL include node identities, run correlation, outcomes, timing, Loop paths, and streaming
invocation metadata.

#### Scenario: Observe finite and streaming execution

- **WHEN** a task, Loop, or streaming workflow executes verbosely
- **THEN** its existing lifecycle boundaries are printed without changing the selected result protocol

## ADDED Requirements

### Requirement: Render readable local lifecycle summaries

Local verbose diagnostics SHALL render readable event descriptions with elapsed time, node outcomes, available node
durations, and relevant port or failure context. Loop paths SHALL show one-based pass numbers and stream identifiers
SHALL preserve their full unsigned values. The run ID SHALL appear at workflow start. Repeated workflow hashes,
schema versions, and routing attributes SHALL be omitted from local lines.

#### Scenario: Scan successful execution

- **WHEN** a user executes a workflow verbosely
- **THEN** stderr shows workflow and node progress with readable timing and completion summaries instead of attribute
  and body JSON dumps

#### Scenario: Correlate local output with export

- **WHEN** a workflow has both local diagnostics and OTLP export
- **THEN** its start line identifies the same run as the exported records without repeating that ID on every event

### Requirement: Keep local diagnostic events on separate lines

Identifiers and failure messages SHALL escape control characters. The local writer SHALL preserve one complete line
per lifecycle event even when execution is concurrent. This presentation SHALL consume the same validated records
used by OTLP and MUST NOT instrument node execution separately or require export SDK support.

#### Scenario: Display an error containing a newline

- **WHEN** a failed node's identifier or error message contains a newline
- **THEN** the newline is escaped within its diagnostic line and cannot become another apparent event

### Requirement: Remind users about long-running nodes

Verbose execution SHALL print a local reminder every 60 seconds while a node invocation remains active, identifying
the node, elapsed running time, and available Loop or stream context. Reminders SHALL use existing lifecycle records,
work without OTLP SDK support, and MUST NOT create synthetic exported events. Quiet execution SHALL start no worker.

#### Scenario: Observe a long-running node without telemetry

- **WHEN** a telemetry-disabled runner executes verbosely and a node remains active beyond 60 seconds
- **THEN** stderr reports that it is still running and repeats at 60-second intervals until completion

### Requirement: Stop reminders for completed or abandoned invocations

Active entries SHALL remain isolated across runs, Loop passes, and stream invocations. Completion, failure, skip,
workflow termination, or observation-scope cleanup SHALL stop the corresponding reminders.

#### Scenario: Complete one of several active invocations

- **WHEN** one invocation completes while another invocation of the same node is still running
- **THEN** reminders stop for the completed invocation and continue only for the active invocation

#### Scenario: Release an abandoned run

- **WHEN** an observation scope is released before workflow completion
- **THEN** its active nodes are removed and no further reminders are written for that run
