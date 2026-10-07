## ADDED Requirements

### Requirement: Bound executable manifest inspection

On supported Linux and macOS executable formats, TUI preflight SHALL inspect regular files with bounded header, table, and manifest reads. It MUST reject ambiguous sections, invalid ranges, length overflow, oversized or incomplete records, unsupported versions, and inconsistent metadata before execution. Malformed or unsupported executables and manifests MUST NOT trigger command fallback or heuristic recovery.

#### Scenario: Inspect a large executable

- **WHEN** a supported executable exceeds the manifest payload budget but contains a valid bounded manifest
- **THEN** preflight reads its manifest without copying the entire executable into unbounded memory or rejecting it solely for executable size

#### Scenario: Reject a forged section range

- **WHEN** an executable declares a manifest range outside the file or an overflowing offset/length pair
- **THEN** inspection reports a bounded parsing failure without starting a process

#### Scenario: Reject malformed manifest framing

- **WHEN** a manifest has invalid magic, an unsupported version, an oversized declared payload, truncated JSON, duplicate JSON members, or unexpected nonzero trailing bytes
- **THEN** inspection fails before launch and does not invoke either inspection command

#### Scenario: Reject ambiguous manifest sections

- **WHEN** a supported executable contains more than one matching manifest section
- **THEN** inspection reports ambiguity without selecting a section or invoking command fallback

#### Scenario: Reject mismatched identities

- **WHEN** an embedded manifest's graph and interface identify different workflows
- **THEN** inspection fails before argument acceptance or process launch

#### Scenario: Reject unsupported executable formats

- **WHEN** the supplied file is not a supported executable container
- **THEN** inspection reports an actionable compatibility error without treating the file as a legacy runner

## MODIFIED Requirements

### Requirement: Launch and observe an existing local executable

`mf run <executable> --tui` SHALL read the supported embedded manifest of a new runner without starting an inspection
process. Only a successfully recognized supported executable lacking a manifest section SHALL use the existing bounded
description commands, requesting a matching startup interface when its description version requires one. Corrupt or
unsupported manifest data MUST prevent launch without command fallback.

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

- **WHEN** a runner's manifest or legacy description uses an unsupported graph or observation version, or a manifest-free runner lacks required description support
- **THEN** the CLI reports an actionable compatibility error before starting workflow execution

#### Scenario: Isolate a local observation session

- **WHEN** the parent environment contains remote signal endpoints and exporter credentials
- **THEN** the launched session sends its telemetry to the local receiver without those credentials and leaves the parent environment unchanged

#### Scenario: Start a fresh session

- **WHEN** the user invokes TUI mode after a previous session ended
- **THEN** the CLI starts a new local run without attaching to a prior process or requesting its event history

#### Scenario: Reject parameters before executing any branch

- **WHEN** a supplied startup argument is missing, unknown, or incompatible with the inspected interface
- **THEN** the CLI reports the affected node/port before launching workflow execution

#### Scenario: Preserve legacy finite runners

- **WHEN** a supported older single-run binary has no manifest, does not require interface inspection, and receives no startup parameters
- **THEN** existing bounded preflight, terminal launch, observation, and cleanup continue to work

#### Scenario: Preflight a manifest without executing code

- **WHEN** a runner with a supported manifest is inspected before launch
- **THEN** metadata discovery starts no process and invokes neither native startup code nor node factories

#### Scenario: Inspect another supported architecture

- **WHEN** a valid supported-format executable has a manifest but targets an architecture different from the inspection host
- **THEN** metadata discovery succeeds without executing it and does not claim the executable can run on that host

#### Scenario: Preserve legacy parameterized runners

- **WHEN** a supported older binary has no manifest and its description requires startup-interface inspection
- **THEN** preflight obtains and validates both legacy documents through the existing bounded command path before launch

#### Scenario: Resolve conditional stdin from the manifest

- **WHEN** a manifest declares stdin ownership unless a particular optional input is supplied
- **THEN** preflight evaluates that condition against the validated arguments and rejects active stdin requirements before launching the child

#### Scenario: Bound legacy inspection failures

- **WHEN** a manifest-free runner's inspection command times out, exceeds output limits, exits unsuccessfully, or returns an incomplete document
- **THEN** preflight reports failure and does not launch workflow execution
