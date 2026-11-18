## ADDED Requirements

### Requirement: Tool argument previews and collapsible results

Chat SHALL group raw argument fragments by model call and provider tool index. Authoritative completion SHALL replace previews without duplicates and group results with their visible calls without changing stored order. Each result SHALL start collapsed, with a leading fold marker and independent keyboard expansion. Arguments and results MUST remain literal.

#### Scenario: Show interleaved argument fragments

- **WHEN** the model sends interleaved fragments for multiple tool calls
- **THEN** each fragment appears in the matching preview without waiting for a tool result
- **AND** canonical transcript entries replace provisional calls when the turn settles

#### Scenario: Pair parallel calls and results

- **WHEN** the visible transcript contains a batch with tool results in a different order than the calls
- **THEN** each result appears under its matching call and keyboard navigation follows that display order

#### Scenario: View a result on a page without its call

- **WHEN** a history page contains a tool result but not its matching call
- **THEN** the result remains visible with its call ID and the fold marker before the ID

#### Scenario: Expand and collapse a result

- **WHEN** the user selects a result with Tab or Shift+Tab in browse mode and presses Space or Ctrl+T
- **THEN** only that result changes expansion state, without running tools, changing model context, or interpreting result Markdown or escape sequences

#### Scenario: Show observed live tool status

- **WHEN** a tool Flow execution is observed for the active chat invocation
- **THEN** the UI shows the observed state beside the matching live call or as a separate execution row when no preview has that ID
- **AND** model argument fragments alone do not establish an execution state

#### Scenario: Bound live preview data

- **WHEN** argument previews exceed the count or byte budget
- **THEN** retained preview data remains bounded and valid UTF-8, omitted data is indicated, and consumption and final settlement continue
