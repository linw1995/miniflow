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

Ordinary single-run observation through `mf run <executable> --tui` SHALL read the supported embedded manifest without
starting an inspection process. Executables lacking a manifest section SHALL be rejected with a recompilation
diagnostic. Corrupt or unsupported manifest data MUST prevent launch without command fallback.

Outside chat mode, the CLI SHALL accept named workflow parameters after its TUI options and reject unexpected positional arguments before
entering terminal mode. It SHALL validate supplied startup arguments, resource availability, and observation
compatibility before launch, establish a loopback OTLP/HTTP receiver, and then launch the actual executable with the
same arguments and session-specific observation configuration. It MUST NOT reimplement workflow execution. The receiver
SHALL accept protobuf logs and traces at `/v1/logs` and `/v1/traces` and associate observations with the expected
workflow and run. Configuration changes MUST be limited to the child environment; inherited remote exporter credentials
MUST NOT be sent to the local receiver.

Each ordinary observation session SHALL observe only its own launched local child. The interface MUST NOT provide remote
or existing-process attachment, observation-session resumption, or telemetry historical replay. Explicit chat mode SHALL
restore persisted chat sessions without attaching to previous processes or replaying old telemetry; each submitted turn
creates a fresh local run. A new invocation SHALL create a new run; missing observations MUST NOT trigger workflow
restart.

#### Scenario: Observe an executable without build inputs

- **WHEN** the user invokes TUI mode with a compatible runner but no workflow source, lockfile, plugin sources, or Rust
  toolchain
- **THEN** the CLI loads its metadata and displays events from execution of that runner

#### Scenario: Prevent a startup race

- **WHEN** a runner completes immediately after process startup
- **THEN** the receiver was ready before execution began and remains available for final telemetry draining

#### Scenario: Reject an incompatible runner

- **WHEN** a runner's manifest uses an unsupported graph or observation version, or the runner has no manifest
- **THEN** the CLI reports an actionable compatibility error before starting workflow execution

#### Scenario: Reject a runner without an embedded manifest

- **WHEN** a supported older executable has no manifest section
- **THEN** preflight reports that recompilation is required without starting an inspection process or workflow execution

#### Scenario: Isolate a local observation session

- **WHEN** the parent environment contains remote signal endpoints and exporter credentials
- **THEN** the launched session sends its telemetry to the local receiver without those credentials and leaves the
  parent environment unchanged

#### Scenario: Start a fresh session

- **WHEN** the user invokes TUI mode after a previous session ended
- **THEN** the CLI starts a new local run without attaching to a prior process or requesting its event history

#### Scenario: Reject parameters before executing any branch

- **WHEN** a supplied startup argument is missing, unknown, or incompatible with the inspected interface
- **THEN** the CLI reports the affected node/port before launching workflow execution

#### Scenario: Preflight a manifest without executing code

- **WHEN** a runner with a supported manifest is inspected before launch
- **THEN** metadata discovery starts no process and invokes neither native startup code nor node factories

#### Scenario: Inspect another supported architecture

- **WHEN** a valid supported-format executable has a manifest but targets an architecture different from the inspection
  host
- **THEN** metadata discovery succeeds without executing it and does not claim the executable can run on that host

#### Scenario: Resolve conditional stdin from the manifest

- **WHEN** a manifest declares stdin ownership unless a particular optional input is supplied
- **THEN** preflight evaluates that condition against the validated arguments and rejects active stdin requirements
  before launching the child

#### Scenario: Observe a source with direct parameters

- **WHEN** the user invokes `mf run workflow --tui --read.path data.txt`
- **THEN** preflight parses the declared string parameter, checks resource ownership, and forwards the canonical flat
  arguments through private transport

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
In ordinary single-run observation mode, it SHALL preserve workflow stdout for delivery after terminal restoration and
retain bounded diagnostic history for display. In ordinary single-run observation mode, the child exit status SHALL
determine workflow command success/failure independently of observation completeness. The receiver SHALL remain
available during bounded post-exit draining. The ordinary single-run final view SHALL remain inspectable until closed,
after which the CLI restores the terminal and returns the child result when CLI capture and output succeeded. Capture or
output-delivery failure SHALL be reported as a separate CLI error while preserving the actual child outcome for display.
Chat mode SHALL consume each turn's structured result for display and storage reconciliation, retain its process outcome
separately, and MUST NOT concatenate turn result objects onto stdout when the interactive session closes.

