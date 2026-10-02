# Spec Delta

## ADDED Requirements

### Requirement: Reject unsupported streaming launch before execution

The terminal launcher SHALL reject a runner that requires streaming input or the new streaming observation protocol during description preflight. Its diagnostic MUST explain how to execute the standalone runner with JSON Lines input. It MUST NOT launch the runner with null stdin and present that empty stream as the requested workflow execution.

#### Scenario: Inspect a streaming executable in terminal mode

- **WHEN** `mf run <executable> --tui` obtains a description requiring streaming execution
- **THEN** it reports the unsupported launch mode before starting workflow execution or enabling snapshot capture

#### Scenario: Preserve existing terminal workflows

- **WHEN** the description uses a supported single-run protocol
- **THEN** terminal launch, observation, input ownership, and process cleanup retain their existing behavior
