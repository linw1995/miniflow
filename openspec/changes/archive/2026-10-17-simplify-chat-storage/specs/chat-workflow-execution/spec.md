## MODIFIED Requirements

### Requirement: Schedule the first automatic title independently

With automatic titles enabled, the host SHALL explicitly schedule titles after conversation persistence and during startup or polling. Candidates SHALL be provisional sessions with a successful turn and no previous title call; scheduling SHALL use the first successful turn and be idempotent. Disabling automatic titles SHALL suppress automatic scheduling and dispatch. Explicit scheduling and retry SHALL remain available.

#### Scenario: Complete the first turn

- **WHEN** the first turn of a provisionally titled session commits
- **THEN** its response is available before title scheduling or generation
- **AND** the automatic host separately queues at most one title request

#### Scenario: Restart before title scheduling

- **WHEN** the host restarts after a successful turn commits but before any title request is queued
- **THEN** automatic dispatch discovers that session and schedules its first successful turn once

#### Scenario: Restart before title execution

- **WHEN** the host restarts with a never-started pending title request
- **THEN** that request retains its identity and remains eligible for execution

#### Scenario: Disable automatic titles

- **WHEN** automatic titles are disabled and a conversation commits
- **THEN** no title request is automatically queued or started
- **AND** explicit scheduling and dispatch remain available

#### Scenario: Fail title generation

- **WHEN** the title provider fails or its call is cancelled or interrupted
- **THEN** the conversation remains committed and its provisional title remains available
- **AND** later conversations and automatic dispatch do not repeat that title attempt
