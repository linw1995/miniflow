## ADDED Requirements

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