#### Scenario: Run a noisy plugin

- **WHEN** a node writes enough output to both standard streams to fill a pipe if unread
- **THEN** execution continues, the terminal remains usable, and child output is handled without unbounded in-memory
  accumulation

#### Scenario: Exit successfully with incomplete telemetry

- **WHEN** the child exits successfully but its terminal telemetry remains incomplete after draining
- **THEN** the CLI preserves successful process completion and marks unresolved node states as unknown instead of
  inventing success

#### Scenario: Inspect a completed failure

- **WHEN** the runner fails and the user closes the final view
- **THEN** the terminal is restored, captured stdout is delivered, and the CLI returns a failure result

### Requirement: Handle interruption and unavailable terminals explicitly

The CLI SHALL reject noninteractive TUI launches before execution. Ctrl-C SHALL stop ordinary runs with bounded
escalation and restore the terminal. Chat cancellation SHALL stop the selected turn and keep chat usable; quit SHALL
cancel active work and restore the terminal. Both SHALL use bounded cleanup. Abrupt death or conflicting telemetry
MUST leave unresolved states unknown or interrupted, never skipped or successful.

#### Scenario: Invoke through a noninteractive terminal

- **WHEN** TUI mode is requested without the required terminal capabilities
- **THEN** the CLI reports the problem without starting workflow execution

#### Scenario: Interrupt a running workflow

- **WHEN** the user interrupts while a node is executing
- **THEN** the CLI terminates the launched child within the configured deadline, restores the terminal, and does not
  mark the unresolved node successful

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
child SHALL always receive null stdin; workflows requiring stdin MUST fail preflight.

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

The terminal launcher SHALL support source-driven streaming protocols and always launch children with null
stdin while retaining terminal keyboard input. File producers SHALL receive paths through workflow startup
parameters. Workflows with active stdin requirements MUST fail preflight before workflow execution.

#### Scenario: Observe an autonomous source

- **WHEN** a file-reading producer receives valid workflow startup parameters
- **THEN** TUI execution launches it with null stdin and displays producer and downstream activity

#### Scenario: Observe an explicit stdin source

- **WHEN** the workflow requires stdin after applying its startup arguments
- **THEN** TUI preflight reports unsupported stdin ingestion before executing the workflow

#### Scenario: Reject missing source input

- **WHEN** a readline node omits its file path and therefore requires stdin
- **THEN** TUI preflight rejects execution and reports that the source needs workflow parameters or direct execution

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

The TUI SHALL enable the common snapshot flag for both execution modes, overriding inherited
capture settings. Lifecycle, trace, and log display SHALL remain usable when snapshot capture
is unavailable. Data SHALL report missing capture support or evidence rather than claiming
successful empty capture.

#### Scenario: Launch with inherited snapshot settings

- **WHEN** inherited settings disable snapshots and the TUI launches either execution mode
- **THEN** the child receives MF_CAPTURE_SNAPSHOTS=1 and lifecycle, spans, and logs remain observable

#### Scenario: Inspect a finite workflow's values

- **WHEN** a supported finite workflow is launched in TUI mode
- **THEN** the shared snapshot pipeline supplies its invocation and session Data lists

#### Scenario: Inspect an older compatible streaming runner

- **WHEN** a compatible lifecycle-schema-4 runner lacks shared snapshot support
- **THEN** Details and Logs remain usable and Data reports absent capture evidence without claiming complete empty history

### Requirement: Retain snapshot decoding failures until presentation

Snapshot capture SHALL retain typed transport, JSON record decoding, and store failures across repeated admission attempts. Data presentation SHALL format the retained error into a diagnostic while leaving previously received values available.

#### Scenario: Admit a malformed snapshot record repeatedly

- **WHEN** a snapshot envelope contains an invalid record and another record is subsequently admitted
- **THEN** both admission failures retain the original typed JSON decoding cause and the Data pane displays the failure

### Requirement: Complete cleanup after display disconnection

A TUI display I/O failure SHALL terminate the child, restore terminal input, and return CLI failure status 1. Cleanup MUST NOT panic when display stderr is unavailable, including during cursor restoration after a completed frame.

#### Scenario: Disconnect a rendered display

