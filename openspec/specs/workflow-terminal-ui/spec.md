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

All semantic connections SHALL remain inspectable. Routing SHALL use the selected view, where each bundle is one visible connection with inspectable members. Completed routes MUST avoid unrelated boxes, labels, arrows, and attachments. Distinct visible connections MUST NOT share collinear runs or appear joined at incidental contacts; perpendicular crossings SHALL remain distinguishable from connections.

#### Scenario: Separate parallel data and control edges

- **WHEN** data and control connections link the same pair of nodes in Ports view
- **THEN** their attachments and routes remain separately traceable with the existing line-kind distinctions

#### Scenario: Route around an intervening node

- **WHEN** a long visible connection spans multiple layers containing unrelated node boxes
- **THEN** its completed route passes through available space without crossing those boxes or their contents

#### Scenario: Preserve branch labels and arrows

- **WHEN** neighboring routes pass near a control label or a target arrowhead
- **THEN** neither the connection geometry nor its crossing markers overwrite that label or arrowhead

### Requirement: Expand geometry to separate crowded connections

The graph SHALL allocate distinct attachments for incident visible connections and enlarge node boxes or routing
channels when their existing capacity is insufficient. Hidden data mappings SHALL NOT reserve attachments; an aggregate
bundle SHALL reserve one attachment per endpoint. Expansion SHALL preserve the flow direction, keep nodes disjoint, and
update the pannable canvas bounds. Diagram dimensions SHALL remain subject to explicit resource limits.

#### Scenario: Exceed four incident edges

- **WHEN** a node has more than four incoming or outgoing visible connections
- **THEN** its attachment positions do not wrap onto positions occupied by other connections

#### Scenario: Exceed the initial channel capacity

- **WHEN** a layer gap lacks sufficient tracks to separate its visible routes within the supported resource limits
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

For a fixed scope and connection view, geometry SHALL remain stable across selection, observation updates, pass/message changes, panning, and resizing. Equivalent descriptions with reordered edge arrays SHALL produce identical nodes, bundles, routes, and crossing layers. Explicit view switches MAY change geometry; returning SHALL reproduce the established layout. Clipping SHALL match the visible region of the full diagram.

#### Scenario: Change execution state at a crossing

- **WHEN** a pending visible connection becomes produced, skipped, or uncertain
- **THEN** its observation style changes without changing the path, marker shape, or upper/lower assignment

#### Scenario: Pan across a partially visible marker

- **WHEN** a viewport clips part of a crossing marker
- **THEN** its visible cells match the same region of a full-canvas rendering, including style

#### Scenario: Reorder connection descriptions

- **WHEN** equivalent descriptions list their data and control connections in different orders
- **THEN** their node boxes, bundle membership, edge paths, and crossing layer assignments match within the same connection view

### Requirement: Bound routing effort without hiding connections

Routing SHALL bound search work, retained state, retries, and canvas dimensions. An unroutable visible connection SHALL be reported with inspectable endpoints, ports, kind, and bundle members where applicable. Routing MUST NOT silently omit that connection, present unsafe geometry as complete, or alter execution or observation integrity. Intentional filtering SHALL be labelled separately from routing failure.

#### Scenario: Exhaust a routing budget

- **WHEN** a congested fixture exhausts a configured routing limit
- **THEN** routing terminates, the graph reports unresolved visible connections with inspectable identities, and no colliding path is presented as complete

#### Scenario: Inspect an unresolved connection

- **WHEN** the user inspects a node incident to an unresolved connection
- **THEN** its connection details identify the source, destination, connected ports, kind, bundle members where applicable, and routing limitation

### Requirement: Apply routing behavior consistently to described scopes

The root graph and each inspectable Loop body SHALL use the same avoidance, overlap separation, crossing, and routing-limit behavior. Scope navigation SHALL preserve each scope's established geometry independently of the selected execution pass or stream invocation.

#### Scenario: Inspect multiple passes of a body

- **WHEN** the user switches between observed passes of the same Loop body
- **THEN** the body retains its routes and crossing layers while displaying the selected pass's observation state

### Requirement: Compact graph rows without losing connections

The graph SHALL use four-row minimum boxes containing borders, titles, and detail lines. It SHALL grow boxes to keep
distinct visible attachments inside their borders, reduce vertical separation where safe, and align one-to-one visible
connections. Compact layouts MUST preserve node disjointness, visible connection identity, inspectable hidden and
bundled mappings, routing limits, and explicit unresolved-route reporting.

#### Scenario: Display a linear workflow compactly

