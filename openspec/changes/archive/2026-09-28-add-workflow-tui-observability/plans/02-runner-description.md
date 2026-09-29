# Plan 2: Embedded Runner Description and Bounded Preflight

## Scope and Acceptance

`--describe` produces one complete JSON graph description from the embedded compiled plan without constructing or executing plugin nodes. This metadata is required to initialize the session; a malformed, missing, or truncated description prevents workflow launch. The acceptable-loss policy for live telemetry does not permit guessing an incomplete graph.

Target the repository's distributed Linux and macOS platforms. Description mode has no process-wide descriptor manipulation. The ordinary execution and validation modes retain their existing standard-stream behavior.

## Description Schema

Return a top-level object with a date-enum `version`, `workflow_id`, `nodes`, `data_edges`, `control_edges`, and `execution_order`. Each node has `id` and `kind`. Edges expose their named source and target ports. The description deliberately omits the full effective port table: unconnected ports, types, and required flags are unavailable to the TUI, not known to be absent. Reuse plan 1's identity algorithm.

Never include node configuration, predicate literals, or business values. Validate IDs, nonempty edge port names, duplicate entries, and complete execution order on CLI ingestion. Array order must be deterministic. Reject unsupported versions before spawning an execution process. Normal compilation validation still constructs plugins and verifies effective port contracts; description mode does not repeat that work.

Initial implementation limits: at most 16 MiB of description JSON and a 30-second description-process deadline. Both are named policy constants, covered by tests and documented for users; exceeding either is a preflight error. Limit graph/event allocations using this validated description and checked arithmetic.

## Machine-Output Channel

Dispatch `--describe` before registry initialization. Parse the embedded plan, construct the owned graph description, and serialize it within the size limit before writing one JSON document plus a newline to stdout. Do not invoke plugin factories, destructors, metadata callbacks, or node execution. Check writes and flushes. Serialization failure returns nonzero without writing a partial document.

Linked native startup code running before `main` remains outside this guarantee. Unexpected stdout bytes or a partial write must cause a clear preflight error; do not use brace scanning or diagnostic-prefix removal. Validation mode continues to construct plugins and report its usual diagnostics separately from description mode.

## CLI Collection and Cleanup

Launch description mode with null stdin and a dedicated process group or Windows Job Object. Read bounded stdout concurrently with draining stderr; retain only the diagnostic tail and show a truncation indicator if it overflows. Accept the description only after successful child exit and bounded stream completion. On overflow, timeout, invalid JSON, or nonzero exit, report a preflight failure and clean up the owned process group before returning.

Use the ownership and deadlines from plan 3 for pipe/child cleanup. Do not enter the terminal's alternate screen until description preflight succeeds. No OTel execution provider is initialized for description mode.

## Implementation Order and Evidence

1. Define the date-versioned JSON DTO and fixtures; verify canonical identity agreement and graph relationships without claiming a full port table.
2. Build the owned description directly from the embedded plan; verify zero factory and execution calls and no configuration fields.
3. Dispatch before registry construction; verify factory diagnostics appear only during validation, not description.
4. Add bounded CLI collection; verify noisy output beyond pipe capacity, invalid versions, oversized output, descendant-held pipes, and partial writes have deterministic outcomes.
5. Compile once, invoke validation and description on that exact binary, then move it away from build inputs and repeat. Confirm no second compilation or TUI dependency is introduced.
