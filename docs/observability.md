# Workflow observation contracts

## Availability and package boundaries

`mf-telemetry` provides versioned descriptions, lifecycle events, workflow/run identities, sequence reservation, validation, and mapping into OpenTelemetry log records. Shared runtime instrumentation and generated execution accept caller-owned observations. Compiled runners embed graph and interface records and initialize OTel export only when an endpoint is configured.

`mf-tui` contains the local OTLP receiver, state reducer, graph renderer, and CLI-only process supervisor used by `mf run <executable> --tui`.

`mf-runtime` depends on the minimal telemetry package. `mf-cli` depends on the separate `mf-tui` package for bounded description preflight and local reception. Neither the compiler nor runtime depends on `mf-tui`. Generated runners enable OTLP export support by default, while `mf compile --no-telemetry` omits the SDK and HTTP client. Neither runner includes terminal dependencies or the compiler's code generation feature.

The `mf-telemetry/otlp` feature selects the OTel SDK and OTLP/HTTP protobuf exporters with an explicit blocking HTTP client and TLS support. Enabling the feature alone does not initialize an exporter. The generated runner owns provider initialization and bounded shutdown; libraries do not install global providers for callers.

```sh
nix develop --command cargo check -p mf-telemetry --no-default-features
nix develop --command cargo test -p mf-telemetry --all-features
nix develop --command cargo test -p mf-compiler --test telemetry_boundary
```

## Observing execution with caller-owned providers

`Observer::new(&traces, &logs)` obtains workflow instruments from application-owned OTel providers without installing globals. `CompiledWorkflow::start_observation(&observer, run_id)` computes the canonical workflow identity, creates one run scope, and emits `mf.workflow.started`. Keep the providers alive for the entire run and flush/shut them down in the application after either success or handled failure.

Use `execute_compiled(&plan, &registry, Some(observation))` to cover node construction, dependency resolution, invocation, publication, and output selection in memory. `Flow::execute_with_observation(Some(observation))` covers execution of nodes that have already been constructed; it cannot report construction that happened before the run. Existing `Flow::execute()` and generated `run_workflow()` remain unobserved entry points.

Source from `CompiledWorkflow::generate_artifacts()` provides `prepare_workflow(registry, observation)` to construct a validated Flow and
`run_workflow_with_observation(&flow, Some(observation))` to execute it. Construction errors use
`WorkflowBuildError`; execution errors use `WorkflowRunError`. For applications that need
registry initialization inside the run, use `ExecutionContext::run(Some(observation), |state| ...)` and call generated
`run_workflow_in_context(&flow, state)` or `Flow::execute_in_context(state)`. These low-level context entry points
execute one matching plan per fresh scope; the surrounding scope owns the terminal event. An early error before a node
is identified becomes a workflow preparation failure.

Standalone projects use `CompiledWorkflow::generate_runner_artifacts()` to emit only the entry points their runner calls:
`prepare_workflow` and `run_workflow_in_context` for task workflows, or `prepare_stream` for streaming workflows.
Custom runners that need the convenience functions above can use `generate_artifacts()` instead.

The runnable example uses in-memory SDK exporters and performs no network export:

```sh
nix develop --command cargo run -p mf-compiler --example observe
```

Expected output:

```text
mf.workflow.started
mf.node.started
mf.node.finished
mf.workflow.finished
spans: 2
{"answer":42}
```

See `crates/mf-compiler/examples/observe.rs` for provider ownership and cleanup. The example's SDK dependencies are development dependencies and do not enter standalone runner dependency graphs.

### Spans, events, and failure boundaries

A workflow span covers its run scope. Each reached top-level node gets a child span under that workflow, including dependency failures and conditional skips. In Loop-capable runs, body-node spans are children of their enclosing Loop-node span. `mf.node.started` is emitted immediately before invoking the implementation;
node success is emitted only after output validation/publication. Construction failure can emit a failed node record
without a start. Skips have an explicit skipped outcome and do not set error status. Root output-selection failure
leaves successful node outcomes intact.

Lifecycle LogRecords go directly to the dedicated OTel logger, independently of span sampling and diagnostic
`event_enabled` checks. They can arrive while node spans remain open. Use an unfiltered lifecycle processor pipeline;
caller-supplied processors/exporters can still discard data, and those losses remain visible through sequence gaps or a
missing final boundary. Use background batch processors for network export because instrumentation invokes the
configured OTel processors synchronously. Custom processors must be nonblocking and must not panic.

The run context is active during preparation/execution and a node context is active during each shared step. A plugin using a caller-provided OTel tracer can start a child span with the ordinary current-context API inside `TaskNode::execute`. The library does not choose plugin tracers or propagate context into plugin-created threads; plugins must explicitly attach a captured context there. Context guards restore the prior caller context on return or unwind.

No library call shuts down the application's providers. Panic/unwind preserves the original panic and ends held spans without fabricating a workflow finish or successful node outcome. An abandoned scope therefore has no terminal boundary. Handled errors retain the original execution result; dropped or unencodable telemetry never becomes a workflow error.
OTel lifecycle events contain execution metadata. Optional input/output history uses the snapshot events described below;
it does not enable workflow replay or retry scheduling.

## Workflow and run identity

A workflow identity is `sha256:` followed by 64 lowercase hex digits. Hash compact UTF-8 JSON of an object containing
`definition`, `execution_order`, and `identity_version: 1`. Recursively sort object keys and preserve array order. Use
serde_json's number and string encoding without a trailing newline. For example, integer `1` and floating-point `1.0`
retain different encodings. The input is the serialized compiled definition, including configuration and defaults,
rather than the raw source file's spelling. It does not identify plugin binary contents.

`WorkflowId::from_definition` implements this contract. Golden input and an independently computed SHA-256 digest live in `crates/mf-telemetry/tests/fixtures/identity-*`. Format changes require a new identity format version. A hash of low-entropy configuration is not a confidentiality boundary.

