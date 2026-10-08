## MODIFIED Requirements

### Requirement: Launch and observe an existing local executable

`mf run <executable> --tui` SHALL read the supported embedded manifest without starting an inspection process.
Executables lacking a manifest section SHALL be rejected with a recompilation diagnostic. Corrupt or unsupported
manifest data MUST prevent launch without command fallback.

The CLI SHALL validate supplied startup arguments, resource availability, and observation compatibility before launch,
establish a loopback OTLP/HTTP receiver, and then launch the actual executable with the same arguments and session-specific
observation configuration. It MUST NOT reimplement workflow execution. The receiver SHALL accept protobuf logs and traces
at `/v1/logs` and `/v1/traces` and associate observations with the expected workflow and run. Configuration changes MUST be
limited to the child environment; inherited remote exporter credentials MUST NOT be sent to the local receiver.

Each TUI session SHALL observe only its own launched local child. The interface MUST NOT provide remote or
existing-process attachment, session resumption, or historical replay. A new invocation SHALL create a new run;
missing observations MUST NOT trigger workflow restart.

#### Scenario: Observe an executable without build inputs

- **WHEN** the user invokes TUI mode with a compatible runner but no workflow source, lockfile, plugin sources, or Rust toolchain
- **THEN** the CLI loads its metadata and displays events from execution of that runner

#### Scenario: Prevent a startup race

- **WHEN** a runner completes immediately after process startup
- **THEN** the receiver was ready before execution began and remains available for final telemetry draining

#### Scenario: Reject an incompatible runner

- **WHEN** a runner's manifest uses an unsupported graph or observation version, or the runner has no manifest
- **THEN** the CLI reports an actionable compatibility error before starting workflow execution

#### Scenario: Reject a runner without an embedded manifest

- **WHEN** a supported older executable has no manifest section
- **THEN** preflight reports that recompilation is required without starting an inspection process or workflow execution

#### Scenario: Isolate a local observation session

- **WHEN** the parent environment contains remote signal endpoints and exporter credentials
- **THEN** the launched session sends its telemetry to the local receiver without those credentials and leaves the parent environment unchanged

#### Scenario: Start a fresh session

- **WHEN** the user invokes TUI mode after a previous session ended
- **THEN** the CLI starts a new local run without attaching to a prior process or requesting its event history

#### Scenario: Reject parameters before executing any branch

- **WHEN** a supplied startup argument is missing, unknown, or incompatible with the inspected interface
- **THEN** the CLI reports the affected node/port before launching workflow execution

#### Scenario: Preflight a manifest without executing code

- **WHEN** a runner with a supported manifest is inspected before launch
- **THEN** metadata discovery starts no process and invokes neither native startup code nor node factories

#### Scenario: Inspect another supported architecture

- **WHEN** a valid supported-format executable has a manifest but targets an architecture different from the inspection host
- **THEN** metadata discovery succeeds without executing it and does not claim the executable can run on that host

#### Scenario: Resolve conditional stdin from the manifest

- **WHEN** a manifest declares stdin ownership unless a particular optional input is supplied
- **THEN** preflight evaluates that condition against the validated arguments and rejects active stdin requirements before launching the child
