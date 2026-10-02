# Design

## Complete preparation results

`TaskNode` contains one execution method taking inputs and a mutable execution context and returning
`NodeResult`. Metadata is stored in `NodeMetadata`: ports, output derivations, and context references.
`PreparedNode` owns that metadata and a task executor. `FlowNode` adds the definition-facing identity.
Metadata is resolved before execution and does not depend on later calls to the executor.

## Explicit construction inputs

`NodeRegistration` selects a `NodeFactory::Plain` or `NodeFactory::Subgraph` function. Plain factories
receive configuration. Subgraph factories additionally receive the occurrence identity, execution
options, and a validated `PreparedSubgraph`. Invalid construction requests fail during preparation.
There is no runnable declaration awaiting a later body binding step.

The compiler prepares bodies using its existing Loop and Iteration lowering. Providers construct the
final executor and own their existing control policies. Generated and in-memory preparation use the
same factory functions and metadata validation; neither performs business execution during validation.

## Runtime ownership

Tasks retain `Send + Sync`, so prepared synchronous bodies and worker execution can share them safely.
This base change has no event state or streaming scheduler. The streaming follow-up selects task or
event execution explicitly and transfers mutable event state to its coordinator before sharing the plan.

## Migration and verification

Update all workspace providers and the external fixture package. Preserve the existing behavior suites
for typed ports, context references, conditional flow, Loop/Iteration, generated runners, and observation.
Exercise both valid subgraph construction and requests missing a required body. Verify the base branch
on its own before updating the dependent streaming PR.