Each invocation has a fresh canonical lowercase UUID v4 `RunId`, independent of trace identity. For the original flat protocol, a node has one lifecycle per `(run_id, node_id)`. In Loop-capable runs, each actual invocation is identified by `(run_id, loop_path, local_node_id)`; repeated passes are not retries. There is no workflow retry policy. Definition IDs and port names are opaque strings and are never split on punctuation.

## Description schema

Description version `2026-09-27` contains `workflow_id`, `nodes`, `data_edges`, `control_edges`, and
`execution_order`. Version `2026-09-29` adds `loop_bodies`: each entry has a static path of
enclosing Loop IDs and a local graph with the same node and edge metadata. The synthetic `%loop`
source appears in its body graph. Nodes contain `id` and `kind`; edge endpoints carry connected port
names. The graph record excludes Loop configuration, variable values, predicates, and ordinary node
configuration. The graph does not include the full effective port table. Compile validation checks
those contracts by constructing plugin instances.

Version `2026-10-03` adds `execution` metadata with the execution mode and lifecycle schema, and requires
interface inspection. The manifest interface record exposes initial-node input types,
required flags, and runtime resource declarations. Inspection reads the embedded manifest without constructing providers.

New runners name the synthetic Loop source `%loop`. Description readers also accept `$loop` from
previously compiled runners, preserving its original node IDs and workflow identity.

`WorkflowDescription::from_json` rejects unsupported versions, oversized input (16 MiB), duplicate IDs, incomplete
execution order, missing endpoints, empty edge port names, backward edges, and duplicate input/control bindings.
`to_json` validates before encoding. Public structs can be assembled by callers; validate them before use. Additive
unknown fields are accepted within the current version and omitted when re-encoded. No description field carries node
configuration, predicate values, or business inputs/outputs.

Graph-relative event validation checks workflow/node identity, position, sequence bounds, and skip causes against known edges. It checks reported produced/skipped port names for nonempty uniqueness, but cannot prove that they enumerate every output or match unconnected dynamic ports. The runtime validates its own effective ports before emitting events; the TUI keeps unavailable port metadata distinct from a missing lifecycle record.

## Embedded manifest format

Manifest payload version `2026-10-07` contains `version`, `description`, and `interface`. The existing graph
and interface protocol versions remain independent; both records must identify the same workflow.
The manifest excludes node configuration and supplied business values. Its combined UTF-8 JSON payload is limited to 16 MiB.
New Linux and macOS runners embed these bytes in `.mf_manifest` (ELF) or `__DATA,__mf_manifest` (Mach-O), respectively.
The section is retained through release optimization, LTO, and supported stripping, including telemetry-disabled builds.
Compatibility commands print the same graph/interface records; they do not refresh the frozen contract by preparing providers.

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 8 bytes | Magic `MFMANIF\0` |
| 8 | 4 bytes | Unsigned little-endian framing version, currently `1` |
| 12 | 8 bytes | Unsigned little-endian JSON payload length |
| 20 | Declared length | One complete JSON manifest |

Readers reject duplicate JSON members, unsupported versions, truncated or oversized records, invalid graph/interface
contracts, and trailing data other than at most 4 KiB of zero alignment padding. These bytes are a portable serialization,
not a Rust object layout. The existing workflow ID identifies the compiled definition rather than authenticating plugin code.

## Lifecycle fields

Use the instrumentation scope `mf.workflow`. Event names, timestamps, and optional trace/span context use native OTel fields. `WireRecord` is a transport-independent logical view of these fields for fixtures and adapters; its JSON representation is not an OTLP/HTTP request envelope.

| Attribute | Type | Meaning |
| --- | --- | --- |
| `mf.schema.version` | Signed integer | Event schema version: 1 for flat runs, 2 for finite Loop-capable runs, 4 for source-driven streams |
| `mf.workflow.id` | String | Identity shared with the description |
| `mf.run.id` | String | Identity of this invocation |
| `mf.event.sequence` | Signed integer | Positive per-run sequence starting at 1 |
| `mf.node.id`, `mf.node.kind` | Strings | Required on node events |
| `mf.outcome` | String | `succeeded`, `failed`, or `skipped` on terminal records |
| `mf.failure.phase` | String | Failure phase when failure context is present |

The body is an OTel structured map, not a JSON message string. All counts and monotonic nanosecond offsets use nonnegative signed 64-bit integers. Optional fields are omitted in emitted records. Native event timestamps use Unix nanoseconds; trace IDs and span IDs must be canonical, nonzero IDs when supplied. Trace flags can describe an unsampled span; lifecycle delivery must remain independent of span sampling.

| Event | Body and boundary |
| --- | --- |
| `mf.workflow.started` | `node_count`, `elapsed_ns: 0`; sequence 1, before preparation |
| `mf.node.started` | `position`, `elapsed_ns`; dependencies resolved, implementation about to run |
| `mf.node.finished` | `position`, `elapsed_ns`, available `duration_ns`, `produced_ports`, `skipped_ports`, optional `failure` map; Loop success also carries `loop_summary` with pass count and stop reason |
| `mf.node.skipped` | `position`, `elapsed_ns`, `causes` with `source_node`/`source_output`, and all skipped output names; no invocation |
| `mf.loop.pass.started` | Loop path and `elapsed_ns`; before each body traversal in schema 2 |
| `mf.loop.pass.finished` | Loop path, `elapsed_ns`, local `visited_node_count`, and `completed`, `exit`, or `failed` outcome in schema 2 |
| `mf.workflow.finished` | `final_sequence`, `elapsed_ns`, `visited_node_count`, optional `failure_node_id` and `failure`; schema 2 also carries `top_level_visited_count` |

`failure` contains a diagnostic `message`; its phase is the `mf.failure.phase` attribute. Phases are `preparation`, `dependency`, `execution`, `publication`, and `output_selection`. Pre-invocation failures have no execution duration. Failed nodes have no published port outcomes. A failed workflow requires failure context; successful outcomes cannot carry it.

