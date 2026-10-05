# Spec Delta

## MODIFIED Requirements

### Requirement: Launch compatible streaming sources

The terminal launcher SHALL support source-driven streaming protocols. It SHALL use null child stdin for
sources whose bindings or startup arguments require none. An active stdin requirement SHALL require `--stream-input <PATH>` while the terminal
remains available for keyboard input. Missing, unreadable, unused, or `-` input selections MUST fail before workflow execution.

#### Scenario: Observe an autonomous source

- **WHEN** a file-reading producer receives valid workflow startup parameters
- **THEN** TUI execution launches it with null stdin and displays producer and downstream activity

#### Scenario: Observe an explicit stdin source

- **WHEN** the workflow declares stdin and the user selects a readable stream-input file
- **THEN** that file supplies the child source while terminal keyboard input remains owned by the TUI

#### Scenario: Reject missing source input

- **WHEN** a workflow requires stdin but no stream-input file was supplied
- **THEN** preflight reports the required option and does not launch an empty stream

#### Scenario: Identify an older streaming runner

- **WHEN** a runner requires an unsupported streaming observation protocol
- **THEN** ordinary compatibility preflight requests recompilation before executing the workflow
