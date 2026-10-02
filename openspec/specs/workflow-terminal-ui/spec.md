# workflow-terminal-ui Specification

## Purpose

Let users inspect the graph and live execution state of a local compiled workflow through a terminal interface hosted by the CLI and isolated from standalone runners.

## Requirements

### Requirement: Keep terminal presentation exclusive to the CLI

The system SHALL provide terminal presentation through a separate `mf-tui` crate used by `mf-cli`. Generated runners and compiler/runtime support dependency paths MUST NOT include `mf-tui` or its terminal rendering dependencies. Runners SHALL expose observation data usable without the CLI or terminal interface.

#### Scenario: Inspect a generated runner's dependencies

- **WHEN** a workflow is compiled with telemetry export support
- **THEN** its resolved dependencies include the required observation support and exclude the terminal UI and rendering stack

### Requirement: Launch and observe an existing local executable

`mf run <executable> --tui` SHALL obtain a supported runner description, establish a loopback OTLP/HTTP receiver, and
then launch the actual executable with session-specific observation configuration. It MUST NOT reimplement workflow
execution. The receiver SHALL accept protobuf logs and traces at `/v1/logs` and `/v1/traces` and associate observations
with the expected workflow and run. Configuration changes MUST be limited to the child environment; inherited remote
exporter credentials MUST NOT be sent to the local receiver.

Each TUI session SHALL observe only its own launched local child. The interface MUST NOT provide remote or existing-process attachment, session resumption, or historical replay. A new invocation SHALL create a new run; missing observations MUST NOT trigger workflow restart.

#### Scenario: Observe an executable without build inputs

- **WHEN** the user invokes TUI mode with a compatible runner but no workflow source, lockfile, plugin sources, or Rust toolchain
- **THEN** the CLI loads its description and displays events from execution of that runner

#### Scenario: Prevent a startup race

- **WHEN** a runner completes immediately after process startup
- **THEN** the receiver was ready before execution began and remains available for final telemetry draining

#### Scenario: Reject an incompatible runner

- **WHEN** the executable lacks description support or returns an unsupported description version
- **THEN** the CLI reports an actionable compatibility error before starting workflow execution

#### Scenario: Isolate a local observation session

- **WHEN** the parent environment contains remote signal endpoints and exporter credentials
- **THEN** the launched session sends its telemetry to the local receiver without those credentials and leaves the parent environment unchanged

#### Scenario: Start a fresh session

- **WHEN** the user invokes TUI mode after a previous session ended
- **THEN** the CLI starts a new local run without attaching to a prior process or requesting its event history

### Requirement: Present graph structure and execution state

The interface SHALL display described nodes and distinguish data edges from control edges. Nodes SHALL initially be
Pending. During execution it SHALL show Running, Succeeded, Failed, Skipped, and NotRun as justified by lifecycle
records or terminal execution boundaries, together with available durations, branch port outcomes, and selected-node diagnostics.
Elapsed time for a running node SHALL update before its completion record arrives. Workflow outcome and observation
completeness SHALL be visible separately.

#### Scenario: Show pending and running nodes

- **WHEN** the description is loaded and one node has started a long operation
- **THEN** the graph includes all nodes, the active node shows Running with advancing elapsed time, and unvisited nodes remain Pending

#### Scenario: Explain an inactive branch

- **WHEN** a conditional output is skipped and dependent nodes are skipped
- **THEN** the interface shows the inactive port and causal skip relationships separately from node failures and NotRun states

### Requirement: Display observation loss and uncertainty

State aggregation SHALL deduplicate by run identity and sequence, tolerate reordering, and prevent late start events
from overwriting terminal states. Gaps SHALL be visible while pending and SHALL clear if delayed records close them.
After bounded draining, unresolved gaps, local lifecycle drops, invalid records, and conflicting observations SHALL
visibly prevent a completeness claim. Only a valid final boundary and every lifecycle sequence through that boundary,
with no unresolved visited-node outcome or integrity fault, SHALL establish a complete lifecycle stream.

Missing terminal evidence SHALL be displayed as an unverified tail with unknown total loss. Known gaps and local-drop counts MUST NOT be summed when they overlap. Node states lacking sufficient evidence SHALL remain unknown or visibly last-known, even when workflow success is known. Diagnostic-history truncation and trace availability SHALL be reported separately from lifecycle completeness. Limits on buffering and retained history MUST bound memory use and expose any resulting loss.

#### Scenario: Receive a late start and duplicate finish

- **WHEN** a node finish arrives before its earlier start and transport redelivers the finish record
- **THEN** the node retains one terminal outcome and never regresses to Running

#### Scenario: Close a temporarily visible gap

- **WHEN** sequence 5 arrives before sequence 4 and sequence 4 subsequently arrives during the live session or drain period
- **THEN** the pending gap is cleared without regressing already established terminal states

#### Scenario: Keep missing node outcomes unknown

- **WHEN** a workflow finish arrives but a visited node's terminal event is absent
- **THEN** the interface displays the workflow outcome and missing lifecycle data while leaving that node's outcome unknown

#### Scenario: Expose local loss

- **WHEN** the receiver rejects a matching lifecycle record because it exceeds an admission limit
- **THEN** the interface records a local-drop reason even if the record's missing sequence is not yet inferable

#### Scenario: Separate diagnostic truncation

- **WHEN** old diagnostic bytes are evicted but every lifecycle record is received and applied
- **THEN** the interface reports diagnostic truncation without mislabeling lifecycle delivery as incomplete

#### Scenario: Receive unrelated observations

- **WHEN** an incoming record identifies a different workflow or run from the launched session
- **THEN** it does not mutate the displayed workflow state

### Requirement: Preserve process outcomes and terminal ownership

