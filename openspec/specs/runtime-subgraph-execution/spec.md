# runtime-subgraph-execution Specification

## Purpose

Provide shared prepared-body binding and scoped execution while node providers own container policies and lifecycle events.

## Requirements

### Requirement: Execute prepared bodies in managed scopes

The runtime SHALL expose a prepared subgraph containing node identities, selected output types, and an execution callback. A scoped invocation SHALL isolate body outputs and restore parent outputs and scope state after success, failure, or unwinding. Scope execution on the same context SHALL retain the remaining step budget; a fresh body context SHALL start an independent budget.

#### Scenario: Recover the parent after a failed body

- **WHEN** a nested body fails or unwinds after executing a node
- **THEN** the parent outputs and scope stack are restored and the consumed steps remain charged to the context

### Requirement: Bind prepared bodies through node providers

The registration contract SHALL provide explicit subgraph factories that construct complete executors
from validated bodies, configuration, and execution options. Registered providers SHALL own their
container execution policies. The compiler SHALL prepare and inject bodies through these factories.
Both in-memory Flows and generated direct node calls SHALL implement the same prepared-body contract.
An executor MUST NOT require a later body-binding method to become runnable.

#### Scenario: Bind either execution backend

- **WHEN** the compiler prepares a Loop or Iteration body in memory or generates a standalone runner
- **THEN** the registered factory receives a prepared body with the same selected output types and constructs the node that owns its invocation policy

### Requirement: Keep container policies above the runtime

The runtime SHALL manage scopes, input sources, node execution, type checks, publication, and budget consumption. `mfn-core` SHALL own Loop termination and Iteration scheduling, result collection, and failure policies. Loop and Iteration SHALL share prepared-body execution through managed runtime scopes.

#### Scenario: Reuse sequential execution

- **WHEN** a Loop repeats its body or an Iteration processes items sequentially
- **THEN** both execute prepared bodies through runtime scopes while supplying their own state, result, and termination policy

### Requirement: Keep container lifecycle observation in node implementations

Node implementations SHALL own Loop pass and Iteration item lifecycle publication. The runtime SHALL propagate existing Run and Body observation contexts and scoped invocation identities during node execution.

#### Scenario: Preserve built-in observation protocols

- **WHEN** an observed Loop pass or Iteration item executes a prepared body
- **THEN** node lifecycle publication and runtime context propagation preserve the existing protocol events
