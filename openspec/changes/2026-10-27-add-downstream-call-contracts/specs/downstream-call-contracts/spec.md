# Selectable workflow contracts

## Added requirements

### Requirement: Derive arguments from graph connections

Selectable workflow calls SHALL use ordinary output-port declarations and target input connections.
The caller SHALL advertise only inputs supplied by its own connected ports. Connections from other
producers SHALL retain external ownership. Explicit connections SHALL take precedence over fallback
ownership, and ordinary type inference SHALL validate their compatibility.

#### Scenario: Combine a model argument and configuration

- GIVEN a Dispatch amount port connected to a tool Flow input and a configuration factor connected
  to another input
- WHEN the model inspects that tool
- THEN only amount is advertised, while execution composes both values through the declared connections

#### Scenario: Reject a duplicate mapping surface

- GIVEN a call declaration containing a separate ports mapping
- WHEN the workflow is decoded
- THEN the unknown field is rejected and the ordinary input connections remain authoritative

### Requirement: Reuse workflow execution and explicit results

Selectable targets SHALL be existing `workflow.call` sites. Their bodies SHALL execute through the
normal workflow executor. Only the body's explicit workflow outputs SHALL return to the caller.
Internal node values SHALL follow ordinary connections and SHALL remain private unless selected.

#### Scenario: Process a tool result downstream

- GIVEN a calculation connected to a formatter inside a tool Flow
- WHEN the workflow is selected
- THEN the formatter runs and its explicitly selected output becomes the returned result

#### Scenario: Keep an internal value private

- GIVEN a calculation publishes a public value and an internal value
- WHEN only the downstream public result appears in the Flow outputs
- THEN the caller receives that selected result and cannot obtain the internal value through the adapter

### Requirement: Preserve normal workflow contracts

Invocation SHALL retain typed input and output validation, value sharing, sensitivity, skip behavior,
error chains, cancellation, execution limits, and existing worker ownership. Unselected tools SHALL
not execute. External prerequisites SHALL not make the caller inherit their skip state.

#### Scenario: Use an unselected failing workflow

- GIVEN an unselected tool Flow contains a failing calculation
- WHEN another tool is selected
- THEN only the selected Flow executes

### Requirement: Reflect through workflow interfaces

Decoded schema refinements SHALL propagate through workflow input descriptors and Call metadata.
Reflection SHALL not execute codecs or defaults. Public projections SHALL hide external parameters
and runtime values. Read-only Catalog views SHALL grant no invocation capability.

#### Scenario: Detect schema-only drift

- GIVEN a compiled tool input whose coarse type stays unchanged but whose decoded schema changes
- WHEN a generated program starts
- THEN startup rejects the changed contract before tool execution

### Requirement: Preserve backend equivalence

Finite workflows, native message frames, and generated programs SHALL consume the same resolved
argument contracts. Native frames SHALL isolate message values and import only required startup
snapshots. Invalid cross-message inputs and producer wait cycles SHALL fail preparation.

### Requirement: Compose model requests explicitly

Catalog, the single model request, workflow selection, and history assembly SHALL be explicit workflow
steps. Dispatch SHALL return correlated results from selected workflow outputs. Repetition and usage
accumulation SHALL be controlled by the containing workflow.
