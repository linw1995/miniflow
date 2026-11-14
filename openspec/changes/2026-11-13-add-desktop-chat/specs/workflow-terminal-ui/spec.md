# Terminal Chat Selection Delta

## MODIFIED Requirements

### Requirement: Launch chat mode explicitly

Without `--desktop`, `mf run <executable> --chat [--chat-db <path>]` SHALL imply `--tui` and validate the shared chat contract, terminal-owned
inputs, and terminal requirements before opening storage. Explicit `--desktop` SHALL select the separately specified desktop interface instead,
without applying terminal requirements. Each turn SHALL run the complete supplied executable
with fresh invocation and run identities, using shared storage and recovery APIs. The CLI MUST NOT substitute
an embedded flow, infer model identities from the graph, or override provider configuration.

Without `--chat-db`, storage SHALL default to `~/Library/Application Support/miniflow/chat.sqlite` on macOS
and `$XDG_DATA_HOME/miniflow/chat.sqlite` on Linux, falling back to `~/.local/share/miniflow/chat.sqlite` when
`XDG_DATA_HOME` is unset, empty, or relative. An explicit path SHALL take precedence without requiring
a resolvable user data directory. Default path resolution SHALL NOT create storage before chat validation.
Run help SHALL display the resolved absolute default path without creating storage. If the user data
directory cannot be resolved, help SHALL remain available and request an explicit `--chat-db` path.

#### Scenario: Execute repeated model calls

- **WHEN** a compatible conversation workflow contains a loop with multiple model calls
- **THEN** chat mode executes the entire workflow and persists its reported message metadata and individual usage facts

#### Scenario: Launch chat with default storage

- **WHEN** the user supplies `--chat` without `--tui` or `--desktop`
- **THEN** chat mode enables terminal observation and uses the user data directory unless `--chat-db` is supplied

#### Scenario: Missing user data directory

- **WHEN** chat mode has neither `--chat-db` nor a resolvable user data directory
- **THEN** the CLI fails with a diagnostic requesting `--chat-db` before opening storage or starting a runner

#### Scenario: Keep ordinary observation available

- **WHEN** the user runs a compatible chat executable with ordinary `--tui` and its required startup arguments
- **THEN** the CLI retains single-run observation semantics instead of entering chat mode implicitly

#### Scenario: Reserve model context inputs

- **WHEN** startup arguments attempt to override prompt, system, or history owned by chat mode
- **THEN** preflight rejects the conflicting binding before launch

#### Scenario: Select a different presentation explicitly

- **WHEN** the user selects `--chat --desktop` in a supported desktop-enabled build
- **THEN** chat uses the same storage and executable contracts without entering terminal mode
