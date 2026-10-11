# Execute a Single Node in a Compiled Runner

## Why

[Issue #125](https://github.com/linw1995/miniflow/issues/125) requests command-line execution of an arbitrary
node already embedded in a flow. Today a runner only executes the complete graph, and its startup interface
only accepts initial-node inputs, so debugging a middle node can invoke unrelated operations.

## What Changes

- Add `--node ID` to newly compiled runners to select one top-level Task executor, including tasks in stream
  flows and Loop or Iteration nodes executed as complete composite operations.
- Reuse the existing qualified named arguments, `--inputs`, and `--inputs-file` against the selected node's
  resolved input contract, without requiring arguments for other workflow roots.
- Add exclusive `--context JSON` and `--context-file PATH` transports for explicitly declared external
  context references; reject unrelated keys and validate supplied values before business execution.
- Invoke the selected prepared task through the shared runtime lifecycle without executing external data
  predecessors, control predecessors, successors, or a containing generated typed segment.
- Return the target's produced outputs under port names, omit skipped ports, and preserve explicit null.
- Give isolated executions a deterministic selected-operation observation identity and accurate lifecycle
  counts while retaining bounded telemetry and opt-in finite snapshots.
- Preserve ordinary graph execution, workflow startup interfaces, and embedded manifest framing. Producer,
  Event, nested-body selection, and TUI execution remain follow-up work; completing this Task phase alone
  does not establish support for every node category requested by #125.

## Capabilities

### New Capabilities

- `single-node-execution`: isolated invocation of a prepared top-level task with validated port values,
  explicitly supplied context, and target-port results.

### Modified Capabilities

- `workflow-runtime-execution`: allow validated prepared task targets alongside complete prepared plans,
  and select targets through compiled indices without runtime graph reconstruction.
- `workflow-binary-compilation`: generate only the preparation and dispatch entry points actually used by
  normal and isolated runner invocations.
- `workflow-observability`: describe and identify isolated finite executions independently of the full
  workflow, including targets selected from a streaming executable.

## Impact

Changes affect `mf-runtime` argument parsing, execution-context binding and task invocation; `mf-compiler`
target records, generated preparation and runner dispatch; generated entry points; telemetry description
selection; integration coverage; and runner documentation. The first implementation reuses whole-plan
preparation and its resolved metadata, so unrelated factory initialization failures can still prevent an
isolated invocation. No additional dependency, definition schema change, manifest interface expansion, or
CLI/TUI feature is planned for this phase. Existing executables require recompilation to gain `--node`.
