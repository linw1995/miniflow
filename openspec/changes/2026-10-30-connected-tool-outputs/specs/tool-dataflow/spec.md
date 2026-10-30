# Connected tool dataflow

## Added requirements

### Requirement: Reflect the actual workflow

Nodes SHALL inspect immutable metadata from their actual prepared graph. The metadata SHALL expose
ports, decoded schemas, connections, and subgraphs without executors or runtime/configuration values.
Nested scopes SHALL restore the enclosing graph on normal completion, errors, and unwinding.

### Requirement: Publish tool requests as ordinary data

The LLM SHALL derive definitions from connected argument-mode Iterations and SHALL publish normal
WorkflowArguments batches. Parameters supplied by shared input connections SHALL be excluded from
model ownership. The LLM SHALL NOT execute downstream nodes. Optional fields, decoded constraints,
and sensitivity SHALL remain consistent with the workflow interface.

#### Scenario: Repeated requests

- GIVEN two calls to the same connected tool in one model response
- WHEN the response completes
- THEN the tool output contains two argument records and Iteration produces two ordered results

#### Scenario: Unused tool

- GIVEN a connected tool is not selected by the model
- WHEN the response completes
- THEN its output is an empty list and its body executes no nodes

### Requirement: Reuse workflow input binding and execution

Argument-mode Iteration SHALL bind each item's arguments through the standard workflow interface.
Ordinary nodes and connections SHALL execute tool work and downstream result processing. Missing,
unknown, or invalid values SHALL retain normal typed workflow errors. Shared values SHALL not leak
between items, and parallel execution SHALL preserve result order.

#### Scenario: Explicit downstream result processing

- GIVEN multiply_flow.product connects to format_result.product
- WHEN an argument item runs
- THEN the formatter consumes the normal Flow output and its selected result reaches the result encoder

### Requirement: Encode connected results

The result encoder SHALL consume explicit result-list connections, preserve original call ordering,
reject ambiguous IDs or count mismatches, redact sensitive values, and produce bounded tool protocol
content. It SHALL NOT execute or invoke another node.

### Requirement: Preserve compiled contract equivalence

Generated finite and native workflows SHALL verify prepared port and decoded schema metadata before
execution. Metadata-only schema drift SHALL fail startup even when coarse port types stay unchanged.
