## Why

The compiler currently derives child scope semantics by matching provider kind strings. `loop.run`,
`builtin.iteration`, and `workflow.call` are recognized by name, so a provider cannot declare a Loop
or item frame under its own kind, and compiler constants leak authoring syntax into execution
semantics. Kind strings should only select a factory; the receiving provider should declare which
frame its child Flow executes in.

## What Changes

- Add `SubgraphScope` with `Host`, `Loop`, and `Iteration` variants.
- Require `scope` on `NodeFactory::Subgraph`; `None` marks an ordinary child Flow value instead of a frame.
- Expose `NodeRegistration::subgraph_scope` and `NodeRegistry::subgraph_scopes`.
- Derive `%loop`, `%iteration`, and host selectors from provider metadata during lowering instead of kind matching.
- Register `loop.run` as `Loop`, `builtin.iteration` as `Iteration`, `workflow.call` as `Host`, and `builtin.constant` as `None`.
- Thread declared scopes through discovery, inline resolution, host-binding canonicalization, and generated plans.
- Keep `loop.exit`, `%loop`, `%iteration`, and `%root` as internal language controls; they are not subgraph factories.

## Non-goals

No change to authoring reference syntax, Loop or Iteration runtime behavior, compiled artifact
shape, or the reserved source instructions. No aliases or compatibility shims for providers that
omit `scope`.

## Capabilities

### New Capabilities

- `provider-subgraph-scopes`: provider-declared child frame semantics consumed by the compiler.

### Modified Capabilities

None.

## Impact

`NodeFactory::Subgraph` is a breaking provider API change. Compiler lowering, discovery, host-binding
canonicalization, generated preparation, built-in node registrations, test fixtures, and node
development documentation change. Workflow JSON is unaffected.
