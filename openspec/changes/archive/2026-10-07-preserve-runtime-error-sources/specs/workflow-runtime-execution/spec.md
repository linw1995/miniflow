## ADDED Requirements

### Requirement: Preserve typed runtime lookup causes

Task dependency and workflow output lookup failures SHALL retain their original `NodeExecutionError` source. Dependency failures SHALL identify the consuming node and input or control dependency and retain the dependency observation phase. Selected-output failures SHALL identify the source node and declared workflow output name and retain the workflow output selection phase. Diagnostic formatting MUST NOT replace the source with a string.

#### Scenario: Preserve a missing data dependency

- **WHEN** a task requires an output that was not published
- **THEN** execution fails before invocation, identifies the consuming node and input, and retains the original lookup error source

#### Scenario: Preserve missing control dependency precedence

- **WHEN** a task has a skipped input and a missing control dependency
- **THEN** the missing dependency failure takes precedence over skipping, identifies the control dependency, and retains the original lookup error source

#### Scenario: Preserve a missing selected output

- **WHEN** a required or optional workflow output selects an output that was not published
- **THEN** selection fails with the declared output name and source node, retains the original lookup error source, and is attributed to workflow output selection