- **WHEN** a workflow consists of two nodes joined by one visible connection
- **THEN** the title and detail lines remain visible and the connection uses aligned rows without a vertical detour

#### Scenario: Compact crowded incident edges

- **WHEN** a node has several incoming or outgoing visible data/control connections
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

#### Scenario: Reveal a selected node

- **WHEN** selection moves with Tab, Shift-Tab, j, or k, or the graph viewport is resized
- **THEN** the viewport scrolls only as far as needed to reveal the selected node on both axes
- **AND** a viewport smaller than the node aligns with its top-left corner
- **AND** ordinary redraws preserve manual panning

#### Scenario: Follow the latest node event

- **WHEN** a newer admitted node event arrives during finite or streaming execution
- **THEN** the current graph scope selects and reveals that node, or its visible enclosing Loop
- **AND** duplicate, conflicting, and older events do not move focus backwards
- **AND** unrelated scopes and historical Loop passes retain their selection
- **AND** redrawing unchanged observation data preserves manual selection

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

`mf run <executable> --chat [--chat-db <path>]` SHALL imply `--tui` and validate the shared chat contract, terminal-owned
inputs, and terminal requirements before opening storage. Each turn SHALL run the complete supplied executable
with fresh invocation and run identities, using shared storage and recovery APIs. The CLI MUST NOT substitute
an embedded flow, infer model identities from the graph, or override provider configuration.

Without `--chat-db`, storage SHALL default to `~/Library/Application Support/miniflow/chat.sqlite` on macOS
and `$XDG_DATA_HOME/miniflow/chat.sqlite` on Linux, falling back to `~/.local/share/miniflow/chat.sqlite` when
`XDG_DATA_HOME` is unset, empty, or relative. An explicit path SHALL take precedence without requiring
a resolvable user data directory. Default path resolution SHALL NOT create storage before chat validation.
Run help SHALL display the resolved absolute default path without creating storage. If the user data
directory cannot be resolved, help SHALL remain available and request an explicit `--chat-db` path.

#### Scenario: Execute repeated model calls

- **WHEN** a compatible conversation workflow contains a loop with multiple model calls
- **THEN** chat mode executes the entire workflow and persists its reported message metadata and individual usage facts

#### Scenario: Launch chat with default storage

- **WHEN** the user supplies `--chat` without `--tui`
- **THEN** chat mode enables terminal observation and uses the user data directory unless `--chat-db` is supplied

#### Scenario: Missing user data directory

- **WHEN** chat mode has neither `--chat-db` nor a resolvable user data directory
- **THEN** the CLI fails with a diagnostic requesting `--chat-db` before opening storage or starting a runner

#### Scenario: Keep ordinary observation available

- **WHEN** the user runs a compatible chat executable with ordinary `--tui` and its required startup arguments
- **THEN** the CLI retains single-run observation semantics instead of entering chat mode implicitly

#### Scenario: Reserve model context inputs

- **WHEN** startup arguments attempt to override prompt, system, or history owned by chat mode
- **THEN** preflight rejects the conflicting binding before launch

### Requirement: Present persistent sessions and editable prompts

Chat mode SHALL present a full-width conversation, a content-sized Unicode input, contextual shortcuts, and
command/session overlays. It SHALL create sessions on first submission and retain drafts when switching idle
conversations. New names SHALL derive from the first prompt. The terminal SHALL omit rename, archive/restore,
usage-dashboard, and system-instruction forms.

#### Scenario: Open without creating an empty session

- **WHEN** the user opens chat mode and has not submitted a prompt
- **THEN** the editor is ready and no new session record is created

#### Scenario: Start a named conversation

- **WHEN** the user sends the first nonempty prompt in a new chat
- **THEN** the terminal creates a session named from that prompt and starts one execution

#### Scenario: Resume a stored conversation

- **WHEN** the user chooses a saved conversation from the session overlay
- **THEN** its latest bounded message page opens, stored system instructions apply, and the next prompt continues it

#### Scenario: Edit multiline text without accidental submission

- **WHEN** the editor has focus and the user presses Enter
- **THEN** a newline is inserted; Alt+Enter or Esc followed by Enter submits the complete prompt

#### Scenario: Preserve a draft during session switching

- **WHEN** the user switches between idle conversations with unsent edits
- **THEN** each draft is restored when its conversation is selected again

#### Scenario: Read older messages

- **WHEN** the user requests an earlier message page
- **THEN** the terminal loads bounded nonoverlapping history and can return to the latest page

### Requirement: Present distinct retry and cancellation actions

