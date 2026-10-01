## REMOVED Requirements

### Requirement: Discover bodies through registered declarations

**Reason**: Automatic third-party discovery and standalone generation are deferred. Their preparation phase and artifact cache are unnecessary for current Loop and Iteration workflows.

**Migration**: Retain explicit `PreparedSubgraph` binding and the shared execution scope API. Compile existing Loop and Iteration bodies through the shared compiler path.

### Requirement: Let providers select scope observation

**Reason**: The scope observer extension framework has no required caller beyond built-in protocol adapters.

**Migration**: Use existing Run/Body observation contexts and publish container lifecycle events from node implementations.

## ADDED Requirements

### Requirement: Keep container lifecycle observation in node implementations

Node implementations SHALL own Loop pass and Iteration item lifecycle publication. The runtime SHALL propagate existing Run and Body observation contexts and scoped invocation identities during node execution.

#### Scenario: Preserve built-in observation protocols

- **WHEN** an observed Loop pass or Iteration item executes a prepared body
- **THEN** node lifecycle publication and runtime context propagation preserve the existing protocol events

## MODIFIED Requirements

### Requirement: Keep container policies above the runtime

The runtime SHALL manage scopes, input sources, node execution, type checks, publication, and budget consumption. `mfn-core` SHALL own Loop termination and Iteration scheduling, result collection, and failure policies. Loop and Iteration SHALL share prepared-body execution through managed runtime scopes.

#### Scenario: Reuse sequential execution

- **WHEN** a Loop repeats its body or an Iteration processes items sequentially
- **THEN** both execute prepared bodies through runtime scopes while supplying their own state, result, and termination policy
