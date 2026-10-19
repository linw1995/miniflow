## Why

Local callers currently save a Flow plan and invoke a second command to execute it. An explicit execution option should complete both operations through `mf`, preserving the existing snapshot-bound consumer and CI coverage ownership.

## What Changes

- Add one `process.run` Task in the existing unpublished tooling package, with configured program/arguments, working directory, JSON stdin, bounded text/JSON stdout, cancellation, and failure propagation.
- Accept stdin plans in the existing Python consumer and keep its structured report separate from test diagnostics.
- Add a default-false `run` parameter and reuse existing conditional nodes after planning. Carry opaque execution context through the analyzer message boundary.
- Keep CI planning-only and reject `ci=true` with `run=true`.

## Capabilities

### New Capabilities

- `process-execution`: bounded owned process execution for developer tooling.

### Modified Capabilities

- `test-affected`: optional local execution after planning through the existing consumer.

## Non-goals

No new selection policy, nextest-specific node, inventory node, generic transport framework, runtime change, CI coverage runner, or release membership change.
