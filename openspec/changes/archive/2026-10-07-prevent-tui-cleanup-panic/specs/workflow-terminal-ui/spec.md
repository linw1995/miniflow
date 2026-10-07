## ADDED Requirements

### Requirement: Complete cleanup after display disconnection

A TUI display I/O failure SHALL terminate the child, restore terminal input, and return CLI failure status 1. Cleanup MUST NOT panic when display stderr is unavailable, including during cursor restoration after a completed frame.

#### Scenario: Disconnect a rendered display

- **WHEN** the display terminal disconnects after a completed frame while the workflow child is still running
- **THEN** the CLI stops the child, restores the input terminal, and exits with status 1 without a cleanup panic