- **WHEN** the display terminal disconnects after a completed frame while the workflow child is still running
- **THEN** the CLI stops the child, restores the input terminal, and exits with status 1 without a cleanup panic

### Requirement: Bound executable manifest inspection

On supported Linux and macOS executable formats, TUI preflight SHALL inspect regular files with bounded header, table, and manifest reads. It MUST reject ambiguous sections, invalid ranges, length overflow, oversized or incomplete records, unsupported versions, and inconsistent metadata before execution. Malformed or unsupported executables and manifests MUST NOT trigger command fallback or heuristic recovery.

#### Scenario: Inspect a large executable

- **WHEN** a supported executable exceeds the manifest payload budget but contains a valid bounded manifest
- **THEN** preflight reads its manifest without copying the entire executable into unbounded memory or rejecting it solely for executable size

#### Scenario: Reject a forged section range

- **WHEN** an executable declares a manifest range outside the file or an overflowing offset/length pair
- **THEN** inspection reports a bounded parsing failure without starting a process

#### Scenario: Reject malformed manifest framing

- **WHEN** a manifest has invalid magic, an unsupported version, an oversized declared payload, truncated JSON, duplicate JSON members, or unexpected nonzero trailing bytes
- **THEN** inspection fails before launch and does not invoke either inspection command

#### Scenario: Reject ambiguous manifest sections

- **WHEN** a supported executable contains more than one matching manifest section
- **THEN** inspection reports ambiguity without selecting a section or invoking command fallback

#### Scenario: Reject mismatched identities

- **WHEN** an embedded manifest's graph and interface identify different workflows
- **THEN** inspection fails before argument acceptance or process launch

#### Scenario: Reject unsupported executable formats

- **WHEN** the supplied file is not a supported executable container
- **THEN** inspection reports an actionable compatibility error without treating the file as a legacy runner

### Requirement: Capture verbose runner diagnostics in terminal execution

`mf run <executable> --tui --verbose` SHALL enable the runner's local diagnostics and capture them through the existing
bounded stderr history. Terminal presentation SHALL retain its existing stdout delivery, observation receiver,
interruption, and cleanup behavior.

#### Scenario: Inspect verbose terminal execution

- **WHEN** a user runs a compatible workflow with both `--tui` and `--verbose`
- **THEN** the child receives the verbose option and its diagnostic stderr is available in captured output history

### Requirement: Keep routed connections individually traceable

The graph SHALL retain every described data and control connection. Completed routes MUST avoid unrelated node boxes, labels, arrowheads, and occupied attachment cells. Distinct connections MUST NOT share collinear runs or appear joined at incidental contacts. Perpendicular crossings SHALL remain distinguishable from connections.

#### Scenario: Separate parallel data and control edges

- **WHEN** data and control connections link the same pair of nodes
- **THEN** their attachments and routes remain separately traceable with the existing line-kind distinctions

#### Scenario: Route around an intervening node

- **WHEN** a long connection spans multiple layers containing unrelated node boxes
- **THEN** its completed route passes through available space without crossing those boxes or their contents

#### Scenario: Preserve branch labels and arrows

- **WHEN** neighboring routes pass near a control label or a target arrowhead
- **THEN** neither the connection geometry nor its crossing markers overwrite that label or arrowhead

### Requirement: Expand geometry to separate crowded connections

The graph SHALL allocate distinct attachments for incident edges and enlarge node boxes or routing channels when their existing capacity is insufficient. Expansion SHALL preserve the flow direction, keep nodes disjoint, and update the pannable canvas bounds. Diagram dimensions SHALL remain subject to explicit resource limits.

#### Scenario: Exceed four incident edges

- **WHEN** a node has more than four incoming or outgoing connections
- **THEN** its attachment positions do not wrap onto positions occupied by other connections

#### Scenario: Exceed the initial channel capacity

- **WHEN** a layer gap lacks sufficient tracks to separate its routes within the supported resource limits
- **THEN** the gap expands and all completed routes remain inside the updated canvas bounds

### Requirement: Identify upper and lower edges at crossings

Crossings SHALL be readable without color and show a continuous upper edge with a blank cell on
each side of the lower edge. Lines outside the gap SHALL retain their single or double strokes.
Routing SHALL reserve the actual one-row or one-column marker footprint and sufficient clearance
from bends, attachments, labels, arrows, and other markers. Markers MUST NOT imply a junction or
an unexplained broken connection.

