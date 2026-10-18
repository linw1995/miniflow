## MODIFIED Requirements

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

## ADDED Requirements

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
