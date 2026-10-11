## ADDED Requirements

### Requirement: Run existing executables without terminal presentation

`mf run <executable>` SHALL execute an existing workflow binary without requiring `--tui`, an interactive terminal,
manifest inspection by the CLI, or a telemetry receiver. It SHALL inherit stdin, stdout, and stderr, forward supplied
workflow parameters without shell interpretation, and preserve ordinary child exit codes. On Unix, child signal
termination SHALL return `128 + signal`. Launch and wait errors SHALL retain their typed I/O causes.

#### Scenario: Run a workflow with piped input

- **WHEN** the user pipes data into `mf run ./workflow` without `--tui`
- **THEN** the workflow receives that input and writes its results and diagnostics to the inherited streams

#### Scenario: Forward named and JSON parameters

- **WHEN** the user supplies named workflow parameters, inline JSON, or a parameter file to direct CLI execution
- **THEN** the runner receives the selected transport and preserves path and value boundaries

#### Scenario: Preserve process failure

- **WHEN** a directly launched workflow exits unsuccessfully or terminates through a Unix signal
- **THEN** the CLI returns the corresponding ordinary exit code or conventional signal status

### Requirement: Report verbose compilation details

The CLI SHALL accept `-v` and `--verbose` for compilation. Verbose compilation SHALL report the definition, output,
selected build options, successful elapsed time, and retained generated-project location on stderr.

#### Scenario: Inspect an explicit compilation

- **WHEN** the user invokes `mf compile flow.json --output ./flow --verbose`
- **THEN** stderr includes compilation options and a successful completion summary without changing build validation