#### Scenario: Display both crossing orientations

- **WHEN** fixtures contain a horizontal-over-vertical crossing and a vertical-over-horizontal crossing
- **THEN** each marker visibly identifies the continuous upper connection and interrupted lower connection in monochrome while retaining the data/control line distinctions

#### Scenario: Move a crowded crossing

- **WHEN** a proposed crossing is adjacent to a bend, arrowhead, label, or another crossing marker
- **THEN** routing separates the conflicting features or explicitly reports the connection as unresolved instead of drawing an ambiguous marker

#### Scenario: Reserve only rendered marker cells

- **WHEN** an unrelated route lies beside a crossing but outside its one-row or one-column marker footprint
- **THEN** the marker does not overwrite that route or reserve unused corner cells

### Requirement: Keep graph geometry independent of observations and viewport

For an unchanged described scope, node geometry, edge paths, and crossing layers SHALL remain stable across lifecycle updates, elapsed-time updates, panning, and viewport resizing. Reordering the edge arrays while preserving the same nodes, execution order, and connections SHALL produce the same routed geometry. Clipping SHALL preserve the visible portion of the full diagram.

#### Scenario: Change execution state at a crossing

- **WHEN** a pending edge becomes produced, skipped, or uncertain
- **THEN** its observation style changes without changing the path, marker shape, or upper/lower assignment

#### Scenario: Pan across a partially visible marker

- **WHEN** a viewport clips part of a crossing marker
- **THEN** its visible cells match the same region of a full-canvas rendering, including style

#### Scenario: Reorder connection descriptions

- **WHEN** equivalent descriptions list their data and control connections in different orders
- **THEN** their node boxes, edge paths, and crossing layer assignments match

### Requirement: Bound routing effort without hiding connections

Routing SHALL use explicit limits for search work, retained routing state, retry rounds, and canvas dimensions. If a connection cannot be safely routed within these limits, the UI SHALL visibly report unresolved routing and retain its endpoints, port names, and kind for inspection. It MUST NOT silently omit the connection or change workflow execution or observation integrity because of a routing limitation.

#### Scenario: Exhaust a routing budget

- **WHEN** a congested fixture exhausts a configured routing limit
- **THEN** routing terminates, the graph reports unresolved connections with inspectable identities, and no colliding path is presented as complete

#### Scenario: Inspect an unresolved connection

- **WHEN** the user inspects a node incident to an unresolved connection
- **THEN** its connection details identify the source, destination, connected ports, kind, and routing limitation

### Requirement: Apply routing behavior consistently to described scopes

The root graph and each inspectable Loop body SHALL use the same avoidance, overlap separation, crossing, and routing-limit behavior. Scope navigation SHALL preserve each scope's established geometry independently of the selected execution pass or stream invocation.

#### Scenario: Inspect multiple passes of a body

- **WHEN** the user switches between observed passes of the same Loop body
- **THEN** the body retains its routes and crossing layers while displaying the selected pass's observation state

### Requirement: Compact graph rows without losing connections

The graph SHALL use four-row minimum boxes containing borders, titles, and detail lines. It SHALL
grow boxes to keep distinct attachments inside their borders, reduce vertical separation where safe,
and align one-to-one connections. Compact layouts MUST preserve node disjointness, connection
identity, routing limits, and explicit unresolved-route reporting.

#### Scenario: Display a linear workflow compactly

- **WHEN** a workflow consists of two nodes joined by one connection
- **THEN** the title and detail lines remain visible and the connection uses aligned rows without a vertical detour

#### Scenario: Compact crowded incident edges

- **WHEN** a node has several incoming or outgoing data/control connections
- **THEN** the box grows to contain distinct attachments and completed routes remain independently traceable without overwriting borders, labels, or arrows

#### Scenario: Preserve routes with opaque node names

- **WHEN** downstream node names sort before their producers
- **THEN** the compact layout retains safe routing independent of that lexical ordering

### Requirement: Share routing budgets when widening compact geometry

When compact geometry prevents safe routing, planning MAY try wider attachment separation once
within the remaining scope-wide budgets. It SHALL select the candidate with fewer unresolved
connections and report both attempts' work. Limits MUST NOT reset, and their combined retained
crossing storage MUST remain bounded.