The command menu SHALL distinguish retrying the latest unsuccessful turn from saving or discarding a retained
reply. Retry SHALL allocate fresh linked identities; save-only recovery SHALL reuse the original identities without
executing the workflow. Ctrl+C SHALL cancel the current child group. Ctrl+Q SHALL quit through supervised cleanup.

#### Scenario: Retry the latest failed turn

- **WHEN** the latest turn failed and the user selects Retry failed turn
- **THEN** a new execution reuses its prompt with fresh identities linked to the failed turn

#### Scenario: Avoid offering stale retries

- **WHEN** the latest turn succeeded or the user is browsing an older history page
- **THEN** the menu does not offer an earlier failure as the current retry action

#### Scenario: Recover an unsaved reply

- **WHEN** a decoded reply or raw outcome needs persistence recovery
- **THEN** the menu offers explicit save and discard actions without silently resending the workflow

#### Scenario: Cancel after durable success

- **WHEN** cancellation races with an already committed reply
- **THEN** reconciliation preserves saved success while process and observation facts remain separate

### Requirement: Associate observations with each chat turn

Each launched turn SHALL have its own workflow/run identity and receiver. The terminal SHALL retain only the
current turn's observation and open the existing graph, trace, log, and data views on demand. Starting another turn
or changing conversation SHALL release the previous observation. Missing telemetry MUST NOT alter persisted messages
or trigger another workflow execution.

#### Scenario: Inspect the current run

- **WHEN** the user opens Inspect current run from the command menu
- **THEN** the shared observation UI shows that run and Esc returns to the conversation

#### Scenario: Replace a completed observation

- **WHEN** the user starts another turn or changes conversation
- **THEN** the earlier transient observation is released while saved messages remain available

#### Scenario: Complete without telemetry

- **WHEN** a turn commits but its telemetry is absent or incomplete
- **THEN** the reply remains available and the observation view independently reports uncertainty

#### Scenario: Retain a reader after early teardown

- **WHEN** supervision ends without an exit status and a reader survives
- **THEN** the receiver closes its observation stream and reports any unverified tail

### Requirement: Close the chat application after supervised cleanup

Quitting SHALL stop admission, cancel and reap the current child, reconcile retained results, and join the worker
before releasing database ownership. Normal exit and recoverable display failure SHALL restore the terminal.
Handled turn failures SHALL allow exit status 0 when cleanup succeeds; display or cleanup failures SHALL return 1.

#### Scenario: Quit with a running turn

- **WHEN** the user quits during execution
- **THEN** the child is cancelled and reaped and its invocation is reconciled before ownership is released

#### Scenario: Disconnect the display

- **WHEN** terminal display I/O fails during execution
- **THEN** bounded child cleanup and terminal restoration are attempted without a restoration panic

#### Scenario: Fail storage access during shutdown

- **WHEN** shutdown cannot finalize the invocation because storage is unavailable
- **THEN** close reports a typed failure and reopening can recover the interrupted turn without resending it

### Requirement: Run one chat turn at a time

The terminal SHALL reserve one execution lane before accepting a submission. A running turn or retained reply
SHALL block additional submissions, new chats, and session switching. Extra submissions MUST be rejected instead
of queued. Storage and supervision SHALL run off the terminal rendering thread, with independent cancellation.

#### Scenario: Reject a second submission

- **WHEN** another prompt arrives while admission, execution, or reply recovery is unresolved
- **THEN** the terminal reports busy without creating a session or starting another child

#### Scenario: Keep the selected conversation stable

- **WHEN** the user requests a new chat or session switch during unresolved work
- **THEN** the current conversation remains selected and the UI explains how to cancel or recover it

#### Scenario: Keep editing responsive

- **WHEN** storage or the child workflow is pending
- **THEN** terminal input and rendering remain responsive and edits made after submission are preserved

### Requirement: Select explicit connection views

The graph SHALL offer Execution, Dependencies, and Ports. Execution SHALL reduce transitive node-pair dependencies,
bundle data, and prefer explicit execution connections for each pair. It SHALL separately count hidden mappings and
transitive pairs. Dependencies SHALL retain direct connections and bundles; Ports SHALL retain every mapping.
Visible ordinary/exclusive data, success edges, and output gates SHALL remain distinct. Reduction SHALL preserve inspection and execution semantics.

#### Scenario: Inspect a parameter-heavy node pair

- **WHEN** eighteen ordinary data edges connect the same source and consumer without an explicit execution connection or alternate dependency path
- **THEN** Execution and Dependencies display one counted bundle with all eighteen inspectable mappings, and Ports displays individual mappings

