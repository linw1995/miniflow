## ADDED Requirements

### Requirement: Capture verbose runner diagnostics in terminal execution

`mf run <executable> --tui --verbose` SHALL enable the runner's local diagnostics and capture them through the existing
bounded stderr history. Terminal presentation SHALL retain its existing stdout delivery, observation receiver,
interruption, and cleanup behavior.

#### Scenario: Inspect verbose terminal execution

- **WHEN** a user runs a compatible workflow with both `--tui` and `--verbose`
- **THEN** the child receives the verbose option and its diagnostic stderr is available in captured output history
