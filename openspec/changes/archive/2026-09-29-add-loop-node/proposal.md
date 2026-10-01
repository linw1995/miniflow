# Proposal

## Why

The current workflow model runs each node at most once in a DAG. It cannot express a refinement
process whose next pass reads values written by the previous pass. Dify's [Loop
node](https://docs.dify.ai/en/cloud/use-dify/nodes/loop) provides the desired behavior: sequential
passes, persistent loop variables, a termination condition, a maximum pass count, and an explicit
early exit. A loop is a control structure around a workflow body, so adding an ordinary node
registration would leave planning, state isolation, generated execution, and observation incorrect.

## What Changes

- Add a versioned, structured Loop definition with typed variables, an inner DAG, a required maximum count, an optional post-pass termination condition, and explicit loop assignment and exit steps.
- Compile the outer graph and each loop body independently while validating their typed connections, control dependencies, context references, and scope boundaries. Preserve the existing DAG rules within each graph.
- Execute loop bodies sequentially with a fresh output scope per pass and persistent, typed loop variables. Publish final variables as Loop outputs only after successful termination.
- Generate structured Rust control flow for compiled runners and use the same runtime scope and step helpers for in-memory execution.
- Version the observation description and lifecycle contract to identify each actual node invocation by its loop path and pass index. Update the TUI to show loop progress without treating repeated invocations as retries.
- Bound individual loops and total scheduled steps; report failures with the loop path and pass index.

## Capabilities

### New Capabilities

- `workflow-loop-execution`: Define, validate, execute, and compile bounded, stateful workflow loops.

### Modified Capabilities

- `workflow-binary-compilation`: Plan nested DAG bodies and generate repeated execution.
- `workflow-observability`: Describe loop scopes and identify repeated node invocations.
- `workflow-terminal-ui`: Display live and completed loop passes with bounded history.

## Impact

- `mf-runtime` gains loop frames, state writes, exit signaling, and an execution budget. Ordinary plugin `Node` methods remain read-only with respect to engine state.
- `mf-compiler` gains recursive structural planning and generated loop control flow. `mfn-core` registers the Loop declaration; assignment and exit remain reserved engine controls. Plugin dependencies remain explicit for the Loop container and ordinary nodes inside and outside it.
- `mf-telemetry` and `mf-tui` gain new protocol versions for repeated invocations; existing runner descriptions and lifecycle records retain their current interpretation.
- Documentation and a compiled example cover variable updates, both stop conditions, early exit, skipped branches, and diagnostics. Dify workflow export/import compatibility is outside this change.