#### Scenario: Recover a congested compact layout

- **WHEN** a mixed data/control graph cannot route all connections at compact attachment spacing
- **THEN** planning may widen spacing within its remaining budgets and select that geometry only if it resolves more connections

#### Scenario: Exhaust the budget before widening

- **WHEN** the compact attempt consumes the scope's geometry or conflict budget
- **THEN** planning retains its safe completed routes and unresolved metadata without resetting limits for another attempt

### Requirement: Identify the node selected for inspection

The graph SHALL visibly identify the node selected for Details in the current root or Loop scope. Focus SHALL remain distinguishable for every displayed lifecycle state while preserving the node's status marker and colors. Selection changes SHALL move the focus indicator and restore the previous node's ordinary presentation without changing graph geometry or connection styling. Graph callers without a selection SHALL retain ordinary rendering.

#### Scenario: Move focus between nodes

- **WHEN** the user changes the selected node with Tab, Shift-Tab, j, or k
- **THEN** the graph highlights the newly selected node and restores the previous node's ordinary presentation

#### Scenario: Focus a running node

- **WHEN** the selected node is Running and another node is also Running
- **THEN** the selected node remains distinguishable while both retain their lifecycle markers and status colors

#### Scenario: Inspect a Loop body

- **WHEN** the user enters a Loop body or changes selection within it
- **THEN** the graph identifies the node selected for Details in that body using the same focus behavior as the root graph

#### Scenario: Pan across a focused title

- **WHEN** the viewport clips part of the selected node's title
- **THEN** its visible focus presentation matches the same region of the full graph rendering

### Requirement: Inspect retained execution spans

The TUI SHALL provide a trace tree grouped by trace and parent span identity, with siblings ordered by start time. It SHALL expose span IDs, timing, status, attributes, events, links, resource, and scope metadata. Selection SHALL remain attached to the same span during reordering. Missing parents and malformed cycles MUST NOT hide retained spans.

#### Scenario: Receive a parent after its child

- **WHEN** a child span is received and selected before its parent
- **THEN** receiving the parent reparents the child without changing the selected span

#### Scenario: Retry an exported span

- **WHEN** the same trace/span identity is exported repeatedly
- **THEN** the trace tree retains one span entry

#### Scenario: Inspect absent trace export

- **WHEN** no spans have arrived during execution or after collection ends
- **THEN** the view distinguishes waiting for export from finished collection without inventing execution spans

### Requirement: Inspect span-related and session logs

The observation view SHALL expose retained LogRecords with timestamps, severity, names, bodies, attributes, trace context, resource, and scope metadata. Span-related logs SHALL match both trace and span IDs. Session-wide browsing SHALL remain available for logs without a retained span. Span events SHALL remain distinguishable from independent LogRecords.

#### Scenario: Receive logs before spans

- **WHEN** correlated LogRecords arrive before their span
- **THEN** they are available in session logs and become visible in the selected span's Logs pane when the span arrives

#### Scenario: Inspect a run-level record

- **WHEN** a matching session record lacks a span ID or its span was not retained
- **THEN** the record remains inspectable in session logs without being assigned to an arbitrary node span

#### Scenario: Ignore unrelated telemetry

- **WHEN** a record identifies a different workflow or run
- **THEN** it does not appear in the current session's observation browser

### Requirement: Inspect decoded node data on its span

The Data pane SHALL display the selected finite node invocation's decoded inputs, outputs, outcome, skipped ports, and available errors. Association SHALL use native trace/span context preserved through snapshot assembly. Repeated names in different Loop passes or Iteration items MUST NOT share data. Missing correlation SHALL produce an explicit diagnostic rather than a guessed association.

#### Scenario: Inspect repeated Loop or Iteration work

- **WHEN** two invocations of the same node emit different snapshots
- **THEN** selecting either span shows only that invocation's values

#### Scenario: Receive data after its span

- **WHEN** snapshot records complete after the selected span was already rendered
- **THEN** its Data pane refreshes to show the received values

#### Scenario: Bound an object preview

- **WHEN** a selected input/output object exceeds the preview limit
- **THEN** the pane marks preview truncation while preserving the received value in the snapshot store

### Requirement: Bound raw telemetry detail independently

