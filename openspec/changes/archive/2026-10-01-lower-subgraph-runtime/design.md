## Context

The compiler already shares graph validation, normalization, inference, and body generation. The remaining architectural duplication is the separate runtime execution models for item contexts and Loop frames, coupled with runtime-owned container policies.

## Decisions

1. `ExecutionScope` carries scope identity, source identity, index, typed values, and execution control state. `ExecutionContext::run_scope` isolates outputs and restores the parent scope on success, failure, and unwinding. A fresh body context starts a fresh budget; a scope on the parent context inherits its budget.
2. `PreparedSubgraph` owns node identities, selected output types, and a body callback. The callback supports either an in-memory `Flow` or generated direct node calls.
3. `Node::with_subgraph` lets the registered provider construct its executor from a prepared body. The default rejects binding; ordinary node factories keep their existing signature.
4. `mfn-core` owns Loop termination and Iteration concurrency, error policies, and result collection. Sequential Iteration and Loop use the same `run_loop` driver. Runtime assignment and exit intrinsics retain their protected access to scope state.
5. Loop completion metadata travels in `NodeResult`; the runtime publishes it without selecting a stop reason.
6. `Node::subgraph_definition` declares the body, its JSON pointer, synthetic source, typed inputs, selected outputs, binding options, and permission to use state intrinsics. Registered preparation and generation consume this contract for built-in and third-party providers. The compiler validates and canonicalizes each body, then binds it through `Node::with_subgraph`.
7. Standalone compilation uses the existing runner's internal preparation mode to load the selected registry and write generated artifacts to a file. The final runner contains direct node calls. Preparation output is separate from stdout because factories may print. Cached prepared artifacts are reused only for matching input plans, preserving unchanged source files on warm builds.
8. `ScopeObserver` receives scope lifecycle and node invocation hooks. Scopes do not select an observation protocol by default. `mfn-core` supplies Loop and Iteration adapters that preserve existing wire events; third-party providers may supply their own hooks, including when no run observer is installed.

## Compatibility

Existing workflow definitions and wire protocols retain their behavior. Nested Loop budgets remain shared; Iteration item budgets remain independent. A registered container owns body declaration and prepared-body binding.
Ordinary node factories keep their signature and use the default no-subgraph implementation. Public planning and generation without a registry retain built-in compatibility adapters; registered preparation and final generation use provider declarations. Runtime assignment and exit intrinsics keep protected mutation access.

## Validation

Reuse the existing Loop, Iteration, generated-runner parity, registration, and observation tests. A scope-lifecycle test covers nested failure, unwinding, parent-output restoration, and inherited budget consumption.
A third-party container fixture verifies declared bodies, memory/generated parity, factory stdout, direct calls, and unchanged warm-build source files. An observer test covers completion, explicit exit, and failure without a run observer.
Keep the concurrency test with the node package that now owns the implementation. Run the repository hooks, targeted coverage, and Nix checks before archival.
