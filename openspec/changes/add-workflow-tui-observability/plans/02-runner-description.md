# Plan 2: Runner Description and Output Isolation

## Scope and Acceptance

`--describe` produces one complete JSON graph description without calling node execution. This metadata is required to initialize the session; a malformed, missing, or truncated description prevents workflow launch. The acceptable-loss policy for live telemetry does not permit guessing an incomplete graph.

Target the repository's distributed Linux and macOS platforms. Keep descriptor handling in the runner entry path; do not redirect process-wide descriptors from the reusable runtime library. The ordinary execution and validation modes retain their existing standard-stream behavior.

## Description Schema

Return a top-level object with `schema_version`, `workflow_id`, `nodes`, `data_edges`, `control_edges`, and `execution_order`. Each node has `id`, `kind`, and effective `inputs`/`outputs` with `name`, `value_type`, and `required`. Ports are resolved independently for each configured instance. Reuse plan 1's identity algorithm.

Never include node configuration, predicate literals, or business values. Validate IDs, endpoints, duplicate entries, and complete execution order on CLI ingestion. Array order must be deterministic. Reject unsupported versions before spawning an execution process.

Initial implementation limits: at most 16 MiB of description JSON and a 30-second description-process deadline. Both are named policy constants, covered by tests and documented for users; exceeding either is a preflight error. Limit graph/event allocations using this validated description and checked arithmetic.

## Machine-Output Channel

Use the runner's original stdout as the public JSON channel. In description mode, before registry initialization or factory invocation:

1. Duplicate the original stdout descriptor into an owned descriptor with close-on-exec enabled.
2. Redirect descriptor 1 to descriptor 2. Plugin stdout and stderr now both go to the diagnostic stream.
3. Prepare instances using the same registry/configuration/port validation rules as validation mode. Build an owned description detached from plugin instances.
4. Serialize the complete description into a size-limited buffer before writing it to the saved stdout descriptor.
5. Write one JSON document plus a newline through that descriptor and close it. Keep ordinary stdout redirected until process exit, including plugin destruction and buffered diagnostic flushes.

Never restore stdout to the JSON pipe during this one-shot mode. A constructor/destructor that prints after metadata preparation must not append text to the description. Do not use brace scanning, diagnostic-prefix removal, or a second runner compilation to obtain clean metadata.

Check every descriptor and write operation. If setup fails, fail before constructing plugins. Preparation/serialization failure returns nonzero and emits no JSON document. A partial I/O write followed by failure is rejected by the CLI because both a successful exit and exactly one valid complete document are required.

This handles ordinary plugin output through inherited stdout/stderr after entry into description mode. Arbitrary native startup code running before `main` or plugins that replace descriptors themselves are outside that guarantee; extra bytes must cause a clear preflight error rather than heuristic recovery.

## CLI Collection and Cleanup

Launch description mode with null stdin and a dedicated process group. Read bounded stdout concurrently with draining stderr; retain only the diagnostic tail and show a truncation indicator if it overflows. Accept the description only after successful child exit and bounded stream completion. On overflow, timeout, invalid JSON, or nonzero exit, report a preflight failure and clean up the description process group before returning.

Use the ownership and deadlines from plan 3 for pipe/child cleanup. Do not enter the terminal's alternate screen until description preflight succeeds. No OTel execution provider is initialized for description mode.

## Implementation Order and Evidence

1. Define the versioned JSON DTO and fixtures; verify canonical identity agreement and distinct dynamic ports for two instances of the same kind.
2. Extract owned description construction from the embedded plan and existing validation preparation path; verify zero execution calls and no configuration fields.
3. Add the entry-point descriptor guard for Linux/macOS and dispatch it before registry construction; verify constructor, destructor, and native stdout writes go to diagnostics.
4. Add bounded CLI collection; verify noisy output beyond pipe capacity, valid JSON-looking diagnostics, invalid versions, oversized output, hanging constructors, and partial writes all have deterministic outcomes.
5. Compile once, invoke validation and description on that exact binary, then move it away from build inputs and repeat. Confirm no second compilation or TUI dependency is introduced.

## References

- [Descriptor duplication and redirection](https://www.man7.org/linux/man-pages/man2/dup2.2.html)
- [Rust Unix child process groups](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html#tymethod.process_group)
