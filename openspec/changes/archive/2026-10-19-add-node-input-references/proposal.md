## Why

Moving data mappings into node-local inputs makes a single execution-edge field sufficient. Workflows also need to order
side effects when a producer has no output port, which existing output-based gates cannot express. Port-level TUI
routing obscures execution relationships in parameter-heavy workflows.

## What Changes

- Introduce definition version `2026-10-19`: `nodes[].inputs` binds data and `edges` exclusively declares execution dependencies. Reject `control_edges` and data-binding fields on new-version edges.
- An edge without `from_output` waits for source Task success; an edge with it retains output-availability gating. Skipped prerequisites skip the target in both cases.
- Parse existing versions with their original semantics, then normalize both formats before graph analysis. Never guess edge meaning from its shape.
- Add frame-local Task outcomes and typed node-success dependencies, with programmatic and generated execution parity. Limit new success edges to ordinary Tasks in the same frame.
- Extend versioned descriptions and skip observations for success dependencies, retaining compatibility with old executables in updated readers.
- Make the TUI default to execution connections, with selected-node bindings, aggregated dependency views, and full port views.

## Non-goals

No new node kind or scheduler, finally-style execution after failure, Event/Stream lifetime completion edges, input
expressions or projections, implicit configuration dependencies, cross-stream joins, automatic source rewriting, or
active-path filtering.

## Capabilities

### New Capabilities

- `node-input-references`: versioned declarative input sources normalized into data bindings.
- `explicit-execution-dependencies`: unified authoring edges and frame-local node-success prerequisites.

### Modified Capabilities

- `workflow-terminal-ui`: separate semantic topology from visible routes and support execution views.
- `conditional-flow-execution`: support versioned execution edges and conjunctive node-success prerequisites.
- `workflow-binary-compilation`: describe node-success dependencies without fabricated ports.

## Impact

Definition parsing, runtime dependencies and outcomes, compiler preparation and generated execution, telemetry
descriptions and skip causes, TUI navigation/rendering, documentation, and examples. Existing definition versions remain
supported. Provider input/output APIs and startup parameter behavior remain unchanged. This changes execution contracts
as well as authoring syntax.
