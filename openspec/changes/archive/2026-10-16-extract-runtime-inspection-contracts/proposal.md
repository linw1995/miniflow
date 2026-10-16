## Why

Hosts need reliable workflow inspection without executing providers. The compiled definition already contains node
configuration, and graph/interface views can be derived from it plus the execution orders and provider input contract.
Persisting those views again duplicates data and creates avoidable consistency relationships.

## What Changes

- Store definition, frozen root/Loop execution orders, and prepared input schema in manifest `2026-10-16`.
- Derive graph description, interface, and workflow identity through runtime helpers; borrow node definitions and schema.
- Share graph projection between compiler and runtime while keeping all execution-order planning in the compiler.
- Use the helpers directly in generated runners and preserve legacy manifest reading through a versioned representation.
- Reuse the existing template evaluator and unique-key JSON decoder for host callers.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-binary-compilation`: persist only inspection facts and derive compatible views without providers or replanning.

## Impact

Runtime manifest API, compiler generation, telemetry error context, TUI callers, tests, and documentation.
Rust callers construct versioned manifest variants and use helper methods instead of graph/interface fields.
The new payload contains full compiled configuration; legacy serialized manifests remain readable.
