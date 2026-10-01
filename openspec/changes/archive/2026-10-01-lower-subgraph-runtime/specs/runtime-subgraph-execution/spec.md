## ADDED Requirements

### Requirement: Execute prepared bodies in managed scopes

The runtime SHALL expose a prepared subgraph containing node identities, selected output types, and an execution callback. A scoped invocation SHALL isolate body outputs and restore parent outputs and scope state after success, failure, or unwinding. Scope execution on the same context SHALL retain the remaining step budget; a fresh body context SHALL start an independent budget.

#### Scenario: Recover the parent after a failed body

- **WHEN** a nested body fails or unwinds after executing a node
- **THEN** the parent outputs and scope stack are restored and the consumed steps remain charged to the context

### Requirement: Bind prepared bodies through node providers

The runtime node contract SHALL provide prepared-body binding. Registered providers SHALL own their container execution policies. The compiler SHALL inject validated bodies through this contract rather than construct Loop or Iteration executors itself. Both in-memory Flows and generated direct node calls SHALL implement the same prepared-body contract.

#### Scenario: Bind either execution backend

- **WHEN** the compiler prepares a Loop or Iteration body in memory or generates a standalone runner
- **THEN** the registered node receives a prepared body with the same selected output types and owns its invocation policy

### Requirement: Keep container policies above the runtime

The runtime SHALL manage scopes, input sources, node execution, type checks, publication, and budget consumption. `mfn-core` SHALL own Loop termination and Iteration scheduling, result collection, and failure policies. Sequential Iteration SHALL use the same sequential Loop driver as Loop.

#### Scenario: Reuse sequential execution

- **WHEN** a Loop repeats its body or an Iteration processes items sequentially
- **THEN** the node package uses the common Loop driver while supplying its own state, result, and termination policy

### Requirement: Discover bodies through registered declarations

Node providers SHALL be able to declare a subgraph with its body location, source identity, input types, selected outputs, binding options, and state-intrinsic permission.
Registered preparation and code generation SHALL validate and canonicalize the declared body independently of the container kind. The final standalone runner SHALL execute generated direct node calls.
Preparation artifacts SHALL be written separately from factory stdout, and cached artifacts SHALL only be reused for matching input plans.

#### Scenario: Execute a third-party container

- **WHEN** a registered third-party node declares a body with a custom synthetic source and repeats its prepared callback
- **THEN** the memory and standalone backends produce equivalent results without a compiler branch for that node kind

#### Scenario: Preserve warm-build sources

- **WHEN** an unchanged workflow is compiled again and preparation produces the same artifacts
- **THEN** generated source files remain unchanged and factory stdout does not corrupt preparation output

### Requirement: Let providers select scope observation

The runtime SHALL provide optional scope lifecycle and node invocation hooks. A scope without an observer SHALL NOT select a container observation protocol. Node packages SHALL own Loop pass and Iteration item protocol adapters. Custom scope lifecycle hooks SHALL run on completion, explicit exit, and failure even when no run observer is installed.

#### Scenario: Preserve built-in observation protocols

- **WHEN** an observed Loop pass or Iteration item executes a prepared body
- **THEN** the node package adapter emits the existing protocol events through the runtime hooks

#### Scenario: Observe a custom scope without a run observer

- **WHEN** a third-party provider installs a scope observer without a run observer
- **THEN** the observer receives the scope identity, index, visited steps, exit state, and failure outcome