In schema 1, `visited_node_count` is the prefix of scheduled steps reached, including a step failing
dependency resolution. In schema 2, it is the total number of scheduled steps across all scopes,
while `top_level_visited_count` retains the outer graph prefix. Each pass finish records its local
visited prefix. Preparation failure visits zero steps; output selection requires the outer graph to
finish. A proven unvisited suffix of an exited or failed pass is NotRun, while future passes that
never started have no node invocations. Missing outcomes inside a visited prefix stay unknown, even
when the workflow succeeded. The final record contains no per-node snapshot.

This logical fixture demonstrates successful node completion:

```json
{
  "scope": "mf.workflow",
  "event_name": "mf.node.finished",
  "time_unix_nano": 1700000000000000000,
  "attributes": {
    "mf.schema.version": 1,
    "mf.workflow.id": "sha256:67389167c589949c07cb3864dbb0bcca61d38bfaef42649c24042cc35875e2a9",
    "mf.run.id": "12345678-1234-4234-9234-123456789abc",
    "mf.event.sequence": 3,
    "mf.node.id": "load.order",
    "mf.node.kind": "fixture.source",
    "mf.outcome": "succeeded"
  },
  "body": {
    "position": 0,
    "elapsed_ns": 20,
    "duration_ns": 10,
    "produced_ports": [
      "value.part"
    ],
    "skipped_ports": []
  }
}
```

`WireRecord::decode` checks required fields and local event invariants. `LifecycleEvent::validate_for` additionally checks graph identity, event bounds, positions, kinds, ports, skip dependencies, and the visited prefix. Run identity matching and cross-record state reduction belong to the later receiver. `WireRecord::write_to` fills a fresh OTel record created by a caller-owned logger; it neither emits that record nor manages the provider.

## Sequences and acceptable loss

Reserve sequences before serialization/enqueueing through `EventSequence`. A dropped record consumes
its sequence. The final reservation closes the sequence permanently, and retransmitting a record
preserves its identity. Schema 1 permits at most `2 * node_count + 2` lifecycle records. Schema 2
permits at most `4 * 10,000 + 4`: two node records and two pass boundaries per scheduled step, with
room for one pass that fails before its first step after budget exhaustion. The sequence helper does
not execute nodes or enforce a scheduler.

Observation loss is acceptable. Consumers must expose gaps and local drops, and must not claim completeness without evidence:

- While active, collect events and show any pending gaps; late arrival can close a gap.
- After bounded draining, a valid final record and every applied sequence through it can establish lifecycle completeness.
- Remaining gaps or local lifecycle drops mean incomplete observation.
- A missing final record means an unverified tail and unknown total loss, including when no telemetry arrived at all.
- A wholly lost suffix during a long-running node cannot be detected immediately without later evidence. Keep completeness unconfirmed.

There is no replay, persistent journal, reconnect protocol, node rescheduling, or full-state recovery. Known missing sequences and local drop counts can overlap; do not sum them as distinct losses. Trace availability and diagnostic-history truncation are separate from lifecycle completeness. Missing telemetry must not alter workflow results. Missing business stdout is a separate CLI output error.

### Loop invocation identity and pass boundaries

Schema 2 node records include a `loop_path` list in the structured body. Each entry has an opaque
Loop node ID and a zero-based pass index. Top-level nodes have an empty path. The event's local node
ID, kind, and position resolve against the body graph named by that path. Nested paths keep scopes
distinct even when bodies reuse node IDs. `mf.loop.pass.started` and `mf.loop.pass.finished` carry
the full path. A pass finish records its visited body prefix and whether the pass completed, exited,
or failed. A successful Loop node finish carries `loop_summary` with the number of passes and a stop
reason of `condition`, `maximum`, or `exit`. No record carries loop variable values or node
configuration.

Schema 2's workflow final record reports the actual number of scheduled steps and the outer graph's visited prefix. The TUI compares the actual step count with observed pass prefixes, so a missing pass cannot look complete merely because the remaining records have contiguous sequence numbers. A known missing body outcome remains unknown after later passes complete; receiving that delayed record can close the gap. The old description and event versions remain supported for existing binaries.

## Iteration observation

An Iteration remains one node in its containing workflow lifecycle stream and graph description. Its ordinary
`mf.node` span and `mf.node.started` / `mf.node.finished` records bracket the complete array operation. Repeated body
invocations use a separate `mf.iteration` instrumentation scope so they do not consume the bounded outer lifecycle
sequence or appear as duplicate node outcomes in the terminal UI.

Each started item emits `mf.iteration.item.started` and `mf.iteration.item.finished` logs and an `mf.iteration.item`
span. Each reached user-defined body node emits `mf.iteration.node.started` and
`mf.iteration.node.finished` logs, or `mf.iteration.node.skipped` when a dependency is skipped, with an
`mf.iteration.node` span. Dependency failures can finish a node without a start record. All detail logs carry
`mf.workflow.id`, `mf.run.id`, `mf.iteration.id`, and zero-based `mf.iteration.index`; body-node records also carry
`mf.node.id` and `mf.node.kind`. Terminal records include an outcome, duration when available, and failure context.
They exclude item and result values. Plugin-provided failure messages can contain data supplied by that plugin.

When an Iteration runs inside a Loop body, item detail spans inherit that pass's Iteration invocation span. The item span is a child of the Iteration node span, and each body-node span is a child of its item span. The
item context is attached in its worker thread, so spans created by a body plugin during its node invocation inherit
that body-node context. Parallel completion order can differ from input order; use the item index and trace parentage
to group records. Under `continue_on_error` or `remove_failed`, failed items and body nodes retain failed detail
outcomes while the outer Iteration node can succeed. The terminal UI currently displays the outer node and does not
render these repeated body invocations.

## Runner export configuration

