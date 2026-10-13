## Context

See `proposal.md` for motivation. Streaming lifecycle schema 4 already supplies run, invocation, trigger, parent, and message identities. Node spans and nested Loop/Iteration spans provide native correlation. These records contain metadata rather than business values.

Finite snapshots use a run-wide value dictionary and persistent history. `StreamOptions.snapshots` and generated `execute_stream` reject that recorder; the TUI clears `MF_CAPTURE_SNAPSHOTS` and its Data preview short-circuits for streams. Removing these guards would retain values indefinitely and would not model a producer emitting repeatedly before its span ends.

Relevant implementation boundaries are `mf-runtime/src/context.rs`, `mf-runtime/src/stream_instance.rs`, `mf-telemetry/src/stream_observation.rs`, `mf-telemetry/src/otlp.rs`, `mf-compiler/src/dependency_project.rs`, and the existing TUI receiver, snapshots, and traces modules. Follow their module boundaries and error ownership when adding support.

## Goals / Non-Goals

Goals are independently inspectable input and emission records, bounded memory at every stage, live data before producer completion, and correlation that survives concurrent execution and transport reordering.

This change does not introduce whole-stream persistent snapshots, replay, remote attachment, automatic payload export in verbose diagnostics, or a new result protocol. It does not capture arbitrary internal node state or invent values for skipped ports. Large values can be explicitly omitted; full inspection is guaranteed only for records within capture limits.

## Decisions

### 1. Add a separate capture mode

Introduce `MF_CAPTURE_STREAM_DATA=1` and an optional runtime-owned stream data recorder on `StreamOptions`. Keep `MF_CAPTURE_SNAPSHOTS` and the existing `snapshots` field restricted to finite operations. TUI streaming launches set the new flag and clear finite capture; finite launches do the inverse. Ordinary runner invocation allocates no data recorder unless explicitly requested.

Capture uses the existing stream observation identities, not a second invocation counter. A programmatic recorder requires a matching stream observation; invalid pairing is a configuration error before startup. Generated runners establish observation when capture is requested and an OTLP logs exporter is available. Missing export support produces an actionable capture diagnostic and preserves business execution. Local verbose observation alone does not export values.

Alternative: reinterpret the existing finite snapshot flag and recorder. Rejected because their run-wide storage and complete-history contract are inappropriate for an unbounded instance and old callers must retain their existing behavior.

### 2. Publish self-contained data records

Use a separate `mf.stream.data` instrumentation scope and version 1 envelope. Keep lifecycle schema 4 and finite snapshot version 1 unchanged. Data records contain workflow/run identity, a checked capture sequence, timestamp, record kind, node/scope identity, existing stream invocation identity, and native trace/span IDs when available. An emission adds its actual output-domain message identity and invocation-local emission ordinal.

Record kinds are capture start, resolved inputs, admitted outputs, bounded capture status, and capture end. Input and output records each carry a port object.
The value depth limit of 48 leaves room for typed wrappers inside the JSON decoder's existing recursion budget. Encode values inline as tagged null/bool/number/string/array/object trees, with decimal number text preserving unsigned integers and integer/float distinctions. No value ID refers to an earlier record. Reuse compatible scalar/value codec logic without routing stream data through the persistent `SnapshotStore`.

Reserve capture sequence numbers before encoding or queue admission; dropped records leave evidence. Capture status and end records report the last reserved sequence and cumulative omission/drop counts without per-invocation history.
They cannot prove successful remote delivery. Receivers report gaps within their bounded verification window and an unverified tail when the end record is absent. Checked counter exhaustion disables capture with a retained diagnostic, never wraps identities or fails the workflow.

Alternative: reuse the finite value dictionary and fragment assembler. Rejected because evicting a definition can make all future records undecodable, and global contiguous assembly can block new data behind an old missing record. Each stream record is admitted independently.

### 3. Capture execution and publication at their existing boundaries

For synchronous tasks, capture resolved inputs immediately before invocation and published outputs after validation succeeds. Carry the recorder and existing stream identity through execution contexts, including nested Loop and Iteration work. If an optimized execution path cannot expose intermediate values, use its existing dynamic fallback while capture is active; disabled capture preserves the fast path.

For producers, capture inputs on the producer worker with its attached node context. Each successful `Emitter::send` captures a distinct output record after validation and message allocation. The record is observable while that invocation is still running. A producer that later fails retains its earlier admitted outputs; unsuccessful sends produce no admitted-output record.

For event callbacks, capture resolved input ports for input triggers, including zero-output Batch buffering. Startup, timer, and upstream-close callbacks have their actual trigger and no invented input message. Capture each emission as it is successfully admitted, even if a later emission or timer update fails. Batch output values belong to the flush callback and its output-domain message; flush reason remains lifecycle metadata.

Reserve emission identity/sequence at the runtime admission boundary, then serialize and enqueue outside shared runtime/coordinator locks. Snapshot the immutable value references and native context needed for this bounded handoff. Do not perform network I/O while holding those locks. Preserve each existing commit boundary and avoid an extra shared mutable history.

