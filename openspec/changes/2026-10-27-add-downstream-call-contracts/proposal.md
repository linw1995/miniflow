## Why

An upstream node cannot currently discover the input contract of a connected downstream node during
execution. Future LLM tool dispatch needs that contract, but a tool may also receive inputs from
constants or configuration services. Exposing every target input to the caller would misrepresent
parameter ownership and permit callers to override externally supplied values.

## What Changes

- Expose immutable, scope-local downstream interface views through the execution context.
- Resolve parameter ownership from explicit port connections first.
- Treat a node-level invocation connection as a default binding for remaining target inputs.
- Project each caller's parameter interface from the resolved bindings, preserving types, requiredness,
  and public descriptions without exposing internal configuration or values.
- Define a separate invocation contract for atomic argument submission and runtime composition with
  inputs from other producers. Ordinary data and control connections retain their existing behavior.
- Stage implementation so generic discovery and binding resolution precede runtime invocation and
  provider-specific LLM ToolUse support.

## Impact

The eventual implementation affects workflow definitions, compiler preparation and generated plans,
runtime invocation contexts, interface reflection, and observation. LLM protocol changes are a later
integration phase. The implementation provides the runtime's call/output model, compiler-backed
ownership resolution, and validation/remapping of caller-owned arguments. It reuses ordinary node
preparation, type compatibility, and exclusive-branch proofs. Scheduling, context discovery, and
workflow-format changes remain subsequent phases.

Runtime topology mutation, dynamically loaded providers, arbitrary cross-scope discovery, and stream
or event nodes as callable tools are outside the initial scope.
