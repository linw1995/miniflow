## ADDED Requirements

### Requirement: Collapsible live tool call groups

Chat SHALL group arguments and results by call. Groups SHALL automatically expand when argument streaming starts and collapse when the completed turn replaces the preview with authoritative messages. Authoritative completion SHALL replace previews without duplicates or changes to stored order. Users MAY then independently expand the complete canonical arguments or history with a leading fold marker. Arguments and results MUST remain literal.

#### Scenario: Show interleaved argument fragments

- **WHEN** the model sends interleaved fragments for multiple tool calls
- **THEN** each fragment appears in its expanded group without waiting for a tool result
- **AND** an unambiguous late wire ID preserves the group's focus and streaming expansion
- **AND** live rows have the same message and inter-tool spacing as the corresponding settled batches

#### Scenario: Highlight incomplete JSON arguments

- **WHEN** an expanded group shows raw argument fragments or canonical arguments
- **THEN** the text receives incremental JSON syntax highlighting
- **AND** incomplete or malformed fragments preserve their exact retained text without validation, reformatting, or Markdown interpretation

#### Scenario: Show a long argument as it arrives

- **WHEN** a tool receives more than 8 KiB of argument fragments before its identity is known
- **THEN** the expanded group displays the entire argument received so far, including its prefix
- **AND** a later identity-only hint updates the group without replacing the argument text
- **AND** successful settlement and SQLite reload use canonical arguments, not provisional fragments

#### Scenario: Pair parallel calls and results

- **WHEN** the visible transcript contains a batch with tool results in a different order than the calls
- **THEN** each result appears under its matching call and keyboard navigation follows that display order

#### Scenario: View a result on a page without its call

- **WHEN** a history page contains a tool result but not its matching call
- **THEN** the result remains visible with its call ID and the fold marker before the ID

#### Scenario: Expand and collapse a group

- **WHEN** the user selects a group with j/k or Tab/Shift+Tab in browse mode and presses Space or Ctrl+T
- **THEN** only that group's arguments and result change visibility, without running tools, changing model context, or interpreting tool payloads as Markdown or escape sequences
- **AND** arrow keys continue scrolling and j/k remain ordinary text in the input editor

#### Scenario: Collapse the completed turn

- **WHEN** the completed turn replaces its preview with authoritative messages
- **THEN** the groups collapse without changing stored order or focus
- **AND** a later manual expansion or SQLite reload displays complete canonical arguments
- **AND** the fold state is local to the TUI and is not persisted

#### Scenario: Show observed live tool status

- **WHEN** a tool Flow execution is observed for the active chat invocation
- **THEN** the UI updates the matching tool call group header without waiting for turn settlement
- **AND** if no preview can be matched by ID, the observation is shown in a tool call group without guessed arguments
- **AND** model argument fragments alone do not establish an execution state

#### Scenario: Continue previewing a long turn

- **WHEN** a turn streams many text and tool groups or large fragments while the terminal is not drawing
- **THEN** all received fragments remain available under their respective groups in first-seen order
- **AND** new groups continue to appear until the authoritative result replaces the preview
- **AND** revisiting an evicted body reloads that snapshot's complete prefix without writing it to session history

#### Scenario: Reopen an unfinished tool call

- **WHEN** a process exits after displaying an unfinished call and the user reopens the conversation
- **THEN** the draft is marked unfinished and its collapsed group can expand to the same literal arguments
- **AND** reopening does not request another model response or execute the call

#### Scenario: Browse a large saved tool exchange

- **WHEN** the terminal opens a page containing offscreen messages and collapsed calls
- **THEN** its index contains call identities and turn facts without loading the bodies or arguments into the view
- **AND** expanding or revisiting a group displays the corresponding SQLite content without changing focus or stored order

## MODIFIED Requirements

### Requirement: Present model call performance beside replies and LLM nodes

Completed assistant replies and LLM graph cards SHALL present first-output latency (FTT), Gen, and E2E throughput from authoritative call facts. Narrow chat views MAY omit E2E. Previews SHALL indicate ongoing measurement. Missing data SHALL remain unknown. Reply summaries SHALL exclude explicitly attributed non-conversation calls, average observed FTT, and weight rates by paired tokens and durations. Footers and cards MUST NOT display a call coverage fraction.

#### Scenario: Reload a completed reply

- **WHEN** a saved assistant reply is loaded from a bounded message page
- **THEN** its footer uses the original committed call facts and preserves unknown timing

#### Scenario: Recover a complete unsaved reply

- **WHEN** a complete reply cannot be saved
- **THEN** its performance footer retains the original facts during save-only recovery

#### Scenario: Inspect a repeated LLM invocation

- **WHEN** an LLM node runs repeatedly or inside a Loop
- **THEN** its graph card uses only snapshots correlated to the displayed invocation, with stable space reserved for FTT and both rates
- **AND** details explain both rate windows and report per-call facts and rate coverage

#### Scenario: Observe missing call data

- **WHEN** an LLM snapshot or provider token count is unavailable
- **THEN** the card displays unknown values without estimating tokens or borrowing another invocation's data