#### Scenario: Keep control and exclusive identities

- **WHEN** one node pair has success edges, control gates, ordinary data bindings, and multiple exclusive-input candidates, with no alternate dependency path
- **THEN** Execution shows the distinct success edges and control gates; Dependencies also shows ordinary/exclusive data bundles, and Details retains each target input and candidate-group identity

#### Scenario: Inspect a data-only workflow

- **WHEN** a scope has data dependencies but no execution connections
- **THEN** Execution preserves dependency reachability with data bundles while leaving independent nodes disconnected

#### Scenario: Hide a transitively redundant connection

- **WHEN** both `A -> B -> C` and `A -> C` are dependencies
- **THEN** Execution hides the direct `A -> C` connection regardless of kind, while Dependencies, Ports, and Details retain it and runtime prerequisites remain unchanged

### Requirement: Inspect bindings independently of visible routes

Selected-node details SHALL retain incoming input bindings, outgoing consumers, source/target ports, and
ordinary/exclusive policy in every connection view. Available observation evidence MAY identify selected sources and
skip causes; incomplete evidence MUST remain unknown rather than inferred from aggregate producer success. Existing
redaction and observation-integrity rules SHALL apply. Selection alone MUST NOT change route membership or trigger
layout recomputation.

#### Scenario: Inspect a hidden exclusive group

- **WHEN** a consumer is selected in Execution view
- **THEN** its details show all exclusive sources and any available selected-source evidence without exposing secret values

#### Scenario: Retain uncertainty

- **WHEN** observations do not establish which candidate supplied an input
- **THEN** the detail view reports unknown selection instead of choosing a producer from its aggregate lifecycle state

### Requirement: Bound presentation state across views

Topology SHALL derive from all described semantic dependencies. Attachment counts, node growth, routes, and route-search
work SHALL derive only from the selected view's visible connections. Layouts SHALL be built lazily and cached under a
shared bounded retained-state allowance across views and scopes. Existing per-layout routing and retry limits SHALL
apply. Hidden mappings SHALL be distinguished from unresolved routes; unresolved bundles SHALL retain every member
identity for inspection.

#### Scenario: Hide a large parameter fan-out

- **WHEN** Execution hides data mappings incident to a node
- **THEN** those mappings do not reserve attachments, enlarge its box, or consume route-search work

#### Scenario: Exhaust routing for an aggregate

- **WHEN** a Dependencies bundle cannot be routed within limits
- **THEN** the view reports an unresolved bundle with its complete member endpoints and does not count intentionally hidden mappings as routing failures

#### Scenario: Switch views across many bodies

- **WHEN** users inspect multiple scopes and connection views
- **THEN** layout-cache retention remains bounded and evicted layouts rebuild deterministically under routing limits

### Requirement: Share connection navigation across graph hosts

Ordinary runs and chat graph inspection SHALL share connection-view navigation and help. The `e` key SHALL cycle views
only when graph navigation owns input, without intercepting chat prompt editing. View switches SHALL preserve the
selected node by identity and restore the saved viewport for each scope/view. State-dependent active-path filtering
SHALL NOT be implied by a connection view.

#### Scenario: Switch connections while inspecting a body

- **WHEN** a user cycles views and later returns to the same body/view
- **THEN** selection and saved viewport are preserved while the established geometry remains deterministic

#### Scenario: Type in chat

- **WHEN** the chat prompt editor owns keyboard input and the user types `e`
- **THEN** the character edits the prompt without changing the graph view

### Requirement: Distinguish execution prerequisites from data mappings

Every view SHALL distinguish node-success edges from output gates and data bindings, including in monochrome and
details. Node-success edges SHALL identify source success without fabricating ports or input values. Details SHALL
retain success prerequisites and typed skip causes. Bundle styles SHALL remain mixed or unknown when member evidence
differs or is incomplete.

#### Scenario: Inspect both execution prerequisites between one node pair

- **WHEN** a pair has both a node-success edge and an output gate
- **THEN** both remain separately identifiable with success and source-output semantics

#### Scenario: Retain a mixed bundle outcome

- **WHEN** members of a data bundle have different or incomplete observations
- **THEN** the bundle does not falsely display uniform success or a single selected source

### Requirement: Inspect static bodies without inventing lifecycle states

When a description exposes an Iteration body, the UI SHALL permit static connection inspection and clearly identify its span-based item observation. It SHALL display unknown item-node states instead of inventing Loop lifecycle outcomes or deriving them from the outer node. An explicitly observed all-skipped input group SHALL be distinguished from missing selection evidence.

