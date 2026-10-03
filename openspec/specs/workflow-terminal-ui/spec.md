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

`mf run <executable> --tui` SHALL obtain a supported runner description and, for runners advertising it, the
matching startup interface. It SHALL validate supplied startup arguments and resource availability, establish
a loopback OTLP/HTTP receiver, and then launch the actual executable with the same arguments and
session-specific observation configuration. It MUST NOT reimplement workflow
execution. The receiver SHALL accept protobuf logs and traces at `/v1/logs` and `/v1/traces` and associate
observations
with the expected workflow and run. Configuration changes MUST be limited to the child environment; inherited
remote
exporter credentials MUST NOT be sent to the local receiver.

Each TUI session SHALL observe only its own launched local child. The interface MUST NOT provide remote or
existing-process attachment, session resumption, or historical replay. A new invocation SHALL create a new
run; missing observations MUST NOT trigger workflow restart.

#### Scenario: Observe an executable without build inputs

- **WHEN** the user invokes TUI mode with a compatible runner but no workflow source, lockfile, plugin
  sources, or Rust toolchain
- **THEN** the CLI loads its description and displays events from execution of that runner

#### Scenario: Prevent a startup race

- **WHEN** a runner completes immediately after process startup
- **THEN** the receiver was ready before execution began and remains available for final telemetry draining

#### Scenario: Reject an incompatible runner

- **WHEN** the executable lacks description support or returns an unsupported description version
- **THEN** the CLI reports an actionable compatibility error before starting workflow execution

#### Scenario: Isolate a local observation session

- **WHEN** the parent environment contains remote signal endpoints and exporter credentials
- **THEN** the launched session sends its telemetry to the local receiver without those credentials and leaves
  the parent environment unchanged

#### Scenario: Start a fresh session

- **WHEN** the user invokes TUI mode after a previous session ended
- **THEN** the CLI starts a new local run without attaching to a prior process or requesting its event history

#### Scenario: Reject parameters before executing any branch

- **WHEN** a supplied startup argument is missing, unknown, or incompatible with the inspected interface
- **THEN** the CLI reports the affected node/port before launching workflow execution

#### Scenario: Preserve legacy finite runners

- **WHEN** a supported older single-run binary does not advertise interface inspection and no startup
  parameters are supplied
- **THEN** existing preflight, terminal launch, observation, and cleanup continue to work

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

On supported Linux and macOS targets, the CLI SHALL supervise the launched child in a separate process group,
drain both
output streams independently of rendering, and use cancellation-aware stream cleanup with finite deadlines.
Stdout SHALL
be preserved as raw bytes in bounded private temporary storage until terminal restoration. Storage or delivery
failure
MUST NOT silently claim complete workflow output; it SHALL produce a CLI capture/output error and bounded
cleanup.
Diagnostic tail eviction SHALL remain nonfatal and visibly counted. The TUI SHALL own terminal input. The
child SHALL receive null stdin when no stdin resource is required, or the explicitly selected stream-input
file when its interface declares that resource.

#### Scenario: Exceed stdout storage budget

- **WHEN** child stdout exceeds the capture budget or writing the spool fails
- **THEN** the CLI reports output capture failure, terminates the child with bounded cleanup, and labels any
  delivered prefix incomplete

#### Scenario: Descendant keeps a pipe open

- **WHEN** the direct child exits while a descendant retains an output pipe
- **THEN** stream cleanup stops waiting at its deadline and reports incomplete capture rather than hanging
  indefinitely

#### Scenario: Ignore an interrupt

- **WHEN** a running child ignores the initial user interrupt
- **THEN** the CLI escalates process-group termination within its deadline, reaps the child, and restores the
  terminal

#### Scenario: Output delivery fails after successful execution

- **WHEN** the child succeeded but copying its captured stdout to the CLI output fails
- **THEN** the CLI reports an output failure and returns failure while retaining the child's successful
  execution result in diagnostics

### Requirement: Present repeated Loop execution with bounded detail

The terminal UI SHALL display a Loop as a container in its outer graph, show the active pass and
completed pass count, and allow inspection of the current body graph and up to 64 recent pass frames
across the run. It SHALL show the observed stop reason, preserve aggregate counts when older pass
details are evicted, and visibly identify detail truncation. Per-invocation state SHALL be keyed by
run identity, containing stream invocation when present, structured Loop path, and local node ID. A missing
event SHALL not be replaced by a
successful outcome inferred from a later pass, Loop completion, or process success. An early exit
SHALL mark a body suffix NotRun only when an observed pass-finish boundary proves it was unvisited.

