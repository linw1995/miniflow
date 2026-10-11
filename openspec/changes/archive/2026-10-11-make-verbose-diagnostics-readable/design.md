# Design

Local rendering consumes the same validated `WireRecord` passed to the OTel logger. A private diagnostics module owns
formatting and periodic stderr output. It does not install another observer or decode a second lifecycle protocol.
The caller buffers one line and writes it under the stderr lock, preserving concurrent line boundaries. Diagnostic
I/O failure remains independent of the workflow result.

Elapsed and node-duration times use milliseconds. Node IDs and error messages use escaped quoted text. Produced and
skipped ports, failure phase, nested one-based Loop paths, pass summaries, and full unsigned stream identifiers retain
their existing meaning. A run ID appears once on the workflow-start line. Workflow hashes, schema versions, and event
sequence routing attributes remain available in OTLP rather than recurring in local output.

Verbose observers own one worker thread, a mutex, and a condition variable. Node-start records populate an active-node
map keyed by workflow, run, node, Loop path, and stream invocation. Each node has a monotonic 60-second deadline;
completion or skip removes that entry. The worker sleeps until the next deadline, writes a `still running` line with
the existing node context, and schedules the next reminder without replaying missed intervals. A dedicated thread
also works while synchronous plugin code blocks the execution thread and requires no export SDK or async runtime.
These local reminders do not fabricate lifecycle records, spans, or OTLP messages.

Observation completion clears remaining entries even when exhausted lifecycle counters prevent a terminal record.
Abandoned finite and streaming observation scopes also clear their run.
Dropping the backend closes and wakes the worker; it holds only the shared reminder state, preventing an ownership
cycle. Shutdown does not join a worker that could be blocked by diagnostic stderr backpressure. The worker rechecks
active entries after obtaining the stderr lock, so reminders cannot follow the corresponding completion line.
Quiet observers allocate no reminder state and start no worker.

Two focused regressions check compact output/control-character escaping and nested Loop/large stream identity context.
Two reminder regressions advance monotonic timestamps without real sleeps to check repeated deadlines, removal on
completion, identity isolation, abandoned scopes, and completion without a terminal record. Existing actual-runner acceptance checks human-readable status boundaries
while continuing to validate unchanged OTLP records and result stdout. A real slow-node run checks the worker's timed
wakeup and telemetry-disabled output without introducing a minute-long automated test.
