## Decisions

The executable is treated as a finite JSON workflow. Chat binds unambiguous prompt/system/history inputs and
validates status, text, metadata, usages, and error values. Internal node kinds, counts, loop bodies, and
output source nodes are not admission rules. The whole workflow executes through the ordinary runner.

Each LLM attempt returns a unique call ID, its resolved public model, optional independent token counters,
and node/loop identity when executed in a workflow. Workflows explicitly combine usage lists with
`builtin.concat`; repeated calls are not collapsed by node identity. Duplicate call IDs in a result fail
validation. No collection state is installed in the runtime.

The runtime exposes only the currently executing node, with nested restoration on errors and unwinding.
Existing scope paths identify loop invocations. The feature is available without observation and applies
to interpreted and generated task calls through the same invocation boundary.

LLM configuration may opt into `capture_errors`. Request failures, node deadlines, and response validation
failures then produce failed status and a usage entry. Valid counters survive response conversion failure;
missing totals are never derived from other counters. Default failure behavior retains typed error chains.
Cancellation, invalid setup, and abrupt process exit may prevent a final result and remain unreported.

Message metadata is an optional object. It is preserved in history but is not automatically sent to providers.
Reply metadata belongs only to the newly saved assistant message. Returned history never rewrites saved
messages. Metadata is included in canonical history and response byte limits.

One durable call row continues to identify a workflow execution. Its optional usages array records actual
model calls; a SQLite view expands those records for aggregation. Legacy rows without a usages list retain
their existing facts. An explicit empty list means no reported model calls. A missing list with no known
single-model identity contributes to unreported_invocations rather than an invented model call count.

Assistant metadata, response usages, and successful state commit together. A failed result saves its usages
and failed state without an assistant message. Save retries preserve the original result and do not invoke
models. The database migration retains old rows and indexes; rollback remains a disposable-development tool.

A shared typed SQL JSON adapter keeps maps and lists as typed public values while preserving serialization
error sources. Existing scalar usage fields remain a single-call compatibility surface, and cannot conflict
with a simultaneously supplied usages list.