#### Scenario: Inspect an Iteration body

- **WHEN** an Iteration body is selected without per-node Loop lifecycle records
- **THEN** the view shows its static dependencies, unknown node states, and an item-span inspection hint

#### Scenario: Inspect an all-skipped group

- **WHEN** the consumer observation explicitly identifies every candidate as skipped without missing or conflicting evidence
- **THEN** details show that no source was selected rather than reporting an unknown winner

### Requirement: Freeze elapsed time after local run completion

The TUI SHALL advance elapsed time while a local run is active. Once completion is published, elapsed time SHALL remain fixed at the observed child process exit time throughout final-view inspection, regardless of exit outcome or missing terminal telemetry. Time spent draining output or inspecting the completed run MUST NOT extend its final duration.

#### Scenario: Observe an active run

- **WHEN** a local run is still active
- **THEN** elapsed time continues to advance as the TUI refreshes

#### Scenario: Inspect a run without terminal telemetry

- **WHEN** the child exits without sending terminal telemetry and the user keeps its completed view open
- **THEN** elapsed time remains fixed across subsequent refreshes using the observed process exit time

### Requirement: Display provisional streamed chat replies

The chat terminal SHALL subscribe before execution and display bounded live answer text before final completion. Records SHALL be consumed independently of render refresh. Separate exported node invocations SHALL remain distinct provisional segments, and unexported internal model output MUST NOT appear as answer text. Stream closure SHALL not imply conversation completion.

#### Scenario: Wait for title generation

- **WHEN** answer increments finish while the user Flow continues generating a title
- **THEN** the terminal keeps the answer visible as provisional and indicates remaining work

#### Scenario: Receive repeated model invocations

- **WHEN** one turn receives multiple answer streams from separate Loop iterations
- **THEN** the terminal distinguishes their provisional segments instead of concatenating unrelated invocations

#### Scenario: Receive an empty stream

- **WHEN** an invoked answer stream opens and closes without increments
- **THEN** the terminal remains responsive and can display the final response when available

### Requirement: Replace previews with authoritative completion state

The terminal SHALL reconcile live text against the final result. Successful persisted text SHALL become the canonical assistant reply. Failed or cancelled partial text SHALL be marked incomplete and excluded from successful history. A complete reply that could not be saved SHALL retain existing save/discard actions without repeating generation. Stale events MUST NOT modify a different turn.

#### Scenario: Display postprocessed final text

- **WHEN** the final selected text differs from its live preview and persistence succeeds
- **THEN** the terminal displays the authoritative saved text as the completed reply

#### Scenario: Cancel after receiving partial text

- **WHEN** a user cancels an active turn after visible increments
- **THEN** the terminal marks that text incomplete and preserves existing cancellation and retry behavior

#### Scenario: Retain a reply after storage failure

- **WHEN** a valid final result exists but its storage transaction fails
- **THEN** the terminal retains the complete unsaved reply and saving it does not invoke the model again

#### Scenario: Receive an event for a finished turn

- **WHEN** a queued event belongs to an earlier invocation than the active turn
- **THEN** it cannot append text to or change the completion state of the active turn

### Requirement: Present model call performance beside replies and LLM nodes

Completed assistant replies and LLM graph cards SHALL present FTT, Gen, and E2E throughput from authoritative call facts. Narrow chat views MAY omit E2E. Previews SHALL show a measuring indicator until final facts arrive. Missing data SHALL remain unknown. Reply summaries SHALL exclude explicitly attributed non-conversation calls, use mean observed FTT, and weight rates by paired tokens and durations.

#### Scenario: Reload a completed reply

- **WHEN** a saved assistant reply is loaded from a bounded message page
- **THEN** its footer uses the original committed call facts and preserves unknown legacy timing

#### Scenario: Recover a complete unsaved reply

- **WHEN** a complete reply cannot be saved
- **THEN** its performance footer retains the original facts during save-only recovery

#### Scenario: Inspect a repeated LLM invocation

- **WHEN** an LLM node runs repeatedly or inside a Loop
- **THEN** its graph card uses only snapshots correlated to the displayed invocation, with stable space reserved for FTT and both rates
- **AND** details explain both rate windows and report per-call facts and observation coverage

#### Scenario: Observe missing call data

- **WHEN** an LLM snapshot or provider token count is unavailable
- **THEN** the card displays unknown values without estimating tokens or borrowing another invocation's data
