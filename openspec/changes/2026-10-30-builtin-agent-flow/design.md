# Design

Workflow version `2026-10-31` introduces `subgraphs` for source aliases and named parameter declarations.
A Flow declares single or collection parameters with optional defaults. Calling Nodes supply actual
arguments through node-level `subgraphs`; references forward lexical bindings through nested Flows
and container bodies. Node sources adapt to single-node Flows; existing file and built-in sources
retain their ordinary resolution and dependency ownership. Earlier definitions keep their `flows`
wire representation.

`NodeFactory::Subgraphs` receives one named map. Native Call, Loop, and Iteration bodies use `body`
in this same map. `PreparedSubgraph` remains the executable carrier. Public input aliases preserve
original decoded types, defaults, optionality, and sensitivity. Preparation validates aliases and
cardinality, rejects missing/unknown arguments, and freezes Node output selection and interfaces.
Generated parameter scopes use the same immutable layouts and contract checks as ordinary bodies.

Interface projections contain only public input/output contracts and stream capabilities. They
carry neither executors nor environment/default/runtime values. Metadata consumers have no call
capability. Execution contexts no longer install or expose a global workflow graph. Prepared Flow
and stream-plan graph descriptors remain useful for compilation, hosts, and generated schema checks.

The agent accepts a caller-owned `tools` collection. Its catalog reads an interface projection once
before the model-round Loop and excludes explicit caller-bound fields from model schemas. One
argument-mode Iteration processes terminal model calls in order. Dispatch selects a named callable
subgraph and enters existing Call scopes; its children run through normal workflow scheduling.
Caller bindings cannot be overwritten by model arguments. The selected child's public output object
is correlated with its call ID and redacted before bounded protocol encoding.

The example supplies multiply as a Flow and add as a Node. An outer factor input binds multiply's
public factor port. Its amount remains model-supplied. Inside the Flow, `multiply_flow.product`
connects directly to `format_multiply.product`. Model rounds retain ordinary Loop state and limits;
`%loop.index` replaces redundant counting and pure initialization values share one node.

Description version `2026-10-31` recognizes explicit callable parameter bodies independently of a
provider's node kind. Existing Call lifecycle records, cancellation, worker/step budgets, typed error
chains, optional streams, and finite-body restrictions continue to apply.