Raw span and log retention SHALL have finite count and byte budgets, deduplicate retransmissions, and expose display drops separately from lifecycle integrity and snapshot diagnostics. Reaching a raw detail limit MUST NOT stop lifecycle reduction, snapshot reconstruction, or workflow execution. Terminal labels and previews SHALL have finite sizes and safe control-character handling.

#### Scenario: Reach raw log capacity

- **WHEN** additional matching lifecycle or snapshot records arrive after raw log retention is full
- **THEN** raw detail drops increase while lifecycle reduction and snapshot assembly continue normally

#### Scenario: Preserve completed inspection

- **WHEN** the runner exits and the final view remains open
- **THEN** retained spans, logs, and associated data remain inspectable until the user closes the TUI

### Requirement: Browse shared snapshot history as Data lists

Both execution modes SHALL share session and exact native trace/span Data lists
over reconstructed snapshot history. Rows SHALL identify node, outcome, time, and
sequence; selection SHALL reveal decoded snapshot values. Capture diagnostics and
history SHALL remain inspectable after execution. History retention SHALL be
caller-owned without stream-only eviction or omission policies.

#### Scenario: Inspect an active producer

- **WHEN** an active producer publishes several equal admitted outputs before its span ends
- **THEN** session Data presents distinct reconstructed snapshot entries before span export

#### Scenario: Inspect repeated invocations

- **WHEN** invocations share a node name but have different native trace/span context
- **THEN** selecting a span shows only its own snapshot entries

#### Scenario: Preserve selection

- **WHEN** a user selects an older entry and more snapshots arrive
- **THEN** that entry stays selected until the user resumes following

#### Scenario: Inspect incomplete history

- **WHEN** collection ends without complete reconstruction
- **THEN** the common capture diagnostic and previously reconstructed entries remain inspectable

#### Scenario: Inspect missing context and large values

- **WHEN** a snapshot lacks native context or exceeds preview limits
- **THEN** it remains in session history without guessed association or truncating stored values

### Requirement: Launch chat mode explicitly

`mf run <executable> --tui --chat --chat-db <path>` SHALL validate the shared chat contract, terminal-owned
inputs, and terminal requirements before opening storage. Each turn SHALL run the complete supplied executable
with fresh invocation and run identities, using shared storage and recovery APIs. The CLI MUST NOT substitute
an embedded flow, infer model identities from the graph, or override provider configuration.

#### Scenario: Execute repeated model calls

- **WHEN** a compatible conversation workflow contains a loop with multiple model calls
- **THEN** chat mode executes the entire workflow and persists its reported message metadata and individual usage facts

#### Scenario: Reject chat without terminal mode

- **WHEN** the user supplies `--chat` without `--tui`
- **THEN** CLI validation fails before opening storage or starting a runner

#### Scenario: Keep ordinary observation available

- **WHEN** the user runs a compatible chat executable with ordinary `--tui` and its required startup arguments
- **THEN** the CLI retains single-run observation semantics instead of entering chat mode implicitly

#### Scenario: Reserve model context inputs

- **WHEN** startup arguments attempt to override prompt, system, or history owned by chat mode
- **THEN** preflight rejects the conflicting binding before launch

### Requirement: Present persistent sessions and editable prompts

Chat mode SHALL provide session creation, selection, renaming, archive/restore, bounded history pages, and a Unicode
prompt editor. Archived sessions SHALL remain readable and reject new submissions. Opening an existing database SHALL
restore persisted conversations. Pending deferred title work MUST NOT be presented as generated success.

#### Scenario: Resume a stored conversation

- **WHEN** the user opens chat mode with an existing session database
- **THEN** stored sessions and ordered messages remain selectable and a new submission continues the selected session

#### Scenario: Edit a multiline Unicode prompt

- **WHEN** the user edits and submits a multiline prompt containing Unicode text
- **THEN** the complete prompt is passed once without corrupting terminal rendering or its stored contents

#### Scenario: Archive during execution

- **WHEN** a session is archived after a turn has begun
- **THEN** that turn can finish into the session while later submissions are rejected until restoration

### Requirement: Keep chat work independent of selected session

Chat mode SHALL admit at most one unresolved turn per session and bound total active child execution. Switching
selection MUST NOT cancel, redirect, or drop an admitted invocation. Capacity and session-busy results MUST NOT
implicitly enqueue or resend prompts. Storage commands and process waits MUST NOT block terminal input or rendering.

