## ADDED Requirements

### Requirement: Live tool lifecycle and collapsible results

The chat transcript SHALL show streamed tool arguments and independently correlated execution states. Outstanding tools SHALL be marked interrupted when execution ends without authoritative completion. Tool results SHALL be collapsed by default and individually expandable using keyboard navigation. Arguments and results MUST remain inert literal data.

#### Scenario: Parallel execution and recovery

- **WHEN** one tool completes while another remains active
- **THEN** their displayed states remain independent
- **AND** authoritative completion replaces provisional records without duplication, including unsaved recovery and history reload

#### Scenario: Expand and collapse a result

- **WHEN** the user selects a result with Tab or Shift+Tab in browse mode and presses Space or Ctrl+T
- **THEN** only that result changes expansion state, without running tools, changing model context, or interpreting result Markdown or escape sequences

#### Scenario: Bound live tool data

- **WHEN** argument or result previews exceed the count or byte budget
- **THEN** retained preview data remains bounded and valid UTF-8, omitted data is indicated, and lifecycle consumption and final settlement continue