Use native trace/span association for a selected node span and explicit stream identities for validation and session browsing. Missing context stays unassociated. Repeated node names, Loop pass numbers, and Iteration indices must never be used as fallback correlation keys.

Alternative: collect all outputs at invocation finish. Rejected because a long-lived producer might never finish and can emit an unbounded number of messages.

### 4. Bound the complete capture pipeline

Initial implementation budgets are listed below. They are defaults to verify with representative workloads; changing them does not change record semantics.

| Stage | Initial limit | Behavior at the limit |
| --- | --- | --- |
| Encoding | 256 KiB per complete encoded record; value depth 48 | Emit a bounded omission record identifying the affected input/emission; never allocate an entire oversized serialization first. |
| Producer handoff/export | 128 queued records and 2 MiB total encoded bytes; 2 MiB per export batch | Nonblocking admission drops additional data, records cumulative drops, and preserves business backpressure. |
| TUI data store | 4,096 records and 16 MiB accounted decoded/encoded storage | Evict oldest data by capture sequence, release their values and indexes, and count local evictions. |
| Deduplication/integrity | 4,096 recent record witnesses and bounded gap ranges | Ignore ancient retransmissions; expose retired verification and unresolved evidence as uncertainty. |
| Presentation | 64 KiB value preview; bounded row labels and diagnostics | Mark preview truncation and escape terminal control characters. |

Budget object/key/string/index overhead, not only wire-body lengths. Data admission must not pass through the generic raw-log preview decoder's shallow truncation rules.
The dedicated decoder applies its own byte/depth limits and returns typed errors. Do not add fragmentation initially: a record within the cap fits the existing 8 MiB OTLP request ceiling, while oversized values have an explicit omission contract. Account OTLP framing in the batch cap so combining records cannot exceed the receiver request limit.

The dedicated data exporter must not inherit the finite snapshot exporter's synchronous flush-every-64 behavior. Queue pressure and failed export remain capture diagnostics. Periodic bounded status records and the final best-effort end record expose sender-side loss when delivery permits. Final flush uses existing finite shutdown deadlines after workers have joined.

Alternative: retain every payload or block a producer until telemetry is delivered. Rejected because observation would become a second business queue with different capacity and shutdown semantics.

### 5. Render a selectable list in the existing Data pane

For a selected stream node span, list only its exact trace/span-correlated input and output records, ordered by capture sequence rather than OTLP arrival time. A row shows input/output kind, port names, trigger, applicable message identity, emission ordinal, timestamp, and bounded summary.
Selecting it shows the decoded port object or omission reason. Multiple emissions stay separate rows. Outcome, skipped ports, and errors remain derived from existing execution metadata rather than inferred from the presence of data.

Provide a session-wide Data scope beside the existing session log scope so live producer data is inspectable before its span exports, and data remains accessible when spans are sampled out or evicted. Retained uncorrelated rows are visible there without being assigned to an arbitrary span. New rows appear as they arrive and are ordered by sequence; the UI never waits for a missing earlier record to render later data.

Selection follows record identity during insertions and reordering. Follow the newest record until the user navigates away; resuming follow returns to the tail. If the selected record is evicted, show a clear eviction notice and move to the nearest retained record. Refresh selected data and capture status from current state, following the existing mutable-preview design.

Present capture availability, value omissions, producer drops, retained transport gaps, local history eviction, and decode/conflict failures separately from lifecycle completeness and raw log/span drops. End-of-session inspection keeps all remaining rows. Finite Data keeps its current invocation snapshot presentation.

### 6. Preserve compatibility

The new flag and independent scope are additive. An older schema-4 runner can execute in the TUI and still provide Details/Logs; absent capture-start evidence yields an unavailable/unsupported diagnostic, never an empty-success claim. Unknown stream data versions affect Data only. Selected finite Tasks from stream binaries continue to use finite snapshots and their selected-operation identity. Whole-stream finite snapshot requests continue to fail before execution.

## Risks / Trade-offs

- Repeated values consume more wire bytes than dictionary snapshots. Bounded, independent records avoid historical dependencies; measure producer overhead before optimizing compression.
- Serialization can cost CPU even with bounded queues. Skip serialization when queue capacity is already exhausted and use a capped encoder with a depth limit.
- Telemetry can lose both values and its own diagnostics. Retain local typed capture failures, expose missing start/end evidence, and avoid claiming complete data from process success.
- Data above the record limit is omitted rather than available in full. Identify the affected ports/emission and keep lifecycle and other received values usable.
- Capture can expose business values. Keep it explicitly enabled and separate from automatic metadata, using the existing TUI child-only endpoint isolation.

## Migration Plan

Land the protocol/codec and bounded exporter first, then runtime hooks, generated runner configuration, receiver storage, and list UI. Add tests and documentation within each stage. No existing manifest or lifecycle version requires migration. Recompile a runner to gain stream data capture; existing executables retain their current execution and observation support. Disabling the new flag restores metadata-only stream observation.
