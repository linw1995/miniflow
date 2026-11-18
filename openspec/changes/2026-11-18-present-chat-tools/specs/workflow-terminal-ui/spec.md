## ADDED Requirements

### Requirement: Tool argument previews and collapsible results

The chat transcript SHALL show raw argument fragments grouped by model call and provider tool index, without tool execution status. Authoritative completion SHALL replace provisional calls without duplication. Tool results from the authoritative transcript SHALL be collapsed by default and individually expandable using the keyboard. Arguments and results MUST remain literal.

#### Scenario: Show interleaved argument fragments

- **WHEN** the model sends interleaved fragments for multiple tool calls
- **THEN** each fragment appears in the matching preview without waiting for a tool result
- **AND** canonical transcript entries replace provisional calls when the turn settles

#### Scenario: Expand and collapse a result

- **WHEN** the user selects a result with Tab or Shift+Tab in browse mode and presses Space or Ctrl+T
- **THEN** only that result changes expansion state, without running tools, changing model context, or interpreting result Markdown or escape sequences

#### Scenario: Bound live preview data

- **WHEN** argument previews exceed the count or byte budget
- **THEN** retained preview data remains bounded and valid UTF-8, omitted data is indicated, and consumption and final settlement continue
