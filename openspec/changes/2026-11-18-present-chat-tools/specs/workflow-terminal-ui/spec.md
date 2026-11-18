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

#### Scenario: Omit historical argument deltas

- **WHEN** a call's argument preview exceeds 8 KiB or the shared preview budget requires eviction
- **THEN** the expanded group retains a recent UTF-8 tail and identifies omitted deltas and bytes
- **AND** the expanded header identifies the total size and truncation
- **AND** canonical arguments after settlement or SQLite reload have no display tail limit or invented delta counts
- **AND** complete authoritative arguments remain persisted and available to dispatch, while delta history and omission markers are not persisted

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

#### Scenario: Bound live preview data

- **WHEN** argument previews exceed the count or byte budget
- **THEN** retained preview data remains bounded and valid UTF-8, recent tool argument deltas replace older display history, omitted data is indicated, and consumption and final settlement continue

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