The CLI SHALL drain child stdout and stderr concurrently without allowing their content to corrupt terminal rendering.
It SHALL preserve workflow stdout for delivery after terminal restoration and retain bounded diagnostic history for
display. The child exit status SHALL determine workflow command success/failure independently of observation
completeness. The receiver SHALL remain available during bounded post-exit draining. The final view SHALL remain
inspectable until closed, after which the CLI restores the terminal and returns the child result when CLI capture and output succeeded. Capture or output-delivery failure SHALL be reported as a separate CLI error while preserving the actual child outcome for display.

#### Scenario: Run a noisy plugin

- **WHEN** a node writes enough output to both standard streams to fill a pipe if unread
- **THEN** execution continues, the terminal remains usable, and child output is handled without unbounded in-memory accumulation

#### Scenario: Exit successfully with incomplete telemetry

- **WHEN** the child exits successfully but its terminal telemetry remains incomplete after draining
- **THEN** the CLI preserves successful process completion and marks unresolved node states as unknown instead of inventing success

#### Scenario: Inspect a completed failure

- **WHEN** the runner fails and the user closes the final view
- **THEN** the terminal is restored, captured stdout is delivered, and the CLI returns a failure result

### Requirement: Handle interruption and unavailable terminals explicitly

The CLI SHALL reject TUI mode without the required interactive terminal before workflow execution. Ctrl-C during execution SHALL request child termination, apply bounded escalation if needed, and restore the terminal on recoverable exit paths. Abrupt child death or disagreement between telemetry and process exit MUST leave unresolved states unknown or interrupted and MUST NOT be represented as ordinary conditional skips or successful node completion.

#### Scenario: Invoke through a noninteractive terminal

- **WHEN** TUI mode is requested without the required terminal capabilities
- **THEN** the CLI reports the problem without starting workflow execution

#### Scenario: Interrupt a running workflow

- **WHEN** the user interrupts while a node is executing
- **THEN** the CLI terminates the launched child within the configured deadline, restores the terminal, and does not mark the unresolved node successful

#### Scenario: Detect process death after an apparent completion event

- **WHEN** telemetry reports success but the child exits unsuccessfully
- **THEN** the CLI reports the unsuccessful process result and exposes the disagreement

### Requirement: Bound child output capture and cleanup

On supported Linux and macOS targets, the CLI SHALL supervise the launched child in a separate process group, drain both
output streams independently of rendering, and use cancellation-aware stream cleanup with finite deadlines. Stdout SHALL
be preserved as raw bytes in bounded private temporary storage until terminal restoration. Storage or delivery failure
MUST NOT silently claim complete workflow output; it SHALL produce a CLI capture/output error and bounded cleanup.
Diagnostic tail eviction SHALL remain nonfatal and visibly counted. The TUI SHALL own terminal input and the child SHALL
receive null stdin.

#### Scenario: Exceed stdout storage budget

- **WHEN** child stdout exceeds the capture budget or writing the spool fails
- **THEN** the CLI reports output capture failure, terminates the child with bounded cleanup, and labels any delivered prefix incomplete

#### Scenario: Descendant keeps a pipe open

- **WHEN** the direct child exits while a descendant retains an output pipe
- **THEN** stream cleanup stops waiting at its deadline and reports incomplete capture rather than hanging indefinitely

#### Scenario: Ignore an interrupt

- **WHEN** a running child ignores the initial user interrupt
- **THEN** the CLI escalates process-group termination within its deadline, reaps the child, and restores the terminal

#### Scenario: Output delivery fails after successful execution

- **WHEN** the child succeeded but copying its captured stdout to the CLI output fails
- **THEN** the CLI reports an output failure and returns failure while retaining the child's successful execution result in diagnostics

### Requirement: Present repeated Loop execution with bounded detail

The terminal UI SHALL display a Loop as a container in its outer graph, show the active pass and
completed pass count, and allow inspection of the current body graph and up to 64 recent pass frames
across the run. It SHALL show the observed stop reason, preserve aggregate counts when older pass
details are evicted, and visibly identify detail truncation. Per-invocation state SHALL be keyed by
run identity, structured Loop path, and local node ID. A missing event SHALL not be replaced by a
successful outcome inferred from a later pass, Loop completion, or process success. An early exit
SHALL mark a body suffix NotRun only when an observed pass-finish boundary proves it was unvisited.

#### Scenario: Inspect live refinement

- **WHEN** a Loop is executing its third pass and a body node has started but not finished
- **THEN** the outer graph shows the active Loop and pass, and the body view shows that node Running with elapsed time

#### Scenario: Retain bounded history

- **WHEN** a Loop completes more passes than the detail retention limit
- **THEN** the UI retains bounded recent detail, shows aggregate pass counts, and indicates how many older passes were evicted

#### Scenario: Keep a missing earlier outcome unknown

- **WHEN** one body completion event is lost and a later pass completes
- **THEN** the earlier invocation remains unknown and observation completeness reflects the missing lifecycle data

### Requirement: Reject unsupported streaming launch before execution

The terminal launcher SHALL reject a runner that requires streaming input or the new streaming observation protocol during description preflight. Its diagnostic MUST explain how to execute the standalone runner with JSON Lines input. It MUST NOT launch the runner with null stdin and present that empty stream as the requested workflow execution.

#### Scenario: Inspect a streaming executable in terminal mode

- **WHEN** `mf run <executable> --tui` obtains a description requiring streaming execution
- **THEN** it reports the unsupported launch mode before starting workflow execution or enabling snapshot capture

#### Scenario: Preserve existing terminal workflows

- **WHEN** the description uses a supported single-run protocol
- **THEN** terminal launch, observation, input ownership, and process cleanup retain their existing behavior
