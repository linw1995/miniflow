## Why

Hosts need reliable workflow inspection without executing providers. The compiled definition already contains node
configuration, and graph/interface views can be derived from it plus the execution orders and provider input contract.
Persisting those views again duplicates data and creates avoidable consistency relationships.

## What Changes

- Store definition, frozen root/Loop execution orders, and prepared input schema in manifest `2026-10-16`.
- Derive graph description, interface, and workflow identity through runtime helpers; borrow node definitions and schema.
- Share graph projection between compiler and runtime while keeping all execution-order planning in the compiler.
- Use the helpers directly in generated runners and remove pre-release manifest compatibility branches.
- Reuse the existing template evaluator and unique-key JSON decoder for host callers.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `workflow-binary-compilation`: persist only inspection facts and derive inspection views without providers or replanning.

## Impact

Runtime manifest API, compiler generation, telemetry error context, TUI callers, tests, and documentation.
Rust callers construct the current manifest struct and use helper methods for derived graph/interface views.
The new payload contains full compiled configuration; older manifests require recompilation.
Execution descriptions derive their event protocol instead of storing a redundant version field.
