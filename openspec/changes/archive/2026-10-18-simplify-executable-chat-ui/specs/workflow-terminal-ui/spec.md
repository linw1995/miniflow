## MODIFIED Requirements

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

## ADDED Requirements

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

## REMOVED Requirements

### Requirement: Keep chat work independent of selected session

**Reason**: The terminal is reduced to one conversation execution lane and core recovery actions.

**Migration**: Use the shared storage and usage APIs for application-level management; resolve the current turn before switching sessions.

### Requirement: Display usage from committed call facts

**Reason**: The terminal is reduced to one conversation execution lane and core recovery actions.

**Migration**: Use the shared storage and usage APIs for application-level management; resolve the current turn before switching sessions.