#### Scenario: Switch while a model is waiting

- **WHEN** session A has an active turn and the user switches to session B
- **THEN** A's result remains bound to A and the terminal can display and operate B independently

#### Scenario: Reach capacity

- **WHEN** the configured active-child limit is reached
- **THEN** another submission receives a visible capacity result without starting a child or creating a turn

#### Scenario: Delay a storage query

- **WHEN** a history or statistics query waits on bounded database contention
- **THEN** terminal rendering and input remain responsive while its result is pending

### Requirement: Present distinct retry and cancellation actions

The interface SHALL distinguish retrying an unsuccessful model turn from saving a retained uncommitted reply. Model
retry SHALL create fresh linked identities; save-only retry SHALL reuse original identities without executing the flow.
Cancellation SHALL terminate the selected process group and reconcile its state before releasing its session
reservation.

#### Scenario: Retry a failed model turn

- **WHEN** the user explicitly retries a failed or interrupted turn
- **THEN** a new launch creates new turn/call IDs with a link to the original unsuccessful turn

#### Scenario: Retry saving a reply

- **WHEN** a valid uncommitted reply is available after child exit
- **THEN** the UI offers save-only retry and abandonment, and does not offer silent model resend as the save action

#### Scenario: Cancel after durable success

- **WHEN** cancellation races with an already committed reply
- **THEN** reconciliation preserves the saved success while reporting any separate process interruption

### Requirement: Associate observations with each chat turn

Each launched turn SHALL have its own workflow/run observation identity and receiver. Chat mode SHALL allow inspection
through the existing graph, trace, log, and data views and bound retained observation detail. Missing telemetry or
evicted observations MUST NOT alter transcript facts, repair unknown node states, or trigger workflow restart.

#### Scenario: Inspect concurrent sessions

- **WHEN** turns in two sessions execute concurrently
- **THEN** selecting either turn shows only its own received observations and does not merge their lifecycle state

#### Scenario: Complete without telemetry

- **WHEN** a chat turn commits but its telemetry is absent or incomplete
- **THEN** its reply remains available while the observation view reports uncertainty independently

#### Scenario: Evict old observation detail

- **WHEN** retained turn observations exceed their configured bound
- **THEN** old transient detail is evicted visibly while persisted message history remains available through pagination

#### Scenario: Retain observations after supervision fails

- **WHEN** supervision ends without an exit status and the UI retains an observation reader
- **THEN** the receiver closes the observation stream, reports any unverified tail, and the completed turn counts
  toward the retention bound without evicting active turns

### Requirement: Display usage from committed call facts

Chat mode SHALL provide session and overall usage queries through `mf-chat`. It SHALL distinguish conversation/title
purposes, known subtotals, reported counters, and missing observations. A query failure MUST NOT change a completed turn
or resend a prompt. Invocations without reported call facts SHALL be displayed separately from actual call attempts.
Deferred title requests MUST NOT count as attempts.

#### Scenario: Display partial usage

- **WHEN** some begun calls lack input or total token counters
- **THEN** the UI marks those subtotals partial or unknown instead of presenting them as complete consumption

#### Scenario: Fail a usage refresh

- **WHEN** a usage query fails after a reply commits
- **THEN** the reply remains saved and the statistics view displays a retryable query error

### Requirement: Close the chat application after supervised cleanup

Quitting SHALL stop admissions, terminate and reap active child groups, reconcile exact invocation states, finalize
recoverable pending completions, and join application workers before releasing database ownership. Terminal restoration
SHALL occur on normal quit and recoverable display failure. Normal quit SHALL return success when shutdown succeeds;
fatal application cleanup failure SHALL return status 1.

#### Scenario: Quit with active sessions

- **WHEN** the user quits while multiple sessions have active children
- **THEN** no child remains supervised as live and ownership is released only after cleanup and finalization are
  attempted

#### Scenario: Disconnect the display

- **WHEN** terminal display I/O fails during a chat turn
- **THEN** bounded child cleanup and terminal restoration are attempted without a restoration panic, and the CLI exits
  with failure

#### Scenario: Quit after a handled turn failure

- **WHEN** a failed turn was reconciled and the user later quits normally
- **THEN** the interactive session exits successfully when application shutdown succeeds and the failed turn remains
  inspectable
