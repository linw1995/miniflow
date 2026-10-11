# Workflow Binary Compilation Delta

## MODIFIED Requirements

### Requirement: Generate standard runner entry points on demand

Standard task and streaming runners SHALL generate only preparation and execution entry points actually
used by their supported whole-flow and isolated Task dispatch paths. Streaming runners MUST NOT generate
unused whole-flow task convenience helpers solely to support isolated Task selection. The general-purpose
generator SHALL retain its existing convenience entry points. Both paths MUST share validation and
execution semantics. Compilation MUST preserve Cargo diagnostics without suppressing unused-function
warnings.

#### Scenario: Compile documented task workflows

- **WHEN** the CLI compiles a documented task workflow using the standard runner
- **THEN** generated workflow source contains only the preparation and context execution entry points needed by that runner, compilation emits no warnings from unused generated functions, and execution produces the documented outputs

#### Scenario: Compile a documented streaming workflow

- **WHEN** the CLI compiles a documented streaming workflow
- **THEN** generated workflow source contains stream preparation and any used isolated Task entry points without unused whole-flow task execution helpers, and execution produces the documented stream outputs

#### Scenario: Compile without telemetry

- **WHEN** the CLI compiles a task runner with `--no-telemetry`
- **THEN** unused convenience functions are not generated, compilation remains warning-free, and runner execution and direct manifest inspection remain available

#### Scenario: Generate a custom runner

- **WHEN** a caller requests general-purpose artifacts for a custom runner
- **THEN** the existing convenience functions remain available for startup arguments, observation, and runtime options, with the same workflow plan and runtime behavior

#### Scenario: Compile isolated execution support once

- **WHEN** a new standalone runner is built for either workflow execution mode
- **THEN** its isolated Task selection records and required entry points are included in the existing validated build without a second compilation or additional provider preparation