#### Scenario: Inspect live refinement

- **WHEN** a Loop is executing its third pass and a body node has started but not finished
- **THEN** the outer graph shows the active Loop and pass, and the body view shows that node Running with
  elapsed time

#### Scenario: Retain bounded history

- **WHEN** a Loop completes more passes than the detail retention limit
- **THEN** the UI retains bounded recent detail, shows aggregate pass counts, and indicates how many older
  passes were evicted

#### Scenario: Keep a missing earlier outcome unknown

- **WHEN** one body completion event is lost and a later pass completes
- **THEN** the earlier invocation remains unknown and observation completeness reflects the missing lifecycle
  data

### Requirement: Launch compatible streaming sources

The terminal launcher SHALL support source-driven streaming protocols. It SHALL use null child stdin for
sources that require none. A declared stdin source SHALL require `--stream-input <PATH>` while the terminal
remains available for keyboard input. Missing, unreadable, unused, or `-` input selections and unsatisfied
host-only resources MUST fail before workflow execution.

#### Scenario: Observe an autonomous source

- **WHEN** a file-reading producer receives valid workflow startup parameters
- **THEN** TUI execution launches it with null stdin and displays producer and downstream activity

#### Scenario: Observe an explicit stdin source

- **WHEN** the workflow declares stdin and the user selects a readable stream-input file
- **THEN** that file supplies the child source while terminal keyboard input remains owned by the TUI

#### Scenario: Reject missing source input

- **WHEN** a workflow requires stdin but no stream-input file was supplied
- **THEN** preflight reports the required option and does not launch an empty stream

#### Scenario: Identify an older streaming runner

- **WHEN** a runner requires an unsupported streaming observation protocol
- **THEN** ordinary compatibility preflight requests recompilation before executing the workflow

### Requirement: Display repeated stream execution with bounded state

Streaming presentation SHALL track invocation identity and show active work, latest established outcomes,
observed counts, batch state, and workflow totals. It SHALL retain at most 64 recent completed invocation
details and bounded active and nested details. Evicting verified completed detail SHALL be reported separately
from lifecycle loss. Later success MUST NOT repair missing earlier outcomes.

#### Scenario: Observe a live source and completed consumers

- **WHEN** a producer remains running while its downstream task completes multiple messages
- **THEN** the source remains Running and the consumers show distinct invocations without treating them as
  conflicting retries

#### Scenario: Bound a long-lived session

- **WHEN** a stream produces more invocations than the retained-history limit
- **THEN** old completed detail is evicted, observed aggregates remain available, and memory does not grow
  with instance age

#### Scenario: Keep body paths distinct across messages

- **WHEN** two messages execute the same Loop body and pass index
- **THEN** their states retain distinct containing stream invocation identities

### Requirement: Bound stream integrity bookkeeping

Stream reduction SHALL compact verified contiguous sequences and retain at most 4,096 recent or out-of-order
record witnesses. Identical retransmissions within the window SHALL be ignored and conflicting records SHALL
invalidate integrity. Older retransmissions MUST NOT be reapplied. Losing unresolved gaps or invocation
evidence to a limit MUST permanently prevent a complete-observation claim.

#### Scenario: Repair a retained gap

- **WHEN** a delayed record closes a gap still inside retained reducer state
- **THEN** the gap clears without regressing any established invocation outcome

#### Scenario: Exceed unresolved-state capacity

- **WHEN** missing events prevent compaction and retained evidence reaches its bound
- **THEN** the UI records observation loss or uncertainty rather than allocating unbounded state or claiming
  complete history

#### Scenario: Receive a record older than retained witnesses

- **WHEN** an ancient sequence is redelivered after its verified detail was compacted
- **THEN** it does not increment counters or alter node state and the UI reports that its old payload
  consistency is outside retained verification coverage

### Requirement: Keep streaming display independent of snapshot capture

The TUI SHALL explicitly disable whole-run value snapshots for streaming children, including inherited capture
settings, while retaining lifecycle display. Its history view SHALL explain that streaming value history is
unavailable. Supported finite workflows SHALL retain their existing snapshot capture and browsing behavior.

#### Scenario: Launch with inherited snapshot settings

- **WHEN** the parent requests snapshot capture and the TUI launches a streaming workflow
- **THEN** the child runs with stream capture disabled and its lifecycle remains observable

#### Scenario: Inspect a finite workflow's values

- **WHEN** a supported finite workflow is launched in TUI mode
- **THEN** its existing value history is captured and browsable
