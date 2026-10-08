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

## Graph presentation

`mf-tui::graph::GraphLayout` uses `rust-sugiyama` to assign layers and sibling order from the complete described graph, including isolated nodes. Data and control edges both contribute to the topology; parallel node pairs are deduplicated only for layout. A fixed-size terminal projection keeps node boxes in place as observations arrive.

`GraphView` draws data and control edges with distinct line symbols, clips to a caller-owned viewport, and renders node status and elapsed time from a state snapshot. Confirmed produced or skipped output ports change the associated edge style; possible lifecycle loss remains visibly uncertain.

The initial routing is a midpoint orthogonal path between boxes. It does not search around intervening boxes, so crowded graphs can have line crossings or obscured segments. Layout is calculated once per described scope; the CLI terminal loop owns scope navigation, panning, status banners, diagnostics, and terminal cleanup.

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

Pass startup arguments with `--inputs '<JSON>'` or `--inputs-file <PATH>`. These options are mutually
exclusive and accept at most 1 MiB. The CLI reads a parameter file once, validates the nested node/port
map, and forwards a canonical copy through a private temporary file kept alive for the child.

TUI children receive null stdin so terminal input remains available for controls. Preflight rejects workflows
with an active stdin requirement. Supply a source file path through workflow parameters for TUI observation;
use the standalone executable for stdin ingestion. Resource metadata determines compatibility for all
registered node kinds.

```sh
mf run ./read-workflow --tui --inputs '{"read":{"path":"/data/events.txt"}}'
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

For finite runs, press `v` to browse recorded inputs and outputs, including Loop passes and Iteration items.
Streaming runs show an explicit unavailable message instead. The launcher sets `MF_CAPTURE_SNAPSHOTS=0`
for streams even when the parent environment enables capture; finite runs retain capture support.
Use `j`/`k` or Up/Down to select a change, Home for the first change, and End/`f` to follow the latest.
PgUp/PgDn scroll values; `v` or Esc returns to the graph. Previews are limited to 64 KiB per
input/output object, while the complete values remain in memory. The view copies only the visible
history entries and shares their immutable roots with the receiver.

The CLI keeps at most 256 MiB of stdout in a private temporary spool and 1 MiB of recent stderr. It drains both pipes
without waiting for a frame. If stdout capture fails or reaches its limit, the CLI stops the process group, displays the
incomplete output state, and delivers any recoverable prefix after leaving the TUI. A failed final stdout copy is
reported separately from the child result. Stderr history eviction is counted and shown. Missing OTel records never
change the runner's exit result: the footer distinguishes known sequence gaps, local drops, and an unverified tail when
the final lifecycle record is absent. Trace drops and diagnostic truncation are separate indicators.

Run the local graph preview from an interactive terminal to see simulated node starts, completions, elapsed time, and an
inactive branch:

```bash
nix develop --command cargo run -p mf-tui --example graph_preview
```

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

Whole-run snapshot capture is unavailable for streaming instances. `StreamOptions.snapshots` and
`MF_CAPTURE_SNAPSHOTS=1` are rejected before input admission or node execution. Validation and description
modes remain free of execution and ignore capture/export setup. Existing finite-run history is unchanged.
