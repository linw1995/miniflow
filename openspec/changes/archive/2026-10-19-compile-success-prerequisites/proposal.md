## Why

Task-success edges currently add runtime dependency dispatch and a per-frame outcome map. The compiler already validates task ordering and can derive activation from predecessor input and output gates. Generated runners should emit these conditions directly, and in-memory preparation should bind the equivalent executable predicates.

## What Changes

- Keep success edges in compiler analysis, scheduling, descriptions and diagnostics.
- Compile each success prerequisite from its source activation condition, including exclusive input alternatives and transitive success prerequisites.
- Emit static Rust guards for generated task, body and stream execution; bind equivalent predicates during in-memory preparation.
- Remove runtime NodeSuccess dispatch, task outcome storage, missing-outcome errors, and outcome-specific copying and tests.
- Preserve error-before-skip precedence, typed transfers, startup eligibility, cancellation, and publication-before-domain-completion.

## Capabilities

### Modified Capabilities

- `explicit-execution-dependencies`: compiler-derived activation replaces runtime outcome lookup.

## Impact

Compiler execution plans and generated code, runtime execution hooks, body and stream execution, tests and documentation. Wire formats and user-visible success/skip semantics remain unchanged.