The generated runner initializes exporters only in execution mode when `OTEL_EXPORTER_OTLP_ENDPOINT`,
`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, or `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` is set to a nonempty URL. Signal-specific
endpoints override the generic URL; a missing signal endpoint without a generic URL leaves that signal local.
`OTEL_EXPORTER_OTLP_HEADERS` and signal-specific headers are interpreted by the official exporter. HTTP protobuf is
selected explicitly. When configured, both providers share a blocking TLS-capable HTTP client and use bounded background
processors. Their shutdown attempts to export remaining records within a finite deadline even when execution fails.

Build validation and direct manifest inspection do not initialize execution export or execute workflow nodes.
The CLI reads graph and interface metadata directly from the executable with bounded file reads.

## Local reception and state aggregation

`mf-tui::receiver::LoopbackReceiver::bind` opens an ephemeral `127.0.0.1` OTLP/HTTP endpoint for one described workflow
and run ID. It accepts protobuf logs at `/v1/logs` and spans at `/v1/traces`. Request bodies are limited to 8 MiB,
each signal request to 1,024 records, and active connections to 16. A request has a two-second read deadline. An HTTP
success response follows decoding, session checks, and state admission; it does not claim that later network hops or
the TUI display are complete. Unrelated runs are ignored. Malformed matching lifecycle records and receiver errors
remain visible as local drops or observation errors. Trace-only drops do not invalidate lifecycle completeness.

For finite runs, `mf-tui::state::SessionState` keeps one state record per outer node, sparse per-invocation Loop state, and sequence membership bounded by the graph's lifecycle event count. It retains details for up to 64 recent pass frames and aggregate counts when older pass details leave the view. Identical retransmissions are ignored; conflicting sequence content or
incompatible terminal outcomes are surfaced without moving a terminal node back to Running. A finish event can arrive
before its start, and later evidence may close an active sequence gap. Unknown node outcomes inside the final visited
prefix remain Unknown; a valid final boundary can prove that later nodes were NotRun.

Session admission limits the total described nodes across all scopes to 10,000 and individual lifecycle events to 128 KiB. State snapshots retain
at most 64 missing ranges and 64 diagnostic entries, each at most 1 KiB; omitted ranges and diagnostic bytes are counted.
Each node retains at most 32 produced and 32 skipped port names of 128 bytes each, 32 short skip causes, and 4 KiB of
failure text. Omitted display metadata is counted separately from lifecycle loss. The receiver rejects records that
exceed admission limits rather than allocating memory proportional to untrusted decoded content indefinitely.

Completeness stays Collecting while the run is active and gaps may still close. A final record with every sequence
applied, every visited node outcome justified, and no local lifecycle loss, observation error, or conflict is Complete. Remaining gaps or local lifecycle
drops after bounded collection are Incomplete. If the final record is absent, the tail is UnverifiedTail and its total
missing count is unknown, even when no telemetry arrived. Known missing ranges and local drop counts may overlap and
must be shown separately. Observed span count, trace drops, and diagnostic truncation are separate indicators; none
proves lifecycle loss. There is no replay, persistence, reconnect, retry scheduling, or workflow restart.

## Input and output history

Generated runners collect value history only when `MF_CAPTURE_SNAPSHOTS=1` and an OTLP logs
endpoint are configured. Finite TUI execution sets this flag and uses its existing loopback `/v1/logs`
endpoint. Ordinary execution does not allocate a snapshot recorder. Runners built with
`--no-telemetry` report that data history requires a telemetry-enabled build.

Both finite and streaming execution attach the same `SnapshotRecorder` notification handle.
It sends current `SnapshotEvent` values and retains no history or encoding state. Callers that
want dictionary encoding and immutable finite history own a `SnapshotRecording` and attach
`recording.recorder()`; its store and sink policy are independent of execution. Equal finite
states retain their existing deduplication, while stream identities and `Emitted` updates
preserve repeated equal-valued sends. Optional identity fields are omitted from finite wire
records, preserving the existing version 1 representation.

Snapshot events use the `mf.snapshot` instrumentation scope and the standard OTLP `LogRecord`
body. Each record includes `mf.workflow.id`, `mf.run.id`, `mf.snapshot.version` (currently 1),
`mf.snapshot.sequence` (starting at zero), and a SHA-256 `mf.snapshot.digest` byte array.
The digest uses the canonical typed-body encoding implemented by `mf-telemetry::snapshot`; map key order is ignored.
`mf.snapshot.record` carries a structured map containing a stream header, a value definition,
a node change, or an end marker. Values are defined once; arrays, objects, and node inputs/outputs
reference value IDs. Typed number definitions use decimal text to preserve unsigned 64-bit integers
and integer/float distinctions. The receiver reconstructs immutable roots with shared descendants.

Bodies larger than 32 KiB are encoded as a standard Protobuf `AnyValue` and split across
`mf.snapshot.fragment` events. Their structured bodies contain `index`, `total`, and `payload`
(bytes). The receiver verifies the assembled digest before admitting a record. Sequence numbers
make retransmissions idempotent and allow reordering; up to 1,024 incomplete or out-of-order records
may wait for a gap to close. Complete history and values are retained throughout the session.

Snapshot publication uses a dedicated log processor on the same OTLP endpoint, with 64-record
batches and a 128-record queue. The producer waits for a flush every 64 packets, preventing queue
overflow during bursts. Export failures disable further capture and produce a diagnostic without
changing the workflow result. A missing definition, conflicting retransmission, or incomplete tail
is shown as incomplete history; previously received snapshots remain available. Capture uses no
temporary files or file polling.

## Streaming data protocol

Generated streaming runners enable bounded value capture only when `MF_CAPTURE_STREAM_DATA=1`
and an OTLP logs exporter are available. TUI streaming execution requests this mode through its
child-only environment and clears `MF_CAPTURE_SNAPSHOTS`; finite TUI execution requests snapshots
and clears the stream data flag. Ordinary execution and local verbosity do not request values.
Recompile older schema-4 runners to gain data capture; they continue to run with their existing
lifecycle and raw detail support when the new flag is ignored.

Missing endpoints, observation setup failures, and `--no-telemetry` builds report capture
availability on stderr and preserve business results. Whole-stream `MF_CAPTURE_SNAPSHOTS=1`
still fails before startup. Selecting a finite Task from a streaming binary keeps the selected
operation's finite snapshot identity and ignores the whole-stream data flag.

Streaming business data uses a separate `mf.stream.data` instrumentation scope and
`mf.stream.data.record` LogRecords. Version 1 carries an independently decodable UTF-8 JSON
envelope in the standard OTLP `BytesValue` body. The envelope contains `version`, `workflow_id`,
`run_id`, unsigned-decimal `sequence`, `time_unix_nano`, optional native `trace_context`, and
`payload`. Routing attributes repeat the workflow, run, version, and capture sequence; native
trace/span fields must agree with the body when correlation is present.

Payload kinds are `start`, `ports`, `status`, and `end`. Port records distinguish inputs from
admitted outputs and retain the node/Loop path, stream invocation/trigger/input message, and
applicable output message and emission ordinal. A bounded port-name summary marks truncation
explicitly. Values use inline tagged null, bool, number, string, array, and object trees; number
text preserves unsigned 64-bit integers and integer/float distinctions. Each record can be
decoded without previous value definitions or contiguous arrival. An omitted value retains its
input/emission identity and a byte/depth reason. Status and end records carry cumulative
omission and sender-drop counts; their delivery does not establish a complete received history.

Complete encoded records are capped at 256 KiB, and value depth at 48 so typed wrappers remain
within the JSON decoder's recursion budget. The dedicated export worker retains at most 128
queued records and 2 MiB of accounted queue storage. Queue admission is nonblocking; pressure
drops data instead of adding business backpressure. The worker exports one bounded record per
request, below its 2 MiB request cap and the receiver's 8 MiB ceiling. Its HTTP and shutdown
operations use finite deadlines, and delivery diagnostics retain typed causes. Existing finite
snapshot version 1 and streaming lifecycle schema 4 retain their original contracts.

### Programmatic stream capture

Attach a `mf_runtime::SnapshotRecorder` through `StreamOptions.snapshots`, using the same
notification interface as finite execution. `SnapshotRecorder::new` accepts an unbound raw
callback and works without telemetry. `SnapshotRecorder::for_run` binds a callback to a run ID;
attaching that handle requires a matching `StreamOptions.observation` before startup.
Callbacks receive original immutable values and optional existing stream identities. They own
retention policy and must be nonblocking and must not unwind.

For OTLP inspection, the callback delegates to `mf_telemetry::stream_data::StreamDataRecorder`.
That adapter owns encoding budgets, capture sequences, loss accounting, and typed diagnostics.
The caller finishes capture after execution on both success and failure, then finishes export.
Custom callbacks own their retention policies and must be nonblocking and must not unwind.

```rust
let observation = plan.start_stream_observation(&providers.observer(), run_id)?;
let mut exporter = mf_telemetry::otlp::StreamDataExporter::from_env()?;
let sender = exporter.sender();
let capacity = sender.clone();
let capture = mf_telemetry::stream_data::StreamDataRecorder::new(
    observation.clone(),
    move |record| Ok(sender.try_emit(record)?),
    move || capacity.available(),
);
let encoded = capture.clone();
let recorder = mf_runtime::SnapshotRecorder::for_run(observation.run_id(), move |event| {
    if let Some(identity) = event.identity {
        let (direction, values) = match event.snapshot.outcome {
            mf_runtime::SnapshotOutcome::Started =>
                (mf_telemetry::stream_data::Direction::Input, &event.snapshot.inputs),
            mf_runtime::SnapshotOutcome::Succeeded | mf_runtime::SnapshotOutcome::Emitted =>
                (mf_telemetry::stream_data::Direction::Output, &event.snapshot.outputs),
            mf_runtime::SnapshotOutcome::Failed | mf_runtime::SnapshotOutcome::Skipped => return,
        };
        encoded.record(identity, direction, values);
    }
});
let result = (|| -> Result<(), Box<dyn std::error::Error>> {
    let instance = mf_compiler::start_stream(
        &plan,
        &registry,
        mf_runtime::StreamOptions {
            observation: Some(observation),
            snapshots: Some(recorder),
            arguments,
            ..Default::default()
        },
    )?;
    while let Some(output) = instance.recv()? {
        handle_output(output)?;
    }
    instance.join()?;
    Ok(())
})();
capture.finish();
if let Some(error) = capture.diagnostic() {
    eprintln!("stream data capture incomplete: {error}");
}
if let Err(error) = exporter.finish() {
    eprintln!("stream data export incomplete: {error}");
}
result?;
```

Inputs are captured after dependency/type validation and before invocation. Synchronous task
outputs are captured after publication validation. Producers capture each successful send while
their span is running; a later producer failure leaves those admitted records available. Event
callbacks record each admitted emission, including earlier emissions when a later publication or
timer update fails. Batch buffering captures inputs without inventing an output. Timer and close
outputs keep their actual trigger and output message identity without a fabricated input message.
Loop and Iteration body work uses its existing invocation and native span identities; virtual
scope input nodes have no independent invocation and are not captured as business operations.

The runtime associates each admitted emission with its business message identity and invokes
observation callbacks after releasing coordinator locks. Telemetry reserves capture sequences
in callback delivery order, captures native context, and encodes values within its own budgets.
Capture sequence order need not match concurrent business admission order; message identities
retain that ordering. The supplied recorder isolates sink errors and panics as typed diagnostics
without changing workflow results, and its sender uses nonblocking bounded queue admission.
Capture-enabled domains use dynamic execution when intermediate values would otherwise remain
inside an optimized typed segment. The runner finishes capture after workers have joined, then
finishes its exporter within the normal deadline. Historical lists belong to the TUI consumer.

## Graph presentation

`mf-tui::graph::GraphLayout` uses `rust-sugiyama` to assign layers and sibling order from the complete described graph, including isolated nodes. Data and control edges both contribute to the topology; parallel node pairs are deduplicated only for layout. A static terminal projection allocates attachment rows and expands boxes or column gaps to fit their routes. Node boxes and routes stay in place as observations arrive.

Node boxes start at four rows: two borders, a title, and a detail line. Compact attachments are spaced
four rows apart. One-to-one connections align input and output rows; fan/join connections stagger
their arrivals so channel crossings stay clear of turns. Boxes grow enough to keep all
attachments inside their borders. Sibling boxes have at least a two-row gap, rounded to keep their
port grids aligned, and disconnected components have an additional four-row gap. Producers are
routed before downstream nodes, with stable connection identities breaking ties.

If the compact layout leaves unresolved connections, planning may try six-row attachment spacing
once using the scope's remaining geometry, conflict, search, retry, and crossing-storage budgets.
The wider layout replaces the compact one only when it resolves more connections. Reported routing
work includes both attempts, and the selected geometry remains fixed during observation.

Routing work is bounded per described scope: 250,000 geometry items; 4,096 sparse search vertices and
16,384 implicit arcs per search; 8,192 expansions per search and 65,536 per scope; 2,000,000 conflict
checks; 16,384 retained crossings; 128 points per route; two local retry rounds; and a 2,000,000-cell
route coordinate limit. These limits govern optional route planning, independently of the existing
10,000-node description admission limit. Routing counters and deterministic fixtures are available
through `benchmark_routes`; performance reports remain local under `target/edge-routing/`.

`GraphView` draws data and control edges with distinct line symbols, clips to a caller-owned viewport, and renders node status and elapsed time from a state snapshot. Confirmed produced or skipped output ports change the associated edge style; possible lifecycle loss remains visibly uncertain.

Edges use ordered integer channel tracks and reserved passages for connections spanning multiple layers.
Obstacle-aware search and bounded local retries handle blocked routes. Completed routes avoid boxes,
labels, arrows, and shared collinear segments. Remaining perpendicular crossings have an explicit
upper/lower marker: the upper edge stays continuous and the lower edge has a blank cell on each side
of the crossing instead of end caps. Each edge retains its single or double line outside the gap.

Data edges pass above control edges; equal kinds use stable connection identities. Crossing layers
and glyphs do not change when observation colors change. Crossings indicate independent connections,
not junctions. Each marker reserves only the lower edge's three-cell gap: one column by three rows
when the upper edge is horizontal, or three columns by one row when it is vertical. Panning and
resizing clip the same planned geometry, including partially visible node text and Unicode labels. A wide text glyph that cannot fit completely at the viewport boundary is blanked so it cannot overwrite an adjacent pane.

If display limits prevent a safe route, the graph shows a separate unresolved-connections warning.
Select an incident node with Tab/j/k to inspect its endpoint IDs, port names, edge kind, and reason in
Details; PgUp/PgDn scroll those details. The connection remains in the graph metadata even though its
unsafe path is not drawn. This warning does not change workflow results or telemetry completeness.
Layout is calculated once per described scope, including Loop bodies; the CLI terminal loop owns
scope navigation, panning, status banners, diagnostics, and terminal cleanup.

## Local TUI execution

Compile a workflow executable, then launch it from an interactive terminal:

```sh
mf compile examples/if-else.json --output ./if-else
mf run ./if-else --tui
```

`mf run` requires terminal stdin and stderr. Stdout can be redirected: the CLI reserves it for the runner's byte-for-byte
output after the final view closes. New runners are inspected directly through their embedded ELF64 or Mach-O64
manifest without starting a process. Container metadata reads are limited to 1 MiB, with at most
4,096 sections/load commands and 1,024 bytes per section-name lookup; manifest payload and padding have separate bounds.
Executables missing that section must be recompiled. Unsupported containers, ambiguous sections, invalid ranges,
and corrupt records fail without launching an inspection process. Preflight validates matching workflow identities, startup parameters,
and declared resources before starting the receiver or execution process. Older finite descriptions remain supported;
older streaming descriptions require recompilation. A fresh run ID and loopback OTLP/HTTP receiver are prepared before launch. The
execution child receives these session settings; inherited `OTEL_EXPORTER_OTLP_*` settings, including remote
endpoints and headers, are removed from the child environment. The parent environment is unchanged.

Pass startup arguments directly as `--node.port VALUE` after the TUI options. The JSON transports
`--inputs '<JSON>'` and `--inputs-file <PATH>` accept flat qualified keys and cannot be combined with
named parameters. Inputs accept at most 1 MiB. The CLI validates parameters and forwards a canonical
copy through a private temporary file kept alive for the child.

TUI children receive null stdin so terminal input remains available for controls. Preflight rejects workflows
with an active stdin requirement. Supply a source file path through workflow parameters for TUI observation;
use the standalone executable for stdin ingestion. Resource metadata determines compatibility for all
registered node kinds.

```sh
mf run ./read-workflow --tui --read.path /data/events.txt
printf '1\n2\n' | ./stream-batch
```

The graph shows data and control edges, node status, elapsed time, and confirmed branch outcomes.
The header keeps the observed workflow outcome separate from the child process result. Arrow keys
pan; `f` resets the viewport; Tab, `j`, and `k` select a node for details. On a Loop node, `l` opens
its body graph; `h` or Esc returns to the parent graph. `[` and `]` inspect older and newer retained
pass frames. The detail pane shows active and completed pass counts, stop reason, and any hidden
older detail. Streaming details also show observed invocation/completion/result counts, Batch state,
message identity, unresolved outcomes, and final workflow totals. Loop pass headers include their owning
invocation so repeated paths in different messages remain distinct. `q` has no action while the workflow runs. Ctrl-C requests interruption, and a second
Ctrl-C or the two-second deadline forces termination. After the runner exits, the view stays open
until `q`, Enter, Esc at the root, or Ctrl-C. The terminal is restored before the captured stdout is
copied to CLI stdout.

Press `v` to open OTEL Tracing for finite and streaming runs. The span tree groups records by
trace and parent span IDs and orders siblings by start time. Children received before their parents
remain visible and move under them when they arrive; selection stays attached to the same span.
Spans appear after completion and export. Use `j`/`k` or Up/Down to select, Home for the first
entry, and End/`f` to follow the last entry in tree order. `v` or Esc returns to the graph.

Tab switches the selected span's right-hand pane:

- **Details** shows timing, status, IDs, attributes, span events, links, resource, and scope metadata.
- **Data** shows finite node inputs/outputs, outcome, skipped ports, errors, and snapshot capture
  diagnostics. Streaming Data lists independently captured inputs and admitted emissions for the
  selected span, ordered by capture sequence. Each row identifies direction, ports, trigger,
  invocation, applicable message, emission ordinal, timestamp, and a bounded value summary.
  Select a row to inspect its decoded values or byte/depth omission reason. Stream outcomes and
  skipped ports keep their lifecycle meanings in Details; a payload never proves successful work.
  Matching uses exact native trace/span IDs. Repeated node names never substitute for invocation identity.
- **Logs** shows independent LogRecords associated with both the selected trace and span IDs. Press
  `l` to focus that list and select a record for its timestamp, severity, body, attributes, and metadata.

Press `g` to switch between spans and session-wide LogRecords. Session logs remain available before
spans finish, when spans are unsampled, or when records lack a retained span. In session log mode,
Tab switches Logs and Data. For streaming sessions, Data lists all retained input/emission records,
including live producers whose spans have not finished or exported. Rows without native context
remain visible without an inferred span. Finite session Data still uses the selected correlated log
for its node snapshot. Span events remain part of Details rather than becoming LogRecords.

In span Data, press `l` to focus the record list; session Data already focuses its list. Use `j`/`k`
or Up/Down to select, Home for the oldest retained row, and End/`f` to resume tail following.
PgUp/PgDn page a focused Data list by ten records; otherwise they scroll the value preview.
Selection follows the capture sequence across late insertions. If a selected value is evicted,
Data reports its sequence and selects the nearest retained row. Scope changes reset following.
The value preview is capped at 64 KiB, preserves UTF-8, escapes terminal controls, and marks
preview truncation while leaving the admitted value in storage. Lists and capture diagnostics
refresh throughout collection and remain inspectable in the finished view.

Finite TUI children keep `MF_CAPTURE_SNAPSHOTS=1` and clear `MF_CAPTURE_STREAM_DATA`.
Streaming children receive `MF_CAPTURE_SNAPSHOTS=0` and `MF_CAPTURE_STREAM_DATA=1`, including
when inherited settings request whole-run snapshots. Details and Logs remain available when a
compatible older runner cannot provide stream data. The receiver retains at most 4,096 unique spans and 4,096 unique
LogRecords, with a separate 16 MiB encoded-data budget for each signal including resource/scope
metadata. Admitted raw detail remains available throughout the session; overflow is counted as
signal-specific display drops and does not stop lifecycle reduction or snapshot reconstruction.
Span/log previews are limited to 64 KiB and each input/output object preview retains its 64 KiB limit.
Capture errors preserve already decoded values and remain separate from raw signal retention loss.

The CLI keeps at most 256 MiB of stdout in a private temporary spool and 1 MiB of recent stderr. It drains both pipes
without waiting for a frame. If stdout capture fails or reaches its limit, the CLI stops the process group, displays the
incomplete output state, and delivers any recoverable prefix after leaving the TUI. A failed final stdout copy is
reported separately from the child result. Stderr history eviction is counted and shown. Missing OTel records never
change the runner's exit result: the footer distinguishes known sequence gaps, local drops, and an unverified tail when
the final lifecycle record is absent. Trace/log display drops and diagnostic truncation are separate indicators.

Run the local graph preview from an interactive terminal to see simulated node starts, completions, elapsed time, and an
inactive branch:

```bash
nix develop --command cargo run -p mf-tui --example graph_preview
```

Export static routing fixtures with `graph_preview -- --dump`, adding `--crowded` for mixed dense
connections or `--body` for the same fixture inside a Loop body. Inspect crossing orientations and line kinds with
`graph_preview -- --crossings`, adding `--color` for a status-color sample. These export modes run
without an interactive terminal.

Arrow keys pan the graph, `f` returns to the origin, and `q` or Esc closes the preview. The preview uses fixture events and does not launch a compiled runner.

See [compiling workflows](compiling.md) for executable commands, endpoint settings, lock migration, and build prerequisites. Automatically generated metadata excludes configuration and business values. Arbitrary plugin failure messages can contain sensitive text and are not automatically sanitized by this contract.

## Streaming observations

New streaming runner descriptions use version `2026-10-03`. Their workflow lifecycle records use event
schema `4`, decoded with `mf_telemetry::stream::StreamRecord`; the existing finite-run decoder keeps
its original schema `1` and `2` contracts. Session admission selects a reducer from the description
protocol and rejects incompatible lifecycle records. Older schema-3 streams require recompilation for TUI use.

A streaming instance has one workflow/run identity and a checked, monotonically increasing lifecycle
sequence. A run can exceed the finite-run event budget. The producer retains counters and shared graph
metadata, while export buffering remains bounded; it does not retain prior message histories. A sequence
is reserved before encoding or export, so failed delivery leaves a gap. A terminal event records its own
final sequence. Counter exhaustion stops trustworthy lifecycle emission without wrapping identifiers or
changing business execution; consumers cannot claim a complete terminal stream in that case.

Node and Loop records include a structured `stream` body with an invocation ID, trigger, optional
message identity, and optional parent invocation ID. Triggers are `startup` for startup-frame invocations, `message` for emitted-message task steps, and
`input`, `timer`, or `upstream_closed` for other callbacks. Startup, timer, and close invocations have no
external input message. Message identity combines a domain with that domain's sequence. Invocation IDs and
message sequences are canonical unsigned decimal strings so OTel encoding cannot round large values.
The lifecycle sequence retains the nonnegative signed OTel counter representation.

Initial producers use `startup`; message-driven producers use `input` for their invocation, including time waiting
for output capacity. The runtime carries the node span onto the producer worker. Successful completion
reports the number of admitted emissions and their produced ports; downstream messages have their own
identities and can run before production ends. Send validation failures report the `publication` phase,
while producer errors and panics report `execution`. Invalid sends fail the instance even when caught
by plugin code. Terminal observation waits for producer cleanup and output delivery.

Successful node completion includes `emission_count`. A Batch input can succeed with zero emissions;
that does not imply downstream execution or completion of the accepted input's business processing.
`mf.batch.buffered` reports the current item count. `mf.batch.flushed` reports the output message,
item count, and `size_exceed`, `timeout_exceed`, or `upstream_closed` reason. Automatic metadata includes
no collected values, node configuration, or unbounded list of constituent input IDs.

Node spans carry `mf.stream.invocation`, and applicable `mf.stream.domain`, `mf.stream.message`, and
`mf.stream.parent_invocation` attributes. Loop paths remain scoped to their containing message.
Iteration item/body logs retain their existing `mf.iteration` scope and outer-sequence independence,
and gain the containing stream identity. Body nodes receive distinct invocation IDs, including when
item indices or Loop paths repeat in a later message. Native span parentage remains workflow/node/item/body.

The final workflow outcome is `succeeded` or `failed`, with aggregate startup-frame,
emitted-message, completed-frame, and delivered-output counters. Startup traversal counts as one frame;
producer lifetimes remain active independently until their output domains close. Older schema-3 records
retain their accepted-input interpretation and are never relabeled as schema 4. Completion follows drain and output
acknowledgement, including the final stdout record. Failure preserves its phase and available node
identity. An earlier missing node outcome remains unknown when a later message succeeds. Consumers
should bound retained detail and distinguish local history eviction from lifecycle transport loss.

The stream reducer retains 4,096 sequence witnesses, up to 4,096 unresolved invocations, 64 recent
completed invocations, and 64 Loop pass details. Each retained pass has compact per-node status and up
to 64 detailed node records. Contiguous verified sequences and completed Loop prefixes compact into
counters. Losing old display detail does not invalidate lifecycle completeness. An ancient retransmission
outside the witness window is ignored and counted as unverified; its payload cannot be compared with
retired evidence. Exhausting an unresolved retention window permanently marks observation incomplete.
Repeated node invocations keep separate identities; node totals describe observed activity, while final
workflow totals come from the terminal record. Missing outcomes remain visible even after later successes.

For in-memory execution, create an observation with `CompiledWorkflow::start_stream_observation`, then
pass it in `StreamOptions.observation` to `mf_compiler::start_stream`. This includes preparation in the
observed lifetime. Providers remain caller-owned; use bounded, nonblocking processors and shut them down
after joining the instance. Generated runners configure the existing bounded OTLP/HTTP providers and
attempt bounded shutdown on success and failure. Disabled export, queue pressure, and unreachable
Collectors do not change workflow results or trigger retries.

Programmatic `StreamOptions.snapshots` uses the common notification handle. Bounded streaming
OTLP data is available through its telemetry adapter or `MF_CAPTURE_STREAM_DATA=1` as described
above. The legacy `MF_CAPTURE_SNAPSHOTS=1` runner flag selects the finite wire backend and remains
unsupported for whole-stream runner execution. Validation and description modes remain free
of execution and ignore capture/export setup. Existing finite-run history is unchanged.

## Isolated task observations

A selected task uses a finite operation description containing that root at position zero, no external
execution edges, and only its required Loop body descriptions. Its identity hashes the original workflow
identity, exact target ID, and isolation format. Different targets and whole-flow execution have distinct
identities; supplying different port or context values does not change a selected operation's identity.
The root span carries `mf.workflow.original_id` and `mf.node.target_id` for correlation with the containing
flow. Automatic descriptions and lifecycle metadata still exclude configuration and business values.

`NodeTarget::start_observation` starts this scope before preparation. Unrelated factory failures retain
preparation diagnostics without inventing a target or provider lifecycle. A successful ordinary selected
Task reports one visited top-level node; a selected Loop adds its actual body invocations and retains pass
paths. Iteration retains one outer lifecycle and its existing item detail records. Neighboring nodes are
absent from the selected description and do not receive synthetic skip or completion events.

Opt-in snapshots belong to the finite selected operation, including when its Task is selected from a
streaming binary. Snapshot export must use the selected description's identity. Whole-stream invocation
continues to reject whole-run snapshot capture while supporting the independent bounded data mode. Export and snapshot sink failures retain existing bounded
cleanup and diagnostics without changing business results or exit status.

### Streaming Data receiver retention

The dedicated `mf.stream.data` scope is decoded before raw-log preview truncation.
Its store is independent of lifecycle reduction, span retention, finite snapshot assembly,
and the 4,096-row raw-log budget. Each payload is independently decodable; later sequences
are immediately inspectable even when earlier packets or native spans are absent.
Only exact native trace/span IDs create span associations. Session browsing also retains
uncorrelated data, without inferring a span from a node name.

The Data store retains at most 4,096 payload rows and 16 MiB of accounted memory,
including decoded strings/collections, row indexes, 4,096 SHA-256 sequence witnesses,
and a 2 MiB reservation for the first typed decoding/conflict diagnostic. Oldest rows
are evicted; old sequence retransmissions cannot repopulate retired history. Public
snapshots share retained rows through `Arc`; consumers must release superseded snapshots.

Capture status separates value omissions, cumulative sender drops, missing transport
sequences, local payload evictions, and decoding/conflict faults. Sender drops and gaps
can describe the same loss and must not be summed. Sequence verification uses a contiguous
watermark plus bounded witnesses; retiring unresolved evidence changes gaps to unknown.
Missing start or end evidence never implies successful empty capture. A complete delivery
status describes received envelopes, while eviction and omission counters describe which
values remain inspectable. Data faults retain typed error sources and do not mark lifecycle
records as lost or stop later independent Data, log, trace, or finite snapshot admission.
