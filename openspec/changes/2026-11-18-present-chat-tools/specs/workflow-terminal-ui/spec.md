## ADDED Requirements

### Requirement: Collapsible live tool call groups

Chat SHALL group arguments and results by call. Groups SHALL start collapsed and be independently expandable during streaming, with a leading fold marker. Authoritative completion SHALL replace previews without duplicates or changes to stored order. Arguments and results MUST remain literal.

#### Scenario: Show interleaved argument fragments

- **WHEN** the model sends interleaved fragments for multiple tool calls
- **THEN** each fragment appears in its expanded group without waiting for a tool result
- **AND** an unambiguous late wire ID and canonical replacement preserve the group's focus and expansion
- **AND** live rows have the same message and inter-tool spacing as the corresponding settled batches

#### Scenario: Highlight incomplete JSON arguments

- **WHEN** an expanded group shows raw argument fragments or canonical arguments
- **THEN** the text receives incremental JSON syntax highlighting
- **AND** incomplete or malformed fragments preserve their exact retained text without validation, reformatting, or Markdown interpretation

#### Scenario: Omit historical argument deltas

- **WHEN** a call's argument preview exceeds 8 KiB or the shared preview budget requires eviction
- **THEN** the expanded group retains a recent UTF-8 tail and identifies omitted deltas and bytes
- **AND** the collapsed header identifies the total size and truncation
- **AND** canonical arguments after settlement or SQLite reload receive the same tail limit without invented delta counts
- **AND** complete authoritative arguments remain persisted and available to dispatch, while delta history and omission markers are not persisted

#### Scenario: Pair parallel calls and results

- **WHEN** the visible transcript contains a batch with tool results in a different order than the calls
- **THEN** each result appears under its matching call and keyboard navigation follows that display order

#### Scenario: View a result on a page without its call

- **WHEN** a history page contains a tool result but not its matching call
- **THEN** the result remains visible with its call ID and the fold marker before the ID

#### Scenario: Expand and collapse a group

- **WHEN** the user selects a group with Tab or Shift+Tab in browse mode and presses Space or Ctrl+T
- **THEN** only that group's arguments and result change visibility, without running tools, changing model context, or interpreting tool payloads as Markdown or escape sequences

#### Scenario: Show observed live tool status

- **WHEN** a tool Flow execution is observed for the active chat invocation
- **THEN** the UI updates the matching tool call group header, even when it is collapsed
- **AND** if no preview can be matched by ID, the observation is shown in a tool call group without guessed arguments
- **AND** model argument fragments alone do not establish an execution state

#### Scenario: Bound live preview data

- **WHEN** argument previews exceed the count or byte budget
- **THEN** retained preview data remains bounded and valid UTF-8, recent tool argument deltas replace older display history, omitted data is indicated, and consumption and final settlement continue
